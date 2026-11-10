use crate::{app::AppState, error::AppError};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use gateway_proto::payment_gateway::v1 as pb;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verify {
    pub professional_id: uuid::Uuid,
    pub provider: String,
    pub proof: String,
}

fn provider(value: &str) -> Result<(), AppError> {
    if !matches!(value, "apple" | "google") {
        return Err(AppError::invalid("provider must be apple or google"));
    }
    Ok(())
}

async fn execute(
    state: AppState,
    request: pb::NativeStoreRequest,
) -> Result<Json<Value>, AppError> {
    if state.api_key.is_none() {
        return Err(AppError::internal("API_KEY is required for native billing"));
    }
    let (_, service) = state.billing_router.resolve(Some("native_stores"))?;
    let response = service
        .native_store(request)
        .await
        .map_err(|e| match e.code() {
            tonic::Code::Unauthenticated => AppError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_identity",
                "Invalid store identity",
            ),
            tonic::Code::PermissionDenied => AppError::new(
                StatusCode::FORBIDDEN,
                "invalid_owner",
                "Purchase belongs to another account",
            ),
            _ => AppError::gateway(&e),
        })?;
    Ok(Json(serde_json::from_str(&response.json).map_err(
        |_| AppError::internal("Invalid native billing response"),
    )?))
}

pub async fn verify(
    State(state): State<AppState>,
    Json(body): Json<Verify>,
) -> Result<Json<Value>, AppError> {
    provider(&body.provider)?;
    if body.proof.is_empty() || body.proof.len() > 32768 {
        return Err(AppError::invalid("Invalid purchase proof"));
    }
    execute(
        state,
        pb::NativeStoreRequest {
            operation: "verify".into(),
            provider: body.provider,
            professional_id: body.professional_id.to_string(),
            proof: body.proof,
            ..Default::default()
        },
    )
    .await
}

pub async fn snapshot(
    State(state): State<AppState>,
    Path(owner): Path<uuid::Uuid>,
) -> Result<Json<Value>, AppError> {
    execute(
        state,
        pb::NativeStoreRequest {
            operation: "snapshot".into(),
            professional_id: owner.to_string(),
            ..Default::default()
        },
    )
    .await
}

pub async fn catalog(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    execute(
        state,
        pb::NativeStoreRequest {
            operation: "catalog".into(),
            ..Default::default()
        },
    )
    .await
}

pub async fn ready(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, AppError> {
    provider(&name)?;
    execute(
        state,
        pb::NativeStoreRequest {
            operation: "ready".into(),
            provider: name,
            ..Default::default()
        },
    )
    .await
}

pub async fn notification(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    provider(&name)?;
    if body.len() > 262144 {
        return Err(AppError::invalid("Notification too large"));
    }
    execute(
        state,
        pb::NativeStoreRequest {
            operation: "notification".into(),
            provider: name,
            proof: String::from_utf8(body.to_vec())
                .map_err(|_| AppError::invalid("Invalid notification encoding"))?,
            authorization: headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .into(),
            ..Default::default()
        },
    )
    .await
}
