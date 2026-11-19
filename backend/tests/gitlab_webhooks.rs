//! GitLab adapter end-to-end tests (Phase 8B, Task 4): the production
//! `GitLabAdapter` through the real intake → worker pipeline with real
//! GitLab webhook headers (token verification, `X-Gitlab-Webhook-UUID`,
//! `X-Gitlab-Event`) and payload shapes.
//!
//! Engine: `SQLite` always; PostgreSQL/MySQL deferred to the final Phase 8
//! cross-database CI gate.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;

use tasker::config::{Config, Environment, PoolConfig, WebhookWorkerConfig};
use tasker::db::Database;
use tasker::http::build_router;
use tasker::http::metrics::Metrics;
use tasker::integrations::Provider;
use tasker::integrations::credentials;
use tasker::integrations::production_registry;
use tasker::integrations::store;
use tasker::integrations::webhook::worker;
use tasker::state::AppState;

const SECRET: &str = "gl-webhook-secret-1";
const TEST_KEY: &[u8] = &[19u8; 32];

fn test_db_urls() -> Vec<(String, String)> {
    vec![(
        "sqlite".to_string(),
        format!(
            "sqlite:{}/{}",
            std::env::temp_dir().display(),
            Uuid::now_v7()
        ),
    )]
}

fn test_config(database_url: String) -> Config {
    Config {
        environment: Environment::Development,
        http_host: "127.0.0.1".into(),
        http_port: 0,
        database_url,
        pool: PoolConfig {
            max_connections: 4,
            acquire_timeout: std::time::Duration::from_secs(5),
            idle_timeout: std::time::Duration::from_secs(60),
            max_lifetime: std::time::Duration::from_secs(300),
        },
        session_ttl: std::time::Duration::from_secs(3600),
        mfa_encryption_key: Some(vec![7u8; 32]),
        mfa_recovery_pepper: Some(vec![11u8; 32]),
        public_registration: false,
        allowed_origins: Vec::new(),
        login_rate_limit_per_min: 100,
        body_limit_bytes: 1_048_576,
        static_dir: None,
        log_format_json: false,
        rules_worker: tasker::config::RulesWorkerConfig::default(),
        webhook_worker: WebhookWorkerConfig {
            enabled: false, // tests drive run_once directly
            ..WebhookWorkerConfig::default()
        },
        integrations_key: Some(TEST_KEY.to_vec()),
        attachments: tasker::config::AttachmentsConfig {
            mode: tasker::config::AttachmentsMode::Disabled,
            local_dir: String::new(),
            max_bytes: 1,
            max_per_task: 0,
            s3_endpoint: None,
            s3_bucket: None,
            s3_access_key: None,
            s3_secret_key: None,
            s3_region: "us-east-1".into(),
        },
    }
}

struct TestApp {
    app: axum::Router,
    db: Arc<Database>,
}

async fn spawn_app(url: &str) -> TestApp {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    let config = test_config(url.to_string());
    let db = Database::connect(&config).await.expect("connect");
    db.migrate()
        .await
        .unwrap_or_else(|e| panic!("migrate failed: {e}"));
    let db = Arc::new(db);
    let state = AppState::with_registry((*db).clone(), config, production_registry());
    TestApp {
        app: build_router(state),
        db,
    }
}

