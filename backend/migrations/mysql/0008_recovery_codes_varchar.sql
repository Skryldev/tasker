-- Phase 7 remediation follow-up (CI integration-matrix finding).
--
-- MySQL's wire protocol reports TEXT columns as BLOB-family types, and
-- sqlx's Any compatibility layer maps every BLOB variant to a Blob value
-- kind that cannot be decoded into String. The project's own rule for the
-- MySQL dialect (see migrations/mysql/0001_init.sql header) is therefore
-- "string columns use VARCHAR with utf8mb4" — migration 0006 violated it
-- by declaring both MFA payload columns as TEXT:
--
--   totp_secret_encrypted  AES-256-GCM envelope, "v1." + ~96 base64url
--                          chars (<= 1024 covers future algorithm tweaks);
--                          every find_user_mfa readback decodes it.
--   recovery_codes_hashed  JSON array of `h1.` HMAC digests (~710 bytes).
--
-- Forward-only: 0006 is already published and must not be edited. Ordering
-- is unconstrained — 0007's statement is content-based (`NOT LIKE '%"h1.%'`)
-- and executes identically against a BLOB or a VARCHAR column, so this
-- migration may safely follow it; MODIFY preserves all values (`[]`, every
-- `h1.` digest, and every encrypted envelope). Fresh installations run
-- 0006 → 0007 → 0008 and end in the same state as upgraded databases. Both
-- sizes are far above the payload bounds and within the InnoDB row limit.
-- PostgreSQL keeps `TEXT` (its wire type decodes to String through the Any
-- driver) and SQLite keeps `TEXT` (dynamic typing); no migration is needed
-- for those dialects.

ALTER TABLE user_mfa
    MODIFY recovery_codes_hashed VARCHAR(4096) CHARACTER SET utf8mb4 NOT NULL;

ALTER TABLE user_mfa
    MODIFY totp_secret_encrypted VARCHAR(1024) CHARACTER SET utf8mb4 NOT NULL;
