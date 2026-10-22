#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("asaas api error (http {status}): {message}")]
    Api { status: u16, message: String },
    #[error("asaas request timeout or transport failure: {0}")]
    Transport(String),
    #[error("malformed asaas response: {0}")]
    Malformed(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("webhook signature rejected: {0}")]
    Webhook(String),
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
            Webhook(msg) => tonic::Status::permission_denied(msg),
        }
    }
}
