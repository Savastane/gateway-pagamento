// Smoke test client do gateway-mercadopago.
// Uso:
//   GATEWAY_ADDR=http://127.0.0.1:50054 ACTION=create METHOD=pix|card cargo run -p gateway-mercadopago --example smoke
//   ACTION=get EXTERNAL_ID=<payment_id>
//   ACTION=refund EXTERNAL_ID=<payment_id> AMOUNT=0
//   ACTION=cancel EXTERNAL_ID=<payment_id>
//   ACTION=webhook                        (envia envelope de exemplo; assinatura ignorada sem MP_WEBHOOK_SECRET)
use gateway_proto::payment_gateway::v1 as pb;
use pb::create_payment_request::Method;
use pb::payment_gateway_client::PaymentGatewayClient;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("GATEWAY_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50054".into());
    let action = std::env::var("ACTION").unwrap_or_else(|_| "create".into());
    let mut client = PaymentGatewayClient::connect(addr).await?;

    match action.as_str() {
        "get" => {
            let resp = client
                .get_payment(pb::GetPaymentRequest {
                    external_id: std::env::var("EXTERNAL_ID")?,
                })
                .await?
                .into_inner();
            let payment = resp.payment.expect("payment");
            println!(
                "fetched: id={} status={} method={}",
                payment.external_id, payment.status, payment.method
            );
        }
        "refund" => {
            let resp = client
                .refund_payment(pb::RefundPaymentRequest {
                    external_id: std::env::var("EXTERNAL_ID")?,
                    amount_minor: std::env::var("AMOUNT")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    idempotency_key: format!("smoke-re-{}", std::process::id()),
                })
                .await?
                .into_inner();
            println!("refund: id={} status={}", resp.refund_id, resp.status);
        }
        "cancel" => {
            match client
                .cancel_payment(pb::CancelPaymentRequest {
                    external_id: std::env::var("EXTERNAL_ID")?,
                    idempotency_key: format!("smoke-ca-{}", std::process::id()),
                })
                .await
            {
                Ok(resp) => {
                    let payment = resp.into_inner().payment.expect("payment");
                    println!(
                        "canceled: id={} status={}",
                        payment.external_id, payment.status
                    );
                }
                Err(status) => println!("cancel: {} {}", status.code(), status.message()),
            }
        }
        "webhook" => {
            let payload = br#"{"id":12345,"type":"payment","action":"payment.updated","data":{"id":"999999999"}}"#;
            let headers = HashMap::new();
            let resp = client
                .parse_webhook(pb::ParseWebhookRequest {
                    payload: payload.to_vec(),
                    headers,
                })
                .await?
                .into_inner();
            println!(
                "webhook: event={} type={} external_id={} kind={} status={}",
                resp.event_id, resp.event_type, resp.external_id, resp.object_kind, resp.status
            );
        }
        _ => {
            let method = std::env::var("METHOD").unwrap_or_else(|_| "pix".into());
            let method = match method.as_str() {
                "card" => Method::Card(pb::CardMethod {
                    token: std::env::var("CARD_TOKEN")
                        .unwrap_or_else(|_| "TOKEN_DO_MERCADOPAGO_JS".into()),
                    installments: std::env::var("INSTALLMENTS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1),
                }),
                _ => Method::Pix(pb::PixMethod {
                    expires_after_seconds: 0,
                }),
            };

            let created = client
                .create_payment(pb::CreatePaymentRequest {
                    idempotency_key: format!("smoke-{}-{}", std::process::id(), now()),
                    amount: Some(pb::Money {
                        amount_minor: 1000,
                        currency: "brl".to_string(),
                    }),
                    customer: Some(pb::Customer {
                        name: "Smoke Test".to_string(),
                        email: std::env::var("EMAIL")
                            .unwrap_or_else(|_| "test_user@example.com".into()),
                        tax_id: std::env::var("DOC").unwrap_or_else(|_| "19119119100".into()),
                        phone: String::new(),
                        ip: String::new(),
                    }),
                    method: Some(method),
                    return_url: String::new(),
                    description: "smoke test".to_string(),
                    metadata: Default::default(),
                })
                .await?
                .into_inner();
            let payment = created.payment.expect("payment");
            println!(
                "created: id={} status={} method={}",
                payment.external_id, payment.status, payment.method
            );
            if let Some(pix) = &payment.pix_details {
                println!(
                    "pix: emv={} expires_at={} ticket={}",
                    pix.emv_qr_code, pix.expires_at, pix.hosted_instructions_url
                );
            }
        }
    }
    Ok(())
}
