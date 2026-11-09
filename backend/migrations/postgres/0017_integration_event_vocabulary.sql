-- Phase 8B (Task 1): extend the shared event vocabulary for integration
-- events (PostgreSQL). See migrations/sqlite/0017_integration_event_vocabulary.sql
-- for the full rationale.
--
-- The CHECK constraint names are the PostgreSQL auto-generated names for the
-- inline column constraints created in 0009_rules.sql
-- ({table}_{column}_check).

ALTER TABLE domain_events ALTER COLUMN task_id DROP NOT NULL;

ALTER TABLE domain_events DROP CONSTRAINT domain_events_event_type_check;
ALTER TABLE domain_events ADD CONSTRAINT domain_events_event_type_check
    CHECK (event_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned',
                          'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
                          'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
                          'git_release_published'));

ALTER TABLE domain_events DROP CONSTRAINT domain_events_origin_check;
ALTER TABLE domain_events ADD CONSTRAINT domain_events_origin_check
    CHECK (origin IN ('user', 'automation', 'integration'));

ALTER TABLE domain_events
    ADD COLUMN integration_id TEXT REFERENCES integrations (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    ADD COLUMN source_delivery_id TEXT REFERENCES webhook_deliveries (id) ON DELETE RESTRICT ON UPDATE RESTRICT;

CREATE UNIQUE INDEX uq_domain_events_delivery ON domain_events (source_delivery_id);

ALTER TABLE rules DROP CONSTRAINT rules_trigger_type_check;
ALTER TABLE rules ADD CONSTRAINT rules_trigger_type_check
    CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned',
                            'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
                            'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
                            'git_release_published'));

ALTER TABLE rule_versions DROP CONSTRAINT rule_versions_trigger_type_check;
ALTER TABLE rule_versions ADD CONSTRAINT rule_versions_trigger_type_check
    CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned',
                            'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
                            'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
                            'git_release_published'));

ALTER TABLE rule_executions DROP CONSTRAINT rule_executions_trigger_type_check;
ALTER TABLE rule_executions ADD CONSTRAINT rule_executions_trigger_type_check
    CHECK (trigger_type IN ('task_status_changed', 'task_priority_changed', 'task_assigned',
                            'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
                            'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
                            'git_release_published'));
