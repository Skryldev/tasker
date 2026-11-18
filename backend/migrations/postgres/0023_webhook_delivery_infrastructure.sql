-- Phase 8B (Task 2): durable webhook delivery infrastructure.
-- See migrations/sqlite/0023_webhook_delivery_infrastructure.sql for the
-- full rationale; dialect notes follow 0016/0017 conventions.

CREATE TABLE webhook_delivery_replays (
    id          TEXT PRIMARY KEY,
    delivery_id TEXT NOT NULL REFERENCES webhook_deliveries (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    replayed_by TEXT NOT NULL REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    prior_attempts       BIGINT NOT NULL,
    prior_error_category TEXT,
    prior_error_detail   TEXT,
    reason      TEXT,
    created_at  BIGINT NOT NULL
);

CREATE INDEX idx_webhook_delivery_replays_delivery ON webhook_delivery_replays (delivery_id);

CREATE INDEX idx_webhook_deliveries_history ON webhook_deliveries (integration_id, created_at);
