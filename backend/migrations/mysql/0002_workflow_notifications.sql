-- Phase 2: task acceptance workflow + in-app notifications (MySQL).
-- See migrations/postgres/0002_workflow_notifications.sql for semantics.

ALTER TABLE tasks
    ADD COLUMN reporter_id CHAR(36) NULL,
    ADD COLUMN resolution VARCHAR(20) NULL CHECK (resolution IN ('accepted', 'rejected')),
    ADD COLUMN resolution_note VARCHAR(2000) NULL,
    ADD COLUMN resolved_by CHAR(36) NULL,
    ADD COLUMN resolved_at BIGINT NULL,
    ADD CONSTRAINT fk_tasks_reporter FOREIGN KEY (reporter_id) REFERENCES users (id) ON DELETE SET NULL,
    ADD CONSTRAINT fk_tasks_resolved_by FOREIGN KEY (resolved_by) REFERENCES users (id) ON DELETE SET NULL;

UPDATE tasks
SET tasks.reporter_id = tasks.created_by
WHERE tasks.reporter_id IS NULL
  AND EXISTS (
    SELECT 1 FROM team_members tm
    WHERE tm.team_id = tasks.team_id AND tm.user_id = tasks.created_by
  );

UPDATE tasks
SET tasks.reporter_id = (SELECT p.owner_id FROM projects p WHERE p.id = tasks.project_id)
WHERE tasks.reporter_id IS NULL;

CREATE INDEX idx_tasks_reporter ON tasks (reporter_id);

CREATE TABLE notifications (
    id           CHAR(36) NOT NULL PRIMARY KEY,
    recipient_id CHAR(36) NOT NULL,
    event        VARCHAR(60) NOT NULL,
    task_id      CHAR(36) NULL,
    actor_id     CHAR(36) NULL,
    summary      VARCHAR(300) NOT NULL,
    created_at   BIGINT NOT NULL,
    read_at      BIGINT NULL,
    CONSTRAINT fk_notifications_recipient FOREIGN KEY (recipient_id) REFERENCES users (id) ON DELETE CASCADE,
    CONSTRAINT fk_notifications_task FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_notifications_actor FOREIGN KEY (actor_id) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_notifications_recipient ON notifications (recipient_id, created_at DESC);
CREATE INDEX idx_notifications_unread ON notifications (recipient_id, read_at);
