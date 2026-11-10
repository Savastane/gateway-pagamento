use api_pagamento::{
    app::{self, AppState},
    config::Config,
    services::{gateway_client::GatewayCaller, gateway_router::GatewayRouter},
    store::MemStore,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use gateway_proto::payment_gateway::v1 as pb;
use secrecy::SecretString;
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;

struct FakeStore;
#[async_trait::async_trait]
impl GatewayCaller for FakeStore {
    async fn native_store(
        &self,
        req: pb::NativeStoreRequest,
    ) -> Result<pb::BillingResponse, tonic::Status> {
        if req.operation == "notification" {
            assert_eq!(req.authorization, "Bearer invalid-test-identity");
            return Err(tonic::Status::unauthenticated("invalid identity"));
        }
        assert_eq!(req.operation, "verify");
        assert_eq!(req.professional_id, "00000000-0000-4000-8000-000000000001");
        assert_eq!(req.provider, "google");
        Ok(pb::BillingResponse {
            json: r#"{"subscriptions":[]}"#.into(),
        })
    }
    async fn create(&self, _: pb::CreatePaymentRequest) -> Result<pb::Payment, tonic::Status> {
        unreachable!()
    }
    async fn get(&self, _: &str) -> Result<pb::Payment, tonic::Status> {
        unreachable!()
    }
    async fn refund(
        &self,
        _: pb::RefundPaymentRequest,
    ) -> Result<pb::RefundPaymentResponse, tonic::Status> {
        unreachable!()
    }
}

fn app(configured: bool) -> axum::Router {
    let providers: HashMap<String, Arc<dyn GatewayCaller>> = HashMap::from([(
        "native_stores".into(),
        Arc::new(FakeStore) as Arc<dyn GatewayCaller>,
    )]);
    let router = GatewayRouter::with_gateways(providers, "native_stores");
    let cfg = Config {
        http_addr: "127.0.0.1:0".parse().unwrap(),
        database_url: None,
        gateways: HashMap::new(),
        default_gateway: "native_stores".into(),
        internal_token: None,
        api_key: configured.then(|| SecretString::new("test-only-api-auth".into())),
    };
    app::router(AppState::new(Arc::new(MemStore::default()), router, &cfg))
}

fn verify(auth: bool, owner: &str) -> Request<Body> {
    let mut builder =
        Request::post("/v1/billing/stores/verify").header("content-type", "application/json");
    if auth {
        builder = builder.header("x-api-key", "test-only-api-auth");
    }
    builder
        .body(Body::from(
            serde_json::json!({
                "professional_id": owner, "provider": "google", "proof": "dummy-test-proof",
            })
            .to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn native_billing_requires_server_auth_and_a_real_owner_uuid() {
    let router = app(true);
    let owner = "00000000-0000-4000-8000-000000000001";
    assert_eq!(
        router
            .clone()
            .oneshot(verify(false, owner))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router
            .clone()
            .oneshot(verify(true, "invalid"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        router.oneshot(verify(true, owner)).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        app(false)
            .oneshot(verify(false, owner))
            .await
            .unwrap()
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn public_notification_forwards_oidc_and_never_accepts_unverified_push() {
    let request = Request::post("/webhooks/stores/google")
        .header("authorization", "Bearer invalid-test-identity")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        app(true).oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}
