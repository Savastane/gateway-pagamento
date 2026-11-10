# Configuração dos Gateways de Pagamento

Guia para configurar cada provedor (Stripe, Appmax, Asaas, Mercado Pago) na
plataforma. Toda a configuração é via **variáveis de ambiente** — nenhum
segredo vai para o código ou para o banco.

## Visão geral

```
provedor ──webhook──► webhook-pagamento (:8081) ──gRPC──► gateway-<provedor> (:5005x)
                          POST /webhooks/{provider}          └──► API do provedor
```

Cada gateway é um serviço gRPC independente. A `api-pagamento` descobre os
provedores pelas variáveis `GATEWAY_<NOME>_ADDR` — registrar um gateway novo
não exige mudança de código.

O caminho mais simples é o Docker Compose (raiz do repo), que já monta
`DATABASE_URL` e os `GATEWAY_*_ADDR` automaticamente a partir do `.env`.

## 1. Preparando o ambiente

```bash
cp .env.example .env   # na raiz do repo; preencher as chaves abaixo
```

Variáveis **compartilhadas** (não são de um provedor específico):

| Variável | Onde | Descrição |
|---|---|---|
| `API_KEY` | api-pagamento | `X-Api-Key` exigido em `POST /v1/*`. Vazio = só dev local |
| `INTERNAL_TOKEN` | api-pagamento + webhook-pagamento | Bearer entre webhook → `/internal/gateway-events`. **Deve ser o mesmo valor nos dois serviços** |
| `DEFAULT_GATEWAY` | api-pagamento | Gateway usado quando o request omite `gateway` (default `stripe`) |
| `DATABASE_URL` | api + webhook | Postgres. Ausente → store em memória (dev apenas) |
| `API_CALLBACK_URL` | webhook-pagamento | Default no compose: `http://api-pagamento:8080/internal/gateway-events` |

## 2. Stripe

### Credenciais

1. Crie uma conta em https://dashboard.stripe.com (modo **sandbox/test**).
2. Em **Developers → API keys**, copie a **Secret key** (`sk_test_...`;
   chaves restritas `rk_test_...` também funcionam — o serviço aceita
   `sk`/`rk`/`skcs`/`rkcs`).
3. Preencha:

```env
STRIPE_SECRET_KEY=sk_test_...
STRIPE_WEBHOOK_SECRET=whsec_...   # gerado no passo do webhook abaixo
```

### Webhook

1. No painel Stripe: **Developers → Webhooks → Add endpoint**.
2. URL: `https://<seu-dominio>/webhooks/stripe` (em dev local use um túnel,
   ex.: `ngrok http 8081`, e aponte para `https://<tunnel>/webhooks/stripe`).
3. Selecione os eventos de pagamento e, se usar **Billing/Assinaturas**
   (`/v1/billing/*`), também os eventos de `invoice.*` e `customer.subscription.*`.
4. Após criar, o painel mostra o **Signing secret** (`whsec_...`) →
   `STRIPE_WEBHOOK_SECRET`.

A assinatura é HMAC-SHA256 sobre o corpo cru; sem `STRIPE_WEBHOOK_SECRET`
configurado, webhooks com corpo são rejeitados.

### Variáveis opcionais

| Variável | Default | Descrição |
|---|---|---|
| `STRIPE_API_BASE` | `https://api.stripe.com` | Sobrescreva só para testes/mocks |
| `STRIPE_API_VERSION` | — | Fixa a versão da API Stripe (Billing usa `2024-04-10` fixo internamente) |
| `HTTP_TIMEOUT_MS` | `20000` | Timeout das chamadas HTTP ao Stripe |

### Billing (assinaturas)

O mesmo `STRIPE_SECRET_KEY` cobre os RPCs de assinatura
(`/v1/billing/checkout-sessions`, `/v1/billing/subscriptions`,
`/v1/billing/portal-sessions`). Preços são fixos no gateway
(4990/47900 centavos BRL, mês/ano) — não vêm do cliente. Billing **falha
fechado** sem `API_KEY` configurada, mesmo em dev.

## 3. Appmax

### Credenciais

1. Acesse o painel sandbox em https://sandboxappmax.com.br (ou o painel
   de produção correspondente).
2. Gere as credenciais OAuth2 do aplicativo:

```env
APPMAX_CLIENT_ID=...
APPMAX_CLIENT_SECRET=...
APPMAX_API_BASE=https://api.sandboxappmax.com.br
```

`APPMAX_AUTH_BASE` é derivado automaticamente de `APPMAX_API_BASE`
(`api.` → `auth.`); só defina se o provedor usar um host de auth diferente.

### Webhook

- Endpoint: `POST /webhooks/appmax` no `webhook-pagamento`.
- **A Appmax não assina webhooks** (sem HMAC/token). A validação de origem
  fica por conta da camada de rede — restrinja por IP/origem no proxy ou
  aceite o risco em dev. Configure o webhook no painel Appmax apontando
  para a URL pública.

### Limitações conhecidas

- **Sem cancelamento**: `refund`/cancel não existe no fluxo Appmax
  (customers → orders → payments, 3 chamadas no create).
- Valores em centavos; idempotência deduplicada na API (sem chave nativa).

## 4. Asaas

### Credenciais

1. Crie conta em https://sandbox.asaas.com (sandbox) ou https://www.asaas.com.
2. Em **Integrações → Chave de API**, gere uma chave `$aact_...`:

