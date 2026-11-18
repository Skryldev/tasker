//! Webhook infrastructure integration tests (Phase 8B, Task 2): intake
//! security, the durable state machine, lease/fencing concurrency, retry and
//! dead-letter policy, audited replay, and end-to-end processing atomicity.
//!
//! Engines: `SQLite` always (temp file). `PostgreSQL` and `MySQL` run when
//! `TASKER_TEST_PG_URL` / `TASKER_TEST_MYSQL_URL` are set (deferred to the
//! final Phase 8 CI gate per the Master Prompt).
//!
//! The `HmacAdapter` used here exercises the real verification boundary —
//! HMAC-SHA256 over the raw body with constant-time comparison, the same
//! scheme the GitHub adapter (Task 3) will implement — through the
//! provider-neutral `ProviderAdapter` contract. It is test code only; no
//! production adapter is registered until Task 3.

use std::collections::HashMap;
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
use tasker::integrations::store;
use tasker::integrations::webhook::delivery::{self, FailureClass};
use tasker::integrations::webhook::worker;
use tasker::integrations::webhook::{
    NormalizedOutcome, ProviderAdapter, ProviderRegistry, RawWebhook, VerificationError,
    VerifiedDelivery,
};
use tasker::rules::events::IntegrationTriggerType;
use tasker::security::token;
use tasker::state::AppState;

/// URLs of additional engines to test (deferred cross-database validation).
fn test_db_urls() -> Vec<(String, String)> {
    let mut urls = vec![(
        "sqlite".to_string(),
        format!(
            "sqlite:{}/{}",
            std::env::temp_dir().display(),
            Uuid::now_v7()
        ),
    )];
    if let Ok(pg) = std::env::var("TASKER_TEST_PG_URL")
        && !pg.is_empty()
    {
        urls.push(("postgres".to_string(), pg));
    }
    if let Ok(mysql) = std::env::var("TASKER_TEST_MYSQL_URL")
        && !mysql.is_empty()
    {
        urls.push(("mysql".to_string(), mysql));
    }
    urls
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
        integrations_key: Some(vec![19u8; 32]),
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

async fn spawn_app_with(url: &str, registry: ProviderRegistry) -> TestApp {
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
    let state = AppState::with_registry((*db).clone(), config, registry);
    TestApp {
        app: build_router(state),
        db,
    }
}

// ---------------------------------------------------------------------------
// Deterministic GitHub-style test adapter (HMAC-SHA256 over the raw body)
// ---------------------------------------------------------------------------

const SECRET: &str = "whsec-test-12345";
const TEST_KEY: &[u8] = &[19u8; 32];

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(body);
    let tag = mac.finalize().into_bytes();
    let hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

struct HmacAdapter;

impl ProviderAdapter for HmacAdapter {
    fn provider(&self) -> Provider {
        Provider::GitHub
    }

    fn verify(
        &self,
        raw: &RawWebhook<'_>,
        secret: &str,
    ) -> Result<VerifiedDelivery, VerificationError> {
        let sig = raw
            .headers
            .get("x-test-signature")
            .and_then(|v| v.to_str().ok())
            .ok_or(VerificationError::MissingSignature)?;
        let hex = sig
            .strip_prefix("sha256=")
            .ok_or(VerificationError::MalformedSignature)?;
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(VerificationError::MalformedSignature);
        }
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
        mac.update(raw.body);
        let tag = mac.finalize().into_bytes();
        let expected_hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        // Constant-time comparison via the established Phase 7 helper.
        if !token::hashes_equal(&hex.to_ascii_lowercase(), &expected_hex) {
            return Err(VerificationError::InvalidSignature);
        }
        let delivery_id = raw
            .headers
            .get("x-test-delivery")
            .and_then(|v| v.to_str().ok())
            .ok_or(VerificationError::InvalidRequest("missing delivery header"))?;
        let event_type_raw = raw
            .headers
            .get("x-test-event")
            .and_then(|v| v.to_str().ok())
            .ok_or(VerificationError::InvalidRequest("missing event header"))?;
        Ok(VerifiedDelivery {
            delivery_id: delivery_id.to_string(),
            event_type_raw: event_type_raw.to_string(),
        })
    }

    fn normalize(&self, event_type_raw: &str, payload: &str) -> NormalizedOutcome {
        match event_type_raw {
            "push" => {
                let parsed: Value = match serde_json::from_str(payload) {
                    Ok(v) => v,
                    Err(_) => {
                        return NormalizedOutcome::Invalid {
                            reason: "payload is not valid JSON",
                        };
                    }
                };
                let branch = parsed
                    .get("branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main")
                    .to_string();
                NormalizedOutcome::Event {
                    event_type: IntegrationTriggerType::GitPush,
                    occurred_at: 1_700_000_000_000,
                    state: json!({"branch": branch, "commit_count": 1}),
                }
            }
            "broken" => NormalizedOutcome::Invalid {
                reason: "test malformed payload",
            },
            _ => NormalizedOutcome::Unsupported {
                reason: "outside the supported test vocabulary",
            },
        }
    }
}

