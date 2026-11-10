import { readFileSync } from 'node:fs';
import { AppStoreServerAPIClient, Environment, SignedDataVerifier, VerificationException, VerificationStatus } from '@apple/app-store-server-library';
import { GoogleAuth, OAuth2Client } from 'google-auth-library';
import { StoreError, appleEntitlement, assertOwner, googleEntitlement } from './domain.mjs';

export class Providers {
  constructor(env = process.env) {
    this.env = env;
    if (env.APPLE_STORE_ENVIRONMENT && !['Production', 'Sandbox'].includes(env.APPLE_STORE_ENVIRONMENT)) {
      throw new Error('APPLE_STORE_ENVIRONMENT must be Production or Sandbox.');
    }
    this.allowAppleSandboxFallback = env.APPLE_ALLOW_SANDBOX_FALLBACK === 'true';
    this.appleSandboxAccounts = new Set((env.APPLE_SANDBOX_ACCOUNT_IDS ?? '').split(',').map(id => id.trim()).filter(Boolean).map(assertOwner));
    if (this.allowAppleSandboxFallback &&
        (env.APPLE_STORE_ENVIRONMENT === 'Sandbox' || this.appleSandboxAccounts.size === 0)) {
      throw new Error('Apple production sandbox fallback requires an explicit account allowlist.');
    }
    this.googleConfig = {
      packageName: env.GOOGLE_PLAY_PACKAGE_NAME ?? 'city.bussola.profissional',
      allowTest: env.STORE_ALLOW_SANDBOX_PURCHASES === 'true',
      products: {
        monthly: env.GOOGLE_PLAY_MONTHLY_PRODUCT ?? 'bussola_pro_mensal',
        yearly: env.GOOGLE_PLAY_YEARLY_PRODUCT ?? 'bussola_pro_anual',
      },
    };
    this.appleConfig = {
      bundleId: env.APPLE_BUNDLE_ID ?? 'city.bussola.profissional',
      environment: env.APPLE_STORE_ENVIRONMENT === 'Sandbox' ? Environment.SANDBOX : Environment.PRODUCTION,
      products: {
        monthly: env.APPLE_MONTHLY_PRODUCT ?? 'city.bussola.profissional.mensal',
        yearly: env.APPLE_YEARLY_PRODUCT ?? 'city.bussola.profissional.anual',
      },
    };
    this.googleAuth = new GoogleAuth({ scopes: ['https://www.googleapis.com/auth/androidpublisher'] });
    this.oidc = new OAuth2Client();
    if (env.APPLE_KEY_ID && env.APPLE_ISSUER_ID && env.APPLE_PRIVATE_KEY_FILE && env.APPLE_ROOT_CERTIFICATES) {
      const appAppleId = env.APPLE_APP_ID ? Number(env.APPLE_APP_ID) : undefined;
      if ((this.appleConfig.environment === Environment.PRODUCTION || appAppleId !== undefined) &&
          (!Number.isSafeInteger(appAppleId) || appAppleId <= 0)) {
        throw new Error('APPLE_APP_ID must be a positive numeric app identifier.');
      }
      const roots = env.APPLE_ROOT_CERTIFICATES.split(',').map(path => readFileSync(path.trim()));
      this.verifier = new SignedDataVerifier(
        roots, true, this.appleConfig.environment, this.appleConfig.bundleId,
        appAppleId,
      );
      this.apple = new AppStoreServerAPIClient(
        readFileSync(env.APPLE_PRIVATE_KEY_FILE, 'utf8'), env.APPLE_KEY_ID, env.APPLE_ISSUER_ID,
        this.appleConfig.bundleId, this.appleConfig.environment,
      );
      if (this.allowAppleSandboxFallback) {
        this.sandboxVerifier = new SignedDataVerifier(roots, true, Environment.SANDBOX, this.appleConfig.bundleId, appAppleId);
        this.sandboxApple = new AppStoreServerAPIClient(
          readFileSync(env.APPLE_PRIVATE_KEY_FILE, 'utf8'), env.APPLE_KEY_ID, env.APPLE_ISSUER_ID,
          this.appleConfig.bundleId, Environment.SANDBOX,
        );
      }
    }
  }

  permits(value) {
    if (value.environment !== Environment.SANDBOX) return true;
    if (value.provider === 'google') return this.googleConfig.allowTest;
    return this.appleConfig.environment === Environment.SANDBOX ||
      (this.allowAppleSandboxFallback && this.appleSandboxAccounts.has(value.professionalId?.toLowerCase()));
  }

  appleChannel(environment = this.appleConfig.environment) {
    if (environment === this.appleConfig.environment && this.apple && this.verifier) {
      return { client: this.apple, verifier: this.verifier, environment };
    }
    if (environment === Environment.SANDBOX && this.allowAppleSandboxFallback && this.sandboxApple && this.sandboxVerifier) {
      return { client: this.sandboxApple, verifier: this.sandboxVerifier, environment };
    }
    throw new StoreError('Apple environment is not configured.', 'FAILED_PRECONDITION');
  }

  async verifyApple(method, proof) {
    const primary = this.appleChannel();
    try { return { decoded: await primary.verifier[method](proof), environment: primary.environment }; }
    catch (error) {
      // Sandbox notifications may omit appAppleId. Reverify the full chain/app/environment, never bypass signatures.
      const mismatch = error instanceof VerificationException &&
        (error.status === VerificationStatus.INVALID_ENVIRONMENT ||
         (method === 'verifyAndDecodeNotification' && error.status === VerificationStatus.INVALID_APP_IDENTIFIER));
      if (!mismatch || !this.allowAppleSandboxFallback) throw error;
      const sandbox = this.appleChannel(Environment.SANDBOX);
      return { decoded: await sandbox.verifier[method](proof), environment: sandbox.environment };
    }
  }

