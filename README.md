# Tasker

Self-hosted team work management platform for startup teams: teams, projects,
tasks, subtasks, comments, and activity history — with a professional
Linear-inspired UI, a Rust/Axum backend, and full PostgreSQL / MySQL / SQLite
support.

```
┌────────────────────────────────────────────────────────┐
│  Tasker                                                │
│  ┌──────────┐  ┌──────────────────────────────────┐    │
│  │ Frontend │  │ Backend (Rust + Axum)            │    │
│  │ React 18 │→ │ REST /api/v1 · OpenAPI 3.1       │    │
│  │ Vite+TS  │  │ sessions · RBAC · activity log   │    │
│  └──────────┘  └──────────────┬───────────────────┘    │
│                               │ SQLx (one portable SQL)│
│                     PostgreSQL · MySQL · SQLite         │
└────────────────────────────────────────────────────────┘
```

## Quick start (development)

Prerequisites: Rust 1.88+, Node 22+, and one of the supported databases
(SQLite needs nothing — it is bundled).

```bash
# 1. Backend
cd backend
export TASKER_DATABASE_URL="sqlite:D:/absolute/path/tasker.db"   # or postgres://…, mysql://…
cargo run -- bootstrap-admin --email you@company.com             # prompts for a password
cargo run -- serve                                               # http://localhost:8080

# 2. Frontend (dev server proxies /api to :8080)
cd ../frontend
npm install
npm run dev                                                      # http://localhost:5173
```

## Quick start (Docker)

```bash
cp deploy/env.example .env         # set TASKER_DB_PASSWORD etc.
docker compose --profile postgres up -d --build

# Create the first administrator (runs inside the backend container):
docker compose exec backend-postgres tasker bootstrap-admin --email you@company.com
```

SQLite and MySQL profiles: `docker compose --profile sqlite up -d --build`,
`docker compose --profile mysql up -d --build`. See `docs/deployment.md`.

## What works (Phase 1)

- Cookie sessions (Argon2id passwords, hashed session tokens, revocation,
  login rate limiting, origin-based CSRF defense)
- Teams with Owner/Admin/Manager/Member roles and a centralized permission
  matrix enforced server-side; users can belong to multiple teams
- Projects with active/archived lifecycle and per-team isolation
- Tasks: statuses (Backlog → Todo → In Progress → In Review → Done),
  priorities, assignees (team members only), due dates, labels, subtasks
  (one level), search/filter/sort, server-side pagination
- Comments with @mentions and a transactional activity history per task
- Reporter acceptance workflow: submit for review, accept, reject with an
  audited reason; resolution state and history
- In-app notifications (assignments, mentions, review decisions) with unread
  badge, panel, and mark-as-read
- Project guest access: read-only, project-scoped grants for external users
- Task dependencies (finish-to-start) with cycle prevention
- Team workload view (server-side aggregation, filters)
- Project task templates (reusable defaults for new tasks)
- List and Kanban (drag-and-drop) views, My Tasks across teams
- Light/dark themes, responsive shell, loading/empty/error states
- Health/readiness endpoints, graceful shutdown, Prometheus-style metrics
- OpenAPI 3.1 contract (`api/openapi.yaml`) with a generated TypeScript client

## Repository layout

```
api/            OpenAPI 3.1 contract (authoritative) + generated TS types
backend/        Rust workspace (Axum modular monolith)
  migrations/   per-dialect schema: postgres/ mysql/ sqlite/
  benches/      Criterion database benchmarks
  tests/        engine-gated HTTP integration tests
frontend/       React app (Vite, Tailwind, shadcn-style UI, dnd-kit)
  e2e/          Playwright end-to-end workflow tests
deploy/         environment examples
docs/           architecture, operations, security, testing, performance, ADRs
scripts/        smoke / e2e / loadtest / test-matrix helpers
```

## Documentation

| Document | Contents |
| --- | --- |
| [docs/architecture.md](docs/architecture.md) | modules, data flow, key decisions |
| [docs/configuration.md](docs/configuration.md) | every environment variable |
| [docs/deployment.md](docs/deployment.md) | Docker profiles, bootstrap, backups, upgrades |
| [docs/database-support.md](docs/portability) | engine matrix, portability strategy, SQLite caveats |
| [docs/security.md](docs/security.md) | authN/authZ model, CSRF, secret handling |
| [docs/testing.md](docs/testing.md) | test layers and exact commands |
| [docs/performance.md](docs/performance.md) | benchmark & load-test baselines and methodology |
| [docs/decisions/](docs/decisions/) | architecture decision records |

## Development commands

```bash
cd backend  && cargo fmt && cargo clippy --all-targets && cargo test
cd frontend && npm run typecheck && npm run lint && npm test && npm run build
scripts/smoke_api.sh   <base-url> <email> <password>   # 32-check API workflow
scripts/e2e.sh                                          # Playwright E2E suite
scripts/loadtest.sh   [port] [secs] [engine]           # HTTP load test
```

## License

MIT (see headers). Phase 1 scope; deferred features are listed in the
implementation report and `docs/architecture.md`.
