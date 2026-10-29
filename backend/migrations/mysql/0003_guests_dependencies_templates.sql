-- Phase 3: project guests, task dependencies, templates, comment visibility
-- (MySQL). See migrations/postgres/0003_guests_dependencies_templates.sql
-- for semantics.

CREATE TABLE project_members (
    project_id  CHAR(36) NOT NULL,
    user_id     CHAR(36) NOT NULL,
    role        VARCHAR(20) NOT NULL DEFAULT 'viewer',
    created_at  BIGINT NOT NULL,
    created_by  CHAR(36) NULL,
    PRIMARY KEY (project_id, user_id),
    CONSTRAINT fk_pm_project FOREIGN KEY (project_id) REFERENCES projects (id) ON DELETE CASCADE,
    CONSTRAINT fk_pm_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    CONSTRAINT fk_pm_granter FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE SET NULL,
    CONSTRAINT chk_pm_role CHECK (role IN ('viewer'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_project_members_user ON project_members (user_id);

CREATE TABLE task_dependencies (
    predecessor_task_id CHAR(36) NOT NULL,
    successor_task_id   CHAR(36) NOT NULL,
    created_at          BIGINT NOT NULL,
    created_by          CHAR(36) NULL,
    PRIMARY KEY (predecessor_task_id, successor_task_id),
    CONSTRAINT fk_dep_pred FOREIGN KEY (predecessor_task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_dep_succ FOREIGN KEY (successor_task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_dep_creator FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_task_dependencies_successor ON task_dependencies (successor_task_id);

CREATE TABLE task_templates (
    id                  CHAR(36) NOT NULL PRIMARY KEY,
    project_id          CHAR(36) NOT NULL,
    name                VARCHAR(160) NOT NULL,
    default_title       VARCHAR(260) NOT NULL,
    default_description VARCHAR(10000) NULL,
    default_priority    VARCHAR(20) NOT NULL DEFAULT 'medium',
    created_by          CHAR(36) NOT NULL,
    created_at          BIGINT NOT NULL,
    updated_at          BIGINT NOT NULL,
    CONSTRAINT fk_templates_project FOREIGN KEY (project_id) REFERENCES projects (id) ON DELETE CASCADE,
    CONSTRAINT fk_templates_creator FOREIGN KEY (created_by) REFERENCES users (id),
    CONSTRAINT chk_templates_priority CHECK (default_priority IN ('low', 'medium', 'high', 'urgent'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_task_templates_project ON task_templates (project_id);

CREATE TABLE task_template_labels (
    template_id CHAR(36) NOT NULL,
    label_id    CHAR(36) NOT NULL,
    PRIMARY KEY (template_id, label_id),
    CONSTRAINT fk_ttl_template FOREIGN KEY (template_id) REFERENCES task_templates (id) ON DELETE CASCADE,
    CONSTRAINT fk_ttl_label FOREIGN KEY (label_id) REFERENCES labels (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

ALTER TABLE comments
    ADD COLUMN guest_visible BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT chk_comments_guest CHECK (guest_visible IN (TRUE, FALSE));

CREATE INDEX idx_comments_task_guest ON comments (task_id, guest_visible);
