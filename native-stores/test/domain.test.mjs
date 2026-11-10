import test from 'node:test';
import assert from 'node:assert/strict';
import { encryption, googleEntitlement, appleEntitlement, assertOwner } from '../src/domain.mjs';

const owner = '00000000-0000-4000-8000-000000000001';
const now = Date.parse('2026-01-15T00:00:00Z');
const config = { products: { monthly: 'monthly', yearly: 'yearly' }, bundleId: 'city.bussola.profissional', environment: 'Sandbox' };
function google(state = 'SUBSCRIPTION_STATE_ACTIVE') {
  return { subscriptionState: state, startTime: '2026-01-01T00:00:00Z',
    acknowledgementState: 'ACKNOWLEDGEMENT_STATE_PENDING',
    externalAccountIdentifiers: { obfuscatedExternalAccountId: owner },
    lineItems: [{ productId: 'monthly', expiryTime: '2026-02-01T00:00:00Z',
      offerDetails: { basePlanId: 'mensal' }, autoRenewingPlan: { autoRenewEnabled: true } }] };
}
function apple() {
  return { appAccountToken: owner, bundleId: config.bundleId, environment: 'Sandbox',
    type: 'Auto-Renewable Subscription', inAppOwnershipType: 'PURCHASED',
    productId: 'monthly', originalTransactionId: '1234',
    purchaseDate: Date.parse('2026-01-01T00:00:00Z'), expiresDate: Date.parse('2026-02-01T00:00:00Z') };
}

for (const [state, active] of [
  ['ACTIVE', true], ['IN_GRACE_PERIOD', true], ['CANCELED', true],
  ['ON_HOLD', false], ['PAUSED', false], ['PENDING', false], ['EXPIRED', false], ['PENDING_PURCHASE_CANCELED', false],
]) test(`Google canonical ${state} entitlement`, () => {
  const result = googleEntitlement(google(`SUBSCRIPTION_STATE_${state}`), 'dummy-test-proof', config, now);
  assert.equal(result.active, active);
  assert.equal(result.professionalId, owner);
  assert.equal(result.plan, 'MENSAL');
  assert.notEqual(result.externalId, 'dummy-test-proof');
});

test('Google expiry overrides an active status', () => {
  assert.equal(googleEntitlement(google(), 'dummy', config, now + 60 * 86400000).active, false);
});
test('Google account, product and base-plan substitution are rejected', () => {
  const data = google();
  delete data.externalAccountIdentifiers;
  assert.throws(() => googleEntitlement(data, 'dummy', config, now));
  const wrongProduct = google(); wrongProduct.lineItems[0].productId = 'attacker';
  assert.throws(() => googleEntitlement(wrongProduct, 'dummy', config, now));
  const wrongBase = google(); wrongBase.lineItems[0].offerDetails.basePlanId = 'prepaid';
  assert.throws(() => googleEntitlement(wrongBase, 'dummy', config, now));
});
test('Google sandbox is opt-in, never silently grants production access', () => {
  const data = { ...google(), testPurchase: {} };
  assert.throws(() => googleEntitlement(data, 'dummy', config, now));
  assert.equal(googleEntitlement(data, 'dummy', { ...config, allowTest: true }, now).environment, 'Sandbox');
});
test('Google cancellation preserves the paid period, without renewing', () => {
  const data = google('SUBSCRIPTION_STATE_CANCELED'); data.lineItems[0].autoRenewingPlan.autoRenewEnabled = false;
  const result = googleEntitlement(data, 'dummy', config, now);
  assert.equal(result.active, true); assert.equal(result.cancelAtPeriodEnd, true);
});
test('Apple revocation overrides a paid future expiry', () => {
  const result = appleEntitlement({ ...apple(), revocationDate: now }, {}, 1, config, now);
  assert.equal(result.active, false); assert.equal(result.storeStatus, '5');
});
test('Apple billing retry alone cannot grant access', () => {
  assert.equal(appleEntitlement(apple(), {}, 3, config, now).active, false);
});
test('Apple verified grace period extends access only to its signed end', () => {
  const result = appleEntitlement({ ...apple(), expiresDate: now - 1 }, { gracePeriodExpiresDate: now + 1000 }, 4, config, now);
  assert.equal(result.active, true); assert.equal(Date.parse(result.expiresAt), now + 1000);
});
test('Apple app, environment, product and family ownership are enforced', () => {
  for (const override of [{ bundleId: 'attacker' }, { environment: 'Production' },
    { productId: 'attacker' }, { inAppOwnershipType: 'FAMILY_SHARED' }, { appAccountToken: null }]) {
    assert.throws(() => appleEntitlement({ ...apple(), ...override }, {}, 1, config, now));
  }
});
test('Receipt credentials are encrypted and tampering fails closed', () => {
  const cipher = encryption(Buffer.alloc(32, 7).toString('base64'));
  const sealed = cipher.seal('dummy-test-proof');
  assert.equal(cipher.open(sealed), 'dummy-test-proof');
  assert.notEqual(cipher.seal('dummy-test-proof'), sealed);
  const damaged = Buffer.from(sealed, 'base64'); damaged[damaged.length - 1] ^= 1;
  assert.throws(() => cipher.open(damaged.toString('base64')));
  assert.throws(() => encryption('invalid'));
  assert.throws(() => assertOwner('not-a-uuid'));
});
