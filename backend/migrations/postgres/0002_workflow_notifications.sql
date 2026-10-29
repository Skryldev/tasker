-- Phase 2: task acceptance workflow + in-app notifications (PostgreSQL).
-- Reporter: defaults backfilled deterministically — the task creator when they
-- are still a team member, otherwise the project owner (always a member).
-- Resolution: the latest acceptance decision ('accepted' | 'rejected' | NULL =
-- no outstanding decision); history remains in the activity table.

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

ALTER TABLE tasks
    ADD COLUMN resolution VARCHAR(20),
    ADD COLUMN resolution_note VARCHAR(2000),
    ADD COLUMN resolved_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN resolved_at BIGINT,
    ADD CONSTRAINT chk_tasks_resolution CHECK (resolution IN ('accepted', 'rejected'));

CREATE INDEX idx_tasks_reporter ON tasks (reporter_id);

CREATE TABLE notifications (
    id           TEXT PRIMARY KEY,
    recipient_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    event        VARCHAR(60) NOT NULL,
    task_id      TEXT REFERENCES tasks (id) ON DELETE CASCADE,
    actor_id     TEXT REFERENCES users (id) ON DELETE SET NULL,
    summary      VARCHAR(300) NOT NULL,
    created_at   BIGINT NOT NULL,
    read_at      BIGINT
);

CREATE INDEX idx_notifications_recipient ON notifications (recipient_id, created_at DESC);
CREATE INDEX idx_notifications_unread ON notifications (recipient_id, read_at);
