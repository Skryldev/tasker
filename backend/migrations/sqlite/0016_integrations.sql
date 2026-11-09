-- Phase 8B (Task 1): provider-neutral Git integration foundation.
-- See docs/integrations.md and ADR-0018.
--
-- Tables:
--   integrations                     team-scoped provider connection (github/gitlab)
--   integration_credentials          ONE active secret row per integration
--                                    (AES-256-GCM envelope, see credentials.rs)
--   integration_repositories         external repositories an integration listens to
--   integration_repository_projects  repo -> Tasker project event routing (M:N)
--   webhook_deliveries               durable webhook intake (schema only in
--                                    Task 1; lifecycle arrives with the
--                                    delivery worker)
--
-- Tenant isolation is database-enforced: `integrations` and `projects` carry
-- UNIQUE(id, team_id) anchors, and the repository/mapping tables carry a
-- `team_id` column with composite foreign keys that force both parents to
-- resolve to the SAME team — a cross-team mapping is unrepresentable.

-- Tenant-isolation anchors: composite foreign keys in
-- integration_repositories/integration_repository_projects reference these
-- UNIQUE(id, team_id) pairs, forcing mapped rows into the same team.
CREATE UNIQUE INDEX uq_projects_id_team_id ON projects (id, team_id);

CREATE TABLE integrations (
    id          TEXT PRIMARY KEY,
    team_id     TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    provider    TEXT NOT NULL CHECK (provider IN ('github', 'gitlab')),
    name        TEXT NOT NULL,
    description TEXT,
    enabled     INTEGER NOT NULL DEFAULT 0,
    created_by  TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    archived_at INTEGER,
    UNIQUE (team_id, name),
    UNIQUE (id, team_id)
);

CREATE INDEX idx_integrations_team ON integrations (team_id);

-- ONE active credential per integration. Rotation replaces the ciphertext in
-- place and increments credential_version; revocation is explicit.
CREATE TABLE integration_credentials (
    id                TEXT PRIMARY KEY,
    integration_id    TEXT NOT NULL UNIQUE REFERENCES integrations (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    secret_encrypted  TEXT NOT NULL,
    credential_version INTEGER NOT NULL CHECK (credential_version >= 1),
    created_by        TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at        INTEGER NOT NULL,
    revoked_at        INTEGER
);

-- External repositories the integration listens to. team_id anchors the
-- composite tenant-isolation foreign key; it is always equal to the owning
-- integration's team.
CREATE TABLE integration_repositories (
    id              TEXT,
    integration_id  TEXT NOT NULL,
    team_id         TEXT NOT NULL,
    external_repo_id VARCHAR(64) NOT NULL,
    full_name       VARCHAR(255) NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (id),
    UNIQUE (id, team_id),
    UNIQUE (integration_id, external_repo_id),
    FOREIGN KEY (integration_id, team_id)
        REFERENCES integrations (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT
);

CREATE INDEX idx_integration_repositories_integration ON integration_repositories (integration_id);

-- Repo -> Tasker project event routing (M:N). Both composite foreign keys
-- must resolve to the same team_id value, so a cross-team mapping is
-- rejected by the database.
CREATE TABLE integration_repository_projects (
    id              TEXT,
    repository_id TEXT NOT NULL,
    project_id   TEXT NOT NULL,
    team_id      TEXT NOT NULL,
    mapped_by    TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (id),
    UNIQUE (repository_id, project_id),
    FOREIGN KEY (repository_id, team_id)
        REFERENCES integration_repositories (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    FOREIGN KEY (project_id, team_id)
        REFERENCES projects (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT
);

CREATE INDEX idx_integration_repo_projects_repo ON integration_repository_projects (repository_id);
CREATE INDEX idx_integration_repo_projects_project ON integration_repository_projects (project_id);

-- Durable webhook intake (schema only in Task 1; the delivery worker and its
-- lifecycle arrive in Task 2). The raw provider payload is preserved for
-- audit/ops under a strict size cap.
CREATE TABLE webhook_deliveries (
    id                  TEXT PRIMARY KEY,
    integration_id      TEXT NOT NULL REFERENCES integrations (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    delivery_id         TEXT NOT NULL,
    event_type_raw      TEXT NOT NULL,
    payload             TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'processed', 'retryable_failed', 'dead_letter', 'unsupported')),
    attempts            INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at     INTEGER NOT NULL,
    claim_token         TEXT,
    worker_id           TEXT,
    claimed_at          INTEGER,
    lease_expires_at    INTEGER,
    last_error_category TEXT,
    last_error_detail   TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    completed_at        INTEGER,
    UNIQUE (integration_id, delivery_id),
    CHECK ((claim_token IS NULL) = (status <> 'processing'))
);

CREATE INDEX idx_webhook_deliveries_claim ON webhook_deliveries (status, next_attempt_at);
CREATE INDEX idx_webhook_deliveries_lease ON webhook_deliveries (status, lease_expires_at);
