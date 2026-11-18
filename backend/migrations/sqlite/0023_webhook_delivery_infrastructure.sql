-- Phase 8B (Task 2): durable webhook delivery infrastructure.
--
-- The delivery lifecycle itself was introduced by 0016 (webhook_deliveries
-- with claim/lease/fencing fields, the state CHECK, and the claim/lease
-- indexes). Task 2 adds only what the engine needs beyond that schema:
--
-- 1. `webhook_delivery_replays` — an append-only audit record for manual
--    replays (who, when, why). Replay transitions the delivery back to
--    `pending` via a guarded update; the original delivery history and every
--    failure record are preserved, and each replay appends one row here.
--    See src/integrations/webhook/delivery.rs (`replay_dead_letter`).
-- 2. `idx_webhook_deliveries_history` — delivery-history inspection by
--    integration in chronological order (future management API pagination);
--    the claim/lease queries keep using the 0016 indexes.

CREATE TABLE webhook_delivery_replays (
    id          TEXT PRIMARY KEY,
    delivery_id TEXT NOT NULL REFERENCES webhook_deliveries (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    replayed_by TEXT NOT NULL REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    -- Diagnostics snapshot: the replay clears the delivery's mutable
    -- last_error_*/attempt fields, so the state being reopened is preserved
    -- here as part of the auditable transition.
    prior_attempts       BIGINT NOT NULL,
    prior_error_category TEXT,
    prior_error_detail   TEXT,
    reason      TEXT,
    created_at  BIGINT NOT NULL
);

CREATE INDEX idx_webhook_delivery_replays_delivery ON webhook_delivery_replays (delivery_id);

CREATE INDEX idx_webhook_deliveries_history ON webhook_deliveries (integration_id, created_at);
