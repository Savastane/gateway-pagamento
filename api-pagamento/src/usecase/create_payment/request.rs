use serde::Deserialize;
use std::collections::HashMap;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePaymentRequest {
    /// Gateway alvo (stripe|appmax|asaas|mercadopago). Omitido = DEFAULT_GATEWAY.
    pub gateway: Option<String>,
    /// Valor em centavos (minor units). Nunca float.
    pub amount_minor: i64,
    /// ISO 4217 minuscula (ex.: "brl").
    pub currency: String,
    pub method: MethodDto,
    pub customer: CustomerDto,
    /// Obrigatorio para PIX no Stripe (retorno pos-confirmacao).
    pub return_url: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MethodDto {
    Card {
        /// Token gerado pelo SDK do provedor (ex.: pm_...). Nunca PAN/CVV.
        token: String,
        #[serde(default)]
        installments: i32,
    },
    Pix {
        /// Segundos ate o QR expirar. 0 = default do provedor.
        #[serde(default)]
        expires_after_seconds: i32,
    },
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CustomerDto {
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub tax_id: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub ip: String,
}
