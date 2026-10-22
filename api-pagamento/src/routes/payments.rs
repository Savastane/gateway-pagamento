use crate::app::AppState;
use crate::error::AppError;
use crate::models::request_hash;
use crate::usecase::create_payment::request::CreatePaymentRequest;
use crate::usecase::create_payment::response::PaymentResponse;
use crate::usecase::get_payment::request::GetPaymentRequest;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use uuid::Uuid;

pub fn idem_key_and_merchant(headers: &HeaderMap) -> Result<(String, String), AppError> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 255)
        .ok_or_else(|| AppError::invalid("header Idempotency-Key obrigatorio (max 255)"))?;
    let merchant = headers
        .get("x-merchant-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    Ok((key.to_string(), merchant.to_string()))
}

#[utoipa::path(
    post,
    path = "/v1/payments",
    tag = "payments",
    params(
        ("Idempotency-Key" = String, Header, description = "Chave de idempotencia do request"),
        ("X-Merchant-Id" = Option<String>, Header, description = "Identificador do merchant (default: 'default')"),
        ("X-Api-Key" = Option<String>, Header, description = "Chave de API (obrigatoria em producao)"),
    ),
    request_body = CreatePaymentRequest,
    responses(
        (status = 200, description = "Pagamento criado", body = PaymentResponse),
        (status = 400, description = "Request invalido", body = crate::error::ProblemDetails),
        (status = 409, description = "Idempotency-Key em conflito ou em voo", body = crate::error::ProblemDetails),
        (status = 422, description = "Gateway desconhecido", body = crate::error::ProblemDetails),
        (status = 502, description = "Erro no provedor", body = crate::error::ProblemDetails),
    ),
    security(("api_key" = [])),
)]
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let (key, merchant) = idem_key_and_merchant(&headers)?;
    let req: CreatePaymentRequest = serde_json::from_slice(&body)
        .map_err(|e| AppError::invalid(format!("corpo JSON invalido: {e}")))?;

    let (status, resp_body) = state
        .create_payment
        .execute(req, &request_hash(&body), &key, &merchant)
        .await?;
    Ok((
        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
        Json(resp_body),
    ))
}

#[utoipa::path(
    get,
    path = "/v1/payments/{id}",
    tag = "payments",
    params(
        ("id" = Uuid, Path, description = "ID interno do pagamento"),
        ("X-Api-Key" = Option<String>, Header, description = "Chave de API"),
    ),
    responses(
        (status = 200, description = "Pagamento encontrado (status sincronizado com o provedor)", body = PaymentResponse),
        (status = 404, description = "Pagamento nao encontrado", body = crate::error::ProblemDetails),
    ),
    security(("api_key" = [])),
)]
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let resp = state.get_payment.execute(GetPaymentRequest { id }).await?;
    Ok(Json(resp))
}
