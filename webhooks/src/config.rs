use secrecy::SecretString;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("HTTP_ADDR invalido: {0}")]
    InvalidAddr(String),
    #[error("REPROCESS_INTERVAL_SECS invalido: {0}")]
    InvalidInterval(String),
    #[error(
        "nenhum gateway configurado: defina ao menos um GATEWAY_<NOME>_ADDR (ex.: GATEWAY_STRIPE_ADDR=http://localhost:50051)"
    )]
    NoGateways,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub http_addr: SocketAddr,
    pub database_url: Option<String>,
    pub gateways: HashMap<String, String>,
    pub callback_url: Option<String>,
    pub internal_token: Option<SecretString>,
    pub reprocess_interval: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let gateways = gateway_addrs();
        if gateways.is_empty() {
            return Err(ConfigError::NoGateways);
        }

        Ok(Self {
            http_addr: std::env::var("HTTP_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8081".to_string())
                .parse()
                .map_err(|_| {
                    ConfigError::InvalidAddr(std::env::var("HTTP_ADDR").unwrap_or_default())
                })?,
            database_url: std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty()),
            gateways,
            callback_url: std::env::var("API_CALLBACK_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            internal_token: std::env::var("INTERNAL_TOKEN")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|t| SecretString::new(t.into())),
            reprocess_interval: Duration::from_secs(
                match std::env::var("REPROCESS_INTERVAL_SECS") {
                    Ok(v) => v
                        .parse::<u64>()
                        .map_err(|_| ConfigError::InvalidInterval(v))?,
                    Err(_) => 60,
                },
            ),
        })
    }
}

// GATEWAY_STRIPE_ADDR=http://localhost:50051 registra o provedor "stripe".
// Adicionar um gateway novo = nova variavel, zero codigo.
pub fn gateway_addrs() -> HashMap<String, String> {
    std::env::vars()
        .filter_map(|(k, v)| {
            k.strip_prefix("GATEWAY_")
                .and_then(|rest| rest.strip_suffix("_ADDR"))
                .map(|name| (name.to_lowercase(), v))
        })
        .collect()
}
