# Database support

Tasker treats **PostgreSQL, MySQL, and SQLite as first-class engines**
selected purely by `TASKER_DATABASE_URL` — no code changes, no separate
builds.

## Status matrix

| Engine | Status | Migrations | Integration tests | Load tested |
| --- | --- | --- | --- | --- |
| PostgreSQL 10+/17 | ✅ verified | `migrations/postgres/` | ✅ security matrix + smoke | ✅ |
| MySQL 8.x protocol | ✅ verified via MariaDB 11.4 | `migrations/mysql/` | ✅ security matrix + smoke | ✅ |
| SQLite 3.35+ | ✅ verified (bundled) | `migrations/sqlite/` | ✅ security matrix + smoke | ✅ |

Honesty note: the MySQL-protocol verification ran on **MariaDB 11.4.5**
(available locally). MySQL 8.x uses the same wire protocol and the same
portable SQL subset (InnoDB, utf8mb4, CHECK constraints supported since
8.0.16); it has *not* been executed against a real MySQL 8 server in this
phase and is labeled accordingly.

## Portability strategy

One SQL codebase runs on all engines. The rules that make this work:

1. **Placeholders**: statements use `?`; the query layer rewrites them to
   numbered `$N` for PostgreSQL at execution time (`db::adapt_sql`, with a
   unit test).
2. **Types**: BIGINT epoch-millisecond timestamps (application-generated),
   TEXT/CHAR ISO dates (`YYYY-MM-DD`, lexicographic = chronological),
   VARCHAR for all indexed strings (MySQL utf8mb4 index limits), BOOLEAN
   columns filtered in SQL rather than decoded into Rust (PG returns real
   bools; MySQL/SQLite integers).
