// Smoke test client: exercita CreatePayment, GetPayment, RefundPayment,
// CancelPayment e ParseWebhook contra o gateway rodando.
// Uso:
//   GATEWAY_ADDR=http://127.0.0.1:50051 ACTION=create METHOD=pix|card cargo run -p gateway-stripe --example smoke
//   ACTION=refund EXTERNAL_ID=pi_... AMOUNT=0
//   ACTION=cancel EXTERNAL_ID=pi_...
//   ACTION=webhook WEBHOOK_SECRET=whsec_...
use gateway_proto::payment_gateway::v1 as pb;
use hmac::{Hmac, Mac};
use pb::create_payment_request::Method;
use pb::payment_gateway_client::PaymentGatewayClient;
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn sign(payload: &[u8], secret: &str, ts: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("{ts}.{}", String::from_utf8_lossy(payload)).as_bytes());
    format!("t={ts},v1={}", hex::encode(mac.finalize().into_bytes()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("GATEWAY_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50051".into());
    let action = std::env::var("ACTION").unwrap_or_else(|_| "create".into());
    let mut client = PaymentGatewayClient::connect(addr).await?;

    match action.as_str() {
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
            let resp = client
                .cancel_payment(pb::CancelPaymentRequest {
                    external_id: std::env::var("EXTERNAL_ID")?,
                    idempotency_key: format!("smoke-ca-{}", std::process::id()),
                })
                .await?
                .into_inner();
            let payment = resp.payment.expect("payment");
            println!(
                "canceled: id={} status={}",
                payment.external_id, payment.status
            );
        }
        "webhook" => {
            let secret = std::env::var("WEBHOOK_SECRET")?;
            let payload = br#"{"id":"evt_smoke_1","object":"event","type":"payment_intent.succeeded","data":{"object":{"id":"pi_smoke","object":"payment_intent","status":"succeeded","amount":5000,"currency":"brl"}}}"#;
            let signature = sign(payload, &secret, now());
            let resp = client
                .parse_webhook(pb::ParseWebhookRequest {
                    payload: payload.to_vec(),
                    headers: HashMap::from([("Stripe-Signature".to_string(), signature)]),
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
                    token: "pm_card_visa".to_string(),
                    ..Default::default()
                }),
                _ => Method::Pix(pb::PixMethod {
                    expires_after_seconds: 3600,
                }),
            };

            let created = client
                .create_payment(pb::CreatePaymentRequest {
                    idempotency_key: format!("smoke-{}-{}", std::process::id(), now()),
                    amount: Some(pb::Money {
                        amount_minor: 5000,
                        currency: "brl".to_string(),
                    }),
                    customer: Some(pb::Customer {
                        name: "Smoke Test".to_string(),
                        email: std::env::var("EMAIL")
                            .unwrap_or_else(|_| "succeed_immediately@example.com".into()),
                        tax_id: "00000000000".to_string(),
                        ..Default::default()
                    }),
                    method: Some(method),
                    return_url: "https://example.com/retorno".to_string(),
                    description: "smoke test".to_string(),
                    metadata: Default::default(),
                })
                .await?
                .into_inner();
            let payment = created.payment.expect("payment");
            println!(
                "created: id={} status={}",
                payment.external_id, payment.status
            );
            if let Some(pix) = &payment.pix_details {
                println!(
                    "pix: emv={} expires_at={} hosted={}",
                    pix.emv_qr_code, pix.expires_at, pix.hosted_instructions_url
                );
            }

            let got = client
                .get_payment(pb::GetPaymentRequest {
                    external_id: payment.external_id.clone(),
                })
                .await?
                .into_inner();
            let payment = got.payment.expect("payment");
            println!(
                "fetched: id={} status={}",
                payment.external_id, payment.status
            );
        }
    }
    Ok(())
}
