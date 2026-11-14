-- Phase 14 (MySQL): dependency engine (WS6, ADR-0023). See the SQLite
-- variant for the model narrative.

CREATE TABLE task_dependencies_new (
    id            CHAR(36) PRIMARY KEY,
    from_task_id  CHAR(36) NOT NULL,
    to_task_id    CHAR(36) NOT NULL,
    type          VARCHAR(16) NOT NULL DEFAULT 'blocks',
    auto_schedule TINYINT NOT NULL DEFAULT 0,
    created_by    CHAR(36) NULL,
    created_at    BIGINT NOT NULL,
    UNIQUE KEY uq_dep (from_task_id, to_task_id, type),
    CONSTRAINT fk_dep_from FOREIGN KEY (from_task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_dep_to FOREIGN KEY (to_task_id) REFERENCES tasks (id) ON DELETE CASCADE,
    CONSTRAINT fk_dep_created_by FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE SET NULL,
    CONSTRAINT chk_dep_type CHECK (type IN ('blocks', 'relates_to', 'duplicates', 'causes'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
INSERT INTO task_dependencies_new
        (id, from_task_id, to_task_id, type, auto_schedule, created_by, created_at)
 SELECT CONCAT('dep-', predecessor_task_id, '-', successor_task_id),
        predecessor_task_id, successor_task_id, 'blocks', 0, created_by, created_at
   FROM task_dependencies;
DROP TABLE task_dependencies;
RENAME TABLE task_dependencies_new TO task_dependencies;
CREATE INDEX idx_dep_to ON task_dependencies (to_task_id);
CREATE INDEX idx_dep_from ON task_dependencies (from_task_id);
