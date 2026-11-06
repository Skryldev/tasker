-- Phase 11: comment threads, template subtasks, task provenance (MySQL).
-- See migrations/sqlite/0012_comment_threads_templates.sql for rationale.

ALTER TABLE comments ADD COLUMN parent_comment_id CHAR(36) NULL,
  ADD CONSTRAINT fk_comments_parent FOREIGN KEY (parent_comment_id)
    REFERENCES comments (id) ON DELETE CASCADE;
CREATE INDEX idx_comments_parent ON comments (parent_comment_id);

ALTER TABLE task_templates ADD COLUMN default_subtasks TEXT NULL;

ALTER TABLE tasks ADD COLUMN from_template_id CHAR(36) NULL,
  ADD CONSTRAINT fk_tasks_from_template FOREIGN KEY (from_template_id)
    REFERENCES task_templates (id) ON DELETE SET NULL;
