use crate::error::GatewayError;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

pub const DEFAULT_TOLERANCE_SECS: i64 = 300;

// Stripe-Signature: "t=<unix_ts>,v1=<hex_hmac>[,v0=...]"
// signed payload = "{t}.{raw_body}", HMAC-SHA256 keyed by whsec_...
pub fn verify_signature(
    payload: &[u8],
    signature_header: &str,
    secret: &str,
    now: i64,
    tolerance_secs: i64,
) -> Result<(), GatewayError> {
    let mut timestamp: Option<i64> = None;
    let mut signatures: Vec<&str> = Vec::new();

    for part in signature_header.split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.trim() {
            "t" => timestamp = value.trim().parse().ok(),
            "v1" => signatures.push(value.trim()),
            _ => {}
        }
    }

    let ts = timestamp.ok_or(GatewayError::InvalidSignature)?;
    if signatures.is_empty() {
        return Err(GatewayError::InvalidSignature);
    }
    if (now - ts).abs() > tolerance_secs {
        return Err(GatewayError::StaleTimestamp);
    }

    let signed_payload = format!("{ts}.{}", String::from_utf8_lossy(payload));
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(signed_payload.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    let matched = signatures
        .iter()
        .any(|s| bool::from(s.as_bytes().ct_eq(expected.as_bytes())));
    if matched {
        Ok(())
    } else {
        Err(GatewayError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(payload: &[u8], secret: &str, ts: i64) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{ts}.{}", String::from_utf8_lossy(payload)).as_bytes());
        format!("t={ts},v1={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn accepts_valid_signature() {
        let payload = br#"{"id":"evt_1","type":"payment_intent.succeeded"}"#;
        let secret = "whsec_test_secret";
        let now = 1_700_000_000;
        let header = sign(payload, secret, now);
        assert!(verify_signature(payload, &header, secret, now, DEFAULT_TOLERANCE_SECS).is_ok());
    }

    #[test]
    fn rejects_tampered_payload() {
        let payload = b"{}";
        let secret = "whsec_test_secret";
        let now = 1_700_000_000;
        let header = sign(payload, secret, now);
        let tampered = b"{\"x\":1}";
        let result = verify_signature(tampered, &header, secret, now, DEFAULT_TOLERANCE_SECS);
        assert!(matches!(result, Err(GatewayError::InvalidSignature)));
    }

    #[test]
    fn rejects_stale_timestamp() {
        let payload = b"{}";
        let secret = "whsec_test_secret";
        let old = 1_700_000_000;
        let header = sign(payload, secret, old);
        let now = old + 301;
        let result = verify_signature(payload, &header, secret, now, DEFAULT_TOLERANCE_SECS);
        assert!(matches!(result, Err(GatewayError::StaleTimestamp)));
    }

    #[test]
    fn rejects_header_without_v1() {
        let result = verify_signature(b"{}", "t=123,v0=abc", "whsec_x", 100, 300);
        assert!(matches!(result, Err(GatewayError::InvalidSignature)));
    }
}