fn registry_with_github() -> ProviderRegistry {
    let mut map = HashMap::new();
    map.insert(
        Provider::GitHub,
        Arc::new(HmacAdapter) as Arc<dyn ProviderAdapter>,
    );
    ProviderRegistry::new(map)
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn to_response(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap_or_default();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn post_webhook(
    app: &axum::Router,
    provider: &str,
    integration_id: &str,
    signature: Option<&str>,
    delivery_id: &str,
    event: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let uri = format!("/api/v1/webhooks/{provider}/{integration_id}");
    let mut builder = Request::builder().method("POST").uri(&uri);
    if let Some(ct) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    if let Some(sig) = signature {
        builder = builder.header("x-test-signature", sig);
    }
    let builder = builder
        .header("x-test-delivery", delivery_id)
        .header("x-test-event", event);
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    to_response(response).await
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

async fn send_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    to_response(response).await
}

// ---------------------------------------------------------------------------
// Fixtures + row readers
// ---------------------------------------------------------------------------

struct Fixture {
    integration_id: String,
    team_id: String,
    admin_id: String,
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

/// One enabled GitHub integration with an active webhook credential.
async fn seed_github_integration(app: &TestApp, suffix: &str) -> Fixture {
    let admin_email = format!("whk-{suffix}@test.local");
    let admin_id = seed_user(&app.db, &admin_email, "Webhook Admin").await;
    let session = login_as(&app.app, &admin_email, "test-password-123").await;
    let (code, team) = send_json(
        &app.app,
        "POST",
        "/api/v1/teams",
        Some(&session),
        json!({"name": "WHK"}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let team_id = team["id"].as_str().unwrap().to_string();
    let integration_id = {
        let mut conn = app.db.conn();
        store::create_integration(
            &mut conn,
            &team_id,
            &format!("hooks-{suffix}"),
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
        // Integrations are created disabled (store contract); enabling is the
        // operator's configure→enable step before the webhook goes live.
        store::set_integration_enabled(&mut conn, &integration_id, true, tasker::util::now_ms())
            .await
            .expect("enable integration");
    }
    Fixture {
        integration_id,
        team_id,
        admin_id,
    }
}

struct DeliveryRow {
    id: String,
    status: String,
    attempts: i64,
    event_type_raw: String,
    claim_token: Option<String>,
    completed_at: Option<i64>,
    next_attempt_at: i64,
    last_error_category: Option<String>,
}

async fn fetch_delivery(
    db: &Database,
    integration_id: &str,
    delivery_id: &str,
) -> Option<DeliveryRow> {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(integration_id).s(delivery_id);
    let row = conn
        .fetch_optional(
            "SELECT id, status, attempts, event_type_raw, claim_token, completed_at, \
                    next_attempt_at, last_error_category \
             FROM webhook_deliveries WHERE integration_id = ? AND delivery_id = ?",
            args,
        )
        .await
        .expect("fetch delivery");
    row.map(|r| DeliveryRow {
        id: r.try_get("id").unwrap(),
        status: r.try_get("status").unwrap(),
        attempts: r.try_get("attempts").unwrap(),
        event_type_raw: r.try_get("event_type_raw").unwrap(),
        claim_token: r.try_get("claim_token").unwrap(),
        completed_at: r.try_get("completed_at").unwrap(),
        next_attempt_at: r.try_get("next_attempt_at").unwrap(),
        last_error_category: r.try_get("last_error_category").unwrap(),
    })
}

async fn fetch_domain_event(db: &Database, delivery_row_id: &str) -> Option<sqlx::any::AnyRow> {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(delivery_row_id);
    conn.fetch_optional(
        "SELECT id, event_type, origin, team_id, task_id, integration_id, \
                source_delivery_id, new_state \
         FROM domain_events WHERE source_delivery_id = ?",
        args,
    )
    .await
    .expect("fetch domain event")
}

/// Run claim cycles until `delivery_id` reaches a terminal state (processed /
/// unsupported / dead_letter) or the bounded loop exhausts. Concurrent
/// processing can hit SQLite's single-writer upgrade deadlock (SELECT →
/// INSERT upgrade); that failure is transient by design — classified
/// retryable with backoff — so deterministic tests drain instead of assuming
/// one-cycle completion. The short backoff base keeps convergence fast.
async fn run_until_terminal(
    app: &TestApp,
    registry: &ProviderRegistry,
    config: &WebhookWorkerConfig,
    metrics: &Metrics,
    semaphore: &Arc<tokio::sync::Semaphore>,
    worker_name: &str,
    integration_id: &str,
    delivery_id: &str,
) -> DeliveryRow {
    let drain_config = WebhookWorkerConfig {
        backoff_base_ms: 50,
        ..config.clone()
    };
    for _ in 0..60 {
        let claimed = worker::run_once(
            &app.db,
            registry,
            &drain_config,
            worker_name,
            metrics,
            semaphore,
        )
        .await
        .unwrap();
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

// ---------------------------------------------------------------------------
// Intake: security + durability + duplicates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_intake_security_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app_with(&url, registry_with_github()).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let fx = seed_github_integration(&app, &suffix).await;
        let body = json!({"branch": "main", "commits": 1}).to_string();
        let ct = "application/json";

        // ---- Happy path: signed POST accepted as a durable pending row ----
        let sig = sign(SECRET, body.as_bytes());
        let (code, resp) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "d-1",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED, "[{engine}] {resp}");
        let row = fetch_delivery(&app.db, &fx.integration_id, "d-1")
            .await
            .expect("[{engine}] durable pending row");
        assert_eq!(row.status, "pending", "[{engine}]");
        assert_eq!(row.event_type_raw, "push", "[{engine}]");
        assert_eq!(row.attempts, 0, "[{engine}]");

        // ---- Duplicate replay: idempotent 200, exactly one row ----
        let (code, resp) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "d-1",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] duplicate: {resp}");
        assert!(
            fetch_delivery(&app.db, &fx.integration_id, "d-1")
                .await
                .is_some()
        );
        let mut conn = app.db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&fx.integration_id).s("d-1");
        let count = conn
            .fetch_optional(
                "SELECT COUNT(*) AS n FROM webhook_deliveries \
                 WHERE integration_id = ? AND delivery_id = ?",
                args,
            )
            .await
            .unwrap()
            .map(|r| r.try_get::<i64, _>("n").unwrap())
            .unwrap_or(0);
        assert_eq!(count, 1, "[{engine}] replay must not create a second row");
        drop(conn);

        // ---- Authentication failures: 401, never persisted ----
        // Wrong secret.
        let wrong = sign("not-the-secret", body.as_bytes());
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&wrong),
            "d-2",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] wrong secret");
        assert!(
            fetch_delivery(&app.db, &fx.integration_id, "d-2")
                .await
                .is_none(),
            "[{engine}] auth failures are never persisted"
        );
        // Missing signature (valid JSON must not bypass verification).
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            None,
            "d-3",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] unsigned refused"
        );
        assert!(
            fetch_delivery(&app.db, &fx.integration_id, "d-3")
                .await
                .is_none()
        );
        // Malformed signature: wrong scheme prefix.
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some("sha1=deadbeef"),
            "d-4",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] wrong scheme");
        // Malformed signature: bad length.
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some("sha256=abcd"),
            "d-5",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] bad length");

        // ---- Size limit: 15,001 bytes refused before persistence ----
        let big = vec![b'a'; 15_001];
        let big_sig = sign(SECRET, &big);
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&big_sig),
            "d-big",
            "push",
            Some(ct),
            big,
        )
        .await;
        assert_eq!(code, StatusCode::PAYLOAD_TOO_LARGE, "[{engine}] over limit");
        assert!(
            fetch_delivery(&app.db, &fx.integration_id, "d-big")
                .await
                .is_none()
        );

        // Oversized body WITHOUT a Content-Length header (the streaming
        // path): the explicit byte-capped read must still refuse it.
        let big = vec![b'a'; 15_001];
        let big_sig = sign(SECRET, &big);
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/webhooks/github/{}", fx.integration_id))
                    .header(header::CONTENT_TYPE, ct)
                    .header("x-test-signature", &big_sig)
                    .header("x-test-delivery", "d-big-nolen")
                    .header("x-test-event", "push")
                    .body(Body::from(big))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "[{engine}] streaming path enforces the byte cap"
        );
        assert!(
            fetch_delivery(&app.db, &fx.integration_id, "d-big-nolen")
                .await
                .is_none()
        );

        // Boundary: exactly 15,000 bytes passes the size limit (intake does
        // not parse the payload; unsupported events are classified by the
        // worker).
        let exact = vec![b'a'; 15_000];
        let exact_sig = sign(SECRET, &exact);
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&exact_sig),
            "d-exact",
            "wiki",
            Some(ct),
            exact,
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED, "[{engine}] at the byte limit");
        assert!(
            fetch_delivery(&app.db, &fx.integration_id, "d-exact")
                .await
                .is_some()
        );

        // ---- Content type: non-JSON refused ----
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "d-ct",
            "push",
            Some("text/plain"),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::UNSUPPORTED_MEDIA_TYPE, "[{engine}] ct");
        assert!(
            fetch_delivery(&app.db, &fx.integration_id, "d-ct")
                .await
                .is_none()
        );

        // ---- Resolution masking: everything unknown is the same 404 ----
        let (code, _) = post_webhook(
            &app.app,
            "gitea",
            &fx.integration_id,
            Some(&sig),
            "d-p",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] unknown provider");
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &Uuid::now_v7().to_string(),
            Some(&sig),
            "d-u",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] unknown integration"
        );
        // Provider mismatch (GitLab path for a GitHub integration).
        let (code, _) = post_webhook(
            &app.app,
            "gitlab",
            &fx.integration_id,
            Some(&sig),
            "d-m",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] provider mismatch");

        // ---- Invalid integration states refuse new deliveries ----
        {
            let mut conn = app.db.conn();
            store::set_integration_enabled(
                &mut conn,
                &fx.integration_id,
                false,
                tasker::util::now_ms(),
            )
            .await
            .unwrap();
        }
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "d-dis",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] disabled → 404");
        {
            let mut conn = app.db.conn();
            store::set_integration_enabled(
                &mut conn,
                &fx.integration_id,
                true,
                tasker::util::now_ms(),
            )
            .await
            .unwrap();
        }
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "d-ren",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED, "[{engine}] re-enabled accepts");
        {
            let mut conn = app.db.conn();
            store::archive_integration(&mut conn, &fx.integration_id, tasker::util::now_ms())
                .await
                .unwrap();
        }
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "d-arc",
            "push",
            Some(ct),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] archived → 404");

        // ---- Rendered metrics expose the intake behavior ----
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, _) = to_response(response).await;
        assert_eq!(status, StatusCode::OK);
        let rendered = String::from_utf8(
            // re-render through a second request to capture the body
            {
                let response = app
                    .app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri("/metrics")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                axum::body::to_bytes(response.into_body(), 1_048_576)
                    .await
                    .unwrap()
                    .to_vec()
            },
        )
        .unwrap();
        assert!(
            rendered.contains("tasker_webhook_intake_accepted_total"),
            "[{engine}] accepted counter rendered"
        );
        assert!(
            rendered.contains("tasker_webhook_intake_duplicate_total"),
            "[{engine}] duplicate counter rendered"
        );
        assert!(
            rendered.contains("reason=\"invalid_signature\""),
            "[{engine}] bounded rejection labels rendered"
        );
        assert!(
            !rendered.contains("d-1"),
            "[{engine}] no delivery identifiers in metrics"
        );
    }
}

