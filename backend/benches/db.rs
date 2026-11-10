// Database-backed benchmarks for representative task operations.
// Run: cargo bench --bench db
//
// Uses SQLite (bundled, always available). The queries exercised are the
// same portable statements used for PostgreSQL and MySQL; per-engine numbers
// belong to the HTTP load-test scenarios (scripts/loadtest.sh).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use sqlx::Row;
use tasker::config::{Config, Environment, PoolConfig};
use tasker::db::{Args, Database};
use tasker::users;

fn bench_config(url: String) -> Config {
    Config {
        environment: Environment::Development,
        http_host: "127.0.0.1".into(),
        http_port: 0,
        database_url: url,
        pool: PoolConfig {
            max_connections: 4,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(300),
            max_lifetime: Duration::from_secs(3600),
        },
        session_ttl: Duration::from_secs(3600),
        mfa_encryption_key: Some(vec![7u8; 32]),
        mfa_recovery_pepper: Some(vec![11u8; 32]),
        public_registration: false,
        allowed_origins: Vec::new(),
        login_rate_limit_per_min: 1000,
        body_limit_bytes: 1_048_576,
        static_dir: None,
        log_format_json: false,
        rules_worker: tasker::config::RulesWorkerConfig::default(),
        integrations_key: Some(vec![19u8; 32]),
    }
}

