//! Taskless automation end-to-end tests (Phase 8B, Task 5): integration
//! events evaluate through the shared Rules Engine — matched rules persist
//! executions and action intents with `task_id NULL` in the webhook
//! worker's processing transaction, and the Rules dispatcher executes them
//! with action compatibility decided at execution time (explicit-recipient
//! notifications run tasklessly; task-scoped actions dead-letter
//! permanently with a diagnosable category).

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
use tasker::integrations::webhook::worker;
use tasker::integrations::webhook::{
    NormalizedOutcome, ProviderAdapter, ProviderRegistry, RawWebhook, VerificationError,
    VerifiedDelivery,
};
use tasker::rules::events::IntegrationTriggerType;
use tasker::security::token;
use tasker::state::AppState;

const SECRET: &str = "whsec-taskless-1";
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

async fn spawn_app(url: &str, registry: ProviderRegistry) -> TestApp {
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

/// The same HMAC adapter scheme as webhooks.rs: HMAC-SHA256 over the raw
/// body via the provider-neutral contract, mapped onto the push trigger.
struct PushAdapter;

impl ProviderAdapter for PushAdapter {
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
                let parsed: Value = serde_json::from_str(payload)
                    .map_err(|_| ())
                    .map_err(|_| NormalizedOutcome::Invalid {
                        reason: "payload is not valid JSON",
                    })
                    .unwrap_or(Value::Null);
                let branch = parsed
                    .get("branch")
                    .and_then(Value::as_str)
                    .unwrap_or("main")
                    .to_string();
                NormalizedOutcome::Event {
                    event_type: IntegrationTriggerType::GitPush,
                    occurred_at: 0,
                    state: json!({
                        "provider": "github",
                        "event_type": "git_push",
                        "branch": branch,
                        "repository": "octocat/tasker",
                        "actor": "octocat",
                        "commit_count": 1,
                    }),
                }
            }
            _ => NormalizedOutcome::Unsupported {
                reason: "unmapped test event",
            },
        }
    }
}

