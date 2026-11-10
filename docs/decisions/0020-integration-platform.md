# ADR-0020: Provider-neutral Git integration platform

Date: 2026-11-09 · Status: accepted

## Context

Phase 8B adds Git provider integration (GitHub, GitLab) to Tasker. The
platform must receive webhook deliveries, verify them, persist them
durably, normalize them into Tasker events, and deliver them to the
existing Rules Engine — without weakening Phase 8A guarantees or letting
provider concepts leak into the core.

## Decision

1. **Shared Event Store.** Integration events are persisted in the same
   `domain_events` table as task events, with `origin = 'integration'` and
   the new event vocabulary (`git_push`, `git_pull_request_*`,
   `git_branch_*`, `git_release_published`). No parallel event store.
   Provenance columns (`integration_id`, `source_delivery_id`) are nullable
   on `domain_events` and required (non-NULL) for integration events; the
   unique index on `source_delivery_id` guarantees a delivery never
   generates duplicate domain events.
2. **One Rules Engine.** Integration rules use the same `rules`,
   `rule_versions`, `rule_executions`, and outbox machinery. The trigger
   vocabulary gains integration values; the condition DSL gains a closed
   `integration.*` field namespace (`provider`, `event_type`, `branch`,
   `repository`, `actor`, `commit_count`) with `Scalar::Text` operands
   (≤ 255 chars). Namespace validation rejects mixed conditions at write
   time. The evaluation-core extraction (task/integration context variants)
   lands in Task 5 with the second caller.
3. **Origin policy.** `should_evaluate`: `User` → evaluate, `Automation` →
   skip (loop prevention), `Integration` → evaluate. Provider logins are
   stored as `external_actor` in the normalized payload — never written to
   `users`, never FK'd, never a session.
4. **Tenant isolation is database-enforced.** `integrations` and `projects`
   carry `UNIQUE(id, team_id)` anchors; `integration_repositories` and
   `integration_repository_projects` carry `team_id` with composite foreign
   keys — a cross-team mapping is rejected by the database.
5. **Credentials.** One active credential per integration
   (`UNIQUE(integration_id)`); webhook secrets stored AES-256-GCM encrypted
   (Phase 7 envelope, distinct `TASKER_INTEGRATIONS_KEY`); rotation
   replaces the ciphertext in place under a version CAS and never
   reactivates a revoked credential; revocation is explicit; re-provisioning
   is a separate guarded operation. Plaintext never persisted, logged, or
   returned.
6. **Payload bounds.** Webhook payloads capped at 15,000 characters
   (413 above), stored `VARCHAR(15000)` on MySQL (utf8mb4-safe row-size
   arithmetic in docs/integrations.md) — TEXT/MEDIUMTEXT rejected (sqlx Any
   BLOB decode trap).

## Consequences

- Task 1 ships the schema (0015/0016), domain types, credential lifecycle,
  capability (`ManageIntegrations`), and tests — no intake/worker/adapters.
- Tasks 2–5 build intake, adapters, and core delivery on these contracts.
- The SQLite rebuilds in 0017 are the riskiest migration step (copy → drop →
  rename under `PRAGMA foreign_keys = OFF`); crash windows documented in the
  migration header.
