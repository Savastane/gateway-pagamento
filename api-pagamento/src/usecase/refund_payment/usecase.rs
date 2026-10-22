use super::request::RefundPaymentRequest;
use super::response::RefundResponse;
use crate::error::AppError;
use crate::models::*;
use crate::services::gateway_router::GatewayRouter;
use crate::store::Store;
use crate::usecase::create_payment::usecase::transient;
use axum::http::StatusCode;
use gateway_proto::payment_gateway::v1 as pb;
use std::sync::Arc;
use uuid::Uuid;

pub struct RefundPayment {
    pub store: Arc<dyn Store>,
    pub router: GatewayRouter,
}

impl RefundPayment {
    // Retorna (http_status, body) para replay idempotente.
    pub async fn execute(
        &self,
        payment_id: Uuid,
        req: RefundPaymentRequest,
        request_hash: &str,
        idem_key: &str,
        merchant: &str,
    ) -> Result<(u16, serde_json::Value), AppError> {
        if req.amount_minor < 0 {
            return Err(AppError::invalid("amount_minor deve ser >= 0"));
        }
        match self
            .store
            .idem_begin(idem_key, merchant, request_hash)
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
        {
            IdemBegin::Replay(body, code) => return Ok((code as u16, body)),
            IdemBegin::InFlight => {
                return Err(AppError::conflict("request em andamento para esta key"));
            }
            IdemBegin::Conflict => {
                return Err(AppError::conflict(
                    "Idempotency-Key reutilizada com corpo diferente",
                ));
            }
            IdemBegin::New => {}
        }

        let finish = |store: &Arc<dyn Store>, code: u16, body: serde_json::Value| {
            let store = store.clone();
            let key = idem_key.to_string();
            let merchant = merchant.to_string();
            async move {
                let _ = store.idem_finish(&key, &merchant, &body, code as i32).await;
                Ok((code, body))
            }
        };

        let payment = match self
            .store
            .get_payment(payment_id)
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
        {
            Some(p) => p,
            None => {
                let e = AppError::not_found(format!("payment {payment_id} nao encontrado"));
                return finish(&self.store, e.status.as_u16(), e.to_problem()).await;
            }
        };
        let Some(external_id) = payment.external_id.clone() else {
            let e = AppError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "refund_impossible",
                "pagamento ainda nao foi criado no provedor",
            );
            return finish(&self.store, e.status.as_u16(), e.to_problem()).await;
        };
        if payment.merchant_id != merchant {
            let e = AppError::not_found(format!("payment {payment_id} nao encontrado"));
            return finish(&self.store, e.status.as_u16(), e.to_problem()).await;
        }

        // Reserva sob lock ANTES de chamar o provedor (concorrencia segura).
        let (refund_id, amount) = match self
            .store
            .reserve_refund(payment.id, req.amount_minor, idem_key)
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
        {
            ReserveOutcome::Reserved { refund_id, amount } => (refund_id, amount),
            ReserveOutcome::NotFound => {
                let e = AppError::not_found(format!("payment {payment_id} nao encontrado"));
                return finish(&self.store, e.status.as_u16(), e.to_problem()).await;
            }
            ReserveOutcome::InvalidState { status } => {
                let e = AppError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "refund_impossible",
                    format!("pagamento em status {status} nao pode ser estornado"),
                );
                return finish(&self.store, e.status.as_u16(), e.to_problem()).await;
            }
            ReserveOutcome::ExceedsPaid { remaining } => {
                let e = AppError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "refund_exceeds_paid",
                    format!("restante estornavel: {remaining}"),
                );
                return finish(&self.store, e.status.as_u16(), e.to_problem()).await;
            }
        };

        let (_, caller) = self.router.resolve(Some(&payment.gateway))?;
        match caller
            .refund(pb::RefundPaymentRequest {
                external_id: external_id.clone(),
                amount_minor: amount,
                idempotency_key: idem_key.to_string(),
            })
            .await
        {
            Ok(resp) => {
                let new_status = self
                    .store
                    .settle_refund(refund_id, &resp.refund_id, true)
                    .await
                    .map_err(|e| AppError::internal(e.to_string()))?
                    .unwrap_or_else(|| PARTIALLY_REFUNDED.to_string());
                let body = serde_json::to_value(RefundResponse {
                    refund_id,
                    payment_id: payment.id,
                    amount_minor: amount,
                    external_id: resp.refund_id,
                    payment_status: new_status,
                })
                .map_err(|e| AppError::internal(e.to_string()))?;
                finish(&self.store, 200, body).await
            }
            Err(status) => {
                let _ = self.store.settle_refund(refund_id, "", false).await;
                let err = AppError::gateway(&status);
                if transient(&status.code()) {
                    let _ = self.store.idem_abort(idem_key, merchant).await;
                    return Err(err);
                }
                finish(&self.store, err.status.as_u16(), err.to_problem()).await
            }
        }
    }
}
