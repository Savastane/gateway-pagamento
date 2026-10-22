use crate::config::Config;
use crate::docs;
use crate::error::AppError;
use crate::routes::{internal, payments, refunds};
use crate::services::gateway_router::GatewayRouter;
use crate::store::Store;
use crate::usecase::create_payment::usecase::CreatePayment;
use crate::usecase::get_payment::usecase::GetPayment;
use crate::usecase::refund_payment::usecase::RefundPayment;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use utoipa::OpenApi;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub create_payment: Arc<CreatePayment>,
    pub get_payment: Arc<GetPayment>,
    pub refund_payment: Arc<RefundPayment>,
    pub internal_token: Option<SecretString>,
    pub api_key: Option<SecretString>,
}

impl AppState {
    pub fn new(store: Arc<dyn Store>, router: GatewayRouter, cfg: &Config) -> Self {
        Self {
            create_payment: Arc::new(CreatePayment {
                store: store.clone(),
                router: router.clone(),
            }),
            get_payment: Arc::new(GetPayment {
                store: store.clone(),
                router: router.clone(),
            }),
            refund_payment: Arc::new(RefundPayment {
                store: store.clone(),
                router: router.clone(),
            }),
            store,
            internal_token: cfg.internal_token.clone(),
            api_key: cfg.api_key.clone(),
        }
    }
}

fn check_secret(header: &HeaderMap, name: &str, expected: &SecretString) -> bool {
    header
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.as_bytes()
                .ct_eq(expected.expose_secret().as_bytes())
                .into()
        })
        .unwrap_or(false)
}

async fn api_key_auth(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(key) = &state.api_key
        && !check_secret(req.headers(), "x-api-key", key)
    {
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "X-Api-Key invalida ou ausente",
        ));
    }
    Ok(next.run(req).await)
}

pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/payments", post(payments::create))
        .route("/payments/{id}", get(payments::get))
        .route("/payments/{id}/refunds", post(refunds::create))
        .route_layer(middleware::from_fn_with_state(state.clone(), api_key_auth));

    Router::new()
        .route("/health", get(internal::health))
        .route("/internal/gateway-events", post(internal::gateway_events))
        .merge(
            utoipa_swagger_ui::SwaggerUi::new("/swagger-ui")
                .url("/api-docs/openapi.json", docs::ApiDoc::openapi()),
        )
        .nest("/v1", v1)
        .with_state(state)
}
