import test from 'node:test';
import assert from 'node:assert/strict';
import { NativeStores } from '../src/service.mjs';
import { maintenanceTick } from '../src/worker.mjs';

test('A damaged encrypted credential cannot prevent other records from being reconciled', async () => {
  const events = [];
  const repository = {
    async due() { return [
      { provider: 'google', externalId: 'damaged', owner: 'owner', credentialEnc: 'bad' },
      { provider: 'apple', externalId: 'healthy', owner: 'owner', credentialEnc: 'valid', environment: 'Sandbox' },
    ]; },
    credential(record) { if (record.externalId === 'damaged') throw new Error('bad authentication tag'); return 'dummy-proof'; },
    async retryLater(provider, id) { events.push(`retry:${id}`); },
  };
  const service = new NativeStores(repository, {});
  service.sync = async (provider, id, lookup, owner, environment) => {
    assert.equal(environment, 'Sandbox'); assert.equal(lookup, 'dummy-proof'); events.push(`sync:${id}`);
  };
  await service.maintenance();
  assert.deepEqual(events, ['retry:damaged', 'sync:healthy']);
});
test('Delivery proceeds even if reconciliation fails and logs never include failure contents', async () => {
  const events = [];
  const repository = {
    async pendingDelivery() { return [{ version: 3 }]; },
    async delivered(value) { events.push(`delivered:${value.version}`); },
  };
  await maintenanceTick({ async maintenance() { throw new Error('dummy-private-error-must-not-be-logged'); } },
    repository, async () => true, (...args) => { assert.equal(args.length, 0); events.push('warn'); });
  assert.deepEqual(events, ['delivered:3', 'warn']);
});
test('Failed callbacks remain pending without starving later deliveries or reconciliation', async () => {
  const delivered = [], calls = [];
  const repository = {
    async pendingDelivery() { return [{ version: 1 }, { version: 2 }]; },
    async delivered(value) { delivered.push(value.version); },
  };
  await maintenanceTick({ async maintenance() { calls.push('reconcile'); } }, repository,
    async value => value.version === 2);
  assert.deepEqual(delivered, [2]); assert.deepEqual(calls, ['reconcile']);
});
test('Delivery storage failure cannot disable the reconciliation phase', async () => {
  let reconciled = false;
  await maintenanceTick({ async maintenance() { reconciled = true; } },
    { async pendingDelivery() { throw new Error('database temporarily unavailable'); } }, async () => true);
  assert.equal(reconciled, true);
});
