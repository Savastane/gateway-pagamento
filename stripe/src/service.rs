use crate::client::StripeClient;
use crate::error::GatewayError;
use crate::models::{Event, PaymentIntent, Refund};
use crate::webhook;
use gateway_proto::payment_gateway::v1 as pb;
use pb::create_payment_request::Method;
use pb::payment_gateway_server::PaymentGateway;
use secrecy::{ExposeSecret, SecretString};
use std::time::{SystemTime, UNIX_EPOCH};
use tonic::{Request, Response, Status};

const PIX_EXPIRATION_MIN_SECS: i32 = 60;
const PIX_EXPIRATION_MAX_SECS: i32 = 1_209_600;

pub struct StripeGateway {
    client: StripeClient,
    webhook_secret: Option<SecretString>,
}

impl StripeGateway {
    pub fn new(client: StripeClient, webhook_secret: Option<SecretString>) -> Self {
        Self {
            client,
            webhook_secret,
        }
    }

    fn check_idempotency_key(key: &str) -> Result<(), GatewayError> {
        if key.is_empty() || key.len() > 255 {
            return Err(GatewayError::InvalidRequest(
                "idempotency_key e obrigatoria (1..255 chars)".to_string(),
            ));
        }
        Ok(())
    }

    fn build_create_params(
        req: &pb::CreatePaymentRequest,
    ) -> Result<Vec<(String, String)>, GatewayError> {
        Self::check_idempotency_key(&req.idempotency_key)?;
        let amount = req
            .amount
            .as_ref()
            .ok_or_else(|| GatewayError::InvalidRequest("amount e obrigatorio".to_string()))?;
        if amount.amount_minor <= 0 {
            return Err(GatewayError::InvalidRequest(
                "amount_minor deve ser positivo".to_string(),
            ));
        }
        if amount.currency.len() != 3 {
            return Err(GatewayError::InvalidRequest(
                "currency deve ser um codigo ISO 4217 de 3 letras".to_string(),
            ));
        }

        let mut params = vec![
            ("amount".to_string(), amount.amount_minor.to_string()),
            ("currency".to_string(), amount.currency.to_lowercase()),
            ("confirm".to_string(), "true".to_string()),
        ];

        let customer = req.customer.as_ref();
        match &req.method {
            Some(Method::Card(card)) => {
                if card.token.is_empty() {
                    return Err(GatewayError::InvalidRequest(
                        "card.token e obrigatorio (use o SDK do Stripe no cliente)".to_string(),
                    ));
                }
                params.push(("payment_method".to_string(), card.token.clone()));
                params.push((
                    "automatic_payment_methods[enabled]".to_string(),
                    "true".to_string(),
                ));
                params.push((
                    "automatic_payment_methods[allow_redirects]".to_string(),
                    "never".to_string(),
                ));
            }
            Some(Method::Pix(pix)) => {
                let customer = customer.ok_or_else(|| {
                    GatewayError::InvalidRequest(
                        "customer (name, email, tax_id) e obrigatorio para PIX".to_string(),
                    )
                })?;
                if customer.name.is_empty()
                    || customer.email.is_empty()
                    || customer.tax_id.is_empty()
                {
                    return Err(GatewayError::InvalidRequest(
                        "customer.name, customer.email e customer.tax_id sao obrigatorios para PIX"
                            .to_string(),
                    ));
                }
                if req.return_url.is_empty() {
                    return Err(GatewayError::InvalidRequest(
                        "return_url e obrigatorio para PIX".to_string(),
                    ));
                }
                if pix.expires_after_seconds != 0
                    && !(PIX_EXPIRATION_MIN_SECS..=PIX_EXPIRATION_MAX_SECS)
                        .contains(&pix.expires_after_seconds)
                {
                    return Err(GatewayError::InvalidRequest(format!(
                        "pix.expires_after_seconds deve estar entre {PIX_EXPIRATION_MIN_SECS} e {PIX_EXPIRATION_MAX_SECS}"
                    )));
                }
                params.push(("payment_method_data[type]".to_string(), "pix".to_string()));
                params.push((
                    "payment_method_data[billing_details][name]".to_string(),
                    customer.name.clone(),
                ));
                params.push((
                    "payment_method_data[billing_details][email]".to_string(),
                    customer.email.clone(),
                ));
                params.push((
                    "payment_method_data[billing_details][tax_id]".to_string(),
                    customer.tax_id.clone(),
                ));
                params.push(("return_url".to_string(), req.return_url.clone()));
                if pix.expires_after_seconds > 0 {
                    params.push((
                        "payment_method_options[pix][expires_after_seconds]".to_string(),
                        pix.expires_after_seconds.to_string(),
                    ));
                }
            }
            None => {
                return Err(GatewayError::InvalidRequest(
                    "method e obrigatorio (card ou pix)".to_string(),
                ));
            }
        }

        if !req.description.is_empty() {
            params.push(("description".to_string(), req.description.clone()));
        }
        for (key, value) in &req.metadata {
            params.push((format!("metadata[{key}]"), value.clone()));
        }
        Ok(params)
    }

