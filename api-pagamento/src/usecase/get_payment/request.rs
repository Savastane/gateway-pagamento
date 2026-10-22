use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub struct GetPaymentRequest {
    pub id: Uuid,
}
