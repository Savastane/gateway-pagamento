# Gateway de Pagamento

Plataforma de pagamentos em **Rust**: uma API REST única (`api-pagamento`) recebe cobranças e delega a provedores (Stripe, Appmax, Asaas, Mercado Pago) através de **serviços gRPC isolados**, um por provedor, todos implementando o mesmo contrato protobuf.

```
                         ┌─────────────┐   gRPC   ┌──────────────────┐        ┌────────────┐
POST /v1/payments ─────► │ api-        │ ───────► │ gateway-stripe   │ ─────► │ Stripe API │
GET  /v1/payments/{id}   │ pagamento   │ ───────► │ gateway-appmax   │ ─────► │ Appmax API │
POST .../refunds         │ (Axum :8080)│ ───────► │ gateway-asaas    │ ─────► │ Asaas API  │
      ▲                  └──────┬──────┘ ───────► │ gateway-mercado- │ ─────► │ MP API     │
      │                         │               │ pago             │        └────────────┘
      │ /internal/              │               └──────────────────┘
      │ gateway-events          │ sqlx
      │                         ▼
      │                  ┌─────────────┐   gRPC ParseWebhook
┌─────┴──────────────┐   │  postgres   │ ◄──────────────┐
│ webhook-pagamento  │   │ (migrations │                │
│ (Axum :8081)       │   │  no `banco`)│                │
└─────▲──────────────┘   └─────────────┘                │
      │ POST /webhooks/{provider} ◄── provedores ───────┘
```

## Estrutura

```
gateway-pagamento/
├── proto/payment_gateway/v1/gateway.proto   # contrato unico (RPCs, status, PIX, cartao)
├── crates/proto/                            # codigo gerado (tonic-build + protoc vendored)
├── banco/                                   # DONO do schema: migrations sqlx de todos os servicos
├── api-pagamento/                           # REST: usecases (request/response/model/repository/usecase)
│   └── http/                                # arquivos .http de teste por gateway (REST Client)
├── webhooks/                                # webhook-pagamento: recepcao, dedup, forward
├── stripe/   appmax/   asaas/   mercadopago/ # um servico gRPC por provedor
├── docker/                                  # compose + imagem unica dos 6 binarios
└── planejamento/plano-api-pagamento.md      # plano de arquitetura e status
```

## Serviços

| Serviço | Porta | Função |
|---|---|---|
| `api-pagamento` | 8080 (HTTP) | REST: criar/consultar/estornar pagamentos, `/internal/gateway-events`, `/swagger-ui` |
| `webhook-pagamento` | 8081 (HTTP) | `POST /webhooks/{provider}` → dedup + persist + forward |
| `gateway-stripe` | 50051 (gRPC) | PaymentIntents, PIX BR, HMAC webhook |
| `gateway-appmax` | 50052 (gRPC) | OAuth2, fluxo customers→orders→payments, sem cancel |
| `gateway-asaas` | 50053 (gRPC) | API key `$aact_`, valores decimais, PIX QR separado |
| `gateway-mercadopago` | 50054 (gRPC) | Bearer + `X-Idempotency-Key`, webhook HMAC manifesto |
| `postgres` | 5432 | 5 tabelas + `_sqlx_migrations` (dono: `banco`) |

## Padrões aplicados (e onde)

| Padrão | Onde | Como |
|---|---|---|
| **Contract-first (proto único)** | `proto/` → todos os gateways + api + webhooks | Um `gateway.proto` provider-neutral; extensões são só aditivas (`Customer.phone`, `PixDetails.qr_code_png_base64`...) |
| **Adapter** | `stripe/`, `appmax/`, `asaas/`, `mercadopago/` | Mesma interface gRPC; cada serviço traduz proto ⇄ API do provedor (moeda, auth, fluxo de N chamadas, assinatura de webhook) |
| **Usecase** (request/response/model/repository/usecase) | `api-pagamento/src/usecase/` | Uma pasta por usecase (`create_payment`, `get_payment`, `refund_payment`); controllers em `routes/` só traduzem HTTP ⇄ usecase |
| **Registry por env** | `api-pagamento`, `webhook-pagamento` | Qualquer `GATEWAY_<NOME>_ADDR` registra o provedor — gateway novo não exige código |
| **Trait + impl dupla (Pg/Mem)** | `api-pagamento` (`Store`), `webhook-pagamento` (`EventStore`) | `PgStore`/`MemStore` atrás de `Arc<dyn ...>` — testes e dev sem banco |
| **Owner único de migrations** | `banco/` | Todas as migrations em `banco/migrations/`; cada serviço aplica via `banco::connect` — uma só `_sqlx_migrations` |
| **Idempotência 2 fases** | `api-pagamento` | `idem_begin` (insert atômico + hash do corpo) → `idem_finish`; falha transitória libera a key (`idem_abort`) |
| **Reserve → call → settle** | `api-pagamento` (`reserve_refund`/`settle_refund`) | Lock `FOR UPDATE` valida `soma ≤ pago` antes de chamar o provedor — sem over-refund concorrente |
| **State machine explícita** | `api-pagamento` (`can_transition`) | Tabela de transições; eventos de webhook não regridem estados terminais |
| **RFC 7807** | `api-pagamento` (`ProblemDetails`) | `application/problem+json`; códigos gRPC mapeados para HTTP |
| **Secrets timing-safe** | todos | `subtle::ConstantTimeEq` em `X-Api-Key`, `INTERNAL_TOKEN` e assinaturas HMAC; segredos só via env (`secrecy::SecretString`) |
| **Webhook: persistir → responder → forward** | `webhook-pagamento` | 2xx só após `INSERT ... ON CONFLICT`; reprocessador retoma falhas |
| **Imagem única multi-binário** | `docker/` | Um `cargo build --release` gera os 6 binários; serviços do compose só mudam o `command` |
| **Smoke example por crate** | cada gateway | `examples/smoke.rs` com target `smoke-<provedor>` (nomes únicos evitam colisão) |

