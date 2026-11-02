-- Phase 4 hotfix: task lifecycle state machine correction (PostgreSQL).
--
-- Legacy status mapping (see ADR-0011):
--   backlog -> backlog, todo -> todo, in_progress -> in_progress,
--   in_review -> in_review            (deterministic; meanings unchanged)
--   done + resolution='accepted'      -> closed   (passed the Phase 2
--      reporter-acceptance process; deterministic evidence in resolution)
--   done without acceptance evidence  -> resolved (conservative: work was
--      marked complete but has not passed the new acceptance process; the
--      reporter can move it In Review -> Closed)
--
-- The old CHECK constraint must be replaced. PostgreSQL DDL is
-- transactional, so this migration runs inside the runner's transaction:
-- the constraint swap, the data remap, and the version record commit
-- together, and a crash rolls the whole migration back for a clean re-run.
--
-- The 0001 schema declares the status CHECK inline, so PostgreSQL
-- auto-named it `tasks_status_check`; both that name and the reserved
-- `chk_tasks_status` are dropped defensively before the remap.

ALTER TABLE tasks DROP CONSTRAINT IF EXISTS tasks_status_check;
ALTER TABLE tasks DROP CONSTRAINT IF EXISTS chk_tasks_status;

UPDATE tasks SET status = 'closed' WHERE status = 'done' AND resolution = 'accepted';
UPDATE tasks SET status = 'resolved' WHERE status = 'done';

ALTER TABLE tasks ADD CONSTRAINT chk_tasks_status
    CHECK (status IN ('backlog', 'todo', 'in_progress', 'resolved', 'in_review', 'closed'));
