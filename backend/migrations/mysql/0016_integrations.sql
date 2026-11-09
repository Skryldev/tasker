-- Phase 8B (Task 1): provider-neutral Git integration foundation (MySQL).
-- See migrations/sqlite/0016_integrations.sql for the full rationale and
-- docs/integrations.md for the domain model.
--
-- MySQL dialect notes (see migrations/mysql/0001_init.sql header):
--   * string columns are VARCHAR with utf8mb4 (TEXT decodes as BLOB through
--     sqlx Any and cannot be read back as String);
--   * delivery payloads are capped at 15000 characters by the application:
--     VARCHAR(15000) = 15000 x 4 = 60000 bytes + 2 length prefix, which keeps
--     the whole row inside the 65535-byte InnoDB row limit (assessment in
--     docs/integrations.md);
--   * tenant isolation uses composite foreign keys over
--     UNIQUE(id, team_id) anchors on integrations, integration_repositories,
--     and projects (the latter added additively below).

ALTER TABLE projects
    ADD UNIQUE KEY uq_projects_id_team_id (id, team_id);

CREATE TABLE integrations (
    id          CHAR(36) NOT NULL,
    team_id     CHAR(36) NOT NULL,
    provider    VARCHAR(16) NOT NULL,
    name        VARCHAR(120) NOT NULL,
    description VARCHAR(500) NULL,
    enabled     BOOLEAN NOT NULL DEFAULT FALSE,
    created_by  CHAR(36) NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL,
    archived_at BIGINT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_integrations_team_name (team_id, name),
    UNIQUE KEY uq_integrations_id_team (id, team_id),
    CONSTRAINT fk_integrations_team FOREIGN KEY (team_id) REFERENCES teams (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_integrations_creator FOREIGN KEY (created_by) REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_integrations_provider CHECK (provider IN ('github', 'gitlab'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_integrations_team ON integrations (team_id);

CREATE TABLE integration_credentials (
    id                 CHAR(36) NOT NULL,
    integration_id     CHAR(36) NOT NULL,
    secret_encrypted   VARCHAR(512) NOT NULL,
    credential_version BIGINT NOT NULL,
    created_by         CHAR(36) NOT NULL,
    created_at         BIGINT NOT NULL,
    revoked_at         BIGINT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_integration_credentials_integration (integration_id),
    CONSTRAINT fk_integration_credentials_integration FOREIGN KEY (integration_id)
        REFERENCES integrations (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_integration_credentials_creator FOREIGN KEY (created_by) REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_integration_credentials_version CHECK (credential_version >= 1)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE TABLE integration_repositories (
    id               CHAR(36) NOT NULL,
    integration_id   CHAR(36) NOT NULL,
    team_id          CHAR(36) NOT NULL,
    external_repo_id VARCHAR(64) NOT NULL,
    full_name        VARCHAR(255) NOT NULL,
    created_at       BIGINT NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_integration_repositories_id_team (id, team_id),
    UNIQUE KEY uq_integration_repositories_integration_repo (integration_id, external_repo_id),
    CONSTRAINT fk_integration_repositories_integration_team
        FOREIGN KEY (integration_id, team_id)
        REFERENCES integrations (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_integration_repositories_integration ON integration_repositories (integration_id);

CREATE TABLE integration_repository_projects (
    id            CHAR(36) NOT NULL,
    repository_id CHAR(36) NOT NULL,
    project_id    CHAR(36) NOT NULL,
    team_id       CHAR(36) NOT NULL,
    mapped_by     CHAR(36) NOT NULL,
    created_at    BIGINT NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_integration_repo_projects_repo_project (repository_id, project_id),
    CONSTRAINT fk_integration_repo_projects_repo_team
        FOREIGN KEY (repository_id, team_id)
        REFERENCES integration_repositories (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_integration_repo_projects_project_team
        FOREIGN KEY (project_id, team_id)
        REFERENCES projects (id, team_id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT fk_integration_repo_projects_mapper FOREIGN KEY (mapped_by) REFERENCES users (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_integration_repo_projects_repo ON integration_repository_projects (repository_id);
CREATE INDEX idx_integration_repo_projects_project ON integration_repository_projects (project_id);

CREATE TABLE webhook_deliveries (
    id                  CHAR(36) NOT NULL,
    integration_id      CHAR(36) NOT NULL,
    delivery_id         VARCHAR(64) NOT NULL,
    event_type_raw      VARCHAR(64) NOT NULL,
    payload             VARCHAR(15000) NOT NULL,
    status              VARCHAR(16) NOT NULL DEFAULT 'pending',
    attempts            BIGINT NOT NULL DEFAULT 0,
    next_attempt_at     BIGINT NOT NULL,
    claim_token         CHAR(36) NULL,
    worker_id           VARCHAR(64) NULL,
    claimed_at          BIGINT NULL,
    lease_expires_at    BIGINT NULL,
    last_error_category VARCHAR(40) NULL,
    last_error_detail   VARCHAR(500) NULL,
    created_at          BIGINT NOT NULL,
    updated_at          BIGINT NOT NULL,
    completed_at        BIGINT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_webhook_deliveries_integration_delivery (integration_id, delivery_id),
    CONSTRAINT fk_webhook_deliveries_integration FOREIGN KEY (integration_id) REFERENCES integrations (id)
        ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_webhook_deliveries_status CHECK (status IN
        ('pending', 'processing', 'processed', 'retryable_failed', 'dead_letter', 'unsupported')),
    CONSTRAINT chk_webhook_deliveries_attempts CHECK (attempts >= 0),
    CONSTRAINT chk_webhook_deliveries_claim_token
        CHECK ((claim_token IS NULL) = (status <> 'processing'))
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_webhook_deliveries_claim ON webhook_deliveries (status, next_attempt_at);
CREATE INDEX idx_webhook_deliveries_lease ON webhook_deliveries (status, lease_expires_at);
