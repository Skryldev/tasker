# Configuration

All configuration is provided through `TASKER_*` environment variables. There
are no config files and no defaults that embed secrets.

## Backend

| Variable | Default | Description |
| --- | --- | --- |
| `TASKER_DATABASE_URL` | **required** | Engine URL: `postgres://user:pass@host:port/db`, `mysql://user:pass@host:port/db`, or `sqlite:<path>` (e.g. `sqlite:D:/data/tasker.db`, `sqlite:/var/lib/tasker/tasker.db`). SQLite databases are created on first connect; migrations run at startup. |
| `TASKER_ENV` | `development` | `development` or `production`. Production sets `Secure` on session cookies. |
| `TASKER_HTTP_HOST` | `0.0.0.0` | Bind address. |
| `TASKER_HTTP_PORT` | `8080` | Bind port. |
| `TASKER_SESSION_TTL_HOURS` | `336` | Fixed session lifetime (14 days). |
| `TASKER_DB_MAX_CONNECTIONS` | `10` | Bounded pool size. |
| `TASKER_DB_ACQUIRE_TIMEOUT_SECS` | `5` | Max wait for a pooled connection. |
| `TASKER_DB_IDLE_TIMEOUT_SECS` | `300` | Idle connection reaping. |
| `TASKER_DB_MAX_LIFETIME_SECS` | `1800` | Connection max age. |
| `TASKER_RATE_LIMIT_LOGIN_PER_MIN` | `10` | Login attempts per minute per client IP (in-memory, per process). |
| `TASKER_MFA_ENCRYPTION_KEY` | unset | AES-256 key (64 hex chars = 32 bytes) encrypting TOTP secrets at rest. Unset disables the 2FA endpoints (fail-closed); a malformed value aborts startup. Generate with `openssl rand -hex 32` and back it up — losing it makes enrolled secrets unrecoverable. |
| `TASKER_RECOVERY_PEPPER` | unset | Independent HMAC-SHA-256 key (64 hex chars = 32 bytes) for recovery-code digests (ADR-0012). Provision independently of the encryption key. Unset disables recovery-code flows (fail-closed); a malformed value aborts startup. Rotating or losing it invalidates stored recovery codes (users regenerate via the authenticated flow). |
| `TASKER_BODY_LIMIT_BYTES` | `1048576` | Maximum request body size. |
| `TASKER_ALLOWED_ORIGINS` | *(empty)* | Comma-separated origins allowed by CORS **and** the CSRF origin check. Empty = same-origin only, which is correct when the app is served behind the bundled proxy. |
| `TASKER_PUBLIC_REGISTRATION` | `false` | Allow `POST /api/v1/auth/register`. Leave disabled; provision members via Team Settings. |
| `TASKER_STATIC_DIR` | *(unset)* | Serve a built frontend directory (SPA fallback to `index.html`). |
| `TASKER_LOG_FORMAT` | `text` | `text` or `json` structured logs. |
| `RUST_LOG` | `info,sqlx=warn` | tracing filter. |

## Frontend

| Variable | Default | Description |
| --- | --- | --- |
| `VITE_API_URL` | `http://localhost:8080` | Dev-server proxy target (`vite.config.ts`). Production deployments use the same origin via the proxy instead. |

## Docker Compose (`.env`)

See `deploy/env.example`: `TASKER_DB_PASSWORD`, `TASKER_DB_ROOT_PASSWORD`
(mysql profile), `TASKER_HTTP_PORT`, `TASKER_ALLOWED_ORIGINS`.

## Secrets

Secrets arrive only via environment variables (Compose injects them from the
gitignored `.env`). Nothing is written to logs: password hashes, session
tokens, and cookies are excluded from all log records, and internal error
details are logged server-side while clients receive sanitized Problem
Details.
