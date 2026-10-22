use crate::models::*;
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
#[error("store error: {0}")]
pub struct StoreError(pub String);

#[derive(Debug, Clone)]
pub struct NewPayment {
    pub merchant_id: String,
    pub gateway: String,
    pub method: String,
    pub amount_minor: i64,
    pub currency: String,
    pub idempotency_key: Option<String>,
    pub metadata: serde_json::Value,
}

#[async_trait::async_trait]
pub trait Store: Send + Sync {
    async fn idem_begin(
        &self,
        key: &str,
        merchant: &str,
        request_hash: &str,
    ) -> Result<IdemBegin, StoreError>;
    async fn idem_finish(
        &self,
        key: &str,
        merchant: &str,
        response: &serde_json::Value,
        status_code: i32,
    ) -> Result<(), StoreError>;

    async fn insert_payment(&self, payment: &NewPayment) -> Result<PaymentRow, StoreError>;
    async fn get_payment(&self, id: Uuid) -> Result<Option<PaymentRow>, StoreError>;
    async fn find_by_external(
        &self,
        gateway: &str,
        external_id: &str,
    ) -> Result<Option<PaymentRow>, StoreError>;
    // Grava resultado do CreatePayment no provedor + evento.
    async fn finalize_create(
        &self,
        id: Uuid,
        status: &str,
        external_id: &str,
        pix_details: Option<&serde_json::Value>,
    ) -> Result<PaymentRow, StoreError>;
    // Marca o pagamento como FAILED quando o provedor rejeitou a cobranca.
    async fn mark_failed(&self, id: Uuid, status: &str) -> Result<(), StoreError>;
    // Aplica transicao de estado vinda de webhook (idempotente, nao regride).
    async fn apply_event(
        &self,
        gateway: &str,
        external_id: &str,
        to_status: &str,
        payload: &serde_json::Value,
    ) -> Result<Option<PaymentRow>, StoreError>;
    // Sincroniza status observado no provedor ao consultar (somente se permitido).
    async fn sync_status(&self, id: Uuid, status: &str) -> Result<PaymentRow, StoreError>;
    // Reserva o refund sob lock (soma <= pago). A chamada ao gateway acontece DEPOIS
    // da reserva, evitando refund concorrente que exceda o valor pago.
    async fn reserve_refund(
        &self,
        payment_id: Uuid,
        amount_minor: i64,
        idempotency_key: &str,
    ) -> Result<ReserveOutcome, StoreError>;
    // Conclui a reserva: ok => status 'done' + transicao do pagamento; falha => 'failed'.
    async fn settle_refund(
        &self,
        refund_id: Uuid,
        external_id: &str,
        ok: bool,
    ) -> Result<Option<String>, StoreError>;

    // Libera a chave de idempotencia apos falha transitoria (permite retry pelo cliente).
    async fn idem_abort(&self, key: &str, merchant: &str) -> Result<(), StoreError>;
}

// ---------- Postgres ----------

pub struct PgStore {
    pool: sqlx::PgPool,
}

impl PgStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        Ok(Self {
            pool: banco::connect(database_url).await?,
        })
    }
}

const PAYMENT_COLS: &str = "id, merchant_id, gateway, method, amount_minor, currency, status, \
     external_id, idempotency_key, metadata, version, created_at";

fn map_sqlx(e: sqlx::Error) -> StoreError {
    StoreError(e.to_string())
}

