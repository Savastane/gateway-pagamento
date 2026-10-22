use gateway_proto::payment_gateway::v1 as pb;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const PENDING: &str = "PENDING";
pub const REQUIRES_ACTION: &str = "REQUIRES_ACTION";
pub const AUTHORIZED: &str = "AUTHORIZED";
pub const PAID: &str = "PAID";
pub const FAILED: &str = "FAILED";
pub const CANCELED: &str = "CANCELED";
pub const EXPIRED: &str = "EXPIRED";
pub const REFUNDED: &str = "REFUNDED";
pub const PARTIALLY_REFUNDED: &str = "PARTIALLY_REFUNDED";

pub fn status_name(code: i32) -> &'static str {
    match pb::PaymentStatus::try_from(code).unwrap_or(pb::PaymentStatus::Unspecified) {
        pb::PaymentStatus::Pending => PENDING,
        pb::PaymentStatus::RequiresAction => REQUIRES_ACTION,
        pb::PaymentStatus::Authorized => AUTHORIZED,
        pb::PaymentStatus::Paid => PAID,
        pb::PaymentStatus::Failed => FAILED,
        pb::PaymentStatus::Canceled => CANCELED,
        pb::PaymentStatus::Expired => EXPIRED,
        pb::PaymentStatus::Refunded => REFUNDED,
        pb::PaymentStatus::PartiallyRefunded => PARTIALLY_REFUNDED,
        pb::PaymentStatus::Unspecified => PENDING,
    }
}

// Maquina de estados: transicoes validas entre status normalizados.
// Estados terminais (FAILED, CANCELED, EXPIRED, REFUNDED) nao regridem.
pub fn can_transition(from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    matches!(
        (from, to),
        (
            PENDING,
            REQUIRES_ACTION | AUTHORIZED | PAID | FAILED | CANCELED | EXPIRED
        ) | (REQUIRES_ACTION, PAID | FAILED | CANCELED | EXPIRED)
            | (AUTHORIZED, PAID | FAILED | CANCELED)
            | (PAID, PARTIALLY_REFUNDED | REFUNDED | FAILED)
            | (PARTIALLY_REFUNDED, REFUNDED)
    )
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PaymentRow {
    pub id: uuid::Uuid,
    pub merchant_id: String,
    pub gateway: String,
    pub method: String,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub external_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub metadata: serde_json::Value,
    pub version: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct IdemRecord {
    pub request_hash: String,
    pub response: Option<serde_json::Value>,
    pub status_code: Option<i32>,
}

#[derive(Debug)]
pub enum IdemBegin {
    New,
    Replay(serde_json::Value, i32),
    InFlight,
    Conflict,
}

#[derive(Debug)]
pub enum ReserveOutcome {
    Reserved { refund_id: uuid::Uuid, amount: i64 },
    NotFound,
    ExceedsPaid { remaining: i64 },
    InvalidState { status: String },
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PixDetailsDto {
    #[serde(default)]
    pub emv_qr_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image_url_png: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hosted_instructions_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub qr_code_png_base64: String,
    #[serde(default)]
    pub expires_at: i64,
}

impl From<&pb::PixDetails> for PixDetailsDto {
    fn from(d: &pb::PixDetails) -> Self {
        Self {
            emv_qr_code: d.emv_qr_code.clone(),
            image_url_png: d.image_url_png.clone(),
            hosted_instructions_url: d.hosted_instructions_url.clone(),
            qr_code_png_base64: d.qr_code_png_base64.clone(),
            expires_at: d.expires_at,
        }
    }
}

pub fn request_hash(body: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_follow_state_machine() {
        assert!(can_transition(PENDING, REQUIRES_ACTION));
        assert!(can_transition(REQUIRES_ACTION, PAID));
        assert!(can_transition(PAID, PARTIALLY_REFUNDED));
        assert!(can_transition(PARTIALLY_REFUNDED, REFUNDED));
        assert!(can_transition(PAID, FAILED)); // chargeback
        assert!(!can_transition(REFUNDED, PAID));
        assert!(!can_transition(PAID, PENDING));
        assert!(!can_transition(CANCELED, PAID));
        assert!(!can_transition(FAILED, PAID));
        assert!(can_transition(PAID, PAID)); // no-op
    }
}
