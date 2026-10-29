# ADR-0003: One portable SQL dialect over sqlx::any

Date: 2026-10-28 · Status: accepted

## Context

The product must run on PostgreSQL, MySQL, and SQLite selected by
configuration. sqlx offers compile-time checked queries (tied to one engine)
and a runtime `AnyPool` (unchecked, multi-engine).

## Decision

Use `sqlx::any` with **runtime-checked queries in a portable subset**:
`?` placeholders adapted to `$N` for PostgreSQL at the query layer,
application-generated epoch-millisecond timestamps, TEXT/CHAR ISO dates,
VARCHAR indexed strings, no `RETURNING`, no dialect upserts, LIKE with an
explicit `|` escape. Schema differences live exclusively in per-dialect
migration files applied by one runner with engine-specific advisory locks.

## Consequences

- No compile-time SQL verification — compensated by the smoke suite (32
  checks) and the integration security matrix running on **every engine** in
  CI and locally.
- One place (`db::Conn`) to adapt engine quirks (placeholder rewriting,
  lock-contention → 503 mapping).
- `sqlx::any` quirks cost real debugging time (URL pragmas rejected,
  MEDIUMTEXT→BLOB decoding, bool typing) — all solved inside the
  infrastructure layer, documented in docs/database-support.md.
