# ADR-0018: Team guest role — explicit grants, deny by default

Date: 2026-11-09 · Status: accepted

## Context

Access control had two tiers: **team members** (Owner/Admin/Manager/Member —
a role matrix) and **Phase 3 project viewers** (non-members granted
read-only access to ONE project). There was no way to give an outside
person a durable, *curated* presence in a team: a viewer sees exactly one
project and nothing else; a member inherits a full role matrix. Reports
asked for a **Guest** role: non-members whose every capability is granted
explicitly, per-team, revocable instantly.

## Decision

1. **Team-scoped registration.** `team_guests (id, team_id, user_id,
   granted_by, created_at, revoked_at)` — unique per (team, user). A user
   may be a guest of many teams; a member cannot also be a guest of the
   same team (membership supersedes — enforced at invite time).
2. **Explicit permission rows.** `team_guest_permissions (id, guest_id,
   permission, scope_type, scope_id, granted_by, created_at)` — closed
   enum:
   - `view_team` (team scope): team name/description only.
   - `view_project` (project scope): project detail, board, task list.
   - `view_task` (project scope): individual task detail.
   - `comment_task` (project scope): POST comments.
   - `view_activity` (project scope): the whitelisted workflow activity
     feed (the Phase 10.5 guest event filter still applies).
   **Deny by default.** A guest with zero rows sees nothing. No
   capability comes from the role matrix (`Role::Guest.can(_) == false`).
3. **404 non-disclosure.** Guest requests without the matching grant fail
   with 404 — indistinguishable from a stranger's request (same rule as
   Phase 1 cross-team).
4. **Authorization wiring (audited).** `require_project_access` resolves
   members first, then Phase 3 viewers, then team guests with
   `view_project`; `require_task_access` additionally demands
   `view_task`; `create_comment` demands `comment_task`; task activity
   demands `view_activity`; `get_team` demands `view_team`. Every other
   endpoint keeps `require_membership`, which guests fail → 404. Mutation
   endpoints (create/edit/delete anything) stay behind membership
   capabilities that guests do not have.
5. **Revocation is immediate.** Deleting the guest row (soft, with
   `revoked_at`) or any permission row takes effect on the next request —
   no cached grants.
6. **Coexistence.** Phase 3 project viewers keep their path
   (`project_members`); the guest check runs only when the caller is not
   a member and not a project viewer, and the comment/activity gates
   explicitly distinguish the two so viewers never gain new powers.
7. **Identity model.** Guests are instance users (ADR-0014): invites
   select an existing account; no guest-only credential path exists.

## Consequences

- Management surface: `GET/POST /teams/{id}/guests2`,
  `DELETE /teams/{id}/guests2/{guest_id}`, `GET/POST
  /teams/{id}/guests2/{guest_id}/permissions`,
  `DELETE .../permissions/{permission_id}` — Owner/Admin only
  (non-managers get 404 on the surface itself). The `guests2` suffix
  avoids colliding with the Phase 3 `guests` routes.
- The switcher lists guest teams with `my_role: "guest"`; the topbar
  shows a Guest badge; the project page's existing guest-read-only
  presentation (Phase 3) covers the restricted UI.
- `guests2` will be renamed to `guests` for v2 of the API; for now the
  Phase 3 routes are load-bearing.
