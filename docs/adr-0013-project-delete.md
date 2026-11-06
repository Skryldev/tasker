# ADR-0013: Project deletion semantics

Date: 2026-11-06 (Phase 10.5)
Status: Accepted

## Context

Projects could be archived (reversible flag) but never removed. Product needs a
safe-by-default deletion path with an escape hatch, without violating the
Phase 2 notification policy, the Phase 8A rules-engine history invariants, or
the audit trail.

## Decision

**Soft-delete by default, purge as a separate, owner-only, re-verified act.**

- `DELETE /api/v1/projects/{id}` sets `projects.deleted_at` (Owner/Admin).
  Managers archive; they do not delete — deletion is a step above archive in
  consequence, so it requires Admin.
- Every live reachability path filters `deleted_at IS NULL`:
  `projects.rs` (find/list/shared), `tasks.rs` (my-tasks visibility + page
  enrichment), `profile.rs` (summary), `workspace.rs` (project→team
  resolution), `teams.rs` (guest listing/revocation), `goals.rs`
  (goal-project associations). Deleted projects therefore 404 like any
  nonexistent project while their rows remain on disk.
- Tasks, comments, and dependencies are **not** tombstoned: all of their
  reachability flows through the project row, so hiding the project hides the
  content. This keeps the migration to one table and avoids a multi-table
  sweep that could not be transactional on SQLite.
- `POST /projects/{id}/restore` clears the flag (Owner/Admin) within a
  30-day retention window; afterwards only purge remains.
- `DELETE /projects/{id}/purge` (Owner only, typed project name + current
  password) hard-deletes the project row; engine-level `ON DELETE CASCADE`
  removes tasks → comments/notifications/dependencies/templates and
  `project_members`/`goal_projects`. **Activity rows are preserved** (their
  `project_id` is historical, not an active foreign key) so the audit trail
  outlives the data, matching the Phase 8A precedent for domain history.
- Notifications policy: nothing changes. Soft-delete writes no rows; purge
  cascades recipient-scoped notifications attached to purged tasks (they
  reference deleted content and are meaningless), consistent with the Phase 2
  rule that notifications live and die with their task.
- Rules engine: Phase 8A rules are **team-scoped** (`rules.team_id`) and
  never reference projects, so the anticipated "rules referencing the project
  block deletion" check has no schema to enforce and is recorded as N/A.

## Alternatives considered

- *Hard delete directly*: irreversible user error; rejected.
- *Tombstone every task/comment row*: transactionally risky on SQLite and
  unnecessary given project-rooted reachability; rejected.
- *Restore window without expiry*: unbounded storage growth for genuinely
  abandoned data; rejected (30-day window, purge available immediately).

## Consequences

- Trash listing is Owner/Admin only (`GET /teams/{id}/projects/trash`).
- `Project.deleted_at` appears in the contract; live payloads omit it
  (`skip_serializing_if`), keeping Phase 1–10 responses byte-identical.
- Migration `0011_project_soft_delete_capacity` (3 dialects) adds the column,
  a `(team_id, deleted_at)` index, and `users.capacity_limit`.
