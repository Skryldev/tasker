-- Tasker initial schema (MySQL dialect).
-- Timestamps: BIGINT unix epoch milliseconds (UTC), application-generated.
-- Dates (due_date): TEXT ISO-8601 YYYY-MM-DD (VARCHAR for MySQL indexability).
-- String columns use VARCHAR with utf8mb4 so values can be indexed without prefix lengths.

CREATE TABLE users (
    id            CHAR(36) NOT NULL PRIMARY KEY,
    email         VARCHAR(255) NOT NULL UNIQUE,
    display_name  VARCHAR(120) NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE TABLE sessions (
    id           CHAR(36) NOT NULL PRIMARY KEY,
    user_id      CHAR(36) NOT NULL,
    token_hash   CHAR(64) NOT NULL UNIQUE,
    created_at   BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL,
    revoked_at   BIGINT NULL,
    last_seen_at BIGINT NOT NULL,
    ip_address   VARCHAR(64) NULL,
    user_agent   VARCHAR(255) NULL,
    CONSTRAINT fk_sessions_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_sessions_user_id ON sessions (user_id);

CREATE TABLE teams (
    id          CHAR(36) NOT NULL PRIMARY KEY,
    name        VARCHAR(120) NOT NULL,
    description VARCHAR(1000) NULL,
    created_by  CHAR(36) NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL,
    CONSTRAINT fk_teams_creator FOREIGN KEY (created_by) REFERENCES users (id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE TABLE team_members (
    team_id    CHAR(36) NOT NULL,
    user_id    CHAR(36) NOT NULL,
    role       VARCHAR(20) NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (team_id, user_id),
    CONSTRAINT fk_tm_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE,
    CONSTRAINT fk_tm_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    CONSTRAINT chk_tm_role CHECK (role IN ('owner', 'admin', 'manager', 'member'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_team_members_user ON team_members (user_id);

CREATE TABLE projects (
    id          CHAR(36) NOT NULL PRIMARY KEY,
    team_id     CHAR(36) NOT NULL,
    name        VARCHAR(160) NOT NULL,
    description VARCHAR(2000) NULL,
    status      VARCHAR(20) NOT NULL DEFAULT 'active',
    owner_id    CHAR(36) NOT NULL,
    created_by  CHAR(36) NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL,
    CONSTRAINT fk_projects_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE,
    CONSTRAINT fk_projects_owner FOREIGN KEY (owner_id) REFERENCES users (id),
    CONSTRAINT fk_projects_creator FOREIGN KEY (created_by) REFERENCES users (id),
    CONSTRAINT chk_projects_status CHECK (status IN ('active', 'archived'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_projects_team ON projects (team_id, status);

CREATE TABLE labels (
    id         CHAR(36) NOT NULL PRIMARY KEY,
    team_id    CHAR(36) NOT NULL,
    name       VARCHAR(80) NOT NULL,
    color      VARCHAR(20) NOT NULL DEFAULT '#6366f1',
    created_at BIGINT NOT NULL,
    UNIQUE KEY uq_labels_team_name (team_id, name),
    CONSTRAINT fk_labels_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE TABLE tasks (
    id             CHAR(36) NOT NULL PRIMARY KEY,
    team_id        CHAR(36) NOT NULL,
    project_id     CHAR(36) NOT NULL,
    parent_task_id CHAR(36) NULL,
    title          VARCHAR(260) NOT NULL,
    description    VARCHAR(10000) NULL,
    status         VARCHAR(20) NOT NULL DEFAULT 'backlog',
    priority       VARCHAR(20) NOT NULL DEFAULT 'medium',
    assignee_id    CHAR(36) NULL,
    created_by     CHAR(36) NOT NULL,
    due_date       CHAR(10) NULL,
    position       DOUBLE NOT NULL DEFAULT 0,
    completed_at   BIGINT NULL,
    created_at     BIGINT NOT NULL,
    updated_at     BIGINT NOT NULL,
    CONSTRAINT fk_tasks_project FOREIGN KEY (project_id) REFERENCES projects (id) ON DELETE CASCADE,
    CONSTRAINT fk_tasks_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE,
    CONSTRAINT fk_tasks_parent FOREIGN KEY (parent_task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_tasks_assignee FOREIGN KEY (assignee_id) REFERENCES users (id),
    CONSTRAINT fk_tasks_creator FOREIGN KEY (created_by) REFERENCES users (id),
    CONSTRAINT chk_tasks_status CHECK (status IN ('backlog', 'todo', 'in_progress', 'in_review', 'done')),
    CONSTRAINT chk_tasks_priority CHECK (priority IN ('low', 'medium', 'high', 'urgent'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_tasks_project_board ON tasks (project_id, status, position);
CREATE INDEX idx_tasks_project_created ON tasks (project_id, created_at DESC);
CREATE INDEX idx_tasks_assignee ON tasks (assignee_id, status);
CREATE INDEX idx_tasks_team ON tasks (team_id);
CREATE INDEX idx_tasks_parent ON tasks (parent_task_id);

CREATE TABLE task_labels (
    task_id  CHAR(36) NOT NULL,
    label_id CHAR(36) NOT NULL,
    PRIMARY KEY (task_id, label_id),
    CONSTRAINT fk_tl_task FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_tl_label FOREIGN KEY (label_id) REFERENCES labels (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_task_labels_label ON task_labels (label_id);

CREATE TABLE comments (
    id         CHAR(36) NOT NULL PRIMARY KEY,
    task_id    CHAR(36) NOT NULL,
    author_id  CHAR(36) NOT NULL,
    body       VARCHAR(10000) NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    CONSTRAINT fk_comments_task FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_comments_author FOREIGN KEY (author_id) REFERENCES users (id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_comments_task ON comments (task_id, created_at);

CREATE TABLE activities (
    id         CHAR(36) NOT NULL PRIMARY KEY,
    team_id    CHAR(36) NOT NULL,
    project_id CHAR(36) NULL,
    task_id    CHAR(36) NULL,
    actor_id   CHAR(36) NULL,
    event      VARCHAR(60) NOT NULL,
    field      VARCHAR(60) NULL,
    old_value  VARCHAR(500) NULL,
    new_value  VARCHAR(500) NULL,
    created_at BIGINT NOT NULL,
    CONSTRAINT fk_act_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE,
    CONSTRAINT fk_act_task FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_act_actor FOREIGN KEY (actor_id) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_activities_task ON activities (task_id, created_at DESC);
CREATE INDEX idx_activities_team ON activities (team_id, created_at DESC);
