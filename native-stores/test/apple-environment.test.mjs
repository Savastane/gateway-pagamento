import test from 'node:test';
import assert from 'node:assert/strict';
import { Environment, VerificationException, VerificationStatus } from '@apple/app-store-server-library';
import { Providers } from '../src/providers.mjs';
import { NativeStores } from '../src/service.mjs';

const owner = '00000000-0000-4000-8000-000000000001';
const other = '00000000-0000-4000-8000-000000000002';
function setup(account = owner) {
  const providers = new Providers({ APPLE_ALLOW_SANDBOX_FALLBACK: 'true', APPLE_SANDBOX_ACCOUNT_IDS: owner });
  const events = [];
  const transaction = { appAccountToken: account, environment: 'Sandbox', bundleId: 'city.bussola.profissional',
    originalTransactionId: '12345', productId: providers.appleConfig.products.monthly,
    type: 'Auto-Renewable Subscription', inAppOwnershipType: 'PURCHASED',
    purchaseDate: Date.parse('2026-01-01T00:00:00Z'), expiresDate: Date.parse('2099-01-01T00:00:00Z') };
  providers.verifier = {
    async verifyAndDecodeTransaction() { throw new VerificationException(VerificationStatus.INVALID_ENVIRONMENT); },
    async verifyAndDecodeNotification() { throw new VerificationException(VerificationStatus.INVALID_APP_IDENTIFIER); },
  };
  providers.apple = { async getAllSubscriptionStatuses() { assert.fail('Sandbox receipt cannot query production'); } };
  providers.sandboxVerifier = {
    async verifyAndDecodeTransaction() { events.push('sandbox-verify'); return transaction; },
    async verifyAndDecodeNotification() { events.push('sandbox-push-verify'); return { data: { signedTransactionInfo: 'dummy-signed-transaction' } }; },
    async verifyAndDecodeRenewalInfo() { return { autoRenewStatus: 1 }; },
  };
  providers.sandboxApple = { async getAllSubscriptionStatuses(id) {
    assert.equal(id, '12345'); events.push('sandbox-api');
    return { data: [{ lastTransactions: [{ originalTransactionId: id, status: 1,
      signedTransactionInfo: 'dummy-signed-transaction', signedRenewalInfo: 'dummy-signed-renewal' }] }] };
  } };
  return { providers, events };
}

test('App Review sandbox fallback requires an explicit account allowlist and valid environment', () => {
  assert.throws(() => new Providers({ APPLE_ALLOW_SANDBOX_FALLBACK: 'true' }), /allowlist/);
  assert.throws(() => new Providers({ APPLE_STORE_ENVIRONMENT: 'sandbox' }), /Production or Sandbox/);
  assert.throws(() => new Providers({ APPLE_ALLOW_SANDBOX_FALLBACK: 'true', APPLE_SANDBOX_ACCOUNT_IDS: 'not-a-uuid' }));
});
test('Allowlisted App Review receipt is verified and reread exclusively in Sandbox', async () => {
  const { providers, events } = setup();
  const result = await providers.appleTransaction('dummy-signed-transaction');
  assert.equal(result.environment, Environment.SANDBOX);
  assert.equal(result.active, true); assert.equal(result.professionalId, owner);
  assert.deepEqual(events, ['sandbox-verify', 'sandbox-api', 'sandbox-verify']);
});
test('Valid sandbox signature alone cannot grant access to a non-allowlisted production account', async () => {
  const { providers, events } = setup(other);
  await assert.rejects(providers.appleTransaction('dummy'), { code: 'PERMISSION_DENIED' });
  assert.deepEqual(events, ['sandbox-verify']);
});
test('Bad signature, certificate, retryable validation failure or wrong transaction app never trigger fallback', async () => {
  for (const status of [VerificationStatus.VERIFICATION_FAILURE, VerificationStatus.INVALID_CERTIFICATE,
    VerificationStatus.RETRYABLE_VERIFICATION_FAILURE, VerificationStatus.INVALID_APP_IDENTIFIER]) {
    const { providers, events } = setup();
    const error = new VerificationException(status);
    providers.verifier.verifyAndDecodeTransaction = async () => { throw error; };
    await assert.rejects(providers.appleTransaction('dummy'), error);
    assert.deepEqual(events, []);
  }
});
test('Sandbox notification without production appAppleId is fully reverified in Sandbox', async () => {
  const { providers, events } = setup();
  const result = await providers.notification('apple', JSON.stringify({ signedPayload: 'dummy-signed-notification' }));
  assert.equal(result.value.professionalId, owner); assert.equal(result.value.environment, Environment.SANDBOX);
  assert.equal(events[0], 'sandbox-push-verify'); assert.ok(events.includes('sandbox-api'));
});
test('Production fallback remains disabled by default', async () => {
  const { providers, events } = setup();
  providers.allowAppleSandboxFallback = false;
  await assert.rejects(providers.appleTransaction('dummy'), { status: VerificationStatus.INVALID_ENVIRONMENT });
  assert.deepEqual(events, []);
});
test('Removing a sandbox account revokes cached rights, not only future purchases', async () => {
  const { providers } = setup();
  let value = { provider: 'apple', externalId: '12345', professionalId: owner, environment: 'Sandbox', active: true, version: 1 };
  const repository = {
    async snapshot() { return [value]; },
    async blockSandbox(provider, id) {
      assert.equal(provider, 'apple'); assert.equal(id, '12345');
      value = { ...value, active: false, storeStatus: 'SANDBOX_DISABLED', version: 2 };
    },
  };
  const service = new NativeStores(repository, providers);
  assert.equal((await service.snapshot(owner))[0].active, true);
  providers.appleSandboxAccounts.delete(owner);
  assert.equal((await service.snapshot(owner))[0].active, false);
  assert.equal((await service.snapshot(owner))[0].storeStatus, 'SANDBOX_DISABLED');
});