3. **No dialect features in shared code**: no `RETURNING` (inserts re-select
   by id), no `INSERT … ON CONFLICT/ON DUPLICATE KEY` (pre-checks +
   unique-violation mapping to HTTP 409), portable `LIKE … ESCAPE '|'
   (backslash is a MySQL string escape).
4. **Schema differences live only in migrations**: three dialect directories
   applied by one runner, each statement split conservatively (string/comment
   aware), executed in a per-migration transaction and recorded in
   `schema_migrations`.

### Migration safety

- Applied automatically at startup, deterministically ordered by version.
- Concurrent startups serialize on an engine-appropriate lock:
  `pg_advisory_lock`, MySQL `GET_LOCK`, and SQLite's native write locking
  (plus a busy timeout).
- Failures abort the process with a precise error — a half-migrated database
  never serves traffic, and non-destructive DDL makes reruns safe.
- Fresh-install and upgrade paths are the same code path; no destructive
  resets are ever issued automatically.
- Migrations that cannot run inside a transaction (SQLite table rebuilds
  needing `PRAGMA foreign_keys = OFF`, MySQL/MariaDB DDL that implicitly
  commits) declare `-- tasker:autocommit` and execute statement-by-statement
  with the version recorded last.

## Engine-specific notes

### PostgreSQL (recommended for teams)
- Row-level locking and MVCC handle concurrent writes well.
- `LOWER(col) LIKE …` benefits from `pg_trgm`/`citext` indexes if search
  grows — an operator-level addition, not an application change.

### MySQL / MariaDB
- `utf8mb4` everywhere; indexed strings are VARCHAR with explicit lengths.
- CHECK constraints require MySQL ≥ 8.0.16 (MariaDB 10.2+ is fine).
- `MEDIUMTEXT` was avoided deliberately: it decodes as BLOB through the
  portable layer; text columns are sized VARCHARs.

### SQLite (lightweight deployments)
- Enabled pragmas on every pooled connection: `foreign_keys=ON`,
  `journal_mode=WAL`, `busy_timeout=5000`, `synchronous=NORMAL`.
- **Honest limits**: SQLite serializes writers. Under heavy concurrent
  writes beyond the busy timeout the backend returns `503` (`type: busy`,
  `Retry-After: 1`) instead of erroring or corrupting — clients may retry.
  Single-team deployments are comfortably within its envelope; growing
  teams should move to PostgreSQL (a URL change; the same migrations
  discipline applies — migrate with a fresh database or a dump/restore).
- The database file is created on first connect (`mode=rwc`).

## Migration 0002 (Phase 2)

Adds `tasks.reporter_id` (nullable FK), acceptance metadata
(`resolution`/`resolution_note`/`resolved_by`/`resolved_at`), and the
`notifications` table with recipient and unread indexes on all three
engines. The reporter backfill is deterministic: tasks inherit their creator
as reporter when the creator is still a team member; otherwise the project
owner (always a validated member). No rows are discarded and no destructive
operations occur.

## Migration 0003 (Phase 3)

Adds `project_members` (guest grants, PK project+user, FK cascade),
`task_dependencies` (PK prevents duplicate edges, FK cascade on task
delete), `task_templates` + `task_template_labels`, and
`comments.guest_visible` (NOT NULL DEFAULT false — non-destructive
backfill: existing comments stay internal) with supporting indexes on all
three engines. Applied by the same locked runner; verified fresh-install
and upgrade paths on SQLite, PostgreSQL, and MariaDB.

## Migration 0004 (Phase 4)

Adds `goals`, `key_results`, `goal_projects`, and `goal_tasks` with FK
cascades (goal deletion cascades key results and association rows; project
and task deletion cleans up association rows while the goal survives), a
CHECK-constrained goal status and metric type, indexes on team listing,
key-result lookup, and association joins, plus a nullable
`activities.goal_id` for goal-scoped history. Verified fresh-install and
upgrade paths on SQLite, PostgreSQL, and MariaDB.

## Migration 0005 (pre-Phase-5 integrity hotfix)

Corrects the task lifecycle to the strict six-state machine (see
ADR-0011). The legacy `done` status is mapped conservatively using the
Phase 2 `resolution` column as evidence: `done` with
`resolution='accepted'` becomes `closed`; any other `done` row becomes
`resolved` (rejected or undecided work stays in the reviewable pipeline
rather than earning unearned acceptance). PostgreSQL swaps the CHECK
constraint atomically: DDL is transactional there, so the whole migration
(constraint drops, remap, re-add, version record) commits together and a
crash rolls back for a clean re-run. Note the base schema declares the
status CHECK inline, so its auto-generated name is `tasks_status_check` —
the migration drops that name and the reserved `chk_tasks_status`
defensively before remapping. MySQL/MariaDB DDL implicitly commits, so the
swap cannot be atomic: a stop between `DROP CHECK` and `ADD CHECK` leaves
valid data without the constraint, and a naive re-run fails at `DROP CHECK`
(fail-closed — the server refuses to start; recovery is to re-apply the
remaining statements manually). SQLite cannot alter a CHECK constraint, so
the tasks table is rebuilt with the new constraint and the status mapping
expressed as a CASE in the copy SELECT — rows are copied before anything
is dropped, so no crash window can lose data (fail-closed recovery windows
are documented in the migration header). The rebuild needs
`PRAGMA foreign_keys = OFF`, which is a no-op inside a transaction —
migrations may now declare an `-- tasker:autocommit` marker; the runner
then executes their statements outside a wrapping transaction (on a single
connection) and records the version at the end. A statement filter for
that mode skips only pure-comment blocks. Verified fresh-install and
upgrade paths on SQLite;
the PostgreSQL/MariaDB variants are exercised by CI's three-engine job.

## Backup and restore

- **SQLite**: stop the server (or use `sqlite3 tasker.db ".backup …"` on a
  quiescent WAL database), copy the `tasker.db` (+ `-wal`/`-shm` while
  stopped). Restore = place the file and start. *This procedure is
  documented but was not exercised as part of Phase 1 verification.*
- **PostgreSQL**: `pg_dump`/`pg_restore` on the `tasker` database.
- **MySQL**: `mysqldump --single-transaction tasker`.
- Migrations are idempotent on an existing schema (applied versions are
  tracked), so restoring a backup from an older version then starting a
  newer backend applies only the missing migrations.

## Upgrade procedure

1. Back up (above).
2. Pull the new image / replace the binary.
3. Start — migrations apply automatically at boot; readiness flips only
   after they succeed.
