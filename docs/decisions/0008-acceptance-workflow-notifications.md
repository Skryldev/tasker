# ADR-0008: Reporter acceptance workflow and transactional notifications

Date: 2026-10-29 · Status: accepted (workflow model superseded by
[ADR-0011](0011-task-lifecycle-integrity.md), which removed the
Owner/Admin reviewer override and the `done` status)

## Context

Phase 1 tasks have an assignee and a creator but no acceptance semantics:
any permitted actor could move a task to Done, so "completed" carried no
reviewer approval. Teams need work to be submitted, accepted or rejected by
a responsible reviewer, with notifications drawing attention to the
decisions — without adding infrastructure (brokers, job systems, workflow
engines) or a second status model.

## Decision

1. **Three independent responsibilities.** `created_by` (creator),
   `reporter_id` (acceptance authority), `assignee_id` (performer).
   `reporter_id` defaults to the creator; selecting another reporter
   requires Manager+ and team membership. Existing rows backfill
   deterministically: creator when still a member, else the project owner.

2. **Validated transitions, not scattered writes.** `plan_transition`
   (pure, unit-tested) maps `(from, to, actor relations, capabilities)` to
   Submit / Accept / Reject / Reopen / Planning / None and enforces the
   permission rules. The generic PATCH path uses the same function, so the
   Kanban board cannot bypass review: Done is reachable only via acceptance.

3. **Reviewer independence.** Accept/reject require the reporter or the
   Owner/Admin `ReviewAnyTask` override — and never the assignee, even when
   the assignee is also the reporter (that role is not independent; such
   tasks need an administrator).

4. **Minimal resolution model.** `resolution` (`accepted`/`rejected`/NULL),
   `resolution_note`, `resolved_by`, `resolved_at` describe only the latest
   decision. Submitting again clears the fields (a new cycle never inherits
   an old decision); the full history stays in the activity table
   (`task_submitted`, `task_accepted`, `task_rejected`,
   `task_rejection_reason`, plus `task_status_changed` rows).

5. **Concurrency via compare-and-update.** Transitions execute
   `UPDATE … WHERE id = ? AND status = ?` and treat zero rows affected as a
   409. No locks, no version columns.

6. **Notifications are transactional rows.** `notifications.rs` writes
   batches inside the caller's transaction (`record_batch`), suppressing
   self-notifications and per-batch `(recipient, event)` duplicates.
   Recipients: assignment → assignee; mentions → active team members parsed
   from `@email` tokens (unknown/non-member addresses resolve to nobody;
   edits notify only newly added mentions); submission → reporter;
   acceptance/rejection → assignee; meaningful status changes → the creator
   when someone else acts. No broker, no async fire-and-forget.

## Rejected alternatives

- A `Resolved` status distinct from `In Review`: two review stages without a
  demonstrated need.
- A `Reopened` status: rejected work returning to In Progress is the same
  actionable state; the resolution field already distinguishes it.
- Per-endpoint CSRF tokens or a notification service: SameSite + origin
  checks and a transactional table cover the threat model.
- A dedicated notification-service crate: one module with two callers does
  not justify a boundary.

## Consequences

- Acceptance semantics are enforced server-side and tested on all engines.
- The notifications table grows with activity; cleanup/retention is future
  work (indexes keep queries fast at current scale: ~30 µs page lists,
  ~22 µs unread counts at 1,000 rows).
- `task_reporter_changed` notifications reference tasks by id only; deleted
  tasks cascade the rows away, and the frontend handles dead links
  gracefully.
