import { StoreError, assertOwner, tokenHash } from './domain.mjs';

export class NativeStores {
  constructor(repository, providers) { this.repository = repository; this.providers = providers; }

  async snapshot(owner) {
    const values = await this.repository.snapshot(owner);
    const denied = values.filter(value => this.providers.permits?.(value) === false);
    for (const value of denied) await this.repository.blockSandbox(value.provider, value.externalId);
    return denied.length ? this.repository.snapshot(owner) : values;
  }

  async sync(provider, externalId, lookup, owner, environment) {
    const read = provider === 'apple'
      ? () => this.providers.appleSubscription(lookup, environment) : () => this.providers.google(lookup);
    const result = await this.repository.sync(provider, externalId, lookup, read, owner);
    if (provider === 'google' && result.acknowledgePending) {
      // Acknowledge only after verified ownership and durable entitlement persistence.
      await this.providers.acknowledge(lookup, result.productId);
      await this.repository.acknowledged(provider, externalId);
    }
    return result;
  }

  async execute(request) {
    if (request.operation === 'ready') {
      await this.providers.ready(request.provider);
      return { ready: true };
    }
    if (request.operation === 'catalog') {
      return { apple: this.providers.appleConfig.products, google: this.providers.googleConfig.products };
    }
    if (request.operation === 'snapshot') {
      return { subscriptions: await this.snapshot(assertOwner(request.professionalId)) };
    }
    if (request.operation === 'verify') {
      const owner = assertOwner(request.professionalId);
      if (!request.proof || request.proof.length > 32768) throw new StoreError('Invalid purchase proof.');
      let id, lookup, environment;
      if (request.provider === 'apple') {
        const verified = await this.providers.appleTransaction(request.proof);
        if (verified.professionalId !== owner) throw new StoreError('Purchase belongs to another account.', 'PERMISSION_DENIED');
        id = lookup = verified.externalId;
        environment = verified.environment;
      } else if (request.provider === 'google') {
        lookup = request.proof;
        id = tokenHash(lookup);
      } else { throw new StoreError('Invalid provider.'); }
      await this.sync(request.provider, id, lookup, owner, environment);
      return { subscriptions: await this.snapshot(owner) };
    }
    if (request.operation === 'notification') {
      const verified = await this.providers.notification(request.provider, request.proof, request.authorization);
      if (verified) await this.sync(request.provider, verified.value.externalId, verified.lookup, undefined, verified.value.environment);
      return { received: true };
    }
    throw new StoreError('Unknown operation.');
  }

  async maintenance() {
    for (const record of await this.repository.due()) {
      try {
        if (this.providers.permits?.({ ...record, professionalId: record.owner }) === false) {
          await this.repository.blockSandbox(record.provider, record.externalId);
          continue;
        }
        await this.sync(record.provider, record.externalId, this.repository.credential(record), record.owner, record.environment);
      }
      catch { await this.repository.retryLater(record.provider, record.externalId); }
    }
  }
}
