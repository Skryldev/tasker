#!/usr/bin/env bash
# Start a fresh, E2E-ready server: builds the frontend, wipes the SQLite file,
# applies migrations, bootstraps the administrator, and serves the UI.
# Usage: scripts/e2e-server.sh [port]
set -euo pipefail
PORT="${1:-8091}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DB_DIR="$ROOT/.devtools/e2e"
mkdir -p "$DB_DIR"
# Convert to a Windows path so the sqlite: URL resolves correctly on MSYS/Git Bash.
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) DB_WIN="$(cygpath -m "$DB_DIR")" ;;
  *) DB_WIN="$DB_DIR" ;;
esac
rm -f "$DB_DIR/e2e.db" "$DB_DIR/e2e.db-wal" "$DB_DIR/e2e.db-shm"

echo "==> Building frontend"
(cd "$ROOT/frontend" && npm run build --silent)

echo "==> Bootstrapping administrator"
TASKER_DATABASE_URL="sqlite:$DB_WIN/e2e.db" \
  "$ROOT/backend/target/debug/tasker" bootstrap-admin \
  --email admin@tasker.local --password Admin-Pass-12345! --display-name "Ada Admin"

echo "==> Starting server on :$PORT"
# TASKER_MFA_ENCRYPTION_KEY / TASKER_RECOVERY_PEPPER: fixed test-only values
# (NOT production secrets); production keys are generated per deployment —
# see docs/security.md.
exec env TASKER_DATABASE_URL="sqlite:$DB_WIN/e2e.db" \
  TASKER_HTTP_PORT="$PORT" \
  TASKER_STATIC_DIR="$ROOT/frontend/dist" \
  TASKER_RATE_LIMIT_LOGIN_PER_MIN=100 \
  TASKER_ATTACHMENTS=local \
  TASKER_ATTACHMENTS_DIR="$ROOT/.devtools/e2e/attachments" \
  TASKER_MFA_ENCRYPTION_KEY="3d2690e6245a4e2fa53b7f8d1c9a07b4e5c6d8f90a1b2c3d4e5f60718293a4b5" \
  TASKER_RECOVERY_PEPPER="7c1f4a9e2b6d8305fa14c7e902b5d8163a9f04cbe7d2185a630fe94c7b2d5186" \
  "$ROOT/backend/target/debug/tasker" serve
