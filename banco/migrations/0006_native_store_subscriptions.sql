CREATE TABLE native_store_subscriptions (
    provider text NOT NULL CHECK (provider IN ('apple', 'google')),
    external_id text NOT NULL,
    professional_id uuid NOT NULL,
    credential_enc text NOT NULL,
    product_id text NOT NULL,
    plan text NOT NULL CHECK (plan IN ('MENSAL','ANUAL')),
    environment text NOT NULL,
    store_status text NOT NULL,
    starts_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    active boolean NOT NULL,
    cancel_at_period_end boolean NOT NULL DEFAULT false,
    acknowledge_pending boolean NOT NULL DEFAULT false,
    version bigint NOT NULL DEFAULT 1,
    delivered_version bigint NOT NULL DEFAULT 0,
    next_check_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, external_id)
);
CREATE INDEX native_store_subscriptions_recheck ON native_store_subscriptions(next_check_at);
CREATE INDEX native_store_subscriptions_owner ON native_store_subscriptions(professional_id);
