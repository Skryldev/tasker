-- Phase 9: Advanced profile + advanced team settings (SQLite dialect).
--
-- users gains work-identity columns:
--   * username            unique @mention handle, `[a-z0-9_]{3,32}`; NULL until
--                         the user sets one. The unique INDEX (not constraint)
--                         is the only cross-dialect way to add uniqueness via
--                         ALTER, and unique indexes ignore NULLs on every
--                         engine, so unset usernames never collide.
--   * bio                 ≤200 characters, shown on avatar hover cards.
--   * timezone            IANA name for date display (browser-validated list;
--                         the server enforces the shape only).
--   * avatar              data URL (image/png|jpeg|webp, square, ≤2MB
--                         decoded). Never serialized into JSON responses —
--                         served by GET /users/{user_id}/avatar.
--   * notification_prefs  JSON object of per-event booleans; NULL = all
--                         enabled (pre-Phase-9 behavior).
--
-- teams gains settings columns:
--   * timezone    default timezone for the team.
--   * avatar      data URL under the same rules as user avatars.
--   * archived_at NULL while active; set by POST /teams/{id}/archive.

ALTER TABLE users ADD COLUMN username TEXT;
ALTER TABLE users ADD COLUMN bio TEXT;
ALTER TABLE users ADD COLUMN timezone TEXT;
ALTER TABLE users ADD COLUMN avatar TEXT;
ALTER TABLE users ADD COLUMN notification_prefs TEXT;

CREATE UNIQUE INDEX idx_users_username ON users (username);

ALTER TABLE teams ADD COLUMN timezone TEXT;
ALTER TABLE teams ADD COLUMN avatar TEXT;
ALTER TABLE teams ADD COLUMN archived_at INTEGER;
