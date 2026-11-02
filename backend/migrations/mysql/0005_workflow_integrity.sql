-- tasker:autocommit
-- Phase 4 hotfix: task lifecycle state machine correction (MariaDB/MySQL).
-- See migrations/postgres/0005_workflow_integrity.sql for the legacy mapping.
--
-- DDL statements implicitly commit in MySQL/MariaDB; this migration therefore
-- runs in autocommit mode and cannot be made atomic. Crash windows and the
-- (fail-closed) recovery for each:
--   * stop before/during the DROP CHECK  -> nothing changed; the runner
--     re-applies the whole migration on the next start;
--   * stop after DROP CHECK, before ADD CHECK -> data still valid (the old
--     `done` rows are untouched or already remapped and both old and new
--     status sets are accepted by no constraint), but a re-run FAILS at
--     `DROP CHECK` because the constraint no longer exists. The server
--     refuses to start (half-applied migrations never serve traffic);
--     recovery is manual: re-apply only the remaining statements, or drop
--     and re-create the named constraint, then restart.
-- The UPDATE statements themselves are idempotent (`WHERE status = 'done'`
-- no-ops once remapped).

ALTER TABLE tasks DROP CHECK chk_tasks_status;

UPDATE tasks SET status = 'closed' WHERE status = 'done' AND resolution = 'accepted';
UPDATE tasks SET status = 'resolved' WHERE status = 'done';

ALTER TABLE tasks ADD CONSTRAINT chk_tasks_status
    CHECK (status IN ('backlog', 'todo', 'in_progress', 'resolved', 'in_review', 'closed'));
