use crate::client::AsaasClient;
use crate::error::GatewayError;
use crate::models::*;
use crate::webhook;
use chrono::TimeZone;
use gateway_proto::payment_gateway::v1 as pb;
use pb::create_payment_request::Method;
use pb::payment_gateway_server::PaymentGateway;
use secrecy::SecretString;
use tonic::{Request, Response, Status};

pub fn minor_to_decimal(minor: i64) -> f64 {
    minor as f64 / 100.0
}

pub struct AsaasGateway {
    client: AsaasClient,
    webhook_token: Option<SecretString>,
}

impl AsaasGateway {
    pub fn new(client: AsaasClient, webhook_token: Option<SecretString>) -> Self {
        Self {
            client,
            webhook_token,
        }
    }

    fn check_idempotency_key(key: &str) -> Result<(), GatewayError> {
        if key.is_empty() || key.len() > 255 {
            return Err(GatewayError::InvalidRequest(
                "idempotency_key e obrigatoria (1..255 chars); o asaas nao tem idempotencia nativa, entao a deduplicacao deve ser garantida pelo chamador".to_string(),
            ));
        }
        Ok(())
    }

    pub fn payment_status(status: &str, deleted: bool) -> i32 {
        if deleted {
            return pb::PaymentStatus::Canceled as i32;
        }
        match status {
            "PENDING" | "AWAITING_RISK_ANALYSIS" => pb::PaymentStatus::Pending as i32,
            "AUTHORIZED" => pb::PaymentStatus::Authorized as i32,
            "CONFIRMED" | "RECEIVED" | "RECEIVED_IN_CASH" | "DUNNING_RECEIVED" => {
                pb::PaymentStatus::Paid as i32
            }
            "OVERDUE" => pb::PaymentStatus::Expired as i32,
            "REFUNDED" => pb::PaymentStatus::Refunded as i32,
            "REFUND_REQUESTED" | "REFUND_IN_PROGRESS" => {
                pb::PaymentStatus::PartiallyRefunded as i32
            }
            "CHARGEBACK_REQUESTED"
            | "CHARGEBACK_DISPUTE"
            | "AWAITING_CHARGEBACK_REVERSAL"
            | "DUNNING_REQUESTED" => pb::PaymentStatus::Failed as i32,
            _ => pb::PaymentStatus::Unspecified as i32,
        }
    }

    fn map_payment(payment: &Payment, method_hint: i32, amount: Option<&pb::Money>) -> pb::Payment {
        let method = payment
            .billing_type
            .as_deref()
            .map(|b| match b {
                "CREDIT_CARD" => pb::PaymentMethod::Card as i32,
                "PIX" => pb::PaymentMethod::Pix as i32,
                _ => method_hint,
            })
            .unwrap_or(method_hint);
        let created_at = payment
            .date_created
            .as_deref()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| dt.and_utc().timestamp())
            .unwrap_or_default();
        pb::Payment {
            external_id: payment.id.clone(),
            status: Self::payment_status(&payment.status, payment.deleted),
            amount: amount.cloned().or_else(|| {
                payment.value.map(|v| pb::Money {
                    amount_minor: (v * 100.0).round() as i64,
                    currency: "brl".to_string(),
                })
            }),
            method,
            created_at,
            ..Default::default()
        }
    }

    fn parse_expiration(value: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(value)
            .map(|dt| dt.timestamp())
            .ok()
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .and_then(|naive| {
                        chrono_tz::America::Sao_Paulo
                            .from_local_datetime(&naive)
                            .single()
                    })
                    .map(|dt| dt.timestamp())
            })
            .unwrap_or_default()
    }

    fn due_date(expires_after_seconds: i32) -> String {
        let due =
            chrono::Utc::now() + chrono::Duration::seconds(expires_after_seconds.max(0) as i64);
        due.format("%Y-%m-%d").to_string()
    }
}

