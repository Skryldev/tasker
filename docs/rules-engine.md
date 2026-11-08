# Rules Engine (Phase 8A)

The Rules Engine adds automation to Tasker: team-scoped rules that react to
committed task-state changes, evaluate bounded declarative conditions, and
plan actions. It lives inside the existing modular monolith
(`backend/src/rules/`) and never touches authentication, session, or MFA
internals. This document covers the domain model, condition DSL, and the database
schema (Tasks 1-2); outbox worker, and API sections are appended by
their tasks.

## Module map

```
src/rules/
├── mod.rs         Rule aggregate, RuleDefinition, RuleError, input validation
├── limits.rs      every engine limit, centralized (character counts)
├── conditions.rs  typed condition DSL: strict parse → validation → pure evaluator
├── actions.rs     typed action model (add_comment, send_notification, send_message)
└── events.rs      domain events, trigger matrix, loop-prevention policy
```

Dependency direction: `engine (Task 3) → conditions/actions/events/outbox`;
nothing outside `rules/` depends on rules internals. The DSL and evaluator
have **zero database dependencies** and are unit-tested in isolation.

## Condition DSL

Strictly typed, closed vocabulary. Unknown fields, unknown operators,
unknown keys, invalid enum values, malformed UUIDs, wrong operand shapes,
oversized and over-deep trees are **validation errors**, never silent
`false`. No eval, scripting, SQL fragments, or user code.

