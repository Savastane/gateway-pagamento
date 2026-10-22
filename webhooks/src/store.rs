use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub gateway: String,
    pub provider_event_id: String,
    pub event_type: String,
    pub external_id: String,
    pub normalized_status: i32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event: NewEvent,
    pub attempts: i32,
    pub processed: bool,
    pub received_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, thiserror::Error)]
#[error("event store error: {0}")]
pub struct StoreError(pub String);

#[async_trait::async_trait]
pub trait EventStore: Send + Sync {
    // Insere se novo; retorna false quando (gateway, provider_event_id) ja existe.
    async fn insert_if_new(&self, event: &NewEvent) -> Result<bool, StoreError>;
    async fn mark_processed(
        &self,
        gateway: &str,
        provider_event_id: &str,
    ) -> Result<(), StoreError>;
    async fn mark_failed(
        &self,
        gateway: &str,
        provider_event_id: &str,
        error: &str,
    ) -> Result<(), StoreError>;
    // Eventos recebidos ha mais de `older_than_secs` ainda nao processados.
    async fn stale_pending(
        &self,
        older_than_secs: i64,
        limit: i64,
    ) -> Result<Vec<StoredEvent>, StoreError>;
}

// ---------- Postgres ----------

pub struct PgEventStore {
    pool: sqlx::PgPool,
}

impl PgEventStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = banco::connect(database_url).await?;
        Ok(Self { pool })
    }
}

#[async_trait::async_trait]
impl EventStore for PgEventStore {
    async fn insert_if_new(&self, event: &NewEvent) -> Result<bool, StoreError> {
        let payload: serde_json::Value = serde_json::from_slice(&event.payload)
            .unwrap_or_else(|_| serde_json::Value::String(String::new()));
        let res = sqlx::query(
            "INSERT INTO webhook_events
                (gateway, provider_event_id, event_type, external_id, normalized_status, payload)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (gateway, provider_event_id) DO NOTHING",
        )
        .bind(&event.gateway)
        .bind(&event.provider_event_id)
        .bind(&event.event_type)
        .bind(&event.external_id)
        .bind(event.normalized_status)
        .bind(payload)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError(e.to_string()))?;
        Ok(res.rows_affected() == 1)
    }

    async fn mark_processed(
        &self,
        gateway: &str,
        provider_event_id: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE webhook_events
             SET state = 'processed', processed_at = now(), last_error = NULL
             WHERE gateway = $1 AND provider_event_id = $2",
        )
        .bind(gateway)
        .bind(provider_event_id)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    async fn mark_failed(
        &self,
        gateway: &str,
        provider_event_id: &str,
        error: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE webhook_events
             SET state = 'failed', attempts = attempts + 1, last_error = $3
             WHERE gateway = $1 AND provider_event_id = $2",
        )
        .bind(gateway)
        .bind(provider_event_id)
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    async fn stale_pending(
        &self,
        older_than_secs: i64,
        limit: i64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                i32,
                serde_json::Value,
                i32,
                chrono::DateTime<chrono::Utc>,
            ),
        >(
            "SELECT gateway, provider_event_id, event_type, external_id,
                    normalized_status, payload, attempts, received_at
             FROM webhook_events
             WHERE state IN ('received', 'failed')
               AND received_at < now() - make_interval(secs => $1)
             ORDER BY received_at
             LIMIT $2",
        )
        .bind(older_than_secs as f64)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(
                |(gateway, id, ty, ext, status, payload, attempts, received_at)| StoredEvent {
                    event: NewEvent {
                        gateway,
                        provider_event_id: id,
                        event_type: ty,
                        external_id: ext,
                        normalized_status: status,
                        payload: serde_json::to_vec(&payload).unwrap_or_default(),
                    },
                    attempts,
                    processed: false,
                    received_at,
                },
            )
            .collect())
    }
}

// ---------- In-memory (dev/testes) ----------

#[derive(Default)]
pub struct MemEventStore {
    events: Mutex<HashMap<(String, String), StoredEvent>>,
}

#[async_trait::async_trait]
impl EventStore for MemEventStore {
    async fn insert_if_new(&self, event: &NewEvent) -> Result<bool, StoreError> {
        let mut events = self.events.lock().expect("events mutex");
        let key = (event.gateway.clone(), event.provider_event_id.clone());
        if events.contains_key(&key) {
            return Ok(false);
        }
        events.insert(
            key,
            StoredEvent {
                event: event.clone(),
                attempts: 0,
                processed: false,
                received_at: chrono::Utc::now(),
            },
        );
        Ok(true)
    }

    async fn mark_processed(
        &self,
        gateway: &str,
        provider_event_id: &str,
    ) -> Result<(), StoreError> {
        let mut events = self.events.lock().expect("events mutex");
        if let Some(e) = events.get_mut(&(gateway.to_string(), provider_event_id.to_string())) {
            e.processed = true;
        }
        Ok(())
    }

    async fn mark_failed(
        &self,
        gateway: &str,
        provider_event_id: &str,
        _error: &str,
    ) -> Result<(), StoreError> {
        let mut events = self.events.lock().expect("events mutex");
        if let Some(e) = events.get_mut(&(gateway.to_string(), provider_event_id.to_string())) {
            e.attempts += 1;
        }
        Ok(())
    }

    async fn stale_pending(
        &self,
        older_than_secs: i64,
        limit: i64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(older_than_secs);
        let events = self.events.lock().expect("events mutex");
        Ok(events
            .values()
            .filter(|e| !e.processed && e.received_at < cutoff)
            .take(limit as usize)
            .cloned()
            .collect())
    }
}
