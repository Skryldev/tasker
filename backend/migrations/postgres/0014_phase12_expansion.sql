-- Phase 12 WS2/WS3/WS4/WS5/WS11 (PostgreSQL).
-- See migrations/sqlite/0014_phase12_expansion.sql for rationale. The legacy
-- priority CHECK is dropped (app-layer validation is the single write path).

ALTER TABLE tasks DROP CONSTRAINT IF EXISTS chk_tasks_priority;

CREATE TABLE task_checklist_items (
    id         TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    is_checked BOOLEAN NOT NULL DEFAULT FALSE,
    position   DOUBLE PRECISION NOT NULL DEFAULT 0,
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
ALTER TABLE tasks ADD COLUMN deadline TEXT;
