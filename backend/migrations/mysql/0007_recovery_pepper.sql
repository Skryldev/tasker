-- Phase 7 remediation (ADR-0012): invalidate legacy recovery-code digest
-- sets. See migrations/postgres/0007_recovery_pepper.sql for the policy:
-- pre-HMAC unkeyed SHA-256 sets are cleared; users regenerate through the
-- existing authenticated recovery-code flow. enabled_at is preserved.

UPDATE user_mfa
   SET recovery_codes_hashed = '[]'
 WHERE recovery_codes_hashed NOT LIKE '%"h1.%';
