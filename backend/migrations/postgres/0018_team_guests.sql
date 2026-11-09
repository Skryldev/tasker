-- Phase 13 (PostgreSQL): team guest role (Bug 7, ADR-0018). See the SQLite
-- variant for the model narrative.

CREATE TABLE team_guests (
    id         TEXT PRIMARY KEY,
    team_id    TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    granted_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at BIGINT NOT NULL,
    revoked_at BIGINT,
    UNIQUE (team_id, user_id)
);
CREATE INDEX idx_team_guests_user ON team_guests (user_id, revoked_at);

CREATE TABLE team_guest_permissions (
    id         TEXT PRIMARY KEY,
    guest_id   TEXT NOT NULL REFERENCES team_guests (id) ON DELETE CASCADE,
    permission TEXT NOT NULL
        CHECK (permission IN ('view_team', 'view_project', 'view_task', 'comment_task', 'view_activity')),
    scope_type TEXT NOT NULL CHECK (scope_type IN ('team', 'project')),
    scope_id   TEXT,
    granted_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at BIGINT NOT NULL,
    UNIQUE (guest_id, permission, scope_type, scope_id)
);
CREATE INDEX idx_guest_permissions_guest ON team_guest_permissions (guest_id);
