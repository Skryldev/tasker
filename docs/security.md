# Security

Security posture of Phase 1: what is implemented, what it protects against,
and what remains out of scope.

## Two-factor authentication (TOTP, Phase 7)

- **Enrollment**: `POST /auth/2fa/setup` generates a fresh 160-bit secret,
  stores it **encrypted with AES-256-GCM**, and returns the `otpauth://`
  URI + Base32 secret. `POST /auth/2fa/enable` verifies one code at
  window 0, atomically flips `enabled_at`, and returns ten plaintext
  recovery codes — the only response that ever contains them. Repeated
  setup replaces the pending secret; an already-enabled configuration
  cannot be replaced or re-enabled.
- **Key management**: secrets are encrypted under `TASKER_MFA_ENCRYPTION_KEY`
  — exactly 64 hex characters (32 bytes). Generate one per deployment with
  `openssl rand -hex 32`. Configure it in the deployment environment (systemd
  `Environment=`, Docker `-e`, or a secrets manager); it is deliberately
  absent by default so MFA endpoints fail closed (500) rather than falling
  back to plaintext. **Losing the key makes every enrolled user's secret
  permanently unrecoverable** — back it up with the same care as the
  database itself. Decryption failures are uniform internal errors that
  never reveal the secret or the crypto details.
- **Login**: a correct password for an enrolled user yields `{mfa_required,
  mfa_token}` instead of a session. The token is a 256-bit random string
  (`mfp_`-prefixed), bound to the user, TTL 10 minutes, single-use
  (server-side `consumed_at` CAS). It authorizes exactly one endpoint
  (`POST /auth/2fa/verify`) and nothing else; cookie-based session
  resolution never sees it. Only a successful verification issues the
  normal session (through the existing session-creation path).
- **Verification**: six digits, SHA-1, 30 s step, **window 0** (only the
  current step). Replay prevention persists the consumed step counter in
  `user_mfa.last_used_step` via a compare-and-set UPDATE — a timestamp
  alone cannot identify a consumed step. Recovery codes (`XXXX-XXXX`,
  31-symbol unambiguous alphabet, ~39.6 bits) are stored as **keyed
  HMAC-SHA-256 digests** (`h1.`-tagged) under the independently
  provisioned `TASKER_RECOVERY_PEPPER` — never derived from the
  encryption key, never stored in the database (ADR-0012: unkeyed SHA-256
  fell to offline brute force in minutes once the database leaked; the
  pepper does not increase code entropy, it binds the offline attack to a
  second server-side secret). Codes are consumed atomically (the stored
  digest array is compared-and-swapped, so concurrent redemptions cannot
  both succeed); regeneration invalidates every previous code. Legacy
  unkeyed digests are rejected unconditionally and were cleared by
  migration 0007.
- **Rate limiting**: `POST /auth/2fa/verify` allows at most 5 attempts per
  user per minute and 10 per IP per minute (the stricter applies), using
  the existing fixed-window limiter. **Single-instance scope**: the
  counters are process-local, matching the documented deployment model;
  horizontal deployments must front the service with a shared limiter for
  distributed enforcement. The IP counter cannot permanently lock a victim
  out (a valid user still verifies from a different IP), and the per-user
  counter cannot be driven by an attacker without already possessing a
  fresh pending token (which requires the correct password).
- **Disable / regenerate** require the current password; disable removes
  the encrypted secret, recovery hashes, and any outstanding pending
  tokens atomically. A deleted user cascades both MFA tables.
- **No secrets in logs or errors**: codes, secrets, provisioning URIs,
  recovery codes, passwords, and tokens are never logged; setup/enable
  responses carry `Cache-Control: no-store`.

## Authentication

- **Passwords**: Argon2id (OWASP-recommended baseline: 19 MiB memory, t=2,
  p=1) with per-password random salts. Verification is constant-time;
  unknown users run a dummy verification to equalize response timing.
- **Password policy (Phase 12.5, ADR-0016)** — enforced server-side at every
  password-set path (change password, public registration, bootstrap-admin,
  Owner-created accounts):
  - at least **12** characters (max 128 as a hashing-DoS guard);
  - at least one lowercase, one uppercase, one digit and one special
    character, each with a specific error message;
  - rejected when present in a bundled blocklist of common leaked
    passwords (~120 curated entries; matching is exact on the lowercased
    password — the character-class rules remain the primary defense
    against variants);
  - rejected when equal (case-insensitively) to the account's email, the
    email local part, the username, or the display name;
  - the change-password flow additionally rejects a new password equal to
    the current one.
  The UI strength meter (weak → very strong) is advisory only; the server
  is the enforcement point.
- **Sessions**: opaque 256-bit OS-random tokens, base64url-encoded, delivered
  in a `tasker_session` cookie: `HttpOnly; SameSite=Lax; Path=/` plus
  `Secure` when `TASKER_ENV=production`. Tokens are never stored in
  localStorage and never logged. The database stores only the SHA-256 hash of
  the token — a database leak does not leak usable credentials.
- **Lifecycle**: fixed TTL (`TASKER_SESSION_TTL_HOURS`, default 14 days),
  revoked on logout, revoked on password change (all other sessions), and
  rejected when the user is deactivated. Expired sessions are purged lazily
  on login.
- **Bootstrap**: `tasker bootstrap-admin` creates the first user — it refuses
  to run once any user exists, takes credentials from flags/env or a hidden
  prompt, and is the only way to create the initial administrator. There is
  no default account and no unauthenticated user creation (public
  registration is disabled unless explicitly enabled).

## Authorization

- Roles: `Owner > Admin > Manager > Member`, evaluated via one capability
  matrix in `backend/src/authorization.rs` (unit-tested per role).
