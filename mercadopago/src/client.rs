use crate::error::GatewayError;
use crate::models::*;
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::time::Duration;

pub struct MercadoPagoClient {
    http: reqwest::Client,
    api_base: String,
    access_token: SecretString,
}

impl MercadoPagoClient {
    pub fn new(
        api_base: impl Into<String>,
        access_token: SecretString,
        timeout: Duration,
    ) -> Result<Self, GatewayError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent("gateway-mercadopago/0.1")
            .build()
            .map_err(|e| GatewayError::Malformed(e.to_string()))?;
        Ok(Self {
            http,
            api_base: api_base.into(),
            access_token,
        })
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(self.access_token.expose_secret())
    }

    async fn send<T: DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<T, GatewayError> {
        let resp = self
            .auth(req)
            .send()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        if !(200..300).contains(&status) {
            let message = serde_json::from_slice::<ErrorBody>(&body)
                .ok()
                .map(|e| {
                    let causes = e
                        .cause
                        .iter()
                        .filter_map(|c| c.description.clone())
                        .collect::<Vec<_>>()
                        .join("; ");
                    match (e.message, causes.is_empty()) {
                        (Some(m), true) => m,
                        (Some(m), false) => format!("{m} ({causes})"),
                        (None, false) => causes,
                        (None, true) => String::new(),
                    }
                })
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| {
                    let snippet = String::from_utf8_lossy(&body);
                    snippet.chars().take(200).collect()
                });
            return Err(GatewayError::Api { status, message });
        }
        serde_json::from_slice(&body).map_err(|e| {
            GatewayError::Malformed(format!("{e}: {}", String::from_utf8_lossy(&body)))
        })
    }

    pub async fn create_payment(
        &self,
        body: &serde_json::Value,
        idempotency_key: &str,
    ) -> Result<Payment, GatewayError> {
        self.send(
            self.http
                .post(format!("{}/v1/payments", self.api_base))
                .header("X-Idempotency-Key", idempotency_key)
                .json(body),
        )
        .await
    }

    pub async fn get_payment(&self, payment_id: &str) -> Result<Payment, GatewayError> {
        self.send(
            self.http
                .get(format!("{}/v1/payments/{payment_id}", self.api_base)),
        )
        .await
    }

    pub async fn cancel_payment(&self, payment_id: &str) -> Result<Payment, GatewayError> {
        self.send(
            self.http
                .put(format!("{}/v1/payments/{payment_id}", self.api_base))
                .json(&json!({ "status": "cancelled" })),
        )
        .await
    }

    pub async fn refund_payment(
        &self,
        payment_id: &str,
        amount_minor: i64,
        idempotency_key: &str,
    ) -> Result<Refund, GatewayError> {
        let body = if amount_minor > 0 {
            json!({ "amount": crate::service::minor_to_decimal(amount_minor) })
        } else {
            json!({})
        };
        self.send(
            self.http
                .post(format!(
                    "{}/v1/payments/{payment_id}/refunds",
                    self.api_base
                ))
                .header("X-Idempotency-Key", idempotency_key)
                .json(&body),
        )
        .await
    }
}
