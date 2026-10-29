# Architecture

Tasker is a **modular monolith**: one deployable backend process with strict
internal module boundaries, plus a single-page frontend served either by the
backend or by the bundled reverse proxy.

## Backend module map

```
src/
├── main.rs             CLI (serve, bootstrap-admin), tracing init, graceful shutdown
├── lib.rs              module registry + small shared utils (ids, dates, validation)
├── config.rs           TASKER_* environment configuration
├── state.rs            AppState (db pool, config, rate limiter, metrics)
├── error.rs            AppError → RFC 9457 Problem Details responses
├── db.rs               engine detection, bounded pool, portable query layer,
│                       migration runner with per-engine locking, ?→$N adaptation
├── security.rs         Argon2id hashing, token generation/hashing, rate limiter
├── auth.rs             sessions: login/logout/register/me/password, cookies
├── authorization.rs    Role enum, capability matrix, membership resolution
├── teams.rs            teams, members, role changes, labels
├── projects.rs         projects, archive/unarchive
├── tasks.rs            tasks, subtasks, board, list filters, my-tasks
├── comments.rs         comment CRUD with ownership policy
├── activity.rs         event recording + task history endpoint
├── notifications.rs    persistent in-app notifications (transactional writes,
│                       recipient-scoped listing/count/read endpoints)
├── users.rs            user entity, provisioning/validation
└── http/
    ├── mod.rs          router assembly, middleware (request id, metrics, origin
    │                   check, body limit, timeouts), health, static serving
    ├── extractors.rs   client IP resolution
    └── metrics.rs      bounded-cardinality Prometheus text format
```

Dependency direction: `http → modules → db/util`; nothing but `http` depends
on Axum types, and business modules depend only on the portable `db::Conn`
handle. There are no repository traits: `db::Conn` is the single persistence
interface (see ADR-0006).

## Request flow

```
Request
  → tower-http body limit / timeout
  → request_context_middleware   (request id + tracing span via .instrument)
  → origin_check_middleware      (CSRF: Origin/Sec-Fetch-Site on unsafe methods)
  → metrics_middleware           (route-template, method, status-class)
  → axum router → handler (thin: extract, delegate, shape response)
      → module service function (validation, authorization, transactions)
          → db::Conn (portable SQL, engine placeholder adaptation)
  → AppError → Problem Details on failure
```

## Database portability

One SQL dialect runs on all engines (ADR-0003): `?` placeholders (adapted to
`$N` for PostgreSQL at execution time), BIGINT epoch-millisecond timestamps,
TEXT ISO dates, no `RETURNING`, no engine upserts. Schema differences live in
per-dialect migration files applied by a single runner that takes an engine
appropriate lock (`pg_advisory_lock`, `GET_LOCK`, SQLite's own write lock).
Details: `docs/database-support.md`.

## Authorization model

- Roles are ordered `Member < Manager < Admin < Owner`; capability checks are
  a single matrix in `authorization.rs`.
- Every request resolves the resource's owning team from the database and
  loads the caller's membership row; client-supplied IDs are never trusted.
- Resources outside the caller's teams return **404**, not 403, so existence
  is not disclosed across team boundaries.
- Cross-cutting rules (last-owner protection, owner-grant restrictions,
  self-role changes) are enforced inside the teams service transactions.

## Authentication

Opaque 256-bit random session tokens in an `HttpOnly; SameSite=Lax` cookie
(`Secure` in production). Only SHA-256 token hashes are stored. Fixed TTL,
revocation on logout/password change, per-IP login rate limiting. CSRF is
defended by the Origin/Sec-Fetch-Site check on state-changing methods — see
`docs/security.md` for the threat model and ADR-0005 for the rationale.

## Observability

- `tracing` spans per request with request IDs; text or JSON output.
- `/api/v1/health/live` (process only) and `/api/v1/health/ready`
  (database ping) — liveness avoids cascading restarts during DB outages.
- `/metrics` in Prometheus text format with bounded cardinality
  (route template × method × status class), protected by network placement
  (not exposed by the frontend proxy config by default? — it is proxied;
  restrict at the firewall or remove the proxy block if undesired).
- Graceful shutdown: SIGINT/SIGTERM → stop accepting → drain → pool close.

## Frontend

React 18 + TypeScript (strict) + Vite. TanStack Query owns all server state;
no global client store. The API client (`src/api/client.ts`) is derived from
the OpenAPI-generated types (`src/api/schema.d.ts`) — response and payload
types are never hand-maintained. The design system is a small set of
shadcn-style primitives on Radix primitives with semantic CSS-variable
tokens (light/dark). Kanban drag-and-drop uses dnd-kit with optimistic
reordering and rollback on failure.

## Task acceptance workflow (Phase 2)

Tasks carry three independent responsibilities: `created_by` (who created
it), `reporter_id` (who accepts or rejects the work), and `assignee_id` (who
does the work). Transitions are validated business operations
(`tasks::plan_transition`, unit-tested), not free-form status writes:

- `In Progress -> In Review` (submit): assignee, creator (unassigned work),
  or Manager+.
- `In Review -> Done` (accept): the reporter, or the Owner/Admin reviewer
  override — never the assignee; a reporter who is also the assignee is not
  independent and must let an administrator accept.
- `In Review -> In Progress` (reject): reporter or Manager+; the dedicated
  endpoint requires an audited reason.
- `Done -> actionable` (reopen): reporter or Manager+; clears the resolution.

Transitions persist through compare-and-update (`WHERE id = ? AND
status = ?`), so stale concurrent requests fail with 409 instead of
corrupting state. Every transition writes its activity rows and
notifications in the same transaction.

## Deliberate non-goals (Phase 1)

Event sourcing, microservices, workflow builders, custom fields, e-mail
delivery, OAuth/OIDC, horizontal scaling guarantees, and plugin systems are
explicitly out of scope. Extension points kept open: configurable statuses
(centralized in `tasks::STATUSES`), activity event vocabulary, per-dialect
migration directories.