#[async_trait::async_trait]
impl Store for PgStore {
    async fn idem_begin(
        &self,
        key: &str,
        merchant: &str,
        request_hash: &str,
    ) -> Result<IdemBegin, StoreError> {
        let res = sqlx::query(
            "INSERT INTO idempotency_keys (key, merchant_id, request_hash, expires_at)
             VALUES ($1, $2, $3, now() + interval '24 hours')
             ON CONFLICT (key, merchant_id) DO NOTHING",
        )
        .bind(key)
        .bind(merchant)
        .bind(request_hash)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        if res.rows_affected() == 1 {
            return Ok(IdemBegin::New);
        }

        let row: (
            String,
            Option<serde_json::Value>,
            Option<i32>,
            chrono::DateTime<chrono::Utc>,
        ) = sqlx::query_as(
            "SELECT request_hash, response, status_code, expires_at
                 FROM idempotency_keys WHERE key = $1 AND merchant_id = $2",
        )
        .bind(key)
        .bind(merchant)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;

        if row.3 < chrono::Utc::now() {
            sqlx::query(
                "UPDATE idempotency_keys
                 SET request_hash = $3, response = NULL, status_code = NULL,
                     expires_at = now() + interval '24 hours'
                 WHERE key = $1 AND merchant_id = $2",
            )
            .bind(key)
            .bind(merchant)
            .bind(request_hash)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx)?;
            return Ok(IdemBegin::New);
        }
        if row.0 != request_hash {
            return Ok(IdemBegin::Conflict);
        }
        match (row.1, row.2) {
            (Some(resp), Some(code)) => Ok(IdemBegin::Replay(resp, code)),
            _ => Ok(IdemBegin::InFlight),
        }
    }

    async fn idem_finish(
        &self,
        key: &str,
        merchant: &str,
        response: &serde_json::Value,
        status_code: i32,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE idempotency_keys SET response = $3, status_code = $4
             WHERE key = $1 AND merchant_id = $2",
        )
        .bind(key)
        .bind(merchant)
        .bind(response)
        .bind(status_code)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn insert_payment(&self, p: &NewPayment) -> Result<PaymentRow, StoreError> {
        sqlx::query_as::<_, PaymentRow>(&format!(
            "INSERT INTO payments
                (merchant_id, gateway, method, amount_minor, currency, status, idempotency_key, metadata)
             VALUES ($1, $2, $3, $4, $5, '{PENDING}', $6, $7)
             RETURNING {PAYMENT_COLS}"
        ))
        .bind(&p.merchant_id)
        .bind(&p.gateway)
        .bind(&p.method)
        .bind(p.amount_minor)
        .bind(&p.currency)
        .bind(&p.idempotency_key)
        .bind(&p.metadata)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)
    }

    async fn get_payment(&self, id: Uuid) -> Result<Option<PaymentRow>, StoreError> {
        sqlx::query_as::<_, PaymentRow>(&format!(
            "SELECT {PAYMENT_COLS} FROM payments WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)
    }

    async fn find_by_external(
        &self,
        gateway: &str,
        external_id: &str,
    ) -> Result<Option<PaymentRow>, StoreError> {
        sqlx::query_as::<_, PaymentRow>(&format!(
            "SELECT {PAYMENT_COLS} FROM payments WHERE gateway = $1 AND external_id = $2"
        ))
        .bind(gateway)
        .bind(external_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)
    }

    async fn finalize_create(
        &self,
        id: Uuid,
        status: &str,
        external_id: &str,
        pix_details: Option<&serde_json::Value>,
    ) -> Result<PaymentRow, StoreError> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx)?;
        let updated = if let Some(pix) = pix_details {
            sqlx::query_as::<_, PaymentRow>(&format!(
                "UPDATE payments SET status = $2, external_id = $3,
                     metadata = metadata || jsonb_build_object('pix_details', $4::jsonb),
                     version = version + 1, updated_at = now()
                 WHERE id = $1 RETURNING {PAYMENT_COLS}"
            ))
            .bind(id)
            .bind(status)
            .bind(external_id)
            .bind(pix)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_sqlx)?
        } else {
            sqlx::query_as::<_, PaymentRow>(&format!(
                "UPDATE payments SET status = $2, external_id = $3,
                     version = version + 1, updated_at = now()
                 WHERE id = $1 RETURNING {PAYMENT_COLS}"
            ))
            .bind(id)
            .bind(status)
            .bind(external_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_sqlx)?
        };
        sqlx::query(
            "INSERT INTO payment_events (payment_id, from_status, to_status, source)
             VALUES ($1, $2, $3, 'api')",
        )
        .bind(id)
        .bind(PENDING)
        .bind(status)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(updated)
    }

    async fn mark_failed(&self, id: Uuid, status: &str) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx)?;
        sqlx::query(
            "UPDATE payments SET status = $2, version = version + 1, updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        sqlx::query(
            "INSERT INTO payment_events (payment_id, from_status, to_status, source)
             VALUES ($1, $2, $3, 'api')",
        )
        .bind(id)
        .bind(PENDING)
        .bind(status)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(())
    }

    async fn apply_event(
        &self,
        gateway: &str,
        external_id: &str,
        to_status: &str,
        payload: &serde_json::Value,
    ) -> Result<Option<PaymentRow>, StoreError> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx)?;
        let current = sqlx::query_as::<_, PaymentRow>(&format!(
            "SELECT {PAYMENT_COLS} FROM payments
             WHERE gateway = $1 AND external_id = $2 FOR UPDATE"
        ))
        .bind(gateway)
        .bind(external_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx)?;

        let Some(payment) = current else {
            tx.rollback().await.map_err(map_sqlx)?;
            return Ok(None);
        };
        if !can_transition(&payment.status, to_status) || payment.status == to_status {
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(Some(payment));
        }
        let updated = sqlx::query_as::<_, PaymentRow>(&format!(
            "UPDATE payments SET status = $2, version = version + 1, updated_at = now()
             WHERE id = $1 RETURNING {PAYMENT_COLS}"
        ))
        .bind(payment.id)
        .bind(to_status)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        sqlx::query(
            "INSERT INTO payment_events (payment_id, from_status, to_status, source, payload)
             VALUES ($1, $2, $3, 'webhook', $4)",
        )
        .bind(payment.id)
        .bind(&payment.status)
        .bind(to_status)
        .bind(payload)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(Some(updated))
    }

    async fn sync_status(&self, id: Uuid, status: &str) -> Result<PaymentRow, StoreError> {
        let updated = sqlx::query_as::<_, PaymentRow>(&format!(
            "UPDATE payments SET status = $2, version = version + 1, updated_at = now()
             WHERE id = $1 AND status <> $2 RETURNING {PAYMENT_COLS}"
        ))
        .bind(id)
        .bind(status)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;
        match updated {
            Some(p) => Ok(p),
            None => self
                .get_payment(id)
                .await?
                .ok_or_else(|| StoreError("payment not found".to_string())),
        }
    }

    async fn reserve_refund(
        &self,
        payment_id: Uuid,
        amount_minor: i64,
        idempotency_key: &str,
    ) -> Result<ReserveOutcome, StoreError> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx)?;
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT status, amount_minor FROM payments WHERE id = $1 FOR UPDATE")
                .bind(payment_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_sqlx)?;

        let Some((status, paid)) = row else {
            tx.rollback().await.map_err(map_sqlx)?;
            return Ok(ReserveOutcome::NotFound);
        };
        if status != PAID && status != PARTIALLY_REFUNDED {
            tx.rollback().await.map_err(map_sqlx)?;
            return Ok(ReserveOutcome::InvalidState { status });
        }
        let refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_minor), 0)::bigint FROM refunds
             WHERE payment_id = $1 AND status <> 'failed'",
        )
        .bind(payment_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx)?;

        let amount = if amount_minor <= 0 {
            paid - refunded
        } else {
            amount_minor
        };
        if amount <= 0 || refunded + amount > paid {
            tx.rollback().await.map_err(map_sqlx)?;
            return Ok(ReserveOutcome::ExceedsPaid {
                remaining: paid - refunded,
            });
        }

        let refund_id: Uuid = sqlx::query_scalar(
            "INSERT INTO refunds (payment_id, amount_minor, status, idempotency_key)
             VALUES ($1, $2, 'processing', $3) RETURNING id",
        )
        .bind(payment_id)
        .bind(amount)
        .bind(idempotency_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(ReserveOutcome::Reserved { refund_id, amount })
    }

    async fn settle_refund(
        &self,
        refund_id: Uuid,
        external_id: &str,
        ok: bool,
    ) -> Result<Option<String>, StoreError> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx)?;
        let row: Option<(Uuid, i64)> =
            sqlx::query_as("SELECT payment_id, amount_minor FROM refunds WHERE id = $1 FOR UPDATE")
                .bind(refund_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_sqlx)?;
        let Some((payment_id, _amount)) = row else {
            tx.rollback().await.map_err(map_sqlx)?;
            return Ok(None);
        };

        if !ok {
            sqlx::query("UPDATE refunds SET status = 'failed' WHERE id = $1")
                .bind(refund_id)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx)?;
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(None);
        }

        sqlx::query("UPDATE refunds SET status = 'done', external_id = $2 WHERE id = $1")
            .bind(refund_id)
            .bind(external_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx)?;

        let (status, paid): (String, i64) =
            sqlx::query_as("SELECT status, amount_minor FROM payments WHERE id = $1 FOR UPDATE")
                .bind(payment_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_sqlx)?;
        let refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_minor), 0)::bigint FROM refunds
             WHERE payment_id = $1 AND status = 'done'",
        )
        .bind(payment_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx)?;

        let new_status = if refunded >= paid {
            REFUNDED
        } else {
            PARTIALLY_REFUNDED
        };
        sqlx::query(
            "UPDATE payments SET status = $2, version = version + 1, updated_at = now()
             WHERE id = $1",
        )
        .bind(payment_id)
        .bind(new_status)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        sqlx::query(
            "INSERT INTO payment_events (payment_id, from_status, to_status, source)
             VALUES ($1, $2, $3, 'api')",
        )
        .bind(payment_id)
        .bind(&status)
        .bind(new_status)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(Some(new_status.to_string()))
    }

    async fn idem_abort(&self, key: &str, merchant: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM idempotency_keys WHERE key = $1 AND merchant_id = $2")
            .bind(key)
            .bind(merchant)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }
}

