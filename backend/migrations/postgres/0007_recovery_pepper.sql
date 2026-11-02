-- Phase 7 remediation (ADR-0012): invalidate legacy recovery-code digest
-- sets.
--
-- Recovery-code digests are now keyed HMAC-SHA-256 under the independently
-- provisioned TASKER_RECOVERY_PEPPER, stored as "h1.<hex>". Digest sets
-- produced before this change hold unkeyed SHA-256 hex digests, which fall
-- to offline brute force in minutes-to-hours once the database leaks.
-- Unkeyed digests cannot be converted to the keyed format (the plaintext
-- codes are never stored), so per the remediation policy they are
-- invalidated here and affected users obtain a fresh set through the
-- existing authenticated recovery-code regeneration flow
-- (POST /auth/2fa/recovery-codes).
--
-- Format detection is content-based and portable: every current-format set
-- contains at least one "h1." element, so any set without one is legacy
-- (or already empty — a no-op). The verification path additionally rejects
-- untagged entries unconditionally, so a pre-migration digest can never be
-- accepted even if this statement did not run.
--
-- enabled_at is deliberately preserved: MFA stays enabled and TOTP login
-- continues to work; only the recovery codes are invalidated. No lockout
-- and no authentication bypass is introduced.

UPDATE user_mfa
   SET recovery_codes_hashed = '[]'
 WHERE recovery_codes_hashed NOT LIKE '%"h1.%';
