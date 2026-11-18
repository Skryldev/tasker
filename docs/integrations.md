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
  `payload` with a storage capacity of 15,000 **characters**
  (MySQL `VARCHAR(15000)`, utf8mb4-safe). This is distinct from the
  Task 2 HTTP intake limit of 15,000 raw **bytes**, enforced on the
  request body *before any buffering*: a UTF-8 body accepted under the
  byte limit can never exceed the column's character capacity, so the
  storage bound cannot be violated by validated intake traffic.

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

## Webhook delivery infrastructure (Task 2)

Provider-neutral durable intake, delivery state machine, and processing
worker. Provider-specific behavior is isolated behind the `ProviderAdapter`
contract (raw-body signature verification + payload normalization); GitHub
and GitLab adapters (Tasks 3–4) implement it without touching the engine.

### Intake endpoint

`POST /api/v1/webhooks/{provider}/{integration_id}` — public (provider-
authenticated), session auth does not apply.

| Situation | Response |
|---|---|
| First valid delivery | `202 {"status":"accepted"}` |
| Duplicate delivery id (any prior state) | `200 {"status":"duplicate"}` |
| Missing/malformed/invalid signature, unverifiable credential state | `401` |
| Malformed request (headers, encoding) | `400` |
| Body beyond 15,000 **bytes** | `413` |
| Non-JSON content type | `415` |
| Unknown provider, unknown/disabled/archived integration, provider mismatch | `404` |
| Database busy | `503` |

Enforcement order: byte limit before buffering (route-scoped body limit +
explicit capped read — never solely `Content-Length`) → provider/integration
resolution (existence masked to a uniform 404) → content type → raw-body
verification (constant-time; unsigned requests are never accepted) →
atomic durable persistence under
`UNIQUE(integration_id, delivery_id)` (the constraint is the correctness
boundary for concurrent duplicates; the pre-check is an efficiency aid).
Rejected requests are never persisted. No payload, signature, or secret
material appears in logs or responses.

### Delivery state machine

```
intake → pending → processing → processed (terminal)
                      │ ├──► unsupported (terminal: outside the vocabulary)
                      │ └──► dead_letter (terminal: permanent / exhausted / expired)
                      └──► retryable_failed ──(backoff)──► claimable again
lease expiry: processing → pending (attempt budget NOT burned)
manual replay: dead_letter → pending (audited, attempts reset)
```

- **Claims** are lease-based (`claim_token`, `worker_id`, `lease_expires_at`);
  the guarded UPDATE makes concurrent claimants exact and portable.
- **Completions/failures are fenced by the claim token**: a worker that lost
  its lease cannot commit anything (its whole transaction rolls back).
- **Retry policy**: exponential backoff with full jitter (base 1s, cap 60s,
  shared with the Rules dispatcher), max 8 attempts, deliveries older than
  the maximum age (7 days) dead-letter as `expired` on their next failure.
  Transient failures (e.g. SQLite single-writer contention) are classified
  retryable; malformed payloads, unavailable adapters, and vanished
  references are permanent.
