# ADR-0017: Cross-team isolation audit and defensive URL guard

Date: 2026-11-08 · Status: accepted

## Context

A user report claimed tasks from the first project of the first team
"appeared in the second team". Cross-team isolation is the product's
baseline security model (Phase 1): every resource read must resolve the
owning team and verify membership, returning 404 on mismatch.

## Audit

**Backend (measured, via a full matrix integration test).** Every read
surface was exercised with a single user who is a member of two teams —
the strongest leak witness — plus a second user who is a member of one
team only, asserting 404 on the other team's direct IDs:

| Surface | Scoping mechanism | Result |
|---|---|---|
| Project list | `require_membership(team_id)` | correct |
| Project detail / board / task list | `require_project_access` → membership or project-guest row, else 404 | correct |
| Task detail / update / transition / position | `require_task_access` → project → team | correct |
| Team task search, workload, capacity, dashboard | `require_membership(team_id)` + team_id in WHERE | correct |
| Goals list + associations | `require_membership(team_id)` | correct |
| Trash | `require_membership(team_id)` | correct |
| my-tasks | assignee filter + membership EXISTS clause | correct (assignee-scoped by design) |

The backend had **no leak**: every row-level query either carries
`team_id` or reaches it through `require_project_access`, and the 404
non-disclosure rule held for every cross-team direct-ID attempt.

**Root cause (frontend, confirmed).** The Phase 12.5 team switcher
preserved the current *section* by rewriting the URL — including
`/projects/:projectId`. A project belongs to exactly one team, so
switching from Team A's project page to Team B produced
`/projects/<teamA-project>?team=<B>`: Team A's project, tasks, and board
rendered under Team B's `?team=` — reading precisely as a cross-team
leak. No data ever crossed teams; the *frame* did.

## Decision

1. **`switchTeamLocation` never carries a project path.** Switching from
   a project detail (or any nested view of one) lands on the new team's
   projects list (`/projects?team=<new>`). Team-scoped section paths
   (`/teams/:teamId/...`) continue to rewrite their segment.
2. **ProjectPage defensive guard (ADR-0017).** When the rendered
   project's `team_id` disagrees with the URL's `?team=` (stale bookmark,
   legacy link), the project's own team wins and the URL is corrected
   with a replace-navigation. One team's context can therefore never
   frame another team's project, regardless of how the URL was produced.
3. **Regression coverage.** The `cross_team_isolation_matrix`
   integration test locks the backend matrix; unit tests lock the
   switcher's project-path behavior; the E2E team-switch journey asserts
   Team B never shows Team A's task in any view.

## Consequences

- Switching teams from a project now intentionally lands on the projects
  list rather than "the same page" — the same page does not exist in the
  other team.
- Any future team-scoped surface must add its read path to the matrix
  test.
