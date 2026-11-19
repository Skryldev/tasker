-- Phase 8B (Task 5): taskless automation — integration events flow through
-- the shared Rules Engine without a Tasker task context.
-- See migrations/sqlite/0024_taskless_automation.sql for the rationale;
-- action compatibility is decided at execution time (src/rules/outbox.rs).
-- MySQL MODIFY restates the full column definition (CHAR(36), utf8mb4 via
-- the table default).

ALTER TABLE rule_executions
    MODIFY task_id CHAR(36) NULL;
ALTER TABLE automation_outbox
    MODIFY task_id CHAR(36) NULL;
