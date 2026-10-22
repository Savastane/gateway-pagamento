use crate::store::NewEvent;
use secrecy::{ExposeSecret, SecretString};
use std::time::Duration;

// Encaminha o evento normalizado para a api-pagamento
// (POST /internal/gateway-events) com retry e backoff.
// Sem callback configurado, o evento so persiste — util em dev.
#[derive(Clone)]
pub struct EventForwarder {
    http: reqwest::Client,
    callback_url: Option<String>,
    internal_token: Option<SecretString>,
}

impl EventForwarder {
    pub fn new(callback_url: Option<String>, internal_token: Option<SecretString>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            callback_url,
            internal_token,
        }
    }

    fn body(event: &NewEvent) -> serde_json::Value {
        let payload: serde_json::Value =
            serde_json::from_slice(&event.payload).unwrap_or(serde_json::Value::Null);
        serde_json::json!({
            "gateway": event.gateway,
            "provider_event_id": event.provider_event_id,
            "event_type": event.event_type,
            "external_id": event.external_id,
            "status": event.normalized_status,
            "provider_payload": payload,
        })
    }

    pub async fn forward(&self, event: &NewEvent) -> Result<(), String> {
        let Some(url) = &self.callback_url else {
            tracing::debug!(gateway = %event.gateway, event = %event.provider_event_id, "sem callback configurado; evento apenas persistido");
            return Ok(());
        };

        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * 2_i64.pow(attempt - 1) as u64))
                    .await;
            }
            let mut req = self.http.post(url).json(&Self::body(event));
            if let Some(token) = &self.internal_token {
                req = req.bearer_auth(token.expose_secret());
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                Ok(resp) => {
                    last_err = format!("callback http {}", resp.status().as_u16());
                    if resp.status().is_client_error() {
                        break; // 4xx nao adianta repetir
                    }
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        Err(last_err)
    }
}
