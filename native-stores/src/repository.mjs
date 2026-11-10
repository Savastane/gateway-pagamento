import { StoreError, assertOwner, encryption } from './domain.mjs';

function publicRow(row) {
  return {
    provider: row.provider, externalId: row.external_id, professionalId: row.professional_id,
    productId: row.product_id, plan: row.plan, environment: row.environment,
    storeStatus: row.store_status, startsAt: row.starts_at.toISOString(),
    expiresAt: row.expires_at.toISOString(), active: row.active && row.expires_at > new Date(),
    cancelAtPeriodEnd: row.cancel_at_period_end, version: Number(row.version),
  };
}

export class Repository {
  constructor(pool, key) { this.pool = pool; this.crypto = encryption(key); }

  async sync(provider, externalId, lookup, read, expectedOwner) {
    const db = await this.pool.connect();
    try {
      await db.query('BEGIN');
      await db.query('SELECT pg_advisory_xact_lock(hashtextextended($1, 0))', [`${provider}:${externalId}`]);
      const { rows: [existing] } = await db.query(
        'SELECT * FROM native_store_subscriptions WHERE provider=$1 AND external_id=$2 FOR UPDATE', [provider, externalId],
      );
      // Fetch canonical state under the lock; never apply an old push payload as current truth.
      const value = await read();
      const owner = assertOwner(value.professionalId);
      if (value.provider !== provider || value.externalId !== externalId ||
          (expectedOwner && assertOwner(expectedOwner) !== owner) ||
          (existing && (existing.professional_id !== owner || existing.environment !== value.environment))) {
        throw new StoreError('Purchase belongs to a different account.', 'PERMISSION_DENIED');
      }
      const { rows: [row] } = await db.query(`
        INSERT INTO native_store_subscriptions(
          provider,external_id,professional_id,credential_enc,product_id,plan,environment,
          store_status,starts_at,expires_at,active,cancel_at_period_end,acknowledge_pending,next_check_at)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,now()+interval '5 minutes')
        ON CONFLICT(provider,external_id) DO UPDATE SET
          product_id=EXCLUDED.product_id, plan=EXCLUDED.plan, store_status=EXCLUDED.store_status,
          starts_at=EXCLUDED.starts_at, expires_at=EXCLUDED.expires_at, active=EXCLUDED.active,
          cancel_at_period_end=EXCLUDED.cancel_at_period_end, acknowledge_pending=EXCLUDED.acknowledge_pending,
          version=native_store_subscriptions.version+CASE WHEN
            ROW(native_store_subscriptions.product_id,native_store_subscriptions.plan,native_store_subscriptions.store_status,
                native_store_subscriptions.starts_at,native_store_subscriptions.expires_at,
                native_store_subscriptions.active,native_store_subscriptions.cancel_at_period_end)
            IS DISTINCT FROM ROW(EXCLUDED.product_id,EXCLUDED.plan,EXCLUDED.store_status,
                EXCLUDED.starts_at,EXCLUDED.expires_at,EXCLUDED.active,EXCLUDED.cancel_at_period_end)
            THEN 1 ELSE 0 END,
          updated_at=now(), next_check_at=now()+interval '5 minutes'
        RETURNING *`,
      [provider, externalId, owner, this.crypto.seal(lookup), value.productId, value.plan, value.environment,
        value.storeStatus, value.startsAt, value.expiresAt, value.active, value.cancelAtPeriodEnd, value.acknowledgePending]);
      await db.query('COMMIT');
      return { ...publicRow(row), acknowledgePending: row.acknowledge_pending };
    } catch (error) {
      await db.query('ROLLBACK');
      throw error;
    } finally { db.release(); }
  }

  async snapshot(owner) {
    const { rows } = await this.pool.query(
      'SELECT * FROM native_store_subscriptions WHERE professional_id=$1 ORDER BY updated_at DESC', [assertOwner(owner)],
    );
    return rows.map(publicRow);
  }

  async acknowledged(provider, externalId) {
    await this.pool.query('UPDATE native_store_subscriptions SET acknowledge_pending=false WHERE provider=$1 AND external_id=$2',
      [provider, externalId]);
  }

  async due() {
    const { rows } = await this.pool.query('SELECT * FROM native_store_subscriptions WHERE next_check_at<=now() ORDER BY next_check_at LIMIT 25');
    return rows.map(row => ({ provider: row.provider, externalId: row.external_id,
      credentialEnc: row.credential_enc, owner: row.professional_id, environment: row.environment }));
  }

  credential(record) { return this.crypto.open(record.credentialEnc); }

  async blockSandbox(provider, externalId) {
    await this.pool.query(`UPDATE native_store_subscriptions SET
      version=version+CASE WHEN active OR store_status<>'SANDBOX_DISABLED' THEN 1 ELSE 0 END,
      active=false, store_status='SANDBOX_DISABLED', updated_at=now(),next_check_at=now()+interval '5 minutes'
      WHERE provider=$1 AND external_id=$2`, [provider, externalId]);
  }

  async retryLater(provider, externalId) {
    await this.pool.query("UPDATE native_store_subscriptions SET next_check_at=now()+interval '5 minutes' WHERE provider=$1 AND external_id=$2",
      [provider, externalId]);
  }

  async pendingDelivery() {
    const { rows } = await this.pool.query('SELECT * FROM native_store_subscriptions WHERE delivered_version<version ORDER BY updated_at LIMIT 25');
    return rows.map(publicRow);
  }

  async delivered(value) {
    await this.pool.query('UPDATE native_store_subscriptions SET delivered_version=GREATEST(delivered_version,$3) WHERE provider=$1 AND external_id=$2',
      [value.provider, value.externalId, value.version]);
  }
}
