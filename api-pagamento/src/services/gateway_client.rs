use gateway_proto::payment_gateway::v1 as pb;
use pb::payment_gateway_client::PaymentGatewayClient;
use std::sync::Arc;
use tonic::transport::Channel;

#[async_trait::async_trait]
pub trait GatewayCaller: Send + Sync {
    async fn native_store(
        &self,
        _req: pb::NativeStoreRequest,
    ) -> Result<pb::BillingResponse, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "provider does not support native stores",
        ))
    }
    async fn create(&self, req: pb::CreatePaymentRequest) -> Result<pb::Payment, tonic::Status>;
    async fn get(&self, external_id: &str) -> Result<pb::Payment, tonic::Status>;
    async fn refund(
        &self,
        req: pb::RefundPaymentRequest,
    ) -> Result<pb::RefundPaymentResponse, tonic::Status>;
}

pub struct GrpcGateway {
    client: PaymentGatewayClient<Channel>,
    native: pb::native_store_billing_client::NativeStoreBillingClient<Channel>,
}

impl GrpcGateway {
    pub async fn connect(addr: String) -> Result<Arc<dyn GatewayCaller>, tonic::transport::Error> {
        let channel = tonic::transport::Endpoint::new(addr)?.connect().await?;
        let client = PaymentGatewayClient::new(channel.clone());
        let native = pb::native_store_billing_client::NativeStoreBillingClient::new(channel);
        Ok(Arc::new(Self { client, native }))
    }
}

#[async_trait::async_trait]
impl GatewayCaller for GrpcGateway {
    async fn native_store(
        &self,
        req: pb::NativeStoreRequest,
    ) -> Result<pb::BillingResponse, tonic::Status> {
        let mut request = tonic::Request::new(req);
        request.set_timeout(std::time::Duration::from_secs(45));
        Ok(self.native.clone().execute(request).await?.into_inner())
    }
    async fn create(&self, req: pb::CreatePaymentRequest) -> Result<pb::Payment, tonic::Status> {
        self.client
            .clone()
            .create_payment(req)
            .await?
            .into_inner()
            .payment
            .ok_or_else(|| tonic::Status::internal("create_payment: resposta sem payment"))
    }

    async fn get(&self, external_id: &str) -> Result<pb::Payment, tonic::Status> {
        self.client
            .clone()
            .get_payment(pb::GetPaymentRequest {
                external_id: external_id.to_string(),
            })
            .await?
            .into_inner()
            .payment
            .ok_or_else(|| tonic::Status::internal("get_payment: resposta sem payment"))
    }

    async fn refund(
        &self,
        req: pb::RefundPaymentRequest,
    ) -> Result<pb::RefundPaymentResponse, tonic::Status> {
        Ok(self.client.clone().refund_payment(req).await?.into_inner())
    }
}
