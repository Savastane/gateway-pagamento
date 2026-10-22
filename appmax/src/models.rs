// Fields exist to mirror the provider payload; not all are read by every RPC.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: u64,
}

#[derive(Debug, Deserialize)]
pub struct ErrorBody {
    #[serde(default)]
    pub error: Option<ErrorDetail>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorDetail {
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Data<T> {
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct CustomerData {
    pub customer: CustomerObj,
}

#[derive(Debug, Deserialize)]
pub struct CustomerObj {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
pub struct OrderData {
    pub order: OrderObj,
}

#[derive(Debug, Deserialize)]
pub struct OrderObj {
    pub id: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub total_paid: Option<i64>,
    #[serde(default)]
    pub amounts: Option<Amounts>,
}

#[derive(Debug, Deserialize)]
pub struct Amounts {
    #[serde(default)]
    pub sub_total: i64,
}

#[derive(Debug, Deserialize)]
pub struct PaymentResponse {
    #[serde(default)]
    pub order: Option<OrderObj>,
    #[serde(default)]
    pub payment: Option<PaymentObj>,
    #[serde(default)]
    pub pix: Option<PixObj>,
}

#[derive(Debug, Deserialize)]
pub struct PaymentObj {
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub paid_at: Option<String>,
    #[serde(default)]
    pub pay_reference: Option<String>,
    #[serde(default)]
    pub pix_qrcode: Option<String>,
    #[serde(default)]
    pub pix_emv: Option<String>,
    #[serde(default)]
    pub pix_expiration_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PixObj {
    #[serde(default)]
    pub qr_code: Option<String>,
    #[serde(default)]
    pub emv_code: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OrderDetail {
    pub order: OrderObj,
    #[serde(default)]
    pub payment: Option<PaymentObj>,
}

#[derive(Debug, Deserialize)]
pub struct RefundData {
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WebhookEvent {
    pub event: String,
    #[serde(default)]
    pub event_type: String,
    #[serde(default)]
    pub data: Option<WebhookData>,
}

#[derive(Debug, Deserialize)]
pub struct WebhookData {
    #[serde(default)]
    pub order: Option<OrderObj>,
    #[serde(default)]
    pub customer: Option<serde_json::Value>,
    #[serde(default)]
    pub payment: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct NewCustomer<'a> {
    pub first_name: &'a str,
    pub last_name: &'a str,
    pub email: &'a str,
    pub phone: &'a str,
    pub ip: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_number: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct NewOrder<'a> {
    pub customer_id: i64,
    pub products: Vec<NewProduct<'a>>,
    pub products_value: i64,
}

#[derive(Debug, Serialize)]
pub struct NewProduct<'a> {
    pub sku: &'a str,
    pub name: &'a str,
    pub quantity: i32,
    pub unit_value: i64,
    #[serde(rename = "type")]
    pub kind: &'a str,
}
