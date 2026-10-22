// Fields exist to mirror the provider payload; not all are read by every RPC.
#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ErrorBody {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub cause: Vec<ErrorCause>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorCause {
    #[serde(default)]
    pub code: Option<serde_json::Value>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Payment {
    pub id: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub status_detail: Option<String>,
    #[serde(default)]
    pub payment_method_id: Option<String>,
    #[serde(default)]
    pub transaction_amount: Option<f64>,
    #[serde(default)]
    pub date_created: Option<String>,
    #[serde(default)]
    pub external_reference: Option<String>,
    #[serde(default)]
    pub point_of_interaction: Option<PointOfInteraction>,
}

#[derive(Debug, Deserialize)]
pub struct PointOfInteraction {
    #[serde(default)]
    pub transaction_data: Option<TransactionData>,
}

#[derive(Debug, Deserialize)]
pub struct TransactionData {
    #[serde(default)]
    pub qr_code: Option<String>,
    #[serde(default)]
    pub qr_code_base64: Option<String>,
    #[serde(default)]
    pub ticket_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Refund {
    pub id: i64,
    #[serde(default)]
    pub payment_id: i64,
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WebhookEvent {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub data: Option<WebhookData>,
}

#[derive(Debug, Deserialize)]
pub struct WebhookData {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
}
