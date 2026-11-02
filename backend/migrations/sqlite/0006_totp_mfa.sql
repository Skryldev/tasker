-- Phase 7: TOTP two-factor authentication (SQLite).
-- See migrations/postgres/0006_totp_mfa.sql for the schema semantics,
-- including the last_used_step replay-protection rationale.

CREATE TABLE user_mfa (
    id                     TEXT PRIMARY KEY,
    user_id                TEXT NOT NULL UNIQUE REFERENCES users (id) ON DELETE CASCADE,
    totp_secret_encrypted  TEXT NOT NULL,
    recovery_codes_hashed  TEXT NOT NULL,
    enabled_at             INTEGER,
    last_used_at           INTEGER,
    last_used_step         INTEGER,
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL
);

CREATE TABLE mfa_pending (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    consumed_at  INTEGER
);

CREATE INDEX idx_mfa_pending_user ON mfa_pending (user_id);