- Server-side enforcement on every request: the resource's team is resolved
  from the database and the caller's membership is loaded before any
  capability check. Client-supplied team/project/task/user IDs carry no
  authority.
- Cross-team data isolation: resources in teams you are not a member of
  return **404** (existence is not disclosed). The negative-authorization
  suite in `backend/tests/integration.rs` and the Playwright E2E run assert
  this on every supported engine.
- Membership rules: only Owner/Admin can add members; only Owner can grant
  Owner; Admins cannot modify Owner-role members; the last owner cannot be
  demoted or removed; nobody changes their own role. Removed members lose
  task assignments atomically (their assignments are cleared in the same
  transaction).

## Workflow authorization (Phase 2, corrected in the pre-Phase-5 integrity hotfix)

- The lifecycle is a strict six-state machine
  (`backlog -> todo -> in_progress -> resolved -> in_review -> closed`,
  plus the `in_review -> in_progress` rejection return). Only the assignee
  executes (pick up, start, resolve); only the reporter reviews (submit,
  accept, reject). No Manager/Admin/Owner override exists — privileged
  roles govern field edits and deletion, never lifecycle transitions.
- The identity invariant (`assignee != reporter`) is enforced server-side
  at creation and on every assignment/reporter update. Requests that would
  give one person both roles are rejected with 422; neither role is changed
  silently. Legacy rows that already overlap cannot be advanced by the
  overlapping user — one of the two roles must be corrected via the generic
  PATCH first.
- The generic task update rejects a `status` field with 400: transitions
  exist only as the dedicated, policy-checked
  `POST /tasks/{task_id}/transition` endpoint; rejection requires a
  non-empty reason (<= 2000 chars) and is audited.
- Task creators do not gain acceptance rights by creation alone.
- Reporter selection at creation or edit requires the Manager role or above
  and must reference an eligible team member.
- Cross-team actors receive 404s (no existence disclosure); workflow state
  conflicts (illegal transition, stale request, terminal `closed`) return
  409 rather than mutating.
- Notifications are strictly recipient-scoped: foreign ids are 404, unread
  counts and lists are keyed on the authenticated user, and no endpoint
  exposes task content beyond the frozen event summary written while the
  recipient had access.

## Project guests (Phase 3)

Guests are external users granted read-only access to one project. The
grant is a `project_members` row; it confers no team membership, no team
data, and no mutation rights anywhere. Every mutation endpoint rejects
guests with 403 (task/comment/workflow/guest/template); every team-level
endpoint 404s them, exactly like outsiders. Internal comments are hidden
by default (`guest_visible=false` — a member must explicitly mark a
comment visible), and the guest activity feed is whitelisted to
non-sensitive workflow events. Revocation is a row delete; the next
request re-resolves and denies access.

## Goals (Phase 4)

Goals are team-scoped and invisible to guests: every goal endpoint resolves
the caller through `require_goal_access`, which 404s anyone without team
membership (including project guests — a guest grant to one project never
confers visibility of team goals or their key results/summaries). Owner
changes validate team membership; project/task associations validate
same-team membership on both sides; all operations require the
`ManageGoals` capability (Owner/Admin/Manager) for mutations.

## CSRF

Cookie authentication requires cross-site request protection. Tasker layers
two defenses (see ADR-0005):

1. `SameSite=Lax` cookies — browsers withhold the cookie on cross-site
   state-changing requests.
2. An Origin/`Sec-Fetch-Site` middleware on POST/PUT/PATCH/DELETE: a present
   `Origin` must match `TASKER_ALLOWED_ORIGINS` or the request's own
   `Host`; a present `Sec-Fetch-Site` must be `same-origin`/`none`.

Non-browser clients (scripts, curl) send neither header and are unaffected.
This tokenless scheme is appropriate for a self-hosted, same-origin
deployment; per-request CSRF tokens would add complexity without closing a
remaining gap here.

## Input handling

- All SQL uses bound parameters through the portable query layer — no string
  interpolation of user input into SQL. `LIKE` patterns escape `%`, `_`, and
  the escape character.
- Every payload is validated server-side (lengths, enum membership, date
  format, email shape, role names); failures return RFC 9457 validation
  problems with field errors, never 500s (covered by tests).
- Request bodies are capped (`TASKER_BODY_LIMIT_BYTES`, default 1 MiB) and
  requests have a 30 s timeout.

## Transport and headers

- In production, terminate TLS at your reverse proxy; session cookies are
  marked `Secure` so browsers only send them over HTTPS.
- The bundled Caddy config sets `X-Content-Type-Options: nosniff` and
  `Referrer-Policy: strict-origin-when-cross-origin`.
- CORS is disabled by default (same-origin only). When
  `TASKER_ALLOWED_ORIGINS` is set, only listed origins are allowed, with
  credentials, and no wildcard is possible.

## Logging discipline

Request logs record method, path template, status, duration, and request ID.
They never contain bodies, cookies, session tokens, or password hashes.
Internal errors log details server-side and return a sanitized generic
message to clients.

## Secrets management

Secrets come from environment variables only (`.env` for Compose, gitignored;
`deploy/env.example` documents the required names). No secret is embedded in
images or committed files, and `bootstrap-admin` accepts the password via
hidden prompt or env var without echoing it.

## Known limitations (Phase 1)

- No 2FA / WebAuthn, no password reset e-mails (documented deferral).
- Login rate limiting is per-process memory; a multi-replica deployment
  should front it with a shared limiter.
- No audit log of *membership* changes (task activity is recorded; team
  membership changes are planned for Phase 2).
- TLS termination is delegated to the operator's proxy.
