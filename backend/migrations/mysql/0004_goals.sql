-- Phase 4: team-level Goals & Key Results (MySQL).
-- See migrations/postgres/0004_goals.sql for semantics.

CREATE TABLE goals (
    id           CHAR(36) NOT NULL PRIMARY KEY,
    team_id      CHAR(36) NOT NULL,
    title        VARCHAR(260) NOT NULL,
    description  VARCHAR(5000) NULL,
    owner_id     CHAR(36) NOT NULL,
    status       VARCHAR(20) NOT NULL DEFAULT 'draft',
    start_date   CHAR(10) NULL,
    due_date     CHAR(10) NULL,
    completed_at BIGINT NULL,
    created_by   CHAR(36) NOT NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    CONSTRAINT fk_goals_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE,
    CONSTRAINT fk_goals_owner FOREIGN KEY (owner_id) REFERENCES users (id),
    CONSTRAINT fk_goals_creator FOREIGN KEY (created_by) REFERENCES users (id),
    CONSTRAINT chk_goals_status CHECK (status IN ('draft', 'active', 'completed', 'cancelled'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_goals_team ON goals (team_id, status);

CREATE TABLE key_results (
    id           CHAR(36) NOT NULL PRIMARY KEY,
    goal_id      CHAR(36) NOT NULL,
    title        VARCHAR(260) NOT NULL,
    description  VARCHAR(5000) NULL,
    metric_type  VARCHAR(20) NOT NULL,
    start_value  DOUBLE NULL,
    current_value DOUBLE NOT NULL DEFAULT 0,
    target_value DOUBLE NULL,
    unit         VARCHAR(30) NULL,
    progress     DOUBLE NOT NULL DEFAULT 0,
    position     DOUBLE NOT NULL DEFAULT 0,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    CONSTRAINT fk_kr_goal FOREIGN KEY (goal_id) REFERENCES goals (id) ON DELETE CASCADE,
    CONSTRAINT chk_kr_metric CHECK (metric_type IN ('increase', 'decrease', 'boolean'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_key_results_goal ON key_results (goal_id, position);

CREATE TABLE goal_projects (
    goal_id    CHAR(36) NOT NULL,
    project_id CHAR(36) NOT NULL,
    created_at BIGINT NOT NULL,
    created_by CHAR(36) NULL,
    PRIMARY KEY (goal_id, project_id),
    CONSTRAINT fk_gp_goal FOREIGN KEY (goal_id) REFERENCES goals (id) ON DELETE CASCADE,
    CONSTRAINT fk_gp_project FOREIGN KEY (project_id) REFERENCES projects (id) ON DELETE CASCADE,
    CONSTRAINT fk_gp_creator FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_goal_projects_project ON goal_projects (project_id);

CREATE TABLE goal_tasks (
    goal_id    CHAR(36) NOT NULL,
    task_id    CHAR(36) NOT NULL,
    created_at BIGINT NOT NULL,
    created_by CHAR(36) NULL,
    PRIMARY KEY (goal_id, task_id),
    CONSTRAINT fk_gt_goal FOREIGN KEY (goal_id) REFERENCES goals (id) ON DELETE CASCADE,
    CONSTRAINT fk_gt_task FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_gt_creator FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_goal_tasks_task ON goal_tasks (task_id);

ALTER TABLE activities
    ADD COLUMN goal_id CHAR(36) NULL,
    ADD CONSTRAINT fk_activities_goal FOREIGN KEY (goal_id) REFERENCES goals (id) ON DELETE CASCADE;

CREATE INDEX idx_activities_goal ON activities (goal_id, created_at DESC);
