# Testing

## Layers

| Layer | Location | What it covers |
| --- | --- | --- |
| Rust unit tests | `backend/src/**` | permission matrix, password/token handling, rate limiter, SQL splitter, placeholder adaptation, driver detection (13 tests) |
| Rust HTTP integration | `backend/tests/integration.rs` | security matrix (unauthenticated access, cross-team 404s, self-escalation, cross-assignment, validation shapes, subtask nesting, archival boundary, logout revocation) **plus the Phase 2 workflow/notification matrix** (submit/accept/reject permissions, reviewer independence, rejection reason, resolution consistency, stale transitions, notification generation, mention dedup, cross-user isolation) over real engines |
| API smoke | `scripts/smoke_api.sh` | 32-check end-to-end product workflow over HTTP |
| Frontend unit | `frontend/src/**/*.test.tsx` | login validation/API errors, badges, empty/error states, theme toggle, toasts (11 tests) |
| End-to-end | `frontend/e2e/workflow.spec.ts`, `frontend/e2e/workflow-p2.spec.ts` | real-browser workflows: drag-and-drop persistence, negative authorization, and Phase 2 submission → acceptance/rejection through the UI with notification navigation (14 tests total) |
| Benchmarks | `backend/benches/db.rs`, `scripts/loadtest.sh` | performance baselines (see docs/performance.md) |

## Exact commands

```bash
# Backend unit + integration (SQLite always)
cd backend
cargo test

# Backend integration on ALL engines (PostgreSQL + MySQL/MariaDB servers
# must be reachable; adjust URLs as needed)
export PGPASSWORD=…  # for the bootstrap psql calls below (optional)
psql -h localhost -p 5433 -U postgres -qc "DROP DATABASE IF EXISTS tasker_it;" \
                              -qc "CREATE DATABASE tasker_it;"
mariadb -h 127.0.0.1 -P 3307 -u root -p… -e "DROP DATABASE IF EXISTS tasker_it; CREATE DATABASE tasker_it;"
TASKER_TEST_PG_URL="postgres://postgres:…@localhost:5433/tasker_it" \
TASKER_TEST_MYSQL_URL="mysql://root:…@127.0.0.1:3307/tasker_it" \
cargo test

# Lints and formatting
cargo fmt --check
cargo clippy --all-targets   # zero-warning policy

# API smoke (server must be running)
scripts/smoke_api.sh http://localhost:8081 admin@example.com 'super-secure-pass-1'

# Frontend
cd frontend
npm run typecheck
npx vitest run
npm run build

# End-to-end (builds frontend, starts a fresh server, runs Playwright)
scripts/e2e.sh
```

## Engine matrix verification (Phase 1 results)

The integration suite runs the identical security matrix against:

- **SQLite** — bundled driver, temp-file database (default).
- **PostgreSQL 18** — `TASKER_TEST_PG_URL` (tested against a local 18 instance).
- **MySQL protocol** — `TASKER_TEST_MYSQL_URL` (tested against MariaDB
  11.4.5; MySQL 8.x compatibility is covered by the portable SQL subset and
  the dedicated migration dialect — see docs/database-support.md caveats).

All three passed the full matrix, plus the 32-check API smoke suite on each
engine (see the implementation report for the recorded runs).

## CI

`.github/workflows/ci.yml` runs on every push/PR: Rust fmt/clippy/tests,
frontend typecheck/lint/tests/build, OpenAPI lint, and (on `main`) the
three-engine integration job with services. Playwright E2E runs via
`scripts/e2e.sh` where a runner with browsers is available.

## Fixtures and isolation

Integration tests create per-run UUID-suffixed databases (SQLite files,
fresh Postgres/MySQL databases) and UUID-suffixed users, so runs are
independent and cleanup-free by construction. E2E always starts from a
fresh bootstrapped database (`scripts/e2e-server.sh` deletes the SQLite
file first).