JSON grammar (shape-keyed; each variant's keys are its discriminator):

```json
{"all": [ <condition>, ... ]}
{"any": [ <condition>, ... ]}
{"not": <condition>}
{"field": "task.status", "op": "EQ", "value": "todo"}
{"field": "task.status", "op": "IN", "values": ["todo", "in_progress"]}
```

### Field registry (closed)

| Field | Kind |
|---|---|
| `task.status` | status enum |
| `task.priority` | priority enum |
| `task.assignee_id` | UUID, nullable |
| `task.reporter_id` | UUID, nullable |
| `event.actor_id` | UUID, nullable |
| `event.previous_status` / `event.new_status` | status enum, nullable |
| `event.previous_priority` / `event.new_priority` | priority enum, nullable |

Enum literals must be members of `tasks::STATUSES` / `tasks::PRIORITIES`
(`backlog/todo/in_progress/resolved/in_review/closed`,
`low/medium/high/urgent`). UUID literals are normalized to canonical
lowercase form. There are no numeric fields, hence no numeric operators.

### Comparison semantics (total two-valued logic)

| Operator | Present field | Missing field |
|---|---|---|
| `EQ x` | canonical equality | `false` (except `EQ null` → `true`) |
| `NEQ x` | negation of `EQ` | `true` (`NEQ null` → `false`) |
| `IN [..]` | membership | `false` |
| `NOT_IN [..]` | negation of membership | `true` |

No implicit coercion: a status field compared against a UUID-shaped value is
a validation error. Equality inside a closed enum domain is canonical string
equality — both sides are constrained to the same vocabulary (DB CHECK
constraints keep stored values canonical), so this is typed equality, not
free-form string matching.

### Limits (all server-side, centralized in `limits.rs`)

| Limit | Value |
|---|---:|
| Rule name / description | 120 / 500 chars |
| Serialized condition JSON | 4,000 chars |
| Serialized actions JSON | 2,000 chars |
| Condition tree depth / nodes | 10 / 100 |
| Actions per rule | 5 |
| `IN` list entries | 16 |
| Automation comment/message body | 2,000 chars |
| Opaque connection/destination refs | 64 chars |

Character counts (Unicode scalar values), matching repo convention; MySQL
columns are sized in migration 0009 at 4 bytes/char worst case (utf8mb4).
Serialized-size gates run **before** parsing, so oversized input never
reaches the deserializer; `serde_json`'s 128-level recursion limit bounds
parser stack depth. Note the composition rule: the per-item body cap and the
aggregate serialized cap interact — a single action with a maximum-length
body exceeds the aggregate cap and is rejected by the gate (documented in
tests).

## Action model

- **`add_comment`** — literal body (no templating in Phase 8A); authored by
  the dedicated Automation Principal (Task 4).
- **`send_notification`** — recipients: team-scoped selectors (`assignee`,
  `reporter`) and/or explicit user ids (≤10), re-verified as team members at
  execution; departed members are skipped, not failed.
- **`send_message`** — provider-neutral delivery intent: opaque
  `connection_ref`/`destination_ref` (pattern `[A-Za-z0-9._-]{1,64}`), format
  (`plain_text`, `markdown_like`), literal body. No URLs exist anywhere in
  the contract, which makes the delivery path SSRF-free by construction. No
  provider, protocol, or transport concepts appear in the model. Phase 8A
  persists intents only; delivery belongs to a future integration service.

Execution order is declaration order; planning is deterministic.

## Domain events and triggers

A trigger is an actual committed change. The classification helpers live in
`events.rs` (`assignment_delta`, `value_changed`) and are wired to the real
mutation paths in Task 3:

| Mutation | Event |
|---|---|
| Task created **with** an initial assignee | `task_assigned` |
| Task created without an assignee | — |
| Status change (transition endpoint only) | `task_status_changed` |
| Priority change (PATCH) | `task_priority_changed` |
| Unassigned → assigned / assignee A → B | `task_assigned` |
| Assigned → unassigned | — |
| Team-member removal auto-unassignment | — |
| No-op update | — |

Events carry UUIDv7 id, type, schema version, team/task/actor ids,
trigger-relevant previous/new state, origin (`user`/`automation`), causing
execution id (automation events), and occurrence time — never task
descriptions, comments, or credentials.

## Loop prevention

Rules evaluate **only** `user`-origin events (`events::should_evaluate`).
Phase 8A's internal actions do not mutate task state, so automation-origin
events cannot arise; the policy is enforced at selection so future triggers
inherit it. Automation-origin events still record `causing_execution_id` for
audit. This conservative policy also ignores events generated by other
rules; loosening it later is a one-function change with a defined audit
chain.

## Transaction boundary (accepted architecture)

Event capture, rule selection, evaluation, execution records, and action
intents all commit inside the originating task-mutation transaction
("Option A"). Evaluation is pure and bounded, so the transaction stays short;
no network I/O ever occurs inside it. A failure rolls back the task mutation,
event, and intents atomically.

---

## Schema (Task 2 — migration `0009_rules`)

Six tables, one forward-only migration per dialect
(`migrations/{sqlite,postgres,mysql}/0009_rules.sql`):

```
teams ──< rules ──< rule_versions
             |              ^
             |              |  composite FK (rule_id, version)
             +──< rule_executions ──< rule_action_results
             |        |                     ^
             |        |                     | UNIQUE(outbox_id)
             |        +──< automation_outbox
             |                  ^
             +──< domain_events (event_id FK; causing_execution_id back-reference)
```

- **`rules`** — current aggregate: team, name (UNIQUE per team, archived
  names stay reserved), description, trigger, `enabled`, `current_version`,
  current condition/actions JSON, creator, timestamps, `archived_at`.
- **`rule_versions`** — immutable definitions, PK `(rule_id, version)`:
  trigger + condition + actions JSON + version author + created_at. History
  is never rewritten; executions and outbox rows cite exact versions through
  composite foreign keys.
- **`domain_events`** — the committed-change record: type, schema version,
  team, task, nullable actor, nullable previous/new trigger-relevant state
  (≤500 chars each), origin (`user`/`automation`), nullable
  `causing_execution_id`, occurred_at.
- **`rule_executions`** — one row per (rule, event) — `UNIQUE(rule_id,
  event_id)` makes duplicate processing impossible — with
  `condition_result`, status (`planned`/`completed`/`failed`), the planned
  actions summary (≤1000 chars), error category/detail, and the composite FK
  `(rule_id, rule_version) → rule_versions`.
- **`automation_outbox`** — delivery intents: action type, serialized
  payload (≤4000 chars), lifecycle state
  (`pending`/`processing`/`delivered`/`retryable_failed`/`dead_letter`),
  `attempts`, `next_attempt_at`, claim fencing (`claim_token`, `worker_id`,
  `claimed_at`, `lease_expires_at`), bounded error metadata, a deterministic
  `idempotency_key` (64 hex, UNIQUE), and the same composite version FK.
  A CHECK enforces the fencing invariant at the schema level: only a row in
  `processing` may carry a claim token.
- **`rule_action_results`** — the idempotency ledger for internal action
  effects: `UNIQUE(outbox_id)` (one result per intent, insert-before-side-
  effect in the executor transaction, Task 4), outcome
  (`completed`/`skipped`), nullable recipient/author ids, body hash, bounded
  detail.

### Foreign-key policy

`ON DELETE RESTRICT ON UPDATE RESTRICT` on every reference (users and teams
are never hard-deleted in this application). **`task_id` on events,
executions, and outbox rows has NO foreign key to `tasks`**: task deletion is
a hard delete, and automation history is immutable — task ids are recorded
historical identifiers, never dangling references. The
`domain_events.causing_execution_id ⇄ rule_executions.event_id` cycle is
closed with `ALTER TABLE ... ADD CONSTRAINT` on PostgreSQL/MySQL (forward
references are illegal there); SQLite resolves FKs at runtime, so it is
declared inline.

### Boolean handling through the Any driver (important)

sqlx's Any driver cannot describe MySQL `TINYINT` at all (no
`AnyTypeInfoKind` mapping — any `SELECT` of a raw BOOLEAN column fails on
MySQL), and SQLite `INTEGER` decodes only as `BIGINT`. The repository's
established pattern (comments.rs) is therefore: keep idiomatic `BOOLEAN`
columns, never list one in a SELECT, and read it as
`CASE WHEN col THEN 1 ELSE 0 END` → `i64` (any integer kind widens to
`i64`). Rules Engine queries follow this pattern; the schema test pins it.

