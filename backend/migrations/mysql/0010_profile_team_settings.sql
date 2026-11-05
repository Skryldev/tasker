-- Phase 9: Advanced profile + advanced team settings (MySQL/MariaDB dialect).
-- See migrations/sqlite/0010_profile_team_settings.sql for the full rationale.
-- VARCHAR lengths mirror the application limits; avatar carries data URLs of
-- up to ~2.7MB base64, hence MEDIUMTEXT (TEXT caps at 64KB).

ALTER TABLE users ADD COLUMN username VARCHAR(32) NULL;
ALTER TABLE users ADD COLUMN bio VARCHAR(600) NULL;
ALTER TABLE users ADD COLUMN timezone VARCHAR(64) NULL;
ALTER TABLE users ADD COLUMN avatar MEDIUMTEXT NULL;
ALTER TABLE users ADD COLUMN notification_prefs VARCHAR(255) NULL;

CREATE UNIQUE INDEX idx_users_username ON users (username);

ALTER TABLE teams ADD COLUMN timezone VARCHAR(64) NULL;
ALTER TABLE teams ADD COLUMN avatar MEDIUMTEXT NULL;
ALTER TABLE teams ADD COLUMN archived_at BIGINT NULL;
