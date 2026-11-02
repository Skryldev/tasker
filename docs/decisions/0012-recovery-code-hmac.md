# ADR-0012: Recovery-code digests — keyed HMAC-SHA-256 (Phase 7 remediation)

Date: 2026-11-01 · Status: accepted (remediates ADR-0008-era recovery storage)

## Context

Recovery codes are 8 characters drawn from a 31-symbol unambiguous alphabet
(`23456789ABCDEFGHJKMNPQRSTUVWXYZ`; no `0/O`, `1/I/L`), displayed
`XXXX-XXXX`. The code space is therefore

    31^8 = 852,891,037,441 ≈ 2^39.6  (~39.6 bits per code; 10 codes per user)

Phase 7 originally stored **plain unkeyed SHA-256 digests** of the normalized
codes. Its report assessed offline brute-force resistance as "≈ centuries at
10⁹ guesses/s" — that estimate was **wrong by roughly six orders of
magnitude**. Correct figures:

| Attacker hardware | Full code space | Average hit |
|---|---|---|
| 1 GH/s SHA-256 (single modern GPU) | ~14 minutes | ~7 minutes |
| 10 GH/s | ~86 seconds | ~43 seconds |
| 100 GH/s (small multi-GPU rig) | ~9 seconds | ~4.5 seconds |

**Corrected threat model.** Online guessing was never the weak point: a code
is only accepted against a valid single-use MFA-pending token (which itself
requires the account password) and attempts are capped at 5/min per user.
The exposure is the **database-leak scenario**: an attacker holding the
`user_mfa` rows can crack an unkeyed digest offline in minutes-to-hours. In
combination with a known password (a common correlation across breaches),
that defeats the second factor — the exact scenario TOTP exists to defend.
An unsalted fast hash is the wrong primitive for low-entropy secrets.

## Decision

Recovery-code digests are now **keyed HMAC-SHA-256** values:

    stored = "h1." + hex( HMAC-SHA-256(pepper, "tasker:recovery-code:v1" || normalized_code) )

- **Key separation.** The pepper comes from `TASKER_RECOVERY_PEPPER`
  (64 hex chars = 32 bytes), independently provisioned — deliberately not
  derived from `TASKER_MFA_ENCRYPTION_KEY`, so the two secrets live in
  separate compromise domains. A fixed domain-separation string is bound
  into every MAC input.
- **The HMAC does not increase the code's intrinsic entropy** (still ~39.6
  bits). It moves the offline attack: with only the database, testing a
  guess now requires the pepper, so leaked digest rows are useless without
  a second server-side secret.
- **Format tagging.** `h1.` makes the current format self-describing. The
  verification path parses entries and accepts only tagged ones; legacy
  bare-hex digests are structurally rejected even outside the migration —
  they can never verify again.
- **Fail-closed.** Absent pepper → recovery-code generation/verification
  return 500 (the server still boots, matching the encryption-key design);
  malformed pepper → startup aborts. The key is never logged or included
  in errors.

### Key provisioning, loss, and rotation

Provision per deployment with `openssl rand -hex 32`; store it like the
database credentials (secrets manager, environment). **Loss of the pepper
makes all stored recovery digests unverifiable** — the remedy is the same
as the migration below: users regenerate codes through the authenticated
flow; MFA-enabled state is untouched. Rotation invalidates existing digests
exactly the same way (regenerate after rotating). The encryption key and
the pepper are rotated independently.

### Existing-code migration

Unkeyed digests cannot be transformed into keyed ones (plaintext codes are
never stored). Migration `0007_recovery_pepper` (all dialects) applies the
documented invalidation policy atomically: any digest set that does not
contain a current-format (`"h1.`) entry is cleared to `[]`. `enabled_at` is
preserved — affected users keep TOTP login and regenerate codes via
`POST /auth/2fa/recovery-codes` (password-gated, existing flow). There is
no lockout beyond the codes already being invalid, and no authentication
bypass: verification also rejects untagged entries unconditionally, so a
pre-migration database cannot verify legacy codes even before 0007 runs.
There is **no permanent dual-accept state**: only `h1.` entries are ever
accepted, and migration clears all others.

### One-time consumption

Unchanged: a matching code is removed from the stored digest array with a
compare-and-set UPDATE (`WHERE recovery_codes_hashed = <previous set>`), so
concurrent redemptions cannot both succeed, and the redemptions themselves
still require the password-gated MFA-pending token with its rate limits.

### Residual risk

If an attacker obtains **both** the database **and** the pepper, offline
recovery of the ~39.6-bit codes falls back to the minutes-scale figures
above; at that point the attacker also holds the encrypted TOTP secrets
defended only by `TASKER_MFA_ENCRYPTION_KEY`, so full-database + full-secret
compromise is out of scope for any client-side storage scheme — depth comes
from requiring both server-side secrets. Online paths remain bounded by the
pending-token + rate-limit design. HMAC is a digests-into-storage fix, not
a code-space enlargement.

## Tests preventing recurrence

`src/mfa.rs` unit tests: digest equals an independently constructed
HMAC (`recovery_digest_is_keyed_hmac_of_normalized_code`); wrong code or
wrong pepper never matches; constant-time comparison path exercised;
legacy bare-hex entries fail parsing and comparison
(`wrong_code_or_pepper_never_matches_and_legacy_rejected`,
`verification_filter_skips_legacy_digests`); every generated digest is
tagged. `tests/integration.rs`
`mfa_legacy_recovery_digests_rejected`: a real stored legacy digest is
rejected at the HTTP layer; invalidation preserves `enabled_at`;
regeneration issues a fresh tagged set that verifies; persisted rows never
contain plaintext codes.
