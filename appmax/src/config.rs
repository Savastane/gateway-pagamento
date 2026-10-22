use secrecy::SecretString;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("variavel de ambiente obrigatoria ausente: {0}")]
    Missing(&'static str),
    #[error("GRPC_ADDR invalido: {0}")]
    InvalidAddr(String),
    #[error("HTTP_TIMEOUT_MS invalido: {0}")]
    InvalidTimeout(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub grpc_addr: SocketAddr,
    pub api_base: String,
    pub auth_base: String,
    pub client_id: String,
    pub client_secret: SecretString,
    pub http_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let client_id = std::env::var("APPMAX_CLIENT_ID")
            .map_err(|_| ConfigError::Missing("APPMAX_CLIENT_ID"))?;
        let client_secret = std::env::var("APPMAX_CLIENT_SECRET")
            .map_err(|_| ConfigError::Missing("APPMAX_CLIENT_SECRET"))?;

        let api_base = std::env::var("APPMAX_API_BASE")
            .unwrap_or_else(|_| "https://api.appmax.com.br".to_string());
        let auth_base = std::env::var("APPMAX_AUTH_BASE")
            .unwrap_or_else(|_| api_base.replacen("api.", "auth.", 1));

        Ok(Self {
            grpc_addr: std::env::var("GRPC_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:50052".to_string())
                .parse()
                .map_err(|_| {
                    ConfigError::InvalidAddr(std::env::var("GRPC_ADDR").unwrap_or_default())
                })?,
            api_base,
            auth_base,
            client_id,
            client_secret: SecretString::new(client_secret.into()),
            http_timeout: Duration::from_millis(match std::env::var("HTTP_TIMEOUT_MS") {
                Ok(v) => v
                    .parse::<u64>()
                    .map_err(|_| ConfigError::InvalidTimeout(v))?,
                Err(_) => 20_000,
            }),
        })
    }
}
