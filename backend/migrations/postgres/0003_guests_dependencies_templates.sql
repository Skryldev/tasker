-- Phase 3: project guests, task dependencies, templates, comment visibility
-- (PostgreSQL).

-- Project-scoped membership. Guests are NOT team members; the minimum role is
-- 'viewer'. team_members is deliberately untouched.
CREATE TABLE project_members (
    project_id  TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role        TEXT NOT NULL DEFAULT 'viewer' CHECK (role IN ('viewer')),
    created_at  BIGINT NOT NULL,
    created_by  TEXT REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (project_id, user_id)
);

CREATE INDEX idx_project_members_user ON project_members (user_id);

-- Directed finish-to-start dependencies: the predecessor must be done before
-- the successor starts. Each edge is stored once; reverse relations are
-- derived by query. Deleting either task cascades the edge away.
CREATE TABLE task_dependencies (
    predecessor_task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    successor_task_id   TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    created_at          BIGINT NOT NULL,
    created_by          TEXT REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (predecessor_task_id, successor_task_id)
);

CREATE INDEX idx_task_dependencies_successor ON task_dependencies (successor_task_id);

-- Project-scoped task templates (defaults only; never live references).
CREATE TABLE task_templates (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    default_title       TEXT NOT NULL,
    default_description TEXT,
    default_priority    TEXT NOT NULL DEFAULT 'medium'
        CHECK (default_priority IN ('low', 'medium', 'high', 'urgent')),
    created_by          TEXT NOT NULL REFERENCES users (id),
    created_at          BIGINT NOT NULL,
    updated_at          BIGINT NOT NULL
);

CREATE INDEX idx_task_templates_project ON task_templates (project_id);

CREATE TABLE task_template_labels (
    template_id TEXT NOT NULL REFERENCES task_templates (id) ON DELETE CASCADE,
    label_id    TEXT NOT NULL REFERENCES labels (id) ON DELETE CASCADE,
    PRIMARY KEY (template_id, label_id)
);

-- Comment guest visibility: existing and new comments are internal-only by
-- default; only a team member may mark a comment visible to project guests.
ALTER TABLE comments ADD COLUMN guest_visible BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX idx_comments_task_guest ON comments (task_id, guest_visible);
