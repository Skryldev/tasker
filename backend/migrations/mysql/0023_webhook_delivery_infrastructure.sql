-- Phase 8B (Task 2): durable webhook delivery infrastructure.
-- See migrations/sqlite/0023_webhook_delivery_infrastructure.sql for the
-- full rationale; dialect notes follow 0016 conventions (CHAR(36) ids,
-- VARCHAR strings, BIGINT epochs, InnoDB + utf8mb4).

CREATE TABLE webhook_delivery_replays (
    id          CHAR(36) NOT NULL,
    delivery_id CHAR(36) NOT NULL,
    replayed_by CHAR(36) NOT NULL,
    prior_attempts       BIGINT NOT NULL,
    prior_error_category VARCHAR(40) NULL,
    prior_error_detail   VARCHAR(500) NULL,
    reason      VARCHAR(500) NULL,
    created_at  BIGINT NOT NULL,
    PRIMARY KEY (id),
    CONSTRAINT fk_webhook_delivery_replays_delivery FOREIGN KEY (delivery_id)
        REFERENCES webhook_deliveries (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_webhook_delivery_replays_user FOREIGN KEY (replayed_by)
        REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_webhook_delivery_replays_delivery ON webhook_delivery_replays (delivery_id);

CREATE INDEX idx_webhook_deliveries_history ON webhook_deliveries (integration_id, created_at);
