#!/usr/bin/env bash
# HTTP load-test scenarios against a real server (SQLite by default).
# Usage: scripts/loadtest.sh [port] [duration_secs]
#
# Scenarios (authenticated, cookie-based):
#   read-list   GET /projects/:id/tasks?page=1 (server-paginated list)
#   read-board  GET /projects/:id/board
#   me-tasks    GET /me/tasks
#   create      POST /projects/:id/tasks
#   update      PATCH /tasks/:id (field edit)
#
# Requires: npx autocannon (fetched on first use).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${1:-8095}"
DURATION="${2:-10}"
ENGINE="${3:-sqlite}"   # sqlite | postgres | mysql
BASE="http://localhost:$PORT"

echo "==> Stopping any running server"
taskkill //F //IM tasker.exe >/dev/null 2>&1 || true
sleep 1

echo "==> Building backend"
(cd "$ROOT/backend" && cargo build 2>&1 | tail -1 >/dev/null)

DB_DIR="$ROOT/.devtools/loadtest"
mkdir -p "$DB_DIR"
rm -f "$DB_DIR/load.db" "$DB_DIR/load.db-wal" "$DB_DIR/load.db-shm"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) DB_WIN="$(cygpath -m "$DB_DIR")" ;;
  *) DB_WIN="$DB_DIR" ;;
esac

if [ "$ENGINE" = "postgres" ]; then
  export PGPASSWORD="${PGPASSWORD:-tasker-test-pw}"
  "$ROOT/.devtools/tools/pgsql/psql" -h "${PGHOST:-localhost}" -p "${PGPORT:-5433}" -U "${PGUSER:-postgres}"     -qc "DROP DATABASE IF EXISTS tasker_load;" -qc "CREATE DATABASE tasker_load;" > /dev/null
  DB_URL="postgres://postgres:${PGPASSWORD:-tasker-test-pw}@${PGHOST:-localhost}:${PGPORT:-5433}/tasker_load"
elif [ "$ENGINE" = "mysql" ]; then
  "$ROOT/.devtools/tools/mariadb/bin/mariadb.exe" -h "${MYSQLHOST:-127.0.0.1}" -P "${MYSQLPORT:-3307}" -u root -p"${MYSQLPASSWORD:-tasker-test-pw}"     -e "DROP DATABASE IF EXISTS tasker_load; CREATE DATABASE tasker_load CHARACTER SET utf8mb4;" 2>/dev/null
  DB_URL="mysql://root:${MYSQLPASSWORD:-tasker-test-pw}@${MYSQLHOST:-127.0.0.1}:${MYSQLPORT:-3307}/tasker_load"
else
  DB_URL="sqlite:$DB_WIN/load.db"
fi
# Windows-style paths for Node tooling (require/fs cannot read MSYS paths).
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) DEV_WIN="$(cygpath -m "$ROOT/.devtools")" ;;
  *) DEV_WIN="$ROOT/.devtools" ;;
esac

echo "==> Bootstrap + start server on :$PORT"
TASKER_DATABASE_URL="sqlite:$DB_WIN/load.db" \
  "$ROOT/backend/target/debug/tasker" bootstrap-admin \
  --email load@tasker.local --password load-admin-pass-1 --display-name "Load" >/dev/null

TASKER_DATABASE_URL="sqlite:$DB_WIN/load.db" \
  TASKER_HTTP_PORT="$PORT" \
  "$ROOT/backend/target/debug/tasker" serve > "$ROOT/.devtools/loadtest-server.log" 2>&1 &
SERVER_PID=$!
trap 'kill $SERVER_PID >/dev/null 2>&1 || true; taskkill //F //IM tasker.exe >/dev/null 2>&1 || true' EXIT

for i in $(seq 1 30); do
  curl -sf "$BASE/api/v1/health/ready" > /dev/null 2>&1 && break
  [ "$i" = 30 ] && { echo "server not ready"; exit 1; }
  sleep 1
done
echo "==> Server ready"

echo "==> Seeding dataset (500 tasks, concurrent)"
SEED_OUT=$(node "$ROOT/scripts/seed-loadtest.mjs" "$BASE" 500 | tail -1)
PROJECT=$(echo "$SEED_OUT" | node -e "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>console.log(JSON.parse(d).project))")
SESSION=$(echo "$SEED_OUT" | node -e "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>console.log(JSON.parse(d).session))")
echo "==> Seeded project=$PROJECT"

TASK_ID=$(curl -s -H "Cookie: $SESSION" \
  "$BASE/api/v1/projects/$PROJECT/tasks?page_size=1" | sed 's/.*"id":"\([^"]*\)".*/\1/')

run() {
  local name="$1"; shift
  echo
  echo "==> Scenario: $name (${DURATION}s)"
  npx --yes autocannon "$@" \
    -d "$DURATION" -j > "$ROOT/.devtools/loadtest-$name.json"
  node -e "
    const r = JSON.parse(require('fs').readFileSync('$DEV_WIN/loadtest-$name.json', 'utf8'));
    console.log('    reqs/sec: ' + r.requests.average.toFixed(0)
      + '  avg: ' + r.latency.average.toFixed(1) + 'ms'
      + '  p99: ' + r.latency.p99.toFixed(1) + 'ms'
      + '  errors: ' + r.errors + '  non2xx: ' + r.non2xxx);
  "
}

run read-list  -H "Cookie: $SESSION" "$BASE/api/v1/projects/$PROJECT/tasks?page=1&page_size=25"
run read-board -H "Cookie: $SESSION" "$BASE/api/v1/projects/$PROJECT/board"
run me-tasks   -H "Cookie: $SESSION" "$BASE/api/v1/me/tasks"
run create     -H "Cookie: $SESSION" -H "Content-Type: application/json" \
  -m POST -b '{"title":"Load generated task","priority":"low"}' \
  "$BASE/api/v1/projects/$PROJECT/tasks"
run update     -H "Cookie: $SESSION" -H "Content-Type: application/json" \
  -m PATCH -b '{"priority":"high"}' \
  "$BASE/api/v1/tasks/$TASK_ID"

echo
echo "==> Raw JSON results in $ROOT/.devtools/loadtest-*.json"
