use crate::client::MercadoPagoClient;
use crate::error::GatewayError;
use crate::models::*;
use crate::webhook;
use gateway_proto::payment_gateway::v1 as pb;
use pb::create_payment_request::Method;
use pb::payment_gateway_server::PaymentGateway;
use secrecy::SecretString;
use tonic::{Request, Response, Status};

pub fn minor_to_decimal(minor: i64) -> f64 {
    minor as f64 / 100.0
}

pub struct MercadoPagoGateway {
    client: MercadoPagoClient,
    webhook_secret: Option<SecretString>,
}

impl MercadoPagoGateway {
    pub fn new(client: MercadoPagoClient, webhook_secret: Option<SecretString>) -> Self {
        Self {
            client,
            webhook_secret,
        }
    }

    fn check_idempotency_key(key: &str) -> Result<(), GatewayError> {
        if key.is_empty() || key.len() > 255 {
            return Err(GatewayError::InvalidRequest(
                "idempotency_key e obrigatoria (1..255 chars) e vai no header X-Idempotency-Key"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn split_name(name: &str) -> (&str, &str) {
        match name.split_once(char::is_whitespace) {
            Some((first, rest)) => (first, rest.trim()),
            None => (name, "-"),
        }
    }

    fn identification_type(tax_id: &str) -> &'static str {
        if tax_id.len() == 11 { "CPF" } else { "CNPJ" }
    }

    pub fn payment_status(status: &str) -> i32 {
        match status {
            "pending" => pb::PaymentStatus::Pending as i32,
            "approved" => pb::PaymentStatus::Paid as i32,
            "authorized" => pb::PaymentStatus::Authorized as i32,
            "in_process" | "in_mediation" => pb::PaymentStatus::Pending as i32,
            "rejected" | "charged_back" => pb::PaymentStatus::Failed as i32,
            "cancelled" => pb::PaymentStatus::Canceled as i32,
            "refunded" => pb::PaymentStatus::Refunded as i32,
            _ => pb::PaymentStatus::Unspecified as i32,
        }
    }

    fn map_payment(payment: &Payment, method_hint: i32, amount: Option<&pb::Money>) -> pb::Payment {
        let method = payment
            .payment_method_id
            .as_deref()
            .map(|m| match m {
                "pix" => pb::PaymentMethod::Pix as i32,
                _ => pb::PaymentMethod::Card as i32,
            })
            .unwrap_or(method_hint);
        let created_at = payment
            .date_created
            .as_deref()
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or_default();
        pb::Payment {
            external_id: payment.id.to_string(),
            status: Self::payment_status(&payment.status),
            amount: amount.cloned().or_else(|| {
                payment.transaction_amount.map(|v| pb::Money {
                    amount_minor: (v * 100.0).round() as i64,
                    currency: "brl".to_string(),
                })
            }),
            method,
            created_at,
            ..Default::default()
        }
    }

    fn pix_details(payment: &Payment) -> Option<pb::PixDetails> {
        let data = payment
            .point_of_interaction
            .as_ref()
            .and_then(|poi| poi.transaction_data.as_ref())?;
        let emv = data.qr_code.clone()?;
        Some(pb::PixDetails {
            emv_qr_code: emv,
            qr_code_png_base64: data.qr_code_base64.clone().unwrap_or_default(),
            hosted_instructions_url: data.ticket_url.clone().unwrap_or_default(),
            expires_at: Self::pix_expiration(payment),
            ..Default::default()
        })
    }

    fn pix_expiration(payment: &Payment) -> i64 {
        payment
            .date_created
            .as_deref()
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map(|dt| dt.timestamp() + 24 * 3600)
            .unwrap_or_default()
    }
}

#[tonic::async_trait]
impl PaymentGateway for MercadoPagoGateway {
    async fn create_payment(
        &self,
        request: Request<pb::CreatePaymentRequest>,
    ) -> Result<Response<pb::CreatePaymentResponse>, Status> {
        let req = request.into_inner();
        Self::check_idempotency_key(&req.idempotency_key)?;
        let amount = req
            .amount
            .as_ref()
            .ok_or_else(|| GatewayError::InvalidRequest("amount e obrigatorio".to_string()))?;
        if amount.amount_minor <= 0 {
            return Err(
                GatewayError::InvalidRequest("amount_minor deve ser positivo".to_string()).into(),
            );
        }
        if !amount.currency.eq_ignore_ascii_case("brl") {
            return Err(GatewayError::InvalidRequest(
                "o mercado pago opera somente BRL neste adapter".to_string(),
            )
            .into());
        }

        let customer = req.customer.as_ref().ok_or_else(|| {
            GatewayError::InvalidRequest("customer e obrigatorio no mercado pago".to_string())
        })?;
        if customer.email.is_empty() {
            return Err(GatewayError::InvalidRequest(
                "customer.email e obrigatorio no mercado pago".to_string(),
            )
            .into());
        }

        let (first_name, last_name) = Self::split_name(&customer.name);
        let mut payer = serde_json::json!({
            "email": customer.email,
            "first_name": first_name,
            "last_name": last_name,
        });
        if !customer.tax_id.is_empty() {
            payer["identification"] = serde_json::json!({
                "type": Self::identification_type(&customer.tax_id),
                "number": customer.tax_id,
            });
        }

        let method_hint;
        let mut body = serde_json::json!({
            "transaction_amount": minor_to_decimal(amount.amount_minor),
            "payer": payer,
            "external_reference": req.idempotency_key,
        });
        if !req.description.is_empty() {
            body["description"] = req.description.as_str().into();
        }

        match &req.method {
            Some(Method::Pix(pix)) => {
                method_hint = pb::PaymentMethod::Pix as i32;
                body["payment_method_id"] = "pix".into();
                if pix.expires_after_seconds > 0 {
                    let exp = chrono::Utc::now()
                        + chrono::Duration::seconds(pix.expires_after_seconds as i64);
                    body["date_of_expiration"] = exp
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                        .into();
                }
            }
            Some(Method::Card(card)) => {
                if card.token.is_empty() {
                    return Err(GatewayError::InvalidRequest(
                        "card.token e obrigatorio (token do MercadoPago.js / CardForm Brick)"
                            .to_string(),
                    )
                    .into());
                }
                method_hint = pb::PaymentMethod::Card as i32;
                body["token"] = card.token.as_str().into();
                body["installments"] = card.installments.max(1).into();
                if let Some(pm) = req.metadata.get("payment_method_id") {
                    body["payment_method_id"] = pm.as_str().into();
                }
                if let Some(issuer) = req.metadata.get("issuer_id") {
                    body["issuer_id"] = issuer.as_str().into();
                }
            }
            None => {
                return Err(GatewayError::InvalidRequest(
                    "method e obrigatorio (card ou pix)".to_string(),
                )
                .into());
            }
        }

        let payment = self
            .client
            .create_payment(&body, &req.idempotency_key)
            .await?;
        let mut mapped = Self::map_payment(&payment, method_hint, req.amount.as_ref());
        if method_hint == pb::PaymentMethod::Pix as i32
            && let Some(details) = Self::pix_details(&payment)
        {
            if mapped.status == pb::PaymentStatus::Pending as i32 {
                mapped.status = pb::PaymentStatus::RequiresAction as i32;
            }
            mapped.pix_details = Some(details);
        }

        Ok(Response::new(pb::CreatePaymentResponse {
            payment: Some(mapped),
        }))
    }

    async fn get_payment(
        &self,
        request: Request<pb::GetPaymentRequest>,
    ) -> Result<Response<pb::GetPaymentResponse>, Status> {
        let req = request.into_inner();
        if req.external_id.is_empty() {
            return Err(
                GatewayError::InvalidRequest("external_id e obrigatorio".to_string()).into(),
            );
        }
        let payment = self.client.get_payment(&req.external_id).await?;
        let mut mapped = Self::map_payment(&payment, pb::PaymentMethod::Unspecified as i32, None);
        if mapped.method == pb::PaymentMethod::Pix as i32 {
            mapped.pix_details = Self::pix_details(&payment);
        }
        Ok(Response::new(pb::GetPaymentResponse {
            payment: Some(mapped),
        }))
    }

    async fn cancel_payment(
        &self,
        request: Request<pb::CancelPaymentRequest>,
    ) -> Result<Response<pb::CancelPaymentResponse>, Status> {
        let req = request.into_inner();
        if req.external_id.is_empty() {
            return Err(
                GatewayError::InvalidRequest("external_id e obrigatorio".to_string()).into(),
            );
        }
        let payment = self.client.cancel_payment(&req.external_id).await?;
        Ok(Response::new(pb::CancelPaymentResponse {
            payment: Some(Self::map_payment(
                &payment,
                pb::PaymentMethod::Unspecified as i32,
                None,
            )),
        }))
    }

    async fn refund_payment(
        &self,
        request: Request<pb::RefundPaymentRequest>,
    ) -> Result<Response<pb::RefundPaymentResponse>, Status> {
        let req = request.into_inner();
        Self::check_idempotency_key(&req.idempotency_key)?;
        if req.external_id.is_empty() {
            return Err(
                GatewayError::InvalidRequest("external_id e obrigatorio".to_string()).into(),
            );
        }
        if req.amount_minor < 0 {
            return Err(GatewayError::InvalidRequest(
                "amount_minor nao pode ser negativo (0 = reembolso total)".to_string(),
            )
            .into());
        }
        let refund = self
            .client
            .refund_payment(&req.external_id, req.amount_minor, &req.idempotency_key)
            .await?;
        let status = if req.amount_minor > 0 {
            pb::PaymentStatus::PartiallyRefunded as i32
        } else {
            pb::PaymentStatus::Refunded as i32
        };
        Ok(Response::new(pb::RefundPaymentResponse {
            refund_id: refund.id.to_string(),
            status,
            amount: Some(pb::Money {
                amount_minor: refund
                    .amount
                    .map(|v| (v * 100.0).round() as i64)
                    .unwrap_or(req.amount_minor),
                currency: "brl".to_string(),
            }),
        }))
    }

    async fn parse_webhook(
        &self,
        request: Request<pb::ParseWebhookRequest>,
    ) -> Result<Response<pb::ParseWebhookResponse>, Status> {
        let req = request.into_inner();
        let event: WebhookEvent = serde_json::from_slice(&req.payload)
            .map_err(|e| GatewayError::Malformed(format!("evento invalido: {e}")))?;
        if let Some(secret) = &self.webhook_secret {
            let data_id = webhook::data_id_string(&event);
            let now_ms = chrono::Utc::now().timestamp_millis();
            webhook::verify_signature(&req.headers, &data_id, secret, now_ms)?;
        }
        let parsed = webhook::normalize(&event);
        let status = match self.client.get_payment(&parsed.external_id).await {
            Ok(p) => Self::payment_status(&p.status),
            Err(_) => webhook::event_status_hint(&event),
        };
        Ok(Response::new(pb::ParseWebhookResponse {
            event_id: parsed.event_id,
            event_type: parsed.event_type,
            external_id: parsed.external_id,
            object_kind: event.kind.unwrap_or_else(|| "payment".to_string()),
            status,
            payload: req.payload,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_payment_statuses() {
        assert_eq!(
            MercadoPagoGateway::payment_status("pending"),
            pb::PaymentStatus::Pending as i32
        );
        assert_eq!(
            MercadoPagoGateway::payment_status("approved"),
            pb::PaymentStatus::Paid as i32
        );
        assert_eq!(
            MercadoPagoGateway::payment_status("authorized"),
            pb::PaymentStatus::Authorized as i32
        );
        assert_eq!(
            MercadoPagoGateway::payment_status("in_process"),
            pb::PaymentStatus::Pending as i32
        );
        assert_eq!(
            MercadoPagoGateway::payment_status("rejected"),
            pb::PaymentStatus::Failed as i32
        );
        assert_eq!(
            MercadoPagoGateway::payment_status("cancelled"),
            pb::PaymentStatus::Canceled as i32
        );
        assert_eq!(
            MercadoPagoGateway::payment_status("refunded"),
            pb::PaymentStatus::Refunded as i32
        );
        assert_eq!(
            MercadoPagoGateway::payment_status("charged_back"),
            pb::PaymentStatus::Failed as i32
        );
    }

    #[test]
    fn converts_minor_to_decimal() {
        assert_eq!(minor_to_decimal(10000), 100.0);
        assert_eq!(minor_to_decimal(1234), 12.34);
    }

    #[test]
    fn splits_names_and_identification() {
        assert_eq!(
            MercadoPagoGateway::split_name("Maria Silva"),
            ("Maria", "Silva")
        );
        assert_eq!(MercadoPagoGateway::split_name("Maria"), ("Maria", "-"));
        assert_eq!(
            MercadoPagoGateway::identification_type("19100000000"),
            "CPF"
        );
        assert_eq!(
            MercadoPagoGateway::identification_type("19100000000100"),
            "CNPJ"
        );
    }

    #[tokio::test]
    async fn create_payment_pix_sends_idempotency_and_reads_qr() {
        use secrecy::SecretString;
        use std::time::Duration;
        use wiremock::matchers::{body_partial_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payments"))
            .and(header("authorization", "Bearer TEST-abc"))
            .and(header("x-idempotency-key", "key-1"))
            .and(body_partial_json(serde_json::json!({
                "transaction_amount": 50.0,
                "payment_method_id": "pix",
                "payer": {
                    "email": "maria@example.com",
                    "first_name": "Maria",
                    "last_name": "Silva",
                    "identification": {"type": "CPF", "number": "19100000000"}
                }
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 999999999,
                "status": "pending",
                "payment_method_id": "pix",
                "transaction_amount": 50.0,
                "date_created": "2025-03-15T15:30:00.000-03:00",
                "point_of_interaction": {
                    "transaction_data": {
                        "qr_code": "emv-pix",
                        "qr_code_base64": "aGVsbG8=",
                        "ticket_url": "https://mpago.la/xyz"
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = MercadoPagoClient::new(
            server.uri(),
            SecretString::new("TEST-abc".into()),
            Duration::from_secs(5),
        )
        .unwrap();
        let gateway = MercadoPagoGateway::new(client, None);

        let resp = gateway
            .create_payment(Request::new(pb::CreatePaymentRequest {
                idempotency_key: "key-1".into(),
                amount: Some(pb::Money {
                    amount_minor: 5000,
                    currency: "brl".into(),
                }),
                customer: Some(pb::Customer {
                    name: "Maria Silva".into(),
                    email: "maria@example.com".into(),
                    tax_id: "19100000000".into(),
                    phone: String::new(),
                    ip: String::new(),
                }),
                method: Some(Method::Pix(pb::PixMethod {
                    expires_after_seconds: 3600,
                })),
                return_url: String::new(),
                description: "Pedido 1".into(),
                metadata: Default::default(),
            }))
            .await
            .unwrap()
            .into_inner();

        let payment = resp.payment.unwrap();
        assert_eq!(payment.external_id, "999999999");
        assert_eq!(payment.status, pb::PaymentStatus::RequiresAction as i32);
        assert_eq!(payment.method, pb::PaymentMethod::Pix as i32);
        let pix = payment.pix_details.unwrap();
        assert_eq!(pix.emv_qr_code, "emv-pix");
        assert_eq!(pix.qr_code_png_base64, "aGVsbG8=");
        assert_eq!(pix.hosted_instructions_url, "https://mpago.la/xyz");
    }
}
