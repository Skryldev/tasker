-- Phase 14 (SQLite): notification action center (WS5, ADR-0022).
--
--   * pinned / pinned_at  — pinned items surface above time groups.
--   * group_key           — client-side batching key (task + actor window).

ALTER TABLE notifications ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE notifications ADD COLUMN pinned_at INTEGER;
ALTER TABLE notifications ADD COLUMN group_key TEXT;
CREATE INDEX idx_notifications_pinned ON notifications (recipient_id, pinned);
