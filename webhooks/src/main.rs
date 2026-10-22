mod config;
mod forward;
mod registry;
mod routes;
mod store;

use config::Config;
use forward::EventForwarder;
use registry::GatewayRegistry;
use routes::AppState;
use std::sync::Arc;
use store::{MemEventStore, PgEventStore};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    let registry = GatewayRegistry::from_addrs(&config.gateways)?;
    tracing::info!(providers = ?registry.names(), "gateways registrados");

    let store: Arc<dyn store::EventStore> = match &config.database_url {
        Some(url) => {
            let pg = PgEventStore::connect(url).await?;
            tracing::info!("event store: postgres");
            Arc::new(pg)
        }
        None => {
            tracing::warn!("DATABASE_URL ausente; event store em memoria (somente dev)");
            Arc::new(MemEventStore::default())
        }
    };

    let state = Arc::new(AppState {
        registry,
        store,
        forwarder: EventForwarder::new(config.callback_url.clone(), config.internal_token.clone()),
    });

    routes::spawn_reprocessor(state.clone(), config.reprocess_interval);

    let listener = tokio::net::TcpListener::bind(config.http_addr).await?;
    tracing::info!(addr = %config.http_addr, "webhook-pagamento listening");
    axum::serve(listener, routes::router(state)).await?;
    Ok(())
}
