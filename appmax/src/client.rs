use crate::error::GatewayError;
use crate::models::*;
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TOKEN_SKEW: Duration = Duration::from_secs(60);

struct CachedToken {
    value: String,
    expires_at: Instant,
}

pub struct CardCharge<'a> {
    pub token: &'a str,
    pub holder_name: &'a str,
    pub holder_document_number: &'a str,
    pub installments: i32,
    pub soft_descriptor: Option<&'a str>,
}

pub struct AppmaxClient {
    http: reqwest::Client,
    api_base: String,
    auth_base: String,
    client_id: String,
    client_secret: SecretString,
    token: Mutex<Option<CachedToken>>,
}

impl AppmaxClient {
    pub fn new(
        api_base: impl Into<String>,
        auth_base: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: SecretString,
        timeout: Duration,
    ) -> Result<Self, GatewayError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| GatewayError::Malformed(e.to_string()))?;
        Ok(Self {
            http,
            api_base: api_base.into(),
            auth_base: auth_base.into(),
            client_id: client_id.into(),
            client_secret,
            token: Mutex::new(None),
        })
    }

    async fn fetch_token(&self) -> Result<String, GatewayError> {
        let resp = self
            .http
            .post(format!("{}/oauth2/token", self.auth_base))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.expose_secret()),
            ])
            .send()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(GatewayError::Auth(format!(
                "oauth2/token http {status}: {}",
                String::from_utf8_lossy(&body)
            )));
        }
        let token: TokenResponse = serde_json::from_slice(&body)
            .map_err(|e| GatewayError::Malformed(format!("token response: {e}")))?;
        let ttl = Duration::from_secs(token.expires_in.max(120)) - TOKEN_SKEW;
        *self.token.lock().expect("token mutex") = Some(CachedToken {
            value: token.access_token.clone(),
            expires_at: Instant::now() + ttl,
        });
        Ok(token.access_token)
    }

    async fn token(&self) -> Result<String, GatewayError> {
        let cached = self
            .token
            .lock()
            .expect("token mutex")
            .as_ref()
            .and_then(|t| (Instant::now() < t.expires_at).then(|| t.value.clone()));
        match cached {
            Some(t) => Ok(t),
            None => self.fetch_token().await,
        }
    }

    fn invalidate_token(&self) {
        *self.token.lock().expect("token mutex") = None;
    }

    async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        for attempt in 0..2 {
            let token = self.token().await?;
            let resp = self
                .http
                .post(format!("{}{}", self.api_base, path))
                .bearer_auth(&token)
                .json(body)
                .send()
                .await
                .map_err(|e| GatewayError::Transport(e.to_string()))?;
            let status = resp.status().as_u16();
            if status == 401 && attempt == 0 {
                self.invalidate_token();
                continue;
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| GatewayError::Transport(e.to_string()))?;
            return self.decode(status, &bytes);
        }
        unreachable!()
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, GatewayError> {
        for attempt in 0..2 {
            let token = self.token().await?;
            let resp = self
                .http
                .get(format!("{}{}", self.api_base, path))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| GatewayError::Transport(e.to_string()))?;
            let status = resp.status().as_u16();
            if status == 401 && attempt == 0 {
                self.invalidate_token();
                continue;
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| GatewayError::Transport(e.to_string()))?;
            return self.decode(status, &bytes);
        }
        unreachable!()
    }

    fn decode<T: DeserializeOwned>(&self, status: u16, body: &[u8]) -> Result<T, GatewayError> {
        if !(200..300).contains(&status) {
            let message = serde_json::from_slice::<ErrorBody>(body)
                .ok()
                .and_then(|e| e.error.and_then(|d| d.message).or(e.message))
                .unwrap_or_else(|| {
                    let snippet = String::from_utf8_lossy(body);
                    snippet.chars().take(200).collect()
                });
            return Err(GatewayError::Api { status, message });
        }
        serde_json::from_slice(body)
            .map_err(|e| GatewayError::Malformed(format!("{e}: {}", String::from_utf8_lossy(body))))
    }

    pub async fn create_customer(
        &self,
        customer: &NewCustomer<'_>,
    ) -> Result<CustomerObj, GatewayError> {
        let resp: Data<CustomerData> = self.post_json("/v1/customers", &json!(customer)).await?;
        Ok(resp.data.customer)
    }

    pub async fn create_order(&self, order: &NewOrder<'_>) -> Result<OrderObj, GatewayError> {
        let resp: Data<OrderData> = self.post_json("/v1/orders", &json!(order)).await?;
        Ok(resp.data.order)
    }

    pub async fn pay_pix(
        &self,
        order_id: i64,
        document_number: &str,
        expiration_date: Option<&str>,
    ) -> Result<PaymentResponse, GatewayError> {
        let mut pix = serde_json::Map::new();
        if !document_number.is_empty() {
            pix.insert("document_number".into(), document_number.into());
        }
        if let Some(exp) = expiration_date {
            pix.insert("expiration_date".into(), exp.into());
        }
        let body = json!({
            "order_id": order_id,
            "payment_data": { "pix": pix }
        });
        let resp: Data<PaymentResponse> = self.post_json("/v1/payments/pix", &body).await?;
        Ok(resp.data)
    }

    pub async fn pay_card(
        &self,
        order_id: i64,
        customer_id: i64,
        card: &CardCharge<'_>,
    ) -> Result<PaymentResponse, GatewayError> {
        let mut card_json = serde_json::json!({
            "token": card.token,
            "holder_name": card.holder_name,
            "holder_document_number": card.holder_document_number,
            "installments": card.installments.max(1),
        });
        if let Some(sd) = card.soft_descriptor {
            card_json["soft_descriptor"] = sd.into();
        }
        let body = json!({
            "order_id": order_id,
            "customer_id": customer_id,
            "payment_data": { "credit_card": card_json }
        });
        let resp: Data<PaymentResponse> = self.post_json("/v1/payments/credit-card", &body).await?;
        Ok(resp.data)
    }

    pub async fn get_order(&self, order_id: &str) -> Result<OrderDetail, GatewayError> {
        let resp: Data<OrderDetail> = self.get_json(&format!("/v1/orders/{order_id}")).await?;
        Ok(resp.data)
    }

    pub async fn refund_order(
        &self,
        order_id: i64,
        amount_minor: i64,
    ) -> Result<RefundData, GatewayError> {
        let body = if amount_minor > 0 {
            json!({ "order_id": order_id, "type": "partial", "value": amount_minor })
        } else {
            json!({ "order_id": order_id, "type": "total" })
        };
        let resp: Data<RefundData> = self.post_json("/v1/orders/refund-request", &body).await?;
        Ok(resp.data)
    }
}
