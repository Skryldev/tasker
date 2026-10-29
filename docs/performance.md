# Performance

Baselines recorded in Phase 1, with the exact methodology to reproduce and
compare future changes. **No production capacity guarantee is implied** —
these are single-machine development measurements whose purpose is
regression detection and identifying bottlenecks early.

## Environment of the recorded baselines

- Windows 11 (10.0.26100), x64 — development machine
- Rust 1.96 debug-profile backend for load tests (release build will be
  faster; criterion benches run with the `bench` profile)
- Local database servers on the same machine (SQLite file, PostgreSQL 18 on
  localhost:5433, MariaDB 11.4.5 on localhost:3307)
- autocannon defaults: 10 connections, no pipelining

## Database benchmarks (Criterion)

Command: `cd backend && cargo bench --bench db`

Dataset: 5,000 root tasks in one project, evenly distributed over statuses
and priorities (SQLite).

| Benchmark | Median time | Notes |
| --- | --- | --- |
| list_paginated (25/page of 5,000) | ~559 µs | created_at DESC, keyset-safe tiebreak on id |
| list_search (`LIKE %42%` over title+description) | ~479 µs | lower() LIKE — no FTS yet |
| board_columns (5 status queries) | ~316 µs | one indexed query per column |
| get_by_id | ~8.9 µs | primary-key lookup |
| insert (with UUIDv7 + commit) | ~130 µs | single transaction |
| update_status | ~76 µs | single-row UPDATE |

## Notification benchmarks (Phase 2)

Same harness, dataset: 1,000 notifications for one recipient (half unread).

| Benchmark | Median time |
| --- | --- |
| notifications_list (page of 20 of 1,000) | ~30 µs |
| notifications_unread_count | ~22 µs |

Both queries are covered by the recipient/unread indexes added in migration
0002. The Phase 1 task benchmarks re-measured within noise of the recorded
baselines (list ~556 µs, search ~481 µs, board ~339 µs, insert ~130 µs,
update ~75 µs) — no regression from the workflow/notification code paths.

## HTTP load test

Command: `scripts/loadtest.sh <port> <seconds> <sqlite|postgres|mysql>`
(boots a fresh server, seeds 500 tasks concurrently, runs five 8-second
authenticated scenarios; raw JSON lands in `.devtools/loadtest-*.json`).

Results (average requests/sec, average latency, p99, errors):

| Scenario | SQLite | PostgreSQL 18 | MariaDB 11.4 |
| --- | --- | --- | --- |
| read-list (paginated 25) | 1,725 r/s · 5.3 ms · p99 10 | 2,187 r/s · 4.1 ms · p99 13 | 2,227 r/s · 4.0 ms · p99 14 |
| read-board (5 columns × 50) | 246 r/s · 40.1 ms · p99 62 | 205 r/s · 48.4 ms · p99 66 | 213 r/s · 46.4 ms · p99 59 |
| me-tasks (empty result set) | 6,153 r/s · 1.1 ms | 6,745 r/s · 0.9 ms | 6,793 r/s · 0.9 ms |
| create task (transactional) | 921 r/s · 10.1 ms | 1,537 r/s · 5.8 ms | 1,461 r/s · 6.1 ms |
| update task status | 6,442 r/s · 0.8 ms | 7,246 r/s · 0.9 ms | 7,251 r/s · 0.9 ms |

All scenarios: **0 errors** on every engine.

## Reading the numbers

- **Write-heavy flows favor server engines**: task creation is ~65% faster on
  PostgreSQL than SQLite under 10 concurrent writers — SQLite serializes
  writers; this is expected and documented (docs/database-support.md). Short
  bursts above SQLite's write throughput receive `503` with `Retry-After`
  (`AppError::Busy`) rather than errors or data corruption.
- **The board endpoint is the most expensive read** (5 column queries + per
  column user/label embedding). Fine at current scale; a single grouped query
  is the first optimization if projects grow beyond thousands of cards.
- **Pagination bounds work** — list queries never scan beyond
  `page_size` rows on indexed paths.

## Known bottlenecks / future work

1. `LOWER(col) LIKE '%q%'` cannot use an index; move to engine-specific FTS
   or pg_trgm when search latency matters at larger datasets.
2. The activity feed loads up to 100 rows per request with a join — acceptable
   now, paginate later.
3. Load tests ran a debug-profile binary; release builds will shift absolute
   numbers upward. Re-record before/after optimization work.

## Limitations of these measurements

Single client machine, local databases, small dataset (500–5,000 tasks),
short sampling windows, no network hop, no TLS. They establish relative
baselines for regression tracking — not SLOs.
