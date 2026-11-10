# Imagem unica com todos os binarios do workspace.
# Cada servico do compose roda um binario diferente via `command`.
FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release --bins

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build \
    /app/target/release/gateway-stripe \
    /app/target/release/gateway-appmax \
    /app/target/release/gateway-asaas \
    /app/target/release/gateway-mercadopago \
    /app/target/release/webhook-pagamento \
    /app/target/release/api-pagamento \
    /usr/local/bin/
