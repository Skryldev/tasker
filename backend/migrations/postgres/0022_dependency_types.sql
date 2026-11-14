-- Phase 14 (PostgreSQL): dependency engine (WS6, ADR-0023). See the SQLite
-- variant for the model narrative.

CREATE TABLE task_dependencies_new (
    id            TEXT PRIMARY KEY,
    from_task_id  TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    to_task_id    TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    type          TEXT NOT NULL DEFAULT 'blocks'
        CHECK (type IN ('blocks', 'relates_to', 'duplicates', 'causes')),
    auto_schedule BOOLEAN NOT NULL DEFAULT FALSE,
    created_by    TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at    BIGINT NOT NULL,
    UNIQUE (from_task_id, to_task_id, type)
);
INSERT INTO task_dependencies_new
        (id, from_task_id, to_task_id, type, auto_schedule, created_by, created_at)
 SELECT 'dep-' || predecessor_task_id || '-' || successor_task_id,
        predecessor_task_id, successor_task_id, 'blocks', FALSE, created_by, created_at
   FROM task_dependencies;
DROP TABLE task_dependencies;
ALTER TABLE task_dependencies_new RENAME TO task_dependencies;
CREATE INDEX idx_dep_to ON task_dependencies (to_task_id);
CREATE INDEX idx_dep_from ON task_dependencies (from_task_id);
