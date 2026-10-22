CREATE TABLE payment_events (
    id BIGSERIAL PRIMARY KEY,
    payment_id UUID NOT NULL REFERENCES payments (id),
    from_status TEXT,
    to_status TEXT NOT NULL,
    source TEXT NOT NULL, -- 'api' | 'webhook'
    payload JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ix_payment_events_payment ON payment_events (payment_id, id);
