use super::request::GetPaymentRequest;
use super::response::PaymentResponse;
use crate::error::AppError;
use crate::models::*;
use crate::services::gateway_router::GatewayRouter;
use crate::store::Store;
use std::sync::Arc;

pub struct GetPayment {
    pub store: Arc<dyn Store>,
    pub router: GatewayRouter,
}

impl GetPayment {
    pub async fn execute(&self, req: GetPaymentRequest) -> Result<PaymentResponse, AppError> {
        let mut row = self
            .store
            .get_payment(req.id)
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
            .ok_or_else(|| AppError::not_found(format!("payment {} nao encontrado", req.id)))?;

        // Rebusca o estado no provedor (best-effort) — o estado final vence.
        if let Some(external_id) = row.external_id.clone()
            && let Ok((_, caller)) = self.router.resolve(Some(&row.gateway))
            && let Ok(observed) = caller.get(&external_id).await
        {
            let observed = status_name(observed.status);
            if observed != row.status
                && can_transition(&row.status, observed)
                && let Ok(updated) = self.store.sync_status(row.id, observed).await
            {
                row = updated;
            }
        }
        Ok(PaymentResponse::from_row(&row))
    }
}
