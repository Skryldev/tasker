# ADR-0009: Project guest access and per-project dependency serialization

Date: 2026-10-29 · Status: accepted

## Context

Phase 3 required read-only, project-scoped access for external users
(guests) without diluting team membership, and finish-to-start task
dependencies that can never form cycles even under concurrent mutation.

## Decision

### Guest access — `ProjectActor`, not role proliferation

- A new `project_members` table (project, user, role=`viewer`, granted_by)
  grants read access to exactly one project. Guests are never added to
  `team_members` — team-level privileges cannot leak.
- `projects::ProjectActor` replaces bare membership in every project- and
  task-scoped check: it resolves as team membership first, then the guest
  grant. `actor.can(capability)` is false for guests by construction (no
  membership), so every mutation path is denied without new conditionals.
  Read paths test `can_view`.
- Cross-boundary non-disclosure is preserved: guests get 404 for team
  settings, roster, workload, other projects, and templates (they are
  internal collaboration defaults). The roster endpoint specifically
  returns 404 to guests rather than the 403 an unauthorized member gets.
- Comment visibility is an explicit column (`guest_visible`, default
  false). Guests see only comments marked visible; the filter runs in SQL
  on both count and list queries, so search/pagination cannot leak.
- The activity feed is whitelisted for guests to non-sensitive workflow
  events (created/status/priority/due/submitted/accepted/rejected);
  comment events, description updates, rejection reasons and deletions
  stay internal.
- Revocation deletes the row; the very next request re-resolves access and
  404s. Verified by test.

Rejected alternatives: modeling guests as a team role (privilege leak),
duplicating comments into a "public" store (sync burden), per-endpoint
ad-hoc checks (scattered, untestable).

### Dependency cycle safety — per-project serialization

- Edges live in `task_dependencies` (predecessor, successor) with the pair
  as primary key; reverse relations are derived by query. FK cascade on
  task delete keeps the graph consistent.
- The cycle check (BFS over predecessor edges) and the INSERT run inside
  the same critical section: `db::ProjectDependencyLock` serializes per
  project — a no-op on SQLite (single writer), `pg_advisory_xact_lock` on
  PostgreSQL (released at commit), `GET_LOCK`/`RELEASE_LOCK` on
  MySQL/MariaDB. Two concurrent requests therefore cannot both pass the
  check and close a loop.
- Known limitation: the MySQL lock lives on the pooled connection; if the
  transaction were retried on a different connection mid-flight the lock
  would be released early. The current code acquires, checks, inserts, and
  releases on one connection inside one transaction; a connection switch
  cannot occur because no await point yields the request to another task
  between acquire and release on the same handle.

### Workload — distribution, not capacity

`GET /teams/:id/workload` runs server-side GROUP BY aggregation (status
and priority per member) plus dedicated overdue count queries; no boolean
expression columns (portable decode). Team-membership gated; project/date
filters re-validated. Explicitly documented as task distribution, not
effort/capacity.

### Templates — defaults, not automation

Project-scoped `task_templates` with label defaults, managed by Manager+.
"Use" pre-fills the standard task creation flow; every validation rule
(reporter, assignee, status, workflow) applies at creation time, so a
template can never produce a task that the user could not create manually.
Created tasks are independent rows.