async fn to_response(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// POST with real GitLab webhook headers.
async fn gitlab_post(
    app: &axum::Router,
    integration_id: &str,
    token: &str,
    delivery_id: &str,
    event: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let uri = format!("/api/v1/webhooks/gitlab/{integration_id}");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-gitlab-token", token)
                .header("x-gitlab-webhook-uuid", delivery_id)
                .header("x-gitlab-event", event)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    to_response(response).await
}

async fn seed_user(db: &Database, email: &str, display: &str) -> String {
    let mut conn = db.begin().await.unwrap();
    let hash = tasker::security::password::hash("test-password-123").unwrap();
    let user = tasker::users::insert(
        &mut conn,
        &tasker::users::NewUser {
            email,
            display_name: display,
            password_hash: &hash,
            username: None,
            instance_role: tasker::users::INSTANCE_ROLE_MEMBER,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("seed_user {email} failed: {e}"));
    conn.commit().await.unwrap();
    user.id
}

async fn login_as(app: &axum::Router, email: &str, password: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "email": email, "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "login failed for {email}"
    );
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn seed_gitlab_integration(app: &TestApp, suffix: &str) -> String {
    let admin_email = format!("gl-{suffix}@test.local");
    let admin_id = seed_user(&app.db, &admin_email, "GL Admin").await;
    let session = login_as(&app.app, &admin_email, "test-password-123").await;
    let response = app
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/teams")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &session)
                .body(Body::from(json!({"name": "GLCREW"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (code, team) = to_response(response).await;
    assert_eq!(code, StatusCode::CREATED);
    let team_id = team["id"].as_str().unwrap().to_string();
    let integration_id = {
        let mut conn = app.db.conn();
        store::create_integration(
            &mut conn,
            &team_id,
            &format!("gl-{suffix}"),
            None,
            Provider::GitLab,
            &admin_id,
        )
        .await
        .expect("create integration")
    };
    {
        let mut conn = app.db.conn();
        credentials::provision_credential(
            &mut conn,
            &integration_id,
            SECRET,
            TEST_KEY,
            &admin_id,
            tasker::util::now_ms(),
        )
        .await
        .expect("provision credential");
        store::set_integration_enabled(&mut conn, &integration_id, true, tasker::util::now_ms())
            .await
            .expect("enable integration");
    }
    integration_id
}

struct DeliveryRow {
    id: String,
    status: String,
}

async fn fetch_delivery(
    db: &Database,
    integration_id: &str,
    delivery_id: &str,
) -> Option<DeliveryRow> {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(integration_id).s(delivery_id);
    conn.fetch_optional(
        "SELECT id, status FROM webhook_deliveries \
         WHERE integration_id = ? AND delivery_id = ?",
        args,
    )
    .await
    .expect("fetch delivery")
    .map(|r| DeliveryRow {
        id: r.try_get("id").unwrap(),
        status: r.try_get("status").unwrap(),
    })
}

async fn fetch_domain_event(db: &Database, delivery_row_id: &str) -> Option<sqlx::any::AnyRow> {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(delivery_row_id);
    conn.fetch_optional(
        "SELECT event_type, origin, new_state \
         FROM domain_events WHERE source_delivery_id = ?",
        args,
    )
    .await
    .expect("fetch domain event")
}

/// Drain claim cycles until the delivery is terminal (SQLite contention
/// retries converge on DB truth).
async fn run_until_terminal(
    app: &TestApp,
    worker_name: &str,
    integration_id: &str,
    delivery_id: &str,
) -> DeliveryRow {
    let config = WebhookWorkerConfig {
        backoff_base_ms: 50,
        ..WebhookWorkerConfig::default()
    };
    let registry = production_registry();
    let metrics = Metrics::new();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    for _ in 0..60 {
        let claimed = worker::run_once(
            &app.db,
            &registry,
            &config,
            worker_name,
            &metrics,
            &semaphore,
        )
        .await
        .unwrap_or(0);
        if let Some(row) = fetch_delivery(&app.db, integration_id, delivery_id).await {
            if matches!(
                row.status.as_str(),
                "processed" | "unsupported" | "dead_letter"
            ) {
                return row;
            }
        }
        if claimed == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
    fetch_delivery(&app.db, integration_id, delivery_id)
        .await
        .expect("delivery row after drain")
}

#[tokio::test]
async fn gitlab_webhook_end_to_end_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let integration_id = seed_gitlab_integration(&app, &suffix).await;

        // ---- Ordinary push → git_push with namespace keys ----
        let push = json!({
            "ref": "refs/heads/main",
            "before": "1111",
            "after": "2222",
            "total_commits_count": 5,
            "user_name": "gluser",
            "project": {"id": 1, "path_with_namespace": "group/tasker"},
        })
        .to_string();
        let (code, _) = gitlab_post(
            &app.app,
            &integration_id,
            SECRET,
            "gl-1",
            "Push Event",
            push.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED, "[{engine}] push accepted");
        let row = run_until_terminal(&app, "w", &integration_id, "gl-1").await;
        assert_eq!(row.status, "processed", "[{engine}]");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] domain event persisted");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_push"
        );
        assert_eq!(event.try_get::<String, _>("origin").unwrap(), "integration");
        let state: Value =
            serde_json::from_str(&event.try_get::<String, _>("new_state").unwrap()).unwrap();
        assert_eq!(state["provider"], "gitlab");
        assert_eq!(state["branch"], "main");
        assert_eq!(state["repository"], "group/tasker");
        assert_eq!(state["actor"], "gluser");
        assert_eq!(state["commit_count"], 5);
        drop(event);

        // ---- Branch creation push (all-zero before) → git_branch_created ----
        let create = json!({
            "ref": "refs/heads/feature/x",
            "before": "0".repeat(64),
            "after": "abc",
            "user_name": "gluser",
            "project": {"path_with_namespace": "group/tasker"},
        })
        .to_string();
        let (code, _) = gitlab_post(
            &app.app,
            &integration_id,
            SECRET,
            "gl-2",
            "Push Event",
            create.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let row = run_until_terminal(&app, "w", &integration_id, "gl-2").await;
        assert_eq!(row.status, "processed", "[{engine}]");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] event");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_branch_created"
        );
        drop(event);

        // ---- Merge request merged → git_pull_request_merged ----
        let mr = json!({
            "user": {"username": "merger"},
            "project": {"path_with_namespace": "group/tasker"},
            "object_attributes": {
                "iid": 12, "title": "Fix login", "state": "opened",
                "action": "merge",
                "source_branch": "fix/login", "target_branch": "main",
                "last_commit": {"id": "def"},
                "author_id": 7,
                "updated_at": "2026-01-02T03:04:05Z",
            },
        })
        .to_string();
        let (code, _) = gitlab_post(
            &app.app,
            &integration_id,
            SECRET,
            "gl-3",
            "Merge Request Event",
            mr.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let row = run_until_terminal(&app, "w", &integration_id, "gl-3").await;
        assert_eq!(row.status, "processed", "[{engine}]");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] event");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_pull_request_merged"
        );
        drop(event);

        // ---- Release published → git_release_published ----
        let release = json!({
            "user": {"username": "gluser"},
            "object_attributes": {"action": "publish", "tag": "v1.2.0",
                                  "released_at": "2026-01-02T03:04:05Z"},
        })
        .to_string();
        let (code, _) = gitlab_post(
            &app.app,
            &integration_id,
            SECRET,
            "gl-4",
            "Release Event",
            release.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let row = run_until_terminal(&app, "w", &integration_id, "gl-4").await;
        assert_eq!(row.status, "processed", "[{engine}]");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] event");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_release_published"
        );
        drop(event);

        // ---- Recognized-but-unsupported events: durable, no domain event ----
        for (delivery, event_name, payload) in [
            (
                "gl-p1",
                "URL Test Event",
                json!({"zen": "ping"}).to_string(),
            ),
            (
                "gl-p2",
                "Merge Request Event",
                json!({"object_attributes": {"action": "update", "iid": 1}}).to_string(),
            ),
            (
                "gl-p3",
                "Tag Push Event",
                json!({"ref": "refs/tags/v1"}).to_string(),
            ),
        ] {
            let (code, _) = gitlab_post(
                &app.app,
                &integration_id,
                SECRET,
                delivery,
                event_name,
                payload.into_bytes(),
            )
            .await;
            assert_eq!(code, StatusCode::ACCEPTED, "[{engine}] {event_name}");
            let row = run_until_terminal(&app, "w", &integration_id, delivery).await;
            assert_eq!(row.status, "unsupported", "[{engine}] {event_name}");
            assert!(
                fetch_domain_event(&app.db, &row.id).await.is_none(),
                "[{engine}] {event_name} never becomes a domain event"
            );
        }

        // ---- Wrong token through the real verifier → 401, never stored ----
        let (code, _) = gitlab_post(
            &app.app,
            &integration_id,
            "not-the-secret",
            "gl-bad",
            "Push Event",
            json!({"ref": "refs/heads/main"}).to_string().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] wrong token");
        assert!(
            fetch_delivery(&app.db, &integration_id, "gl-bad")
                .await
                .is_none(),
            "[{engine}] rejected delivery never persisted"
        );
    }
}