/// Concurrent duplicate intake: the UNIQUE constraint is the correctness
/// boundary — exactly one durable row regardless of racing requests.
#[tokio::test]
async fn webhook_concurrent_duplicate_intake() {
    for (engine, url) in test_db_urls() {
        let app = Arc::new(spawn_app_with(&url, registry_with_github()).await);
        let suffix = Uuid::now_v7().simple().to_string();
        let fx = seed_github_integration(&app, &suffix).await;
        let body = json!({"branch": "race", "commits": 1}).to_string();
        let sig = sign(SECRET, body.as_bytes());

        let mut handles = Vec::new();
        for _ in 0..6 {
            let app = app.clone();
            let integration_id = fx.integration_id.clone();
            let sig = sig.clone();
            let body = body.clone();
            handles.push(tokio::spawn(async move {
                post_webhook(
                    &app.app,
                    "github",
                    &integration_id,
                    Some(&sig),
                    "d-race",
                    "push",
                    Some("application/json"),
                    body.into_bytes(),
                )
                .await
            }));
        }
        let mut accepted = 0usize;
        let mut duplicates = 0usize;
        let mut busy = 0usize;
        for handle in handles {
            let (code, _) = handle.await.unwrap();
            match code {
                StatusCode::ACCEPTED => accepted += 1,
                StatusCode::OK => duplicates += 1,
                // SQLite's single-writer lock can transiently refuse a racing
                // insert: a 503 Busy is a correct intake outcome (the
                // provider retries), never a correctness violation.
                StatusCode::SERVICE_UNAVAILABLE => busy += 1,
                other => panic!("[{engine}] unexpected status {other}"),
            }
        }
        assert_eq!(accepted, 1, "[{engine}] exactly one winner");
        assert_eq!(duplicates + busy, 5, "[{engine}] losers converge or retry");
        let row = fetch_delivery(&app.db, &fx.integration_id, "d-race")
            .await
            .expect("[{engine}] one durable row");
        assert_eq!(row.status, "pending");
    }
}

