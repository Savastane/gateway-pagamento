use crate::error::GatewayError;
use crate::models::WebhookEvent;
use gateway_proto::payment_gateway::v1 as pb;
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use std::collections::HashMap;
use subtle::ConstantTimeEq;

const TOLERANCE_MS: i64 = 5 * 60 * 1000;

pub struct NormalizedEvent {
    pub event_id: String,
    pub event_type: String,
    pub external_id: String,
}

fn header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

// Assinatura do mercado pago: x-signature="ts=<ms>,v1=<hmac-sha256 hex>"
// sobre o manifesto "id:{data.id};request-id:{x-request-id};ts:{ts};"
// (pares omitidos quando vazios; data.id em lowercase). Comparação
// timing-safe e tolerância de 5 min no timestamp.
pub fn verify_signature(
    headers: &HashMap<String, String>,
    data_id: &str,
    secret: &SecretString,
    now_ms: i64,
) -> Result<(), GatewayError> {
    let raw = header(headers, "x-signature")
        .ok_or_else(|| GatewayError::Webhook("header x-signature ausente".to_string()))?;
    let mut ts = None;
    let mut v1 = None;
    for part in raw.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("ts=") {
            ts = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("v1=") {
            v1 = Some(v.to_string());
        }
    }
    let ts = ts.ok_or_else(|| GatewayError::Webhook("x-signature sem ts".to_string()))?;
    let v1 = v1.ok_or_else(|| GatewayError::Webhook("x-signature sem v1".to_string()))?;

    let ts_ms: i64 = ts
        .parse()
        .map_err(|_| GatewayError::Webhook("ts invalido".to_string()))?;
    if (now_ms - ts_ms).abs() > TOLERANCE_MS {
        return Err(GatewayError::Webhook(
            "timestamp fora da tolerancia".to_string(),
        ));
    }

    let mut manifest = String::new();
    if !data_id.is_empty() {
        manifest.push_str(&format!("id:{};", data_id.to_lowercase()));
    }
    if let Some(req_id) = header(headers, "x-request-id")
        && !req_id.is_empty()
    {
        manifest.push_str(&format!("request-id:{req_id};"));
    }
    manifest.push_str(&format!("ts:{ts};"));

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.expose_secret().as_bytes())
        .map_err(|e| GatewayError::Malformed(e.to_string()))?;
    mac.update(manifest.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());
    let ok: bool = expected.as_bytes().ct_eq(v1.as_bytes()).into();
    if !ok {
        return Err(GatewayError::Webhook("assinatura nao confere".to_string()));
    }
    Ok(())
}

pub fn data_id_string(event: &WebhookEvent) -> String {
    event
        .data
        .as_ref()
        .and_then(|d| d.id.as_ref())
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

pub fn normalize(event: &WebhookEvent) -> NormalizedEvent {
    let data_id = data_id_string(event);
    let event_type = event
        .action
        .clone()
        .or_else(|| event.kind.clone())
        .unwrap_or_default();
    let event_id = event
        .id
        .as_ref()
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| format!("{event_type}:{data_id}"));
    NormalizedEvent {
        event_id,
        event_type,
        external_id: data_id,
    }
}

pub fn event_status_hint(_event: &WebhookEvent) -> i32 {
    // O envelope do MP so traz o id do recurso; o status real deve ser
    // rebuscado via GetPayment (regra de ouro de webhooks).
    pb::PaymentStatus::Unspecified as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(manifest: &str, secret: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(manifest.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn verifies_valid_signature() {
        let secret = SecretString::new("mp-secret".to_string().into());
        let ts = 1_700_000_000_000i64;
        let manifest = "id:999999999;request-id:req-1;ts:1700000000000;";
        let v1 = sign(manifest, "mp-secret");
        let mut headers = HashMap::new();
        headers.insert("x-signature".to_string(), format!("ts={ts},v1={v1}"));
        headers.insert("x-request-id".to_string(), "req-1".to_string());
        assert!(verify_signature(&headers, "999999999", &secret, ts + 1000).is_ok());
    }

    #[test]
    fn rejects_tampered_and_stale() {
        let secret = SecretString::new("mp-secret".to_string().into());
        let ts = 1_700_000_000_000i64;
        let manifest = "id:999999999;request-id:req-1;ts:1700000000000;";
        let v1 = sign(manifest, "mp-secret");
        let mut headers = HashMap::new();
        headers.insert("x-signature".to_string(), format!("ts={ts},v1={v1}"));
        headers.insert("x-request-id".to_string(), "req-1".to_string());

        assert!(verify_signature(&headers, "888888888", &secret, ts + 1000).is_err());
        assert!(verify_signature(&headers, "999999999", &secret, ts + 600_000).is_err());
        headers.insert("x-signature".to_string(), format!("ts={ts},v1=deadbeef"));
        assert!(verify_signature(&headers, "999999999", &secret, ts + 1000).is_err());
    }

    #[test]
    fn normalizes_envelope() {
        let event: WebhookEvent = serde_json::from_str(
            r#"{"id":12345,"type":"payment","action":"payment.created","data":{"id":"999999999"}}"#,
        )
        .unwrap();
        let n = normalize(&event);
        assert_eq!(n.event_id, "12345");
        assert_eq!(n.event_type, "payment.created");
        assert_eq!(n.external_id, "999999999");
    }
}
