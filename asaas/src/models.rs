// Fields exist to mirror the provider payload; not all are read by every RPC.
#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ErrorBody {
    #[serde(default)]
    pub errors: Vec<ErrorItem>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorItem {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct Customer {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct CustomerList {
    #[serde(default)]
    pub data: Vec<Customer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payment {
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub billing_type: Option<String>,
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub external_reference: Option<String>,
    #[serde(default)]
    pub date_created: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixQrCode {
    #[serde(default)]
    pub encoded_image: String,
    #[serde(default)]
    pub payload: String,
    #[serde(default)]
    pub expiration_date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEvent {
    #[serde(default)]
    pub id: Option<String>,
    pub event: String,
    #[serde(default)]
    pub date_created: Option<String>,
    #[serde(default)]
    pub payment: Option<WebhookPayment>,
}

#[derive(Debug, Deserialize)]
pub struct WebhookPayment {
    pub id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}