  assertAppleAccount(transaction, environment) {
    if (!this.permits({ provider: 'apple', environment, professionalId: assertOwner(transaction.appAccountToken) })) {
      throw new StoreError('Sandbox account is not allowed.', 'PERMISSION_DENIED');
    }
  }

  async google(token) {
    if (typeof token !== 'string' || !token || token.length > 4096) throw new StoreError('Invalid purchase token.');
    const client = await this.googleAuth.getClient();
    const url = `https://androidpublisher.googleapis.com/androidpublisher/v3/applications/${encodeURIComponent(this.googleConfig.packageName)}/purchases/subscriptionsv2/tokens/${encodeURIComponent(token)}`;
    const { data } = await client.request({ url, timeout: 15000 });
    return googleEntitlement(data, token, this.googleConfig);
  }

  async ready(provider) {
    if (provider === 'apple') {
      if (!this.apple || !this.verifier) throw new StoreError('Apple is not configured.', 'FAILED_PRECONDITION');
      return;
    }
    if (provider !== 'google') throw new StoreError('Invalid provider.');
    const client = await this.googleAuth.getClient();
    const credentials = await client.getAccessToken();
    if (!credentials?.token) throw new StoreError('Google credentials are unavailable.', 'FAILED_PRECONDITION');
  }

  async acknowledge(token, productId) {
    const client = await this.googleAuth.getClient();
    await client.request({
      url: `https://androidpublisher.googleapis.com/androidpublisher/v3/applications/${encodeURIComponent(this.googleConfig.packageName)}/purchases/subscriptions/${encodeURIComponent(productId)}/tokens/${encodeURIComponent(token)}:acknowledge`,
      method: 'POST', data: {}, timeout: 15000,
    });
  }

  async appleTransaction(proof) {
    const { decoded: transaction, environment } = await this.verifyApple('verifyAndDecodeTransaction', proof);
    this.assertAppleAccount(transaction, environment);
    return this.appleSubscription(String(transaction.originalTransactionId), environment);
  }

  async appleSubscription(originalId, environment = this.appleConfig.environment) {
    const { client, verifier } = this.appleChannel(environment);
    if (!/^\d{1,40}$/.test(originalId)) throw new StoreError('Invalid transaction identifier.');
    const response = await client.getAllSubscriptionStatuses(originalId);
    const candidates = [];
    for (const group of response.data ?? []) {
      for (const item of group.lastTransactions ?? []) {
        if (String(item.originalTransactionId) !== originalId || !item.signedTransactionInfo) continue;
        const transaction = await verifier.verifyAndDecodeTransaction(item.signedTransactionInfo);
        this.assertAppleAccount(transaction, environment);
        if (String(transaction.originalTransactionId) !== originalId) throw new StoreError('Transaction chain mismatch.');
        const renewal = item.signedRenewalInfo
          ? await verifier.verifyAndDecodeRenewalInfo(item.signedRenewalInfo) : undefined;
        candidates.push({ value: appleEntitlement(transaction, renewal, item.status, { ...this.appleConfig, environment }), date: transaction.purchaseDate });
      }
    }
    candidates.sort((a, b) => b.date - a.date);
    if (!candidates[0]) throw new StoreError('Subscription not found.');
    return candidates[0].value;
  }

  async notification(provider, raw, authorization) {
    let body;
    try { body = JSON.parse(raw); } catch { throw new StoreError('Invalid notification.'); }
    if (provider === 'apple') {
      const { decoded: notification, environment } = await this.verifyApple('verifyAndDecodeNotification', body.signedPayload);
      if (notification.notificationType === 'TEST') return null;
      if (!notification.data?.signedTransactionInfo) throw new StoreError('Missing signed transaction.');
      const { verifier } = this.appleChannel(environment);
      const transaction = await verifier.verifyAndDecodeTransaction(notification.data.signedTransactionInfo);
      this.assertAppleAccount(transaction, environment);
      return { value: await this.appleSubscription(String(transaction.originalTransactionId), environment), lookup: String(transaction.originalTransactionId) };
    }
    if (provider !== 'google' || !this.env.GOOGLE_PUBSUB_AUDIENCE || !this.env.GOOGLE_PUBSUB_SERVICE_ACCOUNT) {
      throw new StoreError('Push identity is not configured.', 'FAILED_PRECONDITION');
    }
    const bearer = authorization?.match(/^Bearer (.+)$/)?.[1];
    if (!bearer) throw new StoreError('Push identity is required.', 'UNAUTHENTICATED');
    const ticket = await this.oidc.verifyIdToken({ idToken: bearer, audience: this.env.GOOGLE_PUBSUB_AUDIENCE });
    const identity = ticket.getPayload();
    if (identity?.email_verified !== true || identity.email !== this.env.GOOGLE_PUBSUB_SERVICE_ACCOUNT) {
      throw new StoreError('Invalid push identity.', 'UNAUTHENTICATED');
    }
    let data;
    try { data = JSON.parse(Buffer.from(body.message.data, 'base64').toString('utf8')); }
    catch { throw new StoreError('Invalid RTDN data.'); }
    if (data.packageName !== this.googleConfig.packageName) throw new StoreError('Package mismatch.');
    if (data.testNotification) return null;
    const token = data.subscriptionNotification?.purchaseToken ?? data.voidedPurchaseNotification?.purchaseToken;
    return { value: await this.google(token), lookup: token };
  }
}
