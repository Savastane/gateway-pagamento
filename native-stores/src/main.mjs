import { fileURLToPath } from 'node:url';
import grpc from '@grpc/grpc-js';
import loader from '@grpc/proto-loader';
import pg from 'pg';
import { Repository } from './repository.mjs';
import { Providers } from './providers.mjs';
import { NativeStores } from './service.mjs';
import { StoreError } from './domain.mjs';
import { maintenanceTick } from './worker.mjs';

const env = process.env;
for (const name of ['DATABASE_URL', 'STORE_TOKEN_ENCRYPTION_KEY', 'STORE_CALLBACK_URL', 'STORE_CALLBACK_TOKEN']) {
  if (!env[name]) throw new Error(`${name} is required.`);
}
if (env.STORE_CALLBACK_TOKEN.length < 32) throw new Error('STORE_CALLBACK_TOKEN must have at least 32 characters.');
const pool = new pg.Pool({ connectionString: env.DATABASE_URL, max: 5, statement_timeout: 20000 });
const repository = new Repository(pool, env.STORE_TOKEN_ENCRYPTION_KEY);
const service = new NativeStores(repository, new Providers());
const definition = loader.loadSync(fileURLToPath(new URL('../../proto/payment_gateway/v1/gateway.proto', import.meta.url)), {
  longs: String, enums: String, defaults: true,
});
const proto = grpc.loadPackageDefinition(definition).paymentgateway.v1;
const server = new grpc.Server({ 'grpc.max_receive_message_length': 262144 });
server.addService(proto.NativeStoreBilling.service, {
  execute: (call, callback) => {
    service.execute(call.request).then(result => callback(null, { json: JSON.stringify(result) })).catch(error => {
      // Provider failures may contain purchase tokens, JWTs or service-account data.
      const code = error instanceof StoreError ? grpc.status[error.code] : grpc.status.UNAVAILABLE;
      callback({ code: code ?? grpc.status.UNAVAILABLE,
        message: error instanceof StoreError ? error.message : 'Store verification temporarily unavailable.' });
    });
  },
});
server.bindAsync(env.GRPC_ADDR ?? '0.0.0.0:50055', grpc.ServerCredentials.createInsecure(), error => {
  if (error) throw new Error('Unable to bind native store service.');
  console.log('Native store verifier ready.');
});

let running = false;
async function tick() {
  if (running) return;
  running = true;
  try {
    await maintenanceTick(service, repository, async value => {
      const response = await fetch(env.STORE_CALLBACK_URL, {
          method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${env.STORE_CALLBACK_TOKEN}` },
          body: JSON.stringify({ profissionalId: value.professionalId }), signal: AbortSignal.timeout(10000),
      });
      return response.ok;
    }, () => console.warn('Native store maintenance will retry. No purchase data logged.'));
  } finally { running = false; }
}
const timer = setInterval(tick, 60000);
timer.unref();
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    clearInterval(timer);
    server.tryShutdown(() => { void pool.end(); });
  });
}
