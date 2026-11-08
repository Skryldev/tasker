-- Phase 12 WS2/WS3/WS4/WS5/WS11 (MySQL/MariaDB).
-- See migrations/sqlite/0014_phase12_expansion.sql for rationale.

ALTER TABLE tasks DROP CHECK chk_tasks_priority;

CREATE TABLE task_checklist_items (
    id         CHAR(36) NOT NULL,
    task_id    CHAR(36) NOT NULL,
    title      TEXT NOT NULL,
    is_checked TINYINT NOT NULL DEFAULT 0,
    position   DOUBLE NOT NULL DEFAULT 0,
    created_by CHAR(36) NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (id),
    CONSTRAINT fk_checklist_task FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_checklist_creator FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
CREATE INDEX idx_checklist_task ON task_checklist_items (task_id, position);

CREATE TABLE task_attachments (
    id          CHAR(36) NOT NULL,
    task_id     CHAR(36) NOT NULL,
    filename    TEXT NOT NULL,
    mime_type   TEXT NOT NULL,
    size_bytes  BIGINT NOT NULL,
    storage_key TEXT NOT NULL,
    uploaded_by CHAR(36) NULL,
    created_at  BIGINT NOT NULL,
    deleted_at  BIGINT NULL,
    PRIMARY KEY (id),
    CONSTRAINT fk_attachments_task FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_attachments_uploader FOREIGN KEY (uploaded_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
CREATE INDEX idx_attachments_task ON task_attachments (task_id, deleted_at);

ALTER TABLE labels ADD COLUMN icon VARCHAR(64) NULL;
ALTER TABLE tasks ADD COLUMN deadline TEXT NULL;
