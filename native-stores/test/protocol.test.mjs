import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import grpc from '@grpc/grpc-js';
import loader from '@grpc/proto-loader';
import { NativeStores } from '../src/service.mjs';

test('gRPC contract preserves verified owner, proof and sanitized response across the wire', async () => {
  const definition = loader.loadSync(fileURLToPath(new URL('../../proto/payment_gateway/v1/gateway.proto', import.meta.url)), {
    longs: String, enums: String, defaults: true,
  });
  const proto = grpc.loadPackageDefinition(definition).paymentgateway.v1;
  const owner = '00000000-0000-4000-8000-000000000001';
  const service = new NativeStores({
    async sync(provider, externalId, lookup, read, expectedOwner) {
      assert.equal(provider, 'google'); assert.equal(expectedOwner, owner);
      assert.equal(lookup, 'dummy-test-proof'); await read();
      return { acknowledgePending: false };
    },
    async snapshot(account) { assert.equal(account, owner); return [{ professionalId: owner, active: true }]; },
  }, { async google() { return {}; } });
  const server = new grpc.Server();
  server.addService(proto.NativeStoreBilling.service, {
    execute: (call, callback) => service.execute(call.request)
      .then(result => callback(null, { json: JSON.stringify(result) }))
      .catch(error => callback({ code: grpc.status.INVALID_ARGUMENT, message: error.message })),
  });
  const port = await new Promise((resolve, reject) => server.bindAsync('127.0.0.1:0', grpc.ServerCredentials.createInsecure(),
    (error, value) => error ? reject(error) : resolve(value)));
  const client = new proto.NativeStoreBilling(`127.0.0.1:${port}`, grpc.credentials.createInsecure());
  try {
    const response = await new Promise((resolve, reject) => client.execute({
      operation: 'verify', provider: 'google', professionalId: owner, proof: 'dummy-test-proof',
    }, (error, result) => error ? reject(error) : resolve(result)));
    assert.deepEqual(JSON.parse(response.json), { subscriptions: [{ professionalId: owner, active: true }] });
    assert.equal(response.json.includes('dummy-test-proof'), false);
  } finally { client.close(); await new Promise(resolve => server.tryShutdown(resolve)); }
});
