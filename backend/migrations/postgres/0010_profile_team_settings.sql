-- Phase 9: Advanced profile + advanced team settings (PostgreSQL dialect).
-- See migrations/sqlite/0010_profile_team_settings.sql for the full rationale:
-- unique index (not constraint) so ALTER is portable and NULLs never collide;
-- avatars stored as validated data URLs and never serialized into JSON.

ALTER TABLE users ADD COLUMN username TEXT;
ALTER TABLE users ADD COLUMN bio TEXT;
ALTER TABLE users ADD COLUMN timezone TEXT;
ALTER TABLE users ADD COLUMN avatar TEXT;
ALTER TABLE users ADD COLUMN notification_prefs TEXT;

CREATE UNIQUE INDEX idx_users_username ON users (username);

ALTER TABLE teams ADD COLUMN timezone TEXT;
ALTER TABLE teams ADD COLUMN avatar TEXT;
ALTER TABLE teams ADD COLUMN archived_at BIGINT;
