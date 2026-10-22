use crate::error::GatewayError;
use crate::models::*;
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::time::Duration;

pub struct AsaasClient {
    http: reqwest::Client,
    api_base: String,
    access_token: SecretString,
}

impl AsaasClient {
    pub fn new(
        api_base: impl Into<String>,
        access_token: SecretString,
        timeout: Duration,
    ) -> Result<Self, GatewayError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent("gateway-asaas/0.1")
            .build()
            .map_err(|e| GatewayError::Malformed(e.to_string()))?;
        Ok(Self {
            http,
            api_base: api_base.into(),
            access_token,
        })
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("access_token", self.access_token.expose_secret())
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
                    e.errors
                        .iter()
                        .map(|i| format!("{}: {}", i.code, i.description))
                        .collect::<Vec<_>>()
                        .join("; ")
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

    pub async fn find_customer_by_document(
        &self,
        cpf_cnpj: &str,
    ) -> Result<Option<Customer>, GatewayError> {
        let list: CustomerList = self
            .send(self.http.get(format!(
                "{}/customers?cpfCnpj={cpf_cnpj}&limit=1",
                self.api_base
            )))
            .await?;
        Ok(list.data.into_iter().next())
    }

    pub async fn create_customer(
        &self,
        name: &str,
        cpf_cnpj: &str,
        email: &str,
        phone: &str,
    ) -> Result<Customer, GatewayError> {
        let mut body = json!({ "name": name, "cpfCnpj": cpf_cnpj });
        if !email.is_empty() {
            body["email"] = email.into();
        }
        if !phone.is_empty() {
            body["mobilePhone"] = phone.into();
        }
        self.send(
            self.http
                .post(format!("{}/customers", self.api_base))
                .json(&body),
        )
        .await
    }

    pub async fn create_payment(&self, body: &serde_json::Value) -> Result<Payment, GatewayError> {
        self.send(
            self.http
                .post(format!("{}/payments", self.api_base))
                .json(body),
        )
        .await
    }

    pub async fn get_payment(&self, payment_id: &str) -> Result<Payment, GatewayError> {
        self.send(
            self.http
                .get(format!("{}/payments/{payment_id}", self.api_base)),
        )
        .await
    }

    pub async fn delete_payment(&self, payment_id: &str) -> Result<(), GatewayError> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .delete(format!("{}/payments/{payment_id}", self.api_base)),
            )
            .await?;
        Ok(())
    }

    pub async fn refund_payment(
        &self,
        payment_id: &str,
        amount_minor: i64,
    ) -> Result<Payment, GatewayError> {
        let body = if amount_minor > 0 {
            json!({ "value": crate::service::minor_to_decimal(amount_minor) })
        } else {
            json!({})
        };
        self.send(
            self.http
                .post(format!("{}/payments/{payment_id}/refund", self.api_base))
                .json(&body),
        )
        .await
    }

    pub async fn pix_qr_code(&self, payment_id: &str) -> Result<PixQrCode, GatewayError> {
        self.send(
            self.http
                .get(format!("{}/payments/{payment_id}/pixQrCode", self.api_base)),
        )
        .await
    }
}
