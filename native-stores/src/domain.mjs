import { createHash, createCipheriv, createDecipheriv, randomBytes } from 'node:crypto';

export class StoreError extends Error {
  constructor(message, code = 'INVALID_ARGUMENT') { super(message); this.code = code; }
}

export function assertOwner(value) {
  if (typeof value !== 'string' || !/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(value)) {
    throw new StoreError('A valid account token is required.');
  }
  return value.toLowerCase();
}

export function productPlan(product, products) {
  if (product === products.monthly) return 'MENSAL';
  if (product === products.yearly) return 'ANUAL';
  throw new StoreError('Product is not allowed.');
}

export function timestamp(value) {
  const n = typeof value === 'number' ? value : Date.parse(value);
  if (!Number.isFinite(n) || n <= 0) throw new StoreError('Invalid store timestamp.');
  return new Date(n).toISOString();
}

export function tokenHash(token) { return createHash('sha256').update(token).digest('hex'); }

export function encryption(key) {
  const bytes = Buffer.from(key ?? '', 'base64');
  if (bytes.length !== 32) throw new Error('STORE_TOKEN_ENCRYPTION_KEY must be 32 random bytes in base64.');
  return {
    seal(value) {
      const iv = randomBytes(12);
      const cipher = createCipheriv('aes-256-gcm', bytes, iv);
      const data = Buffer.concat([cipher.update(value, 'utf8'), cipher.final()]);
      return Buffer.concat([iv, cipher.getAuthTag(), data]).toString('base64');
    },
    open(value) {
      const data = Buffer.from(value, 'base64');
      const cipher = createDecipheriv('aes-256-gcm', bytes, data.subarray(0, 12));
      cipher.setAuthTag(data.subarray(12, 28));
      return Buffer.concat([cipher.update(data.subarray(28)), cipher.final()]).toString('utf8');
    },
  };
}

export function googleEntitlement(data, token, config, now = Date.now()) {
  if (data.testPurchase && !config.allowTest) throw new StoreError('Sandbox purchases are disabled.');
  const owner = assertOwner(data.externalAccountIdentifiers?.obfuscatedExternalAccountId);
  const items = (data.lineItems ?? []).filter(item =>
    [config.products.monthly, config.products.yearly].includes(item.productId));
  if (items.length !== 1 || !items[0].autoRenewingPlan) throw new StoreError('Expected one auto-renewing subscription.');
  const item = items[0];
  const expectedBase = item.productId === config.products.yearly ? 'anual' : 'mensal';
  if (item.offerDetails?.basePlanId !== expectedBase) throw new StoreError('Base plan is not allowed.');
  const expiresAt = timestamp(item.expiryTime);
  const statuses = ['SUBSCRIPTION_STATE_ACTIVE', 'SUBSCRIPTION_STATE_IN_GRACE_PERIOD', 'SUBSCRIPTION_STATE_CANCELED'];
  return {
    provider: 'google', externalId: tokenHash(token), professionalId: owner,
    productId: item.productId, plan: productPlan(item.productId, config.products),
    environment: data.testPurchase ? 'Sandbox' : 'Production',
    storeStatus: data.subscriptionState, startsAt: timestamp(data.startTime),
    expiresAt, active: statuses.includes(data.subscriptionState) && Date.parse(expiresAt) > now,
    cancelAtPeriodEnd: item.autoRenewingPlan.autoRenewEnabled !== true,
    acknowledgePending: data.acknowledgementState === 'ACKNOWLEDGEMENT_STATE_PENDING' && statuses.includes(data.subscriptionState),
  };
}

export function appleEntitlement(transaction, renewal, status, config, now = Date.now()) {
  const owner = assertOwner(transaction.appAccountToken);
  if (transaction.bundleId !== config.bundleId || transaction.environment !== config.environment ||
      transaction.type !== 'Auto-Renewable Subscription' ||
      transaction.inAppOwnershipType !== 'PURCHASED') throw new StoreError('Invalid app or purchase type.');
  const expiresAt = timestamp(status === 4 && renewal?.gracePeriodExpiresDate
    ? renewal.gracePeriodExpiresDate : transaction.expiresDate);
  return {
    provider: 'apple', externalId: String(transaction.originalTransactionId), professionalId: owner,
    productId: transaction.productId, plan: productPlan(transaction.productId, config.products),
    environment: transaction.environment, storeStatus: String(transaction.revocationDate ? 5 : status),
    startsAt: timestamp(transaction.originalPurchaseDate ?? transaction.purchaseDate),
    expiresAt, active: !transaction.revocationDate && [1, 4].includes(status) && Date.parse(expiresAt) > now,
    cancelAtPeriodEnd: renewal?.autoRenewStatus === 0, acknowledgePending: false,
  };
}