```env
ASAAS_ACCESS_TOKEN=$aact_YTU5...        # o $ é literal — no compose use $$aact_
ASAAS_API_BASE=https://api-sandbox.asaas.com/v3
ASAAS_WEBHOOK_TOKEN=token-aleatorio     # você escolhe este valor
```

> O serviço valida o prefixo `$aact_`. Em `.env`/shell, escape o `$`
> (`$$aact_` no compose, `\$aact_` no bash) para não ser expandido.

### Webhook

1. No painel Asaas: **Integrações → Webhooks**, cadastre a URL
   `https://<seu-dominio>/webhooks/asaas`.
2. O Asaas envia um header `asaas-access-token` em cada notificação.
   Configure lá o **mesmo valor** de `ASAAS_WEBHOOK_TOKEN` — é a única
   autenticação do webhook (token estático, sem HMAC).
3. Se `ASAAS_WEBHOOK_TOKEN` estiver vazio, o gateway aceita webhooks sem
   validar o token — **não deixe vazio em produção**.

Eventos tratados incluem `PAYMENT_CONFIRMED`/`PAYMENT_RECEIVED` → PAID,
`PAYMENT_REFUNDED`/`PAYMENT_PARTIALLY_REFUNDED`, `PAYMENT_OVERDUE` → EXPIRED.

## 5. Mercado Pago

### Credenciais

1. Crie uma aplicação em https://www.mercadopago.com.br/developers
   (painel de desenvolvedor → Suas integrações).
2. Em **Credenciais de teste** copie o **Access Token**:

```env
MP_ACCESS_TOKEN=TEST-...        # sandbox
# MP_ACCESS_TOKEN=APP_USR-...   # produção
MP_API_BASE=https://api.mercadopago.com
```

O serviço valida o prefixo `TEST-`/`APP_USR-`.

### Webhook

1. No painel do desenvolvedor: **Webhooks** → cadastre
   `https://<seu-dominio>/webhooks/mercadopago` com evento `payment`.
2. O Mercado Pago assina o webhook com HMAC sobre um **manifesto**
   (`id:data.id:request-id:x-signature`). Gere um segredo no painel e
   configure:

```env
MP_WEBHOOK_SECRET=segredo-do-painel
```

3. Sem `MP_WEBHOOK_SECRET`, a validação de assinatura é ignorada —
   configure sempre em produção.

## 6. Registrando um gateway na api/webhooks

O registry é dirigido por env: basta exportar `GATEWAY_<NOME>_ADDR`
apontando para o endereço gRPC do serviço. O `<NOME>` em maiúsculas deve
bater com o `gateway` usado nos requests (`stripe`, `appmax`, `asaas`,
`mercadopago`).

Exemplo fora do compose:

```bash
# api-pagamento
GATEWAY_STRIPE_ADDR=http://localhost:50051
GATEWAY_ASAAS_ADDR=http://localhost:50053
# webhook-pagamento (mesmas vars + API_CALLBACK_URL e INTERNAL_TOKEN)
```

O `webhook-pagamento` também usa o registry para parsear/validar a
assinatura de cada provedor — um provedor só recebe webhook se o
`GATEWAY_<NOME>_ADDR` estiver configurado **nele**.

## 7. Rodando

```bash
# na raiz do repo
docker compose up -d --build

# smoke tests por gateway (a partir da raiz do repo)
cargo run -p gateway-stripe --example smoke-stripe   # requer o serviço no ar
```

Requests REST prontos por provedor: `api-pagamento/http/*.http`.
Swagger UI: `http://localhost:8080/swagger-ui`.

## 8. Checklist por provedor

| | Stripe | Appmax | Asaas | Mercado Pago |
|---|---|---|---|---|
| Obrigatório | `STRIPE_SECRET_KEY` | `APPMAX_CLIENT_ID` + `_SECRET` | `ASAAS_ACCESS_TOKEN` (`$aact_`) | `MP_ACCESS_TOKEN` (`TEST-`/`APP_USR-`) |
| Webhook secret | `STRIPE_WEBHOOK_SECRET` (`whsec_`) | **nenhum** (Appmax não assina) | `ASAAS_WEBHOOK_TOKEN` (estático) | `MP_WEBHOOK_SECRET` (HMAC manifesto) |
| Endpoint | `/webhooks/stripe` | `/webhooks/appmax` | `/webhooks/asaas` | `/webhooks/mercadopago` |
| Cancelamento | sim | **não** | sim | sim |
| Validação na subida | prefixo `sk`/`rk` | nenhuma | prefixo `$aact_` | prefixo `TEST-`/`APP_USR-` |

## 9. Problemas comuns

- **`Missing("STRIPE_SECRET_KEY")` / `InvalidKey`**: a chave não começa com
  `sk`/`rk` — confira se copiou a *secret* key, não a publishable (`pk_`).
- **Webhook 401/403**: assinatura não confere — `whsec_` errado (Stripe),
  token divergente (Asaas), segredo errado (MP), ou body re-serializado
  por proxy (Stripe exige o corpo cru).
- **`provedor desconhecido: xxx`**: `GATEWAY_<NOME>_ADDR` não está
  configurado no `webhook-pagamento` — o nome no path deve ser minúsculo.
- **Pagamento fica PENDING**: webhook não chegou — confira a URL pública,
  TLS e os eventos assinados no painel do provedor.
- **Appmax sem cancelamento**: esperado — o fluxo não expõe cancel; use
  estorno parcial via `refund` quando suportado ou oriente o cliente.