- **Manual replay** reopens only `dead_letter` deliveries (CAS-guarded,
  concurrent replays converge), resets the retry budget, snapshots the
  pre-replay diagnostics into `webhook_delivery_replays` (who, when, why,
  prior attempts/error), and never reopens `processed`/`unsupported`. The
  management API exposure arrives with the Task 6 work; the service
  boundary (`delivery::replay_dead_letter`, caller's transaction) is stable
  now.

### Processing atomicity

The worker persists the normalized domain event (with `origin =
'integration'`, full provenance, taskless, external actors never become
Tasker users) and the fenced `processed` completion in ONE transaction: a
crash before commit leaves nothing; after commit both exist. Duplicate
executions converge through the `UNIQUE(source_delivery_id)` provenance
index plus a ledger-style pre-check — one domain event per delivery, ever.
Accepted deliveries process even if the integration is later disabled or
archived (they were authenticated at intake); new deliveries are refused.

### Configuration

`TASKER_WEBHOOK_WORKER_{ENABLED,POLL_MS,BATCH,CONCURRENCY,LEASE_MS,
MAX_ATTEMPTS,BACKOFF_BASE_MS,BACKOFF_MAX_MS,MAX_DELIVERY_AGE_MS}` with
sane production defaults (mirroring the Rules dispatcher) and startup
validation of cross-field invariants. Full-jitter is an internal constant,
not a configuration surface.

### Observability

Bounded-cardinality metrics only (no delivery/integration/tenant
identifiers): `tasker_webhook_intake_accepted_total`,
`tasker_webhook_intake_duplicate_total`,
`tasker_webhook_intake_rejected_total{reason}` (closed reason set),
`tasker_webhook_deliveries_{claimed,processed,already_processed,
unsupported,retryable_failures,dead_lettered,replayed}_total`,
`tasker_webhook_leases_recovered_total`,
`tasker_webhook_stale_rejections_total`, and
`tasker_webhook_processing_duration_seconds`. Structured logs carry
correlation ids and bounded context, never payloads or secrets.

### Measured throughput (SQLite temp file, in-process, 200 deliveries)

- Intake (HMAC verification + durable insert + ack): **≈1,900/s**.
- Worker (sequential; single-writer engine): **≈1,950/s**, 200/200 with zero
  contention retries. Concurrency 4 on SQLite converges through the retry
  policy (no losses) but is dominated by single-writer lock contention;
  concurrent numbers on PostgreSQL/MySQL arrive with the final Phase 8
  cross-database CI gate.

## GitHub adapter (Task 3)

`src/integrations/github.rs` implements the `ProviderAdapter` contract; the
intake resolves it through the production registry (`production_registry()`).

- **Verification**: `X-Hub-Signature-256` = `sha256=<hex>` of HMAC-SHA256
  over the exact raw body, constant-time comparison against the
  integration's decrypted webhook credential. Missing/malformed/invalid
  signatures are distinct internal classes, one indistinguishable 401.
- **Identity**: `X-GitHub-Delivery` + `X-GitHub-Event`, bounded before
  persistence.
- **Closed classification registry** (current vocabulary):

| GitHub event | Vocabulary | Notes |
|---|---|---|
| `push` (refs/heads/*) | `git_push` | `branch`, `commit_count`, `head_sha` |
| `push` (refs/tags/*) | — | durably `unsupported` |
| `push` (deleted) | — | durably `unsupported` (the `delete` event covers it) |
| `pull_request` opened/reopened | `git_pull_request_opened` | |
| `pull_request` closed (merged) | `git_pull_request_merged` | close and merge are never conflated |
| `pull_request` closed (not merged) | `git_pull_request_closed` | |
| `pull_request` other actions | — | durably `unsupported` (vocabulary extension is a controlled schema change) |
| `create` / `delete` (branch) | `git_branch_created` / `git_branch_deleted` | tag refs stay unsupported |
| `release` published | `git_release_published` | `release_tag`, `release_name`, `prerelease` |
| everything else (`ping`, `issues`, `workflow_run`, `check_run`, …) | — | durably `unsupported` |

- **Normalization**: bounded state snapshots using the `integration.*`
  condition-namespace keys (`provider`, `event_type`, `branch`,
  `repository`, `actor`, `commit_count`) plus bounded event fields
  (`head_sha`, `pr_number`, `pr_title`, `pr_state`, `pr_merged`,
  `pr_author`, `base_branch`, `release_tag`, `release_name`, `prerelease`).
  Strings truncated to the 255-char scalar bound; timestamps validated
  (implausible values replaced with processing time); external actors stay
  external identities.
- **Setup**: the webhook secret is the integration's credential
  (provision/rotate/revoke lifecycle from Task 1; management UI in Task 6).

## Later tasks (8B)

- Task 4: GitLab adapter (secret-token verification).
- Task 5: core delivery — shared evaluation core, execution-context and
  action-compatibility decisions (nullable execution/outbox task ids).
- Task 6: integration management API + OpenAPI (and intake-adjacent
  operational endpoints such as delivery inspection and replay).
- Task 5: core delivery — shared evaluation core, execution-context and
  action-compatibility decisions (nullable execution/outbox task ids).
- Task 6: integration management API + OpenAPI (and intake-adjacent
  operational endpoints such as delivery inspection and replay).
