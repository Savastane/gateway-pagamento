use crate::client::AppmaxClient;
use crate::error::GatewayError;
use crate::models::*;
use crate::webhook;
use chrono::TimeZone;
use gateway_proto::payment_gateway::v1 as pb;
use pb::create_payment_request::Method;
use pb::payment_gateway_server::PaymentGateway;
use tonic::{Request, Response, Status};

pub struct AppmaxGateway {
    client: AppmaxClient,
}

impl AppmaxGateway {
    pub fn new(client: AppmaxClient) -> Self {
        Self { client }
    }

    fn check_idempotency_key(key: &str) -> Result<(), GatewayError> {
        if key.is_empty() || key.len() > 255 {
            return Err(GatewayError::InvalidRequest(
                "idempotency_key e obrigatoria (1..255 chars); a appmax nao tem idempotencia nativa, entao a deduplicacao deve ser garantida pelo chamador".to_string(),
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

    pub fn order_status(status: &str) -> i32 {
        match status {
            "pendente" => pb::PaymentStatus::Pending as i32,
            "autorizado" => pb::PaymentStatus::Authorized as i32,
            "aprovado" | "integrado" | "pendente_integracao" | "pendente_integracao_em_analise" => {
                pb::PaymentStatus::Paid as i32
            }
            "cancelado" => pb::PaymentStatus::Canceled as i32,
            "estornado" | "recusado_por_risco" => pb::PaymentStatus::Refunded as i32,
            "chargeback_vencido" => pb::PaymentStatus::Paid as i32,
            s if s.starts_with("chargeback_") => pb::PaymentStatus::Failed as i32,
            _ => pb::PaymentStatus::Unspecified as i32,
        }
    }

    fn map_order(order: &OrderObj, method: i32, amount: Option<&pb::Money>) -> pb::Payment {
        let paid_amount = order
            .total_paid
            .or_else(|| order.amounts.as_ref().map(|a| a.sub_total));
        pb::Payment {
            external_id: order.id.to_string(),
            status: Self::order_status(&order.status),
            amount: amount.cloned().or_else(|| {
                paid_amount.map(|v| pb::Money {
                    amount_minor: v,
                    currency: "brl".to_string(),
                })
            }),
            method,
            ..Default::default()
        }
    }

    fn parse_sao_paulo_ts(value: &str) -> i64 {
        chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
            .ok()
            .and_then(|naive| {
                chrono_tz::America::Sao_Paulo
                    .from_local_datetime(&naive)
                    .single()
            })
            .map(|dt| dt.timestamp())
            .unwrap_or_default()
    }

    fn pix_details(data: &PaymentResponse) -> Option<pb::PixDetails> {
        let from_payment = data.payment.as_ref();
        let from_pix = data.pix.as_ref();

        let emv = from_payment
            .and_then(|p| p.pix_emv.clone())
            .or_else(|| from_pix.and_then(|p| p.emv_code.clone()))?;
        let qr_base64 = from_payment
            .and_then(|p| p.pix_qrcode.clone())
            .or_else(|| from_pix.and_then(|p| p.qr_code.clone()))
            .map(|qr| {
                qr.strip_prefix("data:image/png;base64,")
                    .map(str::to_string)
                    .unwrap_or(qr)
            })
            .unwrap_or_default();
        let expires_at = from_payment
            .and_then(|p| p.pix_expiration_date.clone())
            .or_else(|| from_pix.and_then(|p| p.expires_at.clone()))
            .map(|ts| Self::parse_sao_paulo_ts(&ts))
            .unwrap_or_default();

        Some(pb::PixDetails {
            emv_qr_code: emv,
            qr_code_png_base64: qr_base64,
            expires_at,
            ..Default::default()
        })
    }

    fn sao_paulo_expiration(expires_after_seconds: i32) -> String {
        let future = chrono::Utc::now() + chrono::Duration::seconds(expires_after_seconds as i64);
        future
            .with_timezone(&chrono_tz::America::Sao_Paulo)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }
}

#[tonic::async_trait]
impl PaymentGateway for AppmaxGateway {
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
                GatewayError::InvalidRequest("a appmax opera somente BRL".to_string()).into(),
            );
        }

        let customer = req.customer.as_ref().ok_or_else(|| {
            GatewayError::InvalidRequest(
                "customer (name, email, phone, ip) e obrigatorio na appmax".to_string(),
            )
        })?;
        if customer.name.is_empty()
            || customer.email.is_empty()
            || customer.phone.is_empty()
            || customer.ip.is_empty()
        {
            return Err(GatewayError::InvalidRequest(
                "customer.name, customer.email, customer.phone e customer.ip sao obrigatorios na appmax"
                    .to_string(),
            )
            .into());
        }

        let (first_name, last_name) = Self::split_name(&customer.name);
        let customer_obj = self
            .client
            .create_customer(&NewCustomer {
                first_name,
                last_name,
                email: &customer.email,
                phone: &customer.phone,
                ip: &customer.ip,
                document_number: (!customer.tax_id.is_empty()).then_some(customer.tax_id.as_str()),
            })
            .await?;

        let product_name = if req.description.is_empty() {
            "Pagamento"
        } else {
            req.description.as_str()
        };
        let product_sku = req
            .metadata
            .get("product_sku")
            .map(String::as_str)
            .unwrap_or("PAYMENT");
        let order = self
            .client
            .create_order(&NewOrder {
                customer_id: customer_obj.id,
                products: vec![NewProduct {
                    sku: product_sku,
                    name: product_name,
                    quantity: 1,
                    unit_value: amount.amount_minor,
                    kind: "digital",
                }],
                products_value: amount.amount_minor,
            })
            .await?;

        let method_hint;
        let response = match &req.method {
            Some(Method::Card(card)) => {
                if card.token.is_empty() {
                    return Err(GatewayError::InvalidRequest(
                        "card.token e obrigatorio (use o Appmax JS para tokenizar no cliente)"
                            .to_string(),
                    )
                    .into());
                }
                if customer.tax_id.is_empty() {
                    return Err(GatewayError::InvalidRequest(
                        "customer.tax_id e obrigatorio para pagamento com cartao na appmax"
                            .to_string(),
                    )
                    .into());
                }
                method_hint = pb::PaymentMethod::Card as i32;
                let soft_descriptor = req.metadata.get("soft_descriptor").map(String::as_str);
                self.client
                    .pay_card(
                        order.id,
                        customer_obj.id,
                        &crate::client::CardCharge {
                            token: &card.token,
                            holder_name: &customer.name,
                            holder_document_number: &customer.tax_id,
                            installments: card.installments,
                            soft_descriptor,
                        },
                    )
                    .await?
            }
            Some(Method::Pix(pix)) => {
                method_hint = pb::PaymentMethod::Pix as i32;
                let expiration = (pix.expires_after_seconds > 0)
                    .then(|| Self::sao_paulo_expiration(pix.expires_after_seconds));
                self.client
                    .pay_pix(order.id, &customer.tax_id, expiration.as_deref())
                    .await?
            }
            None => {
                return Err(GatewayError::InvalidRequest(
                    "method e obrigatorio (card ou pix)".to_string(),
                )
                .into());
            }
        };

        let pix_details = if method_hint == pb::PaymentMethod::Pix as i32 {
            Self::pix_details(&response)
        } else {
            None
        };
        let order = response.order.unwrap_or(order);
        let mut payment = Self::map_order(&order, method_hint, req.amount.as_ref());
        payment.pix_details = pix_details;

        Ok(Response::new(pb::CreatePaymentResponse {
            payment: Some(payment),
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
        let detail = self.client.get_order(&req.external_id).await?;
        let method = detail
            .payment
            .as_ref()
            .and_then(|p| p.method.as_deref())
            .map(|m| match m {
                "creditcard" => pb::PaymentMethod::Card as i32,
                "pix" => pb::PaymentMethod::Pix as i32,
                _ => pb::PaymentMethod::Unspecified as i32,
            })
            .unwrap_or(pb::PaymentMethod::Unspecified as i32);
        Ok(Response::new(pb::GetPaymentResponse {
            payment: Some(Self::map_order(&detail.order, method, None)),
        }))
    }

    async fn cancel_payment(
        &self,
        _request: Request<pb::CancelPaymentRequest>,
    ) -> Result<Response<pb::CancelPaymentResponse>, Status> {
        Err(Status::unimplemented(
            "a appmax nao oferece cancelamento via API; o pedido expira sozinho",
        ))
    }

    async fn refund_payment(
        &self,
        request: Request<pb::RefundPaymentRequest>,
    ) -> Result<Response<pb::RefundPaymentResponse>, Status> {
        let req = request.into_inner();
        Self::check_idempotency_key(&req.idempotency_key)?;
        if req.amount_minor < 0 {
            return Err(GatewayError::InvalidRequest(
                "amount_minor nao pode ser negativo (0 = reembolso total)".to_string(),
            )
            .into());
        }
        let order_id: i64 = req.external_id.parse().map_err(|_| {
            GatewayError::InvalidRequest("external_id deve ser o id numerico do pedido".to_string())
        })?;
        self.client.refund_order(order_id, req.amount_minor).await?;
        Ok(Response::new(pb::RefundPaymentResponse {
            refund_id: format!("refund:{order_id}"),
            status: pb::PaymentStatus::Pending as i32,
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
        let event: WebhookEvent = serde_json::from_slice(&req.payload)
            .map_err(|e| GatewayError::Malformed(format!("evento invalido: {e}")))?;
        let parsed = webhook::normalize(&event);
        Ok(Response::new(pb::ParseWebhookResponse {
            event_id: parsed.event_id,
            event_type: event.event.clone(),
            external_id: parsed.external_id,
            object_kind: "order".to_string(),
            status: parsed.status,
            payload: req.payload,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_names() {
        assert_eq!(AppmaxGateway::split_name("Maria Silva"), ("Maria", "Silva"));
        assert_eq!(
            AppmaxGateway::split_name("Maria Clara Souza"),
            ("Maria", "Clara Souza")
        );
        assert_eq!(AppmaxGateway::split_name("Maria"), ("Maria", "-"));
    }

    #[test]
    fn maps_order_statuses() {
        assert_eq!(
            AppmaxGateway::order_status("pendente"),
            pb::PaymentStatus::Pending as i32
        );
        assert_eq!(
            AppmaxGateway::order_status("aprovado"),
            pb::PaymentStatus::Paid as i32
        );
        assert_eq!(
            AppmaxGateway::order_status("integrado"),
            pb::PaymentStatus::Paid as i32
        );
        assert_eq!(
            AppmaxGateway::order_status("autorizado"),
            pb::PaymentStatus::Authorized as i32
        );
        assert_eq!(
            AppmaxGateway::order_status("estornado"),
            pb::PaymentStatus::Refunded as i32
        );
        assert_eq!(
            AppmaxGateway::order_status("chargeback_em_disputa"),
            pb::PaymentStatus::Failed as i32
        );
        assert_eq!(
            AppmaxGateway::order_status("chargeback_vencido"),
            pb::PaymentStatus::Paid as i32
        );
    }

    #[test]
    fn parses_sao_paulo_timestamps() {
        let ts = AppmaxGateway::parse_sao_paulo_ts("2025-03-15 15:30:00");
        assert!(ts > 0);
        assert_eq!(AppmaxGateway::parse_sao_paulo_ts("invalido"), 0);
    }

    #[test]
    fn tolerates_both_pix_response_shapes() {
        let reference_shape: PaymentResponse = serde_json::from_value(serde_json::json!({
            "order": {"id": 1, "status": "pendente"},
            "payment": {
                "pay_reference": "ref",
                "pix_qrcode": "aGVsbG8=",
                "pix_emv": "emv1",
                "pix_expiration_date": "2025-03-15 15:30:00"
            }
        }))
        .unwrap();
        let details = AppmaxGateway::pix_details(&reference_shape).unwrap();
        assert_eq!(details.emv_qr_code, "emv1");
        assert_eq!(details.qr_code_png_base64, "aGVsbG8=");
        assert!(details.expires_at > 0);

        let guide_shape: PaymentResponse = serde_json::from_value(serde_json::json!({
            "order": {"id": 2, "status": "pendente"},
            "pix": {
                "qr_code": "data:image/png;base64,aGVsbG8=",
                "emv_code": "emv2",
                "expires_at": "2025-03-15 15:30:00"
            }
        }))
        .unwrap();
        let details = AppmaxGateway::pix_details(&guide_shape).unwrap();
        assert_eq!(details.emv_qr_code, "emv2");
        assert_eq!(details.qr_code_png_base64, "aGVsbG8=");
    }

    #[tokio::test]
    async fn create_payment_pix_runs_full_appmax_flow() {
        use secrecy::SecretString;
        use std::time::Duration;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "tok-1", "token_type": "Bearer", "expires_in": 3600
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/customers"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "data": {"customer": {"id": 2023}}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/orders"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "data": {"order": {"id": 3531, "status": "pendente"}}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/payments/pix"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "order": {"id": 3531, "status": "pendente"},
                    "pix": {
                        "qr_code": "data:image/png;base64,aGVsbG8=",
                        "emv_code": "emv-pix",
                        "expires_at": "2025-03-15 15:30:00"
                    }
                }
            })))
            .mount(&server)
            .await;

        let client = crate::client::AppmaxClient::new(
            server.uri(),
            server.uri(),
            "client-id",
            SecretString::new("secret".into()),
            Duration::from_secs(5),
        )
        .unwrap();
        let gateway = AppmaxGateway::new(client);

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
        assert_eq!(payment.external_id, "3531");
        assert_eq!(payment.status, pb::PaymentStatus::Pending as i32);
        assert_eq!(payment.method, pb::PaymentMethod::Pix as i32);
        let pix = payment.pix_details.unwrap();
        assert_eq!(pix.emv_qr_code, "emv-pix");
        assert_eq!(pix.qr_code_png_base64, "aGVsbG8=");
        assert!(pix.expires_at > 0);
    }
}
