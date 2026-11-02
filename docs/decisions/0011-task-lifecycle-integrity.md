# ADR-0011: Task lifecycle integrity (state machine correction)

Date: 2026-10-31 · Status: accepted

## Context

The Phase 2 workflow shipped a five-state model
(`backlog/todo/in_progress/in_review/done`) whose rules had drifted from
the intended acceptance process:

- Status writes were accepted by the generic `PATCH /tasks/{id}` endpoint,
  with workflow checks bolted on inside the update handler. Skip moves
  (`backlog -> done`) and reopen paths made the real transition graph
  implicit and hard to audit.
- Role rules leaked overrides: submit allowed "creator or Manager+",
  accept/reject allowed an Owner/Admin reviewer override. With managers
  able to act on any task, the "reporter reviews" model was advisory, not
  enforced.
- The lifecycle vocabulary mixed execution and acceptance in one word
  (`done`), while `resolution` metadata already recorded the acceptance
  decision separately.
- Nothing prevented one person from being both assignee and reporter, so
  self-acceptance was one patch away.
- Three legacy transitions (reopen, manager submit) had no owner in the
  product requirements and no consistent notification recipients.

## Decision

Replace the lifecycle with a strict six-state machine and enforce it in one
place:

```
backlog -> todo -> in_progress -> resolved -> in_review -> closed
                                     ^___________|  (reject, reason required)
```

- **Single policy point.** All authorization and validation for status
  changes lives in `tasks::plan_transition`, a pure, unit-tested function.
  It is the only caller-visible decision point; handlers orchestrate
  persistence around it.
- **Single endpoint.** Status changes go through
  `POST /tasks/{task_id}/transition` (`{to, reason?}`). The generic PATCH
  rejects a `status` field with 400. Illegal pairs, no-op repeats, and any
  transition out of `closed` return 409.
- **Exact roles, no overrides.** The assignee executes (`Backlog -> Todo`,
  `Todo -> In Progress`, `In Progress -> Resolved`); the reporter reviews
  (`Resolved -> In Review`, `In Review -> Closed`, and the rejection return
  with a mandatory audited reason). Manager/Admin/Owner roles have no
  lifecycle bypass — their capabilities govern field edits, membership,
  and deletion, not transitions. The former `ReviewAnyTask` capability
  (the Phase 2 reviewer override) was removed outright during this hotfix:
  nothing consumed it anymore, it was never serialized to clients, and
  leaving it in the matrix would have advertised an authority that no code
  path honors.
- **Identity invariant.** `assignee != reporter` is validated server-side
  at creation and on every assignment/reporter update (422 on violation;
  neither role is ever changed silently). Overlap actors from legacy data
  are stranded for review actions by design and must be corrected via
  PATCH — which remains available precisely so the roles can be fixed.
- **Evidence-based legacy migration.** Migration `0005_workflow_integrity`
  maps the old `done` state conservatively using the Phase 2 `resolution`
  column: `done + resolution=accepted -> closed`; `done` otherwise
  (including `rejected` or no decision) -> `resolved`, keeping the work in
  the reviewable pipeline instead of granting unearned acceptance. The
  SQLite dialect rebuilds the table (CHECK constraints are not alterable)
  under an autocommit migration marker so `PRAGMA foreign_keys = OFF`
  applies; MySQL/MariaDB DDL implicitly commits, so the swap window is
  documented and re-runnable.
- **Creation is actionable-only.** Tasks are born in `backlog`, `todo`, or
  `in_progress`; `resolved`/`in_review`/`closed` are reachable only via
  transitions, so acceptance metadata is always produced by the process
  that owns it.

The frontend mirrors the machine (one `transitionTask` client call,
state-specific action buttons, six board columns) but is never trusted to
enforce it.

## Consequences

- Reopen after acceptance no longer exists: a closed task is final. A new
  task (or a rejection before closure) is the correction path.
- Every lifecycle change is auditable through one activity/event path, and
  notifications have one unambiguous recipient per transition.
- Existing clients that PATCHed status directly break loudly (400) instead
  of silently bypassing the workflow — an intentional contract change,
  reflected in OpenAPI and the regenerated TypeScript types.
- Transition authorization is independent of the capability matrix, which
  keeps the matrix simple; the cost is that privileged automation cannot
  advance tasks on behalf of users without acting as them.

## Verification

- 28 unit tests cover `plan_transition` (every legal edge, role denials,
  overlap stranding, reason validation, terminal `closed`).
- Integration matrices (workflow/notifications, security, Phase 3, goals)
  exercise the full cycle, the 400/403/409/422 error contract, and the
  identity invariant at creation and on PATCH.
- Migration 0005 verified on SQLite for both fresh install and
  legacy-upgrade paths; PostgreSQL/MariaDB migrations follow the portable
  SQL subset and are exercised by CI's three-engine job.
