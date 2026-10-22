use secrecy::SecretString;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("variavel de ambiente obrigatoria ausente: {0}")]
    Missing(&'static str),
    #[error("STRIPE_SECRET_KEY invalida: deve comecar com sk_ ou rk_ (inclui sandbox skcs_/rkcs_)")]
    InvalidKey,
    #[error("GRPC_ADDR invalido: {0}")]
    InvalidAddr(String),
    #[error("HTTP_TIMEOUT_MS invalido: {0}")]
    InvalidTimeout(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub grpc_addr: SocketAddr,
    pub api_base: String,
    pub secret_key: SecretString,
    pub webhook_secret: Option<SecretString>,
    pub api_version: Option<String>,
    pub http_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let secret_key = std::env::var("STRIPE_SECRET_KEY")
            .map_err(|_| ConfigError::Missing("STRIPE_SECRET_KEY"))?;
        if !(secret_key.starts_with("sk") || secret_key.starts_with("rk")) {
            return Err(ConfigError::InvalidKey);
        }

        Ok(Self {
            grpc_addr: std::env::var("GRPC_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:50051".to_string())
                .parse()
                .map_err(|_| {
                    ConfigError::InvalidAddr(std::env::var("GRPC_ADDR").unwrap_or_default())
                })?,
            api_base: std::env::var("STRIPE_API_BASE")
                .unwrap_or_else(|_| "https://api.stripe.com".to_string()),
            secret_key: SecretString::new(secret_key.into()),
            webhook_secret: std::env::var("STRIPE_WEBHOOK_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| SecretString::new(s.into())),
            api_version: std::env::var("STRIPE_API_VERSION")
                .ok()
                .filter(|s| !s.is_empty()),
            http_timeout: Duration::from_millis(match std::env::var("HTTP_TIMEOUT_MS") {
                Ok(v) => v
                    .parse::<u64>()
                    .map_err(|_| ConfigError::InvalidTimeout(v))?,
                Err(_) => 20_000,
            }),
        })
    }
}
