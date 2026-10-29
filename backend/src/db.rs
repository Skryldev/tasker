//! Database infrastructure: engine selection, bounded pools, uniform
//! runtime-checked query execution, and a portable migration runner.
//!
//! Design notes:
//! - Queries are runtime-checked (`sqlx::any`) so one SQL codebase serves
//!   `PostgreSQL`, `MySQL`, and `SQLite`. SQL is restricted to the portable subset:
//!   no `RETURNING`, no dialect-specific upserts, `LOWER(col) LIKE ?` for
//!   search, BIGINT epoch-millisecond timestamps, TEXT ISO dates.
//! - Schema differences live exclusively in per-dialect migration files
//!   (`migrations/<driver>/`) applied by [`Database::migrate`].

use std::collections::HashSet;
use std::sync::Once;

use sqlx::any::{AnyArguments, AnyPoolOptions, AnyRow};
use sqlx::{AnyPool, Arguments, Connection, Executor, Row, Transaction};

use crate::config::Config;
use crate::error::AppError;
use crate::util;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Postgres,
    MySql,
    Sqlite,
}

impl Driver {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Postgres => "postgresql",
            Self::MySql => "mysql",
            Self::Sqlite => "sqlite",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Database {
    pool: AnyPool,
    driver: Driver,
}

static INSTALL_DRIVERS: Once = Once::new();

impl Database {
    /// Create the connection pool for the configured database URL.
    pub async fn connect(config: &Config) -> Result<Self, AppError> {
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);

        let mut url = config.database_url.clone();
        let driver = detect_driver(&url)?;

        if driver == Driver::Sqlite {
            // Create the database file on first connect (standard SQLite
            // `mode=rwc`); migrations then populate the schema.
            if !url.contains('?') {
                url.push_str("?mode=rwc");
            }

            if let Some(parent) = sqlite_path(&url).and_then(|p| {
                std::path::Path::new(&p)
                    .parent()
                    .map(std::path::Path::to_path_buf)
            }) {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AppError::internal(format!("cannot create sqlite data directory: {e}"))
                })?;
            }
        }

        // Production-grade SQLite settings are applied to every pooled
        // connection: foreign keys ON, WAL journaling, busy timeout, and
        // NORMAL fsync. (URL `_pragma` params are not portable through the
        // `Any` driver's URL parser, hence the after_connect hook.)
        let pool = AnyPoolOptions::new()
            .max_connections(config.pool.max_connections)
            .acquire_timeout(config.pool.acquire_timeout)
            .idle_timeout(config.pool.idle_timeout)
            .max_lifetime(config.pool.max_lifetime)
            .after_connect(move |conn, _| {
                Box::pin(async move {
                    if driver == Driver::Sqlite {
                        for pragma in [
                            "PRAGMA foreign_keys = ON",
                            "PRAGMA journal_mode = WAL",
                            "PRAGMA busy_timeout = 5000",
                            "PRAGMA synchronous = NORMAL",
                        ] {
                            sqlx::query(pragma).execute(&mut *conn).await?;
                        }
                    }
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .map_err(|e| {
                AppError::internal(format!("cannot connect to {} database: {e}", driver.name()))
            })?;

        Ok(Self { pool, driver })
    }

    #[must_use]
    pub fn driver(&self) -> Driver {
        self.driver
    }

    #[must_use]
    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// A pooled connection handle with driver-aware query adaptation.
    #[must_use]
    pub fn conn(&self) -> Conn<'_> {
        Conn::from_pool(&self.pool, self.driver)
    }

    /// Cheap connectivity probe for readiness checks.
    pub async fn ping(&self) -> Result<(), AppError> {
        let mut conn = self.conn();
        conn.execute("SELECT 1", Args::new()).await?;
        Ok(())
    }

    /// Close the pool gracefully (waits for in-flight connections).
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Begin a transaction on this database as a uniform connection handle.
    pub async fn begin(&self) -> Result<Conn<'static>, AppError> {
        Ok(Conn {
            driver: self.driver,
            inner: ConnInner::Tx(Box::new(self.pool.begin().await.map_err(AppError::from)?)),
        })
    }

    /// Apply all pending migrations for the active driver, serialized against
    /// concurrent migrators via a driver-appropriate lock, in a single
    /// transaction per migration.
    pub async fn migrate(&self) -> Result<(), AppError> {
        let mut conn = self.pool.acquire().await.map_err(|e| {
            AppError::internal(format!("cannot acquire connection for migrations: {e}"))
        })?;

        let lock = MigrationLock::for_driver(self.driver);
        lock.acquire(&mut conn).await;

        let result = run_pending_migrations(&mut conn, self.driver).await;

        lock.release(&mut conn).await;
        result
    }
}