// ---------- In-memory (dev/testes) ----------

#[derive(Default)]
struct MemDb {
    payments: HashMap<uuid::Uuid, PaymentRow>,
    by_external: HashMap<(String, String), uuid::Uuid>,
    idem: HashMap<(String, String), (IdemRecord, chrono::DateTime<chrono::Utc>)>,
    refunds: HashMap<uuid::Uuid, Vec<i64>>,
    refund_ids: HashMap<uuid::Uuid, uuid::Uuid>,
}

#[derive(Default)]
pub struct MemStore {
    db: Mutex<MemDb>,
}

fn idem_lifetime() -> chrono::Duration {
    chrono::Duration::hours(24)
}

#[async_trait::async_trait]
impl Store for MemStore {
    async fn idem_begin(
        &self,
        key: &str,
        merchant: &str,
        request_hash: &str,
    ) -> Result<IdemBegin, StoreError> {
        let mut db = self.db.lock().expect("memdb");
        let k = (key.to_string(), merchant.to_string());
        match db.idem.get(&k) {
            None => {
                db.idem.insert(
                    k,
                    (
                        IdemRecord {
                            request_hash: request_hash.to_string(),
                            response: None,
                            status_code: None,
                        },
                        chrono::Utc::now() + idem_lifetime(),
                    ),
                );
                Ok(IdemBegin::New)
            }
            Some((_, exp)) if *exp < chrono::Utc::now() => {
                db.idem.insert(
                    k,
                    (
                        IdemRecord {
                            request_hash: request_hash.to_string(),
                            response: None,
                            status_code: None,
                        },
                        chrono::Utc::now() + idem_lifetime(),
                    ),
                );
                Ok(IdemBegin::New)
            }
            Some((rec, _)) => {
                if rec.request_hash != request_hash {
                    return Ok(IdemBegin::Conflict);
                }
                match (&rec.response, rec.status_code) {
                    (Some(resp), Some(code)) => Ok(IdemBegin::Replay(resp.clone(), code)),
                    _ => Ok(IdemBegin::InFlight),
                }
            }
        }
    }

