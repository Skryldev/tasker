# Development

## Toolchain

- Rust 1.88+ (edition 2024), `cargo fmt` + `cargo clippy` (zero-warning
  policy), `cargo bench` for Criterion.
- Node 22+, npm 12.
- Optional: PostgreSQL/MariaDB servers for the full test matrix (SQLite is
  bundled and always available).
- Playwright browsers: `cd frontend && npx playwright install chromium`.

## Repository conventions

- Backend handlers stay thin; business rules live in the module services.
- All SQL is portable across engines — read `docs/database-support.md`
  before adding a query (placeholder, type, and feature rules).
- Any schema change requires a new migration in **all three** dialect
  directories with the same version prefix.
- OpenAPI is authoritative: change `api/openapi.yaml` first, regenerate the
  TypeScript types (`cd frontend && npm run generate:api`), re-convert the
  JSON (`node -e "…" > api/openapi.json` or rerun scripts from api/README),
  then implement the handler.
- Rust: `unsafe_code = "forbid"`, clippy pedantic with a small documented
  allowlist (`backend/Cargo.toml [lints.clippy]`).

## Common tasks

```bash
# Run everything the CI runs, locally:
cd backend  && cargo fmt --check && cargo clippy --all-targets && cargo test
cd frontend && npm run typecheck && npx vitest run && npm run build

# Regenerate the frontend API types after contract changes:
cd frontend && npm run generate:api

# Full E2E cycle (fresh DB, build, Playwright):
scripts/e2e.sh

# Load test against a chosen engine:
scripts/loadtest.sh 8095 10 postgres
```

## Debugging tips

- `RUST_LOG=debug` (or `RUST_LOG=tasker=trace,sqlx=warn`) for verbose logs;
  request IDs are echoed in the `x-request-id` response header.
- The dev frontend proxies `/api` to `http://localhost:8080`; run `tasker
  serve` with any engine while iterating on UI.
- For frontend state issues, React Query devtools are not bundled; use the
  `queryKeys` in `src/api/hooks.ts` to reason about cache scopes.