/// Seed a team/project/user plus `task_count` tasks distributed over statuses.
async fn seed(db: &Database, task_count: usize) -> (String, String, String) {
    let mut conn = db.begin().await.unwrap();
    let now = tasker::util::now_ms();

    let hash = tasker::security::password::hash("bench-password-123").unwrap();
    let user = users::insert(
        &mut conn,
        &users::NewUser {
            email: "bench@tasker.local",
            display_name: "Bench",
            password_hash: &hash,
        },
    )
    .await
    .unwrap();

    let team_id = tasker::util::new_id();
    let mut args = Args::new();
    args.s(&team_id)
        .s("Bench Team")
        .opt_s(None::<String>)
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO teams (id, name, description, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        args,
    )
    .await
    .unwrap();
    let mut args = Args::new();
    args.s(&team_id).s(&user.id).s("owner").i(now);
    conn.execute(
        "INSERT INTO team_members (team_id, user_id, role, created_at) VALUES (?, ?, ?, ?)",
        args,
    )
    .await
    .unwrap();

    let project_id = tasker::util::new_id();
    let mut args = Args::new();
    args.s(&project_id)
        .s(&team_id)
        .s("Bench Project")
        .opt_s(None::<String>)
        .s("active")
        .s(&user.id)
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO projects (id, team_id, name, description, status, owner_id, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await
    .unwrap();

    let statuses = [
        "backlog",
        "todo",
        "in_progress",
        "resolved",
        "in_review",
        "closed",
    ];
    let priorities = ["low", "medium", "high", "urgent"];
    for i in 0..task_count {
        let id = tasker::util::new_id();
        let status = statuses[i % statuses.len()];
        let priority = priorities[i % priorities.len()];
        let mut args = Args::new();
        args.s(&id)
            .s(&team_id)
            .s(&project_id)
            .opt_s(None::<String>)
            .s(format!("Task number {i} is about plumbing fixture {i}"))
            .opt_s(None::<String>)
            .s(status)
            .s(priority)
            .opt_s(None::<String>)
            .s(&user.id)
            .opt_s(None::<String>)
            .f(f64::from(u32::try_from(i).unwrap_or(u32::MAX)))
            .opt_i(None)
            .i(now)
            .i(now);
        conn.execute(
            "INSERT INTO tasks (id, team_id, project_id, parent_task_id, title, description, status, priority, \
             assignee_id, created_by, due_date, position, completed_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .unwrap();
    }
    conn.commit().await.unwrap();
    (team_id, project_id, user.id)
}

/// Seed `count` notifications (half unread) for the bench user.
async fn seed_notifications(
    db: &Database,
    recipient: &str,
    actor: &str,
    task_id: &str,
    count: usize,
) {
    let mut conn = db.begin().await.unwrap();
    let now = tasker::util::now_ms();
    for i in 0..count {
        let id = tasker::util::new_id();
        let mut args = Args::new();
        args.s(&id)
            .s(recipient)
            .s("task_assigned")
            .s(task_id)
            .s(actor)
            .s(format!("Bench notification number {i} about fixture {i}"))
            .i(now - i64::try_from(count - i).unwrap_or(0));
        if i % 2 == 0 {
            args.i(now);
        }
        conn.execute(
            "INSERT INTO notifications (id, recipient_id, event, task_id, actor_id, summary, created_at, read_at)              VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .unwrap();
    }
    conn.commit().await.unwrap();
}

fn db_benchmarks(c: &mut Criterion) {
    const TASKS: usize = 5_000;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let url = format!(
        "sqlite:{}/tasker-bench-{}.db",
        std::env::temp_dir().display(),
        tasker::util::new_id()
    );
    let config = bench_config(url);
    let db: Arc<Database> = Arc::new(runtime.block_on(async {
        let db = Database::connect(&config).await.unwrap();
        db.migrate().await.unwrap();
        db
    }));

    let (team_id, project_id, user_id) = runtime.block_on(async { seed(&db, TASKS).await });

    let mut group = c.benchmark_group("task_operations");
    group.throughput(Throughput::Bytes(TASKS as u64));

    // Task list: paginated + filtered, as issued by the list view.
    group.bench_function(BenchmarkId::new("list_paginated", TASKS), |b| {
        b.to_async(&runtime).iter(|| async {
            let mut conn = db.conn();
            let sql = "SELECT id FROM tasks t WHERE t.project_id = ? AND t.parent_task_id IS NULL \
                       ORDER BY t.created_at DESC, t.id ASC LIMIT ? OFFSET ?";
            let mut args = Args::new();
            args.s(&project_id).i(25i64).i(0i64);
            let rows = conn.fetch_all(sql, args).await.unwrap();
            black_box(rows.len());
        });
    });

    // Task list with search predicate (LIKE over title).
    group.bench_function(BenchmarkId::new("list_search", TASKS), |b| {
        b.to_async(&runtime).iter(|| async {
            let mut conn = db.conn();
            let sql = "SELECT id FROM tasks t WHERE t.project_id = ? AND t.parent_task_id IS NULL \
                       AND (LOWER(t.title) LIKE ? ESCAPE '|' OR LOWER(COALESCE(t.description, '')) LIKE ? ESCAPE '|') \
                       ORDER BY t.created_at DESC LIMIT ? OFFSET ?";
            let mut args = Args::new();
            args.s(&project_id)
                .s("%42%".to_string())
                .s("%42%".to_string())
                .i(25i64)
                .i(0i64);
            let rows = conn.fetch_all(sql, args).await.unwrap();
            black_box(rows.len());
        });
    });

    // Board fetch: one query per status column (as the endpoint does).
    group.bench_function(BenchmarkId::new("board_columns", TASKS), |b| {
        b.to_async(&runtime).iter(|| async {
            let mut conn = db.conn();
            let mut total = 0usize;
            for status in ["backlog", "todo", "in_progress", "resolved", "in_review", "closed"] {
                let sql = "SELECT id FROM tasks t WHERE t.project_id = ? AND t.status = ? AND t.parent_task_id IS NULL \
                           ORDER BY t.position, t.created_at, t.id LIMIT ?";
                let mut args = Args::new();
                args.s(&project_id).s(status).i(50i64);
                let rows = conn.fetch_all(sql, args).await.unwrap();
                total += rows.len();
            }
            black_box(total);
        });
    });

    // Single task row fetch by primary key.
    group.bench_function(BenchmarkId::new("get_by_id", TASKS), |b| {
        b.to_async(&runtime).iter(|| async {
            let mut conn = db.conn();
            let mut args = Args::new();
            args.s(&project_id).i(1i64);
            let rows = conn
                .fetch_all("SELECT id FROM tasks WHERE project_id = ? LIMIT ?", args)
                .await
                .unwrap();
            black_box(rows.len());
        });
    });

    // Insert (includes UUIDv7 generation).
    group.bench_function(BenchmarkId::new("insert", TASKS), |b| {
        b.to_async(&runtime).iter(|| async {
            let mut conn = db.begin().await.unwrap();
            let id = tasker::util::new_id();
            let now = tasker::util::now_ms();
            let mut args = Args::new();
            args.s(&id)
                .s(&team_id)
                .s(&project_id)
                .opt_s(None::<String>)
                .s("Bench inserted task")
                .opt_s(None::<String>)
                .s("todo")
                .s("medium")
                .opt_s(None::<String>)
                .s(&user_id)
                .opt_s(None::<String>)
                .f(9999.0)
                .opt_i(None)
                .i(now)
                .i(now);
            conn.execute(
                "INSERT INTO tasks (id, team_id, project_id, parent_task_id, title, description, status, priority, \
                 assignee_id, created_by, due_date, position, completed_at, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                args,
            )
            .await
            .unwrap();
            conn.commit().await.unwrap();
        });
    });

    // Status update (single-row UPDATE, as the kanban drag performs).
    group.bench_function(BenchmarkId::new("update_status", TASKS), |b| {
        b.to_async(&runtime).iter(|| async {
            let mut conn = db.conn();
            let mut args = Args::new();
            args.s(&project_id).i(1i64);
            let rows = conn
                .fetch_all("SELECT id FROM tasks WHERE project_id = ? LIMIT 1", args)
                .await
                .unwrap();
            if let Some(row) = rows.first() {
                let id: String = row.try_get(0).unwrap();
                let mut args = Args::new();
                args.s("in_progress").i(tasker::util::now_ms()).s(&id);
                conn.execute(
                    "UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?",
                    args,
                )
                .await
                .unwrap();
            }
        });
    });

    // ---- Notification queries (Phase 2) --------------------------------
    let notif_user = runtime.block_on(async {
        let mut conn = db.begin().await.unwrap();
        let hash = tasker::security::password::hash("bench-password-123").unwrap();
        let user = users::insert(
            &mut conn,
            &users::NewUser {
                email: "notif-bench@tasker.local",
                display_name: "Notif Bench",
                password_hash: &hash,
            },
        )
        .await
        .unwrap();
        conn.commit().await.unwrap();
        user.id
    });
    let bench_task_id = runtime.block_on(async {
        let mut conn = db.begin().await.unwrap();
        let id = tasker::util::new_id();
        let now = tasker::util::now_ms();
        let mut args = Args::new();
        args.s(&id)
            .s(&team_id)
            .s(&project_id)
            .opt_s(None::<String>)
            .s("Notification bench task")
            .opt_s(None::<String>)
            .s("todo")
            .s("medium")
            .opt_s(None::<String>)
            .opt_s(None::<String>)
            .s(&user_id)
            .opt_s(None::<String>)
            .f(9_999.0)
            .opt_i(None)
            .i(now)
            .i(now);
        conn.execute(
            "INSERT INTO tasks (id, team_id, project_id, parent_task_id, title, description, status, priority,              assignee_id, reporter_id, created_by, due_date, position, completed_at, created_at, updated_at)              VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .unwrap();
        conn.commit().await.unwrap();
        id
    });
    runtime.block_on(async {
        seed_notifications(&db, &notif_user, &user_id, &bench_task_id, 1_000).await;
    });

    group.throughput(Throughput::Bytes(1_000));
    group.bench_function(BenchmarkId::new("notifications_list", 1_000), |b| {
        b.to_async(&runtime).iter(|| async {
            let mut conn = db.conn();
            let mut args = Args::new();
            args.s(&notif_user).i(20i64).i(0i64);
            let rows = conn
                .fetch_all(
                    "SELECT n.id FROM notifications n WHERE n.recipient_id = ?                      ORDER BY n.created_at DESC, n.id DESC LIMIT ? OFFSET ?",
                    args,
                )
                .await
                .unwrap();
            black_box(rows.len());
        });
    });

    group.bench_function(BenchmarkId::new("notifications_unread_count", 1_000), |b| {
        b.to_async(&runtime).iter(|| async {
            let mut conn = db.conn();
            let mut args = Args::new();
            args.s(&notif_user);
            let row = conn
                .fetch_optional(
                    "SELECT COUNT(*) AS n FROM notifications WHERE recipient_id = ? AND read_at IS NULL",
                    args,
                )
                .await
                .unwrap();
            black_box(row.map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0)));
        });
    });

    group.finish();

    runtime.block_on(async { db.close().await });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(50).measurement_time(Duration::from_secs(3));
    targets = db_benchmarks
}
criterion_main!(benches);
