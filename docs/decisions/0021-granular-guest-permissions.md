# ADR-0021: Granular guest permissions (RBAC v2)

Date: 2026-11-14 · Status: accepted · Supersedes the coarse enum in
[ADR-0018](0018-guest-role.md)

## Context

ADR-0018 shipped the team guest role with five coarse permissions
(`view_team`, `view_project`, `view_task`, `comment_task`,
`view_activity`). Real delegation needs finer control: a guest who may
read the board but not individual task dates, or one who may download
attachments but never upload. The coarse model cannot express "see the
task list but not the board" — every combination below the five grains
required giving away more than intended.

## Decision

1. **Nineteen permissions across four domains.** The enum widens to:
   - **Team** — `view_team`, `view_team_members`, `view_team_activity`
   - **Projects** — `view_project_list`, `view_project`,
     `view_project_tasks`, `view_project_timeline`, `view_project_board`
   - **Tasks** — `view_task`, `view_task_own`, `view_task_dates`,
     `view_task_activity`, `view_task_comments`, `comment_task`,
     `view_task_attachments`, `download_task_attachments`,
     `upload_task_attachments`
   - **Administration** — `invite_guest`, `view_audit_log`
2. **Scoping.** Grant rows gain `scope_type` (`team` | `project` | `task`
   | `global`) and `metadata_json`. Today the API grants exactly two
   scopes: team-scoped rows (the six team/administration permissions,
   enforced at grant time) and project-scoped rows (everything else,
   `project_id` required). `task`/`global` are modeled for forward
   compatibility and rejected by the grant endpoint until a surface
   needs them.
3. **Each surface resolves its own permission.** Endpoint → permission:
   `GET /teams/{id}` → `view_team`; `GET /teams/{id}/members` →
   `view_team_members`; `GET /teams/{id}/projects` → `view_project_list`;
   `GET /projects/{id}` → any project-view grant (the detail payload is
   the page shell; the surfaces below enforce their own permission); `GET /projects/{id}/board` →
   `view_project_board`; `GET /projects/{id}/tasks` → `view_project_tasks`
   (or `view_task_own`, which collapses the list to tasks assigned to the
   caller); list with `view=timeline` → `view_project_timeline`;
   `GET /tasks/{id}` → `view_task` (or `view_task_own` + assignee match);
   due dates/deadlines inside any task payload → `view_task_dates`
   (redacted to `null` otherwise); comments list → `view_task_comments`;
   activity → `view_task_activity`; attachments list / download / upload →
   `view_task_attachments` / `download_task_attachments` /
   `upload_task_attachments`. Guests can never delete attachments.
   `view_team_activity` and `view_audit_log` are grantable model-level
   permissions with no server surface yet (documented forward
   compatibility).
4. **`view_task` beats `view_task_own`.** They are mutually exclusive at
   grant time (the drawer disables the second while the first is on);
   `require_task_access` accepts either, so a stale pair cannot widen
   access beyond `view_task`.
5. **`view_task_own` yields an empty set today.** Guests are never
   assignees (`validate_assignee` requires membership), so the permission
   is wired (list collapses to `assignee_id = caller`; detail requires an
   assignee match) but latent until guest task creation ships.
6. **Two-step guest invitations.** `guest_invitations` (team, guest,
   invitee_email, status `pending|approved|rejected`, requested_by,
   decided_by, decided_at). A guest holding the team-scoped `invite_guest`
   grant files a request (`POST /teams/{id}/guests/invite-requests`);
   Owner/Admin list it and decide via `approve`/`reject`. Approval runs
   the same rules as a direct invite (existing instance user, not a
   member) and creates the `team_guests` row. Nothing changes until a
   decision. One pending request per (team, email); deciding twice is a
   409. The manager queue lives in the team settings guests section.
7. **Drawer.** The permission drawer groups toggles by domain in
   accordions, shows a live grant summary, enforces the dependency rules
   client-side (upload implies comment; view_task disables view_task_own),
   and saves transactionally: the add/remove diff is computed first, and a
   failing step compensates (reverts) the already-applied operations in
   reverse so the stored set matches the pre-save state.
8. **Guest card badges** are grouped by domain with the same metadata
   (icon, color, description) as the drawer.

## Migration (0020) — backward compatibility

Legacy rows map so **existing guests keep exactly the access they had**:

| Legacy | Granular expansion |
| --- | --- |
| `view_team` | `view_team` (team scope) |
| `view_project` | `view_project`, `view_project_tasks`, `view_project_board`, `view_project_timeline` |
| `view_task` | `view_task`, `view_task_dates`, `view_task_comments`, `view_task_activity` |
| `comment_task` | `comment_task` |
| `view_activity` | `view_task_activity` |

`view_project` previously reached *every* surface behind
`require_project_access` (detail, board, task list, timeline), so its
expansion covers all four project views. The CHECK constraint widens in
the same migration; old values disappear after the data moves.

## Consequences

- Per-endpoint permission checks mean the matrix is testable one grant at
  a time (`guest_role_matrix` is organized by domain).
- Non-disclosure stays: every denial is a 404 (or 403 for mutations the
  task is visible for), matching ADR-0018.
- Phase 3 project viewers are untouched: their separate grant model keeps
  full read access on their one project (`guest_has_surface` exempts
  them).
- New surfaces must either reuse an existing permission or add one to the
  closed enum (migration required); silent catch-alls are not allowed.
