#!/usr/bin/env bash
# Full E2E cycle: fresh server (new SQLite DB) + Playwright suite.
# Usage: scripts/e2e.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> Stopping any running tasker server"
taskkill //F //IM tasker.exe >/dev/null 2>&1 || true
sleep 1

echo "==> Building frontend"
(cd "$ROOT/frontend" && npm run build --silent > /dev/null)

echo "==> Building backend"
(cd "$ROOT/backend" && cargo build 2>&1 | tail -1 > /dev/null)

LOG="$ROOT/.devtools/e2e-server.log"
mkdir -p "$ROOT/.devtools"
bash "$ROOT/scripts/e2e-server.sh" 8091 > "$LOG" 2>&1 &
SERVER_PID=$!

cleanup() {
  kill "$SERVER_PID" >/dev/null 2>&1 || true
  taskkill //F //IM tasker.exe >/dev/null 2>&1 || true
}
trap cleanup EXIT

for i in $(seq 1 30); do
  if curl -sf http://localhost:8091/api/v1/health/ready > /dev/null 2>&1; then
    echo "==> Server ready"
    break
  fi
  [ "$i" = 30 ] && { echo "server did not become ready"; tail -20 "$LOG"; exit 1; }
  sleep 1
done

cd "$ROOT/frontend"
npx playwright test "$@"
