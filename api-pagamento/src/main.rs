use api_pagamento::app::AppState;
use api_pagamento::config::Config;
use api_pagamento::services::gateway_router::GatewayRouter;
use api_pagamento::store::{MemStore, PgStore};
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;
    info!(
        "registrando gateways: {:?}",
        cfg.gateways.keys().collect::<Vec<_>>()
    );
    let router = GatewayRouter::from_addrs(&cfg.gateways, &cfg.default_gateway).await?;
    for name in cfg.gateways.keys() {
        info!(gateway = %name, "gateway conectado");
    }

    let store: Arc<dyn api_pagamento::store::Store> = match &cfg.database_url {
        Some(url) => Arc::new(PgStore::connect(url).await?),
        None => {
            warn!("DATABASE_URL nao definido — persistencia em memoria (nao usar em producao)");
            Arc::new(MemStore::default())
        }
    };
    if cfg.api_key.is_none() {
        warn!("API_KEY nao definido — /v1 sem autenticacao (apenas dev)");
    }

    let state = AppState::new(store, router, &cfg);
    let app = api_pagamento::app::router(state);
    let listener = tokio::net::TcpListener::bind(cfg.http_addr).await?;
    info!(addr = %cfg.http_addr, "api-pagamento ouvindo");
    axum::serve(listener, app).await?;
    Ok(())
}