    fn intent_status(status: &str) -> i32 {
        match status {
            "requires_payment_method" => pb::PaymentStatus::Failed as i32,
            "requires_confirmation" => pb::PaymentStatus::Pending as i32,
            "requires_action" => pb::PaymentStatus::RequiresAction as i32,
            "processing" => pb::PaymentStatus::Pending as i32,
            "requires_capture" => pb::PaymentStatus::Authorized as i32,
            "succeeded" => pb::PaymentStatus::Paid as i32,
            "canceled" => pb::PaymentStatus::Canceled as i32,
            _ => pb::PaymentStatus::Unspecified as i32,
        }
    }

    fn refund_status(status: &str) -> i32 {
        match status {
            "pending" => pb::PaymentStatus::Pending as i32,
            "requires_action" => pb::PaymentStatus::RequiresAction as i32,
            "succeeded" => pb::PaymentStatus::Refunded as i32,
            "failed" => pb::PaymentStatus::Failed as i32,
            "canceled" => pb::PaymentStatus::Canceled as i32,
            _ => pb::PaymentStatus::Unspecified as i32,
        }
    }

    fn map_payment(pi: &PaymentIntent, method_hint: Option<pb::PaymentMethod>) -> pb::Payment {
        let expanded_type = pi
            .payment_method
            .as_ref()
            .and_then(|pm| pm.get("type"))
            .and_then(|t| t.as_str());
        let method = method_hint
            .or(match expanded_type {
                Some("pix") => Some(pb::PaymentMethod::Pix),
                Some("card") => Some(pb::PaymentMethod::Card),
                _ => None,
            })
            .or_else(|| {
                if pi.payment_method_types.iter().any(|t| t == "card") {
                    Some(pb::PaymentMethod::Card)
                } else if pi.payment_method_types.iter().any(|t| t == "pix") {
                    Some(pb::PaymentMethod::Pix)
                } else {
                    None
                }
            })
            .map(|m| m as i32)
            .unwrap_or(pb::PaymentMethod::Unspecified as i32);
        let pix_details = pi
            .next_action
            .as_ref()
            .and_then(|na| na.pix_display_qr_code.as_ref())
            .map(|qr| pb::PixDetails {
                emv_qr_code: qr.data.clone().unwrap_or_default(),
                image_url_png: qr.image_url_png.clone().unwrap_or_default(),
                image_url_svg: qr.image_url_svg.clone().unwrap_or_default(),
                hosted_instructions_url: qr.hosted_instructions_url.clone().unwrap_or_default(),
                expires_at: qr.expires_at.unwrap_or_default(),
                ..Default::default()
            });
        pb::Payment {
            external_id: pi.id.clone(),
            status: Self::intent_status(&pi.status),
            amount: Some(pb::Money {
                amount_minor: pi.amount,
                currency: pi.currency.clone(),
            }),
            method,
            pix_details,
            client_secret: pi.client_secret.clone().unwrap_or_default(),
            created_at: pi.created.unwrap_or_default(),
        }
    }
}

