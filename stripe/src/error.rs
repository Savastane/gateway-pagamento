#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error(
        "stripe api error (http {status}, type={error_type}, code={code}, decline={decline_code}): {message}"
    )]
    Api {
        status: u16,
        error_type: String,
        code: String,
        decline_code: String,
        message: String,
    },
    #[error("stripe request timeout or transport failure: {0}")]
    Transport(String),
    #[error("malformed stripe response: {0}")]
    Malformed(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("webhook signature verification failed")]
    InvalidSignature,
    #[error("webhook timestamp outside tolerance")]
    StaleTimestamp,
    #[error("webhook secret not configured")]
    WebhookNotConfigured,
}

impl From<GatewayError> for tonic::Status {
    fn from(err: GatewayError) -> Self {
        use GatewayError::*;
        match err {
            Api { status, .. } => {
                let msg = err.to_string();
                match status {
                    400 | 422 => tonic::Status::invalid_argument(msg),
                    401 | 403 => tonic::Status::internal(msg),
                    402 => tonic::Status::failed_precondition(msg),
                    404 => tonic::Status::not_found(msg),
                    409 => tonic::Status::aborted(msg),
                    429 => tonic::Status::unavailable(msg),
                    s if s >= 500 => tonic::Status::unavailable(msg),
                    _ => tonic::Status::unknown(msg),
                }
            }
            Transport(msg) => tonic::Status::unavailable(msg),
            Malformed(msg) => tonic::Status::internal(msg),
            InvalidRequest(msg) => tonic::Status::invalid_argument(msg),
            InvalidSignature | StaleTimestamp => tonic::Status::unauthenticated(err.to_string()),
            WebhookNotConfigured => tonic::Status::failed_precondition(err.to_string()),
        }
    }
}
