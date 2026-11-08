# ADR-0016: Strict password policy enforced server-side

Date: 2026-11-08 · Status: accepted

## Context

Until Phase 12.5, `security::password::validate` checked only length
(min 10, max 128). A password like `a-good-length-passphrase` was
accepted everywhere passwords are set: change-password, public
registration, bootstrap-admin, and (until ADR-0014) team-scoped member
creation. The UI offered no feedback beyond "too short".

## Decision

1. **Policy** (all checks server-side, in `security::password`):
   - Length ≥ **12** (`MIN_PASSWORD_LEN` raised from 10), ≤ 128.
   - At least one lowercase, one uppercase, one digit, one special
     character — each with its own specific message.
   - **Blocklist**: a hand-curated list of ~120 common leaked passwords
     (the classic top entries that already pass class checks, e.g.
     `Password123!`). Matching is exact on the lowercased password. The
     spec allows a bundled hand-written list instead of a literal top-10k
     dump; the class rules remain the primary defense against variants.
   - **Identity rule** (`validate_for_identity`): the password must not
     equal — case-insensitively — the account's email, the email's local
     part, the username, or the display name. It runs right after the
     length check so its message is not shadowed by class errors.
   - **Not the current password**: the change-password handler rejects
     `new == current` with its own message before hashing.
2. **Enforcement points**: `PUT /me/password`, `POST /auth/register`,
   `bootstrap-admin`, and `POST /admin/users` (temporary passwords,
   ADR-0014). All flow through `create_validated` or the explicit
   validators — no path sets a password without the policy.
3. **UI strength meter is advisory only** (`password::strength`, 0-4:
   length thresholds + character-class count). The server remains the
   enforcement point; the meter never unlocks submission of a weak
   password because the server would reject it anyway.
4. **Error messages are specific** ("password must contain a digit",
   "this password is too common", "password must not match your email,
   username or display name") and surface as field errors on
   `new_password` / `password`.

## Consequences

- Existing passwords are not invalidated; the policy applies at set-time
  only (hashing current credentials again is impossible by design).
- Tests: unit coverage for every failure class in `security.rs`, plus the
  `password_policy_matrix` integration test exercising the HTTP flow.
- `docs/security.md` documents the policy and the blocklist approach.
