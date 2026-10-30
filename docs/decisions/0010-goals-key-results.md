# ADR-0010: Team-level Goals & Key Results

Date: 2026-10-30 · Status: accepted

## Context

Tasker tracked execution (projects, tasks, workflow) but offered no way to
express desired business outcomes. Teams need objectives with measurable key
results and the ability to associate contributing work — without adopting an
enterprise strategy-management suite, a metric-formula engine, or a generic
planning layer.

## Decision

### Domain

- A **Goal** belongs to exactly one team, has a lifecycle
  (`draft` → `active` → `completed`/`cancelled`, with explicit reopen to
  `active`) and an owner (a validated team member).
- A **Key Result** belongs to one goal and is one of three metric types:
  `increase`, `decrease`, or `boolean`. Progress is computed server-side by
  one pure function (`goals::kr_progress`): the standard linear formulas
  clamped to 0–100; boolean KRs are 0% or 100% with only 0/1 as legal
  current values. Invalid configurations (target ≤ start for increase,
  target ≥ start for decrease, missing bounds, non-finite values) are
  rejected at write time — never silently hidden.
- **Overall goal progress** is the arithmetic mean of key-result progress;
  a goal without key results has undefined progress (JSON `null`), not an
  invented 0%.
- **Activation requires at least one key result** — a goal must be
  measurable before it goes live (enforced in `transition_goal`).
- Reaching 100% key-result progress never auto-completes a goal: the
  lifecycle transition is always an explicit, authorized action.

### Measured outcomes vs. task execution

Goal↔project and goal↔task associations are pure many-to-many links
(cascade on either side's deletion; projects and tasks themselves survive
goal deletion). The goal detail exposes a clearly separated **task
execution summary** (total/associated/completed counts from one GROUP BY
query) labeled as execution progress, not goal achievement.

### Authorization

A new `ManageGoals` capability follows the existing matrix pattern: Owner,
Admin, Manager manage goals; Members and above view them; guests (who hold
no team membership) receive the standard 404 for every goal endpoint —
verified in the integration matrix. Cross-team associations and cross-team
owner assignments are rejected server-side.

### Activity integration

The existing `activities` table gains a nullable `goal_id` column; goal
events are written through the same transactional mechanism with
`task_id` NULL, leaving task history and the guest activity whitelist
untouched. A `GET /goals/{id}/activity` endpoint exposes goal history.

## Rejected alternatives

- Storing progress as a client-editable field — invites drift between
  metric values and reported progress.
- Zero-denominator or no-KR goals defaulting to 0% — hides "not measurable
  yet" behind a number.
- A separate goal-activity table — duplicates the existing mechanism for no
  additional capability.
- Weighted key results — defer until a real need exists; the mean is simple
  and deterministic.

## Consequences

- Key-result values are `DOUBLE/REAL` across engines; equality-sensitive
  float logic is centralized in `kr_progress` and unit-tested with
  exactly representable results.
- `activities.goal_id` is indexed (`goal_id, created_at DESC`) for the
  history endpoint; goal lists do one extra per-goal KR fetch (bounded page
  size, indexed `goal_id` lookup) — no N+1 across teams because listing is
  team-scoped and paginated.
