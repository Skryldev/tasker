# Git Integrations (Phase 8B)

Provider-neutral Git integration for GitHub and GitLab. Built on the Phase
8A Rules Engine: integrations receive webhook deliveries, verify them, and
normalize them into Tasker domain events that the existing pipeline
evaluates.

## Foundation (Task 1)

- **Integrations** are team-scoped provider connections (`github`/`gitlab`),
  created by Owner/Admin (`Capability::ManageIntegrations`). Names are
  unique per team; archived integrations are read-only.
- **Credentials** — one active webhook secret per integration, stored
  AES-256-GCM-encrypted (Phase 7 envelope, distinct
  `TASKER_INTEGRATIONS_KEY`). Lifecycle: provision (v1) → rotate (version
  CAS, never reactivates revoked) → revoke (verification fails closed) →
  re-provision (explicit, clears revocation). Plaintext never persisted,
  logged, or returned.
- **Repositories** — external repos an integration listens to, validated
  ids, unique per integration.
- **Project mappings** — M:N repo→project routing with
  **database-enforced tenant isolation**: composite foreign keys
  `(repository_id, team_id)` and `(project_id, team_id)` force both parents
  to the mapping row's `team_id` — cross-team mappings are unrepresentable.
- **Webhook deliveries** — durable intake records (schema in Task 1;
  lifecycle in Task 2): unique `(integration_id, delivery_id)` for replay
  protection, status machine
  `pending → processing → processed | retryable_failed → … | dead_letter`,
  claim/lease/fencing fields, bounded `last_error_*` diagnostics,
  `payload` capped at 15,000 chars (MySQL `VARCHAR(15000)`, utf8mb4-safe).

## Event vocabulary

Integration events share the `domain_events` store with `origin =
'integration'` and the trigger vocabulary `git_push`,
`git_pull_request_opened/merged/closed`, `git_branch_created/deleted`,
`git_release_published`. Provenance columns (`integration_id`,
`source_delivery_id`) trace every integration event to its integration and
delivery; the unique delivery index prevents duplicate domain events.

## Condition namespace

Integration rules reference `integration.*` fields only
(`provider`, `event_type`, `branch`, `repository`, `actor`,
`commit_count`); task rules reference `task.*`/`event.*` fields only.
Namespace validation rejects mixed conditions at write time. Scalar values
are bounded text (≤ 255 chars) or `null`.

## Later tasks (8B)

- Task 2: webhook intake endpoint, delivery worker, claim/lease/retry.
- Task 3: GitHub adapter (HMAC-SHA256 signature verification).
- Task 4: GitLab adapter (secret-token verification).
- Task 5: core delivery — shared evaluation core, execution-context and
  action-compatibility decisions (nullable execution/outbox task ids).