struct MigrationLock {
    acquire_sql: Option<&'static str>,
    release_sql: Option<&'static str>,
}

impl MigrationLock {
    fn for_driver(driver: Driver) -> Self {
        match driver {
            // Session-scoped locks: held by the dedicated migration connection.
            Driver::Postgres => Self {
                acquire_sql: Some("SELECT pg_advisory_lock(812736)"),
                release_sql: Some("SELECT pg_advisory_unlock(812736)"),
            },
            Driver::MySql => Self {
                acquire_sql: Some("SELECT GET_LOCK('tasker_migration', 60)"),
                release_sql: Some("SELECT RELEASE_LOCK('tasker_migration')"),
            },
            // SQLite serializes writers itself; the busy timeout set on the
            // connect URL makes concurrent startups wait instead of failing.
            Driver::Sqlite => Self {
                acquire_sql: None,
                release_sql: None,
            },
        }
    }

    async fn acquire(&self, conn: &mut sqlx::AnyConnection) {
        if let Some(sql) = self.acquire_sql
            && let Err(e) = conn.execute(sql).await
        {
            tracing::error!(error = %e, "failed to acquire migration lock");
        }
    }

    async fn release(&self, conn: &mut sqlx::AnyConnection) {
        if let Some(sql) = self.release_sql
            && let Err(e) = conn.execute(sql).await
        {
            tracing::error!(error = %e, "failed to release migration lock");
        }
    }
}

const POSTGRES_MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_init",
        include_str!("../migrations/postgres/0001_init.sql"),
    ),
    (
        "0002_workflow_notifications",
        include_str!("../migrations/postgres/0002_workflow_notifications.sql"),
    ),
];
const MYSQL_MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_init",
        include_str!("../migrations/mysql/0001_init.sql"),
    ),
    (
        "0002_workflow_notifications",
        include_str!("../migrations/mysql/0002_workflow_notifications.sql"),
    ),
];
const SQLITE_MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_init",
        include_str!("../migrations/sqlite/0001_init.sql"),
    ),
    (
        "0002_workflow_notifications",
        include_str!("../migrations/sqlite/0002_workflow_notifications.sql"),
    ),
];

#[must_use]
pub fn migrations_for(driver: Driver) -> &'static [(&'static str, &'static str)] {
    match driver {
        Driver::Postgres => POSTGRES_MIGRATIONS,
        Driver::MySql => MYSQL_MIGRATIONS,
        Driver::Sqlite => SQLITE_MIGRATIONS,
    }
}

async fn run_pending_migrations(
    conn: &mut sqlx::AnyConnection,
    driver: Driver,
) -> Result<(), AppError> {
    let create_table = match driver {
        Driver::MySql => {
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
             version VARCHAR(64) PRIMARY KEY, applied_at BIGINT NOT NULL) \
             ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
        }
        Driver::Postgres | Driver::Sqlite => {
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
             version TEXT PRIMARY KEY, applied_at BIGINT NOT NULL)"
        }
    };
    conn.execute(create_table)
        .await
        .map_err(|e| AppError::internal(format!("cannot create schema_migrations: {e}")))?;

    let applied: HashSet<String> = sqlx::query("SELECT version FROM schema_migrations")
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| AppError::internal(format!("cannot read applied migrations: {e}")))?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>(0).ok())
        .collect();

    for (version, sql) in migrations_for(driver) {
        if applied.contains(*version) {
            continue;
        }
        tracing::info!(version, driver = driver.name(), "applying migration");
        let mut tx: Transaction<'_, sqlx::Any> = conn
            .begin()
            .await
            .map_err(|e| AppError::internal(format!("cannot begin migration {version}: {e}")))?;
        for statement in split_statements(sql) {
            sqlx::query(statement)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::internal(format!("migration {version} failed: {e}")))?;
        }
        let now = util::now_ms();
        sqlx::query(&adapt_sql(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?, ?)",
            driver,
        ))
        .bind(*version)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::internal(format!("cannot record migration {version}: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| AppError::internal(format!("cannot commit migration {version}: {e}")))?;
    }
    Ok(())
}