// ---------------------------------------------------------------------------
// Worker: processing atomicity, provenance, unsupported, permanent failures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_worker_processing_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app_with(&url, registry_with_github()).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let fx = seed_github_integration(&app, &suffix).await;
        let registry = registry_with_github();
        let metrics = Metrics::new();
        let config = WebhookWorkerConfig::default();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1)); // sequential processing: SQLite is single-writer (see the benchmark note);
        let ct = "application/json";

        // ---- Happy path: event + completion commit atomically ----
        let body = json!({"branch": "feature/x", "commits": 3}).to_string();
        let sig = sign(SECRET, body.as_bytes());
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "w-1",
            "push",
            Some(ct),
            body.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let claimed = worker::run_once(&app.db, &registry, &config, "w1", &metrics, &semaphore)
            .await
            .unwrap();
        assert_eq!(claimed, 1, "[{engine}] one delivery claimed");
        let row = run_until_terminal(
            &app,
            &registry,
            &config,
            &metrics,
            &semaphore,
            "w1",
            &fx.integration_id,
            "w-1",
        )
        .await;
        assert_eq!(row.status, "processed", "[{engine}]");
        assert!(row.completed_at.is_some(), "[{engine}] terminal stamp");
        assert_eq!(row.claim_token, None, "[{engine}] fence released");
        let event = fetch_domain_event(&app.db, &row.id)
            .await
            .expect("[{engine}] domain event persisted");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_push"
        );
        assert_eq!(event.try_get::<String, _>("origin").unwrap(), "integration");
        assert_eq!(event.try_get::<String, _>("team_id").unwrap(), fx.team_id);
        assert_eq!(
            event.try_get::<String, _>("integration_id").unwrap(),
            fx.integration_id
        );
        assert_eq!(
            event.try_get::<String, _>("source_delivery_id").unwrap(),
            row.id
        );
        assert!(
            event
                .try_get::<Option<String>, _>("task_id")
                .unwrap()
                .is_none(),
            "[{engine}] integration events are taskless"
        );

        // Second cycle: nothing new to claim; no duplicate domain event.
        let claimed = worker::run_once(&app.db, &registry, &config, "w1", &metrics, &semaphore)
            .await
            .unwrap();
        assert_eq!(claimed, 0, "[{engine}] processed deliveries not re-claimed");
        let count = fetch_domain_event(&app.db, &row.id).await;
        assert!(count.is_some(), "[{engine}] event survives");
        drop(event);

        // ---- Unsupported: durable terminal record, no domain event ----
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sign(SECRET, br#"{"page": 1}"#)),
            "w-2",
            "wiki",
            Some(ct),
            br#"{"page": 1}"#.to_vec(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let claimed = worker::run_once(&app.db, &registry, &config, "w1", &metrics, &semaphore)
            .await
            .unwrap();
        assert_eq!(claimed, 1);
        let row = run_until_terminal(
            &app,
            &registry,
            &config,
            &metrics,
            &semaphore,
            "w1",
            &fx.integration_id,
            "w-2",
        )
        .await;
        assert_eq!(row.status, "unsupported", "[{engine}]");
        assert_eq!(
            row.last_error_category.as_deref(),
            Some("unsupported_event"),
            "[{engine}]"
        );
        assert!(
            fetch_domain_event(&app.db, &row.id).await.is_none(),
            "[{engine}] unsupported events never become domain events"
        );

        // ---- Invalid payload: permanent failure → immediate dead letter ----
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sign(SECRET, b"not json")),
            "w-3",
            "push",
            Some(ct),
            b"not json".to_vec(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let claimed = worker::run_once(&app.db, &registry, &config, "w1", &metrics, &semaphore)
            .await
            .unwrap();
        assert_eq!(claimed, 1);
        let row = run_until_terminal(
            &app,
            &registry,
            &config,
            &metrics,
            &semaphore,
            "w1",
            &fx.integration_id,
            "w-3",
        )
        .await;
        assert_eq!(row.status, "dead_letter", "[{engine}] permanent");
        assert_eq!(row.attempts, 1, "[{engine}] one burned attempt");
        assert_eq!(
            row.last_error_category.as_deref(),
            Some("permanent"),
            "[{engine}]"
        );
        assert!(fetch_domain_event(&app.db, &row.id).await.is_none());

        // ---- Fault injection: adapter unregistered between intake and
        // ---- processing (configuration regression) → permanent dead letter.
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sign(SECRET, json!({"branch": "y"}).to_string().as_bytes())),
            "w-4",
            "push",
            Some(ct),
            json!({"branch": "y"}).to_string().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let empty_registry = ProviderRegistry::default();
        let claimed = worker::run_once(
            &app.db,
            &empty_registry,
            &config,
            "w1",
            &metrics,
            &semaphore,
        )
        .await
        .unwrap();
        assert_eq!(claimed, 1, "[{engine}] claim still happens");
        let row = run_until_terminal(
            &app,
            &empty_registry,
            &config,
            &metrics,
            &semaphore,
            "w1",
            &fx.integration_id,
            "w-4",
        )
        .await;
        assert_eq!(row.status, "dead_letter", "[{engine}] adapter missing");
        assert_eq!(
            row.last_error_category.as_deref(),
            Some("permanent"),
            "[{engine}]"
        );
    }
}

