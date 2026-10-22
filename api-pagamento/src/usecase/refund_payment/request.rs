use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct RefundPaymentRequest {
    /// Centavos; 0 ou ausente = estorno total do restante.
    #[serde(default)]
    pub amount_minor: i64,
}
