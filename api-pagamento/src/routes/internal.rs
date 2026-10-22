use crate::app::AppState;
use crate::error::AppError;
use crate::models::status_name;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use secrecy::ExposeSecret;
use serde::Deserialize;
use subtle::ConstantTimeEq;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct GatewayEvent {
    pub gateway: String,
    pub provider_event_id: String,
    pub event_type: String,
    pub external_id: String,
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub provider_payload: serde_json::Value,
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "internal",
    responses((status = 200, description = "Servico vivo"))
)]
pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

#[utoipa::path(
    post,
    path = "/internal/gateway-events",
    tag = "internal",
    params(
        ("Authorization" = String, Header, description = "Bearer <INTERNAL_TOKEN>"),
    ),
    request_body = GatewayEvent,
    responses(
        (status = 200, description = "Evento avaliado (applied=false quando nao aplicavel)"),
        (status = 401, description = "Token interno invalido", body = crate::error::ProblemDetails),
    ),
    security(("internal_token" = [])),
)]
pub async fn gateway_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewayEvent>,
) -> Result<impl IntoResponse, AppError> {
    if let Some(expected) = &state.internal_token {
        let given = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .unwrap_or("");
        if given
            .as_bytes()
            .ct_eq(expected.expose_secret().as_bytes())
            .into()
        {
        } else {
            return Err(AppError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "token interno invalido",
            ));
        }
    }

    if event.external_id.is_empty() || event.status == 0 {
        return Ok((
            StatusCode::OK,
            Json(serde_json::json!({"applied": false, "reason": "evento sem status/external_id"})),
        ));
    }
    let to_status = status_name(event.status);
    let applied = state
        .store
        .apply_event(
            &event.gateway.to_lowercase(),
            &event.external_id,
            to_status,
            &event.provider_payload,
        )
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    match applied {
        Some(p) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "applied": true,
                "payment_id": p.id,
                "status": p.status,
            })),
        )),
        None => Ok((
            StatusCode::OK,
            Json(serde_json::json!({"applied": false, "reason": "pagamento desconhecido"})),
        )),
    }
}
