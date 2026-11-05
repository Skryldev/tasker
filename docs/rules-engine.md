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