    async fn idem_finish(
        &self,
        key: &str,
        merchant: &str,
        response: &serde_json::Value,
        status_code: i32,
    ) -> Result<(), StoreError> {
        let mut db = self.db.lock().expect("memdb");
        if let Some((rec, _)) = db.idem.get_mut(&(key.to_string(), merchant.to_string())) {
            rec.response = Some(response.clone());
            rec.status_code = Some(status_code);
        }
        Ok(())
    }

    async fn insert_payment(&self, p: &NewPayment) -> Result<PaymentRow, StoreError> {
        let mut db = self.db.lock().expect("memdb");
        let row = PaymentRow {
            id: uuid::Uuid::new_v4(),
            merchant_id: p.merchant_id.clone(),
            gateway: p.gateway.clone(),
            method: p.method.clone(),
            amount_minor: p.amount_minor,
            currency: p.currency.clone(),
            status: PENDING.to_string(),
            external_id: None,
            idempotency_key: p.idempotency_key.clone(),
            metadata: p.metadata.clone(),
            version: 0,
            created_at: chrono::Utc::now(),
        };
        db.payments.insert(row.id, row.clone());
        Ok(row)
    }

    async fn get_payment(&self, id: Uuid) -> Result<Option<PaymentRow>, StoreError> {
        Ok(self.db.lock().expect("memdb").payments.get(&id).cloned())
    }

    async fn find_by_external(
        &self,
        gateway: &str,
        external_id: &str,
    ) -> Result<Option<PaymentRow>, StoreError> {
        let db = self.db.lock().expect("memdb");
        Ok(db
            .by_external
            .get(&(gateway.to_string(), external_id.to_string()))
            .and_then(|id| db.payments.get(id).cloned()))
    }

