use crate::forward::EventForwarder;
use crate::registry::GatewayRegistry;
use crate::store::{EventStore, NewEvent};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_proto::payment_gateway::v1 as pb;
use std::collections::HashMap;
use std::sync::Arc;

pub struct AppState {
    pub registry: GatewayRegistry,
    pub store: Arc<dyn EventStore>,
    pub forwarder: EventForwarder,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/webhooks/{provider}", post(handle_webhook))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

fn header_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|s| (k.as_str().to_string(), s.to_string()))
        })
        .collect()
}

fn grpc_to_http(status: &tonic::Status) -> StatusCode {
    use tonic::Code::*;
    match status.code() {
        Unauthenticated | PermissionDenied => StatusCode::UNAUTHORIZED,
        InvalidArgument => StatusCode::BAD_REQUEST,
        NotFound => StatusCode::NOT_FOUND,
        Unimplemented => StatusCode::BAD_GATEWAY,
        Unavailable | DeadlineExceeded => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// Fluxo: valida/normaliza via gRPC do gateway -> persiste (dedup)
// -> responde 2xx -> encaminha para a api-pagamento em background.
async fn handle_webhook(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(client) = state.registry.get(&provider) else {
        return (
            StatusCode::NOT_FOUND,
            format!("provedor desconhecido: {provider}"),
        )
            .into_response();
    };

    let parsed = match client
        .parse_webhook(body.to_vec(), header_map(&headers))
        .await
    {
        Ok(p) => p,
        Err(status) => {
            tracing::warn!(provider = %provider, code = ?status.code(), "webhook rejeitado");
            return (grpc_to_http(&status), status.message().to_string()).into_response();
        }
    };

    // Status final vem do evento normalizado; quando o envelope nao traz
    // status (UNSPECIFIED), rebusca o objeto no provedor.
    let normalized_status = match pb::PaymentStatus::try_from(parsed.status) {
        Ok(pb::PaymentStatus::Unspecified) | Err(_) if !parsed.external_id.is_empty() => client
            .get_payment(&parsed.external_id)
            .await
            .map(|p| p.status)
            .unwrap_or(parsed.status),
        _ => parsed.status,
    };

    let event = NewEvent {
        gateway: provider.clone(),
        provider_event_id: parsed.event_id,
        event_type: parsed.event_type,
        external_id: parsed.external_id,
        normalized_status,
        payload: parsed.payload,
    };

    match state.store.insert_if_new(&event).await {
        Ok(false) => {
            return (
                StatusCode::OK,
                Json(serde_json::json!({"result": "duplicate"})),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(provider = %provider, error = %e, "falha ao persistir evento");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "falha ao persistir evento",
            )
                .into_response();
        }
        Ok(true) => {}
    }

    // Encaminhamento em background: o 2xx nao pode depender da api-pagamento.
    let state = state.clone();
    tokio::spawn(async move {
        match state.forwarder.forward(&event).await {
            Ok(()) => {
                let _ = state
                    .store
                    .mark_processed(&event.gateway, &event.provider_event_id)
                    .await;
            }
            Err(e) => {
                tracing::warn!(
                    gateway = %event.gateway,
                    event = %event.provider_event_id,
                    error = %e,
                    "encaminhamento falhou; reprocessador retomara"
                );
                let _ = state
                    .store
                    .mark_failed(&event.gateway, &event.provider_event_id, &e)
                    .await;
            }
        }
    });

    (
        StatusCode::OK,
        Json(serde_json::json!({"result": "received"})),
    )
        .into_response()
}

// Retenta eventos nao processados (api-pagamento fora do ar, restart etc.).
pub fn spawn_reprocessor(state: Arc<AppState>, interval: std::time::Duration) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            match state.store.stale_pending(30, 50).await {
                Err(e) => tracing::error!(error = %e, "reprocessador: falha ao listar pendentes"),
                Ok(events) if !events.is_empty() => {
                    tracing::info!(count = events.len(), "reprocessando eventos pendentes");
                    for stored in events {
                        let event = stored.event;
                        match state.forwarder.forward(&event).await {
                            Ok(()) => {
                                let _ = state
                                    .store
                                    .mark_processed(&event.gateway, &event.provider_event_id)
                                    .await;
                            }
                            Err(e) => {
                                let _ = state
                                    .store
                                    .mark_failed(&event.gateway, &event.provider_event_id, &e)
                                    .await;
                            }
                        }
                    }
                }
                Ok(_) => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ProviderClient;
    use crate::store::MemEventStore;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    struct FakeProvider {
        resp: Result<pb::ParseWebhookResponse, tonic::Status>,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ProviderClient for FakeProvider {
        async fn parse_webhook(
            &self,
            _payload: Vec<u8>,
            _headers: HashMap<String, String>,
        ) -> Result<pb::ParseWebhookResponse, tonic::Status> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.resp.clone()
        }

        async fn get_payment(&self, _external_id: &str) -> Result<pb::Payment, tonic::Status> {
            Err(tonic::Status::unimplemented("not needed"))
        }
    }

    fn app(provider: Arc<dyn ProviderClient>, store: Arc<dyn EventStore>) -> Router {
        let registry = GatewayRegistry::with_provider("fake", provider);
        router(Arc::new(AppState {
            registry,
            store,
            forwarder: EventForwarder::new(None, None),
        }))
    }

    fn event() -> pb::ParseWebhookResponse {
        pb::ParseWebhookResponse {
            event_id: "evt_1".into(),
            event_type: "payment.paid".into(),
            external_id: "pay_1".into(),
            object_kind: "payment".into(),
            status: pb::PaymentStatus::Paid as i32,
            payload: br#"{"id":"evt_1"}"#.to_vec(),
        }
    }

    #[tokio::test]
    async fn accepts_persists_and_dedupes() {
        let provider = Arc::new(FakeProvider {
            resp: Ok(event()),
            calls: AtomicUsize::new(0),
        });
        let store = Arc::new(MemEventStore::default());
        let app = app(provider, store.clone());

        let req = || {
            Request::post("/webhooks/fake")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"id":"evt_1"}"#))
                .unwrap()
        };

        let resp = app.clone().oneshot(req()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app.oneshot(req()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("duplicate"));
    }

    #[tokio::test]
    async fn rejects_bad_signature_and_unknown_provider() {
        let provider = Arc::new(FakeProvider {
            resp: Err(tonic::Status::permission_denied("assinatura nao confere")),
            calls: AtomicUsize::new(0),
        });
        let app = app(provider, Arc::new(MemEventStore::default()));

        let resp = app
            .clone()
            .oneshot(
                Request::post("/webhooks/fake")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let resp = app
            .oneshot(
                Request::post("/webhooks/inexistente")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
