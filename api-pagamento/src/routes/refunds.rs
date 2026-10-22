use super::payments::idem_key_and_merchant;
use crate::app::AppState;
use crate::error::AppError;
use crate::models::request_hash;
use crate::usecase::refund_payment::request::RefundPaymentRequest;
use crate::usecase::refund_payment::response::RefundResponse;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use uuid::Uuid;

#[utoipa::path(
    post,
    path = "/v1/payments/{id}/refunds",
    tag = "refunds",
    params(
        ("id" = Uuid, Path, description = "ID interno do pagamento"),
        ("Idempotency-Key" = String, Header, description = "Chave de idempotencia do estorno"),
        ("X-Api-Key" = Option<String>, Header, description = "Chave de API"),
    ),
    request_body = RefundPaymentRequest,
    responses(
        (status = 200, description = "Estorno aplicado", body = RefundResponse),
        (status = 404, description = "Pagamento nao encontrado", body = crate::error::ProblemDetails),
        (status = 409, description = "Idempotency-Key em conflito", body = crate::error::ProblemDetails),
        (status = 422, description = "Estorno impossivel ou excede o valor pago", body = crate::error::ProblemDetails),
    ),
    security(("api_key" = [])),
)]
pub async fn create(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let (key, merchant) = idem_key_and_merchant(&headers)?;
    let req: RefundPaymentRequest = if body.is_empty() {
        RefundPaymentRequest { amount_minor: 0 }
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::invalid(format!("corpo JSON invalido: {e}")))?
    };

    let (status, resp_body) = state
        .refund_payment
        .execute(id, req, &request_hash(&body), &key, &merchant)
        .await?;
    Ok((
        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
        Json(resp_body),
    ))
}
