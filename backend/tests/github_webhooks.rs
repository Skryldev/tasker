//! GitHub adapter end-to-end tests (Phase 8B, Task 3): the production
//! `GitHubAdapter` through the real intake → worker pipeline with real
//! GitHub webhook headers and payload shapes.
//!
//! Engine: `SQLite` always; PostgreSQL/MySQL deferred to the final Phase 8
//! cross-database CI gate (see `webhooks.rs` for the shared infrastructure
//! coverage, which uses a custom adapter to prove provider-neutrality —
//! this file proves the GitHub adapter itself).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
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

const SECRET: &str = "gh-webhook-secret-1";
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

fn signature(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(body);
    let tag = mac.finalize().into_bytes();
    let hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

/// POST with real GitHub webhook headers.
async fn github_post(
    app: &axum::Router,
    integration_id: &str,
    secret: &str,
    delivery_id: &str,
    event: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let sig = signature(secret, &body);
    let uri = format!("/api/v1/webhooks/github/{integration_id}");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-hub-signature-256", &sig)
                .header("x-github-delivery", delivery_id)
                .header("x-github-event", event)
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

async fn seed_github_integration(app: &TestApp, suffix: &str) -> String {
    let admin_email = format!("gh-{suffix}@test.local");
    let admin_id = seed_user(&app.db, &admin_email, "GH Admin").await;
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
                .body(Body::from(json!({"name": "GHCREW"}).to_string()))
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
            &format!("gh-{suffix}"),
            None,
            Provider::GitHub,
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
        "SELECT event_type, origin, team_id, integration_id, source_delivery_id, new_state \
         FROM domain_events WHERE source_delivery_id = ?",
        args,
    )
    .await
    .expect("fetch domain event")
}

/// Drain claim cycles until the delivery is terminal (see webhooks.rs for
/// the rationale: SQLite contention retries converge on DB truth).
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
async fn github_webhook_end_to_end_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let integration_id = seed_github_integration(&app, &suffix).await;

        // ---- Real push payload → git_push domain event with namespace keys ----
        let push = json!({
            "ref": "refs/heads/feature/login",
            "after": "abc123",
            "deleted": false,
            "sender": {"login": "octocat"},
            "repository": {"id": 1, "full_name": "octocat/tasker"},
            "commits": [{"id": "1"}, {"id": "2"}, {"id": "3"}],
            "head_commit": {"timestamp": "2026-01-02T03:04:05Z"},
        })
        .to_string();
        let (code, _) = github_post(
            &app.app,
            &integration_id,
            SECRET,
            "gh-1",
            "push",
            push.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED, "[{engine}] push accepted");
        let row = run_until_terminal(&app, "w", &integration_id, "gh-1").await;
        assert_eq!(row.status, "processed", "[{engine}]");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] domain event persisted");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_push"
        );
        assert_eq!(event.try_get::<String, _>("origin").unwrap(), "integration");
        let team_id = team_of(&app, &integration_id).await;
        assert_eq!(event.try_get::<String, _>("team_id").unwrap(), team_id);
        let state: Value =
            serde_json::from_str(&event.try_get::<String, _>("new_state").unwrap()).unwrap();
        assert_eq!(state["provider"], "github");
        assert_eq!(state["event_type"], "git_push");
        assert_eq!(state["branch"], "feature/login");
        assert_eq!(state["repository"], "octocat/tasker");
        assert_eq!(state["actor"], "octocat");
        assert_eq!(state["commit_count"], 3);
        assert_eq!(state["head_sha"], "abc123");
        drop(event);

        // ---- PR merged (closed + merged=true) → git_pull_request_merged ----
        let pr = json!({
            "action": "closed",
            "sender": {"login": "merger"},
            "repository": {"full_name": "octocat/tasker"},
            "pull_request": {
                "number": 42, "title": "Add login", "state": "closed",
                "merged": true, "user": {"login": "author"},
                "head": {"ref": "feature/login", "sha": "abc"},
                "base": {"ref": "main"},
                "merged_at": "2026-01-02T04:04:05Z",
            },
        })
        .to_string();
        let (code, _) = github_post(
            &app.app,
            &integration_id,
            SECRET,
            "gh-2",
            "pull_request",
            pr.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let row = run_until_terminal(&app, "w", &integration_id, "gh-2").await;
        assert_eq!(row.status, "processed", "[{engine}]");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] event");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_pull_request_merged",
            "[{engine}] close and merge are never conflated"
        );
        drop(event);

        // ---- Release published → git_release_published ----
        let release = json!({
            "action": "published",
            "sender": {"login": "releaser"},
            "repository": {"full_name": "octocat/tasker"},
            "release": {
                "tag_name": "v2.0.0", "name": "Second", "prerelease": false,
                "published_at": "2026-01-02T05:04:05Z",
            },
        })
        .to_string();
        let (code, _) = github_post(
            &app.app,
            &integration_id,
            SECRET,
            "gh-3",
            "release",
            release.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let row = run_until_terminal(&app, "w", &integration_id, "gh-3").await;
        assert_eq!(row.status, "processed", "[{engine}]");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] event");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_release_published"
        );
        drop(event);

        // ---- Branch created → git_branch_created ----
        let create = json!({
            "ref_type": "branch", "ref": "feature/x",
            "sender": {"login": "octocat"},
            "repository": {"full_name": "octocat/tasker"},
        })
        .to_string();
        let (code, _) = github_post(
            &app.app,
            &integration_id,
            SECRET,
            "gh-4",
            "create",
            create.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let row = run_until_terminal(&app, "w", &integration_id, "gh-4").await;
        assert_eq!(row.status, "processed", "[{engine}]");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] event");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_branch_created"
        );
        drop(event);

        // ---- Recognized-but-unsupported events: durable, no domain event ----
        for (delivery, event_name, payload) in [
            (
                "gh-p1",
                "ping",
                json!({"zen": "Keep it simple."}).to_string(),
            ),
            (
                "gh-p2",
                "pull_request",
                json!({"action": "synchronize", "pull_request": {"number": 1}}).to_string(),
            ),
            (
                "gh-p3",
                "push",
                json!({"ref": "refs/tags/v1.0"}).to_string(),
            ),
        ] {
            let (code, _) = github_post(
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

        // ---- Tampered body through the real verifier → 401, never stored ----
        let body = json!({"ref": "refs/heads/main"}).to_string();
        let sig = signature(SECRET, body.as_bytes());
        let tampered = body.replace("main", "evil");
        let uri = format!("/api/v1/webhooks/github/{integration_id}");
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-hub-signature-256", &sig)
                    .header("x-github-delivery", "gh-tamper")
                    .header("x-github-event", "push")
                    .body(Body::from(tampered.into_bytes()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "[{engine}] tamper"
        );
        assert!(
            fetch_delivery(&app.db, &integration_id, "gh-tamper")
                .await
                .is_none(),
            "[{engine}] tampered delivery never persisted"
        );
    }
}

async fn team_of(app: &TestApp, integration_id: &str) -> String {
    let mut conn = app.db.conn();
    let mut args = tasker::db::Args::new();
    args.s(integration_id);
    conn.fetch_optional("SELECT team_id FROM integrations WHERE id = ?", args)
        .await
        .unwrap()
        .map(|r| r.try_get::<String, _>("team_id").unwrap())
        .unwrap_or_default()
}
