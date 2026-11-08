# ADR-0014: Instance-wide users; team membership is a separate assignment

Date: 2026-11-08 · Status: accepted

## Context

Since Phase 1, `POST /teams/{id}/members` created a Tasker account *and*
attached it to the team in one step (with an administrator-chosen temporary
password). This had four compounding problems:

- **Accounts were team appendages.** A user created in Team A could not be
  referenced by Team B without being re-created — producing duplicate
  identities with the same email blocked by the instance-wide UNIQUE
  constraint, or half-shared via manual member re-adds.
- **Account creation authority was wrong.** Whoever could add a member to
  any team (every team Admin) could mint instance citizens, set their
  display identity, and hand out credentials — an instance-level power
  exercised through a team-scoped endpoint.
- **The temporary-password flow bypassed the password policy** (until
  Phase 12.5 Bug 8) and had no lifecycle: nothing prompted the recipient
  to rotate the credential.
- The invite-by-email feature the field comments anticipated never
  shipped; the temporary password carried all the risk with none of the
  workflow.

## Decision

1. **Tasker accounts are instance citizens.** They are created exactly one
   way: `POST /admin/users`, Owner-only (instance role `owner`). The
   payload is `{email, display_name, username?, temporary_password}`; the
   temporary password must satisfy the strict password policy (ADR-0016)
   and the generated default from the Admin UI always does.
2. **Team membership is a separate action.** `POST /teams/{id}/members`
   now only attaches an existing, active account (`{email, role}`). An
   unknown email returns 422 with the actionable message *"No Tasker user
   with this email. Ask an Owner to create the account first."* The
   `display_name` and `temp_password` request fields are gone; serde
   ignores them for tolerance with older clients.
3. **Instance role.** `users.instance_role ∈ {owner, member}`. Only
   owners can list, create, update, deactivate or delete accounts. Owners
   are minted exclusively by `bootstrap-admin` (first-setup) — the API
   refuses to grant `owner`. An owner cannot deactivate, demote, or
   delete their own account, so an instance can never strand itself
   without an owner.
4. **Existing data needs no migration of accounts**, only of authority:
   migration 0015 adds the column and promotes the *earliest team owner*
   as the instance owner — deterministically the bootstrap administrator
   in every deployment we know of. The heuristic is documented in the
   migration; a future `bootstrap-admin --promote <email>` would make it
   explicit (out of scope here).
5. **Hard delete is guarded.** `DELETE /admin/users/{id}` without a body
   soft-deactivates (reversible, sessions revoked). With
   `confirm_email` + the requesting owner's `current_password` a hard
   delete is attempted, refused with 409 while the account still has team
   memberships, created tasks, or comments — the FK graph has NOT NULL
   creator columns, so blind deletion would either cascade into data loss
   or violate constraints depending on engine.

## Consequences

- Admin UI: a new Owner-only **Users** page (searchable, paginated, with
  create/deactivate/hard-delete and a generated strong temporary
  password). Team Settings → Members becomes a searchable picker over
  eligible instance users (`GET /teams/{id}/members/eligible`), with the
  "Ask an Owner to create this account" empty state.
- The public-registration endpoint (disabled by default) is unchanged but
  now also runs the identity-aware password policy.
- Multi-team membership becomes a first-class flow: one account, N
  teams, verified by integration tests.
- Backwards compatibility: previously created users keep working (they
  were always instance rows); only the creation *behavior* changes.
