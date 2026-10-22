mod client;
mod config;
mod error;
mod models;
mod service;
mod webhook;

use config::Config;
use gateway_proto::payment_gateway::v1::payment_gateway_server::PaymentGatewayServer;
use service::AsaasGateway;
use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    let client = client::AsaasClient::new(
        &config.api_base,
        config.access_token.clone(),
        config.http_timeout,
    )?;
    let gateway = AsaasGateway::new(client, config.webhook_token.clone());

    let (mut reporter, health_service) = tonic_health::server::health_reporter();
    reporter
        .set_serving::<PaymentGatewayServer<AsaasGateway>>()
        .await;

    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(gateway_proto::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    tracing::info!(addr = %config.grpc_addr, "gateway-asaas listening");
    Server::builder()
        .add_service(health_service)
        .add_service(reflection)
        .add_service(PaymentGatewayServer::new(gateway))
        .serve(config.grpc_addr)
        .await?;
    Ok(())
}