### Indexes and the queries they serve

| Index | Query pattern |
|---|---|
| `rules (trigger_type, enabled)` | event-time candidate selection (Task 3) |
| `rules UNIQUE (team_id, name)` | team-scoped listing + name reservation |
| `domain_events (task_id, occurred_at)` | per-task event history |
| `domain_events (team_id, occurred_at)` | team-scoped event scans |
| `domain_events (causing_execution_id)` | recursion audit chain |
| `rule_executions UNIQUE (rule_id, event_id)` | duplicate prevention + rule-scoped lookups |
| `rule_executions (event_id)` | executions for an event |
| `rule_executions (team_id, created_at)` / `(task_id, created_at)` | history queries |
| `automation_outbox (status, next_attempt_at)` | worker claim query (Task 5) |
| `automation_outbox (status, lease_expires_at)` | stale-claim reaper (Task 5) |
| `automation_outbox UNIQUE (idempotency_key)` | intent dedup + future provider idempotency |
| `rule_action_results UNIQUE (outbox_id)` | idempotency ledger |
| `rule_action_results (execution_id)` | action results per execution |

### MySQL 8.4 row-size assessment (corrected arithmetic)

`CHAR(36)` = 144 B; `VARCHAR(n)` = 4n + 1/2 B (utf8mb4 worst case, length
prefix included); `BIGINT` = 8 B; `BOOLEAN` = TINYINT = 1 B. InnoDB row limit
is 65,535 B.

| Table | UUID cols | Total | Margin |
|---|---:|---:|---:|
| `rules` | 3 | 27,083 B | 58.7% free |
| `rule_versions` | 2 | 25,138 B | 61.6% free |
| `domain_events` | 5 | 7,034 B | 89.3% free |
| `rule_executions` | 4 | 10,872 B | 83.4% free |
| `automation_outbox` | 5 + claim token | 19,812 B | 69.8% free |
| `rule_action_results` | 5 | 3,242 B | 95.1% free |

Largest index key: `idempotency_key CHAR(64)` = 256 B ≪ InnoDB's 3,072 B
key limit; composite FK keys are 152 B; no payload column is indexed. The
assessment is worst-case in-row (DYNAMIC row format may spill long VARCHARs
off-page with 20-byte pointers, so real storage is smaller). The practical
validation is migration 0009 executing on real MySQL 8.4 (CI) — oversized
rows fail loudly at `CREATE TABLE`.

### Engine differences

- MySQL: named `fk_`/`chk_`/`uq_` constraints (repo convention), `CHAR(36)`
  ids, `VARCHAR(n)` payload columns sized from `src/rules/limits.rs`
  (sqlx Any reports MySQL TEXT as BLOB — undecodable as String), cycle FK
  via ALTER.
- PostgreSQL: TEXT payloads (application-bounded), BOOLEAN, cycle FK via
  ALTER.
- SQLite: TEXT/INTEGER, inline forward reference for the cycle FK.

### Validation results

