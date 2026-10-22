use secrecy::SecretString;
use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("HTTP_ADDR invalido: {0}")]
    InvalidAddr(String),
    #[error(
        "nenhum gateway configurado: defina ao menos um GATEWAY_<NOME>_ADDR (ex.: GATEWAY_STRIPE_ADDR=http://localhost:50051)"
    )]
    NoGateways,
    #[error("DEFAULT_GATEWAY '{0}' nao corresponde a nenhum GATEWAY_<NOME>_ADDR configurado")]
    BadDefault(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub http_addr: SocketAddr,
    pub database_url: Option<String>,
    pub gateways: HashMap<String, String>,
    pub default_gateway: String,
    pub api_key: Option<SecretString>,
    pub internal_token: Option<SecretString>,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let gateways = gateway_addrs();
        if gateways.is_empty() {
            return Err(ConfigError::NoGateways);
        }
        let default_gateway =
            std::env::var("DEFAULT_GATEWAY").unwrap_or_else(|_| "stripe".to_string());
        if !gateways.contains_key(&default_gateway) {
            return Err(ConfigError::BadDefault(default_gateway));
        }

        Ok(Self {
            http_addr: std::env::var("HTTP_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
                .parse()
                .map_err(|_| {
                    ConfigError::InvalidAddr(std::env::var("HTTP_ADDR").unwrap_or_default())
                })?,
            database_url: std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty()),
            gateways,
            default_gateway,
            api_key: std::env::var("API_KEY")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|k| SecretString::new(k.into())),
            internal_token: std::env::var("INTERNAL_TOKEN")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|t| SecretString::new(t.into())),
        })
    }
}

pub fn gateway_addrs() -> HashMap<String, String> {
    std::env::vars()
        .filter_map(|(k, v)| {
            k.strip_prefix("GATEWAY_")
                .and_then(|rest| rest.strip_suffix("_ADDR"))
                .map(|name| (name.to_lowercase(), v))
        })
        .collect()
}
