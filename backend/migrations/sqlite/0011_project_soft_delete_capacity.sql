-- Phase 10.5: project soft-delete + per-user capacity limit (SQLite).
--
-- projects.deleted_at  Soft-delete timestamp (epoch ms, NULL while live).
--                      Deleted projects are invisible to every project-scoped
--                      query; recovery is possible within the retention
--                      window via POST /projects/{id}/restore. Task/comment/
--                      dependency rows are intentionally left untouched:
--                      every reachability path flows through the project row
--                      (see docs/adr-0013-project-delete.md), so hiding the
--                      project hides its content without a risky multi-table
--                      tombstone sweep.
-- users.capacity_limit Forward-looking capacity unit (default 10, 1-100)
--                      used by the Capacity view's utilization math.

ALTER TABLE projects ADD COLUMN deleted_at BIGINT;
CREATE INDEX idx_projects_team_deleted ON projects (team_id, deleted_at);

ALTER TABLE users ADD COLUMN capacity_limit INTEGER NOT NULL DEFAULT 10;