- SQLite: full migration sequence + the complete constraint/decoding suite
  (`rules_schema_migrations_and_constraints`) passes locally.
- PostgreSQL 17 / MySQL 8.4: same test is env-gated
  (`TASKER_TEST_PG_URL` / `TASKER_TEST_MYSQL_URL`) and runs in CI; not
  executed locally (no Docker; no local PG/MySQL services).

---

## Event capture & transactional evaluation (Task 3)

`rules::engine::capture_and_evaluate` runs **inside the originating task
mutation's transaction**, after the mutation and its activity/notification
writes, before the caller commits:

1. **Event persistence** — one `domain_events` row per supported change,
   with previous/new trigger-relevant state built from server-side
   authoritative values (never raw request fields) and `origin` hardcoded to
   `user` by the handlers — the client has no origin field to forge.
2. **Origin policy** — `should_evaluate` (Task 1) gates evaluation:
   user-origin events only. Automation-origin events are still recorded for
   audit (with `causing_execution_id` once automation exists).
3. **Selection + version loading** — one indexed query joins `rules` with
   `rule_versions` on `current_version`, filtered by trigger + `enabled` +
   team. Rule edits are compare-and-swap in their own transaction, so the
   selected version is transactionally consistent by construction; no N+1.
4. **Evaluation** — the stored condition is re-parsed (bounded by the Task 1
   caps) and evaluated with the pure evaluator. A stored definition that
   fails validation is a data-integrity violation: the error propagates and
   the whole transaction rolls back — corrupted rules never silently skip.
5. **Execution records** — one row per matching rule, `condition_result =
   'matched'`, `status = 'planned'`, a bounded `planned_actions` summary
   (`[{"index":0,"action_type":"add_comment"}, ...]`), and the exact
   `rule_version` cited. `UNIQUE(rule_id, event_id)` + conflict-clause
   `DO NOTHING`/`ON DUPLICATE KEY UPDATE id=id` (dialect-branched) makes
   re-processing the same logical event a no-op; `rows_affected == 0`
   short-circuits the intents.
6. **Action intents** — one `automation_outbox` row per configured action in
   declaration order, `payload` = the canonical Task 1 action JSON
   (provider-neutral for `send_message`; no URLs exist in the model),
   `idempotency_key` = SHA-256 of `event_id|rule_id|version|action_index`,
   `status='pending'`, `attempts=0` — eligible for the Task 5 worker.

### Wired mutation paths

| Path | Event | Notes |
|---|---|---|
| `create_task_inner` (assignee set) | `task_assigned` (initial) | no event without an assignee |
| `update_task` (PATCH) | `task_priority_changed` / `task_assigned` | from the server-side diff; unassignment and no-ops emit nothing |
| `apply_status_transition` (via `run_transition`) | `task_status_changed` | prev/new from the guarded compare-and-update |

**Intentionally excluded** (no supported trigger): comment creation,
`delete_task`, team-member-removal unassignment (`teams.rs` — an
unassignment is not an assignment), due-date/reporter/label/title changes.
Rejected or unauthorized mutations never reach capture (it runs after
validation/authorization, before commit).

### Atomicity

Any failure — event insert, selection, version parse, execution insert,
intent insert — propagates through `?` and drops the transaction: the task
mutation, event, executions, and intents commit together or not at all.
Failure injection in `rules_capture_atomicity_on_failure` (raw-inserted
corrupt condition / oversized action list) proves 500 + full rollback of all
four artifact classes, with a healthy-rule control.

### Concurrency

Duplicate protection is constraint-driven (`UNIQUE(rule_id, event_id)`,
`UNIQUE(idempotency_key)`), not lock-based. Concurrent transitions of the
same task are serialized by the existing status compare-and-update
(ADR-0011); the loser 409s and emits nothing (tested). Rule tables are
team-scoped and indexed; per-event evaluation is bounded by the rule count
of one team and the Task 1 complexity caps.

---

## Internal actions, Automation Principal & idempotency ledger (Task 4)

### Automation Execution Principal

- **Identity**: fixed `users` row, id `00000000-0000-0000-0000-000000000001`,
  email `automation@tasker.local`, display name "Tasker Automation",
  `password_hash` = a deliberately invalid PHC string (Argon2 parse fails →
  no password verifies → no interactive session possible).
