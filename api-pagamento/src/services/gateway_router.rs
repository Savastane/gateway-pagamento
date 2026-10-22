use super::gateway_client::{GatewayCaller, GrpcGateway};
use crate::error::AppError;
use axum::http::StatusCode;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct GatewayRouter {
    gateways: HashMap<String, Arc<dyn GatewayCaller>>,
    default: String,
}

impl GatewayRouter {
    pub async fn from_addrs(
        addrs: &HashMap<String, String>,
        default: &str,
    ) -> Result<Self, tonic::transport::Error> {
        let mut gateways = HashMap::with_capacity(addrs.len());
        for (name, addr) in addrs {
            gateways.insert(name.clone(), GrpcGateway::connect(addr.clone()).await?);
        }
        Ok(Self {
            gateways,
            default: default.to_string(),
        })
    }

    pub fn with_gateways(gateways: HashMap<String, Arc<dyn GatewayCaller>>, default: &str) -> Self {
        Self {
            gateways,
            default: default.to_string(),
        }
    }

    pub fn resolve(
        &self,
        gateway: Option<&str>,
    ) -> Result<(String, Arc<dyn GatewayCaller>), AppError> {
        let name = gateway.unwrap_or(&self.default).to_lowercase();
        match self.gateways.get(&name) {
            Some(g) => Ok((name, g.clone())),
            None => Err(AppError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "gateway_unavailable",
                format!("gateway '{name}' nao registrado"),
            )),
        }
    }
}
