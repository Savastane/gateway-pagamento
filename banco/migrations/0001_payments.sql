CREATE TABLE payments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id TEXT NOT NULL DEFAULT 'default',
    gateway TEXT NOT NULL,
    method TEXT NOT NULL,
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    currency TEXT NOT NULL,
    status TEXT NOT NULL,
    external_id TEXT,
    idempotency_key TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',
    version INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX ux_payments_gateway_external_id
    ON payments (gateway, external_id)
    WHERE external_id IS NOT NULL;

CREATE INDEX ix_payments_merchant ON payments (merchant_id, created_at);
