use crate::error::GatewayError;
use crate::models::{PaymentIntent, Refund, StripeErrorBody};
use reqwest::RequestBuilder;
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use std::time::Duration;

pub struct StripeClient {
    http: reqwest::Client,
    api_base: String,
    secret_key: SecretString,
    api_version: Option<String>,
}

impl StripeClient {
    pub fn new(
        api_base: impl Into<String>,
        secret_key: SecretString,
        api_version: Option<String>,
        timeout: Duration,
    ) -> Result<Self, GatewayError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| GatewayError::Malformed(e.to_string()))?;
        Ok(Self {
            http,
            api_base: api_base.into(),
            secret_key,
            api_version,
        })
    }

    fn apply_auth(&self, req: RequestBuilder, idempotency_key: Option<&str>) -> RequestBuilder {
        let mut req = req.basic_auth(self.secret_key.expose_secret(), Option::<&str>::None);
        if let Some(version) = &self.api_version {
            req = req.header("Stripe-Version", version);
        }
        if let Some(key) = idempotency_key {
            req = req.header("Idempotency-Key", key);
        }
        req
    }

    async fn send<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T, GatewayError> {
        let resp = req
            .send()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        if !(200..300).contains(&status) {
            let detail = serde_json::from_slice::<StripeErrorBody>(&body).ok();
            let (t, c, d, m) = detail
                .map(|b| {
                    (
                        b.error.r#type.unwrap_or_default(),
                        b.error.code.unwrap_or_default(),
                        b.error.decline_code.unwrap_or_default(),
                        b.error.message.unwrap_or_default(),
                    )
                })
                .unwrap_or_else(|| {
                    let snippet = String::from_utf8_lossy(&body);
                    let snippet: String = snippet.chars().take(200).collect();
                    (String::new(), String::new(), String::new(), snippet)
                });
            return Err(GatewayError::Api {
                status,
                error_type: t,
                code: c,
                decline_code: d,
                message: m,
            });
        }
        serde_json::from_slice(&body).map_err(|e| {
            GatewayError::Malformed(format!("{e}: {}", String::from_utf8_lossy(&body)))
        })
    }

    async fn post_form<T: DeserializeOwned>(
        &self,
        path: &str,
        form: &[(String, String)],
        idempotency_key: Option<&str>,
    ) -> Result<T, GatewayError> {
        let req = self
            .http
            .post(format!("{}{}", self.api_base, path))
            .form(form);
        self.send(self.apply_auth(req, idempotency_key)).await
    }

    pub async fn create_payment_intent(
        &self,
        form: &[(String, String)],
        idempotency_key: &str,
    ) -> Result<PaymentIntent, GatewayError> {
        let mut form = form.to_vec();
        form.push(("expand[]".to_string(), "payment_method".to_string()));
        self.post_form("/v1/payment_intents", &form, Some(idempotency_key))
            .await
    }

    pub async fn retrieve_payment_intent(&self, id: &str) -> Result<PaymentIntent, GatewayError> {
        let req = self
            .http
            .get(format!("{}/v1/payment_intents/{id}", self.api_base))
            .query(&[("expand[]", "payment_method")]);
        self.send(self.apply_auth(req, None)).await
    }

    pub async fn cancel_payment_intent(
        &self,
        id: &str,
        idempotency_key: &str,
    ) -> Result<PaymentIntent, GatewayError> {
        self.post_form(
            &format!("/v1/payment_intents/{id}/cancel"),
            &[],
            Some(idempotency_key),
        )
        .await
    }

    pub async fn create_refund(
        &self,
        payment_intent_id: &str,
        amount_minor: i64,
        idempotency_key: &str,
    ) -> Result<Refund, GatewayError> {
        let mut form = vec![("payment_intent".to_string(), payment_intent_id.to_string())];
        if amount_minor > 0 {
            form.push(("amount".to_string(), amount_minor.to_string()));
        }
        self.post_form("/v1/refunds", &form, Some(idempotency_key))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(uri: String) -> StripeClient {
        StripeClient::new(
            uri,
            SecretString::new("sk_test_123".into()),
            Some("2025-01-01.test".to_string()),
            Duration::from_secs(5),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn create_payment_intent_returns_pix_details() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment_intents"))
            .and(header("Idempotency-Key", "key-1"))
            .and(header("Stripe-Version", "2025-01-01.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "pi_123",
                "status": "requires_action",
                "amount": 5000,
                "currency": "brl",
                "client_secret": "sec_1",
                "payment_method_types": ["pix"],
                "next_action": {
                    "type": "pix_display_qr_code",
                    "pix_display_qr_code": {
                        "data": "emv-payload",
                        "image_url_png": "https://qr/png",
                        "image_url_svg": "https://qr/svg",
                        "hosted_instructions_url": "https://pay.stripe.com/x",
                        "expires_at": 1700003600
                    }
                }
            })))
            .mount(&server)
            .await;

        let pi = client(server.uri())
            .create_payment_intent(&[], "key-1")
            .await
            .unwrap();
        assert_eq!(pi.id, "pi_123");
        let qr = pi
            .next_action
            .and_then(|n| n.pix_display_qr_code)
            .expect("pix qr code");
        assert_eq!(qr.data.as_deref(), Some("emv-payload"));
        assert_eq!(qr.expires_at, Some(1700003600));
    }

    #[tokio::test]
    async fn card_decline_maps_to_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment_intents"))
            .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
                "error": {
                    "type": "card_error",
                    "code": "card_declined",
                    "decline_code": "generic_decline",
                    "message": "Your card was declined."
                }
            })))
            .mount(&server)
            .await;

        let err = client(server.uri())
            .create_payment_intent(&[], "key-1")
            .await
            .unwrap_err();
        match err {
            GatewayError::Api {
                status,
                error_type,
                code,
                decline_code,
                ..
            } => {
                assert_eq!(status, 402);
                assert_eq!(error_type, "card_error");
                assert_eq!(code, "card_declined");
                assert_eq!(decline_code, "generic_decline");
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refund_sends_amount_only_when_positive() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/refunds"))
            .and(wiremock::matchers::body_string_contains(
                "payment_intent=pi_1",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "re_1",
                "amount": 2000,
                "currency": "brl",
                "status": "succeeded"
            })))
            .mount(&server)
            .await;

        let refund = client(server.uri())
            .create_refund("pi_1", 2000, "key-2")
            .await
            .unwrap();
        assert_eq!(refund.id, "re_1");
        assert_eq!(refund.amount, 2000);
    }
}
