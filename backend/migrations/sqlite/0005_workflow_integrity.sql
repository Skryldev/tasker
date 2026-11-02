-- tasker:autocommit
-- Phase 4 hotfix: task lifecycle state machine correction (SQLite).
-- See migrations/postgres/0005_workflow_integrity.sql (postgres) for the
-- legacy status mapping rationale.
--
-- SQLite CHECK constraints cannot be altered with ALTER TABLE: the tasks
-- table is rebuilt with the new constraint. The rebuild requires
-- PRAGMA foreign_keys = OFF (a no-op inside a transaction), hence the
-- autocommit marker. Child tables reference tasks(id) by name; the copy is
-- dropped BEFORE the rename on purpose: SQLite (>= 3.25) rewrites
-- REFERENCES clauses in other tables when a table is renamed, so renaming
-- `tasks` out of the way first would repoint every child table at the
-- retired table. Dropping first leaves the child clauses dangling only
-- momentarily; the rename then resolves them to the rebuilt table. Status
-- mapping happens IN the copy: 'done' rows cannot enter the new table (its
-- CHECK rejects them), so the evidence-based mapping is a CASE expression
-- in the SELECT.
--
-- Crash windows (all fail-closed; the version is recorded only after the
-- last statement succeeds, so the runner re-applies on the next start):
--   * stop before the DROP TABLE tasks -> a re-run fails at
--     CREATE TABLE tasks_new ("already exists"); recovery: DROP TABLE
--     tasks_new and restart — the original table is intact;
--   * stop after DROP TABLE tasks, before the rename -> recovery:
--     ALTER TABLE tasks_new RENAME TO tasks and restart; the copy holds
--     every row already mapped.
-- No window can lose data: rows are copied before anything is dropped.

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
    priority       TEXT NOT NULL DEFAULT 'medium'
        CHECK (priority IN ('low', 'medium', 'high', 'urgent')),
    assignee_id    TEXT REFERENCES users (id),
    reporter_id    TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_by     TEXT NOT NULL REFERENCES users (id),
    resolution     TEXT,
    resolution_note TEXT,
    resolved_by    TEXT REFERENCES users (id) ON DELETE SET NULL,
    resolved_at    INTEGER,
    due_date       TEXT,
    position       REAL NOT NULL DEFAULT 0,
    completed_at   INTEGER,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

INSERT INTO tasks_new (
    id, team_id, project_id, parent_task_id, title, description, status, priority,
    assignee_id, reporter_id, created_by, resolution, resolution_note, resolved_by,
    resolved_at, due_date, position, completed_at, created_at, updated_at
)
SELECT
    id, team_id, project_id, parent_task_id, title, description,
    CASE
        WHEN status = 'done' AND resolution = 'accepted' THEN 'closed'
        WHEN status = 'done' THEN 'resolved'
        ELSE status
    END,
    priority,
    assignee_id, reporter_id, created_by, resolution, resolution_note, resolved_by,
    resolved_at, due_date, position, completed_at, created_at, updated_at
FROM tasks;

DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;

CREATE INDEX idx_tasks_project_board ON tasks (project_id, status, position);
CREATE INDEX idx_tasks_project_created ON tasks (project_id, created_at DESC);
CREATE INDEX idx_tasks_assignee ON tasks (assignee_id, status);
CREATE INDEX idx_tasks_team ON tasks (team_id);
CREATE INDEX idx_tasks_parent ON tasks (parent_task_id);
CREATE INDEX idx_tasks_reporter ON tasks (reporter_id);

PRAGMA foreign_keys = ON;
