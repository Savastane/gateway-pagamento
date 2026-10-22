use super::request::CreatePaymentRequest;
use super::response::PaymentResponse;
use crate::error::AppError;
use crate::models::*;
use crate::services::gateway_router::GatewayRouter;
use crate::store::{NewPayment, Store};
use std::sync::Arc;

pub struct CreatePayment {
    pub store: Arc<dyn Store>,
    pub router: GatewayRouter,
}

pub fn transient(code: &tonic::Code) -> bool {
    matches!(
        code,
        tonic::Code::Unavailable
            | tonic::Code::DeadlineExceeded
            | tonic::Code::Unknown
            | tonic::Code::Internal
            | tonic::Code::ResourceExhausted
            | tonic::Code::Aborted
    )
}

impl CreatePayment {
    // Retorna (http_status, body) para permitir replay de resposta idempotente.
    pub async fn execute(
        &self,
        req: CreatePaymentRequest,
        request_hash: &str,
        idem_key: &str,
        merchant: &str,
    ) -> Result<(u16, serde_json::Value), AppError> {
        req.validate()?;
        let (gateway_name, caller) = self.router.resolve(req.gateway.as_deref())?;

        match self
            .store
            .idem_begin(idem_key, merchant, request_hash)
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
        {
            IdemBegin::Replay(body, code) => return Ok((code as u16, body)),
            IdemBegin::InFlight => {
                return Err(AppError::conflict(
                    "request com esta Idempotency-Key ainda em andamento",
                ));
            }
            IdemBegin::Conflict => {
                return Err(AppError::conflict(
                    "Idempotency-Key reutilizada com corpo diferente",
                ));
            }
            IdemBegin::New => {}
        }

        let payment = self
            .store
            .insert_payment(&NewPayment {
                merchant_id: merchant.to_string(),
                gateway: gateway_name,
                method: req.method_name().to_string(),
                amount_minor: req.amount_minor,
                currency: req.currency.to_lowercase(),
                idempotency_key: Some(idem_key.to_string()),
                metadata: req.metadata_json(),
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;

        let result = match caller.create(req.to_proto(idem_key)).await {
            Ok(p) => {
                let pix_json = p
                    .pix_details
                    .as_ref()
                    .map(|d| serde_json::to_value(PixDetailsDto::from(d)).unwrap_or_default());
                let status = status_name(p.status);
                let row = self
                    .store
                    .finalize_create(payment.id, status, &p.external_id, pix_json.as_ref())
                    .await
                    .map_err(|e| AppError::internal(e.to_string()))?;
                let body = serde_json::to_value(PaymentResponse::from_row(&row))
                    .map_err(|e| AppError::internal(e.to_string()))?;
                (200u16, body)
            }
            Err(status) => {
                let err = AppError::gateway(&status);
                if transient(&status.code()) {
                    // falha transitoria: libera a chave para o cliente poder retentar
                    let _ = self.store.idem_abort(idem_key, merchant).await;
                    return Err(err);
                }
                let _ = self.store.mark_failed(payment.id, FAILED).await;
                (err.status.as_u16(), err.to_problem())
            }
        };

        let _ = self
            .store
            .idem_finish(idem_key, merchant, &result.1, result.0 as i32)
            .await;
        Ok(result)
    }
}
