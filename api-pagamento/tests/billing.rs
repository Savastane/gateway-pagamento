use api_pagamento::{
    app::{self, AppState},
    config::Config,
    services::{gateway_client::GatewayCaller, gateway_router::GatewayRouter},
    store::MemStore,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use gateway_proto::payment_gateway::v1 as pb;
use secrecy::SecretString;
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;

struct FakeBilling;
#[async_trait::async_trait]
impl GatewayCaller for FakeBilling {
    async fn billing(&self, req: pb::BillingRequest) -> Result<pb::BillingResponse, tonic::Status> {
        assert_eq!(req.operation, pb::BillingOperation::CreateCheckout as i32);
        assert_eq!(req.idempotency_key, "attempt-1");
        assert_eq!(req.reference, "local-1");
        Ok(pb::BillingResponse {
            json: r#"{"id":"cs_test_1","url":"https://checkout.stripe.com/test"}"#.into(),
        })
    }
    async fn create(&self, _: pb::CreatePaymentRequest) -> Result<pb::Payment, tonic::Status> {
        unimplemented!()
    }
    async fn get(&self, _: &str) -> Result<pb::Payment, tonic::Status> {
        unimplemented!()
    }
    async fn refund(
        &self,
        _: pb::RefundPaymentRequest,
    ) -> Result<pb::RefundPaymentResponse, tonic::Status> {
        unimplemented!()
    }
}

fn router(configured: bool) -> axum::Router {
    let mut providers: HashMap<String, Arc<dyn GatewayCaller>> = HashMap::new();
    providers.insert("stripe".into(), Arc::new(FakeBilling));
    let gateways = GatewayRouter::with_gateways(providers, "stripe");
    let cfg = Config {
        http_addr: "127.0.0.1:0".parse().unwrap(),
        database_url: None,
        gateways: HashMap::new(),
        default_gateway: "stripe".into(),
        internal_token: None,
        api_key: configured.then(|| SecretString::new("local-key".into())),
    };
    app::router(AppState::new(Arc::new(MemStore::default()), gateways, &cfg))
}

fn request(auth: bool, idem: bool, raw_card: bool) -> Request<Body> {
    let mut body = serde_json::json!({
        "reference":"local-1", "professional_id":"pro-1", "email":"p@example.com", "plan":"MENSAL",
        "success_url":"https://example.com/ok", "cancel_url":"https://example.com/cancel"
    });
    if raw_card {
        body["cvv"] = "must-not-be-accepted".into();
    }
    let mut req =
        Request::post("/v1/billing/checkout-sessions").header("content-type", "application/json");
    if auth {
        req = req.header("x-api-key", "local-key");
    }
    if idem {
        req = req.header("idempotency-key", "attempt-1");
    }
    req.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn billing_requires_credentials_idempotency_and_card_free_body() {
    let app = router(true);
    assert_eq!(
        app.clone()
            .oneshot(request(false, true, false))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone()
            .oneshot(request(true, false, false))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.clone()
            .oneshot(request(true, true, true))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let result = app.oneshot(request(true, true, false)).await.unwrap();
    assert_eq!(result.status(), StatusCode::OK);
    let bytes = to_bytes(result.into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("cs_test_1"));
}

#[tokio::test]
async fn billing_is_disabled_without_api_key_even_in_dev() {
    let response = router(false)
        .oneshot(request(false, true, false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