#[tonic::async_trait]
impl PaymentGateway for StripeGateway {
    async fn create_payment(
        &self,
        request: Request<pb::CreatePaymentRequest>,
    ) -> Result<Response<pb::CreatePaymentResponse>, Status> {
        let req = request.into_inner();
        let params = Self::build_create_params(&req)?;
        let method_hint = req.method.as_ref().map(|m| match m {
            Method::Card(_) => pb::PaymentMethod::Card,
            Method::Pix(_) => pb::PaymentMethod::Pix,
        });
        let pi = self
            .client
            .create_payment_intent(&params, &req.idempotency_key)
            .await?;
        Ok(Response::new(pb::CreatePaymentResponse {
            payment: Some(Self::map_payment(&pi, method_hint)),
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
        let pi = self
            .client
            .retrieve_payment_intent(&req.external_id)
            .await?;
        Ok(Response::new(pb::GetPaymentResponse {
            payment: Some(Self::map_payment(&pi, None)),
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
        Self::check_idempotency_key(&req.idempotency_key)?;
        let pi = self
            .client
            .cancel_payment_intent(&req.external_id, &req.idempotency_key)
            .await?;
        Ok(Response::new(pb::CancelPaymentResponse {
            payment: Some(Self::map_payment(&pi, None)),
        }))
    }

    async fn refund_payment(
        &self,
        request: Request<pb::RefundPaymentRequest>,
    ) -> Result<Response<pb::RefundPaymentResponse>, Status> {
        let req = request.into_inner();
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
        Self::check_idempotency_key(&req.idempotency_key)?;
        let refund: Refund = self
            .client
            .create_refund(&req.external_id, req.amount_minor, &req.idempotency_key)
            .await?;
        Ok(Response::new(pb::RefundPaymentResponse {
            refund_id: refund.id,
            status: Self::refund_status(refund.status.as_deref().unwrap_or_default()),
            amount: Some(pb::Money {
                amount_minor: refund.amount,
                currency: refund.currency,
            }),
        }))
    }

    async fn parse_webhook(
        &self,
        request: Request<pb::ParseWebhookRequest>,
    ) -> Result<Response<pb::ParseWebhookResponse>, Status> {
        let req = request.into_inner();
        let secret = self
            .webhook_secret
            .as_ref()
            .ok_or(GatewayError::WebhookNotConfigured)?;
        let signature = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("stripe-signature"))
            .map(|(_, v)| v.as_str())
            .ok_or(GatewayError::InvalidSignature)?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        webhook::verify_signature(
            &req.payload,
            signature,
            secret.expose_secret(),
            now,
            webhook::DEFAULT_TOLERANCE_SECS,
        )?;

        let event: Event = serde_json::from_slice(&req.payload)
            .map_err(|e| GatewayError::Malformed(format!("evento invalido: {e}")))?;
        let object = &event.data.object;
        let object_kind = object
            .get("object")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let object_status = object.get("status").and_then(|v| v.as_str());
        let external_id = match object_kind.as_str() {
            "charge" | "refund" => object
                .get("payment_intent")
                .and_then(|v| v.as_str())
                .or_else(|| object.get("id").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string(),
            _ => object
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        };
        let status = match event.r#type.as_str() {
            "charge.refunded" => pb::PaymentStatus::Refunded as i32,
            _ => match (object_kind.as_str(), object_status) {
                ("payment_intent", Some(s)) => Self::intent_status(s),
                ("refund", Some(s)) => Self::refund_status(s),
                _ => pb::PaymentStatus::Unspecified as i32,
            },
        };

        Ok(Response::new(pb::ParseWebhookResponse {
            event_id: event.id,
            event_type: event.r#type,
            external_id,
            object_kind,
            status,
            payload: req.payload,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pix_request() -> pb::CreatePaymentRequest {
        pb::CreatePaymentRequest {
            idempotency_key: "key-1".to_string(),
            amount: Some(pb::Money {
                amount_minor: 5000,
                currency: "BRL".to_string(),
            }),
            customer: Some(pb::Customer {
                name: "Maria Silva".to_string(),
                email: "maria@example.com".to_string(),
                tax_id: "00000000000".to_string(),
                ..Default::default()
            }),
            method: Some(Method::Pix(pb::PixMethod {
                expires_after_seconds: 3600,
            })),
            return_url: "https://loja.example/retorno".to_string(),
            description: String::new(),
            metadata: std::collections::HashMap::from([("order".to_string(), "123".to_string())]),
        }
    }

    fn card_request() -> pb::CreatePaymentRequest {
        pb::CreatePaymentRequest {
            method: Some(Method::Card(pb::CardMethod {
                token: "pm_card_visa".to_string(),
                ..Default::default()
            })),
            customer: None,
            ..pix_request()
        }
    }

    #[test]
    fn pix_params_include_billing_details_and_expiration() {
        let params = StripeGateway::build_create_params(&pix_request()).unwrap();
        let has = |k: &str, v: &str| params.iter().any(|(pk, pv)| pk == k && pv == v);
        assert!(has("amount", "5000"));
        assert!(has("currency", "brl"));
        assert!(has("confirm", "true"));
        assert!(has("payment_method_data[type]", "pix"));
        assert!(has(
            "payment_method_data[billing_details][tax_id]",
            "00000000000"
        ));
        assert!(has(
            "payment_method_options[pix][expires_after_seconds]",
            "3600"
        ));
        assert!(has("return_url", "https://loja.example/retorno"));
        assert!(has("metadata[order]", "123"));
    }

    #[test]
    fn card_params_use_token() {
        let params = StripeGateway::build_create_params(&card_request()).unwrap();
        assert!(
            params
                .iter()
                .any(|(k, v)| k == "payment_method" && v == "pm_card_visa")
        );
        assert!(!params.iter().any(|(k, _)| k == "payment_method_types[]"));
    }

    #[test]
    fn pix_requires_customer_and_return_url() {
        let mut req = pix_request();
        req.customer = None;
        assert!(StripeGateway::build_create_params(&req).is_err());

        let mut req = pix_request();
        req.return_url = String::new();
        assert!(StripeGateway::build_create_params(&req).is_err());
    }

    #[test]
    fn pix_expiration_out_of_range_is_rejected() {
        let mut req = pix_request();
        req.method = Some(Method::Pix(pb::PixMethod {
            expires_after_seconds: 30,
        }));
        assert!(StripeGateway::build_create_params(&req).is_err());
    }

    #[tokio::test]
    async fn create_payment_pix_end_to_end() {
        use secrecy::SecretString;
        use std::time::Duration;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment_intents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "pi_pix_1",
                "status": "requires_action",
                "amount": 5000,
                "currency": "brl",
                "client_secret": "sec_1",
                "payment_method_types": ["pix"],
                "next_action": {
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

        let client = crate::client::StripeClient::new(
            server.uri(),
            SecretString::new("sk_test_123".into()),
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        let gateway = StripeGateway::new(client, None);

        let response = gateway
            .create_payment(Request::new(pix_request()))
            .await
            .unwrap()
            .into_inner();
        let payment = response.payment.unwrap();
        assert_eq!(payment.external_id, "pi_pix_1");
        assert_eq!(payment.status, pb::PaymentStatus::RequiresAction as i32);
        assert_eq!(payment.method, pb::PaymentMethod::Pix as i32);
        let pix = payment.pix_details.unwrap();
        assert_eq!(pix.emv_qr_code, "emv-payload");
        assert_eq!(pix.expires_at, 1700003600);
    }

    #[test]
    fn status_mapping_covers_stripe_states() {
        assert_eq!(
            StripeGateway::intent_status("succeeded"),
            pb::PaymentStatus::Paid as i32
        );
        assert_eq!(
            StripeGateway::intent_status("requires_action"),
            pb::PaymentStatus::RequiresAction as i32
        );
        assert_eq!(
            StripeGateway::intent_status("processing"),
            pb::PaymentStatus::Pending as i32
        );
        assert_eq!(
            StripeGateway::intent_status("canceled"),
            pb::PaymentStatus::Canceled as i32
        );
        assert_eq!(
            StripeGateway::intent_status("requires_payment_method"),
            pb::PaymentStatus::Failed as i32
        );
    }
}
