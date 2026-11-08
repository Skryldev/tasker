-- Phase 12.5 (MySQL): instance user administration + team timezone default.
-- See the SQLite variant (0015_instance_users_timezone.sql) for the full
-- narrative: instance_role owner promotion heuristic and the Asia/Tehran
-- timezone backfill.

ALTER TABLE users ADD COLUMN instance_role VARCHAR(16) NOT NULL DEFAULT 'member';

UPDATE users u
JOIN (
    SELECT user_id FROM team_members
    WHERE role = 'owner'
    ORDER BY created_at ASC, user_id ASC
    LIMIT 1
) first_owner ON first_owner.user_id = u.id
SET u.instance_role = 'owner';

UPDATE teams SET timezone = 'Asia/Tehran' WHERE timezone IS NULL;
