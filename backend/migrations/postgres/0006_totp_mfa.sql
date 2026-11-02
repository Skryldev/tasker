-- Phase 7: TOTP two-factor authentication (PostgreSQL).
--
-- user_mfa holds one MFA configuration per user (user_id is UNIQUE):
--   * totp_secret_encrypted  AES-256-GCM envelope (see src/mfa.rs), TEXT.
--     The plaintext secret is never persisted anywhere.
--   * recovery_codes_hashed  JSON array of SHA-256 hex digests of the
--     normalized recovery codes; plaintext codes are never persisted.
--   * enabled_at             set when a TOTP code confirms enrollment.
--   * last_used_at / last_used_step  replay protection for TOTP: a timestamp
--     alone cannot identify a consumed 30-second step, so the consumed
--     step counter (unix_time / 30) is persisted and compared atomically
--     (compare-and-set) to make replay prevention safe under concurrency.
--
-- mfa_pending holds the short-lived, one-time-use MFA-pending credentials
-- issued between password verification and TOTP verification. Only the
-- SHA-256 hash of the token is stored; consumed_at makes redemption
-- single-use under concurrency.

CREATE TABLE user_mfa (
    id                     TEXT PRIMARY KEY,
    user_id                TEXT NOT NULL UNIQUE REFERENCES users (id) ON DELETE CASCADE,
    totp_secret_encrypted  TEXT NOT NULL,
    recovery_codes_hashed  TEXT NOT NULL,
    enabled_at             BIGINT,
    last_used_at           BIGINT,
    last_used_step         BIGINT,
    created_at             BIGINT NOT NULL,
    updated_at             BIGINT NOT NULL
);

CREATE TABLE mfa_pending (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL,
    consumed_at  BIGINT
);

CREATE INDEX idx_mfa_pending_user ON mfa_pending (user_id);
