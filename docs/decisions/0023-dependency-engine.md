# ADR-0023: Typed dependency engine

Date: 2026-11-14 · Status: accepted · Replaces the single-type engine
implicit in the Phase 12 finish-to-start model

## Context

Task dependencies existed only as an implicit finish-to-start edge
(`predecessor_task_id`) with cycle prevention and blockers listing. Real
plans need informational links (duplicates, related work, causation) that
must NOT participate in cycle prevention or scheduling, plus an explicit
record of which edges the scheduler is allowed to act on.

## Decision

1. **Schema (migration 0022).** `task_dependencies` rebuilds with a
   surrogate `id` (the old composite business key remains `UNIQUE
   (from_task_id, to_task_id, type)`), a `type` CHECK over
   `blocks | relates_to | duplicates | causes`, `auto_schedule` (boolean,
   default false), `created_by`, `created_at`, and indexes on both
   endpoints. Every legacy row migrates to `type = 'blocks'` with a
   deterministic id (`dep-<from>-<to>`), preserving semantics.
2. **Type semantics.** Only `blocks` participates in cycle prevention and
   scheduling; `relates_to`, `duplicates`, and `causes` are informational.
   The reverse-pair rule (A blocks B while B blocks A) is a cycle for
   `blocks` and an allowed pair for informational types.
3. **Cycle prevention.** DFS over `blocks` edges from the candidate
   `to_task`; if the `from_task` is reachable, the edge would close a loop
   and is rejected (409). Two concurrent creates are serialized per
   project by `db::ProjectDependencyLock` (an advisory row inside the
   caller's transaction), so two requests cannot both pass the DFS check
   and close a loop together.
4. **Opt-in auto-scheduling (CPM pull).** An edge created with
   `auto_schedule = true` lets the engine pull the dependent task's due
   date forward to the dependency's due date when the dependency moves
   later (evaluated at edge creation and when the dependency's due date
   changes). Auto-scheduling never pushes dates later and never touches
   deadlines — it is a guardrail, not a planner.
5. **Endpoints.** `GET/POST /tasks/{id}/dependencies` (list grouped as
   `blocked_by`/`blocking` with `dep_id`, type, task ref, status, due
   date, plus `can_manage`; create with `from_task_id`, `dep_type`
   defaulting to `blocks`, `auto_schedule`), `DELETE
   /tasks/{id}/dependencies/{dep_id}` (surrogate id), and `GET
   /tasks/{id}/dependencies/graph` (blocks-only one-level expansion for
   the visualization).
6. **Server-enforced close-with-blockers.** Transitioning to `closed`
   with open `blocks` dependencies (status not `closed`/`resolved`)
   returns 409 listing them. Closing anyway requires an override reason
   in the transition body, which is recorded in the audit trail. The UI
   turns the 409 into an override modal.
7. **Guests.** Guests read edges (`can_manage: false`); only members
   create/remove them.
8. **UI.** The picker is a searchable command-menu (query, arrow/enter
   keyboard) with a type radio and an auto-schedule checkbox for `blocks`.
   Edge cards group by type with a colored left bar and an **At Risk**
   badge for overdue unfinished dependencies. A Graph modal (lazy-loaded,
   hand-written SVG — no graph library, per the bundle budget) draws the
   blocks subgraph: blockers → task → blocked. Timeline arrows consume the
   same grouped listing.
9. **Deleting a task cascades its edges** (FK ON DELETE CASCADE on both
   endpoints), so dangling references cannot occur.

## Consequences

- The dependency matrix is testable per type: cycles rejected for
  `blocks` regardless of depth (two-node and multi-node covered in the
  integration matrix), accepted for informational types.
- Surrogate ids decouple the REST surface from the composite business
  key, so a future type change on an edge needs no URL rewrite.
- Auto-scheduling is conservative by design: it only ever moves a due
  date forward to honor a dependency and only when opted in per edge.
