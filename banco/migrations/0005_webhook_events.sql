CREATE TABLE webhook_events (
    id BIGSERIAL PRIMARY KEY,
    gateway TEXT NOT NULL,
    provider_event_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    external_id TEXT NOT NULL,
    normalized_status INT NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'received',
    payload JSONB NOT NULL,
    attempts INT NOT NULL DEFAULT 0,
    last_error TEXT,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ,
    UNIQUE (gateway, provider_event_id)
);

CREATE INDEX ix_webhook_events_pending
    ON webhook_events (received_at)
    WHERE state = 'received';
