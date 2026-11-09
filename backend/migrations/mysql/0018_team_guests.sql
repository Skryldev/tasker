-- Phase 13 (MySQL): team guest role (Bug 7, ADR-0018). See the SQLite
-- variant for the model narrative.

CREATE TABLE team_guests (
    id         CHAR(36) PRIMARY KEY,
    team_id    CHAR(36) NOT NULL,
    user_id    CHAR(36) NOT NULL,
    granted_by CHAR(36) NULL,
    created_at BIGINT NOT NULL,
    revoked_at BIGINT NULL,
    UNIQUE KEY uq_team_guests (team_id, user_id),
    INDEX idx_team_guests_user (user_id, revoked_at),
    CONSTRAINT fk_team_guests_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE,
    CONSTRAINT fk_team_guests_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    CONSTRAINT fk_team_guests_granter FOREIGN KEY (granted_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE team_guest_permissions (
    id         CHAR(36) PRIMARY KEY,
    guest_id   CHAR(36) NOT NULL,
    permission VARCHAR(32) NOT NULL,
    scope_type VARCHAR(16) NOT NULL,
    scope_id   CHAR(36) NULL,
    granted_by CHAR(36) NULL,
    created_at BIGINT NOT NULL,
    UNIQUE KEY uq_guest_permissions (guest_id, permission, scope_type, scope_id),
    INDEX idx_guest_permissions_guest (guest_id),
    CONSTRAINT fk_guest_permissions_guest FOREIGN KEY (guest_id) REFERENCES team_guests (id) ON DELETE CASCADE,
    CONSTRAINT fk_guest_permissions_granter FOREIGN KEY (granted_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
