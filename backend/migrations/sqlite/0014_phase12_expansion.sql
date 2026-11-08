-- tasker:autocommit
-- Phase 12 (SQLite): priority system expansion + deadline + checklists +
-- attachments.
--
-- SQLite CHECK constraints cannot be altered with ALTER TABLE: the tasks
-- table is rebuilt (0005 pattern) to drop the legacy four-level priority
-- CHECK. The six-level priority set is enforced in the application layer
-- (tasks::validate_priority — the single write path), so the rebuilt table
-- deliberately carries NO priority CHECK.
--
-- Also in this migration:
--   * tasks.deadline            WS3 hard-cutoff date (app: >= due_date).
--   * task_checklist_items      WS4 lightweight checklist entries.
--   * task_attachments          WS11 file metadata (bytes in storage backend).
--   * labels.icon             WS5 label icon (app-whitelisted Lucide name).
--
-- Crash windows (all fail-closed; the version is recorded only after the
-- last statement succeeds): identical to the 0005 rebuild — see that file.
--
-- The new child tables are created AFTER the rebuild so their FK clauses
-- resolve to the rebuilt tasks table.

PRAGMA foreign_keys = OFF;

CREATE TABLE tasks_new (
    id             TEXT PRIMARY KEY,
    team_id        TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    project_id     TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    parent_task_id TEXT REFERENCES tasks (id) ON DELETE CASCADE,
    title          TEXT NOT NULL,
    description    TEXT,
    status         TEXT NOT NULL DEFAULT 'backlog'
        CHECK (status IN ('backlog', 'todo', 'in_progress', 'resolved', 'in_review', 'closed')),
    priority       TEXT NOT NULL DEFAULT 'medium',
    assignee_id    TEXT REFERENCES users (id),
    reporter_id    TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_by     TEXT NOT NULL REFERENCES users (id),
    resolution     TEXT,
    resolution_note TEXT,
    resolved_by    TEXT REFERENCES users (id) ON DELETE SET NULL,
    resolved_at    INTEGER,
    due_date       TEXT,
    deadline       TEXT,
    position       REAL NOT NULL DEFAULT 0,
    completed_at   INTEGER,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    from_template_id TEXT REFERENCES task_templates (id) ON DELETE SET NULL
);

INSERT INTO tasks_new (
    id, team_id, project_id, parent_task_id, title, description, status, priority,
    assignee_id, reporter_id, created_by, resolution, resolution_note, resolved_by,
    resolved_at, due_date, position, completed_at, created_at, updated_at, from_template_id
)
SELECT
    id, team_id, project_id, parent_task_id, title, description, status, priority,
    assignee_id, reporter_id, created_by, resolution, resolution_note, resolved_by,
    resolved_at, due_date, position, completed_at, created_at, updated_at, from_template_id
FROM tasks;

DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;

CREATE INDEX idx_tasks_project_board ON tasks (project_id, status, position);
CREATE INDEX idx_tasks_project_created ON tasks (project_id, created_at DESC);
CREATE INDEX idx_tasks_assignee ON tasks (assignee_id, status);
CREATE INDEX idx_tasks_team ON tasks (team_id);
CREATE INDEX idx_tasks_parent ON tasks (parent_task_id);
CREATE INDEX idx_tasks_reporter ON tasks (reporter_id);
CREATE INDEX idx_tasks_project_due ON tasks (project_id, due_date);

CREATE TABLE task_checklist_items (
    id         TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    is_checked INTEGER NOT NULL DEFAULT 0,
    position   REAL NOT NULL DEFAULT 0,
    created_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_checklist_task ON task_checklist_items (task_id, position);

CREATE TABLE task_attachments (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    filename    TEXT NOT NULL,
    mime_type   TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL,
    storage_key TEXT NOT NULL,
    uploaded_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);
CREATE INDEX idx_attachments_task ON task_attachments (task_id, deleted_at);

ALTER TABLE labels ADD COLUMN icon TEXT;

PRAGMA foreign_keys = ON;
