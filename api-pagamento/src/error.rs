use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

// RFC 7807 (application/problem+json)
#[derive(Debug, Serialize, ToSchema)]
pub struct ProblemDetails {
    pub r#type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
}

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl AppError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn gateway(status: &tonic::Status) -> Self {
        use tonic::Code::*;
        let http = match status.code() {
            InvalidArgument => StatusCode::BAD_REQUEST,
            NotFound => StatusCode::NOT_FOUND,
            AlreadyExists | Aborted | FailedPrecondition => StatusCode::CONFLICT,
            Unauthenticated | PermissionDenied => StatusCode::BAD_GATEWAY,
            Unavailable | DeadlineExceeded | Internal | Unknown => StatusCode::BAD_GATEWAY,
            Unimplemented => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::BAD_GATEWAY,
        };
        Self::new(http, "provider_error", status.message().to_string())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }
}

impl AppError {
    pub fn to_problem(&self) -> serde_json::Value {
        serde_json::to_value(ProblemDetails {
            r#type: format!("https://gateway.dev/errors/{}", self.code),
            title: self.code.to_string(),
            status: self.status.as_u16(),
            detail: self.message.clone(),
        })
        .unwrap_or_default()
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(self.to_problem())).into_response()
    }
}

impl From<tonic::Status> for AppError {
    fn from(s: tonic::Status) -> Self {
        Self::gateway(&s)
    }
}