/// Integration state mid-flight: already-accepted deliveries are durable and
/// still process after the integration is disabled or archived (they were
/// authenticated at intake); new deliveries are refused.
#[tokio::test]
async fn webhook_integration_state_during_processing() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app_with(&url, registry_with_github()).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let fx = seed_github_integration(&app, &suffix).await;
        let registry = registry_with_github();
        let metrics = Metrics::new();
        let config = WebhookWorkerConfig::default();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1)); // sequential processing: SQLite is single-writer (see the benchmark note);
        let body = json!({"branch": "main"}).to_string();
        let sig = sign(SECRET, body.as_bytes());

        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "mid-1",
            "push",
            Some("application/json"),
            body.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);

        // Archive BEFORE processing: the accepted delivery still processes.
        {
            let mut conn = app.db.conn();
            store::archive_integration(&mut conn, &fx.integration_id, tasker::util::now_ms())
                .await
                .unwrap();
        }
        let claimed = worker::run_once(&app.db, &registry, &config, "w1", &metrics, &semaphore)
            .await
            .unwrap();
        assert_eq!(claimed, 1, "[{engine}] accepted deliveries are durable");
        let row = run_until_terminal(
            &app,
            &registry,
            &config,
            &metrics,
            &semaphore,
            "w1",
            &fx.integration_id,
            "mid-1",
        )
        .await;
        assert_eq!(row.status, "processed", "[{engine}]");

        // New deliveries are refused after archival.
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            "mid-2",
            "push",
            Some("application/json"),
            json!({"branch": "main"}).to_string().into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] archived refuses");
    }
}

// ---------------------------------------------------------------------------
// Claim / lease / fencing / retry / dead-letter / replay (fault injection)
// ---------------------------------------------------------------------------

