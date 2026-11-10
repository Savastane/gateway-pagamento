import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import pg from 'pg';
import { Repository } from '../src/repository.mjs';

test('Postgres: immutable owner, serial reconciliation, encrypted credential and durable delivery', {
  skip: !process.env.TEST_DATABASE_URL,
}, async () => {
  const admin = new pg.Pool({ connectionString: process.env.TEST_DATABASE_URL });
  const schema = `native_test_${randomUUID().replaceAll('-', '')}`;
  await admin.query(`CREATE SCHEMA "${schema}"`);
  const pool = new pg.Pool({ connectionString: process.env.TEST_DATABASE_URL, options: `-c search_path=${schema},public` });
  try {
    await pool.query(readFileSync(new URL('../../banco/migrations/0006_native_store_subscriptions.sql', import.meta.url), 'utf8'));
    const repository = new Repository(pool, Buffer.alloc(32, 9).toString('base64'));
    const owner = '00000000-0000-4000-8000-000000000001';
    const other = '00000000-0000-4000-8000-000000000002';
    const value = { provider: 'google', externalId: 'dummy-hash', professionalId: owner,
      productId: 'monthly', plan: 'MENSAL', environment: 'Sandbox', storeStatus: 'SUBSCRIPTION_STATE_ACTIVE',
      startsAt: '2026-01-01T00:00:00Z', expiresAt: '2099-01-01T00:00:00Z',
      active: true, cancelAtPeriodEnd: false, acknowledgePending: true };
    const first = await repository.sync('google', value.externalId, 'dummy-test-proof', async () => value, owner);
    assert.equal(first.version, 1);
    assert.equal((await repository.snapshot(other)).length, 0);
    const { rows: [stored] } = await pool.query('SELECT * FROM native_store_subscriptions');
    assert.notEqual(stored.credential_enc, 'dummy-test-proof');
    assert.equal(repository.crypto.open(stored.credential_enc), 'dummy-test-proof');
    assert.equal((await repository.pendingDelivery()).length, 1);
    await repository.delivered(first);
    assert.equal((await repository.pendingDelivery()).length, 0);
    const repeated = await repository.sync('google', value.externalId, 'dummy-test-proof', async () => value, owner);
    assert.equal(repeated.version, 1, 'Unchanged store data must not generate a new entitlement version');
    assert.equal((await repository.pendingDelivery()).length, 0);
    await assert.rejects(repository.sync('google', value.externalId, 'dummy', async () => ({ ...value, professionalId: other }), other),
      { code: 'PERMISSION_DENIED' });
    assert.equal((await repository.snapshot(owner))[0].version, 1);
    await assert.rejects(repository.sync('google', value.externalId, 'dummy', async () => ({ ...value, environment: 'Production' }), owner),
      { code: 'PERMISSION_DENIED' });

    let releaseRead, enteredRead;
    const barrier = new Promise(resolve => { releaseRead = resolve; });
    const entered = new Promise(resolve => { enteredRead = resolve; });
    const slow = repository.sync('google', value.externalId, 'dummy-test-proof', async () => {
      enteredRead(); await barrier; return value;
    }, owner);
    await entered;
    let secondRead = false;
    const revoked = repository.sync('google', value.externalId, 'dummy-test-proof', async () => {
      secondRead = true; return { ...value, active: false, storeStatus: 'SUBSCRIPTION_STATE_EXPIRED' };
    }, owner);
    await new Promise(resolve => setTimeout(resolve, 40));
    assert.equal(secondRead, false);
    releaseRead();
    await Promise.all([slow, revoked]);
    const final = (await repository.snapshot(owner))[0];
    assert.equal(final.version, 2); assert.equal(final.active, false);
    assert.equal(final.storeStatus, 'SUBSCRIPTION_STATE_EXPIRED');
    await repository.delivered(first);
    assert.equal((await repository.pendingDelivery()).length, 1, 'An old callback cannot acknowledge newer state');
    await repository.acknowledged('google', value.externalId);
    assert.equal((await pool.query('SELECT acknowledge_pending FROM native_store_subscriptions')).rows[0].acknowledge_pending, false);
    await repository.blockSandbox('google', value.externalId);
    assert.equal((await repository.snapshot(owner))[0].version, 3);
    await repository.blockSandbox('google', value.externalId);
    assert.equal((await repository.snapshot(owner))[0].version, 3, 'Policy blocks are idempotent too');
    assert.equal((await repository.snapshot(owner))[0].storeStatus, 'SANDBOX_DISABLED');
  } finally {
    await pool.end();
    await admin.query(`DROP SCHEMA "${schema}" CASCADE`);
    await admin.end();
  }
});
