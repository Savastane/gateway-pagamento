use crate::models::WebhookEvent;
use gateway_proto::payment_gateway::v1 as pb;
use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;

pub struct NormalizedEvent {
    pub event_id: String,
    pub external_id: String,
    pub status: i32,
}

// O asaas nao assina o corpo do webhook: a validacao e o `authToken`
// estatico enviado no header `asaas-access-token`, configurado por
// webhook na conta. Comparacao em tempo constante.
pub fn verify_access_token(
    headers: &std::collections::HashMap<String, String>,
    expected: &SecretString,
) -> Result<(), crate::error::GatewayError> {
    let provided: Vec<&str> = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("asaas-access-token"))
        .map(|(_, v)| v.as_str())
        .collect();
    if provided.is_empty() {
        return Err(crate::error::GatewayError::Webhook(
            "header asaas-access-token ausente".to_string(),
        ));
    }
    let ok: bool = provided.iter().any(|v| {
        v.as_bytes()
            .ct_eq(expected.expose_secret().as_bytes())
            .into()
    });
    if !ok {
        return Err(crate::error::GatewayError::Webhook(
            "asaas-access-token nao confere".to_string(),
        ));
    }
    Ok(())
}

pub fn normalize(event: &WebhookEvent) -> NormalizedEvent {
    let payment = event.payment.as_ref();
    let external_id = payment.map(|p| p.id.clone()).unwrap_or_default();
    let event_id = event
        .id
        .clone()
        .unwrap_or_else(|| format!("{}:{}", event.event, external_id));

    let status = match event.event.as_str() {
        "PAYMENT_AUTHORIZED" => pb::PaymentStatus::Authorized as i32,
        "PAYMENT_CONFIRMED"
        | "PAYMENT_RECEIVED"
        | "PAYMENT_RECEIVED_IN_CASH_UNDONE"
        | "PAYMENT_APPROVED_BY_RISK_ANALYSIS" => pb::PaymentStatus::Paid as i32,
        "PAYMENT_AWAITING_RISK_ANALYSIS" | "PAYMENT_CREATED" | "PAYMENT_UPDATED" => {
            pb::PaymentStatus::Pending as i32
        }
        "PAYMENT_REFUNDED" => pb::PaymentStatus::Refunded as i32,
        "PAYMENT_PARTIALLY_REFUNDED" | "PAYMENT_REFUND_IN_PROGRESS" => {
            pb::PaymentStatus::PartiallyRefunded as i32
        }
        "PAYMENT_OVERDUE" => pb::PaymentStatus::Expired as i32,
        "PAYMENT_DELETED" => pb::PaymentStatus::Canceled as i32,
        "PAYMENT_REPROVED_BY_RISK_ANALYSIS"
        | "PAYMENT_CREDIT_CARD_CAPTURE_REFUSED"
        | "PAYMENT_CHARGEBACK_REQUESTED"
        | "PAYMENT_CHARGEBACK_DISPUTE"
        | "PAYMENT_DUNNING_REQUESTED" => pb::PaymentStatus::Failed as i32,
        _ => payment
            .map(|p| {
                p.status
                    .as_deref()
                    .map(|s| crate::service::AsaasGateway::payment_status(s, p.deleted))
                    .unwrap_or(pb::PaymentStatus::Unspecified as i32)
            })
            .unwrap_or(pb::PaymentStatus::Unspecified as i32),
    };

    NormalizedEvent {
        event_id,
        external_id,
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(payload: &str) -> NormalizedEvent {
        let event: WebhookEvent = serde_json::from_str(payload).unwrap();
        normalize(&event)
    }

    #[test]
    fn normalizes_payment_received() {
        let n = parse(
            r#"{"id":"evt_abc&123","event":"PAYMENT_RECEIVED","payment":{"id":"pay_123","status":"RECEIVED"}}"#,
        );
        assert_eq!(n.event_id, "evt_abc&123");
        assert_eq!(n.external_id, "pay_123");
        assert_eq!(n.status, pb::PaymentStatus::Paid as i32);
    }

    #[test]
    fn normalizes_deleted_and_partial_refund() {
        let n = parse(r#"{"event":"PAYMENT_DELETED","payment":{"id":"pay_1"}}"#);
        assert_eq!(n.status, pb::PaymentStatus::Canceled as i32);
        let n = parse(
            r#"{"event":"PAYMENT_PARTIALLY_REFUNDED","payment":{"id":"pay_1","status":"RECEIVED"}}"#,
        );
        assert_eq!(n.status, pb::PaymentStatus::PartiallyRefunded as i32);
    }

    #[test]
    fn falls_back_to_payment_status() {
        let n =
            parse(r#"{"event":"PAYMENT_RESTORED","payment":{"id":"pay_1","status":"PENDING"}}"#);
        assert_eq!(n.status, pb::PaymentStatus::Pending as i32);
    }

    #[test]
    fn verifies_access_token() {
        let token = SecretString::new("secret-token".to_string().into());
        let mut headers = std::collections::HashMap::new();
        assert!(verify_access_token(&headers, &token).is_err());
        headers.insert("Asaas-Access-Token".to_string(), "secret-token".to_string());
        assert!(verify_access_token(&headers, &token).is_ok());
        headers.insert("asaas-access-token".to_string(), "wrong".to_string());
        assert!(verify_access_token(&headers, &token).is_ok());
        headers.remove("Asaas-Access-Token");
        assert!(verify_access_token(&headers, &token).is_err());
    }
}
