use crate::models::WebhookEvent;
use gateway_proto::payment_gateway::v1 as pb;

pub struct NormalizedEvent {
    pub event_id: String,
    pub external_id: String,
    pub status: i32,
}

// A appmax nao assina webhooks (sem HMAC/token): validacao de origem
// fica a cargo da camada de recepcao (allowlist de IP, chaves do envelope).
// Aqui so normalizamos o envelope {event, event_type, data.order}.
pub fn normalize(event: &WebhookEvent) -> NormalizedEvent {
    let order = event.data.as_ref().and_then(|d| d.order.as_ref());
    let external_id = order.map(|o| o.id.to_string()).unwrap_or_default();
    let event_id = format!("{}:{}", event.event, external_id);

    let status = match event.event.as_str() {
        "order_authorized" | "order_authorized_with_delay" => pb::PaymentStatus::Authorized as i32,
        "order_approved"
        | "order_paid"
        | "order_paid_by_pix"
        | "order_integrated"
        | "order_pending_integration" => pb::PaymentStatus::Paid as i32,
        "order_refund" => pb::PaymentStatus::Refunded as i32,
        "order_partial_refund" => pb::PaymentStatus::PartiallyRefunded as i32,
        "order_pix_expired" | "order_billet_overdue" => pb::PaymentStatus::Expired as i32,
        "order_refused_by_risk" => pb::PaymentStatus::Refunded as i32,
        "payment_not_authorized" | "order_chargeback_in_treatment" => {
            pb::PaymentStatus::Failed as i32
        }
        _ => order
            .map(|o| crate::service::AppmaxGateway::order_status(&o.status))
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
    fn normalizes_order_approved() {
        let n = parse(
            r#"{"event":"order_approved","event_type":"order","data":{"order":{"id":3531,"status":"aprovado","total_paid":12300}}}"#,
        );
        assert_eq!(n.external_id, "3531");
        assert_eq!(n.event_id, "order_approved:3531");
        assert_eq!(n.status, pb::PaymentStatus::Paid as i32);
    }

    #[test]
    fn normalizes_pix_expired() {
        let n = parse(
            r#"{"event":"order_pix_expired","event_type":"order","data":{"order":{"id":9,"status":"cancelado"}}}"#,
        );
        assert_eq!(n.status, pb::PaymentStatus::Expired as i32);
    }

    #[test]
    fn falls_back_to_order_status() {
        let n = parse(
            r#"{"event":"order_up_sold","event_type":"order","data":{"order":{"id":7,"status":"aprovado"}}}"#,
        );
        assert_eq!(n.status, pb::PaymentStatus::Paid as i32);
    }
}
