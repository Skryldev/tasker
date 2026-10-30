-- Phase 4: team-level Goals & Key Results (PostgreSQL).

CREATE TABLE goals (
    id           TEXT PRIMARY KEY,
    team_id      TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    title        TEXT NOT NULL,
    description  TEXT,
    owner_id     TEXT NOT NULL REFERENCES users (id),
    status       TEXT NOT NULL DEFAULT 'draft'
        CHECK (status IN ('draft', 'active', 'completed', 'cancelled')),
    start_date   TEXT,
    due_date     TEXT,
    completed_at BIGINT,
    created_by   TEXT NOT NULL REFERENCES users (id),
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
);

CREATE INDEX idx_goals_team ON goals (team_id, status);

CREATE TABLE key_results (
    id           TEXT PRIMARY KEY,
    goal_id      TEXT NOT NULL REFERENCES goals (id) ON DELETE CASCADE,
    title        TEXT NOT NULL,
    description  TEXT,
    metric_type  TEXT NOT NULL CHECK (metric_type IN ('increase', 'decrease', 'boolean')),
    start_value  DOUBLE PRECISION,
    current_value DOUBLE PRECISION NOT NULL DEFAULT 0,
    target_value DOUBLE PRECISION,
    unit         VARCHAR(30),
    progress     DOUBLE PRECISION NOT NULL DEFAULT 0,
    position     DOUBLE PRECISION NOT NULL DEFAULT 0,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
);

CREATE INDEX idx_key_results_goal ON key_results (goal_id, position);

-- Goal ↔ project associations: same team only (enforced in the service),
-- association rows disappear with either side; never deletes the project.
CREATE TABLE goal_projects (
    goal_id    TEXT NOT NULL REFERENCES goals (id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    created_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (goal_id, project_id)
);

CREATE INDEX idx_goal_projects_project ON goal_projects (project_id);

-- Goal ↔ task associations: same semantics; tasks stay in their projects.
CREATE TABLE goal_tasks (
    goal_id    TEXT NOT NULL REFERENCES goals (id) ON DELETE CASCADE,
    task_id    TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    created_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (goal_id, task_id)
);

CREATE INDEX idx_goal_tasks_task ON goal_tasks (task_id);

-- Goal-scoped activity events: the existing activities table is task-scoped;
-- a nullable goal_id extends it without disturbing task history.
ALTER TABLE activities ADD COLUMN goal_id TEXT REFERENCES goals (id) ON DELETE CASCADE;

CREATE INDEX idx_activities_goal ON activities (goal_id, created_at DESC);
