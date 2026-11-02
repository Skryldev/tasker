-- Phase 7 remediation follow-up (CI integration-matrix finding).
--
-- MySQL's wire protocol reports TEXT columns as BLOB-family types, and
-- sqlx's Any driver cannot decode a BLOB-kind value into String — the
-- recovery-code digest readback (`recovery_codes_hashed` is a JSON array
-- of `h1.` HMAC digests, ~710 bytes at most) fails on MySQL while the
-- same code works on PostgreSQL `text` and SQLite `TEXT`. Re-declare the
-- column as a bounded textual type: VARCHAR(4096) maps to VAR_STRING on
-- the wire, which the Any driver decodes as text.
--
-- Forward-only: 0006 is already published and must not be edited. Ordering
-- is unconstrained — 0007's statement is content-based (`NOT LIKE '%"h1.%'`)
-- and executes identically against a BLOB or a VARCHAR column, so 0008 may
-- safely follow it; MODIFY preserves all values (`[]` and every `h1.`
-- digest, the largest of which is 68 characters). Fresh installations run
-- 0006 → 0007 → 0008 and end in the same state as upgraded databases.
-- 4096 chars of utf8mb4 (16 KiB) is far above the payload bound and within
-- the InnoDB row limit.

ALTER TABLE user_mfa
    MODIFY recovery_codes_hashed VARCHAR(4096) NOT NULL CHARACTER SET utf8mb4;
