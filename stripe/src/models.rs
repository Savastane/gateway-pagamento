use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct StripeErrorBody {
    pub error: StripeErrorDetail,
}

#[derive(Debug, Deserialize)]
pub struct StripeErrorDetail {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub decline_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PaymentIntent {
    pub id: String,
    pub status: String,
    pub amount: i64,
    pub currency: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub next_action: Option<NextAction>,
    #[serde(default)]
    pub payment_method: Option<serde_json::Value>,
    #[serde(default)]
    pub payment_method_types: Vec<String>,
    #[serde(default)]
    pub created: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct NextAction {
    #[serde(default)]
    pub pix_display_qr_code: Option<PixQrCode>,
}

#[derive(Debug, Deserialize)]
pub struct PixQrCode {
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub image_url_png: Option<String>,
    #[serde(default)]
    pub image_url_svg: Option<String>,
    #[serde(default)]
    pub hosted_instructions_url: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct Refund {
    pub id: String,
    pub amount: i64,
    pub currency: String,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Event {
    pub id: String,
    pub r#type: String,
    pub data: EventData,
}

#[derive(Debug, Deserialize)]
pub struct EventData {
    pub object: serde_json::Value,
}