## Princípios do contrato

- **Dinheiro**: `amount_minor` em centavos (`int64`) — nunca float. Adapters convertem p/ decimal quando o provedor exige (Asaas, MP).
- **Cartão**: sempre token do SDK do provedor (`pm_...`, Appmax.js, creditCardToken, MP.js) — **nunca PAN/CVV** pelo backend.
- **Idempotência**: header `Idempotency-Key` obrigatório nos POSTs; replay devolve a resposta gravada; Stripe/MP recebem a chave nativamente, Appmax/Asaas deduplicam na API.
- **Estados**: PENDING → REQUIRES_ACTION/AUTHORIZED → PAID → PARTIALLY_REFUNDED → REFUNDED (+ FAILED/CANCELED/EXPIRED); terminais não regridem.
- **Webhooks**: 2xx só após persistir; dedup por `(gateway, provider_event_id)`; re-fetch do estado quando o envelope não traz status; retry com backoff no forward.

## Quickstart

```bash
# 1. credenciais
cd docker && cp .env.example .env   # preencher chaves sandbox

# 2. subir tudo (imagem unica, ~1min de build)
docker compose up -d --build

# 3. testar
curl localhost:8080/health
curl -X POST localhost:8080/v1/payments \
  -H 'Content-Type: application/json' -H 'Idempotency-Key: k-1' -H 'X-Api-Key: <API_KEY>' \
  -d '{"gateway":"stripe","amount_minor":12990,"currency":"brl",
       "method":{"type":"pix"},"customer":{"name":"Ana","email":"a@b.c","tax_id":"12345678909"},
       "return_url":"https://loja.test/retorno"}'
```

Swagger UI: `http://localhost:8080/swagger-ui` · Requests prontos: `api-pagamento/http/*.http`

## Variáveis de ambiente

| Var | Onde | Descrição |
|---|---|---|
| `DATABASE_URL` | api, webhook | Postgres; ausente → store em memória (só dev) |
| `GATEWAY_<NOME>_ADDR` | api, webhook | ex.: `GATEWAY_STRIPE_ADDR=http://gateway-stripe:50051` — registra o provedor sem código |
| `API_KEY` | api | `X-Api-Key` em `/v1/*` (opcional em dev) |
| `INTERNAL_TOKEN` | api, webhook | Bearer entre webhook → api |
| `DEFAULT_GATEWAY` | api | gateway quando `gateway` omitido (default `stripe`) |
| `STRIPE_SECRET_KEY` / `STRIPE_WEBHOOK_SECRET` | stripe | `sk_`/`rk_` + `whsec_` |
| `APPMAX_CLIENT_ID` / `APPMAX_CLIENT_SECRET` | appmax | OAuth2 |
| `ASAAS_ACCESS_TOKEN` / `ASAAS_WEBHOOK_TOKEN` | asaas | `$aact_...` + token estático de webhook |
| `MP_ACCESS_TOKEN` / `MP_WEBHOOK_SECRET` | mercadopago | `TEST-`/`APP_USR-` + segredo do painel |

## Verificação

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# Postgres real: DATABASE_URL=... cargo test -p api-pagamento --test pg -- --ignored
```

## Diferenças entre os provedores (resumo)

| | Stripe | Appmax | Asaas | Mercado Pago |
|---|---|---|---|---|
| Idempotência nativa | sim | não | não | sim |
| Dinheiro | centavos | centavos | decimal | decimal |
| Chamadas no create | 1 | 3 | 2 | 1 |
| Cancelamento | sim | **não** | sim | sim |
| Auth do webhook | HMAC corpo | **nenhuma** | token estático | HMAC manifesto |

Detalhes de cada adapter e o plano de fases: `planejamento/plano-api-pagamento.md`.
