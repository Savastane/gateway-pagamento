use gateway_proto::payment_gateway::v1 as pb;
use pb::payment_gateway_client::PaymentGatewayClient;
use std::collections::HashMap;
use std::sync::Arc;
use tonic::transport::{Channel, Endpoint};

#[async_trait::async_trait]
pub trait ProviderClient: Send + Sync {
    async fn parse_webhook(
        &self,
        payload: Vec<u8>,
        headers: HashMap<String, String>,
    ) -> Result<pb::ParseWebhookResponse, tonic::Status>;

    async fn get_payment(&self, external_id: &str) -> Result<pb::Payment, tonic::Status>;
}

struct GrpcProviderClient {
    client: PaymentGatewayClient<Channel>,
}

#[async_trait::async_trait]
impl ProviderClient for GrpcProviderClient {
    async fn parse_webhook(
        &self,
        payload: Vec<u8>,
        headers: HashMap<String, String>,
    ) -> Result<pb::ParseWebhookResponse, tonic::Status> {
        let resp = self
            .client
            .clone()
            .parse_webhook(pb::ParseWebhookRequest { payload, headers })
            .await?;
        Ok(resp.into_inner())
    }

    async fn get_payment(&self, external_id: &str) -> Result<pb::Payment, tonic::Status> {
        let resp = self
            .client
            .clone()
            .get_payment(pb::GetPaymentRequest {
                external_id: external_id.to_string(),
            })
            .await?;
        resp.into_inner()
            .payment
            .ok_or_else(|| tonic::Status::internal("resposta sem payment"))
    }
}

#[derive(Clone, Default)]
pub struct GatewayRegistry {
    providers: HashMap<String, Arc<dyn ProviderClient>>,
}

impl GatewayRegistry {
    pub fn from_addrs(addrs: &HashMap<String, String>) -> Result<Self, tonic::transport::Error> {
        let mut providers = HashMap::new();
        for (name, addr) in addrs {
            let channel = Endpoint::from_shared(addr.clone())?
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(30))
                .connect_lazy();
            providers.insert(
                name.clone(),
                Arc::new(GrpcProviderClient {
                    client: PaymentGatewayClient::new(channel),
                }) as Arc<dyn ProviderClient>,
            );
        }
        Ok(Self { providers })
    }

    pub fn get(&self, provider: &str) -> Option<Arc<dyn ProviderClient>> {
        self.providers.get(&provider.to_lowercase()).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    #[cfg(test)]
    pub fn with_provider(name: &str, client: Arc<dyn ProviderClient>) -> Self {
        let mut providers = HashMap::new();
        providers.insert(name.to_string(), client);
        Self { providers }
    }
}