/// Split a migration file into individual statements, respecting string
/// literals, quoted identifiers, and comments. Statement separators are
/// semicolons followed by end-of-line or EOF.
fn split_statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let bytes = sql.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while i < bytes.len() {
        let c = bytes[i];
        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
        } else if in_block_comment {
            if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 1;
            }
        } else if in_single {
            if c == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 1; // escaped quote
                } else {
                    in_single = false;
                }
            }
        } else if in_double {
            if c == b'"' {
                in_double = false;
            }
        } else if c == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            in_line_comment = true;
            i += 1;
        } else if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            in_block_comment = true;
            i += 1;
        } else if c == b'\'' {
            in_single = true;
        } else if c == b'"' {
            in_double = true;
        } else if c == b';' {
            let rest_is_ws = bytes[i + 1..].iter().all(u8::is_ascii_whitespace);
            if rest_is_ws || bytes.get(i + 1) == Some(&b'\n') || bytes.get(i + 1) == Some(&b'\r') {
                let stmt = &sql[start..=i];
                if stmt.trim().chars().any(|ch| !ch.is_whitespace()) {
                    statements.push(stmt.trim());
                }
                start = i + 1;
            }
        }
        i += 1;
    }
    let tail = &sql[start..];
    if tail.trim().chars().any(|ch| !ch.is_whitespace()) {
        statements.push(tail.trim());
    }
    statements
}

/// Typed parameter list bound positionally (`?` placeholders on all engines).
#[derive(Default)]
pub struct Args<'q> {
    inner: AnyArguments<'q>,
}

impl Args<'_> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn s(&mut self, v: impl Into<String>) -> &mut Self {
        self.inner
            .add(v.into())
            .expect("AnyArguments::add is infallible for supported scalar types");
        self
    }

    pub fn opt_s(&mut self, v: Option<impl Into<String>>) -> &mut Self {
        self.inner
            .add(v.map(Into::into))
            .expect("AnyArguments::add is infallible for supported scalar types");
        self
    }

    pub fn i(&mut self, v: i64) -> &mut Self {
        self.inner
            .add(v)
            .expect("AnyArguments::add is infallible for supported scalar types");
        self
    }

    pub fn opt_i(&mut self, v: Option<i64>) -> &mut Self {
        self.inner
            .add(v)
            .expect("AnyArguments::add is infallible for supported scalar types");
        self
    }

    pub fn f(&mut self, v: f64) -> &mut Self {
        self.inner
            .add(v)
            .expect("AnyArguments::add is infallible for supported scalar types");
        self
    }

    pub fn b(&mut self, v: bool) -> &mut Self {
        self.inner
            .add(v)
            .expect("AnyArguments::add is infallible for supported scalar types");
        self
    }
}

/// A connection handle that abstracts over the pool and an open transaction,
/// carrying the driver so placeholders can be adapted per engine.
pub struct Conn<'a> {
    driver: Driver,
    inner: ConnInner<'a>,
}

enum ConnInner<'a> {
    Pool(&'a AnyPool),
    Tx(Box<Transaction<'static, sqlx::Any>>),
}

impl<'a> Conn<'a> {
    #[must_use]
    fn from_pool(pool: &'a AnyPool, driver: Driver) -> Self {
        Self {
            driver,
            inner: ConnInner::Pool(pool),
        }
    }

    /// Begin a transaction; all subsequent operations on the returned handle
    /// run inside it until it is committed or rolled back.
    pub async fn begin(&mut self) -> Result<Conn<'static>, AppError> {
        match &self.inner {
            ConnInner::Pool(pool) => Ok(Conn {
                driver: self.driver,
                inner: ConnInner::Tx(Box::new(pool.begin().await.map_err(AppError::from)?)),
            }),
            ConnInner::Tx(_) => Err(AppError::internal("nested transactions are not supported")),
        }
    }

    pub async fn execute(&mut self, sql: &str, args: Args<'_>) -> Result<u64, AppError> {
        let sql = adapt_sql(sql, self.driver);
        let query = sqlx::query_with(&sql, args.inner);
        let rows = match &mut self.inner {
            ConnInner::Pool(pool) => query.execute(*pool).await?,
            ConnInner::Tx(tx) => query.execute(&mut ***tx).await?,
        };
        Ok(rows.rows_affected())
    }

    pub async fn fetch_optional(
        &mut self,
        sql: &str,
        args: Args<'_>,
    ) -> Result<Option<AnyRow>, AppError> {
        let sql = adapt_sql(sql, self.driver);
        let query = sqlx::query_with(&sql, args.inner);
        let row = match &mut self.inner {
            ConnInner::Pool(pool) => query.fetch_optional(*pool).await?,
            ConnInner::Tx(tx) => query.fetch_optional(&mut ***tx).await?,
        };
        Ok(row)
    }

    pub async fn fetch_all(&mut self, sql: &str, args: Args<'_>) -> Result<Vec<AnyRow>, AppError> {
        let sql = adapt_sql(sql, self.driver);
        let query = sqlx::query_with(&sql, args.inner);
        let rows = match &mut self.inner {
            ConnInner::Pool(pool) => query.fetch_all(*pool).await?,
            ConnInner::Tx(tx) => query.fetch_all(&mut ***tx).await?,
        };
        Ok(rows)
    }

    /// Commit the transaction (no-op when running on the plain pool).
    pub async fn commit(self) -> Result<(), AppError> {
        match self.inner {
            ConnInner::Pool(_) => Ok(()),
            ConnInner::Tx(tx) => tx.commit().await.map_err(AppError::from),
        }
    }

    /// Roll back the transaction (no-op when running on the plain pool).
    pub async fn rollback(self) {
        if let ConnInner::Tx(tx) = self.inner {
            let _ = tx.rollback().await;
        }
    }
}

