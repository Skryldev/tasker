-- Rules Engine (Phase 8A): rules, immutable rule versions, domain events,
-- execution records, the transactional outbox, and the internal-action
-- idempotency ledger. See docs/rules-engine.md for the full schema story.
--
-- Foreign-key policy:
--   * task_id columns on domain_events / rule_executions /
--     automation_outbox carry NO foreign key to tasks. Task deletion is a
--     hard delete; automation history is immutable and must survive it.
--   * Every other reference is ON DELETE RESTRICT ON UPDATE RESTRICT.
--
-- Referential cycle: automation-origin events reference the execution that
-- caused them while executions reference their event. MySQL cannot reference
-- a table that does not exist yet, so the cycle FK is added via ALTER TABLE
-- after rule_executions is created.
--
-- Dialect notes:
--   * String columns are VARCHAR with utf8mb4 (project rule, see
--     migrations/mysql/0001_init.sql header): sqlx Any reports MySQL TEXT as
--     a BLOB-family wire type that cannot decode into String. Sizes are the
--     character limits from src/rules/limits.rs; utf8mb4 reserves 4 bytes per
--     character, assessed per table in docs/rules-engine.md.
--   * Small integer counters use BIGINT so sqlx Any decodes them uniformly
--     as i64.

CREATE TABLE rules (
    id              CHAR(36) NOT NULL,
    team_id         CHAR(36) NOT NULL,
    name            VARCHAR(120) NOT NULL,
    description     VARCHAR(500) NULL,
    trigger_type    VARCHAR(32) NOT NULL,
    enabled         BOOLEAN NOT NULL DEFAULT FALSE,
    current_version BIGINT NOT NULL DEFAULT 1,
    condition_json  VARCHAR(4000) NOT NULL,
    actions_json    VARCHAR(2000) NOT NULL,
    created_by      CHAR(36) NOT NULL,
    created_at      BIGINT NOT NULL,
    updated_at      BIGINT NOT NULL,
    archived_at     BIGINT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_rules_team_name (team_id, name),
    CONSTRAINT fk_rules_team FOREIGN KEY (team_id) REFERENCES teams (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_rules_creator FOREIGN KEY (created_by) REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_rules_trigger_type CHECK (trigger_type IN
        ('task_status_changed', 'task_priority_changed', 'task_assigned')),
    CONSTRAINT chk_rules_current_version CHECK (current_version >= 1)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Rule candidate selection: WHERE trigger_type = ? AND enabled = TRUE.
CREATE INDEX idx_rules_trigger_enabled ON rules (trigger_type, enabled);

CREATE TABLE rule_versions (
    rule_id        CHAR(36) NOT NULL,
    version        BIGINT NOT NULL,
    trigger_type   VARCHAR(32) NOT NULL,
    condition_json VARCHAR(4000) NOT NULL,
    actions_json   VARCHAR(2000) NOT NULL,
    created_by     CHAR(36) NOT NULL,
    created_at     BIGINT NOT NULL,
    PRIMARY KEY (rule_id, version),
    CONSTRAINT fk_rule_versions_rule FOREIGN KEY (rule_id) REFERENCES rules (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_rule_versions_version CHECK (version >= 1),
    CONSTRAINT chk_rule_versions_trigger_type CHECK (trigger_type IN
        ('task_status_changed', 'task_priority_changed', 'task_assigned'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE TABLE domain_events (
    id                   CHAR(36) NOT NULL,
    event_type           VARCHAR(32) NOT NULL,
    schema_version       BIGINT NOT NULL DEFAULT 1,
    team_id              CHAR(36) NOT NULL,
    -- Historical identifier: deliberately NOT a foreign key to tasks.
    task_id              CHAR(36) NOT NULL,
    actor_id             CHAR(36) NULL,
    previous_state       VARCHAR(500) NULL,
    new_state            VARCHAR(500) NULL,
    origin               VARCHAR(16) NOT NULL,
    causing_execution_id CHAR(36) NULL,
    occurred_at          BIGINT NOT NULL,
    PRIMARY KEY (id),
    CONSTRAINT fk_domain_events_team FOREIGN KEY (team_id) REFERENCES teams (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_domain_events_actor FOREIGN KEY (actor_id) REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_domain_events_event_type CHECK (event_type IN
        ('task_status_changed', 'task_priority_changed', 'task_assigned')),
    CONSTRAINT chk_domain_events_origin CHECK (origin IN ('user', 'automation'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_domain_events_task ON domain_events (task_id, occurred_at);
CREATE INDEX idx_domain_events_team ON domain_events (team_id, occurred_at);
CREATE INDEX idx_domain_events_causing ON domain_events (causing_execution_id);

CREATE TABLE rule_executions (
    id               CHAR(36) NOT NULL,
    rule_id          CHAR(36) NOT NULL,
    rule_version     BIGINT NOT NULL,
    event_id         CHAR(36) NOT NULL,
    team_id          CHAR(36) NOT NULL,
    task_id          CHAR(36) NOT NULL,
    trigger_type     VARCHAR(32) NOT NULL,
    condition_result VARCHAR(16) NOT NULL,
    status           VARCHAR(16) NOT NULL DEFAULT 'planned',
    planned_actions  VARCHAR(1000) NULL,
    error_category   VARCHAR(40) NULL,
    error_detail     VARCHAR(500) NULL,
    created_at       BIGINT NOT NULL,
    completed_at     BIGINT NULL,
    PRIMARY KEY (id),
    -- One execution per (rule, event): duplicate processing is impossible.
    UNIQUE KEY uq_rule_executions_rule_event (rule_id, event_id),
    CONSTRAINT fk_rule_executions_rule FOREIGN KEY (rule_id) REFERENCES rules (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_rule_executions_event FOREIGN KEY (event_id) REFERENCES domain_events (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_rule_executions_team FOREIGN KEY (team_id) REFERENCES teams (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    -- The exact evaluated definition, checked by the database.
    CONSTRAINT fk_rule_executions_version FOREIGN KEY (rule_id, rule_version)
        REFERENCES rule_versions (rule_id, version) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_rule_executions_trigger_type CHECK (trigger_type IN
        ('task_status_changed', 'task_priority_changed', 'task_assigned')),
    CONSTRAINT chk_rule_executions_condition_result CHECK (condition_result IN
        ('matched', 'not_matched')),
    CONSTRAINT chk_rule_executions_status CHECK (status IN
        ('planned', 'completed', 'failed'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_rule_executions_event ON rule_executions (event_id);
CREATE INDEX idx_rule_executions_team_created ON rule_executions (team_id, created_at);
CREATE INDEX idx_rule_executions_task_created ON rule_executions (task_id, created_at);

-- Close the domain_events <-> rule_executions cycle (see header); the index
-- created above serves this FK.
ALTER TABLE domain_events
    ADD CONSTRAINT fk_domain_events_causing_execution
    FOREIGN KEY (causing_execution_id) REFERENCES rule_executions (id)
    ON DELETE RESTRICT ON UPDATE RESTRICT;

CREATE TABLE automation_outbox (
    id                  CHAR(36) NOT NULL,
    execution_id        CHAR(36) NOT NULL,
    event_id            CHAR(36) NOT NULL,
    rule_id             CHAR(36) NOT NULL,
    rule_version        BIGINT NOT NULL,
    task_id             CHAR(36) NOT NULL,
    action_type         VARCHAR(32) NOT NULL,
    schema_version      BIGINT NOT NULL DEFAULT 1,
    payload             VARCHAR(4000) NOT NULL,
    idempotency_key     CHAR(64) NOT NULL,
    status              VARCHAR(16) NOT NULL DEFAULT 'pending',
    attempts            BIGINT NOT NULL DEFAULT 0,
    next_attempt_at     BIGINT NOT NULL,
    claim_token         CHAR(36) NULL,
    worker_id           VARCHAR(64) NULL,
    claimed_at          BIGINT NULL,
    lease_expires_at    BIGINT NULL,
    last_error_category VARCHAR(40) NULL,
    last_error_detail   VARCHAR(500) NULL,
    created_at          BIGINT NOT NULL,
    updated_at          BIGINT NOT NULL,
    completed_at        BIGINT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_automation_outbox_idempotency (idempotency_key),
    CONSTRAINT fk_automation_outbox_execution FOREIGN KEY (execution_id) REFERENCES rule_executions (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_automation_outbox_event FOREIGN KEY (event_id) REFERENCES domain_events (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_automation_outbox_rule FOREIGN KEY (rule_id) REFERENCES rules (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_automation_outbox_version FOREIGN KEY (rule_id, rule_version)
        REFERENCES rule_versions (rule_id, version) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_automation_outbox_action_type CHECK (action_type IN
        ('send_notification', 'add_comment', 'send_message')),
    CONSTRAINT chk_automation_outbox_status CHECK (status IN
        ('pending', 'processing', 'delivered', 'retryable_failed', 'dead_letter')),
    CONSTRAINT chk_automation_outbox_attempts CHECK (attempts >= 0),
    -- Fencing invariant: only a row in `processing` carries a claim token.
    CONSTRAINT chk_automation_outbox_claim_token
        CHECK ((claim_token IS NULL) = (status <> 'processing'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Claim query: WHERE status = 'pending' AND next_attempt_at <= now.
CREATE INDEX idx_automation_outbox_claim ON automation_outbox (status, next_attempt_at);
-- Stale-claim reaper: WHERE status = 'processing' AND lease_expires_at < now.
CREATE INDEX idx_automation_outbox_lease ON automation_outbox (status, lease_expires_at);

CREATE TABLE rule_action_results (
    id           CHAR(36) NOT NULL,
    outbox_id    CHAR(36) NOT NULL,
    execution_id CHAR(36) NOT NULL,
    action_type  VARCHAR(32) NOT NULL,
    outcome      VARCHAR(16) NOT NULL,
    recipient_id CHAR(36) NULL,
    author_id    CHAR(36) NULL,
    body_hash    CHAR(64) NULL,
    detail       VARCHAR(500) NULL,
    created_at   BIGINT NOT NULL,
    PRIMARY KEY (id),
    -- Idempotency ledger: one result per outbox item, enforced here.
    UNIQUE KEY uq_rule_action_results_outbox (outbox_id),
    CONSTRAINT fk_rule_action_results_outbox FOREIGN KEY (outbox_id) REFERENCES automation_outbox (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_rule_action_results_execution FOREIGN KEY (execution_id) REFERENCES rule_executions (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_rule_action_results_recipient FOREIGN KEY (recipient_id) REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_rule_action_results_author FOREIGN KEY (author_id) REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_rule_action_results_action_type CHECK (action_type IN
        ('send_notification', 'add_comment', 'send_message')),
    CONSTRAINT chk_rule_action_results_outcome CHECK (outcome IN
        ('completed', 'skipped'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_rule_action_results_execution ON rule_action_results (execution_id);
