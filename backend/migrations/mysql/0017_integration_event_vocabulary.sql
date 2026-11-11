-- Phase 8B (Task 1): extend the shared event vocabulary for integration
-- events (MySQL). See migrations/sqlite/0017_integration_event_vocabulary.sql
-- for the full rationale.
--
-- The CHECK constraint names are the ones created in 0016's predecessor
-- migrations/mysql/0009_rules.sql. MySQL evaluates CHECKs per statement, so
-- the drop/add pairs below are safe against the existing data: every current
-- row satisfies both the old and the new expressions.

ALTER TABLE domain_events
    MODIFY task_id CHAR(36) NULL,
    DROP CHECK chk_domain_events_event_type,
    ADD CONSTRAINT chk_domain_events_event_type CHECK (event_type IN
        ('task_status_changed', 'task_priority_changed', 'task_assigned',
         'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
         'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
         'git_release_published')),
    DROP CHECK chk_domain_events_origin,
    ADD CONSTRAINT chk_domain_events_origin CHECK (origin IN ('user', 'automation', 'integration')),
    ADD COLUMN integration_id CHAR(36) NULL,
    ADD COLUMN source_delivery_id CHAR(36) NULL,
    ADD CONSTRAINT fk_domain_events_integration FOREIGN KEY (integration_id)
        REFERENCES integrations (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    ADD CONSTRAINT fk_domain_events_delivery FOREIGN KEY (source_delivery_id)
        REFERENCES webhook_deliveries (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    ADD UNIQUE INDEX uq_domain_events_delivery (source_delivery_id),
    ADD CONSTRAINT chk_domain_events_provenance
        CHECK ((origin = 'integration')
               = (integration_id IS NOT NULL AND source_delivery_id IS NOT NULL));

ALTER TABLE rules
    DROP CHECK chk_rules_trigger_type,
    ADD CONSTRAINT chk_rules_trigger_type CHECK (trigger_type IN
        ('task_status_changed', 'task_priority_changed', 'task_assigned',
         'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
         'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
         'git_release_published'));

ALTER TABLE rule_versions
    DROP CHECK chk_rule_versions_trigger_type,
    ADD CONSTRAINT chk_rule_versions_trigger_type CHECK (trigger_type IN
        ('task_status_changed', 'task_priority_changed', 'task_assigned',
         'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
         'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
         'git_release_published'));

ALTER TABLE rule_executions
    DROP CHECK chk_rule_executions_trigger_type,
    ADD CONSTRAINT chk_rule_executions_trigger_type CHECK (trigger_type IN
        ('task_status_changed', 'task_priority_changed', 'task_assigned',
         'git_push', 'git_pull_request_opened', 'git_pull_request_merged',
         'git_pull_request_closed', 'git_branch_created', 'git_branch_deleted',
         'git_release_published'));
