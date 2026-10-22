use crate::models::{PaymentRow, PixDetailsDto};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct PaymentResponse {
    pub id: Uuid,
    pub gateway: String,
    pub method: String,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub external_id: Option<String>,
    pub pix_details: Option<PixDetailsDto>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl PaymentResponse {
    pub fn from_row(row: &PaymentRow) -> Self {
        let pix_details = row
            .metadata
            .get("pix_details")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());
        Self {
            id: row.id,
            gateway: row.gateway.clone(),
            method: row.method.clone(),
            amount_minor: row.amount_minor,
            currency: row.currency.clone(),
            status: row.status.clone(),
            external_id: row.external_id.clone(),
            pix_details,
            created_at: row.created_at,
        }
    }
}
