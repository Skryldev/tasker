-- tasker:autocommit
-- Phase 8B (Task 5): taskless automation — integration events flow through
-- the shared Rules Engine without a Tasker task context.
--
-- rule_executions and automation_outbox become task-optional: an integration
-- rule evaluated on a taskless event (e.g. a Git push) records an execution
-- and enqueues action intents with `task_id NULL`. Action compatibility is
-- decided at execution time (see src/rules/outbox.rs): task-scoped actions
-- (`add_comment`, selector-based notifications) dead-letter immediately with
-- a diagnosable category instead of retrying; explicit-recipient
-- notifications execute with `notifications.task_id NULL` (the column has
-- been nullable since 0002).
--
-- SQLite requires a table rebuild to change column nullability: the
-- 0005/0017-established copy → drop → rename pattern runs statement-by-
-- statement under `PRAGMA foreign_keys = OFF` (the autocommit marker). The
-- documented crash windows apply: the rebuild is idempotent-safe because the
-- migration is recorded only after every statement completes, and a crash
-- mid-rebuild leaves the migration unrecorded (the next boot replays it
-- against the original tables — the first statement below is the new-table
-- create, which fails harmlessly on name collision only if a prior run
-- committed partway; deployment guidance: run migrations under normal
-- single-process startup, as every Tasker boot does).

-- ---- rule_executions: task_id becomes nullable ----
CREATE TABLE rule_executions_new (
    id               TEXT PRIMARY KEY,
    rule_id          TEXT NOT NULL REFERENCES rules (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    rule_version     INTEGER NOT NULL,
    event_id         TEXT NOT NULL REFERENCES domain_events (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    team_id          TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    -- Taskless automation (Phase 8B, Task 5): NULL for integration events.
    task_id          TEXT,
    trigger_type     TEXT NOT NULL
        CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned',
                                'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
                                'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
                                'git_release_published')),
    condition_result TEXT NOT NULL CHECK (condition_result IN ('matched', 'not_matched')),
    status           TEXT NOT NULL DEFAULT 'planned'
        CHECK (status IN ('planned', 'completed', 'failed')),
    planned_actions  TEXT,
    error_category   TEXT,
    error_detail     TEXT,
    created_at       INTEGER NOT NULL,
    completed_at     INTEGER,
    UNIQUE (rule_id, event_id),
    FOREIGN KEY (rule_id, rule_version)
        REFERENCES rule_versions (rule_id, version) ON DELETE RESTRICT ON UPDATE RESTRICT
);

INSERT INTO rule_executions_new
    (id, rule_id, rule_version, event_id, team_id, task_id, trigger_type, condition_result,
     status, planned_actions, error_category, error_detail, created_at, completed_at)
SELECT id, rule_id, rule_version, event_id, team_id, task_id, trigger_type, condition_result,
       status, planned_actions, error_category, error_detail, created_at, completed_at
FROM rule_executions;

DROP TABLE rule_executions;

ALTER TABLE rule_executions_new RENAME TO rule_executions;

CREATE INDEX idx_rule_executions_event ON rule_executions (event_id);
CREATE INDEX idx_rule_executions_team_created ON rule_executions (team_id, created_at);
CREATE INDEX idx_rule_executions_task_created ON rule_executions (task_id, created_at);

-- ---- automation_outbox: task_id becomes nullable ----
CREATE TABLE automation_outbox_new (
    id                  TEXT PRIMARY KEY,
    execution_id        TEXT NOT NULL REFERENCES rule_executions (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    event_id            TEXT NOT NULL REFERENCES domain_events (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    rule_id             TEXT NOT NULL REFERENCES rules (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    rule_version        INTEGER NOT NULL,
    -- Taskless automation (Phase 8B, Task 5): NULL for integration events.
    task_id             TEXT,
    action_type         TEXT NOT NULL
        CHECK (action_type IN ('send_notification', 'add_comment', 'send_message')),
    schema_version      INTEGER NOT NULL DEFAULT 1,
    payload             TEXT NOT NULL,
    idempotency_key     TEXT NOT NULL UNIQUE,
    status              TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'delivered', 'retryable_failed', 'dead_letter')),
    attempts            INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at     INTEGER NOT NULL,
    claim_token         TEXT,
    worker_id           TEXT,
    claimed_at          INTEGER,
    lease_expires_at    INTEGER,
    last_error_category TEXT,
    last_error_detail   TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    completed_at        INTEGER,
    FOREIGN KEY (rule_id, rule_version)
        REFERENCES rule_versions (rule_id, version) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CHECK ((claim_token IS NULL) = (status <> 'processing'))
);

INSERT INTO automation_outbox_new
    (id, execution_id, event_id, rule_id, rule_version, task_id, action_type, schema_version,
     payload, idempotency_key, status, attempts, next_attempt_at, created_at, updated_at,
     completed_at)
SELECT id, execution_id, event_id, rule_id, rule_version, task_id, action_type, schema_version,
       payload, idempotency_key, status, attempts, next_attempt_at, created_at, updated_at,
       completed_at
FROM automation_outbox;

DROP TABLE automation_outbox;

ALTER TABLE automation_outbox_new RENAME TO automation_outbox;

CREATE INDEX idx_automation_outbox_claim ON automation_outbox (status, next_attempt_at);
CREATE INDEX idx_automation_outbox_lease ON automation_outbox (status, lease_expires_at);

PRAGMA foreign_keys = ON;
