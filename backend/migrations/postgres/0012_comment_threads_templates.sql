-- Phase 11: comment threads, template subtasks, task provenance (PostgreSQL).
-- See migrations/sqlite/0012_comment_threads_templates.sql for rationale.

ALTER TABLE comments ADD COLUMN parent_comment_id TEXT REFERENCES comments (id) ON DELETE CASCADE;
CREATE INDEX idx_comments_parent ON comments (parent_comment_id);

ALTER TABLE task_templates ADD COLUMN default_subtasks TEXT;

ALTER TABLE tasks ADD COLUMN from_template_id TEXT REFERENCES task_templates (id) ON DELETE SET NULL;