#[tonic::async_trait]
impl PaymentGateway for AsaasGateway {
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
            return Err(
                GatewayError::InvalidRequest("o asaas opera somente BRL".to_string()).into(),
            );
        }

        let customer = req.customer.as_ref().ok_or_else(|| {
            GatewayError::InvalidRequest("customer e obrigatorio no asaas".to_string())
        })?;
        if customer.name.is_empty() || customer.tax_id.is_empty() {
            return Err(GatewayError::InvalidRequest(
                "customer.name e customer.tax_id (cpfCnpj) sao obrigatorios no asaas".to_string(),
            )
            .into());
        }

        let customer_obj = match self
            .client
            .find_customer_by_document(&customer.tax_id)
            .await?
        {
            Some(existing) => existing,
            None => {
                self.client
                    .create_customer(
                        &customer.name,
                        &customer.tax_id,
                        &customer.email,
                        &customer.phone,
                    )
                    .await?
            }
        };

        let mut body = serde_json::json!({
            "customer": customer_obj.id,
            "dueDate": Self::due_date(match &req.method {
                Some(Method::Pix(pix)) => pix.expires_after_seconds,
                _ => 0,
            }),
            "externalReference": req.idempotency_key,
        });
        if !req.description.is_empty() {
            body["description"] = req.description.as_str().into();
        }

        let method_hint;
        match &req.method {
            Some(Method::Pix(_)) => {
                method_hint = pb::PaymentMethod::Pix as i32;
                body["billingType"] = "PIX".into();
                body["value"] = minor_to_decimal(amount.amount_minor).into();
            }
            Some(Method::Card(card)) => {
                if card.token.is_empty() {
                    return Err(GatewayError::InvalidRequest(
                        "card.token e obrigatorio (creditCardToken; requer tokenizacao habilitada na conta)"
                            .to_string(),
                    )
                    .into());
                }
                if customer.ip.is_empty() {
                    return Err(GatewayError::InvalidRequest(
                        "customer.ip e obrigatorio para pagamento com cartao no asaas (remoteIp; IP do pagador, nao do servidor)"
                            .to_string(),
                    )
                    .into());
                }
                method_hint = pb::PaymentMethod::Card as i32;
                body["billingType"] = "CREDIT_CARD".into();
                body["creditCardToken"] = card.token.as_str().into();
                body["remoteIp"] = customer.ip.as_str().into();
                if card.installments > 1 {
                    body["installmentCount"] = card.installments.into();
                    body["totalValue"] = minor_to_decimal(amount.amount_minor).into();
                } else {
                    body["value"] = minor_to_decimal(amount.amount_minor).into();
                }
            }
            None => {
                return Err(GatewayError::InvalidRequest(
                    "method e obrigatorio (card ou pix)".to_string(),
                )
                .into());
            }
        }

        let payment = self.client.create_payment(&body).await?;
        let mut mapped = Self::map_payment(&payment, method_hint, req.amount.as_ref());

        if method_hint == pb::PaymentMethod::Pix as i32
            && matches!(
                mapped.status,
                s if s == pb::PaymentStatus::Pending as i32
                    || s == pb::PaymentStatus::RequiresAction as i32
            )
        {
            let qr = self.client.pix_qr_code(&payment.id).await?;
            if mapped.status == pb::PaymentStatus::Pending as i32 {
                mapped.status = pb::PaymentStatus::RequiresAction as i32;
            }
            mapped.pix_details = Some(pb::PixDetails {
                emv_qr_code: qr.payload,
                qr_code_png_base64: qr.encoded_image,
                expires_at: qr
                    .expiration_date
                    .as_deref()
                    .map(Self::parse_expiration)
                    .unwrap_or_default(),
                ..Default::default()
            });
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
        Ok(Response::new(pb::GetPaymentResponse {
            payment: Some(Self::map_payment(
                &payment,
                pb::PaymentMethod::Unspecified as i32,
                None,
            )),
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
        self.client.delete_payment(&req.external_id).await?;
        let payment = match self.client.get_payment(&req.external_id).await {
            Ok(mut p) => {
                p.deleted = true;
                Self::map_payment(&p, pb::PaymentMethod::Unspecified as i32, None)
            }
            Err(_) => pb::Payment {
                external_id: req.external_id.clone(),
                status: pb::PaymentStatus::Canceled as i32,
                ..Default::default()
            },
        };
        Ok(Response::new(pb::CancelPaymentResponse {
            payment: Some(payment),
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
        let payment = self
            .client
            .refund_payment(&req.external_id, req.amount_minor)
            .await?;
        Ok(Response::new(pb::RefundPaymentResponse {
            refund_id: format!("refund:{}", req.external_id),
            status: Self::payment_status(&payment.status, payment.deleted),
            amount: Some(pb::Money {
                amount_minor: req.amount_minor,
                currency: "brl".to_string(),
            }),
        }))
    }

    async fn parse_webhook(
        &self,
        request: Request<pb::ParseWebhookRequest>,
    ) -> Result<Response<pb::ParseWebhookResponse>, Status> {
        let req = request.into_inner();
        if let Some(token) = &self.webhook_token {
            webhook::verify_access_token(&req.headers, token)?;
        }
        let event: WebhookEvent = serde_json::from_slice(&req.payload)
            .map_err(|e| GatewayError::Malformed(format!("evento invalido: {e}")))?;
        let parsed = webhook::normalize(&event);
        Ok(Response::new(pb::ParseWebhookResponse {
            event_id: parsed.event_id,
            event_type: event.event.clone(),
            external_id: parsed.external_id,
            object_kind: "payment".to_string(),
            status: parsed.status,
            payload: req.payload,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_payment_statuses() {
        let s = |st: &str| AsaasGateway::payment_status(st, false);
        assert_eq!(s("PENDING"), pb::PaymentStatus::Pending as i32);
        assert_eq!(s("AUTHORIZED"), pb::PaymentStatus::Authorized as i32);
        assert_eq!(s("CONFIRMED"), pb::PaymentStatus::Paid as i32);
        assert_eq!(s("RECEIVED"), pb::PaymentStatus::Paid as i32);
        assert_eq!(s("OVERDUE"), pb::PaymentStatus::Expired as i32);
        assert_eq!(s("REFUNDED"), pb::PaymentStatus::Refunded as i32);
        assert_eq!(
            s("REFUND_IN_PROGRESS"),
            pb::PaymentStatus::PartiallyRefunded as i32
        );
        assert_eq!(s("CHARGEBACK_REQUESTED"), pb::PaymentStatus::Failed as i32);
        assert_eq!(
            AsaasGateway::payment_status("PENDING", true),
            pb::PaymentStatus::Canceled as i32
        );
    }

    #[test]
    fn converts_minor_to_decimal() {
        assert_eq!(minor_to_decimal(12990), 129.9);
        assert_eq!(minor_to_decimal(50), 0.5);
        assert_eq!(
            serde_json::json!({"value": minor_to_decimal(12990)}).to_string(),
            r#"{"value":129.9}"#
        );
    }

    #[test]
    fn parses_expiration_formats() {
        assert!(AsaasGateway::parse_expiration("2025-03-15T15:30:00-03:00") > 0);
        assert!(AsaasGateway::parse_expiration("2025-03-15 15:30:00") > 0);
        assert_eq!(AsaasGateway::parse_expiration("invalido"), 0);
    }

    #[tokio::test]
    async fn create_payment_pix_runs_full_asaas_flow() {
        use secrecy::SecretString;
        use std::time::Duration;
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/customers"))
            .and(query_param("cpfCnpj", "19100000000"))
            .and(header("access_token", "$aact_test_key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/customers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "cus_000001"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "pay_123", "status": "PENDING", "billingType": "PIX",
                "value": 50.0, "dateCreated": "2025-03-15", "deleted": false
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/payments/pay_123/pixQrCode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "encodedImage": "aGVsbG8=",
                "payload": "emv-pix",
                "expirationDate": "2025-03-15T18:00:00-03:00"
            })))
            .mount(&server)
            .await;

        let client = AsaasClient::new(
            server.uri(),
            SecretString::new("$aact_test_key".into()),
            Duration::from_secs(5),
        )
        .unwrap();
        let gateway = AsaasGateway::new(client, None);

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
                    phone: "51999999999".into(),
                    ip: "127.0.0.1".into(),
                }),
                method: Some(Method::Pix(pb::PixMethod {
                    expires_after_seconds: 0,
                })),
                return_url: String::new(),
                description: "Pedido 1".into(),
                metadata: Default::default(),
            }))
            .await
            .unwrap()
            .into_inner();

        let payment = resp.payment.unwrap();
        assert_eq!(payment.external_id, "pay_123");
        assert_eq!(payment.status, pb::PaymentStatus::RequiresAction as i32);
        assert_eq!(payment.method, pb::PaymentMethod::Pix as i32);
        let pix = payment.pix_details.unwrap();
        assert_eq!(pix.emv_qr_code, "emv-pix");
        assert_eq!(pix.qr_code_png_base64, "aGVsbG8=");
        assert!(pix.expires_at > 0);
    }

    #[tokio::test]
    async fn create_payment_card_sends_installments_and_remote_ip() {
        use secrecy::SecretString;
        use std::time::Duration;
        use wiremock::matchers::{body_partial_json, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/customers"))
            .and(query_param("cpfCnpj", "19100000000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "cus_000001"}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payments"))
            .and(body_partial_json(serde_json::json!({
                "billingType": "CREDIT_CARD",
                "creditCardToken": "tok_asaas",
                "remoteIp": "177.0.0.1",
                "installmentCount": 3,
                "totalValue": 150.0,
                "customer": "cus_000001"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "pay_456", "status": "CONFIRMED", "billingType": "CREDIT_CARD",
                "value": 150.0, "dateCreated": "2025-03-15"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = AsaasClient::new(
            server.uri(),
            SecretString::new("$aact_test_key".into()),
            Duration::from_secs(5),
        )
        .unwrap();
        let gateway = AsaasGateway::new(client, None);

        let resp = gateway
            .create_payment(Request::new(pb::CreatePaymentRequest {
                idempotency_key: "key-2".into(),
                amount: Some(pb::Money {
                    amount_minor: 15000,
                    currency: "brl".into(),
                }),
                customer: Some(pb::Customer {
                    name: "Maria Silva".into(),
                    email: "maria@example.com".into(),
                    tax_id: "19100000000".into(),
                    phone: String::new(),
                    ip: "177.0.0.1".into(),
                }),
                method: Some(Method::Card(pb::CardMethod {
                    token: "tok_asaas".into(),
                    installments: 3,
                })),
                return_url: String::new(),
                description: String::new(),
                metadata: Default::default(),
            }))
            .await
            .unwrap()
            .into_inner();

        let payment = resp.payment.unwrap();
        assert_eq!(payment.external_id, "pay_456");
        assert_eq!(payment.status, pb::PaymentStatus::Paid as i32);
        assert_eq!(payment.method, pb::PaymentMethod::Card as i32);
    }
}
