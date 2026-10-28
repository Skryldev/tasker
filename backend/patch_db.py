"""One-off patch: add driver-aware placeholder adaptation to db.rs Conn."""
import io

p = r"D:\Golang\tasker\backend\src\db.rs"
s = io.open(p, encoding="utf-8").read()

for f in ["activity.rs", "auth.rs", "projects.rs", "comments.rs", "tasks.rs", "teams.rs"]:
    fp = rf"D:\Golang\tasker\backend\src\{f}"
    t = io.open(fp, encoding="utf-8").read()
    t = t.replace("Conn::from_pool(state.db.pool())", "state.db.conn()")
    io.open(fp, "w", encoding="utf-8", newline="\n").write(t)

old_conn = '''/// A connection handle that abstracts over the pool and an open transaction.
pub enum Conn<'a> {
    Pool(&'a AnyPool),
    Tx(Box<Transaction<'static, sqlx::Any>>),
}

impl<'a> Conn<'a> {
    #[must_use]
    pub fn from_pool(pool: &'a AnyPool) -> Self {
        Self::Pool(pool)
    }
'''
new_conn = '''/// A connection handle that abstracts over the pool and an open transaction,
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
'''
assert old_conn in s, "old_conn not found"
s = s.replace(old_conn, new_conn)

old_begin = '''    /// Begin a transaction; all subsequent operations on the returned handle
    /// run inside it until it is committed or rolled back.
    pub async fn begin(&mut self) -> Result<Conn<'static>, AppError> {
        match self {
            Conn::Pool(pool) => Ok(Conn::Tx(Box::new(
                pool.begin().await.map_err(AppError::from)?,
            ))),
            Conn::Tx(_) => Err(AppError::internal("nested transactions are not supported")),
        }
    }

    pub async fn execute(&mut self, sql: &str, args: Args<'_>) -> Result<u64, AppError> {
        let query = sqlx::query_with(sql, args.inner);
        let rows = match self {
            Conn::Pool(pool) => query.execute(*pool).await?,
            Conn::Tx(tx) => query.execute(&mut ***tx).await?,
        };
        Ok(rows.rows_affected())
    }

    pub async fn fetch_optional(
        &mut self,
        sql: &str,
        args: Args<'_>,
    ) -> Result<Option<AnyRow>, AppError> {
        let query = sqlx::query_with(sql, args.inner);
        let row = match self {
            Conn::Pool(pool) => query.fetch_optional(*pool).await?,
            Conn::Tx(tx) => query.fetch_optional(&mut ***tx).await?,
        };
        Ok(row)
    }

    pub async fn fetch_all(
        &mut self,
        sql: &str,
        args: Args<'_>,
    ) -> Result<Vec<AnyRow>, AppError> {
        let query = sqlx::query_with(sql, args.inner);
        let rows = match self {
            Conn::Pool(pool) => query.fetch_all(*pool).await?,
            Conn::Tx(tx) => query.fetch_all(&mut ***tx).await?,
        };
        Ok(rows)
    }

    /// Commit the transaction (no-op when running on the plain pool).
    pub async fn commit(self) -> Result<(), AppError> {
        match self {
            Conn::Pool(_) => Ok(()),
            Conn::Tx(tx) => tx.commit().await.map_err(AppError::from),
        }
    }

    /// Roll back the transaction (no-op when running on the plain pool).
    pub async fn rollback(self) {
        if let Conn::Tx(tx) = self {
            let _ = tx.rollback().await;
        }
    }
}'''
new_begin = '''    /// Begin a transaction; all subsequent operations on the returned handle
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
        let query = sqlx::query_with(&adapt_sql(sql, self.driver), args.inner);
        let rows = match &mut self.inner {
            ConnInner::Pool(pool) => query.execute(**pool).await?,
            ConnInner::Tx(tx) => query.execute(&mut ***tx).await?,
        };
        Ok(rows.rows_affected())
    }

    pub async fn fetch_optional(
        &mut self,
        sql: &str,
        args: Args<'_>,
    ) -> Result<Option<AnyRow>, AppError> {
        let query = sqlx::query_with(&adapt_sql(sql, self.driver), args.inner);
        let row = match &mut self.inner {
            ConnInner::Pool(pool) => query.fetch_optional(**pool).await?,
            ConnInner::Tx(tx) => query.fetch_optional(&mut ***tx).await?,
        };
        Ok(row)
    }

    pub async fn fetch_all(
        &mut self,
        sql: &str,
        args: Args<'_>,
    ) -> Result<Vec<AnyRow>, AppError> {
        let query = sqlx::query_with(&adapt_sql(sql, self.driver), args.inner);
        let rows = match &mut self.inner {
            ConnInner::Pool(pool) => query.fetch_all(**pool).await?,
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

/// Adapt portable `?` placeholders to the engine's native form. PostgreSQL
/// requires numbered `$N` placeholders; MySQL and SQLite use `?`.
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
}'''
assert old_begin in s, "old_begin not found"
s = s.replace(old_begin, new_begin)

old_helpers = '''    /// Begin a transaction on this database as a uniform connection handle.
    pub async fn begin(&self) -> Result<Conn<'static>, AppError> {
        Ok(Conn::Tx(Box::new(
            self.pool.begin().await.map_err(AppError::from)?,
        )))
    }'''
new_helpers = '''    /// A pooled connection handle with driver-aware query adaptation.
    #[must_use]
    pub fn conn(&self) -> Conn<'_> {
        Conn::from_pool(&self.pool, self.driver)
    }

    /// Begin a transaction on this database as a uniform connection handle.
    pub async fn begin(&self) -> Result<Conn<'static>, AppError> {
        Ok(Conn {
            driver: self.driver,
            inner: ConnInner::Tx(Box::new(
                self.pool.begin().await.map_err(AppError::from)?,
            )),
        })
    }'''
assert old_helpers in s, "old_helpers not found"
s = s.replace(old_helpers, new_helpers)

s = s.replace('''    pub async fn ping(&self) -> Result<(), AppError> {
        let mut conn = Conn::from_pool(&self.pool);
        conn.execute("SELECT 1", Args::new()).await?;
        Ok(())
    }''', '''    pub async fn ping(&self) -> Result<(), AppError> {
        let mut conn = self.conn();
        conn.execute("SELECT 1", Args::new()).await?;
        Ok(())
    }''')

s = s.replace(
    'sqlx::query("INSERT INTO schema_migrations (version, applied_at) VALUES (?, ?)")',
    'sqlx::query(&adapt_sql(\n            "INSERT INTO schema_migrations (version, applied_at) VALUES (?, ?)",\n            driver,\n        ))',
)

s = s.replace('''    #[test]
    fn driver_detection() {''', '''    #[test]
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
    fn driver_detection() {''')

io.open(p, "w", encoding="utf-8", newline="\n").write(s)
print("patched db.rs OK")