/// Adapt portable `?` placeholders to the engine's native form. `PostgreSQL`
/// requires numbered `$N` placeholders; `MySQL` and `SQLite` use `?`.
fn adapt_sql(sql: &str, driver: Driver) -> std::borrow::Cow<'_, str> {
    if driver != Driver::Postgres || !sql.contains('?') {
        return std::borrow::Cow::Borrowed(sql);
    }
    let mut out = String::with_capacity(sql.len() + 8);
    let mut index = 0usize;
    for c in sql.chars() {
        if c == '?' {
            index += 1;
            out.push('$');
            out.push_str(&index.to_string());
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Map a unique-constraint violation to a friendly conflict error, leaving
/// other database errors untouched. Works across all three engines because
/// sqlx normalizes the error kind.
#[must_use]
pub fn map_unique_violation(err: AppError, conflict: String) -> AppError {
    match &err {
        AppError::Internal(msg)
            if msg.contains("UNIQUE constraint failed")
                || msg.contains("Duplicate entry")
                || msg.contains("unique constraint")
                || msg.contains("duplicate key") =>
        {
            AppError::Conflict(conflict)
        }
        _ => err,
    }
}

fn detect_driver(url: &str) -> Result<Driver, AppError> {
    let lower = url.to_lowercase();
    if lower.starts_with("postgres://") || lower.starts_with("postgresql://") {
        Ok(Driver::Postgres)
    } else if lower.starts_with("mysql://") || lower.starts_with("mariadb://") {
        Ok(Driver::MySql)
    } else if lower.starts_with("sqlite:") {
        // Matches sqlite:, sqlite:relative, sqlite:/abs and sqlite:///D:/win.
        Ok(Driver::Sqlite)
    } else {
        Err(AppError::internal(
            "unsupported database URL scheme — expected postgres://, mysql:// or sqlite:<path>",
        ))
    }
}

/// `SQLite` database file path from a `sqlite:` URL, if any. Handles both
/// `sqlite:relative.db`, `sqlite:/abs/path`, `sqlite:///D:/win/path` and the
/// Windows-friendly opaque form `sqlite:D:/win/path`.
fn sqlite_path(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))?;
    let path = rest.split('?').next()?;
    if path.is_empty() || path == ":memory:" {
        return None;
    }
    // A leading "/D:/" style prefix is a Windows drive with a stray slash
    // from URL normalization; keep "/C:/x" as "C:/x".
    if let Some(win) = path.strip_prefix('/')
        && win.as_bytes().get(1) == Some(&b':')
    {
        return Some(win.to_string());
    }
    Some(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_statements_respects_comments_and_strings() {
        let sql = "-- a comment; with semicolon\nCREATE TABLE a (x TEXT);\nINSERT INTO a VALUES ('hi; there');\n";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2, "parts: {parts:?}");
        assert!(parts[0].contains("CREATE TABLE"), "parts[0]: {}", parts[0]);
        assert!(parts[1].contains("hi; there"), "parts[1]: {}", parts[1]);
    }

    #[test]
    fn pg_placeholder_adaptation() {
        let sql = "SELECT a FROM t WHERE b = ? AND c = ?";
        assert_eq!(
            adapt_sql(sql, Driver::Postgres),
            "SELECT a FROM t WHERE b = $1 AND c = $2"
        );
        assert_eq!(adapt_sql(sql, Driver::MySql), sql);
        assert_eq!(adapt_sql("SELECT 1", Driver::Postgres), "SELECT 1");
    }

    #[test]
    fn driver_detection() {
        assert_eq!(
            detect_driver("postgres://u:p@h/db").unwrap(),
            Driver::Postgres
        );
        assert_eq!(detect_driver("mysql://u:p@h/db").unwrap(), Driver::MySql);
        assert_eq!(detect_driver("sqlite://tasker.db").unwrap(), Driver::Sqlite);
    }
}
