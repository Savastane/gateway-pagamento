use super::request::{CreatePaymentRequest, MethodDto};
use crate::error::AppError;
use gateway_proto::payment_gateway::v1 as pb;

impl CreatePaymentRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.amount_minor <= 0 {
            return Err(AppError::invalid("amount_minor deve ser > 0"));
        }
        let currency = self.currency.to_lowercase();
        if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_lowercase()) {
            return Err(AppError::invalid("currency deve ser ISO 4217 minuscula"));
        }
        if self.customer.name.trim().is_empty() {
            return Err(AppError::invalid("customer.name obrigatorio"));
        }
        if self.customer.email.trim().is_empty() {
            return Err(AppError::invalid("customer.email obrigatorio"));
        }
        match &self.method {
            MethodDto::Card {
                token,
                installments,
            } => {
                if token.trim().is_empty() {
                    return Err(AppError::invalid("method.token obrigatorio"));
                }
                if *installments < 0 {
                    return Err(AppError::invalid("installments deve ser >= 0"));
                }
            }
            MethodDto::Pix {
                expires_after_seconds,
            } => {
                if self.customer.tax_id.trim().is_empty() {
                    return Err(AppError::invalid("customer.tax_id obrigatorio para pix"));
                }
                if *expires_after_seconds < 0 {
                    return Err(AppError::invalid("expires_after_seconds deve ser >= 0"));
                }
            }
        }
        Ok(())
    }

    pub fn method_name(&self) -> &'static str {
        match self.method {
            MethodDto::Card { .. } => "card",
            MethodDto::Pix { .. } => "pix",
        }
    }

    pub fn to_proto(&self, idempotency_key: &str) -> pb::CreatePaymentRequest {
        let method = match &self.method {
            MethodDto::Card {
                token,
                installments,
            } => pb::create_payment_request::Method::Card(pb::CardMethod {
                token: token.clone(),
                installments: *installments,
            }),
            MethodDto::Pix {
                expires_after_seconds,
            } => pb::create_payment_request::Method::Pix(pb::PixMethod {
                expires_after_seconds: *expires_after_seconds,
            }),
        };
        pb::CreatePaymentRequest {
            idempotency_key: idempotency_key.to_string(),
            amount: Some(pb::Money {
                amount_minor: self.amount_minor,
                currency: self.currency.to_lowercase(),
            }),
            customer: Some(pb::Customer {
                name: self.customer.name.clone(),
                email: self.customer.email.clone(),
                tax_id: self.customer.tax_id.clone(),
                phone: self.customer.phone.clone(),
                ip: self.customer.ip.clone(),
            }),
            method: Some(method),
            return_url: self.return_url.clone().unwrap_or_default(),
            description: self.description.clone().unwrap_or_default(),
            metadata: self.metadata.clone(),
        }
    }

    pub fn metadata_json(&self) -> serde_json::Value {
        serde_json::json!({
            "customer": {
                "name": self.customer.name,
                "email": self.customer.email,
                "tax_id": self.customer.tax_id,
                "phone": self.customer.phone,
                "ip": self.customer.ip,
            },
            "description": self.description,
            "metadata": self.metadata,
        })
    }
}
