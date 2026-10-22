CREATE TABLE idempotency_keys (
    key TEXT NOT NULL,
    merchant_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    response JSONB,
    status_code INT,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (key, merchant_id)
);

CREATE INDEX ix_idempotency_keys_expiry ON idempotency_keys (expires_at);
