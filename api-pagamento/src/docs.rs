use utoipa::Modify;
use utoipa::OpenApi;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, Http, HttpAuthScheme, SecurityScheme};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "api-pagamento",
        version = "0.1.0",
        description = "API REST de pagamentos multi-gateway (Stripe, Appmax, Asaas, Mercado Pago). \
            Valores em centavos (amount_minor). Cartao sempre via token do SDK do provedor — nunca PAN/CVV. \
            Erros em RFC 7807 (application/problem+json)."
    ),
    paths(
        crate::routes::payments::create,
        crate::routes::payments::get,
        crate::routes::refunds::create,
        crate::routes::internal::health,
        crate::routes::internal::gateway_events,
    ),
    components(schemas(
        crate::usecase::create_payment::request::CreatePaymentRequest,
        crate::usecase::create_payment::request::MethodDto,
        crate::usecase::create_payment::request::CustomerDto,
        crate::usecase::create_payment::response::PaymentResponse,
        crate::usecase::refund_payment::request::RefundPaymentRequest,
        crate::usecase::refund_payment::response::RefundResponse,
        crate::models::PixDetailsDto,
        crate::routes::internal::GatewayEvent,
        crate::error::ProblemDetails,
    )),
    modifiers(&SecurityAddon),
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let Some(components) = openapi.components.as_mut() else {
            return;
        };
        components.add_security_scheme(
            "api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-Api-Key"))),
        );
        components.add_security_scheme(
            "internal_token",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
    }
}
