-- Phase 14 (SQLite): dependency engine (WS6, ADR-0023).
--
-- The Phase 3 `task_dependencies` table stored one implicit relationship
-- type (predecessor blocks successor). Phase 14 generalizes it:
--
--   * `type` column — `blocks` | `relates_to` | `duplicates` | `causes`.
--     Existing rows migrate to `blocks` (their only possible meaning).
--   * `id` surrogate key — many-to-many of the same pair with the same
--     type is still unique, but different types between the same pair are
--     allowed (A blocks B AND A relates to B).
--   * `auto_schedule` flag — opt-in CPM: when the predecessor's due date
--     moves later, the successor's due date may be pulled forward.
--   * `idx_dep_to` index — reverse (blocked-by) lookups.
--
-- Cycle prevention applies only to `blocks` and is enforced in the service
-- layer (DFS from the successor; if the predecessor is reachable, reject).

-- Rebuild: SQLite cannot ALTER ADD a column with a NOT NULL default of a
-- new enum cleanly alongside a PK change, and the old composite PK must be
-- replaced by a surrogate id.
CREATE TABLE task_dependencies_new (
    id            TEXT PRIMARY KEY,
    from_task_id  TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    to_task_id    TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    type          TEXT NOT NULL DEFAULT 'blocks'
        CHECK (type IN ('blocks', 'relates_to', 'duplicates', 'causes')),
    auto_schedule INTEGER NOT NULL DEFAULT 0,
    created_by    TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE (from_task_id, to_task_id, type)
);
INSERT INTO task_dependencies_new
        (id, from_task_id, to_task_id, type, auto_schedule, created_by, created_at)
 SELECT 'dep-' || predecessor_task_id || '-' || successor_task_id,
        predecessor_task_id, successor_task_id, 'blocks', 0, created_by, created_at
   FROM task_dependencies;
DROP TABLE task_dependencies;
ALTER TABLE task_dependencies_new RENAME TO task_dependencies;
CREATE INDEX idx_dep_to ON task_dependencies (to_task_id);
CREATE INDEX idx_dep_from ON task_dependencies (from_task_id);
