import test from 'node:test';
import assert from 'node:assert/strict';
import { NativeStores } from '../src/service.mjs';
import { StoreError } from '../src/domain.mjs';
import { Providers } from '../src/providers.mjs';
import { SignedDataVerifier, Environment } from '@apple/app-store-server-library';

const owner = '00000000-0000-4000-8000-000000000001';
test('Purchase preflight fails closed without Apple configuration or a Google access token', async () => {
  const providers = new Providers({});
  await assert.rejects(providers.ready('apple'), { code: 'FAILED_PRECONDITION' });
  providers.googleAuth = { async getClient() { return { async getAccessToken() { return { token: null }; } }; } };
  await assert.rejects(providers.ready('google'), { code: 'FAILED_PRECONDITION' });
  providers.googleAuth = { async getClient() { return { async getAccessToken() { return { token: 'dummy-test-token' }; } }; } };
  await providers.ready('google');
});
test('Google acknowledgement happens only after verified ownership and durable persistence', async () => {
  const events = [];
  const repository = {
    async sync(provider, id, token, read, expected) {
      assert.equal(expected, owner); await read(); events.push('committed');
      return { productId: 'monthly', acknowledgePending: true };
    },
    async acknowledged() { events.push('ack-recorded'); },
    async snapshot() { return []; },
  };
  const providers = { async google() { events.push('verified'); }, async acknowledge() { events.push('ack'); } };
  await new NativeStores(repository, providers).execute({ operation: 'verify', provider: 'google', professionalId: owner, proof: 'dummy-test-proof' });
  assert.deepEqual(events, ['verified', 'committed', 'ack', 'ack-recorded']);
  repository.sync = async () => { throw new StoreError('Wrong owner', 'PERMISSION_DENIED'); };
  events.length = 0;
  await assert.rejects(new NativeStores(repository, providers).execute({ operation: 'verify', provider: 'google', professionalId: owner, proof: 'dummy' }));
  assert.deepEqual(events, []);
});
test('Forged Apple JWS is rejected by the official certificate verifier', async () => {
  const verifier = new SignedDataVerifier([], true, Environment.SANDBOX, 'city.bussola.profissional');
  await assert.rejects(verifier.verifyAndDecodeTransaction('e30.e30.invalid'));
});
test('Google push OIDC identity is checked before any store API lookup', async () => {
  const providers = new Providers({ GOOGLE_PUBSUB_AUDIENCE: 'https://example.test/push', GOOGLE_PUBSUB_SERVICE_ACCOUNT: 'push@example.test' });
  providers.google = async () => assert.fail('No store lookup before authenticated push');
  providers.oidc = { async verifyIdToken() { return { getPayload: () => ({ email_verified: false, email: 'push@example.test' }) }; } };
  await assert.rejects(providers.notification('google', '{}', 'Bearer dummy'), { code: 'UNAUTHENTICATED' });
  await assert.rejects(providers.notification('google', '{}', ''), { code: 'UNAUTHENTICATED' });
});
test('Verified Google push cannot target another package', async () => {
  const providers = new Providers({ GOOGLE_PUBSUB_AUDIENCE: 'https://example.test/push', GOOGLE_PUBSUB_SERVICE_ACCOUNT: 'push@example.test' });
  providers.oidc = { async verifyIdToken() { return { getPayload: () => ({ email_verified: true, email: 'push@example.test' }) }; } };
  providers.google = async () => assert.fail('No store lookup for a different app');
  const body = { message: { data: Buffer.from(JSON.stringify({ packageName: 'attacker', subscriptionNotification: { purchaseToken: 'dummy' } })).toString('base64') } };
  await assert.rejects(providers.notification('google', JSON.stringify(body), 'Bearer dummy'), /Package mismatch/);
});
