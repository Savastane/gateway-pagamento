# Assinaturas App Store e Google Play

## Arquitetura

- Apps: StoreKit 2 / Play Billing 8.3; preco e moeda exibidos pelo SDK.
- API profissional: autentica o profissional, reserva a tentativa entre lojas e
  espelha direitos de acesso validados. Nao recebe numeros de cartao nem CVV.
- `api-pagamento`: fachada HTTP protegida por `API_KEY`, com RPC interno para
  `native-stores/` (Node 22, bibliotecas oficiais Apple e Google).
- `banco/migrations/0006_native_store_subscriptions.sql`: vinculo duravel de
  original transaction ID Apple / SHA-256 do token Google a uma unica conta.
  Credencial para reconciliacao criptografada; nao registrar recibos em logs.
- Renovacao/cancelamento/reembolso: notificacoes autenticadas, releitura da loja
  sob lock e callback com marcador duravel de entrega. Reconciliacao a cada cinco
  minutos recupera notificacoes perdidas e acknowledgement Google pendente.
  Estado identico nao incrementa a versao nem gera outro callback. Falha de uma
  credencial nao impede reconciliar as demais; entregas pendentes sao processadas
  independentemente da reconciliacao.
- Acesso exige periodo vigente e estado autorizado. Cancelar a renovacao
  preserva o periodo pago. Hold, pausa, expiracao e revogacao nao concedem acesso.
  Grace period so vale enquanto confirmado pela loja.

Nao exponha a porta gRPC 50055 publicamente. Restrinja a rede Docker e mantenha
`/v1/billing/stores/*` acessivel somente aos backends autorizados.

## Produtos

Crie os produtos nas contas de **city.bussola.profissional**; os nomes abaixo sao
IDs de configuracao, nao produtos criados automaticamente por este codigo.

| Loja | Plano | ID padrao | Periodicidade |
|---|---|---|---|
| Apple | MENSAL | city.bussola.profissional.mensal | 1 mes |
| Apple | ANUAL | city.bussola.profissional.anual | 1 ano |
| Google | MENSAL | bussola_pro_mensal | base plan `mensal`, auto-renovavel, P1M |
| Google | ANUAL | bussola_pro_anual | base plan `anual`, auto-renovavel, P1Y |

Apple: coloque mensal e anual no **mesmo subscription group** e configure os
niveis do grupo para impedir assinaturas paralelas. Nao habilite Family Sharing:
os direitos pertencem a uma conta profissional individual.

Google: mantenha somente um base plan auto-renovavel correspondente por produto.
O app nao oferece troca de plano durante uma assinatura ativa; use o gerenciamento
da loja. Upgrades entre produtos Google com replacement token nao estao habilitados.
Defina precos, paises e disponibilidade nos consoles; o app nao fixa valores BRL.

## Segredos e identidades

Copie as variaveis de `.env.example` para um arquivo ignorado. Nunca envie valores
reais pelo chat, Git, imagens Docker ou parametros publicos do aplicativo.

- `API_KEY`: gateway; igual a `PAYMENT_GATEWAY_API_KEY` na API profissional.
- `STORE_TOKEN_ENCRYPTION_KEY`: 32 bytes aleatorios em base64 (AES-256-GCM).
  Guarde backup seguro; perder/trocar a chave impede reconciliar os tokens existentes.
  Rotacao exige recriptografia controlada, nao apenas mudar a variavel.
- `STORE_CALLBACK_TOKEN`: segredo aleatorio de 32+ caracteres, igual na API profissional.
- `STORE_CALLBACK_URL`: URL HTTPS da API profissional, terminando em
  `/api/assinatura/lojas/notificacao`. Callback indisponivel e repetido a cada minuto.
- Google: habilite Android Publisher API e conceda a conta de servico acesso ao app
  no Play Console, incluindo consultar assinaturas e gerenciar acknowledgement.
  Monte `secrets/google-play.json` ou use ADC/Workload Identity ao executar fora do Compose.
- Apple: chave **In-App Purchase/App Store Server API**, `APPLE_KEY_ID`,
  `APPLE_ISSUER_ID`, `APPLE_APP_ID` numerico e `APPLE_BUNDLE_ID`.
  Monte `secrets/apple-subscriptions.p8` e certificados raiz DER obtidos da pagina
  oficial Apple PKI, como `secrets/AppleRootCA-G3.cer`.
  `APPLE_ROOT_CERTIFICATES` aceita varios caminhos separados por virgula.
  A biblioteca oficial verifica cadeia, app, ambiente, assinatura e revogacao online.

Os arquivos em `secrets/` sao montados somente para leitura. Garanta permissao
de leitura para o usuario `node` do container, sem tornar as chaves publicas.

## Notificacoes

Exponha via HTTPS no gateway, sem adicionar autenticacao de API na frente das lojas:

- App Store Server Notifications **V2**: `POST /webhooks/stores/apple`.
  Configure as URLs de producao e sandbox nos ambientes correspondentes.
