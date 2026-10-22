use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct RefundResponse {
    pub refund_id: Uuid,
    pub payment_id: Uuid,
    pub amount_minor: i64,
    pub external_id: String,
    pub payment_status: String,
}
