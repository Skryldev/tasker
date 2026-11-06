-- Phase 10.5: project soft-delete + per-user capacity limit (MySQL/MariaDB).
-- See migrations/sqlite/0011_project_soft_delete_capacity.sql for rationale.

ALTER TABLE projects ADD COLUMN deleted_at BIGINT NULL;
CREATE INDEX idx_projects_team_deleted ON projects (team_id, deleted_at);

ALTER TABLE users ADD COLUMN capacity_limit INT NOT NULL DEFAULT 10;
