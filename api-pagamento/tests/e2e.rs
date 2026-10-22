use api_pagamento::app::{self, AppState};
use api_pagamento::config::Config;
use api_pagamento::services::gateway_client::GatewayCaller;
use api_pagamento::services::gateway_router::GatewayRouter;
use api_pagamento::store::MemStore;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use gateway_proto::payment_gateway::v1 as pb;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

struct FakeGateway;

#[async_trait::async_trait]
impl GatewayCaller for FakeGateway {
    async fn create(&self, _req: pb::CreatePaymentRequest) -> Result<pb::Payment, tonic::Status> {
        Ok(pb::Payment {
            external_id: "ext_fake_1".to_string(),
            status: pb::PaymentStatus::RequiresAction as i32,
            amount: Some(pb::Money {
                amount_minor: 12990,
                currency: "brl".to_string(),
            }),
            method: pb::PaymentMethod::Pix as i32,
            pix_details: Some(pb::PixDetails {
                emv_qr_code: "000201...EMV".to_string(),
                expires_at: 1_800_000_000,
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    async fn get(&self, _external_id: &str) -> Result<pb::Payment, tonic::Status> {
        Err(tonic::Status::unavailable("fake: sem consulta"))
    }

    async fn refund(
        &self,
        _req: pb::RefundPaymentRequest,
    ) -> Result<pb::RefundPaymentResponse, tonic::Status> {
        Ok(pb::RefundPaymentResponse {
            refund_id: "re_fake_1".to_string(),
            status: pb::PaymentStatus::Refunded as i32,
            amount: None,
        })
    }
}

fn app() -> axum::Router {
    let mut gateways: HashMap<String, Arc<dyn GatewayCaller>> = HashMap::new();
    gateways.insert("fake".to_string(), Arc::new(FakeGateway));
    let router = GatewayRouter::with_gateways(gateways, "fake");
    let store = Arc::new(MemStore::default());
    let cfg = Config {
        http_addr: "127.0.0.1:0".parse().unwrap(),
        database_url: None,
        gateways: HashMap::new(),
        default_gateway: "fake".to_string(),
        api_key: None,
        internal_token: None,
    };
    app::router(AppState::new(store, router, &cfg))
}

fn post(uri: &str, body: serde_json::Value, idem_key: Option<&str>) -> Request<Body> {
    let mut b = Request::post(uri)
        .header("content-type", "application/json")
        .header("x-merchant-id", "m1");
    if let Some(k) = idem_key {
        b = b.header("idempotency-key", k);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

fn pix_payment_body() -> serde_json::Value {
    serde_json::json!({
        "gateway": "fake",
        "amount_minor": 12990,
        "currency": "brl",
        "method": {"type": "pix", "expires_after_seconds": 3600},
        "customer": {"name": "Ana", "email": "a@b.c", "tax_id": "12345678909"}
    })
}

async fn json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
async fn create_replay_conflict_webhook_and_refund() {
    let app = app();

    // 1. create PIX
    let (s, body) = json(
        app.clone()
            .oneshot(post("/v1/payments", pix_payment_body(), Some("k-1")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "REQUIRES_ACTION");
    assert_eq!(body["external_id"], "ext_fake_1");
    assert_eq!(body["pix_details"]["emv_qr_code"], "000201...EMV");
    let payment_id = body["id"].as_str().unwrap().to_string();

    // 2. replay: mesma key + mesmo corpo => resposta identica
    let (s, replay) = json(
        app.clone()
            .oneshot(post("/v1/payments", pix_payment_body(), Some("k-1")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(replay, body);

    // 3. mesma key + corpo diferente => 409
    let mut other = pix_payment_body();
    other["amount_minor"] = serde_json::json!(1);
    let (s, _) = json(
        app.clone()
            .oneshot(post("/v1/payments", other, Some("k-1")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // 4. evento de webhook (status PAID=4) aplica transicao
    let (s, body) = json(
        app.clone()
            .oneshot(
                Request::post("/internal/gateway-events")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "gateway": "fake",
                            "provider_event_id": "evt_1",
                            "event_type": "payment.paid",
                            "external_id": "ext_fake_1",
                            "status": 4
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], true);

    // 5. GET reflete PAID
    let (s, body) = json(
        app.clone()
            .oneshot(
                Request::get(format!("/v1/payments/{payment_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "PAID");

    // 6. refund total
    let (s, body) = json(
        app.clone()
            .oneshot(post(
                &format!("/v1/payments/{payment_id}/refunds"),
                serde_json::json!({}),
                Some("k-ref-1"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["payment_status"], "REFUNDED");
    assert_eq!(body["external_id"], "re_fake_1");

    // 7. refund de novo => excede o pago (422)
    let (s, body) = json(
        app.clone()
            .oneshot(post(
                &format!("/v1/payments/{payment_id}/refunds"),
                serde_json::json!({"amount_minor": 100}),
                Some("k-ref-2"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // 8. sem Idempotency-Key => 400
    let (s, _) = json(
        app.oneshot(post("/v1/payments", pix_payment_body(), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