async fn insert_raw_delivery(
    app: &TestApp,
    integration_id: &str,
    delivery_id: &str,
    event: &str,
    payload: &str,
) -> String {
    let mut conn = app.db.conn();
    let accepted = store::insert_delivery(
        &mut conn,
        integration_id,
        delivery_id,
        event,
        payload,
        tasker::util::now_ms(),
    )
    .await
    .expect("insert delivery");
    assert!(accepted, "delivery id must be fresh");
    fetch_delivery(&app.db, integration_id, delivery_id)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn webhook_claim_lease_fencing_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app_with(&url, registry_with_github()).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let fx = seed_github_integration(&app, &suffix).await;
        let metrics = Metrics::new();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1)); // sequential processing: SQLite is single-writer (see the benchmark note);
        let payload = json!({"branch": "main"}).to_string();

        for id in ["c-1", "c-2", "c-3"] {
            insert_raw_delivery(&app, &fx.integration_id, id, "push", &payload).await;
        }
        let config = WebhookWorkerConfig {
            max_attempts: 2,
            ..WebhookWorkerConfig::default()
        };
        let now = tasker::util::now_ms();

        // ---- Concurrent claims are disjoint: each row claimed once ----
        let w1 = {
            let mut conn = app.db.conn();
            delivery::claim_batch(&mut conn, "w1", 10, 60_000, now)
                .await
                .unwrap()
        };
        let w2 = {
            let mut conn = app.db.conn();
            delivery::claim_batch(&mut conn, "w2", 10, 60_000, now)
                .await
                .unwrap()
        };
        assert_eq!(w1.len(), 3, "[{engine}] first worker takes the backlog");
        assert!(
            w2.is_empty(),
            "[{engine}] claimed rows are not re-claimable"
        );
        for claim in &w1 {
            assert_eq!(
                claim.claim_token.len() > 10,
                true,
                "[{engine}] fenced token"
            );
            assert_eq!(
                claim.team_id, fx.team_id,
                "[{engine}] tenant scope from the database"
            );
        }

        // ---- Stale-worker fencing: the loser cannot commit anything ----
        let stale = &w1[0];
        // Expire the lease, recover, and re-claim as a new worker.
        let later = now + 60_000 + 1;
        {
            let mut conn = app.db.conn();
            let recovered = delivery::recover_expired_leases(&mut conn, later)
                .await
                .unwrap();
            assert_eq!(recovered, 3, "[{engine}] all expired claims recovered");
        }
        // Recovery did NOT burn attempts.
        let row = fetch_delivery(&app.db, &fx.integration_id, "c-1")
            .await
            .unwrap();
        assert_eq!(row.status, "pending");
        assert_eq!(row.attempts, 0, "[{engine}] crashes do not burn budget");

        let w3 = {
            let mut conn = app.db.conn();
            delivery::claim_batch(&mut conn, "w3", 10, 60_000, later)
                .await
                .unwrap()
        };
        assert_eq!(w3.len(), 3, "[{engine}] new worker re-claims everything");
        let current = w3
            .iter()
            .find(|c| c.delivery_row_id == stale.delivery_row_id)
            .expect("[{engine}] re-claimed");
        assert_ne!(
            current.claim_token, stale.claim_token,
            "[{engine}] new claim generation"
        );

        // The stale worker's whole transaction must roll back: event insert
        // + fenced completion under the OLD token.
        let committed_by_stale = {
            let mut tx = app.db.begin().await.unwrap();
            delivery::insert_integration_domain_event(
                &mut tx,
                stale,
                IntegrationTriggerType::GitPush,
                tasker::util::now_ms(),
                r#"{"branch":"stale"}"#,
            )
            .await
            .unwrap();
            let complete = delivery::complete_processed(
                &mut tx,
                &stale.delivery_row_id,
                &stale.claim_token,
                tasker::util::now_ms(),
            )
            .await;
            if complete.is_ok() {
                tx.commit().await.unwrap();
                true
            } else {
                tx.rollback().await;
                false
            }
        };
        assert!(
            !committed_by_stale,
            "[{engine}] stale token must not commit"
        );
        assert!(
            fetch_domain_event(&app.db, &stale.delivery_row_id)
                .await
                .is_none(),
            "[{engine}] the stale event insert rolled back with the fence"
        );

        // The current owner processes cleanly: w3's claims expire, recovery
        // re-pends them, and a fresh claim cycle processes all three. (w3's
        // claims are `processing`, so run_once — which claims fresh rows —
        // must not touch them until the lease cycle.)
        let later2 = later + 60_000 + 1;
        {
            let mut conn = app.db.conn();
            let recovered = delivery::recover_expired_leases(&mut conn, later2)
                .await
                .unwrap();
            assert_eq!(recovered, 3, "[{engine}] re-pend for the owner cycle");
        }
        // Concurrent processing can hit SQLite's single-writer upgrade
        // deadlock (SELECT → INSERT upgrade): transient by design, classified
        // retryable, and converged by the next cycle. Drain with bounded
        // cycles AND sleeps, converging on DATABASE TRUTH (all three rows
        // terminal) rather than claim counts — a contention-retried claim is
        // not a completion. The short backoff base keeps the test fast.
        let drain_config = WebhookWorkerConfig {
            backoff_base_ms: 50,
            ..config.clone()
        };
        let registry = registry_with_github();
        for _ in 0..60 {
            let claimed = worker::run_once(
                &app.db,
                &registry,
                &drain_config,
                "w4",
                &metrics,
                &semaphore,
            )
            .await
            .unwrap();
            let all_terminal = [
                fetch_delivery(&app.db, &fx.integration_id, "c-1").await,
                fetch_delivery(&app.db, &fx.integration_id, "c-2").await,
                fetch_delivery(&app.db, &fx.integration_id, "c-3").await,
            ]
            .into_iter()
            .all(|r| r.is_some_and(|row| row.status == "processed"));
            if all_terminal {
                break;
            }
            if claimed == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
        for id in ["c-1", "c-2", "c-3"] {
            let row = fetch_delivery(&app.db, &fx.integration_id, id)
                .await
                .unwrap_or_else(|| panic!("[{engine}] {id} exists"));
            assert_eq!(row.status, "processed", "[{engine}] {id} processed");
        }
        let event = fetch_domain_event(&app.db, &stale.delivery_row_id)
            .await
            .expect("[{engine}] exactly one event, by the owner");
        assert_eq!(
            event.try_get::<String, _>("new_state").unwrap(),
            r#"{"branch":"main","commit_count":1}"#
        );
        drop(event);

        // ---- Retry scheduling: backoff, due gating, exhaustion ----
        insert_raw_delivery(&app, &fx.integration_id, "r-1", "push", &payload).await;
        let r1_row_id = fetch_delivery(&app.db, &fx.integration_id, "r-1")
            .await
            .unwrap()
            .id;
        let mut claim = {
            let mut conn = app.db.conn();
            delivery::claim_batch(&mut conn, "wr", 10, 60_000, tasker::util::now_ms())
                .await
                .unwrap()
                .into_iter()
                .find(|c| c.delivery_row_id == r1_row_id)
                .expect("[{engine}] r-1 claimed")
        };
        // First failure: rescheduled with full-jitter backoff (base 1s).
        let t0 = tasker::util::now_ms();
        let disposition = {
            let mut conn = app.db.conn();
            delivery::record_failure(
                &mut conn,
                &claim,
                FailureClass::Retryable,
                &tasker::error::AppError::internal("transient database hiccup"),
                config.max_attempts,
                1_000,
                60_000,
                1_000, // full jitter → deterministic delay
                i64::try_from(config.max_delivery_age_ms).unwrap(),
                t0,
            )
            .await
            .unwrap()
        };
        assert_eq!(disposition, delivery::FailureDisposition::Rescheduled);
        let row = fetch_delivery(&app.db, &fx.integration_id, "r-1")
            .await
            .unwrap();
        assert_eq!(row.status, "retryable_failed");
        assert_eq!(row.attempts, 1);
        assert_eq!(row.next_attempt_at, t0 + 1_000, "[{engine}] base backoff");
        // Not due before the backoff elapses.
        let not_due = {
            let mut conn = app.db.conn();
            delivery::claim_batch(&mut conn, "wr", 10, 60_000, t0 + 999)
                .await
                .unwrap()
        };
        assert!(
            !not_due.iter().any(|c| c.delivery_row_id == row.id),
            "[{engine}] backoff gates the retry"
        );
        // Due afterwards: re-claimed with the attempt count carried.
        claim = {
            let mut conn = app.db.conn();
            delivery::claim_batch(&mut conn, "wr", 10, 60_000, t0 + 1_000)
                .await
                .unwrap()
                .into_iter()
                .find(|c| c.delivery_row_id == row.id)
                .expect("[{engine}] r-1 due again")
        };
        assert_eq!(
            claim.attempts, 1,
            "[{engine}] attempts carried into the retry"
        );
        // Second failure exhausts max_attempts = 2 → dead letter.
        let disposition = {
            let mut conn = app.db.conn();
            delivery::record_failure(
                &mut conn,
                &claim,
                FailureClass::Retryable,
                &tasker::error::AppError::internal("still failing"),
                config.max_attempts,
                1_000,
                60_000,
                1_000,
                i64::try_from(config.max_delivery_age_ms).unwrap(),
                tasker::util::now_ms(),
            )
            .await
            .unwrap()
        };
        assert_eq!(disposition, delivery::FailureDisposition::DeadLettered);
        let row = fetch_delivery(&app.db, &fx.integration_id, "r-1")
            .await
            .unwrap();
        assert_eq!(row.status, "dead_letter");
        assert_eq!(row.attempts, 2);
        assert_eq!(row.last_error_category.as_deref(), Some("exhausted"));

        // ---- Maximum retry age: an old delivery dead-letters as expired ----
        insert_raw_delivery(&app, &fx.integration_id, "age-1", "push", &payload).await;
        let ancient = tasker::util::now_ms() - (config.max_delivery_age_ms as i64) - 1_000;
        {
            let mut conn = app.db.conn();
            let mut args = tasker::db::Args::new();
            args.i(ancient)
                .s(&fetch_delivery(&app.db, &fx.integration_id, "age-1")
                    .await
                    .unwrap()
                    .id);
            conn.execute(
                "UPDATE webhook_deliveries SET created_at = ? WHERE id = ?",
                args,
            )
            .await
            .unwrap();
        }
        let claim = {
            let mut conn = app.db.conn();
            let row_id = fetch_delivery(&app.db, &fx.integration_id, "age-1")
                .await
                .unwrap()
                .id;
            delivery::claim_batch(&mut conn, "wr", 10, 60_000, tasker::util::now_ms())
                .await
                .unwrap()
                .into_iter()
                .find(|c| c.delivery_row_id == row_id)
                .expect("[{engine}] age-1 claimed")
        };
        let disposition = {
            let mut conn = app.db.conn();
            delivery::record_failure(
                &mut conn,
                &claim,
                FailureClass::Retryable,
                &tasker::error::AppError::internal("transient"),
                config.max_attempts,
                1_000,
                60_000,
                1_000,
                i64::try_from(config.max_delivery_age_ms).unwrap(),
                tasker::util::now_ms(),
            )
            .await
            .unwrap()
        };
        assert_eq!(disposition, delivery::FailureDisposition::DeadLettered);
        let age_row = fetch_delivery(&app.db, &fx.integration_id, "age-1")
            .await
            .unwrap();
        assert_eq!(age_row.last_error_category.as_deref(), Some("expired"));

        // ---- Audited manual replay: dead_letter → pending, history kept ----
        // Target r-1's row explicitly (dead-lettered by exhaustion above).
        let dead_row = fetch_delivery(&app.db, &fx.integration_id, "r-1")
            .await
            .unwrap();
        assert_eq!(dead_row.status, "dead_letter", "[{engine}] pre-replay");
        let replays_before = replay_count(&app.db, &dead_row.id).await;
        assert_eq!(replays_before, 0);
        let replayed = {
            let mut tx = app.db.begin().await.unwrap();
            let ok = delivery::replay_dead_letter(
                &mut tx,
                &dead_row.id,
                &fx.admin_id,
                Some("operator retry after fix"),
                tasker::util::now_ms(),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            ok
        };
        assert!(replayed, "[{engine}] dead_letter replay accepted");
        let row = fetch_delivery(&app.db, &fx.integration_id, "r-1")
            .await
            .unwrap();
        assert_eq!(row.status, "pending", "[{engine}] reopened");
        assert_eq!(row.attempts, 0, "[{engine}] fresh retry budget");
        assert_eq!(
            row.last_error_category, None,
            "[{engine}] diagnostics reset"
        );
        // The audit record preserves the pre-replay diagnostics.
        let (prior_attempts, prior_category, reason) = replay_record(&app.db, &row.id)
            .await
            .expect("[{engine}] audited");
        assert_eq!(prior_attempts, 2);
        assert_eq!(prior_category.as_deref(), Some("exhausted"));
        assert_eq!(reason.as_deref(), Some("operator retry after fix"));
        // A second replay is refused (no longer dead_letter).
        let replayed_again = {
            let mut tx = app.db.begin().await.unwrap();
            let ok = delivery::replay_dead_letter(
                &mut tx,
                &row.id,
                &fx.admin_id,
                None,
                tasker::util::now_ms(),
            )
            .await
            .unwrap();
            tx.rollback().await;
            ok
        };
        assert!(!replayed_again, "[{engine}] pending is not replayable");
        // Terminal `processed` is never reopenable.
        let processed_row = fetch_delivery(&app.db, &fx.integration_id, "c-1")
            .await
            .unwrap();
        assert_eq!(processed_row.status, "processed");
        let replayed_processed = {
            let mut tx = app.db.begin().await.unwrap();
            let ok = delivery::replay_dead_letter(
                &mut tx,
                &processed_row.id,
                &fx.admin_id,
                None,
                tasker::util::now_ms(),
            )
            .await
            .unwrap();
            tx.rollback().await;
            ok
        };
        assert!(!replayed_processed, "[{engine}] processed is terminal");
        // The replayed delivery processes again end-to-end.
        let claimed = worker::run_once(
            &app.db,
            &registry_with_github(),
            &config,
            "wr",
            &metrics,
            &semaphore,
        )
        .await
        .unwrap();
        assert_eq!(claimed, 1, "[{engine}] replayed delivery claimed");
        let row = fetch_delivery(&app.db, &fx.integration_id, "r-1")
            .await
            .unwrap();
        assert_eq!(row.status, "processed", "[{engine}] replay converged");
        let event = fetch_domain_event(&app.db, &row.id).await.unwrap();
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "git_push",
            "[{engine}] one event per delivery even across replay"
        );
    }
}