    async fn finalize_create(
        &self,
        id: Uuid,
        status: &str,
        external_id: &str,
        pix_details: Option<&serde_json::Value>,
    ) -> Result<PaymentRow, StoreError> {
        let mut db = self.db.lock().expect("memdb");
        let Some(p) = db.payments.get_mut(&id) else {
            return Err(StoreError("payment not found".to_string()));
        };
        p.status = status.to_string();
        p.external_id = Some(external_id.to_string());
        p.version += 1;
        if let Some(pix) = pix_details {
            p.metadata["pix_details"] = pix.clone();
        }
        let gateway = p.gateway.clone();
        let result = p.clone();
        db.by_external
            .insert((gateway, external_id.to_string()), id);
        Ok(result)
    }

    async fn mark_failed(&self, id: Uuid, status: &str) -> Result<(), StoreError> {
        let mut db = self.db.lock().expect("memdb");
        if let Some(p) = db.payments.get_mut(&id) {
            p.status = status.to_string();
            p.version += 1;
        }
        Ok(())
    }

    async fn apply_event(
        &self,
        gateway: &str,
        external_id: &str,
        to_status: &str,
        _payload: &serde_json::Value,
    ) -> Result<Option<PaymentRow>, StoreError> {
        let mut db = self.db.lock().expect("memdb");
        let Some(id) = db
            .by_external
            .get(&(gateway.to_string(), external_id.to_string()))
            .copied()
        else {
            return Ok(None);
        };
        let p = db.payments.get_mut(&id).expect("payment");
        if !can_transition(&p.status, to_status) || p.status == to_status {
            return Ok(Some(p.clone()));
        }
        p.status = to_status.to_string();
        p.version += 1;
        Ok(Some(p.clone()))
    }

    async fn sync_status(&self, id: Uuid, status: &str) -> Result<PaymentRow, StoreError> {
        let mut db = self.db.lock().expect("memdb");
        let Some(p) = db.payments.get_mut(&id) else {
            return Err(StoreError("payment not found".to_string()));
        };
        if p.status != status {
            p.status = status.to_string();
            p.version += 1;
        }
        Ok(p.clone())
    }

    async fn reserve_refund(
        &self,
        payment_id: Uuid,
        amount_minor: i64,
        _idempotency_key: &str,
    ) -> Result<ReserveOutcome, StoreError> {
        let mut db = self.db.lock().expect("memdb");
        let Some(p) = db.payments.get(&payment_id).cloned() else {
            return Ok(ReserveOutcome::NotFound);
        };
        if p.status != PAID && p.status != PARTIALLY_REFUNDED {
            return Ok(ReserveOutcome::InvalidState { status: p.status });
        }
        let refunded: i64 = db
            .refunds
            .get(&payment_id)
            .map(|v| v.iter().sum())
            .unwrap_or(0);
        let amount = if amount_minor <= 0 {
            p.amount_minor - refunded
        } else {
            amount_minor
        };
        if amount <= 0 || refunded + amount > p.amount_minor {
            return Ok(ReserveOutcome::ExceedsPaid {
                remaining: p.amount_minor - refunded,
            });
        }
        let refund_id = uuid::Uuid::new_v4();
        db.refunds.entry(payment_id).or_default().push(amount);
        db.refund_ids.insert(refund_id, payment_id);
        Ok(ReserveOutcome::Reserved { refund_id, amount })
    }

    async fn settle_refund(
        &self,
        refund_id: Uuid,
        _external_id: &str,
        ok: bool,
    ) -> Result<Option<String>, StoreError> {
        let mut db = self.db.lock().expect("memdb");
        let Some(&payment_id) = db.refund_ids.get(&refund_id) else {
            return Ok(None);
        };
        if !ok {
            if let Some(v) = db.refunds.get_mut(&payment_id) {
                v.pop();
            }
            db.refund_ids.remove(&refund_id);
            return Ok(None);
        }
        let paid = db
            .payments
            .get(&payment_id)
            .map(|p| p.amount_minor)
            .unwrap_or(0);
        let refunded: i64 = db
            .refunds
            .get(&payment_id)
            .map(|v| v.iter().sum())
            .unwrap_or(0);
        let new_status = if refunded >= paid {
            REFUNDED
        } else {
            PARTIALLY_REFUNDED
        };
        if let Some(p) = db.payments.get_mut(&payment_id) {
            p.status = new_status.to_string();
            p.version += 1;
        }
        Ok(Some(new_status.to_string()))
    }

    async fn idem_abort(&self, key: &str, merchant: &str) -> Result<(), StoreError> {
        let mut db = self.db.lock().expect("memdb");
        db.idem.remove(&(key.to_string(), merchant.to_string()));
        Ok(())
    }
}
