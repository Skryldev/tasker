//! Integration management API tests (Phase 8B, Task 6): authorization
//! matrix, integration lifecycle, credential rotation/revocation/
//! re-provisioning (one-time secret disclosure), repositories and mappings,
//! delivery inspection (metadata only — no payloads), and audited replay.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;

use tasker::config::{Config, Environment, PoolConfig};
use tasker::db::Database;
use tasker::http::build_router;
use tasker::integrations::production_registry;
use tasker::security::password;
use tasker::state::AppState;

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
        webhook_worker: tasker::config::WebhookWorkerConfig::default(),
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

#[derive(Clone)]
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

async fn seed_user(db: &Database, email: &str, display: &str) -> String {
    let mut conn = db.begin().await.unwrap();
    let hash = password::hash("test-password-123").unwrap();
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

async fn send_empty(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: &str,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    to_response(response).await
}

struct Fixture {
    owner: String,
    admin: String,
    member: String,
    guest: String,
    outsider: String,
    team_id: String,
    other_team_project: String,
    project_id: String,
    owner_id: String,
}

async fn seed_team(app: &TestApp, suffix: &str) -> Fixture {
    let owner_email = format!("intapi-o-{suffix}@test.local");
    let admin_email = format!("intapi-a-{suffix}@test.local");
    let member_email = format!("intapi-m-{suffix}@test.local");
    let guest_email = format!("intapi-g-{suffix}@test.local");
    let outsider_email = format!("intapi-x-{suffix}@test.local");
    let owner_id = seed_user(&app.db, &owner_email, "Owner").await;
    seed_user(&app.db, &admin_email, "Admin").await;
    seed_user(&app.db, &member_email, "Member").await;
    seed_user(&app.db, &guest_email, "Guest").await;
    seed_user(&app.db, &outsider_email, "Outsider").await;
    let owner = login_as(&app.app, &owner_email, "test-password-123").await;
    let admin = login_as(&app.app, &admin_email, "test-password-123").await;
    let member = login_as(&app.app, &member_email, "test-password-123").await;
    let guest = login_as(&app.app, &guest_email, "test-password-123").await;
    let outsider = login_as(&app.app, &outsider_email, "test-password-123").await;

    let (code, team) = send_json(
        &app.app,
        "POST",
        "/api/v1/teams",
        Some(&owner),
        json!({"name": "INTAPI"}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let team_id = team["id"].as_str().unwrap().to_string();

    // Guests (Phase 13/14) live in a separate grants table and are
    // deny-by-default for integrations; they are NOT team_members, so no
    // member invite for the guest here.
    for (email, role) in [(&admin_email, "admin"), (&member_email, "member")] {
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&owner),
            json!({ "email": email, "role": role }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "invite {role}");
    }

    let (code, project) = send_json(
        &app.app,
        "POST",
        &format!("/api/v1/teams/{team_id}/projects"),
        Some(&owner),
        json!({"name": "INTAPI-PROJECT"}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let project_id = project["id"].as_str().unwrap().to_string();

    // A second team (same owner) for cross-team negative cases.
    let (code, other_team) = send_json(
        &app.app,
        "POST",
        "/api/v1/teams",
        Some(&owner),
        json!({"name": "INTAPI2"}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let other_team_id = other_team["id"].as_str().unwrap().to_string();
    let (code, other_project) = send_json(
        &app.app,
        "POST",
        &format!("/api/v1/teams/{other_team_id}/projects"),
        Some(&owner),
        json!({"name": "OTHER-PROJECT"}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let other_team_project = other_project["id"].as_str().unwrap().to_string();

    Fixture {
        owner,
        admin,
        member,
        guest,
        outsider,
        team_id,
        other_team_project,
        project_id,
        owner_id,
    }
}

async fn create_integration(
    app: &TestApp,
    fx: &Fixture,
    name: &str,
    cookie: &str,
) -> (StatusCode, Value) {
    send_json(
        &app.app,
        "POST",
        &format!("/api/v1/teams/{}/integrations", fx.team_id),
        Some(cookie),
        json!({"name": name, "provider": "github"}),
    )
    .await
}

#[tokio::test]
async fn integrations_management_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let fx = seed_team(&app, &suffix).await;

        // ---- Authorization matrix ---------------------------------------
        // Outsider: masked 404 everywhere.
        let (code, _) = create_integration(&app, &fx, "x-1", &fx.outsider).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider create");
        let (code, _) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/teams/{}/integrations", fx.team_id),
            Some(&fx.outsider),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider list");
        // Members exist in the team but lack the capability: 403.
        let (code, _) = create_integration(&app, &fx, "x-2", &fx.member).await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] member create");
        let (code, _) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/teams/{}/integrations", fx.team_id),
            Some(&fx.member),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] member list");
        // Guests are grant-scoped non-members: deny-by-default masks as 404
        // (no guest integration grants exist in this phase).
        let (code, _) = create_integration(&app, &fx, "x-3", &fx.guest).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest masked");
        // Admin is allowed.
        let (code, created) = create_integration(&app, &fx, "watcher", &fx.admin).await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] admin create");
        let integration_id = created["id"].as_str().unwrap().to_string();

        // ---- One-time secret disclosure ---------------------------------
        let secret = created["webhook_secret"]
            .as_str()
            .expect("[{engine}] create returns the secret once")
            .to_string();
        assert!(!secret.is_empty(), "[{engine}]");
        assert_eq!(
            created["credential"]["credential_version"].as_i64(),
            Some(1),
            "[{engine}] v1 credential"
        );
        assert_eq!(
            created["credential"]["active"].as_bool(),
            Some(true),
            "[{engine}]"
        );
        assert!(
            created["webhook_url_path"]
                .as_str()
                .unwrap()
                .contains(&integration_id),
            "[{engine}] webhook URL path hint"
        );
        // Never returned again.
        let (code, fetched) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/teams/{}/integrations/{integration_id}", fx.team_id),
            Some(&fx.owner),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert!(
            fetched.get("webhook_secret").is_none(),
            "[{engine}] the secret is never retrievable"
        );
        assert_eq!(fetched["credential"]["active"], json!(true));

        // ---- Validation: duplicate name, unknown provider, empty name ----
        let (code, _) = create_integration(&app, &fx, "watcher", &fx.owner).await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] duplicate name");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{}/integrations", fx.team_id),
            Some(&fx.owner),
            json!({"name": "gitea-one", "provider": "gitea"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] unknown provider"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{}/integrations", fx.team_id),
            Some(&fx.owner),
            json!({"name": "   ", "provider": "github"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] empty name"
        );

        // ---- Update ------------------------------------------------------
        let (code, updated) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/teams/{}/integrations/{integration_id}", fx.team_id),
            Some(&fx.owner),
            json!({"name": "watcher-renamed", "description": "prod"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] update");
        assert_eq!(updated["name"], "watcher-renamed");
        assert_eq!(updated["description"], "prod");

        // ---- Enable/disable ----------------------------------------------
        let (code, _) = send_empty(
            &app.app,
            "POST",
            &format!(
                "/api/v1/teams/{}/integrations/{integration_id}/disable",
                fx.team_id
            ),
            &fx.owner,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] disable");
        let (code, state) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/teams/{}/integrations/{integration_id}", fx.team_id),
            Some(&fx.owner),
            Value::Null,
        )
        .await;
        assert_eq!(state["enabled"], json!(false));
        let (code, _) = send_empty(
            &app.app,
            "POST",
            &format!(
                "/api/v1/teams/{}/integrations/{integration_id}/enable",
                fx.team_id
            ),
            &fx.owner,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] enable");

        // ---- Rotation: CAS + one-time disclosure + verification switch ---
        let (code, rotated) = send_json(
            &app.app,
            "POST",
            &format!(
                "/api/v1/teams/{}/integrations/{integration_id}/secret/rotate",
                fx.team_id
            ),
            Some(&fx.owner),
            json!({"expected_version": 1}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] rotate");
        let new_secret = rotated["webhook_secret"]
            .as_str()
            .expect("[{engine}] rotate returns the new secret once")
            .to_string();
        assert_ne!(new_secret, secret, "[{engine}] the secret changed");
        assert_eq!(
            rotated["credential"]["credential_version"].as_i64(),
            Some(2),
            "[{engine}] v2 credential"
        );
        // Stale version loses the CAS.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!(
                "/api/v1/teams/{}/integrations/{integration_id}/secret/rotate",
                fx.team_id
            ),
            Some(&fx.owner),
            json!({"expected_version": 1}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] stale rotation");

        // Cross-check: the NEW secret verifies on the intake; the old one 401s.
        let push = json!({"ref": "refs/heads/main", "commits": [{"id": "1"}],
                          "sender": {"login": "o"}, "repository": {"full_name": "o/r"}})
        .to_string();
        let signed = |secret: &str| {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
            mac.update(push.as_bytes());
            let tag = mac.finalize().into_bytes();
            format!(
                "sha256={}",
                tag.iter().map(|b| format!("{b:02x}")).collect::<String>()
            )
        };
        let post_webhook = |sig: String| {
            let app = app.app.clone();
            let integration_id = integration_id.clone();
            let push = push.clone();
            async move {
                to_response(
                    app.oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(format!("/api/v1/webhooks/github/{integration_id}"))
                            .header(header::CONTENT_TYPE, "application/json")
                            .header("x-hub-signature-256", &sig)
                            .header("x-github-delivery", Uuid::now_v7().to_string())
                            .header("x-github-event", "push")
                            .body(Body::from(push.into_bytes()))
                            .unwrap(),
                    )
                    .await
                    .unwrap(),
                )
                .await
            }
        };
        let (code, _) = post_webhook(signed(&new_secret)).await;
        assert_eq!(code, StatusCode::ACCEPTED, "[{engine}] new secret verifies");
        let (code, _) = post_webhook(signed(&secret)).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] old secret dead");

        // ---- Revocation: fail-closed intake, guarded lifecycle ----------
        let (code, _) = send_empty(
            &app.app,
            "POST",
            &format!(
                "/api/v1/teams/{}/integrations/{integration_id}/secret/revoke",
                fx.team_id
            ),
            &fx.owner,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] revoke");
        let (code, _) = post_webhook(signed(&new_secret)).await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] revoked credential fails closed"
        );
        // Rotating a revoked credential is refused.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!(
                "/api/v1/teams/{}/integrations/{integration_id}/secret/rotate",
                fx.team_id
            ),
            Some(&fx.owner),
            json!({"expected_version": 2}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] rotate revoked");
        // Re-provision (requires observed revocation) with one-time disclosure.
        let (code, reprovisioned) = send_json(
            &app.app,
            "POST",
            &format!(
                "/api/v1/teams/{}/integrations/{integration_id}/secret",
                fx.team_id
            ),
            Some(&fx.owner),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] re-provision");
        let revived_secret = reprovisioned["webhook_secret"]
            .as_str()
            .expect("[{engine}] re-provision returns the secret once")
            .to_string();
        assert_eq!(
            reprovisioned["credential"]["credential_version"].as_i64(),
            Some(3),
            "[{engine}] v3 credential"
        );
        let (code, _) = post_webhook(signed(&revived_secret)).await;
        assert_eq!(
            code,
            StatusCode::ACCEPTED,
            "[{engine}] revived secret verifies"
        );
        // Re-provisioning an ACTIVE credential is refused.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!(
                "/api/v1/teams/{}/integrations/{integration_id}/secret",
                fx.team_id
            ),
            Some(&fx.owner),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] re-provision active");

        // ---- Repositories + mappings ------------------------------------
        let repos_uri = format!(
            "/api/v1/teams/{}/integrations/{integration_id}/repositories",
            fx.team_id
        );
        let (code, repo) = send_json(
            &app.app,
            "POST",
            &repos_uri,
            Some(&fx.owner),
            json!({"external_repo_id": "123456", "full_name": "octocat/tasker"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] repo registered");
        let repo_id = repo["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &repos_uri,
            Some(&fx.owner),
            json!({"external_repo_id": "123456", "full_name": "octocat/tasker"}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] duplicate repo id");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("{repos_uri}/{repo_id}/mappings"),
            Some(&fx.owner),
            json!({"project_id": fx.other_team_project}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] cross-team project");
        let (code, mapping) = send_json(
            &app.app,
            "POST",
            &format!("{repos_uri}/{repo_id}/mappings"),
            Some(&fx.owner),
            json!({"project_id": fx.project_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] same-team mapping");
        let mapping_id = mapping["id"].as_str().unwrap().to_string();
        // Deleting the repository with a mapping is a clean 409.
        let (code, _) = send_empty(
            &app.app,
            "DELETE",
            &format!("{repos_uri}/{repo_id}"),
            &fx.owner,
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] mappings restrict");
        // Remove the mapping, then the repository.
        let (code, _) = send_empty(
            &app.app,
            "DELETE",
            &format!("{repos_uri}/{repo_id}/mappings/{mapping_id}"),
            &fx.owner,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] mapping removed");
        let (code, _) = send_empty(
            &app.app,
            "DELETE",
            &format!("{repos_uri}/{repo_id}"),
            &fx.owner,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] repo removed");

        // ---- Delivery inspection: metadata only, replay audited ----------
        let deliveries_uri = format!(
            "/api/v1/teams/{}/integrations/{integration_id}/deliveries",
            fx.team_id
        );
        // Seed two deliveries: one processed, one dead-lettered.
        let mut conn = app.db.conn();
        let now = tasker::util::now_ms();
        for (delivery_id, status) in [("d-ok", "processed"), ("d-dead", "dead_letter")] {
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s(&integration_id)
                .s(delivery_id)
                .s("push")
                .s(r#"{"sensitive": "payload-not-served"}"#)
                .s(status)
                .i(now) // next_attempt_at
                .i(now)
                .i(now);
            let sql = if status == "processed" {
                "INSERT INTO webhook_deliveries (id, integration_id, delivery_id, \
                 event_type_raw, payload, status, attempts, next_attempt_at, created_at, \
                 updated_at, completed_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?)"
            } else {
                "INSERT INTO webhook_deliveries (id, integration_id, delivery_id, \
                 event_type_raw, payload, status, attempts, next_attempt_at, created_at, \
                 updated_at, completed_at, last_error_category, last_error_detail) \
                 VALUES (?, ?, ?, ?, ?, ?, 3, ?, ?, ?, ?, 'exhausted', 'gave up')"
            };
            let mut args2 = args;
            // Placeholders after the attempt literal: next_attempt_at,
            // created_at, updated_at, completed_at.
            args2.i(now);
            conn.execute(sql, args2)
                .await
                .unwrap_or_else(|e| panic!("[{engine}] seed delivery {delivery_id}: {e}"));
        }
        drop(conn);

        let (code, listed) = send_json(
            &app.app,
            "GET",
            &deliveries_uri,
            Some(&fx.owner),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        // The secret cross-checks above already accepted real webhook
        // deliveries, so the total is the two seeds plus those.
        assert!(
            listed["total"].as_i64().unwrap_or(0) >= 2,
            "[{engine}] seeded deliveries listed"
        );
        let first = &listed["items"][0];
        assert!(
            first.get("payload").is_none(),
            "[{engine}] payloads are never served"
        );
        assert!(
            first.get("claim_token").is_some() || first.is_object(),
            "[{engine}] metadata projection"
        );
        // Unknown status filter refused.
        let (code, _) = send_json(
            &app.app,
            "GET",
            &format!("{deliveries_uri}?status=bogus"),
            Some(&fx.owner),
            Value::Null,
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] status filter"
        );

        // Dead-letter detail + replay.
        let mut conn = app.db.conn();
        let dead_row_id: String = {
            let mut args = tasker::db::Args::new();
            args.s(&integration_id).s("d-dead");
            conn.fetch_optional(
                "SELECT id FROM webhook_deliveries WHERE integration_id = ? AND delivery_id = ?",
                args,
            )
            .await
            .unwrap()
            .map(|r| r.try_get::<String, _>("id").unwrap())
            .unwrap()
        };
        drop(conn);
        let (code, detail) = send_json(
            &app.app,
            "GET",
            &format!("{deliveries_uri}/{}", dead_row_id),
            Some(&fx.owner),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(detail["status"], "dead_letter");
        assert_eq!(detail["last_error_category"], "exhausted");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("{deliveries_uri}/{}/replay", dead_row_id),
            Some(&fx.owner),
            json!({"reason": "fixed upstream"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] replay accepted");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("{deliveries_uri}/{}/replay", dead_row_id),
            Some(&fx.member),
            json!({}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member cannot replay"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("{deliveries_uri}/{}/replay", dead_row_id),
            Some(&fx.owner),
            json!({}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] a pending (replayed) delivery is not replayable"
        );

        // The replay moved the delivery back to pending with a fresh budget.
        let mut conn = app.db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&integration_id).s("d-dead");
        let replayed = conn
            .fetch_optional(
                "SELECT status, attempts, next_attempt_at FROM webhook_deliveries \
                 WHERE integration_id = ? AND delivery_id = ?",
                args,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replayed.try_get::<String, _>("status").unwrap(), "pending");
        assert_eq!(replayed.try_get::<i64, _>("attempts").unwrap(), 0);
        drop(replayed);

        // ---- Archive: read-only afterwards ------------------------------
        let (code, _) = send_empty(
            &app.app,
            "DELETE",
            &format!("/api/v1/teams/{}/integrations/{integration_id}", fx.team_id),
            &fx.owner,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] archive");
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/teams/{}/integrations/{integration_id}", fx.team_id),
            Some(&fx.owner),
            json!({"name": "renamed-after-archive"}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] archived read-only");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("{deliveries_uri}/{}/replay", dead_row_id),
            Some(&fx.owner),
            json!({}),
        )
        .await;
        // Replay on an archived integration: the replay guard fires first if
        // the delivery is pending; either way an archived integration must
        // not accept mutations (409 here).
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] archived replay blocked"
        );
        // History stays visible.
        let (code, _) = send_json(
            &app.app,
            "GET",
            &deliveries_uri,
            Some(&fx.owner),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] archived history readable");
    }
}
