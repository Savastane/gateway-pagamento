use secrecy::SecretString;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("variavel de ambiente obrigatoria ausente: {0}")]
    Missing(&'static str),
    #[error("ASAAS_ACCESS_TOKEN invalido: deve ser uma chave de API asaas ($aact_...)")]
    InvalidToken,
    #[error("GRPC_ADDR invalido: {0}")]
    InvalidAddr(String),
    #[error("HTTP_TIMEOUT_MS invalido: {0}")]
    InvalidTimeout(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub grpc_addr: SocketAddr,
    pub api_base: String,
    pub access_token: SecretString,
    pub webhook_token: Option<SecretString>,
    pub http_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let access_token = std::env::var("ASAAS_ACCESS_TOKEN")
            .map_err(|_| ConfigError::Missing("ASAAS_ACCESS_TOKEN"))?;
        if !access_token.starts_with("$aact_") {
            return Err(ConfigError::InvalidToken);
        }

        let webhook_token = std::env::var("ASAAS_WEBHOOK_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .map(|t| SecretString::new(t.into()));

        Ok(Self {
            grpc_addr: std::env::var("GRPC_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:50053".to_string())
                .parse()
                .map_err(|_| {
                    ConfigError::InvalidAddr(std::env::var("GRPC_ADDR").unwrap_or_default())
                })?,
            api_base: std::env::var("ASAAS_API_BASE")
                .unwrap_or_else(|_| "https://api.asaas.com/v3".to_string()),
            access_token: SecretString::new(access_token.into()),
            webhook_token,
            http_timeout: Duration::from_millis(match std::env::var("HTTP_TIMEOUT_MS") {
                Ok(v) => v
                    .parse::<u64>()
                    .map_err(|_| ConfigError::InvalidTimeout(v))?,
                Err(_) => 60_000,
            }),
        })
    }
}
