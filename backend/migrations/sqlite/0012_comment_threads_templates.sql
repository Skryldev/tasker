-- Phase 11: comment threads, template subtasks, task provenance (SQLite).
--
-- comments.parent_comment_id  Nullable self-reference: NULL for top-level
--                             comments, the parent's id for replies. Depth is
--                             enforced in the application (max 2: a reply may
--                             never target another reply) — a DB CHECK cannot
--                             see the parent's own parent without a trigger.
--                             ON DELETE CASCADE implements the ADR-0014
--                             choice: deleting a parent deletes its replies.
-- task_templates.default_subtasks  JSON array of subtask titles applied by
--                             POST /tasks/from-template/{template_id}.
-- tasks.from_template_id      Provenance for the "From template" badge; NULL
--                             for tasks created directly. ON DELETE SET NULL
--                             so purging a template never deletes tasks.

ALTER TABLE comments ADD COLUMN parent_comment_id TEXT REFERENCES comments (id) ON DELETE CASCADE;
CREATE INDEX idx_comments_parent ON comments (parent_comment_id);

ALTER TABLE task_templates ADD COLUMN default_subtasks TEXT;

ALTER TABLE tasks ADD COLUMN from_template_id TEXT REFERENCES task_templates (id) ON DELETE SET NULL;
