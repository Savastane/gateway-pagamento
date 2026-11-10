use crate::{app::AppState, error::AppError};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use gateway_proto::payment_gateway::v1 as pb;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    reference: String,
    professional_id: String,
    plan: String,
    email: String,
    success_url: String,
    cancel_url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Portal {
    customer: String,
    return_url: String,
}

fn key(headers: &HeaderMap) -> Result<String, AppError> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 255)
        .map(str::to_owned)
        .ok_or_else(|| AppError::invalid("Idempotency-Key is required"))
}

async fn execute(state: AppState, request: pb::BillingRequest) -> Result<Json<Value>, AppError> {
    // Billing cannot run without server-to-server authentication, even in development.
    if state.api_key.is_none() {
        return Err(AppError::internal("API_KEY is required for billing"));
    }
    let (_, gateway) = state.billing_router.resolve(Some("stripe"))?;
    let result = gateway.billing(request).await?;
    Ok(Json(serde_json::from_str(&result.json).map_err(|_| {
        AppError::internal("invalid billing response")
    })?))
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Checkout>,
) -> Result<Json<Value>, AppError> {
    execute(
        state,
        pb::BillingRequest {
            operation: pb::BillingOperation::CreateCheckout as i32,
            idempotency_key: key(&headers)?,
            reference: body.reference,
            professional_id: body.professional_id,
            plan: body.plan,
            email: body.email,
            success_url: body.success_url,
            cancel_url: body.cancel_url,
            ..Default::default()
        },
    )
    .await
}

pub async fn get_checkout(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    execute(
        state,
        pb::BillingRequest {
            operation: pb::BillingOperation::GetCheckout as i32,
            external_id: id,
            ..Default::default()
        },
    )
    .await
}

pub async fn get_subscription(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    execute(
        state,
        pb::BillingRequest {
            operation: pb::BillingOperation::GetSubscription as i32,
            external_id: id,
            ..Default::default()
        },
    )
    .await
}

pub async fn portal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Portal>,
) -> Result<Json<Value>, AppError> {
    execute(
        state,
        pb::BillingRequest {
            operation: pb::BillingOperation::CreatePortal as i32,
            idempotency_key: key(&headers)?,
            external_id: body.customer,
            success_url: body.return_url,
            ..Default::default()
        },
    )
    .await
}
