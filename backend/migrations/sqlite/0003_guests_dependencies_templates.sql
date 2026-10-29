-- Phase 3: project guests, task dependencies, templates, comment visibility
-- (SQLite). See migrations/postgres/0003_guests_dependencies_templates.sql
-- for semantics.

CREATE TABLE project_members (
    project_id  TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role        TEXT NOT NULL DEFAULT 'viewer' CHECK (role IN ('viewer')),
    created_at  INTEGER NOT NULL,
    created_by  TEXT REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (project_id, user_id)
);

CREATE INDEX idx_project_members_user ON project_members (user_id);

CREATE TABLE task_dependencies (
    predecessor_task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    successor_task_id   TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    created_at          INTEGER NOT NULL,
    created_by          TEXT REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (predecessor_task_id, successor_task_id)
);

CREATE INDEX idx_task_dependencies_successor ON task_dependencies (successor_task_id);

CREATE TABLE task_templates (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    default_title       TEXT NOT NULL,
    default_description TEXT,
    default_priority    TEXT NOT NULL DEFAULT 'medium'
        CHECK (default_priority IN ('low', 'medium', 'high', 'urgent')),
    created_by          TEXT NOT NULL REFERENCES users (id),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);

CREATE INDEX idx_task_templates_project ON task_templates (project_id);

CREATE TABLE task_template_labels (
    template_id TEXT NOT NULL REFERENCES task_templates (id) ON DELETE CASCADE,
    label_id    TEXT NOT NULL REFERENCES labels (id) ON DELETE CASCADE,
    PRIMARY KEY (template_id, label_id)
);

ALTER TABLE comments ADD COLUMN guest_visible INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_comments_task_guest ON comments (task_id, guest_visible);
