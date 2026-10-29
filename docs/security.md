# Security

Security posture of Phase 1: what is implemented, what it protects against,
and what remains out of scope.

## Authentication

- **Passwords**: Argon2id (OWASP-recommended baseline: 19 MiB memory, t=2,
  p=1) with per-password random salts. Minimum length 10, maximum 128
  (hashing DoS guard). Verification is constant-time; unknown users run a
  dummy verification to equalize response timing.
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
