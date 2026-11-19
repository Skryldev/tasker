-- Phase 8B (Task 5): taskless automation — integration events flow through
-- the shared Rules Engine without a Tasker task context.
-- See migrations/sqlite/0024_taskless_automation.sql for the rationale;
-- action compatibility is decided at execution time (src/rules/outbox.rs).

ALTER TABLE rule_executions ALTER COLUMN task_id DROP NOT NULL;
ALTER TABLE automation_outbox ALTER COLUMN task_id DROP NOT NULL;
