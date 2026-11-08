-- Phase 12.5 (PostgreSQL): instance user administration + team timezone
-- default. See the SQLite variant (0015_instance_users_timezone.sql) for the
-- full narrative: instance_role owner promotion heuristic and the
-- Asia/Tehran timezone backfill.

ALTER TABLE users ADD COLUMN instance_role TEXT NOT NULL DEFAULT 'member';

UPDATE users SET instance_role = 'owner'
WHERE id = (
    SELECT user_id FROM team_members
    WHERE role = 'owner'
    ORDER BY created_at ASC, user_id ASC
    LIMIT 1
);

UPDATE teams SET timezone = 'Asia/Tehran' WHERE timezone IS NULL;