- **Provisioning** (`rules::actions::provision_principal`): idempotent
  upsert keyed on the fixed id, lazily inside the execution transaction. A
  foreign row holding the reserved email fails closed with
  `principal_email_conflict` — never adopted.
- **Reservation**: `util::validate_email` (the single choke point used by
  registration, admin provisioning, and bootstrap) rejects the address.
- **No privileges**: no `team_members` rows; every capability check denies
  it. Permission comes only from the executor's explicit scope policy.

### Internal action execution (`rules::outbox::execute_internal_action`)

Minimal primitive called with a transaction + intent id (the Task 5 worker
claims, then calls this). Pipeline inside ONE transaction:

1. load the intent with its full scope chain and **validate cross-references**
   (intent ↔ execution ↔ event ↔ rule agree on task, team, and rule version —
   corrupted references fail loudly),
2. ledger pre-check (`rule_action_results.outbox_id`) → `AlreadyExecuted`,
3. re-validate the stored payload against the Task 1 model (type must match
   the intent's action type),
4. classify: `send_message` → `ExternalPending` (intent untouched, stays
   `pending` for the Task 5 dispatcher; **no** delivery result recorded),
5. revalidate the task: hard-deleted → durable `skipped(task_deleted)`
   result; archived project → `skipped(project_archived)`; no effect either way,
6. execute the internal effect — `add_comment` via the narrow
   `comments::create_automation_comment` path (same body validation, activity
   record, and mention notifications as the interactive path; author =
   principal; never guest-visible) — `send_notification` via the existing
   `notifications::record_batch` (selectors resolved against current task
   state; explicit users revalidated as team members; ineligible recipients
   skipped and counted),
7. insert the ledger row (`UNIQUE(outbox_id)`) and transition the outbox row
   to `delivered` guarded on `status IN ('pending','processing')`, and commit.

Exactly-once internal effect = effect + ledger entry + outbox completion in
one transaction. Concurrent duplicates: one commits, the loser rolls back
(loud unique violation or the pre-check); constraints are the defense — no
process-local state.

---

## Outbox dispatcher & worker (Task 5)

### Architecture

`rules::dispatcher` owns the polling loop and lifecycle; `rules::outbox` is
the repository boundary (claim, lease recovery, execution, failure
recording). One poll cycle (`dispatcher::run_once`) is independently
callable — the integration tests drive cycles directly, without the HTTP
server or wall-clock sleeps.

### Claim and lease semantics

1. `recover_expired_leases` first: `processing` rows whose
   `lease_expires_at < now` return to `pending` with all claim fields
   cleared. **Attempt count is not incremented** — a crash between claim and
   execute never burns retry budget.
2. Eligibility: `next_attempt_at <= now`, status `pending` or
   `retryable_failed` (retry whose backoff elapsed), and an internal action
   type. `send_message` is **never claimed** by this dispatcher.
3. Claiming is a guarded per-row `UPDATE … WHERE id = ? AND next_attempt_at
   <= ? AND status IN ('pending','retryable_failed')` — the winner set is
   exact (affected-rows == 1 per row); a competing worker's update
   re-evaluates the predicate against the locked row and loses. No
   `SKIP LOCKED`, no `LIMIT`-in-subquery — portable across all three engines.

### Fencing-token guarantees

Every claim carries a fresh token (256-bit, hashed storage domain); every
subsequent completion/failure update is predicated on it. A worker whose
lease was reaped and whose record was re-claimed holds a stale token: its
completion matches zero rows, so `execute_internal_action` returns
`Conflict` and the **whole execution transaction — business effect, ledger
entry, completion — rolls back**. A stale worker can neither mark an intent
delivered nor overwrite a newer worker's claim, and it cannot extend a lease
(no lease-extension operation exists).

### Retry and terminal failure

`classify_failure` is explicit: payload/type/scope/principal-corruption
errors are **Permanent** (immediate `dead_letter`); everything else
(DB errors, commit races) is **Retryable**. `record_failure` (fenced by
token) increments `attempts`, persists bounded sanitized diagnostics
(`last_error_category`, truncated `last_error_detail`), and either
reschedules — `status='retryable_failed'`, `next_attempt_at = now +
min(max, base·2^(attempt−1)) · jitter` (full-jitter bounds unit-tested) — or
dead-letters when `attempts >= max_attempts`. Config knobs:
`TASKER_RULES_WORKER_{ENABLED,POLL_MS,BATCH,CONCURRENCY,LEASE_MS,MAX_ATTEMPTS,BACKOFF_BASE_MS,BACKOFF_MAX_MS}`
(defaults: enabled, 1000, 16, 2, 60000, 8, 1000, 60000).

### Execution transaction boundary

Per claimed intent: `db.begin()` → `execute_internal_action(tx, id,
Some(token))` (Task 4 primitive: scope validation, payload revalidation,
principal provisioning, effect, ledger row, fenced completion) → commit.
The transaction covers only database work — no network I/O. Commit failure
leaves the lease to expire and the intent to retry; duplicate effects
converge through the ledger (`AlreadyExecuted` + fenced completion of the
still-owned row).

### Lifecycle

`dispatcher::spawn` is called from `main` only when `enabled`, wired to the
server's shutdown signal via a `watch` channel: shutdown stops claiming
immediately, the loop exits, `main` awaits the join handle before closing
the pool. No detached tasks; the semaphore bounds concurrent executions
(default 2) inside each cycle.

### Observability

`Metrics::observe_outbox` feeds bounded-cardinality counters exposed on
`/metrics`: `tasker_outbox_intents_claimed_total`,
`_actions_executed_total`, `_already_executed_total`,
`_external_pending_total`, `_retryable_failures_total`,
`_dead_lettered_total`, `_leases_recovered_total`,
`_stale_rejections_total`, and
`tasker_outbox_execution_duration_seconds_{sum,count}`. Structured tracing
covers startup/shutdown, claim batches, executions, retries, dead-letters,
and stale rejections — with intent/action/worker fields, never payloads or
secrets.

### External `send_message` handoff boundary

The internal dispatcher's claim query excludes `send_message`. The future
Messaging Integration Service identifies work via
`status='pending' AND action_type='send_message'` and can reuse the same
claim/lease/fence fields and `rule_action_results` ledger semantics without
any Rules Engine contract change. Until that service exists, those intents
remain durable, visible, and unconsumed.

### Cross-database validation risks (deferred to the final Phase 8 CI gate)

- The guarded claim UPDATE relies on read-committed re-evaluation of the
  `status` predicate under concurrent workers — verified on SQLite only;
  PostgreSQL 17 and MySQL 8.4 semantics must be confirmed in CI.
- `LIMIT ?` bound-parameter behavior through the Any driver on MySQL.
- CHECK-constraint timing on MySQL 8.4 (old-row vs new-row token predicates)
  for the fenced completion under concurrency.
- `recover_expired_leases` + `claim_batch` ordering under MySQL's
  REPEATABLE READ isolation.

### Failure classification policy (reliability review)

`classify_failure` maps every error reaching the dispatcher:

| Error | Class | Attempt burned? | Dead-letters? | Duplicate-effect risk |
|---|---|---|---|---|
| Payload/type validation failure (`stored action payload…`) | Permanent | Yes (1) | Immediately | None — effect tx rolled back with it |
| Corrupted scope chain / task-team mismatch | Permanent | Yes (1) | Immediately | None |
| Principal identity corrupted | Permanent | Yes (1) | Immediately | None |
| `principal_email_conflict` (foreign holder of the reserved email) | **Permanent** | Yes (1) | Immediately | None |
| Vanished scope row (`NotFound` on intent/execution/event/rule load) | **Permanent** | Yes (1) | Immediately | None — rows are FK-anchored, so a miss is corruption |
| DB unavailability, busy, commit races, stale fencing token | Retryable | Yes (1) | After `max_attempts` | None — effect tx rolled back; ledger pre-check converges |
| Unique violation on the ledger (racing direct calls) | Retryable | Yes (1) | Never in practice — retry pre-check → `AlreadyExecuted` | None — effect rolled back with the failed ledger insert |
| Commit failure | Retryable via the fenced recorder | Yes (1) **if** the commit truly rolled back | After max_attempts | None — if the commit actually landed, the fenced update matches zero rows (token cleared) and the ledger pre-check converges |
| Unknown/unrecognized shapes | **Retryable (documented policy)** | Yes (1) | After max_attempts | None — same rollback guarantee |

The unknown-failure policy is explicit: unrecognized error shapes are treated
as Retryable because (a) the effect's transaction always rolls back with the
failure, so no committed work can be duplicated; (b) attempts burn only when
`record_failure` lands, bounding any unknown failure to `max_attempts` and a
`dead_letter` terminal state; (c) the ledger pre-check converges a
mis-retried already-committed effect. Stale-token errors (`Conflict: no
longer owned`) never burn the dead attempt's budget — the fence rejects them
and the current owner owns the outcome.

A lost failure-record race (infrastructure down while writing the failure)
is logged as an error and leaves the row `processing` — the lease expires
and the intent retries without burning an attempt. Distinct from a lost
*claim* (`Conflict` → `StaleRejected` metric).

Lease recovery is observed on `/metrics`
(`tasker_outbox_leases_recovered_total`) and logged as a structured warning
with the recovered count — a persistently nonzero value is the crash-loop
signal. Recovery never increments attempts, so repeated worker crashes
cannot strand or exhaust a record.

---

## Rules management API (Task 6)

Endpoints (all team-scoped; contract in `api/openapi.yaml`, regenerated
client in `frontend/src/api/schema.d.ts`):

| Operation | Endpoint | Authorization |
|---|---|---|
| Create | `POST /teams/{team_id}/rules` | Owner/Admin (`Capability::ManageRules`) |
| List (paginated, trigger/enabled filters) | `GET /teams/{team_id}/rules` | Owner/Admin |
| Detail | `GET /rules/{rule_id}` | Owner/Admin |
| Update (CAS on `expected_version`) | `PATCH /rules/{rule_id}` | Owner/Admin |
| Enable / disable | `POST /rules/{rule_id}/enable` · `/disable` | Owner/Admin |
| Archive | `POST /rules/{rule_id}/archive` | Owner/Admin |
| Version history | `GET /rules/{rule_id}/versions` | Owner/Admin |
| Execution history (paginated) | `GET /rules/{rule_id}/history` | Owner/Admin/Manager |

Semantics: updates write an immutable `rule_versions` row and bump
`current_version` under CAS (`WHERE current_version = expected`); a lost race
is 409. Members get 403; outsiders/guests get the standard 404 (no
enumeration). Invalid conditions/actions yield 422 with `errors[]` field
entries (via `From<RuleError> for AppError`). Archived rules are read-only,
stop evaluating, and keep their names reserved. Enable/disable/archive
record activity-trail entries (`rule_created/updated/enabled/disabled/archived`)
attributed to the acting user. History survives task deletion by design.

---

## Phase 8B handoff — Git integration (architectural proposal, not implemented)

### Module boundaries (inside Tasker Core, Phase 8B)

```
src/integrations/
├── mod.rs        integration registry, provider-neutral intake contract
├── github.rs     GitHub adapter: webhook verification, event parsing
├── gitlab.rs     GitLab adapter: webhook verification, event parsing
└── normalize.rs  Git event → Tasker domain-event mapping
```

Provider-specific parsing, signature schemes, and API clients live only in
the adapters; nothing downstream may import a provider type.

### Webhook ingestion requirements

1. Signature/token verification per provider (HMAC for GitHub, token for
   GitLab) before any parsing.
2. Provider identity + installation/project context resolved to a Tasker
   team via an explicit mapping table (new migration in 8B).
3. Replay protection: delivery-id dedup with a bounded retention window.
4. Durable intake: webhooks land in an ingestion table inside one
   transaction; processing is retry-safe and idempotent (delivery id is the
   idempotency anchor).
5. Bounded payloads: size caps and per-provider event whitelists; unknown
   events are recorded and skipped, never guessed at.

### Event normalization

Adapters translate provider payloads into the existing `domain_events`
pipeline with `origin` extended by a provider-neutral metadata struct
(provider name, repo, delivery id) — the Rules Engine itself must not learn
provider names. Rules gain no new trigger types in 8B beyond what the
normalizer emits.

### Core authorization boundary

Normalized Git events may only act through the established application
services (task/comment creation, transitions) with an explicit integration
principal — the Automation Principal pattern reused, never a bypass. The
Rules Engine's team scoping and `origin` policy apply unchanged.

### Suggested 8B sequence

1. Intake table + ingestion endpoint skeleton (signature verification first).
2. GitHub adapter (push/PR/issue events) → normalized events.
3. GitLab adapter (push/MR events) → normalized events.
4. Rules integration: new trigger vocabulary + docs, reusing the entire
   evaluate → outbox → dispatch pipeline unchanged.
