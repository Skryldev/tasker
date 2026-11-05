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
-- caused them while executions reference their event. PostgreSQL cannot
-- reference a table that does not exist yet, so the cycle FK is added via
-- ALTER TABLE after rule_executions is created.
--
-- Small integer counters use BIGINT so sqlx Any decodes them uniformly as
-- i64. JSON/text payloads are TEXT: the application contract bounds every
-- serialized field (see src/rules/limits.rs); PostgreSQL TEXT decodes as
-- String through the Any driver without issue.

CREATE TABLE rules (
    id              TEXT PRIMARY KEY,
    team_id         TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    name            TEXT NOT NULL,
    description     TEXT,
    trigger_type    TEXT NOT NULL
        CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned')),
    enabled         BOOLEAN NOT NULL DEFAULT FALSE,
    current_version BIGINT NOT NULL DEFAULT 1 CHECK (current_version >= 1),
    condition_json  TEXT NOT NULL,
    actions_json    TEXT NOT NULL,
    created_by      TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at      BIGINT NOT NULL,
    updated_at      BIGINT NOT NULL,
    archived_at     BIGINT,
    UNIQUE (team_id, name)
);

-- Rule candidate selection: WHERE trigger_type = ? AND enabled = TRUE.
CREATE INDEX idx_rules_trigger_enabled ON rules (trigger_type, enabled);

CREATE TABLE rule_versions (
    rule_id        TEXT NOT NULL REFERENCES rules (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    version        BIGINT NOT NULL CHECK (version >= 1),
    trigger_type   TEXT NOT NULL
        CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned')),
    condition_json TEXT NOT NULL,
    actions_json   TEXT NOT NULL,
    created_by     TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at     BIGINT NOT NULL,
    PRIMARY KEY (rule_id, version)
);

CREATE TABLE domain_events (
    id                   TEXT PRIMARY KEY,
    event_type           TEXT NOT NULL
        CHECK (event_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned')),
    schema_version       BIGINT NOT NULL DEFAULT 1,
    team_id              TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    -- Historical identifier: deliberately NOT a foreign key to tasks.
    task_id              TEXT NOT NULL,
    actor_id             TEXT REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    previous_state       TEXT,
    new_state            TEXT,
    origin               TEXT NOT NULL CHECK (origin IN ('user', 'automation')),
    causing_execution_id TEXT,
    occurred_at          BIGINT NOT NULL
);

CREATE INDEX idx_domain_events_task ON domain_events (task_id, occurred_at);
CREATE INDEX idx_domain_events_team ON domain_events (team_id, occurred_at);
CREATE INDEX idx_domain_events_causing ON domain_events (causing_execution_id);

CREATE TABLE rule_executions (
    id               TEXT PRIMARY KEY,
    rule_id          TEXT NOT NULL REFERENCES rules (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    rule_version     BIGINT NOT NULL,
    event_id         TEXT NOT NULL REFERENCES domain_events (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    team_id          TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    task_id          TEXT NOT NULL,
    trigger_type     TEXT NOT NULL
        CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned')),
    condition_result TEXT NOT NULL CHECK (condition_result IN ('matched', 'not_matched')),
    status           TEXT NOT NULL DEFAULT 'planned'
        CHECK (status IN ('planned', 'completed', 'failed')),
    planned_actions  TEXT,
    error_category   TEXT,
    error_detail     TEXT,
    created_at       BIGINT NOT NULL,
    completed_at     BIGINT,
    -- One execution per (rule, event): duplicate processing is impossible.
    UNIQUE (rule_id, event_id),
    -- The exact evaluated definition, checked by the database.
    FOREIGN KEY (rule_id, rule_version)
        REFERENCES rule_versions (rule_id, version) ON DELETE RESTRICT ON UPDATE RESTRICT
);

CREATE INDEX idx_rule_executions_event ON rule_executions (event_id);
CREATE INDEX idx_rule_executions_team_created ON rule_executions (team_id, created_at);
CREATE INDEX idx_rule_executions_task_created ON rule_executions (task_id, created_at);

-- Close the domain_events <-> rule_executions cycle (see header).
ALTER TABLE domain_events
    ADD CONSTRAINT fk_domain_events_causing_execution
    FOREIGN KEY (causing_execution_id) REFERENCES rule_executions (id)
    ON DELETE RESTRICT ON UPDATE RESTRICT;

CREATE TABLE automation_outbox (
    id                  TEXT PRIMARY KEY,
    execution_id        TEXT NOT NULL REFERENCES rule_executions (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    event_id            TEXT NOT NULL REFERENCES domain_events (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    rule_id             TEXT NOT NULL REFERENCES rules (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    rule_version        BIGINT NOT NULL,
    task_id             TEXT NOT NULL,
    action_type         TEXT NOT NULL
        CHECK (action_type IN ('send_notification', 'add_comment', 'send_message')),
    schema_version      BIGINT NOT NULL DEFAULT 1,
    payload             TEXT NOT NULL,
    idempotency_key     TEXT NOT NULL UNIQUE,
    status              TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'delivered', 'retryable_failed', 'dead_letter')),
    attempts            BIGINT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at     BIGINT NOT NULL,
    claim_token         TEXT,
    worker_id           TEXT,
    claimed_at          BIGINT,
    lease_expires_at    BIGINT,
    last_error_category TEXT,
    last_error_detail   TEXT,
    created_at          BIGINT NOT NULL,
    updated_at          BIGINT NOT NULL,
    completed_at        BIGINT,
    FOREIGN KEY (rule_id, rule_version)
        REFERENCES rule_versions (rule_id, version) ON DELETE RESTRICT ON UPDATE RESTRICT,
    -- Fencing invariant: only a row in `processing` carries a claim token.
    CHECK ((claim_token IS NULL) = (status <> 'processing'))
);

-- Claim query: WHERE status = 'pending' AND next_attempt_at <= now.
CREATE INDEX idx_automation_outbox_claim ON automation_outbox (status, next_attempt_at);
-- Stale-claim reaper: WHERE status = 'processing' AND lease_expires_at < now.
CREATE INDEX idx_automation_outbox_lease ON automation_outbox (status, lease_expires_at);

CREATE TABLE rule_action_results (
    id           TEXT PRIMARY KEY,
    outbox_id    TEXT NOT NULL UNIQUE REFERENCES automation_outbox (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    execution_id TEXT NOT NULL REFERENCES rule_executions (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    action_type  TEXT NOT NULL
        CHECK (action_type IN ('send_notification', 'add_comment', 'send_message')),
    outcome      TEXT NOT NULL CHECK (outcome IN ('completed', 'skipped')),
    recipient_id TEXT REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    author_id    TEXT REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    body_hash    TEXT,
    detail       TEXT,
    created_at   BIGINT NOT NULL
);

CREATE INDEX idx_rule_action_results_execution ON rule_action_results (execution_id);