- Google Pub/Sub authenticated push: `POST /webhooks/stores/google`.
  `GOOGLE_PUBSUB_AUDIENCE` deve ser a audience exata do token OIDC do push.
  `GOOGLE_PUBSUB_SERVICE_ACCOUNT` deve ser o email da conta usada no push,
  nao necessariamente a conta Android Publisher.
  Configure RTDN no Play Console e conceda permissao de publicar no topico a
  `google-play-developer-notifications@system.gserviceaccount.com`.

Nem o tipo recebido no evento nem o status informado pelo app liberam acesso:
o gateway consulta novamente a API oficial. Eventos repetidos/fora de ordem
nao sobrescrevem um estado mais recente. Erros de validacao nao retornam sucesso.
Google acknowledgement ocorre somente depois de persistir titularidade e estado.

## Execucao

Na raiz `gateway-pagamento`, depois de configurar `docker/.env` e montar os arquivos:

```bash
docker compose --env-file docker/.env -f compose.yml -f compose.native-stores.yml up -d --build
```

O gateway aplica sua migration ao iniciar. A API profissional aplica a migration
compartilhada `1800000000003` (tambem registrada nas APIs consumidor e admin).
Para Node fora do Compose, configure `DATABASE_URL`, `GRPC_ADDR`,
`GOOGLE_APPLICATION_CREDENTIALS` e `APPLE_PRIVATE_KEY_FILE` explicitamente.

## Sandbox e publicacao

Use banco e backend de teste separados, `APPLE_STORE_ENVIRONMENT=Sandbox` e
`STORE_ALLOW_SANDBOX_PURCHASES=true` para testes Google. Producao recusa Google
test purchases por padrao. Por padrao a Apple tambem aceita somente o ambiente
configurado; aponte builds TestFlight/sandbox para o backend correspondente.

### App Review no backend de producao

Para homologar o mesmo build/backend que sera publicado, sem liberar recibos de
teste para todas as contas:

1. Prepare uma conta profissional exclusiva de revisao, aprovada e sem assinatura
   antiga aberta. Obtenha seu UUID pelo backoffice/conta autenticada.
2. Mantenha `APPLE_STORE_ENVIRONMENT=Production`. Defina
   `APPLE_ALLOW_SANDBOX_FALLBACK=true` e `APPLE_SANDBOX_ACCOUNT_IDS` com os UUIDs
   autorizados, separados por virgula. Sem uma lista valida o servico recusa iniciar.
3. Configure tambem a URL de notificacoes Sandbox no App Store Connect. O gateway
   revalida assinatura, cadeia, app e ambiente com um verificador Sandbox separado,
   e consulta a API Apple do ambiente correto. Nao ha fallback para assinatura ou
   certificado invalido; notificacoes Sandbox sem appAppleId de producao sao
   revalidadas integralmente. O appAccountToken assinado deve pertencer a conta listada.
4. Forneca a conta de revisao nas instrucoes da submissao. As compras das demais
   contas continuam exigindo producao. Nao e necessario habilitar testes Google.
5. Remova os UUIDs e desabilite o fallback depois da homologacao se nao desejar
   manter acesso de teste. Snapshots e worker bloqueiam direitos Sandbox ja
   persistidos quando a autorizacao e removida; apenas parar compras futuras
   nao seria suficiente.

Distribua a mesma politica a todas as replicas. O ambiente de um vinculo existente
e imutavel: uma compra Sandbox nao pode substituir o mesmo identificador em producao.

No Android, instale pela faixa de testes e use um license tester. No iOS, use
StoreKit sandbox/TestFlight em dispositivo com o backend sandbox. Recibos locais
de um arquivo `.storekit` do Xcode nao sao aceitos pelo backend como compra real.
Publique politica de privacidade e termos e configure seus links nos apps antes
de distribuir. IAP exige contratos, dados fiscais/bancarios e produtos aprovados.

## Contratos HTTP internos

- `GET /v1/billing/stores/catalog`: IDs permitidos, nunca credenciais.
- `GET /v1/billing/stores/ready/apple|google`: preflight de configuracao; Google
  exige obter credencial OAuth antes de permitir uma nova compra. Nao substitui
  homologar as permissoes e produtos no sandbox.
- `POST /v1/billing/stores/verify`: `{provider, professional_id, proof}`.
  `professional_id` deve vir do JWT validado pela API profissional, nunca do app.
  Apple proof e o JWS StoreKit; Google proof e o purchase token.
- `GET /v1/billing/stores/{uuid}`: snapshot canonico persistido, sem credenciais.

## Verificacao

```bash
cd native-stores && npm ci --ignore-scripts && npm test
# na raiz
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Os testes usam fixtures e mocks, sem compras, cancelamentos ou APIs reais.
`TEST_DATABASE_URL` habilita o teste Postgres do servico Node; use somente um
banco de testes dedicado. O teste cria e remove apenas seu schema temporario.
A validacao completa exige sandbox das lojas e Postgres dedicado. O adapter
Stripe de pagamentos avulsos foi preservado; os endpoints/RPC de Stripe Billing
e o checkout de assinaturas foram removidos. Isso nao cancela renovacoes remotas.
