-- Phase 8B (Task 1): provider-neutral Git integration foundation (PostgreSQL).
-- See migrations/sqlite/0016_integrations.sql for the full rationale.
--
-- Tenant isolation uses composite unique anchors + composite foreign keys so
-- a cross-team repository/project mapping is rejected by the database.

ALTER TABLE projects
    ADD CONSTRAINT uq_projects_id_team_id UNIQUE (id, team_id);

CREATE TABLE integrations (
    id          TEXT PRIMARY KEY,
    team_id     TEXT NOT NULL REFERENCES teams (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    provider    TEXT NOT NULL CHECK (provider IN ('github', 'gitlab')),
    name        TEXT NOT NULL,
    description TEXT,
    enabled     BOOLEAN NOT NULL DEFAULT FALSE,
    created_by  TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL,
    archived_at BIGINT,
    UNIQUE (team_id, name),
    UNIQUE (id, team_id)
);

CREATE INDEX idx_integrations_team ON integrations (team_id);

CREATE TABLE integration_credentials (
    id                 TEXT PRIMARY KEY,
    integration_id     TEXT NOT NULL UNIQUE REFERENCES integrations (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    secret_encrypted   TEXT NOT NULL,
    credential_version BIGINT NOT NULL CHECK (credential_version >= 1),
    created_by         TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at         BIGINT NOT NULL,
    revoked_at         BIGINT
);

CREATE TABLE integration_repositories (
    id               TEXT PRIMARY KEY,
    integration_id   TEXT NOT NULL,
    team_id          TEXT NOT NULL,
    external_repo_id VARCHAR(64) NOT NULL,
    full_name        VARCHAR(255) NOT NULL,
    created_at       BIGINT NOT NULL,
    UNIQUE (id, team_id),
    UNIQUE (integration_id, external_repo_id),
    FOREIGN KEY (integration_id, team_id)
        REFERENCES integrations (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT
);

CREATE INDEX idx_integration_repositories_integration ON integration_repositories (integration_id);

CREATE TABLE integration_repository_projects (
    id            TEXT PRIMARY KEY,
    repository_id TEXT NOT NULL,
    project_id    TEXT NOT NULL,
    team_id       TEXT NOT NULL,
    mapped_by     TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    created_at    BIGINT NOT NULL,
    UNIQUE (repository_id, project_id),
    FOREIGN KEY (repository_id, team_id)
        REFERENCES integration_repositories (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    FOREIGN KEY (project_id, team_id)
        REFERENCES projects (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT
);

CREATE INDEX idx_integration_repo_projects_repo ON integration_repository_projects (repository_id);
CREATE INDEX idx_integration_repo_projects_project ON integration_repository_projects (project_id);

CREATE TABLE webhook_deliveries (
    id                  TEXT PRIMARY KEY,
    integration_id      TEXT NOT NULL REFERENCES integrations (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    delivery_id         VARCHAR(64) NOT NULL,
    event_type_raw      VARCHAR(64) NOT NULL,
    payload             TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'processed', 'retryable_failed', 'dead_letter', 'unsupported')),
    attempts            BIGINT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at     BIGINT NOT NULL,
    claim_token         TEXT,
    worker_id           VARCHAR(64),
    claimed_at          BIGINT,
    lease_expires_at    BIGINT,
    last_error_category VARCHAR(40),
    last_error_detail   VARCHAR(500),
    created_at          BIGINT NOT NULL,
    updated_at          BIGINT NOT NULL,
    completed_at        BIGINT,
    UNIQUE (integration_id, delivery_id),
    CHECK ((claim_token IS NULL) = (status <> 'processing'))
);

CREATE INDEX idx_webhook_deliveries_claim ON webhook_deliveries (status, next_attempt_at);
CREATE INDEX idx_webhook_deliveries_lease ON webhook_deliveries (status, lease_expires_at);
