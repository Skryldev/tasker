-- Phase 14 (PostgreSQL): notification action center (WS5, ADR-0022).
-- See the SQLite variant for the model narrative.

ALTER TABLE notifications ADD COLUMN pinned BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE notifications ADD COLUMN pinned_at BIGINT;
ALTER TABLE notifications ADD COLUMN group_key TEXT;
CREATE INDEX idx_notifications_pinned ON notifications (recipient_id, pinned);
