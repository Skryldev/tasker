-- Phase 7: TOTP two-factor authentication (MySQL/MariaDB).
-- See migrations/postgres/0006_totp_mfa.sql for the schema semantics,
-- including the last_used_step replay-protection rationale.

CREATE TABLE user_mfa (
    id                     CHAR(36) NOT NULL,
    user_id                CHAR(36) NOT NULL,
    totp_secret_encrypted  TEXT NOT NULL,
    recovery_codes_hashed  TEXT NOT NULL,
    enabled_at             BIGINT NULL,
    last_used_at           BIGINT NULL,
    last_used_step         BIGINT NULL,
    created_at             BIGINT NOT NULL,
    updated_at             BIGINT NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_user_mfa_user (user_id),
    CONSTRAINT fk_user_mfa_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE TABLE mfa_pending (
    id           CHAR(36) NOT NULL,
    user_id      CHAR(36) NOT NULL,
    token_hash   CHAR(64) NOT NULL,
    created_at   BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL,
    consumed_at  BIGINT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_mfa_pending_token (token_hash),
    CONSTRAINT fk_mfa_pending_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE INDEX idx_mfa_pending_user ON mfa_pending (user_id);
