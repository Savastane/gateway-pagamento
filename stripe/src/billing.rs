use crate::client::StripeClient;
use gateway_proto::payment_gateway::v1 as pb;
use tonic::{Request, Response, Status};

pub struct Billing(pub StripeClient);

fn identifier(value: &str, prefix: &str) -> Result<(), Status> {
    if !value.starts_with(prefix)
        || value.len() > 255
        || !value.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(Status::invalid_argument("invalid provider identifier"));
    }
    Ok(())
}

fn return_url(value: &str) -> Result<(), Status> {
    let url = reqwest::Url::parse(value).map_err(|_| Status::invalid_argument("invalid return URL"))?;
    if url.scheme() != "https"
        && !(url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1")))
    {
        return Err(Status::invalid_argument("return URL requires HTTPS"));
    }
    Ok(())
}

fn checkout_form(r: &pb::BillingRequest) -> Result<Vec<(String, String)>, Status> {
    let (amount, interval) = match r.plan.as_str() {
        "MENSAL" => ("4990", "month"),
        "ANUAL" => ("47900", "year"),
        _ => return Err(Status::invalid_argument("invalid plan")),
    };
    if r.reference.is_empty() || r.professional_id.is_empty() || r.email.is_empty() {
        return Err(Status::invalid_argument("reference, professional_id and email are required"));
    }
    return_url(&r.success_url)?;
    return_url(&r.cancel_url)?;
    let mut form = vec![
        ("mode".into(), "subscription".into()),
        ("payment_method_types[0]".into(), "card".into()),
        ("locale".into(), "pt-BR".into()),
        ("client_reference_id".into(), r.reference.clone()),
        ("customer_email".into(), r.email.clone()),
        ("success_url".into(), r.success_url.clone()),
        ("cancel_url".into(), r.cancel_url.clone()),
        ("line_items[0][quantity]".into(), "1".into()),
        ("line_items[0][price_data][currency]".into(), "brl".into()),
        ("line_items[0][price_data][unit_amount]".into(), amount.into()),
        ("line_items[0][price_data][recurring][interval]".into(), interval.into()),
        ("line_items[0][price_data][product_data][name]".into(), format!("Bussola Pro {}", r.plan)),
    ];
    for scope in ["metadata", "subscription_data[metadata]"] {
        for (key, value) in [
            ("assinatura_id", &r.reference),
            ("profissional_id", &r.professional_id),
            ("plano", &r.plan),
        ] {
            form.push((format!("{scope}[{key}]"), value.clone()));
        }
    }
    Ok(form)
}

#[tonic::async_trait]
impl pb::stripe_billing_server::StripeBilling for Billing {
    async fn execute(&self, request: Request<pb::BillingRequest>) -> Result<Response<pb::BillingResponse>, Status> {
        let r = request.into_inner();
        let op = pb::BillingOperation::try_from(r.operation)
            .map_err(|_| Status::invalid_argument("invalid billing operation"))?;
        if matches!(op, pb::BillingOperation::CreateCheckout | pb::BillingOperation::CreatePortal)
            && (r.idempotency_key.is_empty() || r.idempotency_key.len() > 255)
        {
            return Err(Status::invalid_argument("idempotency_key is required"));
        }
        let json = match op {
            pb::BillingOperation::CreateCheckout => {
                self.0.billing_post("/v1/checkout/sessions", &checkout_form(&r)?, &r.idempotency_key).await?
            }
            pb::BillingOperation::GetCheckout => {
                identifier(&r.external_id, "cs_")?;
                self.0.billing_get(&format!("/v1/checkout/sessions/{}", r.external_id)).await?
            }
            pb::BillingOperation::GetSubscription => {
                identifier(&r.external_id, "sub_")?;
                self.0.billing_get(&format!("/v1/subscriptions/{}?expand[]=latest_invoice", r.external_id)).await?
            }
            pb::BillingOperation::CreatePortal => {
                identifier(&r.external_id, "cus_")?;
                return_url(&r.success_url)?;
                self.0.billing_post("/v1/billing_portal/sessions", &[
                    ("customer".into(), r.external_id),
                    ("return_url".into(), r.success_url),
                ], &r.idempotency_key).await?
            }
            _ => return Err(Status::invalid_argument("billing operation is required")),
        };
        Ok(Response::new(pb::BillingResponse { json: json.to_string() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recurring_prices_are_server_owned_and_card_only() {
        let mut r = pb::BillingRequest {
            reference: "local-1".into(), professional_id: "pro-1".into(),
            plan: "ANUAL".into(), email: "test@example.com".into(),
            success_url: "https://example.com/ok".into(), cancel_url: "https://example.com/cancel".into(),
            ..Default::default()
        };
        let form = checkout_form(&r).unwrap();
        assert!(form.contains(&("line_items[0][price_data][unit_amount]".into(), "47900".into())));
        assert!(form.contains(&("line_items[0][price_data][recurring][interval]".into(), "year".into())));
        assert!(form.contains(&("payment_method_types[0]".into(), "card".into())));
        r.plan = "FREE".into();
        assert!(checkout_form(&r).is_err());
        assert!(identifier("../secrets", "sub_").is_err());
        assert!(return_url("bussola://assinatura/sucesso").is_err());
    }
}
