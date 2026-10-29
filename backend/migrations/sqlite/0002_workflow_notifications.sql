-- Phase 2: task acceptance workflow + in-app notifications (SQLite).
-- See migrations/postgres/0002_workflow_notifications.sql for semantics.

ALTER TABLE tasks ADD COLUMN reporter_id TEXT REFERENCES users (id) ON DELETE SET NULL;

UPDATE tasks
SET reporter_id = tasks.created_by
WHERE reporter_id IS NULL
  AND EXISTS (
    SELECT 1 FROM team_members tm
    WHERE tm.team_id = tasks.team_id AND tm.user_id = tasks.created_by
  );

UPDATE tasks
SET reporter_id = (SELECT p.owner_id FROM projects p WHERE p.id = tasks.project_id)
WHERE reporter_id IS NULL;

ALTER TABLE tasks ADD COLUMN resolution TEXT;
ALTER TABLE tasks ADD COLUMN resolution_note TEXT;
ALTER TABLE tasks ADD COLUMN resolved_by TEXT REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN resolved_at INTEGER;

CREATE INDEX idx_tasks_reporter ON tasks (reporter_id);

CREATE TABLE notifications (
    id           TEXT PRIMARY KEY,
    recipient_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    event        TEXT NOT NULL,
    task_id      TEXT REFERENCES tasks (id) ON DELETE CASCADE,
    actor_id     TEXT REFERENCES users (id) ON DELETE SET NULL,
    summary      TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    read_at      INTEGER
);

CREATE INDEX idx_notifications_recipient ON notifications (recipient_id, created_at DESC);
CREATE INDEX idx_notifications_unread ON notifications (recipient_id, read_at);
