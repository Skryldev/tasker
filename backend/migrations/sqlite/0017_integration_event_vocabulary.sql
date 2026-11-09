-- tasker:autocommit
-- Phase 8B (Task 1): extend the shared event vocabulary for integration
-- events and make task scope optional on domain_events.
--
-- What changes:
--   * domain_events.event_type  gains the Git integration vocabulary
--     (git_push, git_pull_request_opened, git_pull_request_merged,
--     git_pull_request_closed, git_branch_created, git_branch_deleted,
--     git_release_published);
--   * domain_events.origin      gains 'integration';
--   * domain_events.task_id     becomes NULLable (integration events are
--     taskless; task events always populate it);
--   * domain_events gains integration provenance: nullable integration_id and
--     source_delivery_id (RESTRICT FKs, unique per delivery);
--   * rules / rule_versions trigger_type CHECK gains the same integration
--     vocabulary (one rule set, two namespaces — see docs/rules-engine.md).
--
-- SQLite CHECK constraints cannot be altered in place: the three tables are
-- rebuilt using the established copy -> drop -> rename pattern with
-- PRAGMA foreign_keys = OFF (see migrations/sqlite/0005_workflow_integrity.sql
-- for the crash-window analysis; the same fail-closed windows apply here).
--
-- rule_executions.trigger_type is rebuilt too: Task 5 stores integration
-- triggers on executions. automation_outbox.task_id is deliberately NOT
-- touched (Task 5 decision).

PRAGMA foreign_keys = OFF;

-- ---- domain_events ----
CREATE TABLE domain_events_new (
    id                   TEXT PRIMARY KEY,
    event_type           TEXT NOT NULL
        CHECK (event_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned',
                              'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
                              'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
                              'git_release_published')),
    schema_version       INTEGER NOT NULL DEFAULT 1,
    team_id              TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    -- Historical identifier for task-origin events; NULL for taskless
    -- integration events. Deliberately NOT a foreign key to tasks.
    task_id              TEXT,
    actor_id             TEXT REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    previous_state       TEXT,
    new_state            TEXT,
    origin               TEXT NOT NULL CHECK (origin IN ('user', 'automation', 'integration')),
    causing_execution_id TEXT REFERENCES rule_executions (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    occurred_at          INTEGER NOT NULL,
    -- Integration provenance (NULL for task events; required for
    -- integration events): a delivery must not generate duplicate domain
    -- events, and provenance stays queryable after processing completes.
    integration_id       TEXT REFERENCES integrations (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    source_delivery_id   TEXT REFERENCES webhook_deliveries (id) ON DELETE RESTRICT ON UPDATE RESTRICT
);

INSERT INTO domain_events_new
    (id, event_type, schema_version, team_id, task_id, actor_id, previous_state, new_state,
     origin, causing_execution_id, occurred_at, integration_id, source_delivery_id)
SELECT id, event_type, schema_version, team_id, task_id, actor_id, previous_state, new_state,
       origin, causing_execution_id, occurred_at, NULL, NULL
FROM domain_events;

DROP TABLE domain_events;

ALTER TABLE domain_events_new RENAME TO domain_events;

CREATE INDEX idx_domain_events_task ON domain_events (task_id, occurred_at);
CREATE INDEX idx_domain_events_team ON domain_events (team_id, occurred_at);
CREATE INDEX idx_domain_events_causing ON domain_events (causing_execution_id);
CREATE UNIQUE INDEX uq_domain_events_delivery ON domain_events (source_delivery_id);

-- ---- rules ----
CREATE TABLE rules_new (
    id              TEXT PRIMARY KEY,
    team_id         TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    name            TEXT NOT NULL,
    description     TEXT,
    trigger_type    TEXT NOT NULL
        CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned',
                                'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
                                'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
                                'git_release_published')),
    enabled         INTEGER NOT NULL DEFAULT 0,
    current_version INTEGER NOT NULL DEFAULT 1 CHECK (current_version >= 1),
    condition_json  TEXT NOT NULL,
    actions_json    TEXT NOT NULL,
    created_by      TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    archived_at     INTEGER,
    UNIQUE (team_id, name)
);

INSERT INTO rules_new
    (id, team_id, name, description, trigger_type, enabled, current_version,
     condition_json, actions_json, created_by, created_at, updated_at, archived_at)
SELECT id, team_id, name, description, trigger_type, enabled, current_version,
       condition_json, actions_json, created_by, created_at, updated_at, archived_at
FROM rules;

DROP TABLE rules;

ALTER TABLE rules_new RENAME TO rules;

CREATE INDEX idx_rules_trigger_enabled ON rules (trigger_type, enabled);

-- ---- rule_versions ----
CREATE TABLE rule_versions_new (
    rule_id        TEXT NOT NULL,
    version        INTEGER NOT NULL CHECK (version >= 1),
    trigger_type   TEXT NOT NULL
        CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned',
                                'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
                                'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
                                'git_release_published')),
    condition_json TEXT NOT NULL,
    actions_json   TEXT NOT NULL,
    created_by     TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (rule_id, version)
);

INSERT INTO rule_versions_new
    (rule_id, version, trigger_type, condition_json, actions_json, created_by, created_at)
SELECT rule_id, version, trigger_type, condition_json, actions_json, created_by, created_at
FROM rule_versions;

DROP TABLE rule_versions;

ALTER TABLE rule_versions_new RENAME TO rule_versions;

-- ---- rule_executions ----
CREATE TABLE rule_executions_new (
    id               TEXT PRIMARY KEY,
    rule_id          TEXT NOT NULL REFERENCES rules (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    rule_version     INTEGER NOT NULL,
    event_id         TEXT NOT NULL REFERENCES domain_events (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    team_id          TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    task_id          TEXT NOT NULL,
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

PRAGMA foreign_keys = ON;
