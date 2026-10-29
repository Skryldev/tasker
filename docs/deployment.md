# Deployment

Self-hosted deployment with Docker Compose. Three profiles choose the
database engine; only the frontend service publishes a port.

## Prerequisites

- Docker Engine 24+ with Compose v2 (not available on the Phase 1 authoring
  machine — the compose files are provided and reviewed but **not executed
  here**; see "Verification status" below).
- A `docker-compose.yml`, `backend/Dockerfile`, `frontend/Dockerfile` from
  the repository root.

## Profiles

```bash
cp deploy/env.example .env
# edit .env: TASKER_DB_PASSWORD (+ TASKER_DB_ROOT_PASSWORD for mysql)

docker compose --profile sqlite up -d --build      # single backend container + frontend
docker compose --profile postgres up -d --build    # recommended for teams
docker compose --profile mysql up -d --build       # MySQL 8 compatible
```

What each profile starts:

| Profile | Services | Persistent volume |
| --- | --- | --- |
| `sqlite` | `backend`, `frontend` | `sqlite-data` |
| `postgres` | `backend-postgres`, `postgres`, `frontend` | `postgres-data` |
| `mysql` | `backend-mysql`, `mysql`, `frontend` | `mysql-data` |

Databases are **not** exposed on host ports — only the frontend (8088 by
default) is reachable; internal services communicate on a private network.
Restart policy is `unless-stopped`; backends have health checks
(`/api/v1/health/ready`), database services have native healthchecks, and
the backend waits for them before starting.

## Initial administrator

The backend never creates credentials by itself. After the stack is up:

```bash
# postgres profile:
docker compose exec backend-postgres tasker bootstrap-admin --email you@company.com
# (prompts for a password; or pass --password / TASKER_ADMIN_PASSWORD)

# sqlite profile:
docker compose exec backend tasker bootstrap-admin --email you@company.com
# mysql profile: backend-mysql
```

`bootstrap-admin` refuses to run once any user exists, never prints the
password, and creates the account with the Owner-ready setup (log in, create
a team, invite members from Team Settings). Public self-registration stays
disabled.

## Migration behavior

Migrations run automatically at backend startup, before the server accepts
traffic, serialized against concurrent instances (advisory locks). Readiness
fails until they complete, so orchestrators never route to a half-migrated
instance.

## Persistence and upgrades

- Data lives in the named volumes above; recreate containers freely.
- Upgrade: back up (below), `docker compose --profile <p> pull` or rebuild,
  then `up -d` — new migrations apply at boot.
- Downgrade is unsupported (migrations are forward-only in Phase 1).

## Backup and restore

| Engine | Backup | Restore |
| --- | --- | --- |
| PostgreSQL | `docker compose exec postgres pg_dump -U tasker tasker > backup.sql` | `cat backup.sql | docker compose exec -T postgres psql -U tasker tasker` |
| MySQL | `docker compose exec mysql mysqldump -u tasker -p… tasker > backup.sql` | pipe into `mysql tasker` |
| SQLite | stop backend; copy volume contents (`tasker.db`) | place file, start |

These commands follow each engine's standard tooling; they are documented
rather than executed during Phase 1 verification (no Docker on the authoring
machine) — exercise them once in your environment before relying on them.

## Runtime without Docker

Any static file server or process manager works:

```bash
cd frontend && npm ci && npm run build            # produces frontend/dist
cd ../backend && cargo build --release
TASKER_ENV=production \
TASKER_DATABASE_URL="postgres://…" \
TASKER_STATIC_DIR=/path/to/frontend/dist \
TASKER_ALLOWED_ORIGINS=https://tasker.example.com \
./target/release/tasker serve
```

Put TLS termination (nginx/Caddy/traefik) in front and set
`TASKER_ALLOWED_ORIGINS` to the public origin so the CSRF origin check and
CORS stay correct. `/api/v1/health/live`, `/api/v1/health/ready`, and
`/metrics` are available for probes and scraping.

## Recovery from common startup failures

| Symptom | Cause / fix |
| --- | --- |
| `database initialization failed: cannot connect` | Wrong URL/credentials, database down, or (Compose) dependency not yet healthy — check the DB container logs. |
| `migration failed: …` | A previous partial state or an incompatible engine version; inspect the logged migration version, restore from backup if damaged, then retry. |
| `cannot bind 0.0.0.0:8080` | Port taken — change `TASKER_HTTP_PORT` / published port. |
| Login returns 429 | Login rate limit hit (per IP per minute); wait or raise `TASKER_RATE_LIMIT_LOGIN_PER_MIN`. |
| 503 `busy` problem on writes (SQLite) | Write contention — retry; consider PostgreSQL for multi-user load. |

## Verification status (honesty statement)

Phase 1 verified the application end-to-end **without Docker** (the machine
has no container runtime): native backend + built frontend, all three
database engines, smoke + integration + E2E suites. The Compose files and
Dockerfiles are written to the conventions above (multi-stage, non-root,
healthchecks, volumes, no baked secrets) but their `up` path was not
executed here and should be validated on a Docker host before production
use.
