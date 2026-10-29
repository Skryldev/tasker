# ADR-0005: Cookie sessions with origin-based CSRF defense

Date: 2026-10-28 · Status: accepted

## Context

Phase 1 needs browser authentication for a same-origin SPA without an
external identity provider. Options: session cookie (+ CSRF strategy), JWT
in localStorage, or Bearer tokens with refresh machinery.

## Decision

Server-side sessions: opaque 256-bit tokens in an `HttpOnly; SameSite=Lax`
cookie (`Secure` in production); only SHA-256 token hashes stored. CSRF is
defended by (1) SameSite=Lax and (2) an Origin/`Sec-Fetch-Site` check on all
state-changing methods against `TASKER_ALLOWED_ORIGINS`/the request Host.

## Rejected alternatives

- **JWT in localStorage**: XSS-stealable, no server-side revocation.
- **Per-request CSRF tokens**: complicates every client call for no
  additional protection once SameSite + Origin checks hold in a same-origin
  deployment; revisit if cross-site embedding of the API is ever needed.

## Consequences

- Logout/password change revoke server-side instantly (unlike stateless JWT).
- Sessions cost one indexed lookup per request; `last_seen` updates are
  throttled to once per minute per session.
- Non-browser API clients work with zero CSRF ceremony.
- The Origin check treats "same Host" as same-origin so plain HTTP
  deployments work without configuration.
