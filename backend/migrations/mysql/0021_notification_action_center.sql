-- Phase 14 (MySQL): notification action center (WS5, ADR-0022).
-- See the SQLite variant for the model narrative.

ALTER TABLE notifications
    ADD COLUMN pinned TINYINT NOT NULL DEFAULT 0,
    ADD COLUMN pinned_at BIGINT NULL,
    ADD COLUMN group_key VARCHAR(120) NULL;
CREATE INDEX idx_notifications_pinned ON notifications (recipient_id, pinned);
