-- tasker:autocommit
-- Phase 12.5 (SQLite): instance user administration + team timezone default.
--
--   * users.instance_role    Bug 2: 'owner' | 'member'. The instance owner
--                            manages every Tasker account (created via
--                            POST /admin/users, assigned to teams
--                            separately). Team-scoped account creation is
--                            removed (ADR-0014).
--   * teams.timezone         Bug 7: NULL timezones default to the product's
--                            primary market, Asia/Tehran; creation defaults
--                            to the same value.
--
-- Owner promotion heuristic: existing deployments have exactly one
-- administrator — the bootstrap user, who is the earliest team owner.
-- Migration promotes the earliest team owner (by membership creation) so
-- every instance has exactly one account with instance administration
-- rights. bootstrap-admin re-runs are refused while users exist, so this is
-- the only promotion path for existing data.

ALTER TABLE users ADD COLUMN instance_role TEXT NOT NULL DEFAULT 'member';

-- The earliest team owner becomes the instance owner (heuristic documented
-- above; deterministic ordering by membership creation time, then id).
UPDATE users SET instance_role = 'owner'
WHERE id = (
    SELECT user_id FROM team_members
    WHERE role = 'owner'
    ORDER BY created_at ASC, user_id ASC
    LIMIT 1
);

UPDATE teams SET timezone = 'Asia/Tehran' WHERE timezone IS NULL;