fn registry() -> ProviderRegistry {
    let mut map = std::collections::HashMap::new();
    map.insert(
        Provider::GitHub,
        Arc::new(PushAdapter) as Arc<dyn ProviderAdapter>,
    );
    ProviderRegistry::new(map)
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

struct Fixture {
    integration_id: String,
    team_id: String,
    admin_id: String,
}

async fn seed_fixture(app: &TestApp, suffix: &str) -> Fixture {
    let admin_email = format!("tl-{suffix}@test.local");
    let admin_id = seed_user(&app.db, &admin_email, "Taskless Admin").await;
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
                .body(Body::from(json!({"name": "TL"}).to_string()))
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
            &format!("tl-{suffix}"),
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
    Fixture {
        integration_id,
        team_id,
        admin_id,
    }
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

/// Seed one enabled rule row + immutable version directly (the rules
/// management API is Task 6; Task 5 proves the evaluation pipeline).
async fn seed_integration_rule(
    app: &TestApp,
    team_id: &str,
    created_by: &str,
    name: &str,
    condition_json: &str,
    actions_json: &str,
) -> String {
    let rule_id = Uuid::now_v7().to_string();
    let mut conn = app.db.conn();
    let now = tasker::util::now_ms();
    let mut args = tasker::db::Args::new();
    args.s(&rule_id)
        .s(team_id)
        .s(name)
        .opt_s(None::<String>) // description
        .s("git_push")
        .b(true)
        .i(1i64)
        .s(condition_json)
        .s(actions_json)
        .s(created_by)
        .i(now)
        .i(now)
        .opt_i(None::<i64>); // archived_at
    conn.execute(
        "INSERT INTO rules (id, team_id, name, description, trigger_type, enabled, \
         current_version, condition_json, actions_json, created_by, created_at, updated_at, \
         archived_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await
    .unwrap_or_else(|e| panic!("rule insert: {e}"));
    let mut args = tasker::db::Args::new();
    args.s(&rule_id)
        .i(1i64)
        .s("git_push")
        .s(condition_json)
        .s(actions_json)
        .s(created_by)
        .i(now);
    conn.execute(
        "INSERT INTO rule_versions (rule_id, version, trigger_type, condition_json, \
         actions_json, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await
    .unwrap_or_else(|e| panic!("rule version insert: {e}"));
    rule_id
}

async fn fetch_delivery_status(
    db: &Database,
    integration_id: &str,
    delivery_id: &str,
) -> Option<(String, String)> {
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
    .map(|r| {
        (
            r.try_get::<String, _>("id").unwrap(),
            r.try_get::<String, _>("status").unwrap(),
        )
    })
}

#[tokio::test]
async fn taskless_automation_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url, registry()).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let fx = seed_fixture(&app, &suffix).await;
        let registry = registry();
        let metrics = Metrics::new();
        let config = WebhookWorkerConfig::default();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1)); // single-writer engine

        // Two rules on git_push: one taskless-compatible (explicit-recipient
        // notification), one task-scoped (add_comment — must dead-letter).
        let notify_rule = seed_integration_rule(
            &app,
            &fx.team_id,
            &fx.admin_id,
            &format!("notify-{suffix}"),
            r#"{"field":"integration.branch","op":"EQ","value":"feature/login"}"#,
            &json!([
                {"recipients": {"selectors": [], "users": [fx.admin_id.clone()]}}
            ])
            .to_string(),
        )
        .await;
        let comment_rule = seed_integration_rule(
            &app,
            &fx.team_id,
            &fx.admin_id,
            &format!("comment-{suffix}"),
            r#"{"field":"integration.provider","op":"EQ","value":"github"}"#,
            &json!([
                {"body": "does not belong anywhere"}
            ])
            .to_string(),
        )
        .await;

        // GitHub push to the matching branch, through the real pipeline.
        let push = json!({
            "branch": "feature/login",
            "commits": 1,
        })
        .to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(push.as_bytes());
        let tag = mac.finalize().into_bytes();
        let sig = format!(
            "sha256={}",
            tag.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        let (code, _) = to_response(
            app.app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/v1/webhooks/github/{}", fx.integration_id))
                        .header(header::CONTENT_TYPE, "application/json")
                        .header("x-test-signature", &sig)
                        .header("x-test-delivery", "taskless-1")
                        .header("x-test-event", "push")
                        .body(Body::from(push.into_bytes()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED, "[{engine}] push accepted");

        // Drain to terminal: event + evaluations + completion commit atomically.
        for _ in 0..60 {
            let claimed = worker::run_once(&app.db, &registry, &config, "w", &metrics, &semaphore)
                .await
                .unwrap_or(0);
            if claimed == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                if fetch_delivery_status(&app.db, &fx.integration_id, "taskless-1")
                    .await
                    .is_some_and(|(_, status)| status == "processed")
                {
                    break;
                }
            }
        }
        let (delivery_row_id, status) =
            fetch_delivery_status(&app.db, &fx.integration_id, "taskless-1")
                .await
                .unwrap();
        assert_eq!(status, "processed", "[{engine}]");

        // The event evaluated: execution + intent with task_id NULL.
        let mut conn = app.db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&notify_rule).s(&delivery_row_id);
        let execution = conn
            .fetch_optional(
                "SELECT e.task_id AS exec_task, o.task_id AS outbox_task, \
                        o.status AS outbox_status \
                 FROM rule_executions e \
                 LEFT JOIN automation_outbox o ON o.execution_id = e.id \
                 WHERE e.rule_id = ? AND e.event_id = \
                 (SELECT id FROM domain_events WHERE source_delivery_id = ?)",
                args,
            )
            .await
            .unwrap();
        let exec = execution.expect("[{engine}] notify rule evaluated");
        assert!(
            exec.try_get::<Option<String>, _>("exec_task")
                .unwrap()
                .is_none(),
            "[{engine}] integration executions are taskless"
        );
        assert!(
            exec.try_get::<Option<String>, _>("outbox_task")
                .unwrap()
                .is_none(),
            "[{engine}] taskless intent"
        );
        assert_eq!(
            exec.try_get::<String, _>("outbox_status").unwrap(),
            "pending",
            "[{engine}] intent queued"
        );

        // The task-scoped rule also evaluated (its intent will dead-letter).
        let mut args = tasker::db::Args::new();
        args.s(&comment_rule).s(&delivery_row_id);
        let comment_exec = conn
            .fetch_optional(
                "SELECT e.id FROM rule_executions e \
                 WHERE e.rule_id = ? AND e.event_id = \
                 (SELECT id FROM domain_events WHERE source_delivery_id = ?)",
                args,
            )
            .await
            .unwrap();
        assert!(comment_exec.is_some(), "[{engine}] comment rule evaluated");
        drop(conn);

        // ---- Rules dispatcher executes the taskless intents ----
        let rules_config = tasker::config::RulesWorkerConfig {
            batch_size: 16,
            max_concurrent: 1, // single-writer engine
            ..tasker::config::RulesWorkerConfig::default()
        };
        let rules_metrics = Metrics::new();
        let rules_semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        for _ in 0..10 {
            let claimed = tasker::rules::dispatcher::run_once(
                &app.db,
                &rules_config,
                "rules-w",
                &rules_metrics,
                &rules_semaphore,
            )
            .await
            .unwrap_or(0);
            if claimed == 0 {
                let pending = {
                    let mut conn = app.db.conn();
                    conn.fetch_optional(
                        "SELECT COUNT(*) AS n FROM automation_outbox \
                         WHERE status IN ('pending', 'processing', 'retryable_failed')",
                        tasker::db::Args::new(),
                    )
                    .await
                    .unwrap()
                    .map(|r| r.try_get::<i64, _>("n").unwrap())
                    .unwrap_or(0)
                };
                if pending == 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }

        // The notification executed tasklessly: task_id NULL, recipient got it.
        let mut conn = app.db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&fx.admin_id);
        let notified = conn
            .fetch_optional(
                "SELECT id FROM notifications \
                 WHERE recipient_id = ? AND event = 'automation_triggered' AND task_id IS NULL",
                args,
            )
            .await
            .unwrap();
        assert!(
            notified.is_some(),
            "[{engine}] taskless notification created"
        );

        // The task-scoped intent dead-lettered permanently, diagnosably.
        let dead = conn
            .fetch_optional(
                "SELECT status, last_error_category, last_error_detail \
                 FROM automation_outbox WHERE action_type = 'add_comment'",
                tasker::db::Args::new(),
            )
            .await
            .unwrap()
            .expect("[{engine}] comment intent exists");
        assert_eq!(
            dead.try_get::<String, _>("status").unwrap(),
            "dead_letter",
            "[{engine}] task-scoped action on a taskless intent is terminal"
        );
        assert_eq!(
            dead.try_get::<String, _>("last_error_category").unwrap(),
            "permanent",
            "[{engine}]"
        );
        let detail = dead.try_get::<String, _>("last_error_detail").unwrap();
        assert!(
            detail.contains("requires a task context"),
            "[{engine}] diagnosable: {detail}"
        );
        drop(dead);

        // Replay protection: one execution per (rule, event) — the delivery
        // is terminal, so no re-evaluation can happen.
        let mut args = tasker::db::Args::new();
        args.s(&notify_rule);
        let executions = conn
            .fetch_optional(
                "SELECT COUNT(*) AS n FROM rule_executions WHERE rule_id = ?",
                args,
            )
            .await
            .unwrap()
            .map(|r| r.try_get::<i64, _>("n").unwrap())
            .unwrap_or(0);
        assert_eq!(executions, 1, "[{engine}] one execution per (rule, event)");
    }
}