async fn replay_count(db: &Database, delivery_row_id: &str) -> i64 {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(delivery_row_id);
    conn.fetch_optional(
        "SELECT COUNT(*) AS n FROM webhook_delivery_replays WHERE delivery_id = ?",
        args,
    )
    .await
    .unwrap()
    .map(|r| r.try_get::<i64, _>("n").unwrap())
    .unwrap_or(0)
}

async fn replay_record(
    db: &Database,
    delivery_row_id: &str,
) -> Option<(i64, Option<String>, Option<String>)> {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(delivery_row_id);
    conn.fetch_optional(
        "SELECT prior_attempts, prior_error_category, reason \
         FROM webhook_delivery_replays WHERE delivery_id = ? ORDER BY created_at DESC LIMIT 1",
        args,
    )
    .await
    .unwrap()
    .map(|r| {
        (
            r.try_get("prior_attempts").unwrap(),
            r.try_get("prior_error_category").unwrap(),
            r.try_get("reason").unwrap(),
        )
    })
}

// ---------------------------------------------------------------------------
// Benchmark (measured, not claimed): intake + end-to-end processing
// ---------------------------------------------------------------------------

/// Run with `cargo test --test webhooks webhooks_throughput_benchmark
/// -- --ignored --nocapture`. Measures in-process intake throughput and
/// worker drain throughput on the local engine (SQLite temp file).
#[tokio::test]
#[ignore = "throughput benchmark; run explicitly with --ignored --nocapture"]
async fn webhooks_throughput_benchmark() {
    let (engine, url) = test_db_urls().remove(0);
    let app = spawn_app_with(&url, registry_with_github()).await;
    let suffix = Uuid::now_v7().simple().to_string();
    let fx = seed_github_integration(&app, &suffix).await;
    let registry = registry_with_github();
    let metrics = Metrics::new();
    let config = WebhookWorkerConfig::default();
    // SQLite is single-writer: concurrent processing transactions contend on
    // the write lock and converge through the retry policy (measured
    // separately: concurrency 4 completes all 200 with ~315 claim cycles —
    // no losses, but throughput is dominated by backoff). For a throughput
    // measurement on this engine, process sequentially; concurrent numbers
    // on PostgreSQL/MySQL arrive with the final Phase 8 CI gate.
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    const N: usize = 200;

    let intake_start = std::time::Instant::now();
    for i in 0..N {
        let body = json!({"branch": format!("bench-{i}"), "commits": 1}).to_string();
        let sig = sign(SECRET, body.as_bytes());
        let (code, _) = post_webhook(
            &app.app,
            "github",
            &fx.integration_id,
            Some(&sig),
            &format!("bench-{i}"),
            "push",
            Some("application/json"),
            body.into_bytes(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
    }
    let intake_elapsed = intake_start.elapsed();

    let mut claims = 0usize;
    let process_start = std::time::Instant::now();
    let mut processed;
    loop {
        let claimed = worker::run_once(&app.db, &registry, &config, "bench", &metrics, &semaphore)
            .await
            .unwrap_or(0);
        claims += claimed;
        processed = {
            let mut conn = app.db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&fx.integration_id);
            conn.fetch_optional(
                "SELECT COUNT(*) AS n FROM webhook_deliveries \
                 WHERE integration_id = ? AND status = 'processed'",
                args,
            )
            .await
            .unwrap()
            .map(|r| r.try_get::<i64, _>("n").unwrap())
            .unwrap_or(0) as usize
        };
        if processed == N {
            break;
        }
        if claimed == 0 {
            // Backoff may still be running on contention-retried deliveries.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if process_start.elapsed() > std::time::Duration::from_secs(120) {
            break;
        }
    }
    let process_elapsed = process_start.elapsed();

    println!(
        "[{engine}] intake: {N} signed deliveries in {intake_elapsed:?} ({:.0}/s)",
        N as f64 / intake_elapsed.as_secs_f64()
    );
    println!(
        "[{engine}] worker: {processed}/{N} deliveries durably processed in {process_elapsed:?} ({:.0}/s); {claims} claim cycles including contention retries",
        processed as f64 / process_elapsed.as_secs_f64()
    );
    assert_eq!(processed, N, "all benchmark deliveries processed");
}
