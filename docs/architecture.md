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

## Task lifecycle workflow (Phase 2, corrected in the pre-Phase-5 integrity hotfix)

Tasks carry three independent responsibilities: `created_by` (who created
it), `reporter_id` (who accepts or rejects the work), and `assignee_id` (who
does the work). The lifecycle is a strict linear machine with one return
edge, enforced centrally by `tasks::plan_transition` (pure, unit-tested):

- `Backlog -> Todo`: assignee only.
- `Todo -> In Progress`: assignee only.
- `In Progress -> Resolved`: assignee only.
- `Resolved -> In Review` (submit): reporter only.
- `In Review -> Closed` (accept): reporter only.
- `In Review -> In Progress` (reject): reporter only; the transition
  endpoint requires an audited reason.

No other transition exists: `closed` is terminal, skips are rejected, and
no Manager/Admin/Owner override bypasses the matrix — general
task-management capabilities govern field edits and deletion, not
lifecycle transitions. The identity invariant (`assignee != reporter`) is
validated server-side at creation and on every assignment/reporter update;
requests that would give one person both roles are rejected, never silently
corrected. Legacy rows with overlapping roles cannot be advanced by the
overlapping user — they are corrected through the generic PATCH, which no
longer accepts a `status` field at all (400; transitions have the dedicated
`POST /tasks/{id}/transition` endpoint).

Transitions persist through compare-and-update (`WHERE id = ? AND
status = ?`), so stale concurrent requests fail with 409 instead of
corrupting state. Every transition writes its activity rows and
notifications in the same transaction.

## Phase 3: guests, dependencies, workload, templates

- **Project guests**: `workspace.rs` + `ProjectActor` in `projects.rs`.
  Guests hold an explicit `project_members` row (viewer) and no team
  membership; every capability check denies them by construction. Internal
  content is protected by `comments.guest_visible` (SQL-filtered) and an
  activity-event whitelist.
- **Dependencies**: `workspace.rs` with per-project serialization via
  `db::ProjectDependencyLock` (SQLite single-writer / PG advisory xact
  lock / MySQL GET_LOCK) around the cycle BFS + insert.
- **Workload**: `GET /teams/:id/workload` — SQL GROUP BY aggregation,
  membership-gated, distribution only (not capacity).
- **Templates**: project-scoped defaults feeding the standard task
  creation flow (no parallel creation path).

## Phase 4: Goals & Key Results

`goals.rs` implements team-level objectives with a validated lifecycle
(draft → active → completed/cancelled + explicit reopen) and server-computed
key-result progress (increase/decrease/boolean — clamped linear formulas in
one pure function). Overall progress is the mean of key-result progress or
`null` when none exist; activation requires at least one key result;
reaching 100% never auto-completes. Goal↔project/task associations are
same-team, non-owning links; the goal detail exposes a task execution
summary that is explicitly distinct from measured outcome progress. Goal
events are written transactionally to the existing activities table via a
nullable `goal_id` column. Authorization uses the existing matrix
(`ManageGoals`: Owner/Admin/Manager); guests receive the standard 404.

## Deliberate non-goals (Phase 1)

Event sourcing, microservices, workflow builders, custom fields, e-mail
delivery, OAuth/OIDC, horizontal scaling guarantees, and plugin systems are
explicitly out of scope. Extension points kept open: configurable statuses
(centralized in `tasks::STATUSES`), activity event vocabulary, per-dialect
migration directories.
