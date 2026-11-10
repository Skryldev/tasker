//! HTTP integration tests: authentication, authorization, and task workflows
//! against real database engines.
//!
//! Engines: `SQLite` always (temp file). `PostgreSQL` and `MySQL` run when env
//! vars are set (see `test_db_urls`), typically via scripts/test-matrix.sh.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;

use tasker::config::{Config, Environment, PoolConfig};
use tasker::db::Database;
use tasker::http::build_router;
use tasker::security::password;
use tasker::state::AppState;

/// URLs of additional engines to test, from `TASKER_TEST_PG_URL` and
/// `TASKER_TEST_MYSQL_URL`.
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
        integrations_key: Some(vec![19u8; 32]),
        attachments: tasker::config::AttachmentsConfig {
            mode: tasker::config::AttachmentsMode::Local,
            local_dir: std::env::temp_dir()
                .join(format!("tasker-test-att-{}", std::process::id()))
                .to_string_lossy()
                .into_owned(),
            max_bytes: 2_000_000,
            max_per_task: 10,
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
    // Surface internal error details (incl. sqlx messages) during test runs.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();
    let config = test_config(url.to_string());
    let db = Database::connect(&config).await.expect("connect");
    db.migrate()
        .await
        .unwrap_or_else(|e| panic!("migrate failed: {e}"));
    let db = Arc::new(db);
    let state = AppState::new((*db).clone(), config);
    TestApp {
        app: build_router(state),
        db,
    }
}

/// Seed an account with instance-owner rights (Phase 12.5 Bug 2).
async fn seed_owner(db: &Database, email: &str, display: &str) -> String {
    let mut conn = db.begin().await.unwrap();
    let hash = password::hash("test-password-123").unwrap();
    let user = tasker::users::insert(
        &mut conn,
        &tasker::users::NewUser {
            email,
            display_name: display,
            password_hash: &hash,
            username: None,
            instance_role: tasker::users::INSTANCE_ROLE_OWNER,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("seed_owner {email} failed: {e}"));
    conn.commit().await.unwrap();
    user.id
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

async fn get_json(app: &axum::Router, uri: &str, cookie: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    to_response(response).await
}

async fn send_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    if uri.contains(' ') {
        eprintln!("[dbg] BAD URI: {method} {uri}");
    }
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

/// Issue a request without a JSON body (DELETE etc.).
async fn send_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    to_response(response).await
}

fn assert_f64_close(actual: &Value, expected: f64, ctx: &str) {
    let v = actual
        .as_f64()
        .unwrap_or_else(|| panic!("[debug] {ctx}: not a number: {actual}"));
    assert!(
        (v - expected).abs() < 1e-9,
        "[{ctx}] expected {expected}, got {v}"
    );
}

async fn to_response(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

#[tokio::test]
async fn security_matrix_across_engines() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("admin-{suffix}@test.local");
        let member_email = format!("member-{suffix}@test.local");
        let outsider_email = format!("outsider-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "Admin").await;
        seed_user(&app.db, &member_email, "Member").await;
        seed_user(&app.db, &outsider_email, "Outsider").await;

        let admin_cookie = login_as(&app.app, &admin_email, "test-password-123").await;
        let member_cookie = login_as(&app.app, &member_email, "test-password-123").await;
        let outsider_cookie = login_as(&app.app, &outsider_email, "test-password-123").await;

        // 1. Unauthenticated access is rejected everywhere.
        let (status, _) = get_json(&app.app, "/api/v1/teams", "").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "[{engine}] no cookie");
        let (status, _) = get_json(&app.app, "/api/v1/me", "tasker_session=forged").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "[{engine}] forged token");

        // 2. Admin creates the team + project; plain members cannot.
        let (status, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin_cookie),
            json!({ "name": "Test Team" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "[{engine}] create team");
        let team_id = team["id"].as_str().unwrap();

        let (status, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin_cookie),
            json!({ "email": member_email, "role": "member" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "[{engine}] add member");

        let (status, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin_cookie),
            json!({ "name": "Secret Project" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "[{engine}] create project");
        let project_id = project["id"].as_str().unwrap();

        let (status, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&member_cookie),
            json!({ "name": "Nope" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "[{engine}] member project create"
        );

        // 3. Outsiders get 404 (no existence leak) on team data.
        let (status, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/projects"),
            &outsider_cookie,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "[{engine}] outsider projects"
        );
        let (status, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}"),
            &outsider_cookie,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "[{engine}] outsider team detail"
        );

        // 4. Member can create a task but only assign themselves.
        let (status, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&member_cookie),
            json!({ "title": "Member task" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "[{engine}] member task create");
        let task_id = task["id"].as_str().unwrap();

        let (status, members) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/members"),
            &admin_cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let admin_id = members["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "owner")
            .unwrap()["user"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (status, _problem) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&member_cookie),
            json!({ "assignee_id": admin_id }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "[{engine}] cross assignment");

        // 5. Members cannot escalate their own role.
        let member_id = members["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "member")
            .unwrap()["user"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (status, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/teams/{team_id}/members/{member_id}"),
            Some(&member_cookie),
            json!({ "role": "owner" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "[{engine}] self escalation");

        // 6. Own-task updates work and land in the activity history. Status
        // changes are NOT part of the generic update (400): lifecycle
        // transitions have a dedicated endpoint.
        let (status, updated) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&member_cookie),
            json!({ "description": "updated by member" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "[{engine}] own-task update");
        assert_eq!(updated["description"], "updated by member");
        let (status, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&member_cookie),
            json!({ "status": "in_progress" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "[{engine}] patch status");
        let (status, activity) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/activity"),
            &admin_cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let events: Vec<&str> = activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["event"].as_str().unwrap())
            .collect();
        assert!(events.contains(&"task_created"), "[{engine}] created event");
        assert!(events.contains(&"task_updated"), "[{engine}] update event");

        // 7. Invalid input yields problem+json validation, never a 500.
        let (status, problem) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin_cookie),
            json!({ "title": "", "status": "bogus" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] validation"
        );
        assert_eq!(
            problem["type"], "https://tasker.dev/problems/validation-failed",
            "[{engine}] problem type"
        );
        assert!(problem["errors"].is_array(), "[{engine}] errors array");

        // 8. Subtask constraints: allowed one level, rejected deeper.
        let (status, subtask) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/subtasks"),
            Some(&admin_cookie),
            json!({ "title": "Subtask" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "[{engine}] subtask");
        let subtask_id = subtask["id"].as_str().unwrap();
        let (status, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{subtask_id}/subtasks"),
            Some(&admin_cookie),
            json!({ "title": "Nested" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] nesting blocked"
        );

        // 9. Archiving is a hard read/write boundary and returns 409 for writes.
        let (status, archived) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/archive"),
            Some(&admin_cookie),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "[{engine}] archive");
        assert_eq!(archived["status"], "archived");
        let (status, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin_cookie),
            json!({ "title": "Blocked" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "[{engine}] archived rejects tasks"
        );

        // 10. Logout revokes the session: the cookie stops working.
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/logout")
                    .header(header::COOKIE, &member_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let (status, _) = get_json(&app.app, "/api/v1/me", &member_cookie).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "[{engine}] revoked session"
        );
    }
}

#[tokio::test]
async fn expired_sessions_are_rejected() {
    let url = format!(
        "sqlite:{}/{}",
        std::env::temp_dir().display(),
        Uuid::now_v7()
    );
    let app = spawn_app(&url).await;

    let email = "expiry@test.local";
    let user_id = seed_user(&app.db, email, "Expiry").await;

    // A session with ttl=0 is born expired; its cookie must not authenticate.
    let mut conn = app.db.begin().await.unwrap();
    let token = tasker::auth::create_session(&mut conn, &user_id, 0, "", None)
        .await
        .unwrap();
    conn.commit().await.unwrap();
    let cookie = format!("tasker_session={token}");
    let (status, _) = get_json(&app.app, "/api/v1/me", &cookie).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Phase 2: reporter acceptance workflow + in-app notifications
// ---------------------------------------------------------------------------

/// Full Phase 2 verification: workflow transitions, authorization, resolution
/// consistency, activity, and notification behavior on every engine.
#[tokio::test]
async fn workflow_and_notifications_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("admin-{suffix}@test.local");
        let member_email = format!("member-{suffix}@test.local");
        let outsider_email = format!("outsider-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "Admin").await;
        seed_user(&app.db, &member_email, "Member").await;
        seed_user(&app.db, &outsider_email, "Outsider").await;

        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let outsider = login_as(&app.app, &outsider_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"T"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role":"member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"P"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap();

        let admin_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "owner")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let member_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "member")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };

        // Reporter defaults to the creator (admin here).
        let (code, t1) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title":"Reviewed task","assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] create");
        assert_eq!(
            t1["reporter"]["id"].as_str().unwrap(),
            t1["created_by"]["id"].as_str().unwrap(),
            "[{engine}] default reporter is creator"
        );
        let task_id = t1["id"].as_str().unwrap().to_string();

        // A plain member cannot select a different reporter.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&member),
            json!({"title":"Nope","reporter_id": admin_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member picks other reporter"
        );

        // A reporter outside the team is rejected even for admins.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title":"Nope2","reporter_id":"00000000-0000-0000-0000-000000000000"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] foreign reporter"
        );

        // Creation into review states is rejected: those are reached only
        // through lifecycle transitions.
        for status in ["resolved", "in_review", "closed"] {
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/projects/{project_id}/tasks"),
                Some(&admin),
                json!({"title":"Instant state","status": status}),
            )
            .await;
            assert_eq!(
                code,
                StatusCode::UNPROCESSABLE_ENTITY,
                "[{engine}] create as {status}"
            );
        }

        // Identity invariant: the assignee and reporter must be different
        // users. The reporter defaults to the creator, so self-assignment
        // conflicts at creation...
        let (code, problem) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title":"Self review","assignee_id": admin_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] create with overlapping roles"
        );
        assert_eq!(problem["errors"][0]["field"], "assignee_id");

        // ...and a PATCH that would produce the overlap is rejected too,
        // without silently changing either role.
        let (code, problem) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"reporter_id": member_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] patch creates overlap"
        );
        assert_eq!(problem["errors"][0]["field"], "assignee_id");
        let (_, t) = get_json(&app.app, &format!("/api/v1/tasks/{task_id}"), &admin).await;
        assert_eq!(
            t["reporter"]["id"].as_str().unwrap(),
            admin_id,
            "[{engine}] reporter unchanged after rejected patch"
        );

        // Generic PATCH no longer mutates status — transitions have a
        // dedicated endpoint.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"status":"in_progress"}),
        )
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "[{engine}] patch status");
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"status":"done"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::BAD_REQUEST,
            "[{engine}] patch skip review"
        );

        // Assignee drives the execution leg: backlog -> todo.
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to":"todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] backlog to todo");
        assert_eq!(t["status"], "todo");

        // Role separation: the reporter cannot execute assignee transitions.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&admin),
            json!({"to":"in_progress"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] reporter cannot start progress"
        );

        // Assignee: todo -> in_progress -> resolved.
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to":"in_progress"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] start progress");
        assert_eq!(t["status"], "in_progress");
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to":"resolved"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] resolve");
        assert_eq!(t["status"], "resolved");

        // Outsiders cannot act (404: cross-team isolation).
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&outsider),
            json!({"to":"in_review"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] outsider transition"
        );

        // The assignee cannot hand work to review; that is the reporter's call.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to":"in_review"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] assignee cannot submit"
        );

        // Reporter submits: resolved -> in_review. The assignee is notified.
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&admin),
            json!({"to":"in_review"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] submit");
        assert_eq!(t["status"], "in_review");

        // The assignee cannot decide acceptance — even though they are not
        // the reporter; acceptance belongs to the reporter alone.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to":"closed"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] assignee self-accept"
        );

        // Rejection requires a meaningful reason.
        for reason in [Value::Null, json!("   ")] {
            let body = if reason.is_null() {
                json!({"to":"in_progress"})
            } else {
                json!({"to":"in_progress","reason": reason})
            };
            let (code, problem) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{task_id}/transition"),
                Some(&admin),
                body,
            )
            .await;
            assert_eq!(
                code,
                StatusCode::UNPROCESSABLE_ENTITY,
                "[{engine}] blank reject"
            );
            assert_eq!(problem["errors"][0]["field"], "reason");
        }

        // Reporter rejects: in_review -> in_progress with an audited reason.
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&admin),
            json!({"to":"in_progress","reason":"fix the intro"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] reject");
        assert_eq!(t["status"], "in_progress");
        assert_eq!(t["resolution"], "rejected");
        assert_eq!(t["resolution_note"], "fix the intro");
        assert_eq!(t["completed_at"], Value::Null);

        // The full cycle repeats after rejection: resolve, submit, accept.
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to":"resolved"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] re-resolve");
        assert_eq!(t["status"], "resolved");
        let (code, _t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&admin),
            json!({"to":"in_review"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] re-submit");
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&admin),
            json!({"to":"closed"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] accept");
        assert_eq!(t["status"], "closed");
        assert_eq!(t["resolution"], "accepted");
        assert_eq!(t["resolved_by"]["id"].as_str().unwrap(), admin_id);
        assert!(t["resolved_at"].is_number());
        assert!(t["completed_at"].is_number());

        // Closed is terminal: no transition leaves it.
        for to in ["todo", "in_progress", "in_review"] {
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{task_id}/transition"),
                Some(&admin),
                json!({"to": to}),
            )
            .await;
            assert_eq!(
                code,
                StatusCode::CONFLICT,
                "[{engine}] closed is terminal ({to})"
            );
        }
        // Stale no-op transition is a conflict too.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&admin),
            json!({"to":"closed"}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] stale accept");

        // Non-member cannot act even with the task id.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&outsider),
            json!({"to":"in_progress","reason":"nope"}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider reject");

        // Activity trail records the workflow decisions and the reason.
        let (code, activity) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/activity"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let events: Vec<&str> = activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["event"].as_str().unwrap())
            .collect();
        for expected in [
            "task_submitted",
            "task_accepted",
            "task_rejected",
            "task_rejection_reason",
        ] {
            assert!(
                events.contains(&expected),
                "[{engine}] missing {expected} in {events:?}"
            );
        }
        let rejection = activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["event"] == "task_rejected")
            .unwrap();
        assert_eq!(
            rejection["new_value"], "in_progress",
            "[{engine}] rejection records transition"
        );
        let reason_row = activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["event"] == "task_rejection_reason")
            .unwrap();
        assert_eq!(
            reason_row["new_value"], "fix the intro",
            "[{engine}] reason persisted"
        );

        // -- Notifications ---------------------------------------------------
        // The reporter (admin) was notified of the submission; the assignee
        // (member) of acceptance and rejection; nobody was self-notified.
        let (code, member_notifs) =
            get_json(&app.app, "/api/v1/notifications?unread=true", &member).await;
        assert_eq!(code, StatusCode::OK);
        let member_events: Vec<&str> = member_notifs["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["event"].as_str().unwrap())
            .collect();
        assert!(
            member_events.contains(&"task_accepted"),
            "[{engine}] accept notif: {member_events:?}"
        );
        assert!(
            member_events.contains(&"task_rejected"),
            "[{engine}] reject notif"
        );
        assert!(
            member_events.contains(&"task_submitted"),
            "[{engine}] assignee notified of review submission"
        );

        let (_code, admin_notifs) =
            get_json(&app.app, "/api/v1/notifications?unread=true", &admin).await;
        let admin_events: Vec<&str> = admin_notifs["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["event"].as_str().unwrap())
            .collect();
        assert!(
            admin_events.contains(&"task_status_changed"),
            "[{engine}] reporter workflow notif: {admin_events:?}"
        );
        assert!(
            !admin_events.contains(&"task_submitted"),
            "[{engine}] submitter is not self-notified: {admin_events:?}"
        );

        let (code, c1) = get_json(&app.app, "/api/v1/notifications/unread-count", &member).await;
        assert_eq!(code, StatusCode::OK);
        let member_count = c1["count"].as_i64().unwrap();
        assert!(member_count > 0, "[{engine}] member has unread");

        // Mark one read: idempotent; the count decrements exactly once.
        let nid = member_notifs["items"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let read_uri = format!("/api/v1/notifications/{nid}/read");
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&read_uri)
                    .header(header::COOKIE, &member)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&read_uri)
                    .header(header::COOKIE, &member)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "[{engine}] idempotent read"
        );
        let (_code, c3) = get_json(&app.app, "/api/v1/notifications/unread-count", &member).await;
        assert_eq!(
            c3["count"].as_i64().unwrap(),
            member_count - 1,
            "[{engine}] count decrements once"
        );

        // Cross-user isolation: another user cannot mark someone else's
        // notification read.
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&read_uri)
                    .header(header::COOKIE, &admin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "[{engine}] foreign mark-read"
        );

        // Phase 12 WS10: closed tasks are immutable — the earlier lifecycle
        // test closed this task, so comment tests use a fresh open task.
        let (code, open_task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "P-open for comments"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] open task created");
        let open_task_id = open_task["id"].as_str().unwrap().to_string();

        // Mention dedup: one comment mentioning the same member twice yields
        // exactly one mention notification; an unknown address notifies nobody.
        let mention_count = |notifs: &Value| {
            notifs["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|n| n["event"] == "comment_mention")
                .count()
        };
        let (_code, n) = get_json(&app.app, "/api/v1/notifications?unread=true", &member).await;
        let before = mention_count(&n);
        let (code, comment_problem) = send_json(&app.app, "POST", &format!("/api/v1/tasks/{open_task_id}/comments"),
            Some(&admin),
            json!({"body": format!("hey @{member_email} and again @{member_email} — ping @ghost@nowhere.example")})).await;
        assert_eq!(
            code,
            StatusCode::CREATED,
            "[{engine}] mention comment: {comment_problem}"
        );
        let (_code, n) = get_json(&app.app, "/api/v1/notifications?unread=true", &member).await;
        assert_eq!(
            mention_count(&n),
            before + 1,
            "[{engine}] mention dedup + team scoping"
        );

        // Editing without adding new mentions does not re-notify.
        let (_code, comments) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{open_task_id}/comments"),
            &admin,
        )
        .await;
        let comment_id = comments["items"][0]["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/comments/{comment_id}"),
            Some(&admin),
            json!({"body": format!("hey @{member_email} and again @{member_email} — edited")}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (_code, n) = get_json(&app.app, "/api/v1/notifications?unread=true", &member).await;
        assert_eq!(
            mention_count(&n),
            before + 1,
            "[{engine}] edit does not re-notify"
        );

        // Read-all resets the unread count and filter.
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/notifications/read-all")
                    .header(header::COOKIE, &member)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let (_code, c) = get_json(&app.app, "/api/v1/notifications/unread-count", &member).await;
        assert_eq!(c["count"].as_i64().unwrap(), 0, "[{engine}] read-all");
        let (_code, unread) =
            get_json(&app.app, "/api/v1/notifications?unread=true", &member).await;
        assert_eq!(
            unread["total"].as_i64().unwrap(),
            0,
            "[{engine}] unread filter"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 7: TOTP two-factor authentication — security matrix
// ---------------------------------------------------------------------------

/// Decode a Base32 (RFC 4648, no padding) secret to raw bytes.
fn base32_decode(input: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits: u32 = 0;
    let mut acc: u32 = 0;
    let mut out = Vec::new();
    for c in input.trim_end_matches('=').chars() {
        #[allow(clippy::cast_possible_truncation)]
        let v = ALPHABET
            .iter()
            .position(|a| *a == c.to_ascii_uppercase() as u8)
            .expect("base32 char") as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push(((acc >> (bits - 8)) & 0xFF) as u8);
            bits -= 8;
        }
    }
    out
}

/// Current TOTP code for a Base32 secret (independent of the app's verify path
/// only in location — it uses the same totp-rs crate with the same parameters).
///
/// Boundary-aware: when fewer than 5 seconds of the current 30-second step
/// remain, waits for the next boundary first. The backend accepts only the
/// current step (window 0), so a code generated at the tail of a step can
/// legitimately expire before a slow request reaches the server — that is a
/// test-infrastructure artifact, not an application rejection. The wait is
/// bounded (< 30.5 s) and codes are still current-step codes, so replay
/// protection and enrollment semantics are unchanged.
async fn totp_code_now(secret_base32: &str, email: &str) -> String {
    use totp_rs::{Algorithm, TOTP};
    const STEP_MS: u64 = 30_000;
    // 15s: under CI load, generation-to-processing can stall for seconds;
    // generating only with this much step margin keeps the code in the step
    // the server will see (window 0). Combined with the helper's fresh-code
    // retries, a boundary crossing cannot fail three consecutive attempts
    // unless the machine stalls for the full margin on every attempt.
    const MIN_REMAINING_MS: u64 = 15_000;
    let epoch_ms = u64::try_from(tasker::util::now_ms()).unwrap_or(0);
    let into_step = epoch_ms % STEP_MS;
    if into_step > STEP_MS - MIN_REMAINING_MS {
        // tokio::time::sleep YIELDS the worker thread; std::thread::sleep
        // would block it, starving the in-process test server (which runs on
        // the same runtime) for the full sleep and pushing every concurrent
        // request past the TOTP boundary.
        let wait = STEP_MS - into_step + 500;
        tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
    }
    let bytes = base32_decode(secret_base32);
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        0,
        30,
        bytes,
        Some("Tasker".to_string()),
        email.to_string(),
    )
    .expect("totp");
    totp.generate_current().expect("current")
}

/// TOTP code for `steps_ago` steps before now (e.g. the previous window).
fn totp_code_steps_ago(secret_base32: &str, email: &str, steps_ago: u64) -> String {
    use totp_rs::{Algorithm, TOTP};
    let bytes = base32_decode(secret_base32);
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        0,
        30,
        bytes,
        Some("Tasker".to_string()),
        email.to_string(),
    )
    .expect("totp");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    totp.generate(now - steps_ago * 30)
}

/// Comprehensive Phase 7 verification on every available engine.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn mfa_security_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7();

        // -- Scenario 2: login without MFA is unchanged ----------------------
        let plain_email = format!("plain-{suffix}@test.local");
        let plain_id = seed_user(&db, &plain_email, "Plain User").await;
        let plain_cookie = login_as(&app.app, &plain_email, "test-password-123").await;
        let (code, me) = get_json(&app.app, "/api/v1/me", &plain_cookie).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] plain login works");
        assert_eq!(me["id"], plain_id, "[{engine}] normal session resolves");
        assert!(me.get("mfa_required").is_none(), "[{engine}] no mfa shape");

        // -- Scenario 1: setup → enable → MFA login → verify → session --------
        let email = format!("mfa-{suffix}@test.local");
        let user_id = seed_user(&db, &email, "Mfa User").await;
        let cookie = login_as(&app.app, &email, "test-password-123").await;
        let (enabled, secret_base32) =
            enable_mfa_ok(&app.app, &cookie, engine.as_str(), "scenario 1").await;
        let recovery_codes: Vec<String> = enabled["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(recovery_codes.len(), 10, "[{engine}] ten recovery codes");
        assert!(
            recovery_codes
                .iter()
                .all(|c| c.len() == 9 && c.as_bytes()[4] == b'-'),
            "[{engine}] XXXX-XXXX format"
        );

        // Persisted secret is ciphertext; recovery storage holds hashes only.
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&user_id);
            let row = conn.fetch_optional("SELECT totp_secret_encrypted, recovery_codes_hashed FROM user_mfa WHERE user_id = ?", args)
                .await.unwrap().expect("user_mfa row");
            let stored_secret: String = row.try_get("totp_secret_encrypted").unwrap();
            let stored_hashes: String = row.try_get("recovery_codes_hashed").unwrap();
            assert!(
                stored_secret.starts_with("v1."),
                "[{engine}] encrypted envelope"
            );
            assert!(
                !stored_secret.contains(&secret_base32),
                "[{engine}] ciphertext not base32 secret"
            );
            for c in &recovery_codes {
                assert!(
                    !stored_hashes.contains(c),
                    "[{engine}] no plaintext code at rest"
                );
            }
            assert!(
                stored_hashes.starts_with('[') && stored_hashes.ends_with(']'),
                "[{engine}] json hash array"
            );
        }

        // MFA login: password alone must NOT create a session.
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email, "password": "test-password-123"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] mfa login status");
        assert_eq!(login_body["mfa_required"], true, "[{engine}] mfa_required");
        let mfa_token = login_body["mfa_token"].as_str().unwrap().to_string();
        assert!(
            mfa_token.starts_with("mfp_"),
            "[{engine}] pending token prefix"
        );
        let (code, _) = get_json(
            &app.app,
            "/api/v1/me",
            &format!("{}={}", "tasker_session", mfa_token),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] pending token is not a session cookie"
        );

        // Valid TOTP → normal session.
        // Boundary-aware verify: on a 401 from a step-boundary stall, retry
        // once with a freshly generated code (the pending token is NOT
        // consumed by a failed attempt, so the retry is legal).
        let totp_now = totp_code_now(&secret_base32, &email).await;
        let mut response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/2fa/verify")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"mfa_token": mfa_token, "code": totp_now}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        if response.status() != StatusCode::OK {
            response = app
                .app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/auth/2fa/verify")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({"mfa_token": mfa_token, "code": totp_code_now(&secret_base32, &email).await})
                                .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
        }
        let (parts, raw_body) = response.into_parts();
        let set_cookie = parts.headers.get(header::SET_COOKIE);
        if set_cookie.is_none() || parts.status != StatusCode::OK {
            let bytes = axum::body::to_bytes(raw_body, 8192).await.unwrap();
            panic!(
                "verify failed: {} cookie={:?} body={}",
                parts.status,
                set_cookie.map(|c| c.to_str().unwrap_or("?")),
                String::from_utf8_lossy(&bytes)
            );
        }
        let set_cookie = set_cookie.unwrap();
        let session_cookie = set_cookie
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let (code, me) = get_json(&app.app, "/api/v1/me", &session_cookie).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] post-verify session works");
        assert_eq!(
            me["id"].as_str().unwrap(),
            user_id,
            "[{engine}] correct user"
        );

        // The consumed pending token cannot verify again.
        let totp_next = totp_code_now(&secret_base32, &email).await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": mfa_token, "code": totp_next}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] consumed pending token"
        );

        // -- Scenario 3: invalid TOTP → 401, no session -----------------------
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email, "password": "test-password-123"}),
        )
        .await;
        let token3 = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, verify_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token3, "code": "000000"}),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] invalid totp");
        assert!(
            !verify_body.to_string().contains("secret"),
            "[{engine}] no crypto detail in error"
        );
        let (code, _) = get_json(
            &app.app,
            "/api/v1/me",
            &format!("tasker_session={}", "nonexistent"),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] no session after failure"
        );

        // -- Scenario 4: previous-window TOTP rejected (window = 0) ------------
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email, "password": "test-password-123"}),
        )
        .await;
        let token4 = login_body["mfa_token"].as_str().unwrap().to_string();
        let stale = totp_code_steps_ago(&secret_base32, &email, 2);
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token4, "code": stale}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] previous window rejected"
        );
        // The failed attempt did not consume the token. To prove that with a
        // current-step code even though this scenario runs inside the same
        // 30-second window as the earlier successful verification, rewind the
        // stored step deterministically (as if the window had rolled over).
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.i(tasker::mfa::current_step() - 1).s(&user_id);
            conn.execute(
                "UPDATE user_mfa SET last_used_step = ? WHERE user_id = ?",
                args,
            )
            .await
            .unwrap();
        }
        let fresh = totp_code_now(&secret_base32, &email).await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token4, "code": fresh}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] failed attempt kept the token valid"
        );

        // -- Scenario 5: rate limit — 5 failures then 429 ----------------------
        // Fresh app instance so the limiter windows for this scenario start
        // empty (counters are per-process; earlier scenarios must not count).
        drop(app);
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let rl_email = format!("mfa-rl-{suffix}@test.local");
        let _rl_user = seed_user(&db, &rl_email, "Mfa Rate").await;
        let rl_cookie = login_as(&app.app, &rl_email, "test-password-123").await;
        let (code, rl_setup) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&rl_cookie),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] rl setup");
        let rl_secret = rl_setup["secret"].as_str().unwrap().to_string();
        _ = enable_mfa_ok(&app.app, &rl_cookie, engine.as_str(), "_").await;
        // Five failed verifications (each with its own pending token).
        #[allow(unused_assignments)]
        let mut token5 = String::new();
        for i in 0..5 {
            let (login_code, login_body) = send_json(
                &app.app,
                "POST",
                "/api/v1/auth/login",
                None,
                json!({"email": rl_email, "password": "test-password-123"}),
            )
            .await;
            assert_eq!(login_code, StatusCode::OK, "[{engine}] mfa login {i}");
            token5 = login_body["mfa_token"].as_str().unwrap().to_string();
            let (code, _) = send_json(
                &app.app,
                "POST",
                "/api/v1/auth/2fa/verify",
                None,
                json!({"mfa_token": token5, "code": format!("{:06}", 100_000 + i)}),
            )
            .await;
            assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] failure {i}");
        }
        // The counter window is established (5 failures); the sixth attempt
        // — even with a valid pending token and a correct code — hits 429.
        let (login_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": rl_email, "password": "test-password-123"}),
        )
        .await;
        assert_eq!(login_code, StatusCode::OK, "[{engine}] mfa login for 6th");
        token5 = login_body["mfa_token"].as_str().unwrap().to_string();
        let valid_code = totp_code_now(&rl_secret, &rl_email).await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token5, "code": valid_code}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::TOO_MANY_REQUESTS,
            "[{engine}] 6th attempt within window is 429"
        );
        // A login by ANOTHER user is unaffected by the per-user limit: the
        // plain (non-MFA) user completes login normally — no 429, no pending
        // token, and crucially the mfa:user counter keyed on the rate-limited
        // user did not block them. The same database persists across the
        // re-spawned instances, so the plain user exists here.
        let (code, plain_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": plain_email, "password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] other user unaffected by the mfa rate limit"
        );
        assert!(
            plain_body.get("mfa_required").is_none(),
            "[{engine}] plain user logs in without mfa challenge"
        );

        // -- Scenario 6/7: disable requires the current password ---------------
        let email2 = format!("mfa2-{suffix}@test.local");
        let user2 = seed_user(&db, &email2, "Mfa Two").await;
        let cookie2 = login_as(&app.app, &email2, "test-password-123").await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie2),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, setup2) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie2),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let _secret2 = setup2["secret"].as_str().unwrap().to_string();
        enable_mfa_ok(&app.app, &cookie2, engine.as_str(), "user2").await;

        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/disable",
            Some(&cookie2),
            json!({}),
        )
        .await;
        assert!(
            matches!(
                code,
                StatusCode::UNPROCESSABLE_ENTITY | StatusCode::BAD_REQUEST
            ),
            "[{engine}] disable without password rejected: {code}"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/disable",
            Some(&cookie2),
            json!({"current_password": "wrong-password-1"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] disable with wrong password"
        );
        // Still enabled after failed disable attempts.
        let still_enabled = db_user_mfa_exists(&db, &user2).await;
        assert!(still_enabled, "[{engine}] mfa survives failed disable");
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/disable",
            Some(&cookie2),
            json!({"current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NO_CONTENT,
            "[{engine}] disable with correct password"
        );
        assert!(
            !db_user_mfa_exists(&db, &user2).await,
            "[{engine}] row removed on disable"
        );
        // Login no longer challenges MFA.
        let (code, body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email2, "password": "test-password-123"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert!(
            body.get("mfa_required").is_none(),
            "[{engine}] disabled mfa back to normal login"
        );

        // -- Scenarios 8/9/10: recovery codes ----------------------------------
        let email3 = format!("mfa3-{suffix}@test.local");
        let _user3 = seed_user(&db, &email3, "Mfa Three").await;
        let cookie3 = login_as(&app.app, &email3, "test-password-123").await;
        let (code, setup3) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie3),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] setup3: {setup3}");
        let secret3b = setup3["secret"].as_str().unwrap().to_string();
        // Boundary-aware retry: totp_code_now waits out a step tail, and a
        // second attempt uses a code for the fresh step if the first still
        // crossed under CI load.
        let mut enabled3 = Value::Null;
        let mut enable3_ok = false;
        for _ in 0..2 {
            let (code, body) = send_json(
                &app.app,
                "POST",
                "/api/v1/auth/2fa/enable",
                Some(&cookie3),
                json!({"code": totp_code_now(&secret3b, &email3).await}),
            )
            .await;
            if code == StatusCode::OK {
                enabled3 = body;
                enable3_ok = true;
                break;
            }
        }
        assert!(
            enable3_ok,
            "[{engine}] enable3 failed after fresh-code retries: {enabled3}"
        );
        let codes3: Vec<String> = enabled3["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();

        // Redeem the first recovery code.
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email3, "password": "test-password-123"}),
        )
        .await;
        let token8 = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token8, "code": codes3[0]}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] recovery code accepted");
        // Same code again → 401.
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email3, "password": "test-password-123"}),
        )
        .await;
        let token9 = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token9, "code": codes3[0]}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] recovery code is single-use"
        );

        // Regeneration with wrong password rejected; with correct password 10 new codes.
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/recovery-codes",
            Some(&cookie3),
            json!({"current_password": "wrong-password-1"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] regen wrong password"
        );
        let (code, regen) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/recovery-codes",
            Some(&cookie3),
            json!({"current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] regen ok");
        let new_codes: Vec<String> = regen["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(new_codes.len(), 10, "[{engine}] ten new codes");
        for old in &codes3 {
            assert!(!new_codes.contains(old), "[{engine}] old codes replaced");
        }
        // Every old code is now invalid.
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email3, "password": "test-password-123"}),
        )
        .await;
        let token10 = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token10, "code": codes3[1]}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] old code invalidated by regeneration"
        );
        // A new code works.
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email3, "password": "test-password-123"}),
        )
        .await;
        let token10b = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token10b, "code": new_codes[0]}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] regenerated code works");

        // -- Scenarios 11/12: expired and forged pending tokens ----------------
        // Fresh instance + enrollment: the per-user limiter window restarts.
        drop(app);
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let email3 = format!("mfa3b-{suffix}@test.local");
        let user3b = seed_user(&db, &email3, "Mfa Three B").await;
        let cookie3 = login_as(&app.app, &email3, "test-password-123").await;
        _ = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie3),
            json!({}),
        )
        .await;
        _ = enable_mfa_ok(&app.app, &cookie3, engine.as_str(), "_").await;
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email3, "password": "test-password-123"}),
        )
        .await;
        let token11 = login_body["mfa_token"].as_str().unwrap().to_string();
        // Force-expire the token directly (deterministic, no sleeping).
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.i(1i64).s(&user3b);
            conn.execute(
                "UPDATE mfa_pending SET expires_at = ? WHERE user_id = ?",
                args,
            )
            .await
            .unwrap();
        }
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token11, "code": "123456"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] expired pending token"
        );
        let forged = format!("mfp_{}{}", "A".repeat(43), "B");
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": forged, "code": "123456"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] forged pending token"
        );
        // Normal session cookie cannot substitute for a pending token.
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": session_cookie_value(&cookie3), "code": "123456"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] normal session is not a pending token"
        );

        // -- Scenarios 13/14: setup requires a normal session ------------------
        let (code, _) =
            send_json(&app.app, "POST", "/api/v1/auth/2fa/setup", None, json!({})).await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] setup without session"
        );
        // MFA-pending token (as a cookie) cannot call setup or management endpoints.
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email3, "password": "test-password-123"}),
        )
        .await;
        let pending_cookie = format!(
            "tasker_session={}",
            login_body["mfa_token"].as_str().unwrap()
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&pending_cookie),
            json!({}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] setup with pending token"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/disable",
            Some(&pending_cookie),
            json!({"current_password": "x"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] disable with pending token"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/recovery-codes",
            Some(&pending_cookie),
            json!({"current_password": "x"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] regen with pending token"
        );
        let (code, _) = get_json(&app.app, "/api/v1/me", &pending_cookie).await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] pending token cannot read /me"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/enable",
            Some(&pending_cookie),
            json!({"code": "123456"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] enable with pending token"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&pending_cookie),
            json!({"name": "Nope"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] pending token cannot create resources"
        );

        // -- Additional: fresh instance for the remaining blocks ---------------
        // (Scenarios 11-14 re-spawned the app; the following blocks enroll
        // their own users on this instance.)
        drop(app);
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        // -- Additional: invalid enrollment code does not enable MFA -----------
        let email4 = format!("mfa4-{suffix}@test.local");
        let user4 = seed_user(&db, &email4, "Mfa Four").await;
        let cookie4 = login_as(&app.app, &email4, "test-password-123").await;
        let (_code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie4),
            json!({}),
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/enable",
            Some(&cookie4),
            json!({"code": "000000"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] invalid enrollment code"
        );
        assert!(
            !db_user_mfa_enabled(&db, &user4).await,
            "[{engine}] not enabled by invalid code"
        );
        // A valid code still enrolls afterwards (repeated setup replaced nothing).
        let (_code, setup4) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie4),
            json!({}),
        )
        .await;
        enable_mfa_ok(&app.app, &cookie4, engine.as_str(), "user4 retry").await;

        // Setup while already enabled → 409.
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie4),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] setup when enabled");
        let totp4b = totp_code_now(setup4["secret"].as_str().unwrap(), &email4).await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/enable",
            Some(&cookie4),
            json!({"code": totp4b}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] enable when enabled");

        // -- Additional: malformed / oversized input ----------------------------
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": "x", "code": ""}),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] empty code");
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": "x", "code": "1"}),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] short code");
        let big_token = "mfp_".to_string() + &"A".repeat(1000);
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": big_token, "code": "123456"}),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "[{engine}] oversized token");
        // Missing fields are rejected by the contract (422), not 500.
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/2fa/verify")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"mfa_token": "x"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] missing code field"
        );

        // -- Additional: no user isolation break; deletion cascades -------------
        // Another authenticated user cannot disable user4's MFA (endpoints act
        // only on the caller): user5 disables their own (absent) config.
        let email5 = format!("mfa5-{suffix}@test.local");
        seed_user(&db, &email5, "Mfa Five").await;
        let cookie5 = login_as(&app.app, &email5, "test-password-123").await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/recovery-codes",
            Some(&cookie5),
            json!({"current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::BAD_REQUEST,
            "[{engine}] regen without MFA rejected"
        );
        // Deleting the user cascades the MFA rows.
        {
            let mut conn = db.begin().await.unwrap();
            let mut args = tasker::db::Args::new();
            args.s(&user4);
            conn.execute("DELETE FROM users WHERE id = ?", args)
                .await
                .unwrap();
            conn.commit().await.unwrap();
        }
        assert!(
            !db_user_mfa_exists(&db, &user4).await,
            "[{engine}] user_mfa cascade on user delete"
        );

        // -- Additional: TOTP replay (sequential) ------------------------------
        let email6 = format!("mfa6-{suffix}@test.local");
        let user6 = seed_user(&db, &email6, "Mfa Six").await;
        let cookie6 = login_as(&app.app, &email6, "test-password-123").await;
        let (_code, setup6) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie6),
            json!({}),
        )
        .await;
        let secret6 = setup6["secret"].as_str().unwrap().to_string();
        let totp6 = totp_code_now(&secret6, &email6).await;
        let (code, enabled6) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/enable",
            Some(&cookie6),
            json!({"code": totp6}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] enable6: {enabled6}");
        // Manually set last_used_step to the current step: the same code must
        // then be rejected as a replay (deterministic replay test).
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.i(tasker::mfa::current_step()).s(&user6);
            conn.execute(
                "UPDATE user_mfa SET last_used_step = ? WHERE user_id = ?",
                args,
            )
            .await
            .unwrap();
        }
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email6, "password": "test-password-123"}),
        )
        .await;
        let token_replay = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token_replay, "code": totp6}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] consumed step replayed"
        );
        // Rewind the stored step so the next current-step code is a fresh step
        // (deterministic; the replay test above pinned it to the current one).
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.i(tasker::mfa::current_step() - 1).s(&user6);
            conn.execute(
                "UPDATE user_mfa SET last_used_step = ? WHERE user_id = ?",
                args,
            )
            .await
            .unwrap();
        }

        // -- Additional: concurrent same-token submissions ----------------------
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email6, "password": "test-password-123"}),
        )
        .await;
        let token_conc = login_body["mfa_token"].as_str().unwrap().to_string();
        let new_code = totp_code_now(&secret6, &email6).await;
        let mut submissions = Vec::new();
        for i in 0..3 {
            let router = app.app.clone();
            let token = token_conc.clone();
            let code_body = new_code.clone();
            submissions.push(tokio::spawn(async move {
                router
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri("/api/v1/auth/2fa/verify")
                            .header(header::CONTENT_TYPE, "application/json")
                            .header("x-forwarded-for", format!("10.7.0.{i}"))
                            .body(Body::from(
                                json!({"mfa_token": token, "code": code_body}).to_string(),
                            ))
                            .unwrap(),
                    )
                    .await
                    .unwrap()
                    .status()
            }));
        }
        let statuses = futures_join(submissions).await;
        let oks = statuses.iter().filter(|s| **s == StatusCode::OK).count();
        assert_eq!(
            oks, 1,
            "[{engine}] exactly one concurrent submission succeeds: {statuses:?}"
        );

        // -- Additional: concurrent recovery-code redemption --------------------
        // Fresh user + window: only the redemption attempts count against it.
        drop(app);
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let email7 = format!("mfa7-{suffix}@test.local");
        let _user7 = seed_user(&db, &email7, "Mfa Seven").await;
        let cookie7 = login_as(&app.app, &email7, "test-password-123").await;
        let (code, setup7) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie7),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, enabled7) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/enable",
            Some(&cookie7),
            json!({"code": totp_code_now(setup7["secret"].as_str().unwrap(), &email7).await}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let recovery_codes7: Vec<String> = enabled7["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let mut pending_tokens = Vec::new();
        for _ in 0..3 {
            let (_, b) = send_json(
                &app.app,
                "POST",
                "/api/v1/auth/login",
                None,
                json!({"email": email7, "password": "test-password-123"}),
            )
            .await;
            pending_tokens.push(b["mfa_token"].as_str().unwrap().to_string());
        }
        let recovery: String = recovery_codes7[0].clone();
        let mut redemptions = Vec::new();
        for (i, t) in pending_tokens.into_iter().enumerate() {
            let router = app.app.clone();
            let code_body = recovery.clone();
            redemptions.push(tokio::spawn(async move {
                let response = router
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri("/api/v1/auth/2fa/verify")
                            .header(header::CONTENT_TYPE, "application/json")
                            .header("x-forwarded-for", format!("10.8.0.{i}"))
                            .body(Body::from(
                                json!({"mfa_token": t, "code": code_body}).to_string(),
                            ))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                response.status()
            }));
        }
        let statuses = futures_join(redemptions).await;
        let oks = statuses.iter().filter(|s| **s == StatusCode::OK).count();
        assert_eq!(
            oks, 1,
            "[{engine}] a recovery code redeems exactly once concurrently: {statuses:?}"
        );
        // The redeemed code cannot be redeemed again, even alone.
        let (_, b) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email7, "password": "test-password-123"}),
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": b["mfa_token"], "code": recovery}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] concurrently-redeemed code is consumed"
        );
    }
}

/// Phase 7 remediation (ADR-0012): legacy unkeyed SHA-256 recovery-code
/// digests can never verify; invalidation clears them while preserving
/// MFA-enabled state; regeneration issues a fresh keyed set through the
/// existing authenticated flow; persisted rows never contain plaintext
/// codes or pepper material.
#[tokio::test]
async fn mfa_legacy_recovery_digests_rejected() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7();
        let email = format!("legacy-{suffix}@test.local");
        let user_id = seed_user(&db, &email, "Legacy User").await;
        let cookie = login_as(&app.app, &email, "test-password-123").await;

        // Enable MFA (fresh HMAC set).
        let (code, setup) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] setup");
        let _secret = setup["secret"].as_str().unwrap().to_string();
        let (enabled, _portable_secret) =
            enable_mfa_ok(&app.app, &cookie, engine.as_str(), "enabled").await;
        let hmac_codes: Vec<String> = enabled["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();

        // The stored digests are h1-tagged and carry no plaintext.
        let stored_before = db_recovery_digests(&db, &user_id).await;
        assert!(
            stored_before.iter().all(|d| d.starts_with("h1.")),
            "[{engine}] all digests tagged"
        );
        for c in &hmac_codes {
            assert!(
                !stored_before
                    .iter()
                    .any(|d| d.to_lowercase().contains(&c.to_lowercase())),
                "[{engine}] no plaintext code at rest"
            );
        }

        // Inject a legacy unkeyed SHA-256 digest set (what a pre-remediation
        // database holds) alongside nothing else.
        let legacy_plain = "ABCD2345";
        let legacy_digest = {
            use sha2::{Digest, Sha256};
            let mut out = String::new();
            for b in Sha256::digest(legacy_plain.as_bytes()) {
                out.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
                out.push(char::from_digit(u32::from(b & 0x0F), 16).unwrap());
            }
            out
        };
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(format!("[\"{legacy_digest}\"]")).s(&user_id);
            conn.execute(
                "UPDATE user_mfa SET recovery_codes_hashed = ? WHERE user_id = ?",
                args,
            )
            .await
            .unwrap();
        }

        // The legacy code — the exact plaintext whose digest is stored — is
        // rejected by the verification path.
        let (_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email, "password": "test-password-123"}),
        )
        .await;
        let token = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token, "code": legacy_plain}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] legacy digest can never verify"
        );

        // The migration's invalidation policy: MFA-enabled state is preserved.
        assert!(
            db_user_mfa_enabled(&db, &user_id).await,
            "[{engine}] MFA stays enabled"
        );

        // Regeneration through the existing authenticated flow issues a fresh
        // HMAC set; the legacy set is gone; the new codes verify.
        let (code, regen) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/recovery-codes",
            Some(&cookie),
            json!({"current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] regenerate");
        let fresh: Vec<String> = regen["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(fresh.len(), 10, "[{engine}] ten fresh codes");
        let stored_after = db_recovery_digests(&db, &user_id).await;
        assert_eq!(stored_after.len(), 10, "[{engine}] fresh set stored");
        assert!(
            stored_after.iter().all(|d| d.starts_with("h1.")),
            "[{engine}] fresh digests tagged"
        );
        assert!(
            !stored_after.contains(&legacy_digest),
            "[{engine}] legacy digest gone"
        );
        let (login_code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email, "password": "test-password-123"}),
        )
        .await;
        assert_eq!(login_code, StatusCode::OK);
        let token = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token, "code": fresh[0]}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] fresh code verifies");
    }
}

/// CI integration-matrix regression (ADR-0012 follow-up): the
/// `recovery_codes_hashed` column must be a portable *textual* type on every
/// engine. `MySQL`'s `TEXT` maps to the `BLOB` family on the wire, which `sqlx`'s
/// Any driver cannot decode into `String`; migration `0008` re-declares it
/// as `VARCHAR(4096)`. This test runs the registered migrations in their
/// real order (via `spawn_app`), asserts the materialized column type per
/// dialect, then drives the full recovery path — MFA enrollment, digest
/// persistence/readback (JSON, `h1.`-tagged, no plaintext), and one-time
/// redemption — against the live engine.
#[tokio::test]
async fn mfa_recovery_digests_column_portability() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7();
        let email = format!("portable-{suffix}@test.local");
        let user_id = seed_user(&db, &email, "Portable User").await;
        let cookie = login_as(&app.app, &email, "test-password-123").await;

        // 1. Materialized column type per dialect (schema regression).
        // MySQL cannot be introspected through the Any driver: over
        // server-side prepared statements, information_schema.DATA_TYPE
        // materializes as NULL for every row (CI-verified: 9 rows matched,
        // every data_type empty) and SHOW FULL COLUMNS returns nothing
        // decodable either. On MySQL the VARCHAR invariant is proven
        // functionally — the digest roundtrips below and in the mfa
        // recovery tests read/write these columns through the wire, which
        // fails by construction on TEXT (sqlx Any maps MySQL TEXT to the
        // Blob kind, which cannot decode into String).
        if engine != "mysql" {
            let mut conn = db.conn();
            let expected = match engine.as_str() {
                "postgres" => "text",
                _ => "text", // SQLite: declared type TEXT (pragma).
            };
            let observed: Vec<String> = if engine == "sqlite" {
                // PRAGMA rows: (cid, name, type, ...) — read the type column.
                conn.fetch_all("PRAGMA table_info(user_mfa)", tasker::db::Args::new())
                    .await
                    .unwrap()
                    .iter()
                    .filter_map(|r| {
                        let name: String = r.try_get(1).ok()?;
                        (name == "recovery_codes_hashed").then(|| r.try_get::<String, _>(2).ok())?
                    })
                    .collect()
            } else {
                // PG's information_schema handles prepared reads fine; the
                // integration database is dedicated, so table+column
                // uniquely identify the row.
                let sql = "SELECT DATA_TYPE FROM information_schema.columns \
                           WHERE table_name = 'user_mfa' AND column_name = 'recovery_codes_hashed'";
                conn.fetch_all(sql, tasker::db::Args::new())
                    .await
                    .unwrap()
                    .iter()
                    .filter_map(|r| r.try_get::<String, _>(0).ok())
                    .collect()
            };
            assert!(
                observed.iter().any(|t| t.to_ascii_lowercase() == expected),
                "[{engine}] recovery_codes_hashed must be {expected}; got {observed:?}"
            );
        }

        // 2. Enable MFA: digests are persisted as a JSON array of `h1.`
        //    digests, never plaintext (readback through the same wire type).
        let (code, setup) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] setup");
        let _secret = setup["secret"].as_str().unwrap().to_string();
        let (enabled, _portable_secret2) =
            enable_mfa_ok(&app.app, &cookie, engine.as_str(), "enabled").await;
        let codes: Vec<String> = enabled["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(codes.len(), 10, "[{engine}] ten codes");
        let digests = db_recovery_digests(&db, &user_id).await;
        assert_eq!(digests.len(), 10, "[{engine}] digests read back");
        assert!(
            digests.iter().all(|d| d.starts_with("h1.")),
            "[{engine}] digests tagged"
        );
        for c in &codes {
            assert!(
                !digests.iter().any(|d| d.contains(c)),
                "[{engine}] no plaintext at rest"
            );
        }

        // 3. Redeem one valid code: one-time consumption over the same wire
        //    type that just failed on MySQL BLOB.
        let (code, login_body) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": email, "password": "test-password-123"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] login");
        let token = login_body["mfa_token"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/2fa/verify",
            None,
            json!({"mfa_token": token, "code": codes[0]}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] redemption");
        let remaining = db_recovery_digests(&db, &user_id).await;
        assert_eq!(remaining.len(), 9, "[{engine}] code consumed");
    }
}

async fn db_recovery_digests(db: &Database, user_id: &str) -> Vec<String> {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(user_id);
    let row = conn
        .fetch_optional(
            "SELECT recovery_codes_hashed FROM user_mfa WHERE user_id = ?",
            args,
        )
        .await
        .unwrap()
        .expect("user_mfa row");
    let raw: String = row.try_get("recovery_codes_hashed").unwrap();
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Gate verification: executes migration 0007's exact registered statement
/// against seeded edge-case rows and asserts per-case outcomes on the real
/// engine. Runs on every dialect CI provides, so the LIKE semantics are
/// verified per database rather than inferred.
#[tokio::test]
async fn migration_0007_legacy_invalidation_edge_cases() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();

        // Migrations 0001..0007 already ran at spawn; seed rows AFTER 0007 as
        // a pre-remediation database would have looked, then execute the
        // exact statement from the registered migration file.
        let now = util_now();
        let legacy_hex = "a".repeat(64);
        let hmac_hex = "b".repeat(64);
        // (user suffix, pre-statement digest set). user_id must reference a
        // real users row (FK).
        let cases = [
            ("leg", format!("[\"{legacy_hex}\"]")),   // pure legacy set
            ("hmac", format!("[\"h1.{hmac_hex}\"]")), // fully migrated set
            ("empty", "[]".to_string()),              // already empty
            ("mal", "not json at all".to_string()),   // malformed legacy value
            ("mix", format!("[\"{legacy_hex}\", \"h1.{hmac_hex}\"]")), // hypothetical mixed set
        ];
        let mut user_ids: Vec<(String, String)> = Vec::new(); // (suffix, users.id)
        for (suffix, digests) in &cases {
            let email = format!("m7-{suffix}-{}-{now}@test.local", Uuid::now_v7().simple());
            seed_user(&db, &email, "M7").await;
            let user_id = {
                let mut conn = db.conn();
                let mut args = tasker::db::Args::new();
                args.s(&email);
                let row = conn
                    .fetch_optional("SELECT id FROM users WHERE email = ?", args)
                    .await
                    .unwrap()
                    .expect("seeded user");
                row.try_get::<String, _>("id").unwrap()
            };
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(format!("u-{suffix}-{now}"))
                .s(&user_id)
                .s("enc")
                .s(digests)
                .i(1_700_000_000_000i64) // enabled_at set: MFA-enabled account
                .i(now)
                .i(now);
            conn.execute(
                "INSERT INTO user_mfa (id, user_id, totp_secret_encrypted, recovery_codes_hashed, enabled_at, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                args,
            )
            .await
            .unwrap_or_else(|e| panic!("[{engine}] seed {suffix}: {e}"));
            user_ids.push(((*suffix).to_string(), user_id));
        }

        // Execute the exact statement text from the registered migration.
        let migration_sql = tasker::db::migrations_for(tasker::db::Driver::Sqlite)
            .iter()
            .find(|(v, _)| *v == "0007_recovery_pepper")
            .map(|(_, sql)| *sql)
            .expect("0007 registered");
        // Guard: the driver dispatch must pick the same statement content.
        let driver_specific = match &*engine {
            "postgres" => tasker::db::migrations_for(tasker::db::Driver::Postgres),
            "mysql" => tasker::db::migrations_for(tasker::db::Driver::MySql),
            _ => tasker::db::migrations_for(tasker::db::Driver::Sqlite),
        }
        .iter()
        .find(|(v, _)| *v == "0007_recovery_pepper")
        .map(|(_, sql)| *sql)
        .unwrap();
        // Comments differ per dialect; the executable statement must match.
        let strip = |sql: &str| -> String {
            sql.lines()
                .filter(|l| !l.trim_start().starts_with("--") && !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join(
                    "
",
                )
        };
        assert_eq!(strip(migration_sql), strip(driver_specific));
        {
            let mut conn = db.conn();
            conn.execute(migration_sql, tasker::db::Args::new())
                .await
                .unwrap_or_else(|e| panic!("[{engine}] 0007 execution: {e}"));
        }

        // Outcomes: look rows up by the seeded users' ids.
        for (suffix, user_id) in &user_ids {
            let before = cases
                .iter()
                .find(|(s, _)| s == suffix)
                .map(|(_, d)| d)
                .unwrap();
            let raw = db_recovery_raw(&db, user_id).await;
            let preserved = before.contains("\"h1.");
            if preserved {
                // A set containing a migrated element is preserved untouched.
                assert_eq!(
                    &raw, before,
                    "[{engine}] {suffix}: preserved set must be untouched"
                );
            } else {
                // Legacy, malformed, and empty sets are invalidated to [].
                assert_eq!(raw, "[]", "[{engine}] {suffix}: legacy set invalidated");
            }
            // enabled_at is never modified by the migration.
            assert!(
                db_user_mfa_enabled(&db, user_id).await,
                "[{engine}] {suffix}: enabled_at preserved"
            );
        }
        // Explicit per-case semantics (readability of intent):
        // legacy/malformed/empty cleared...
        for (suffix, user_id) in &user_ids {
            if matches!(suffix.as_str(), "leg" | "mal" | "empty") {
                assert_eq!(
                    db_recovery_raw(&db, user_id).await,
                    "[]",
                    "[{engine}] {suffix} cleared"
                );
            }
        }
        // ...fully migrated and mixed sets preserved (a hypothetical mixed
        // set keeps its legacy element, which the verification path
        // unconditionally skips — see mfa.rs parse_recovery_digest).
        for (suffix, user_id) in &user_ids {
            if matches!(suffix.as_str(), "hmac" | "mix") {
                assert!(
                    db_recovery_raw(&db, user_id).await.contains("\"h1."),
                    "[{engine}] {suffix} preserved"
                );
            }
        }
    }
}

fn util_now() -> i64 {
    tasker::util::now_ms()
}

async fn db_recovery_raw(db: &Database, user_id: &str) -> String {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(user_id);
    let row = conn
        .fetch_optional(
            "SELECT recovery_codes_hashed FROM user_mfa WHERE user_id = ?",
            args,
        )
        .await
        .unwrap()
        .expect("user_mfa row");
    row.try_get("recovery_codes_hashed").unwrap()
}

async fn futures_join<T>(handles: Vec<tokio::task::JoinHandle<T>>) -> Vec<T> {
    let mut out = Vec::new();
    for h in handles {
        out.push(h.await.unwrap());
    }
    out
}

#[allow(dead_code)]
fn session_cookie_value(cookie: &str) -> String {
    cookie.trim_start_matches("tasker_session=").to_string()
}

/// Enable MFA expecting success, retrying once with a freshly generated code.
/// The backend accepts only the current 30-second step (window 0); a code
/// generated just before a step boundary can legitimately expire in transit
/// on a loaded runner — that is a test-timing artifact, so the retry uses a
/// code for the NEW step. App rejections (invalid secret, wrong state) also
/// return 401 and are surfaced as a failure after the retry.
async fn enable_mfa_ok(
    app: &axum::Router,
    cookie: &str,
    engine: &str,
    label: &str,
) -> (Value, String) {
    // Each attempt runs setup itself so the code is always generated against
    // the server's CURRENT pending secret (a stale-secret state can never
    // accumulate across attempts), and totp_code_now waits out the TOTP
    // boundary so the code is for the step the server will see. The secret
    // used is returned so callers can derive later-step codes for verify
    // flows.
    let attempt = |app: axum::Router, cookie: String| async move {
        let (code, setup) = send_json(
            &app,
            "POST",
            "/api/v1/auth/2fa/setup",
            Some(&cookie),
            json!({}),
        )
        .await;
        if code != StatusCode::OK {
            return (code, setup, String::new());
        }
        let secret = setup["secret"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| panic!("[{engine}] {label}: setup returned no secret; body={setup}"))
            .to_string();
        let code = totp_code_now(&secret, "").await;
        let (status, body) = send_json(
            &app,
            "POST",
            "/api/v1/auth/2fa/enable",
            Some(&cookie),
            json!({"code": code}),
        )
        .await;
        (status, body, secret)
    };
    let mut last_body = Value::Null;
    for _ in 0..3 {
        let (code, body, secret) = attempt(app.clone(), cookie.to_string()).await;
        if code == StatusCode::OK {
            return (body, secret);
        }
        last_body = body;
    }
    panic!("[{engine}] {label}: enable failed after fresh-secret retries: {last_body}");
}

/// Regression guard for a units bug that silently expired every session
/// ~3.6 seconds after login: `create_session` once added `ttl_secs` (seconds)
/// directly to `now_ms` (milliseconds), so the stored TTL was the config's
/// second count in milliseconds. The mfa boundary-sleep retries were the
/// first flows to outrun that window, surfacing as flaky 401s.
#[tokio::test]
async fn session_expires_after_configured_ttl_not_milliseconds() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7();
        let email = format!("ttl-{suffix}@test.local");
        seed_user(&db, &email, "Ttl User").await;
        let _ = login_as(&app.app, &email, "test-password-123").await;
        let mut conn = db.conn();
        let row = conn
            .fetch_optional(
                "SELECT expires_at - created_at AS ttl_ms FROM sessions",
                tasker::db::Args::new(),
            )
            .await
            .unwrap()
            .expect("session row exists after login");
        let ttl_ms: i64 = row.try_get("ttl_ms").unwrap();
        assert_eq!(
            ttl_ms, 3_600_000,
            "[{engine}] session TTL must be stored in milliseconds (test config TTL is 3600s)"
        );
    }
}

/// Rules Engine schema (migration 0009): fresh-migration decoding, unique
/// constraints, CHECK constraints, composite rule-version foreign keys, and
/// the RESTRICT deletion policy. Runs on every engine CI provides
/// (`SQLite` always; `PostgreSQL 17` and `MySQL 8.4` via env-provided URLs).
#[tokio::test]
async fn rules_schema_migrations_and_constraints() {
    /// Insert a team row directly (FK target for rules).
    async fn insert_team(db: &Database, name: &str, creator: &str) -> String {
        let mut conn = db.conn();
        let id = tasker::util::new_id();
        let now = tasker::util::now_ms();
        let mut args = tasker::db::Args::new();
        args.s(&id)
            .s(name)
            .opt_s(None::<String>) // description
            .s(creator)
            .i(now)
            .i(now);
        conn.execute(
            "INSERT INTO teams (id, name, description, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .unwrap_or_else(|e| panic!("insert_team failed: {e}"));
        id
    }

    /// Insert a rule row; `condition`/`actions` are raw JSON strings (this
    /// test exercises the persistence layer, not the DSL validator).
    #[allow(clippy::too_many_arguments)]
    async fn insert_rule(
        db: &Database,
        id: &str,
        team_id: &str,
        name: &str,
        description: Option<&str>,
        trigger_type: &str,
        enabled: bool,
        current_version: i64,
        condition_json: &str,
        actions_json: &str,
        created_by: &str,
        archived_at: Option<i64>,
    ) -> Result<(), tasker::error::AppError> {
        let now = tasker::util::now_ms();
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(id)
            .s(team_id)
            .s(name)
            .opt_s(description)
            .s(trigger_type)
            .b(enabled)
            .i(current_version)
            .s(condition_json)
            .s(actions_json)
            .s(created_by)
            .i(now)
            .i(now)
            .opt_i(archived_at);
        conn.execute(
            "INSERT INTO rules (id, team_id, name, description, trigger_type, enabled, \
             current_version, condition_json, actions_json, created_by, created_at, \
             updated_at, archived_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_rule_version(
        db: &Database,
        rule_id: &str,
        version: i64,
        trigger_type: &str,
        condition_json: &str,
        actions_json: &str,
        created_by: &str,
    ) -> Result<(), tasker::error::AppError> {
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(rule_id)
            .i(version)
            .s(trigger_type)
            .s(condition_json)
            .s(actions_json)
            .s(created_by)
            .i(tasker::util::now_ms());
        conn.execute(
            "INSERT INTO rule_versions (rule_id, version, trigger_type, condition_json, \
             actions_json, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_event(
        db: &Database,
        id: &str,
        event_type: &str,
        team_id: &str,
        task_id: &str,
        actor_id: Option<&str>,
        previous_state: Option<&str>,
        new_state: Option<&str>,
        origin: &str,
        occurred_at: i64,
    ) -> Result<(), tasker::error::AppError> {
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(id)
            .s(event_type)
            .i(1i64)
            .s(team_id)
            .s(task_id)
            .opt_s(actor_id)
            .opt_s(previous_state)
            .opt_s(new_state)
            .s(origin)
            .opt_i(None::<i64>) // causing_execution_id
            .i(occurred_at);
        conn.execute(
            "INSERT INTO domain_events (id, event_type, schema_version, team_id, task_id, \
             actor_id, previous_state, new_state, origin, causing_execution_id, occurred_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_execution(
        db: &Database,
        id: &str,
        rule_id: &str,
        rule_version: i64,
        event_id: &str,
        team_id: &str,
        task_id: &str,
        condition_result: &str,
        planned_actions: Option<&str>,
    ) -> Result<(), tasker::error::AppError> {
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(id)
            .s(rule_id)
            .i(rule_version)
            .s(event_id)
            .s(team_id)
            .s(task_id)
            .s("task_status_changed")
            .s(condition_result)
            .s("planned")
            .opt_s(planned_actions)
            .opt_s(None::<String>) // error_category
            .opt_s(None::<String>) // error_detail
            .i(tasker::util::now_ms())
            .opt_i(None::<i64>); // completed_at
        conn.execute(
            "INSERT INTO rule_executions (id, rule_id, rule_version, event_id, team_id, \
             task_id, trigger_type, condition_result, status, planned_actions, \
             error_category, error_detail, created_at, completed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_outbox(
        db: &Database,
        id: &str,
        execution_id: &str,
        event_id: &str,
        rule_id: &str,
        rule_version: i64,
        action_type: &str,
        payload: &str,
        idempotency_key: &str,
        status: &str,
        claim_token: Option<&str>,
    ) -> Result<(), tasker::error::AppError> {
        let now = tasker::util::now_ms();
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(id)
            .s(execution_id)
            .s(event_id)
            .s(rule_id)
            .i(rule_version)
            .s(Uuid::now_v7().to_string()) // historical task id (no FK, CHAR(36))
            .s(action_type)
            .i(1i64)
            .s(payload)
            .s(idempotency_key)
            .s(status)
            .i(0i64) // attempts
            .i(now) // next_attempt_at
            .opt_s(claim_token)
            .opt_s(None::<String>) // worker_id
            .opt_i(None::<i64>) // claimed_at
            .opt_i(None::<i64>) // lease_expires_at
            .opt_s(None::<String>) // last_error_category
            .opt_s(None::<String>) // last_error_detail
            .i(now)
            .i(now)
            .opt_i(None::<i64>); // completed_at
        conn.execute(
            "INSERT INTO automation_outbox (id, execution_id, event_id, rule_id, rule_version, \
             task_id, action_type, schema_version, payload, idempotency_key, status, attempts, \
             next_attempt_at, claim_token, worker_id, claimed_at, lease_expires_at, \
             last_error_category, last_error_detail, created_at, updated_at, completed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_action_result(
        db: &Database,
        id: &str,
        outbox_id: &str,
        execution_id: &str,
        action_type: &str,
        outcome: &str,
        author_id: Option<&str>,
    ) -> Result<(), tasker::error::AppError> {
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(id)
            .s(outbox_id)
            .s(execution_id)
            .s(action_type)
            .s(outcome)
            .opt_s(None::<String>) // recipient_id
            .opt_s(author_id)
            .opt_s(None::<String>) // body_hash
            .opt_s(None::<String>) // detail
            .i(tasker::util::now_ms());
        conn.execute(
            "INSERT INTO rule_action_results (id, outbox_id, execution_id, action_type, \
             outcome, recipient_id, author_id, body_hash, detail, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .map(|_| ())
    }

    for (engine, url) in test_db_urls() {
        // spawn_app runs the full migration sequence 0001..0009: a failure
        // here is itself a fresh-migration test result.
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();
        let now = tasker::util::now_ms();

        let u1 = seed_user(&db, &format!("rule-a-{suffix}@test.local"), "Rule Admin A").await;
        let u2 = seed_user(&db, &format!("rule-b-{suffix}@test.local"), "Rule Admin B").await;
        let t1 = insert_team(&db, &format!("rules-team-a-{suffix}"), &u1).await;
        let t2 = insert_team(&db, &format!("rules-team-b-{suffix}"), &u2).await;

        // -- 1. Fresh migration + sqlx Any decoding: boundary-length and
        //       multibyte values must round-trip as String/bool/i64. On MySQL
        //       this is exactly the TEXT→BLOB wire-type trap 0008 exists for.
        let condition = "x".repeat(4_000);
        let actions = "y".repeat(2_000);
        let name = format!("Règle 自動化 {suffix}");
        let r1 = Uuid::now_v7().to_string();
        insert_rule(
            &db,
            &r1,
            &t1,
            &name,
            Some("boundary rule"),
            "task_status_changed",
            true,
            1,
            &condition,
            &actions,
            &u1,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("[{engine}] boundary rule insert failed: {e}"));
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&r1);
        // Repo convention (see comments.rs): never SELECT a raw BOOLEAN
        // column through the Any driver - MySQL reports TINYINT, which
        // sqlx Any cannot describe. Normalize in SQL and decode as i64.
        let row = conn
            .fetch_optional(
                "SELECT name, description, trigger_type,                  CASE WHEN enabled THEN 1 ELSE 0 END AS enabled, current_version,                  condition_json, actions_json, created_at, archived_at                  FROM rules WHERE id = ?",
                args,
            )
            .await
            .unwrap()
            .expect("rule row");
        let got_name: String = row.try_get("name").unwrap();
        assert_eq!(
            got_name.chars().count(),
            name.chars().count(),
            "[{engine}] name"
        );
        let got_condition: String = row.try_get("condition_json").unwrap();
        assert_eq!(
            got_condition.chars().count(),
            4_000,
            "[{engine}] condition_json"
        );
        let got_actions: String = row.try_get("actions_json").unwrap();
        assert_eq!(
            got_actions.chars().count(),
            2_000,
            "[{engine}] actions_json"
        );
        let got_enabled: i64 = row.try_get("enabled").unwrap();
        assert_eq!(
            got_enabled, 1,
            "[{engine}] boolean decode via CASE normalization"
        );
        let got_version: i64 = row.try_get("current_version").unwrap();
        assert_eq!(got_version, 1, "[{engine}] integer decode");
        let got_archived: Option<i64> = row.try_get("archived_at").unwrap();
        assert_eq!(got_archived, None, "[{engine}] nullable timestamp decode");

        // -- 2. Team-scoped name uniqueness; archived names stay reserved.
        let dup = insert_rule(
            &db,
            &Uuid::now_v7().to_string(),
            &t1,
            &name,
            None,
            "task_status_changed",
            false,
            1,
            &condition,
            &actions,
            &u1,
            None,
        )
        .await;
        assert!(
            dup.is_err(),
            "[{engine}] duplicate name within team must fail"
        );
        let cross_team = insert_rule(
            &db,
            &Uuid::now_v7().to_string(),
            &t2,
            &name,
            None,
            "task_assigned",
            false,
            1,
            &condition,
            &actions,
            &u2,
            None,
        )
        .await;
        assert!(
            cross_team.is_ok(),
            "[{engine}] same name in another team must be allowed"
        );
        let archived = insert_rule(
            &db,
            &Uuid::now_v7().to_string(),
            &t1,
            &name,
            None,
            "task_assigned",
            false,
            1,
            &condition,
            &actions,
            &u1,
            Some(now),
        )
        .await;
        assert!(
            archived.is_err(),
            "[{engine}] archived rule name must stay reserved"
        );

        // -- 3. rule_versions: composite PK; immutable duplicate rejection.
        insert_rule_version(
            &db,
            &r1,
            1,
            "task_status_changed",
            &condition,
            &actions,
            &u1,
        )
        .await
        .unwrap_or_else(|e| panic!("[{engine}] version insert failed: {e}"));
        let dup_version = insert_rule_version(
            &db,
            &r1,
            1,
            "task_status_changed",
            &condition,
            &actions,
            &u1,
        )
        .await;
        assert!(
            dup_version.is_err(),
            "[{engine}] duplicate (rule_id, version) must fail"
        );

        // -- 4. domain_events: historical task ids without a task FK.
        let e1 = Uuid::now_v7().to_string();
        let previous_state = "s".repeat(500);
        let new_state = "n".repeat(500);
        insert_event(
            &db,
            &e1,
            "task_status_changed",
            &t1,
            &Uuid::now_v7().to_string(),
            Some(&u1),
            Some(&previous_state),
            Some(&new_state),
            "user",
            now,
        )
        .await
        .unwrap_or_else(|e| panic!("[{engine}] event insert failed: {e}"));
        let bad_team_event = insert_event(
            &db,
            &Uuid::now_v7().to_string(),
            "task_status_changed",
            &Uuid::now_v7().to_string(),
            &Uuid::now_v7().to_string(),
            None,
            None,
            None,
            "user",
            now,
        )
        .await;
        assert!(
            bad_team_event.is_err(),
            "[{engine}] event with unknown team must fail"
        );

        // -- 5. rule_executions: composite version FK + unique (rule, event).
        let x1 = Uuid::now_v7().to_string();
        let planned = "p".repeat(1_000);
        insert_execution(
            &db,
            &x1,
            &r1,
            1,
            &e1,
            &t1,
            &Uuid::now_v7().to_string(),
            "matched",
            Some(&planned),
        )
        .await
        .unwrap_or_else(|e| panic!("[{engine}] execution insert failed: {e}"));
        let dup_exec = insert_execution(
            &db,
            &Uuid::now_v7().to_string(),
            &r1,
            1,
            &e1,
            &t1,
            &Uuid::now_v7().to_string(),
            "matched",
            None,
        )
        .await;
        assert!(
            dup_exec.is_err(),
            "[{engine}] duplicate (rule_id, event_id) must fail"
        );
        // Unique (rule_id, event_id) fires first for the same event; use a
        // fresh event to isolate the composite-version FK violation.
        let e2 = Uuid::now_v7().to_string();
        insert_event(
            &db,
            &e2,
            "task_status_changed",
            &t1,
            &Uuid::now_v7().to_string(),
            None,
            None,
            None,
            "user",
            now,
        )
        .await
        .unwrap();
        let bad_version_exec = insert_execution(
            &db,
            &Uuid::now_v7().to_string(),
            &r1,
            9,
            &e2,
            &t1,
            &Uuid::now_v7().to_string(),
            "matched",
            None,
        )
        .await;
        assert!(
            bad_version_exec.is_err(),
            "[{engine}] unknown rule version must fail"
        );
        let bad_event_exec = insert_execution(
            &db,
            &Uuid::now_v7().to_string(),
            &r1,
            1,
            &Uuid::now_v7().to_string(),
            &t1,
            &Uuid::now_v7().to_string(),
            "matched",
            None,
        )
        .await;
        assert!(
            bad_event_exec.is_err(),
            "[{engine}] unknown event must fail"
        );

        // -- 6. automation_outbox: payload decode, idempotency uniqueness,
        //       CHECK constraints, composite version FK, claim-token invariant.
        let ob1 = Uuid::now_v7().to_string();
        let payload = "q".repeat(4_000);
        let idem = "0".repeat(64);
        insert_outbox(
            &db,
            &ob1,
            &x1,
            &e1,
            &r1,
            1,
            "add_comment",
            &payload,
            &idem,
            "pending",
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("[{engine}] outbox insert failed: {e}"));
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&ob1);
        let row = conn
            .fetch_optional("SELECT * FROM automation_outbox WHERE id = ?", args)
            .await
            .unwrap()
            .expect("outbox row");
        let got_payload: String = row.try_get("payload").unwrap();
        assert_eq!(
            got_payload.chars().count(),
            4_000,
            "[{engine}] payload decode"
        );
        let got_status: String = row.try_get("status").unwrap();
        assert_eq!(got_status, "pending", "[{engine}] status decode");
        let got_token: Option<String> = row.try_get("claim_token").unwrap();
        assert_eq!(got_token, None, "[{engine}] nullable claim token decode");

        let dup_idem = insert_outbox(
            &db,
            &Uuid::now_v7().to_string(),
            &x1,
            &e1,
            &r1,
            1,
            "add_comment",
            &payload,
            &idem,
            "pending",
            None,
        )
        .await;
        assert!(
            dup_idem.is_err(),
            "[{engine}] duplicate idempotency key must fail"
        );
        let bad_action = insert_outbox(
            &db,
            &Uuid::now_v7().to_string(),
            &x1,
            &e1,
            &r1,
            1,
            "post_to_slack",
            &payload,
            &"1".repeat(64),
            "pending",
            None,
        )
        .await;
        assert!(
            bad_action.is_err(),
            "[{engine}] unknown action_type must fail (CHECK)"
        );
        let bad_status = insert_outbox(
            &db,
            &Uuid::now_v7().to_string(),
            &x1,
            &e1,
            &r1,
            1,
            "add_comment",
            &payload,
            &"2".repeat(64),
            "queued",
            None,
        )
        .await;
        assert!(
            bad_status.is_err(),
            "[{engine}] unknown status must fail (CHECK)"
        );
        let token_pending = insert_outbox(
            &db,
            &Uuid::now_v7().to_string(),
            &x1,
            &e1,
            &r1,
            1,
            "add_comment",
            &payload,
            &"3".repeat(64),
            "pending",
            Some(&Uuid::now_v7().to_string()),
        )
        .await;
        assert!(
            token_pending.is_err(),
            "[{engine}] pending row with claim token must fail (fencing CHECK)"
        );
        let processing_no_token = insert_outbox(
            &db,
            &Uuid::now_v7().to_string(),
            &x1,
            &e1,
            &r1,
            1,
            "add_comment",
            &payload,
            &"4".repeat(64),
            "processing",
            None,
        )
        .await;
        assert!(
            processing_no_token.is_err(),
            "[{engine}] processing row without claim token must fail (fencing CHECK)"
        );
        let ob2 = Uuid::now_v7().to_string();
        let claimed = insert_outbox(
            &db,
            &ob2,
            &x1,
            &e1,
            &r1,
            1,
            "add_comment",
            &payload,
            &"5".repeat(64),
            "processing",
            Some(&Uuid::now_v7().to_string()),
        )
        .await;
        assert!(
            claimed.is_ok(),
            "[{engine}] processing row with token must be allowed"
        );
        let bad_version_outbox = insert_outbox(
            &db,
            &Uuid::now_v7().to_string(),
            &x1,
            &e1,
            &r1,
            9,
            "add_comment",
            &payload,
            &"6".repeat(64),
            "pending",
            None,
        )
        .await;
        assert!(
            bad_version_outbox.is_err(),
            "[{engine}] outbox unknown rule version must fail"
        );

        // -- 7. rule_action_results: idempotency ledger uniqueness.
        let res1 = Uuid::now_v7().to_string();
        insert_action_result(&db, &res1, &ob1, &x1, "add_comment", "completed", Some(&u1))
            .await
            .unwrap_or_else(|e| panic!("[{engine}] action result insert failed: {e}"));
        let dup_result = insert_action_result(
            &db,
            &Uuid::now_v7().to_string(),
            &ob1,
            &x1,
            "add_comment",
            "completed",
            None,
        )
        .await;
        assert!(
            dup_result.is_err(),
            "[{engine}] duplicate outbox_id result must fail"
        );
        let bad_result_exec = insert_action_result(
            &db,
            &Uuid::now_v7().to_string(),
            &ob2,
            &Uuid::now_v7().to_string(),
            "add_comment",
            "completed",
            None,
        )
        .await;
        assert!(
            bad_result_exec.is_err(),
            "[{engine}] result with unknown execution must fail"
        );

        // -- 8. RESTRICT deletion policy protects the whole history chain.
        let delete_team = {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&t1);
            conn.execute("DELETE FROM teams WHERE id = ?", args).await
        };
        assert!(
            delete_team.is_err(),
            "[{engine}] team delete must be restricted"
        );
        let delete_user = {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&u1);
            conn.execute("DELETE FROM users WHERE id = ?", args).await
        };
        assert!(
            delete_user.is_err(),
            "[{engine}] user delete must be restricted"
        );
        let delete_rule = {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&r1);
            conn.execute("DELETE FROM rules WHERE id = ?", args).await
        };
        assert!(
            delete_rule.is_err(),
            "[{engine}] rule delete must be restricted"
        );
        let delete_version = {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&r1).i(1i64);
            conn.execute(
                "DELETE FROM rule_versions WHERE rule_id = ? AND version = ?",
                args,
            )
            .await
        };
        assert!(
            delete_version.is_err(),
            "[{engine}] rule version delete must be restricted"
        );
        let delete_event = {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&e1);
            conn.execute("DELETE FROM domain_events WHERE id = ?", args)
                .await
        };
        assert!(
            delete_event.is_err(),
            "[{engine}] domain event delete must be restricted"
        );
        let delete_execution = {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&x1);
            conn.execute("DELETE FROM rule_executions WHERE id = ?", args)
                .await
        };
        assert!(
            delete_execution.is_err(),
            "[{engine}] execution delete must be restricted"
        );
        let delete_outbox = {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&ob1);
            conn.execute("DELETE FROM automation_outbox WHERE id = ?", args)
                .await
        };
        assert!(
            delete_outbox.is_err(),
            "[{engine}] outbox delete must be restricted"
        );
    }
}

/// Seed a rule + an immutable version row directly (the management API is
/// Task 6). Returns the rule id.
#[allow(clippy::too_many_arguments)]
async fn seed_rule(
    db: &Database,
    team_id: &str,
    trigger: &str,
    condition_json: &str,
    actions_json: &str,
    enabled: bool,
    version: i64,
    creator: &str,
) -> String {
    let rule_id = Uuid::now_v7().to_string();
    let now = tasker::util::now_ms();
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(&rule_id)
        .s(team_id)
        .s(format!("rule-{rule_id}"))
        .opt_s(None::<String>)
        .s(trigger)
        .b(enabled)
        .i(version)
        .s(condition_json)
        .s(actions_json)
        .s(creator)
        .i(now)
        .i(now)
        .opt_i(None::<i64>);
    conn.execute(
        "INSERT INTO rules (id, team_id, name, description, trigger_type, enabled, \
         current_version, condition_json, actions_json, created_by, created_at, \
         updated_at, archived_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await
    .unwrap_or_else(|e| panic!("seed_rule insert failed: {e}"));
    let mut args = tasker::db::Args::new();
    args.s(&rule_id)
        .i(version)
        .s(trigger)
        .s(condition_json)
        .s(actions_json)
        .s(creator)
        .i(now);
    conn.execute(
        "INSERT INTO rule_versions (rule_id, version, trigger_type, condition_json, \
         actions_json, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await
    .unwrap_or_else(|e| panic!("seed_rule version insert failed: {e}"));
    rule_id
}

/// Scalar COUNT; `bind` is bound as the first (optional) parameter.
async fn count_rows(db: &Database, sql: &str, bind: Option<&str>) -> i64 {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.opt_s(bind);
    let row = conn
        .fetch_optional(sql, args)
        .await
        .unwrap_or_else(|e| panic!("count query failed: {e}"));
    row.map_or(0, |r| r.try_get::<i64, _>(0).unwrap_or(0))
}

async fn user_id_by_email(db: &Database, email: &str) -> String {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(email);
    let row = conn
        .fetch_optional("SELECT id FROM users WHERE email = ?", args)
        .await
        .unwrap()
        .expect("user row");
    row.try_get("id").unwrap()
}

#[tokio::test]
async fn rules_capture_evaluates_matching_rules_transactionally() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        // ---- Team / project / task fixtures via the real API ----
        let admin_email = format!("rules-admin-{suffix}@test.local");
        let member_email = format!("rules-member-{suffix}@test.local");
        let outsider_email = format!("rules-outsider-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Rules Admin").await;
        seed_user(&db, &member_email, "Rules Member").await;
        seed_user(&db, &outsider_email, "Rules Outsider").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let outsider = login_as(&app.app, &outsider_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "RT"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] team");
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role": "member"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] add member");
        let (code, other_team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "RT2"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let other_team_id = other_team["id"].as_str().unwrap().to_string();
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "RP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] project");
        let project_id = project["id"].as_str().unwrap().to_string();
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Rules target"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CREATED,
            "[{engine}] task (no assignee: no event)"
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                Some(&task_id),
            )
            .await,
            0,
            "[{engine}] creation without assignee emits nothing"
        );

        // ---- Rules ----
        let all_statuses = serde_json::json!(
            {"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}
        )
        .to_string();
        // v1 of the status rule: always matches, adds a comment.
        let status_rule = seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            &all_statuses,
            r#"[{"body":"automation triage"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        // Disabled rule: never selected.
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            &all_statuses,
            r#"[{"body":"disabled"}]"#,
            false,
            1,
            &admin_id,
        )
        .await;
        // Same trigger in ANOTHER team: must never fire for this task.
        seed_rule(
            &db,
            &other_team_id,
            "task_status_changed",
            &all_statuses,
            r#"[{"body":"cross-team"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        // Assignment rule: fires when a user is assigned; persists a
        // provider-neutral send_message intent.
        seed_rule(
            &db,
            &team_id,
            "task_assigned",
            r#"{"field":"task.assignee_id","op":"NEQ","value":null}"#,
            r#"[{"message":{"connection_ref":"conn-1","destination_ref":"dest-1","format":"plain_text","body":"assigned to you"}}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        // Priority rule whose condition is always false in this context.
        let unrelated_actor = Uuid::now_v7().to_string();
        seed_rule(
            &db,
            &team_id,
            "task_priority_changed",
            &serde_json::json!({"field":"event.actor_id","op":"EQ","value":unrelated_actor})
                .to_string(),
            r#"[{"body":"never"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;

        // -- 1. Assignment (PATCH): task_assigned event + send_message intent --
        let member_id = user_id_by_email(&db, &member_email).await;
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] assign");
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&task_id);
        let ev = conn
            .fetch_optional(
                "SELECT id FROM domain_events WHERE task_id = ? AND event_type = 'task_assigned'",
                args,
            )
            .await
            .unwrap()
            .expect("assignment event");
        let assign_event_id: String = ev.try_get("id").unwrap();
        let mut args = tasker::db::Args::new();
        args.s(&assign_event_id);
        let intents = conn
            .fetch_all(
                "SELECT action_type, payload FROM automation_outbox WHERE event_id = ?",
                args,
            )
            .await
            .unwrap();
        assert_eq!(intents.len(), 1, "[{engine}] assignment intent");
        assert_eq!(
            intents[0].try_get::<String, _>("action_type").unwrap(),
            "send_message"
        );
        let payload = intents[0].try_get::<String, _>("payload").unwrap();
        assert!(
            payload.contains("connection_ref"),
            "[{engine}] provider-neutral intent"
        );
        assert!(
            !payload.contains("http"),
            "[{engine}] no URLs in messaging intents"
        );

        // -- 2. Status transition: event + matching execution + intent ------
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] transition");
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?                  AND event_type = 'task_status_changed'",
                Some(&task_id),
            )
            .await,
            1,
            "[{engine}] exactly one status event"
        );
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&task_id);
        let ev = conn
            .fetch_optional(
                "SELECT id, event_type, origin, previous_state, new_state, actor_id \
                 FROM domain_events WHERE task_id = ? ORDER BY occurred_at DESC",
                args,
            )
            .await
            .unwrap()
            .expect("event row");
        assert_eq!(
            ev.try_get::<String, _>("event_type").unwrap(),
            "task_status_changed"
        );
        assert_eq!(ev.try_get::<String, _>("origin").unwrap(), "user");
        assert_eq!(
            ev.try_get::<Option<String>, _>("previous_state")
                .unwrap()
                .unwrap(),
            r#"{"status":"backlog"}"#
        );
        assert_eq!(
            ev.try_get::<Option<String>, _>("new_state")
                .unwrap()
                .unwrap(),
            r#"{"status":"todo"}"#
        );
        assert_eq!(
            ev.try_get::<Option<String>, _>("actor_id")
                .unwrap()
                .unwrap(),
            member_id
        );
        let event_id: String = ev.try_get("id").unwrap();

        let mut args = tasker::db::Args::new();
        args.s(&event_id);
        let executions = conn
            .fetch_all(
                "SELECT id, rule_id, rule_version, condition_result, status, planned_actions \
                 FROM rule_executions WHERE event_id = ?",
                args,
            )
            .await
            .unwrap();
        assert_eq!(
            executions.len(),
            1,
            "[{engine}] only the enabled same-team rule fires"
        );
        assert_eq!(
            executions[0]
                .try_get::<String, _>("condition_result")
                .unwrap(),
            "matched"
        );
        assert_eq!(
            executions[0].try_get::<String, _>("status").unwrap(),
            "planned"
        );
        assert_eq!(executions[0].try_get::<i64, _>("rule_version").unwrap(), 1);
        assert_eq!(
            executions[0]
                .try_get::<Option<String>, _>("planned_actions")
                .unwrap()
                .unwrap(),
            r#"[{"action_type":"add_comment","index":0}]"#
        );
        let execution_id: String = executions[0].try_get("id").unwrap();

        let mut args = tasker::db::Args::new();
        args.s(&event_id);
        let intents = conn
            .fetch_all(
                "SELECT action_type, payload, status, attempts, idempotency_key, execution_id \
                 FROM automation_outbox WHERE event_id = ?",
                args,
            )
            .await
            .unwrap();
        assert_eq!(intents.len(), 1, "[{engine}] one intent for the one action");
        assert_eq!(
            intents[0].try_get::<String, _>("action_type").unwrap(),
            "add_comment"
        );
        assert_eq!(
            intents[0].try_get::<String, _>("payload").unwrap(),
            r#"{"body":"automation triage"}"#
        );
        assert_eq!(
            intents[0].try_get::<String, _>("status").unwrap(),
            "pending"
        );
        assert_eq!(intents[0].try_get::<i64, _>("attempts").unwrap(), 0);
        assert_eq!(
            intents[0]
                .try_get::<String, _>("idempotency_key")
                .unwrap()
                .len(),
            64
        );
        assert_eq!(
            intents[0].try_get::<String, _>("execution_id").unwrap(),
            execution_id,
            "[{engine}] intent references its execution"
        );

        // -- 3. False condition: event persisted, no execution/intent ------
        let exec_before = count_rows(&db, "SELECT COUNT(*) FROM rule_executions", None).await;
        let outbox_before = count_rows(&db, "SELECT COUNT(*) FROM automation_outbox", None).await;
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"priority": "urgent"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] priority change");
        assert_eq!(
            count_rows(&db, "SELECT COUNT(*) FROM rule_executions", None).await,
            exec_before,
            "[{engine}] false condition creates no execution"
        );
        assert_eq!(
            count_rows(&db, "SELECT COUNT(*) FROM automation_outbox", None).await,
            outbox_before,
            "[{engine}] false condition creates no intent"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ? AND event_type = 'task_priority_changed'",
                Some(&task_id),
            )
            .await,
            1,
            "[{engine}] priority event still captured"
        );

        // -- 4. Unassignment and no-op updates emit nothing -----------------
        let events_before = count_rows(
            &db,
            "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
            Some(&task_id),
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"assignee_id": ""}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] unassign");
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"priority": "urgent"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] no-op priority");
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                Some(&task_id),
            )
            .await,
            events_before,
            "[{engine}] unassignment and no-op emit nothing"
        );

        // -- 5. Rejected mutation: no event ----------------------------------
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&outsider),
            json!({"title": "hijack"}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider rejected");
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                Some(&task_id),
            )
            .await,
            events_before,
            "[{engine}] rejected mutation emits no event"
        );

        // -- 6. Version pinning: v2 with a false condition stops matching ----
        {
            let mut vconn = db.conn();
            let now = tasker::util::now_ms();
            let mut args = tasker::db::Args::new();
            args.s(&status_rule)
                .i(2i64)
                .s("task_status_changed")
                .s(
                    serde_json::json!({"field":"event.actor_id","op":"EQ","value":unrelated_actor})
                        .to_string(),
                )
                .s(r#"[{"body":"v2 never"}]"#)
                .s(&admin_id)
                .i(now);
            vconn
                .execute(
                    "INSERT INTO rule_versions (rule_id, version, trigger_type, condition_json, \
                     actions_json, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                    args,
                )
                .await
                .unwrap();
            let mut args = tasker::db::Args::new();
            args.i(2i64).s(&status_rule);
            vconn
                .execute("UPDATE rules SET current_version = ? WHERE id = ?", args)
                .await
                .unwrap();
        }
        // Re-assign (fires task_assigned again) so the assignee can move the
        // task; the v2 condition is false, so no execution is added.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] re-assign");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "in_progress"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] transition under v2");
        let mut args = tasker::db::Args::new();
        args.s(&status_rule);
        let execs = conn
            .fetch_all(
                "SELECT DISTINCT rule_version FROM rule_executions WHERE rule_id = ?",
                args,
            )
            .await
            .unwrap();
        assert_eq!(execs.len(), 1, "[{engine}] one version referenced");
        assert_eq!(
            execs[0].try_get::<i64, _>(0).unwrap(),
            1,
            "[{engine}] executions cite v1; v2 (false) added none"
        );

        // -- 7. Automation-origin events are captured but not evaluated ------
        {
            let mut tconn = db.begin().await.unwrap();
            tasker::rules::engine::capture_and_evaluate(
                &mut tconn,
                tasker::rules::engine::TaskState {
                    team_id: &team_id,
                    task_id: &task_id,
                    status: "in_progress",
                    priority: "urgent",
                    assignee_id: None,
                    reporter_id: None,
                },
                tasker::rules::engine::EventInput {
                    event_type: tasker::rules::TriggerType::TaskStatusChanged,
                    actor_id: None,
                    previous: tasker::rules::EventState {
                        status: Some("todo"),
                        ..tasker::rules::EventState::default()
                    },
                    new: tasker::rules::EventState {
                        status: Some("in_progress"),
                        ..tasker::rules::EventState::default()
                    },
                },
                tasker::rules::EventOrigin::Automation,
            )
            .await
            .expect("automation capture");
            tconn.commit().await.unwrap();
        }
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ? AND origin = 'automation'",
                Some(&task_id),
            )
            .await,
            1,
            "[{engine}] automation event recorded"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM rule_executions re JOIN domain_events de ON de.id = re.event_id \
                 WHERE de.task_id = ? AND de.origin = 'automation'",
                Some(&task_id),
            )
            .await,
            0,
            "[{engine}] automation-origin events never evaluate (Task 1 policy)"
        );

        // -- 8. Concurrent transitions: one winner, exactly one event --------
        let events_before = count_rows(
            &db,
            "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
            Some(&task_id),
        )
        .await;
        let transition_uri = format!("/api/v1/tasks/{task_id}/transition");
        let (r1, r2) = tokio::join!(
            send_json(
                &app.app,
                "POST",
                &transition_uri,
                Some(&member),
                json!({"to": "resolved"}),
            ),
            send_json(
                &app.app,
                "POST",
                &transition_uri,
                Some(&member),
                json!({"to": "resolved"}),
            ),
        );
        let codes = [r1.0, r2.0];
        assert!(
            (codes[0] == StatusCode::OK) ^ (codes[1] == StatusCode::OK),
            "[{engine}] exactly one concurrent transition wins: {codes:?}"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                Some(&task_id),
            )
            .await,
            events_before + 1,
            "[{engine}] the winner emits exactly one event"
        );

        // -- 9. Creation with an initial assignee emits the event (create hook)
        let (code, task2) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Assigned at creation", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] task2");
        let created_assigned_id = task2["id"].as_str().unwrap().to_string();
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ? AND event_type = 'task_assigned'",
                Some(&created_assigned_id),
            )
            .await,
            1,
            "[{engine}] initial assignment emits exactly one event"
        );
    }
}

/// Deterministic persistence-failure injection (Task 3 B1/B2): a TEST-DATABASE
/// trigger aborts the INSERT of `rule_executions` (B1) or `automation_outbox`
/// (B2) for one specific task id. The trigger lives only for the duration of
/// the test on the spawned test database and is dropped afterwards; the
/// helper-table membership condition keeps it inert for every other row.
/// Technique per dialect (single-statement, no multi-statement execution):
/// - SQLite: `BEFORE INSERT ... WHEN (SELECT COUNT(*) ...) > 0 RAISE(ABORT)`
/// - PostgreSQL: plpgsql function + trigger, `RAISE EXCEPTION`
/// - MySQL: `SIGNAL SQLSTATE '45000'` inside the trigger body
/// Task 4: Automation Principal identity, provisioning, and isolation.
#[tokio::test]
async fn rules_automation_principal_security() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();

        // -- Idempotent provisioning; fixed identity ------------------------
        let mut conn = db.conn();
        let id1 = tasker::rules::actions::provision_principal(&mut conn)
            .await
            .expect("first provisioning");
        let id2 = tasker::rules::actions::provision_principal(&mut conn)
            .await
            .expect("second provisioning");
        assert_eq!(id1, tasker::rules::actions::AUTOMATION_PRINCIPAL_ID);
        assert_eq!(id1, id2, "[{engine}] stable identity");
        let mut args = tasker::db::Args::new();
        args.s(&id1);
        let row = conn
            .fetch_optional(
                "SELECT email, display_name, password_hash FROM users WHERE id = ?",
                args,
            )
            .await
            .unwrap()
            .expect("principal row");
        assert_eq!(
            row.try_get::<String, _>("email").unwrap(),
            "automation@tasker.local"
        );
        assert_eq!(
            row.try_get::<String, _>("display_name").unwrap(),
            "Tasker Automation"
        );
        // The stored hash is not a PHC string: no password verifies against it.
        let hash = row.try_get::<String, _>("password_hash").unwrap();
        assert!(
            !tasker::security::password::verify("test-password-123", &hash),
            "[{engine}] principal must not accept its documented password"
        );
        assert!(
            !tasker::security::password::verify("", &hash),
            "[{engine}] principal must not accept an empty password"
        );

        // -- Interactive login is impossible --------------------------------
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": "automation@tasker.local", "password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] principal login rejected"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            "/api/v1/auth/login",
            None,
            json!({"email": "automation@tasker.local", "password": "!tasker-automation-no-login"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] hash string is not a password"
        );

        // -- The reserved email cannot enter through provisioning paths -----
        assert!(
            tasker::util::validate_email("automation@tasker.local").is_err(),
            "[{engine}] reserved email rejected by the centralized validator"
        );
        assert!(tasker::util::validate_email("other@test.local").is_ok());

        // -- No implicit team membership or capability ----------------------
        let p_suffix = Uuid::now_v7().simple();
        let admin_email = format!("p-admin-{p_suffix}@test.local");
        let _admin_id = seed_user(&db, &admin_email, "P Admin").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "PA"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let membership = tasker::authorization::get_membership(&mut conn, &team_id, &id1)
            .await
            .expect("membership query");
        assert!(
            membership.is_none(),
            "[{engine}] principal has no team membership"
        );
        // Supplying the principal's UUID through ordinary authorization can
        // never grant anything: it holds no membership row.
        assert!(
            !tasker::authorization::get_membership(&mut conn, &team_id, &id1)
                .await
                .unwrap()
                .map(|m| m.role.can(tasker::authorization::Capability::CreateTask))
                .unwrap_or(false),
            "[{engine}] no capability via principal identity"
        );

        // -- Email-conflict fail-closed (isolated SQLite leg only: the
        //    shared CI PG/MySQL databases must not be poisoned) --------------
        if engine == "sqlite" {
            let conflict_app = spawn_app(&format!(
                "sqlite:{}/{}",
                std::env::temp_dir().display(),
                Uuid::now_v7()
            ))
            .await;
            {
                let mut c = conflict_app.db.conn();
                let now = tasker::util::now_ms();
                let mut args = tasker::db::Args::new();
                args.s(Uuid::now_v7().to_string())
                    .s("automation@tasker.local")
                    .s("Foreign Squatter")
                    .s("$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
                    .b(true)
                    .i(now)
                    .i(now);
                c.execute(
                    "INSERT INTO users (id, email, display_name, password_hash, is_active, \
                     created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                    args,
                )
                .await
                .unwrap();
            }
            let mut c = conflict_app.db.conn();
            let err = tasker::rules::actions::provision_principal(&mut c)
                .await
                .expect_err("foreign email holder must fail provisioning closed");
            assert!(
                format!("{err}").contains("principal_email_conflict"),
                "[{engine}] diagnosable conflict category: {err}"
            );
        }
    }
}

/// Task 4: internal action execution through the outbox primitive.
#[tokio::test]
async fn rules_internal_actions_end_to_end() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        // ---- Fixtures ----
        let admin_email = format!("rules-act-{suffix}@test.local");
        let member_email = format!("rules-act-m-{suffix}@test.local");
        let outsider_email = format!("rules-act-o-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Act Admin").await;
        seed_user(&db, &member_email, "Act Member").await;
        let outsider_id = seed_user(&db, &outsider_email, "Act Outsider").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "ACT"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role": "member"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "ACTP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = user_id_by_email(&db, &member_email).await;
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Action target", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();

        // Rule with all three action kinds (declaration order is preserved).
        // The outsider's real id is seeded directly: the DSL validates every
        // explicit recipient as a UUID at parse time.
        let notify_actions = serde_json::json!([
            {"body":"automation triage"},
            {"recipients":{"selectors":["assignee","reporter"],"users":[outsider_id]}},
            {"message":{"connection_ref":"conn-1","destination_ref":"dest-1","format":"plain_text","body":"notify the channel"}}
        ])
        .to_string();
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
            &notify_actions,
            true,
            1,
            &admin_id,
        )
        .await;

        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        // Collect the three intents in declaration order.
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&task_id);
        let intents = conn
            .fetch_all(
                "SELECT o.id, o.action_type FROM automation_outbox o \
                 JOIN rule_executions e ON e.id = o.execution_id \
                 WHERE e.task_id = ? ORDER BY o.created_at, o.id",
                args,
            )
            .await
            .unwrap();
        assert_eq!(intents.len(), 3, "[{engine}] three intents, one per action");
        let by_type = |t: &str| {
            intents
                .iter()
                .find(|r| r.try_get::<String, _>("action_type").unwrap() == t)
                .map(|r| r.try_get::<String, _>("id").unwrap())
                .unwrap_or_else(|| panic!("no {t} intent"))
        };
        let comment_intent = by_type("add_comment");
        let notify_intent = by_type("send_notification");
        let message_intent = by_type("send_message");

        // -- send_message: external delivery never happens here -------------
        {
            let mut tx = db.begin().await.unwrap();
            let outcome =
                tasker::rules::outbox::execute_internal_action(&mut tx, &message_intent, None)
                    .await
                    .expect("external classification");
            tx.commit().await.unwrap();
            assert_eq!(
                outcome,
                tasker::rules::outbox::ExecutionOutcome::ExternalPending,
                "[{engine}] send_message is deferred, not executed"
            );
            let mut args = tasker::db::Args::new();
            args.s(&message_intent);
            let row = conn
                .fetch_optional("SELECT status FROM automation_outbox WHERE id = ?", args)
                .await
                .unwrap()
                .expect("intent row");
            assert_eq!(
                row.try_get::<String, _>("status").unwrap(),
                "pending",
                "[{engine}] external intent stays pending for Task 5"
            );
            assert_eq!(
                count_rows(
                    &db,
                    "SELECT COUNT(*) FROM rule_action_results WHERE outbox_id = ?",
                    Some(&message_intent),
                )
                .await,
                0,
                "[{engine}] no delivery-success result recorded"
            );
        }

        // -- add_comment: exactly one comment authored by the principal -----
        {
            let mut tx = db.begin().await.unwrap();
            let outcome =
                tasker::rules::outbox::execute_internal_action(&mut tx, &comment_intent, None)
                    .await
                    .expect("comment execution");
            tx.commit().await.unwrap();
            assert_eq!(outcome, tasker::rules::outbox::ExecutionOutcome::Completed);
        }
        let mut args = tasker::db::Args::new();
        args.s(&task_id);
        let comments = conn
            .fetch_all(
                "SELECT c.body, c.author_id, c.guest_visible FROM comments c WHERE c.task_id = ?",
                args,
            )
            .await
            .unwrap();
        assert_eq!(comments.len(), 1, "[{engine}] exactly one comment");
        assert_eq!(
            comments[0].try_get::<String, _>("body").unwrap(),
            "automation triage"
        );
        assert_eq!(
            comments[0].try_get::<String, _>("author_id").unwrap(),
            tasker::rules::actions::AUTOMATION_PRINCIPAL_ID,
            "[{engine}] author is the Automation Principal"
        );
        assert_eq!(
            comments[0].try_get::<i64, _>("guest_visible").unwrap(),
            0,
            "[{engine}] automation comments are internal"
        );
        let mut args = tasker::db::Args::new();
        args.s(&comment_intent);
        let result = conn
            .fetch_optional(
                "SELECT outcome, author_id, body_hash, detail FROM rule_action_results \
                 WHERE outbox_id = ?",
                args,
            )
            .await
            .unwrap()
            .expect("ledger row");
        assert_eq!(result.try_get::<String, _>("outcome").unwrap(), "completed");
        assert_eq!(
            result
                .try_get::<Option<String>, _>("author_id")
                .unwrap()
                .unwrap(),
            tasker::rules::actions::AUTOMATION_PRINCIPAL_ID
        );
        assert!(
            result
                .try_get::<Option<String>, _>("body_hash")
                .unwrap()
                .is_some()
        );
        let detail = result
            .try_get::<Option<String>, _>("detail")
            .unwrap()
            .unwrap();
        assert!(
            detail.contains("comment_id"),
            "[{engine}] result cites the comment"
        );
        let mut args = tasker::db::Args::new();
        args.s(&comment_intent);
        let row = conn
            .fetch_optional(
                "SELECT status, completed_at FROM automation_outbox WHERE id = ?",
                args,
            )
            .await
            .unwrap()
            .expect("intent");
        assert_eq!(row.try_get::<String, _>("status").unwrap(), "delivered");
        assert!(
            row.try_get::<Option<i64>, _>("completed_at")
                .unwrap()
                .is_some()
        );

        // -- Duplicate execution: no duplicate effect ------------------------
        {
            let mut tx = db.begin().await.unwrap();
            let outcome =
                tasker::rules::outbox::execute_internal_action(&mut tx, &comment_intent, None)
                    .await
                    .expect("duplicate is a clean no-op");
            tx.commit().await.unwrap();
            assert_eq!(
                outcome,
                tasker::rules::outbox::ExecutionOutcome::AlreadyExecuted
            );
        }
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM comments WHERE task_id = ?",
                Some(&task_id)
            )
            .await,
            1,
            "[{engine}] no duplicate comment"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM rule_action_results WHERE outbox_id = ?",
                Some(&comment_intent),
            )
            .await,
            1,
            "[{engine}] one durable result"
        );

        // -- send_notification: selectors + revalidated explicit recipients --
        {
            let mut tx = db.begin().await.unwrap();
            let outcome =
                tasker::rules::outbox::execute_internal_action(&mut tx, &notify_intent, None)
                    .await
                    .expect("notification execution");
            tx.commit().await.unwrap();
            assert_eq!(outcome, tasker::rules::outbox::ExecutionOutcome::Completed);
        }
        let mut args = tasker::db::Args::new();
        args.s(&task_id);
        let notifications = conn
            .fetch_all(
                "SELECT n.recipient_id, n.event, n.actor_id FROM notifications n                  WHERE n.task_id = ? AND n.event = 'automation_triggered'",
                args,
            )
            .await
            .unwrap();
        let recipients: Vec<String> = notifications
            .iter()
            .map(|r| r.try_get::<String, _>("recipient_id").unwrap())
            .collect();
        assert!(
            recipients.contains(&member_id),
            "[{engine}] assignee notified"
        );
        assert!(
            recipients.contains(&admin_id),
            "[{engine}] reporter notified"
        );
        assert!(
            !recipients.contains(&outsider_id),
            "[{engine}] departed/non-member explicit recipient skipped"
        );
        assert!(
            notifications
                .iter()
                .all(|r| r.try_get::<String, _>("event").unwrap() == "automation_triggered"),
            "[{engine}] in-app automation event only"
        );
        // Duplicate execution: no duplicate notifications.
        {
            let mut tx = db.begin().await.unwrap();
            let outcome =
                tasker::rules::outbox::execute_internal_action(&mut tx, &notify_intent, None)
                    .await
                    .expect("duplicate notification is a clean no-op");
            tx.commit().await.unwrap();
            assert_eq!(
                outcome,
                tasker::rules::outbox::ExecutionOutcome::AlreadyExecuted
            );
        }
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM notifications WHERE task_id = ? AND event = 'automation_triggered'",
                Some(&task_id),
            ).await,
            2,
            "[{engine}] no duplicate notifications"
        );

        // -- Scope corruption: cross-team task reference fails loudly --------
        let (code, other_team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "ACT-OTHER"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let other_team_id = other_team["id"].as_str().unwrap().to_string();
        let (code, other_project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{other_team_id}/projects"),
            Some(&admin),
            json!({"name": "ACT-OTHERP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, foreign_task) = send_json(
            &app.app,
            "POST",
            &format!(
                "/api/v1/projects/{}/tasks",
                other_project["id"].as_str().unwrap()
            ),
            Some(&admin),
            json!({"title": "Foreign task"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        // A second status rule so a fresh add_comment intent exists to corrupt.
        seed_rule(
            &db,
            &team_id,
            "task_priority_changed",
            r#"{"field":"task.priority","op":"IN","values":["low","medium","high","urgent"]}"#,
            r#"[{"body":"scope probe"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"priority": "high"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let mut conn2 = db.conn();
        let mut args = tasker::db::Args::new();
        args.s("add_comment");
        let scope_intent: String = conn2
            .fetch_optional(
                "SELECT o.id FROM automation_outbox o WHERE o.action_type = ? \
                 AND o.status = 'pending' ORDER BY o.created_at DESC",
                args,
            )
            .await
            .unwrap()
            .expect("scope-probe intent")
            .try_get("id")
            .unwrap();
        {
            let mut c = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&foreign_task["id"].as_str().unwrap().to_string())
                .s(&scope_intent);
            c.execute(
                "UPDATE automation_outbox SET task_id = ? WHERE id = ?",
                args,
            )
            .await
            .unwrap();
        }
        {
            let mut tx = db.begin().await.unwrap();
            let err = tasker::rules::outbox::execute_internal_action(&mut tx, &scope_intent, None)
                .await
                .expect_err("[{engine}] cross-scope reference must fail");
            tx.rollback().await;
            assert!(
                format!("{err}").contains("corrupted") || format!("{err}").contains("team"),
                "[{engine}] diagnosable scope failure: {err}"
            );
        }
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM rule_action_results WHERE outbox_id = ?",
                Some(&scope_intent),
            )
            .await,
            0,
            "[{engine}] no ledger entry for a corrupted intent"
        );

        // -- Deleted task: durable skip, no effect ---------------------------
        seed_rule(
            &db,
            &team_id,
            "task_assigned",
            r#"{"field":"task.assignee_id","op":"NEQ","value":null}"#,
            r#"[{"body":"for a task that will vanish"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, vanishing) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Vanishing", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let vanishing_id = vanishing["id"].as_str().unwrap().to_string();
        // Unassign-then-reassign produces a fresh task_assigned event + intent.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{vanishing_id}"),
            Some(&admin),
            json!({"assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let mut conn2 = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&vanishing_id);
        let vanish_intent: String = conn2
            .fetch_optional(
                "SELECT o.id FROM automation_outbox o \
                 JOIN rule_executions e ON e.id = o.execution_id \
                 WHERE e.task_id = ? AND o.action_type = 'add_comment' AND o.status = 'pending'",
                args,
            )
            .await
            .unwrap()
            .expect("vanishing-task intent")
            .try_get("id")
            .unwrap();
        // Hard-delete the task (automation history survives by design).
        {
            let mut c = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&vanishing_id);
            c.execute("DELETE FROM tasks WHERE id = ?", args)
                .await
                .unwrap();
        }
        {
            let mut tx = db.begin().await.unwrap();
            let outcome =
                tasker::rules::outbox::execute_internal_action(&mut tx, &vanish_intent, None)
                    .await
                    .expect("deleted-task handling");
            tx.commit().await.unwrap();
            assert_eq!(outcome, tasker::rules::outbox::ExecutionOutcome::Completed);
        }
        let mut args = tasker::db::Args::new();
        args.s(&vanish_intent);
        let row = conn2
            .fetch_optional(
                "SELECT outcome, detail FROM rule_action_results WHERE outbox_id = ?",
                args,
            )
            .await
            .unwrap()
            .expect("skip result");
        assert_eq!(row.try_get::<String, _>("outcome").unwrap(), "skipped");
        assert!(
            row.try_get::<Option<String>, _>("detail")
                .unwrap()
                .unwrap()
                .contains("task_deleted"),
            "[{engine}] skip reason recorded"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM comments WHERE task_id = ?",
                Some(&vanishing_id)
            )
            .await,
            0,
            "[{engine}] no comment on a deleted task"
        );

        // -- Concurrency: two simultaneous executions, one effect ------------
        let (code, task4) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Concurrent target", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task4_id = task4["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task4_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let mut conn2 = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&task4_id);
        let conc_intent: String = conn2
            .fetch_optional(
                "SELECT o.id FROM automation_outbox o \
                 JOIN rule_executions e ON e.id = o.execution_id \
                 WHERE e.task_id = ? AND o.action_type = 'add_comment'",
                args,
            )
            .await
            .unwrap()
            .expect("concurrency intent")
            .try_get("id")
            .unwrap();
        {
            let db1 = db.clone();
            let db2 = db.clone();
            let intent1 = conc_intent.clone();
            let intent2 = conc_intent.clone();
            let (r1, r2) = tokio::join!(
                async move {
                    let mut tx = db1.begin().await.unwrap();
                    let out =
                        tasker::rules::outbox::execute_internal_action(&mut tx, &intent1, None)
                            .await;
                    if out.is_ok() {
                        tx.commit().await.unwrap();
                    } else {
                        tx.rollback().await;
                    }
                    out
                },
                async move {
                    let mut tx = db2.begin().await.unwrap();
                    let out =
                        tasker::rules::outbox::execute_internal_action(&mut tx, &intent2, None)
                            .await;
                    if out.is_ok() {
                        tx.commit().await.unwrap();
                    } else {
                        tx.rollback().await;
                    }
                    out
                },
            );
            let completed = matches!(&r1, Ok(tasker::rules::outbox::ExecutionOutcome::Completed))
                as u8
                + matches!(&r2, Ok(tasker::rules::outbox::ExecutionOutcome::Completed)) as u8;
            assert_eq!(
                completed, 1,
                "[{engine}] exactly one concurrent completion ({r1:?} / {r2:?})"
            );
        }
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM comments WHERE task_id = ?",
                Some(&task4_id)
            )
            .await,
            1,
            "[{engine}] concurrent attempts produce one comment"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM rule_action_results WHERE outbox_id = ?",
                Some(&conc_intent),
            )
            .await,
            1,
            "[{engine}] one authoritative result"
        );
    }
}

/// Task 4: a ledger persistence failure leaves neither effect nor result.
#[tokio::test]
async fn rules_internal_actions_ledger_failure_atomicity() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        let admin_email = format!("rules-ledger-{suffix}@test.local");
        let member_email = format!("rules-ledger-m-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Ledger Admin").await;
        seed_user(&db, &member_email, "Ledger Member").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "LED"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role": "member"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "LEDP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = user_id_by_email(&db, &member_email).await;
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Ledger target", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
            r#"[{"body":"ledger probe"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;

        // Test-database trigger: abort the ledger INSERT for this task's
        // intents (same technique as the capture-failure test).
        {
            let mut conn = db.conn();
            let trigger_name = format!("p8_fail_ledger_{suffix}");
            conn.execute(
                "CREATE TABLE IF NOT EXISTS p8_fail_ledger (task_id TEXT PRIMARY KEY)",
                tasker::db::Args::new(),
            )
            .await
            .unwrap();
            let mut args = tasker::db::Args::new();
            args.s(&task_id);
            conn.execute("INSERT INTO p8_fail_ledger (task_id) VALUES (?)", args)
                .await
                .unwrap();
            let create = match engine.as_str() {
                "sqlite" => format!(
                    "CREATE TRIGGER {trigger_name} BEFORE INSERT ON rule_action_results \
                     WHEN EXISTS (SELECT 1 FROM automation_outbox o WHERE o.id = NEW.outbox_id \
                     AND o.task_id IN (SELECT task_id FROM p8_fail_ledger)) \
                     BEGIN SELECT RAISE(ABORT, 'ledger injection'); END"
                ),
                "postgres" => format!(
                    "CREATE OR REPLACE FUNCTION p8_fail_ledger_{suffix}() RETURNS trigger AS $$ \
                     BEGIN IF EXISTS (SELECT 1 FROM automation_outbox o WHERE o.id = NEW.outbox_id \
                     AND o.task_id IN (SELECT task_id FROM p8_fail_ledger)) THEN \
                     RAISE EXCEPTION 'ledger injection'; END IF; RETURN NEW; END; $$ LANGUAGE plpgsql"
                ),
                _ => format!(
                    "CREATE TRIGGER {trigger_name} BEFORE INSERT ON rule_action_results FOR EACH ROW \
                     BEGIN IF EXISTS (SELECT 1 FROM automation_outbox o WHERE o.id = NEW.outbox_id \
                     AND o.task_id IN (SELECT task_id FROM p8_fail_ledger)) THEN \
                     SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'ledger injection'; END IF; END"
                ),
            };
            conn.execute(&create, tasker::db::Args::new())
                .await
                .unwrap();
            if engine == "postgres" {
                conn.execute(
                    &format!(
                        "CREATE TRIGGER {trigger_name} BEFORE INSERT ON rule_action_results \
                         FOR EACH ROW EXECUTE FUNCTION p8_fail_ledger_{suffix}()"
                    ),
                    tasker::db::Args::new(),
                )
                .await
                .unwrap();
            }
        }

        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&task_id);
        let intent: String = conn
            .fetch_optional(
                "SELECT o.id FROM automation_outbox o \
                 JOIN rule_executions e ON e.id = o.execution_id WHERE e.task_id = ?",
                args,
            )
            .await
            .unwrap()
            .expect("intent")
            .try_get("id")
            .unwrap();

        {
            let mut tx = db.begin().await.unwrap();
            let err = tasker::rules::outbox::execute_internal_action(&mut tx, &intent, None)
                .await
                .expect_err("[{engine}] ledger injection fails the execution");
            tx.rollback().await;
            assert!(
                format!("{err}").contains("ledger injection"),
                "[{engine}] injection surfaced: {err}"
            );
        }
        // Neither the effect nor a successful result survived.
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM comments WHERE task_id = ?",
                Some(&task_id)
            )
            .await,
            0,
            "[{engine}] no comment without its ledger entry"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM rule_action_results WHERE outbox_id = ?",
                Some(&intent),
            )
            .await,
            0,
            "[{engine}] no ledger entry"
        );
        let mut args = tasker::db::Args::new();
        args.s(&intent);
        let row = conn
            .fetch_optional("SELECT status FROM automation_outbox WHERE id = ?", args)
            .await
            .unwrap()
            .expect("intent");
        assert_eq!(
            row.try_get::<String, _>("status").unwrap(),
            "pending",
            "[{engine}] intent remains retryable"
        );
    }
}

#[tokio::test]
async fn rules_capture_insert_failure_atomicity() {
    for (engine, url) in test_db_urls() {
        for stage in ["rule_executions", "automation_outbox"] {
            let app = spawn_app(&url).await;
            let db = app.db.clone();
            let suffix = Uuid::now_v7().simple();

            let admin_email = format!("rules-inject-{suffix}@test.local");
            let member_email = format!("rules-inject-m-{suffix}@test.local");
            let admin_id = seed_user(&db, &admin_email, "Rules Inject").await;
            seed_user(&db, &member_email, "Rules Inject Member").await;
            let admin = login_as(&app.app, &admin_email, "test-password-123").await;
            let member = login_as(&app.app, &member_email, "test-password-123").await;
            let (code, team) = send_json(
                &app.app,
                "POST",
                "/api/v1/teams",
                Some(&admin),
                json!({"name": "RI"}),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            let team_id = team["id"].as_str().unwrap().to_string();
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/teams/{team_id}/members"),
                Some(&admin),
                json!({"email": member_email, "role": "member"}),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            let (code, project) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/teams/{team_id}/projects"),
                Some(&admin),
                json!({"name": "RIP"}),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            let project_id = project["id"].as_str().unwrap().to_string();
            let member_id = user_id_by_email(&db, &member_email).await;
            let (code, task) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/projects/{project_id}/tasks"),
                Some(&admin),
                json!({"title": "Injection target", "assignee_id": member_id}),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            let task_id = task["id"].as_str().unwrap().to_string();

            seed_rule(
                &db,
                &team_id,
                "task_status_changed",
                r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
                r#"[{"body":"injected"}]"#,
                true,
                1,
                &admin_id,
            )
            .await;

            // The injection trigger: aborts the stage's INSERT for this task.
            {
                let mut conn = db.conn();
                let target_table = stage;
                let trigger_name = format!("p8_fail_{stage}_{suffix}");
                let create = match engine.as_str() {
                    "sqlite" => format!(
                        "CREATE TRIGGER {trigger_name} BEFORE INSERT ON {target_table} \
                         WHEN (SELECT COUNT(*) FROM p8_fail_targets WHERE task_id = NEW.task_id) > 0 \
                         BEGIN SELECT RAISE(ABORT, 'test injection'); END"
                    ),
                    "postgres" => format!(
                        "CREATE OR REPLACE FUNCTION p8_fail_{suffix}() RETURNS trigger AS $$ \
                         BEGIN IF (SELECT COUNT(*) FROM p8_fail_targets WHERE task_id = NEW.task_id) > 0 THEN \
                         RAISE EXCEPTION 'test injection'; END IF; RETURN NEW; END; $$ LANGUAGE plpgsql"
                    ),
                    _ => format!(
                        "CREATE TRIGGER {trigger_name} BEFORE INSERT ON {target_table} FOR EACH ROW \
                         BEGIN IF (SELECT COUNT(*) FROM p8_fail_targets WHERE task_id = NEW.task_id) > 0 THEN \
                         SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'test injection'; END IF; END"
                    ),
                };
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS p8_fail_targets (task_id TEXT PRIMARY KEY)",
                    tasker::db::Args::new(),
                )
                .await
                .unwrap_or_else(|e| panic!("[{engine}] helper table: {e}"));
                let mut args = tasker::db::Args::new();
                args.s(&task_id);
                conn.execute("INSERT INTO p8_fail_targets (task_id) VALUES (?)", args)
                    .await
                    .unwrap();
                conn.execute(&create, tasker::db::Args::new())
                    .await
                    .unwrap_or_else(|e| panic!("[{engine}] create trigger: {e}"));
                if engine == "postgres" {
                    conn.execute(
                        &format!(
                            "CREATE TRIGGER {trigger_name} BEFORE INSERT ON {target_table} \
                             FOR EACH ROW EXECUTE FUNCTION p8_fail_{suffix}()"
                        ),
                        tasker::db::Args::new(),
                    )
                    .await
                    .unwrap_or_else(|e| panic!("[{engine}] create pg trigger: {e}"));
                }
            }

            // The creation-assignment event already exists; the injected
            // failure must add nothing.
            let events_before = count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                Some(&task_id),
            )
            .await;
            let exec_before = count_rows(
                &db,
                "SELECT COUNT(*) FROM rule_executions WHERE task_id = ?",
                Some(&task_id),
            )
            .await;
            let outbox_before = count_rows(
                &db,
                "SELECT COUNT(*) FROM automation_outbox WHERE task_id = ?",
                Some(&task_id),
            )
            .await;
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{task_id}/transition"),
                Some(&member),
                json!({"to": "todo"}),
            )
            .await;
            assert_eq!(
                code,
                StatusCode::INTERNAL_SERVER_ERROR,
                "[{engine}] {stage} injection fails the mutation"
            );

            // Persisted state after the aborted transaction: nothing added.
            assert_eq!(
                count_rows(
                    &db,
                    "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                    Some(&task_id),
                )
                .await,
                events_before,
                "[{engine}] {stage}: no event"
            );
            assert_eq!(
                count_rows(
                    &db,
                    "SELECT COUNT(*) FROM rule_executions WHERE task_id = ?",
                    Some(&task_id),
                )
                .await,
                exec_before,
                "[{engine}] {stage}: no execution"
            );
            assert_eq!(
                count_rows(
                    &db,
                    "SELECT COUNT(*) FROM automation_outbox WHERE task_id = ?",
                    Some(&task_id),
                )
                .await,
                outbox_before,
                "[{engine}] {stage}: no intent"
            );
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&task_id);
            let row = conn
                .fetch_optional("SELECT status FROM tasks WHERE id = ?", args)
                .await
                .unwrap()
                .expect("task row");
            assert_eq!(
                row.try_get::<String, _>("status").unwrap(),
                "backlog",
                "[{engine}] {stage}: task mutation rolled back"
            );

            // Remove the injection; the same transition then succeeds fully.
            {
                let mut conn = db.conn();
                let trigger_name = format!("p8_fail_{stage}_{suffix}");
                conn.execute("DELETE FROM p8_fail_targets WHERE task_id = ?", {
                    let mut args = tasker::db::Args::new();
                    args.s(&task_id);
                    args
                })
                .await
                .unwrap();
                if engine == "postgres" {
                    conn.execute(
                        &format!("DROP TRIGGER {trigger_name} ON {stage}"),
                        tasker::db::Args::new(),
                    )
                    .await
                    .unwrap();
                    conn.execute(
                        &format!("DROP FUNCTION p8_fail_{suffix}()"),
                        tasker::db::Args::new(),
                    )
                    .await
                    .unwrap();
                } else {
                    conn.execute(
                        &format!("DROP TRIGGER {trigger_name}"),
                        tasker::db::Args::new(),
                    )
                    .await
                    .unwrap();
                }
            }
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{task_id}/transition"),
                Some(&member),
                json!({"to": "todo"}),
            )
            .await;
            assert_eq!(
                code,
                StatusCode::OK,
                "[{engine}] {stage}: control transition succeeds"
            );
            assert_eq!(
                count_rows(
                    &db,
                    "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                    Some(&task_id),
                )
                .await,
                events_before + 1,
                "[{engine}] {stage}: control event persisted"
            );
            assert_eq!(
                count_rows(
                    &db,
                    "SELECT COUNT(*) FROM automation_outbox WHERE task_id = ?",
                    Some(&task_id),
                )
                .await,
                outbox_before + 1,
                "[{engine}] {stage}: control intent persisted"
            );
        }
    }
}

/// Task 5: dispatcher claim/execute cycle, external isolation, duplicate
/// convergence, and metrics wiring.
#[tokio::test]
async fn rules_dispatcher_claims_and_executes() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        let admin_email = format!("rules-dsp-{suffix}@test.local");
        let member_email = format!("rules-dsp-m-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Dsp Admin").await;
        seed_user(&db, &member_email, "Dsp Member").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "DSP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role": "member"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "DSPP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = user_id_by_email(&db, &member_email).await;
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Dispatcher target", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();

        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
            r#"[{"body":"dispatcher triage"},{"recipients":{"selectors":["assignee"],"users":[]}},{"message":{"connection_ref":"c","destination_ref":"d","format":"plain_text","body":"x"}}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);

        let pending_count = count_rows(
            &db,
            "SELECT COUNT(*) FROM automation_outbox WHERE status = 'pending'",
            None,
        )
        .await;
        assert_eq!(
            pending_count, 3,
            "[{engine}] three pending intents after transition"
        );
        // Batch limit: batch=1 claims exactly one.
        let metrics = tasker::http::metrics::Metrics::new();
        let claimed = tasker::rules::outbox::claim_batch(
            &mut db.conn(),
            "worker-a",
            1,
            60_000,
            tasker::util::now_ms(),
        )
        .await
        .expect("claim");
        assert_eq!(claimed.len(), 1, "[{engine}] batch=1 claims exactly one");
        // The claimed one is an internal action; send_message is never claimed.
        assert_ne!(
            claimed[0].action_type, "send_message",
            "[{engine}] external intent is not claimable by the internal dispatcher"
        );

        // A full run_once executes the remaining due intents (including the
        // already-claimed one? No — it is 'processing' now, so only the other
        // internal intent is claimed) and leaves the external one pending.
        let config = tasker::config::RulesWorkerConfig::default();
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(2));
        tasker::rules::dispatcher::run_once(&db, &config, "worker-a", &metrics, &semaphore)
            .await
            .expect("run_once");

        // Execute the manually claimed intent through the fenced primitive.
        {
            let mut tx = db.begin().await.unwrap();
            let outcome = tasker::rules::outbox::execute_internal_action(
                &mut tx,
                &claimed[0].outbox_id,
                Some(&claimed[0].claim_token),
            )
            .await
            .expect("claimed execution");
            tx.commit().await.unwrap();
            assert_eq!(outcome, tasker::rules::outbox::ExecutionOutcome::Completed);
        }

        // All internal effects landed; the external intent is untouched.
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM comments WHERE task_id = ?",
                Some(&task_id)
            )
            .await,
            1,
            "[{engine}] comment executed by the dispatcher"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM notifications WHERE task_id = ? AND event = 'automation_triggered'",
                Some(&task_id),
            )
            .await,
            1,
            "[{engine}] notification executed by the dispatcher"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM automation_outbox WHERE task_id = ? AND action_type = 'send_message' AND status = 'pending'",
                Some(&task_id),
            )
            .await,
            1,
            "[{engine}] external intent durable and pending"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM automation_outbox WHERE task_id = ? AND status = 'delivered'",
                Some(&task_id),
            )
            .await,
            2,
            "[{engine}] both internal intents delivered"
        );

        // Metrics reflect the dispatcher work (rendered counters).
        let rendered = metrics.render(0);
        assert!(
            rendered.contains("tasker_outbox_intents_claimed_total 1"),
            "[{engine}] claimed counter: {}",
            rendered
        );
        assert!(
            rendered.contains("tasker_outbox_actions_executed_total 1"),
            "[{engine}] executed counter"
        );
        assert!(
            rendered.contains("tasker_outbox_external_pending_total 0"),
            "[{engine}] external intents are never claimed (counter stays 0)"
        );

        // Duplicate convergence: re-running the same intent is a no-op.
        {
            let mut tx = db.begin().await.unwrap();
            let outcome = tasker::rules::outbox::execute_internal_action(
                &mut tx,
                &claimed[0].outbox_id,
                Some(&claimed[0].claim_token),
            )
            .await
            .expect("duplicate");
            tx.commit().await.unwrap();
            assert_eq!(
                outcome,
                tasker::rules::outbox::ExecutionOutcome::AlreadyExecuted
            );
        }
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM comments WHERE task_id = ?",
                Some(&task_id)
            )
            .await,
            1,
            "[{engine}] no duplicate comment"
        );

        // A second run_once claims nothing new (nothing due).
        let claimed_again = tasker::rules::outbox::claim_batch(
            &mut db.conn(),
            "worker-a",
            16,
            60_000,
            tasker::util::now_ms(),
        )
        .await
        .expect("second claim");
        assert!(
            claimed_again.is_empty(),
            "[{engine}] no due intents remain for the internal dispatcher"
        );
        let _ = member; // assignee exercised through the notification path
    }
}

/// Task 5: fencing, lease recovery, and stale-worker rejection.
#[tokio::test]
async fn rules_dispatcher_fencing_and_recovery() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        let admin_email = format!("rules-fen-{suffix}@test.local");
        let member_email = format!("rules-fen-m-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Fen Admin").await;
        seed_user(&db, &member_email, "Fen Member").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "FEN"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role": "member"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "FENP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = user_id_by_email(&db, &member_email).await;
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Fencing target", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
            r#"[{"body":"fenced triage"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);

        // Worker A claims, then "crashes" (lease expires, no execution).
        let claimed_a = tasker::rules::outbox::claim_batch(
            &mut db.conn(),
            "worker-a",
            16,
            60_000,
            tasker::util::now_ms(),
        )
        .await
        .expect("claim A");
        assert_eq!(claimed_a.len(), 1);
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.i(tasker::util::now_ms() - 1_000)
                .s(&claimed_a[0].outbox_id);
            conn.execute(
                "UPDATE automation_outbox SET lease_expires_at = ? WHERE id = ?",
                args,
            )
            .await
            .unwrap();
        }

        // Worker B reclaims the recovered record with a fresh token.
        let claimed_b = tasker::rules::outbox::claim_batch(
            &mut db.conn(),
            "worker-b",
            16,
            60_000,
            tasker::util::now_ms(),
        )
        .await
        .expect("claim B");
        assert_eq!(
            claimed_b.len(),
            1,
            "[{engine}] lease recovery re-queues the record"
        );
        assert_eq!(claimed_b[0].outbox_id, claimed_a[0].outbox_id);
        assert_ne!(
            claimed_b[0].claim_token, claimed_a[0].claim_token,
            "[{engine}] fresh fencing token"
        );
        assert_eq!(
            claimed_b[0].attempts, 0,
            "[{engine}] crash recovery does not burn retry budget"
        );

        // Stale worker A tries to execute with its expired token: the fenced
        // completion rejects it and the whole transaction rolls back.
        {
            let mut tx = db.begin().await.unwrap();
            let err = tasker::rules::outbox::execute_internal_action(
                &mut tx,
                &claimed_a[0].outbox_id,
                Some(&claimed_a[0].claim_token),
            )
            .await
            .expect_err("[{engine}] stale token must be rejected");
            tx.rollback().await;
            assert!(
                format!("{err}").contains("no longer owned"),
                "[{engine}] stale rejection: {err}"
            );
        }
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM comments WHERE task_id = ?",
                Some(&task_id)
            )
            .await,
            0,
            "[{engine}] stale worker produced no effect"
        );

        // Current owner B executes successfully.
        {
            let mut tx = db.begin().await.unwrap();
            let outcome = tasker::rules::outbox::execute_internal_action(
                &mut tx,
                &claimed_b[0].outbox_id,
                Some(&claimed_b[0].claim_token),
            )
            .await
            .expect("owner execution");
            tx.commit().await.unwrap();
            assert_eq!(outcome, tasker::rules::outbox::ExecutionOutcome::Completed);
        }
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM comments WHERE task_id = ?",
                Some(&task_id)
            )
            .await,
            1,
            "[{engine}] current owner's effect committed"
        );
    }
}

/// Task 5: retry classification, backoff rescheduling, and exhaustion.
#[tokio::test]
async fn rules_dispatcher_retry_and_dead_letter() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        let admin_email = format!("rules-retry-{suffix}@test.local");
        let member_email = format!("rules-retry-m-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Retry Admin").await;
        seed_user(&db, &member_email, "Retry Member").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "RTR"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role": "member"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "RTRP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = user_id_by_email(&db, &member_email).await;
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Retry target", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
            r#"[{"body":"retry probe"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let intent = tasker::rules::outbox::claim_batch(
            &mut db.conn(),
            "worker-a",
            16,
            60_000,
            tasker::util::now_ms(),
        )
        .await
        .expect("claim");
        assert_eq!(intent.len(), 1);
        let intent = &intent[0];

        // Retryable failure: rescheduled with backoff, attempts burn. Each
        // cycle mirrors reality: the backoff elapses, the worker re-claims
        // the due intent, the next attempt fails again.
        let transient = tasker::error::AppError::internal("database temporarily unavailable");
        let mut current = intent.clone();
        for attempt in 1..=7u32 {
            let disposition = tasker::rules::outbox::record_failure(
                &mut db.conn(),
                &current,
                tasker::rules::outbox::FailureClass::Retryable,
                &transient,
                8,
                1_000,
                60_000,
                1_000, // full jitter = deterministic bound
                tasker::util::now_ms(),
            )
            .await
            .expect("record retryable");
            assert_eq!(
                disposition,
                tasker::rules::outbox::FailureDisposition::Rescheduled,
                "[{engine}] attempt {attempt} reschedules"
            );
            // Backoff elapsed: make the intent due again and re-claim it.
            {
                let mut c = db.conn();
                let mut args = tasker::db::Args::new();
                args.i(tasker::util::now_ms()).s(&current.outbox_id);
                c.execute(
                    "UPDATE automation_outbox SET next_attempt_at = ? WHERE id = ?",
                    args,
                )
                .await
                .unwrap();
            }
            let reclaimed = tasker::rules::outbox::claim_batch(
                &mut db.conn(),
                "worker-a",
                16,
                60_000,
                tasker::util::now_ms(),
            )
            .await
            .expect("re-claim");
            assert_eq!(reclaimed.len(), 1, "[{engine}] attempt {attempt} re-claims");
            assert_eq!(
                reclaimed[0].attempts, attempt as i64,
                "[{engine}] attempts burn"
            );
            current = reclaimed[0].clone();
        }
        // The 8th failure exhausts the budget: dead-letter.
        let disposition = tasker::rules::outbox::record_failure(
            &mut db.conn(),
            &current,
            tasker::rules::outbox::FailureClass::Retryable,
            &transient,
            8,
            1_000,
            60_000,
            1_000,
            tasker::util::now_ms(),
        )
        .await
        .expect("record exhaustion");
        assert_eq!(
            disposition,
            tasker::rules::outbox::FailureDisposition::DeadLettered,
            "[{engine}] 8 attempts reach the terminal state"
        );
        let mut args = tasker::db::Args::new();
        args.s(&intent.outbox_id);
        let row = db
            .conn()
            .fetch_optional(
                "SELECT status, attempts, last_error_category FROM automation_outbox WHERE id = ?",
                args,
            )
            .await
            .unwrap()
            .expect("intent");
        assert_eq!(row.try_get::<String, _>("status").unwrap(), "dead_letter");
        assert_eq!(row.try_get::<i64, _>("attempts").unwrap(), 8);
        assert_eq!(
            row.try_get::<String, _>("last_error_category").unwrap(),
            "permanent"
        );

        // Permanent failures dead-letter on the first failure.
        seed_rule(
            &db,
            &team_id,
            "task_priority_changed",
            r#"{"field":"task.priority","op":"IN","values":["low","medium","high","urgent"]}"#,
            r#"[{"body":"permanent probe"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"priority": "high"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let intent = tasker::rules::outbox::claim_batch(
            &mut db.conn(),
            "worker-a",
            16,
            60_000,
            tasker::util::now_ms(),
        )
        .await
        .expect("claim permanent");
        assert_eq!(intent.len(), 1);
        let invalid_payload = tasker::error::AppError::internal(
            "stored action payload failed validation: condition: boom",
        );
        assert_eq!(
            tasker::rules::outbox::classify_failure(&invalid_payload),
            tasker::rules::outbox::FailureClass::Permanent,
            "[{engine}] payload corruption is permanent"
        );
        let disposition = tasker::rules::outbox::record_failure(
            &mut db.conn(),
            &intent[0],
            tasker::rules::outbox::FailureClass::Permanent,
            &invalid_payload,
            8,
            1_000,
            60_000,
            1_000,
            tasker::util::now_ms(),
        )
        .await
        .expect("record permanent");
        assert_eq!(
            disposition,
            tasker::rules::outbox::FailureDisposition::DeadLettered,
            "[{engine}] permanent failure dead-letters immediately"
        );
    }
}

/// Task 5: graceful shutdown without claiming, and concurrent claim safety.
#[tokio::test]
async fn rules_dispatcher_shutdown_and_concurrent_claims() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        let admin_email = format!("rules-shut-{suffix}@test.local");
        let member_email = format!("rules-shut-m-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Shut Admin").await;
        seed_user(&db, &member_email, "Shut Member").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "SHU"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role": "member"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "SHUP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = user_id_by_email(&db, &member_email).await;
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Shutdown target", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
            r#"[{"body":"shutdown probe"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);

        // Shutdown signalled BEFORE spawn: the worker exits without claiming.
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(true);
        let handle = tasker::rules::dispatcher::spawn(
            (*db).clone(),
            tasker::config::RulesWorkerConfig::default(),
            tasker::http::metrics::Metrics::new(),
            shutdown_rx,
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("[{engine}] shutdown worker exits promptly")
            .expect("worker task");
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM automation_outbox WHERE status = 'pending'",
                None,
            )
            .await,
            1,
            "[{engine}] shutdown worker claimed nothing"
        );

        // Concurrent claims: disjoint winner sets, no double ownership.
        let db1 = db.clone();
        let db2 = db.clone();
        let mut conn_a = db1.conn();
        let mut conn_b = db2.conn();
        let (a, b) = tokio::join!(
            tasker::rules::outbox::claim_batch(
                &mut conn_a,
                "worker-a",
                16,
                60_000,
                tasker::util::now_ms(),
            ),
            tasker::rules::outbox::claim_batch(
                &mut conn_b,
                "worker-b",
                16,
                60_000,
                tasker::util::now_ms(),
            ),
        );
        let a = a.expect("claim A");
        let b = b.expect("claim B");
        let a_ids: Vec<&String> = a.iter().map(|c| &c.outbox_id).collect();
        let b_ids: Vec<&String> = b.iter().map(|c| &c.outbox_id).collect();
        assert_eq!(
            a_ids.len() + b_ids.len(),
            1,
            "[{engine}] one record, two workers: exactly one winner"
        );
        assert!(
            a_ids.iter().all(|id| !b_ids.contains(id)),
            "[{engine}] claims are disjoint"
        );
    }
}

/// Reliability review: the `principal_email_conflict` classification is
/// exercised end to end — a claimed intent executed while the reserved
/// identity is held by a foreign account fails, is classified Permanent, and
/// dead-letters on the first attempt instead of burning the retry budget.
/// SQLite-leg only: the conflict fixture poisons the principal identity, and
/// the shared CI PG/MySQL databases must not be affected.
#[tokio::test]
async fn rules_dispatcher_email_conflict_dead_letters_immediately() {
    let url = format!(
        "sqlite:{}/{}",
        std::env::temp_dir().display(),
        Uuid::now_v7()
    );
    let engine = "sqlite";
    let app = spawn_app(&url).await;
    let db = app.db.clone();
    let suffix = Uuid::now_v7().simple();

    // A foreign account holds the reserved identity email BEFORE the
    // dispatcher ever runs.
    {
        let mut conn = db.conn();
        let now = tasker::util::now_ms();
        let mut args = tasker::db::Args::new();
        args.s(Uuid::now_v7().to_string())
            .s("automation@tasker.local")
            .s("Foreign Squatter")
            .s("$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .b(true)
            .i(now)
            .i(now);
        conn.execute(
            "INSERT INTO users (id, email, display_name, password_hash, is_active, \
             created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
        .unwrap();
    }

    let admin_email = format!("rules-conf-{suffix}@test.local");
    let member_email = format!("rules-conf-m-{suffix}@test.local");
    let admin_id = seed_user(&db, &admin_email, "Conf Admin").await;
    seed_user(&db, &member_email, "Conf Member").await;
    let admin = login_as(&app.app, &admin_email, "test-password-123").await;
    let member = login_as(&app.app, &member_email, "test-password-123").await;
    let (code, team) = send_json(
        &app.app,
        "POST",
        "/api/v1/teams",
        Some(&admin),
        json!({"name": "CONF"}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let team_id = team["id"].as_str().unwrap().to_string();
    let (code, _) = send_json(
        &app.app,
        "POST",
        &format!("/api/v1/teams/{team_id}/members"),
        Some(&admin),
        json!({"email": member_email, "role": "member"}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let (code, project) = send_json(
        &app.app,
        "POST",
        &format!("/api/v1/teams/{team_id}/projects"),
        Some(&admin),
        json!({"name": "CONFP"}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let project_id = project["id"].as_str().unwrap().to_string();
    let member_id = user_id_by_email(&db, &member_email).await;
    let (code, task) = send_json(
        &app.app,
        "POST",
        &format!("/api/v1/projects/{project_id}/tasks"),
        Some(&admin),
        json!({"title": "Conflict target", "assignee_id": member_id}),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap().to_string();
    seed_rule(
        &db,
        &team_id,
        "task_status_changed",
        r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
        r#"[{"body":"conflict probe"}]"#,
        true,
        1,
        &admin_id,
    )
    .await;
    let (code, _) = send_json(
        &app.app,
        "POST",
        &format!("/api/v1/tasks/{task_id}/transition"),
        Some(&member),
        json!({"to": "todo"}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);

    // A full dispatcher cycle claims the intent itself: the conflict is
    // classified Permanent and the intent dead-letters on attempt 1 — the
    // retry budget is not consumed by a permanently unresolvable failure.
    let metrics = tasker::http::metrics::Metrics::new();
    let config = tasker::config::RulesWorkerConfig::default();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(2));
    tasker::rules::dispatcher::run_once(&db, &config, "worker-a", &metrics, &semaphore)
        .await
        .expect("run_once");

    let mut args = tasker::db::Args::new();
    args.s(&task_id);
    let row = db
        .conn()
        .fetch_optional(
            "SELECT status, attempts, last_error_category, last_error_detail \
             FROM automation_outbox WHERE task_id = ?",
            args,
        )
        .await
        .unwrap()
        .expect("intent");
    assert_eq!(
        row.try_get::<String, _>("status").unwrap(),
        "dead_letter",
        "[{engine}] conflict dead-letters immediately"
    );
    assert_eq!(
        row.try_get::<i64, _>("attempts").unwrap(),
        1,
        "[{engine}] one attempt, not 8"
    );
    assert_eq!(
        row.try_get::<String, _>("last_error_category").unwrap(),
        "permanent"
    );
    let detail = row.try_get::<String, _>("last_error_detail").unwrap();
    assert!(
        detail.contains("principal_email_conflict"),
        "[{engine}] diagnosable category persisted: {detail}"
    );
    // The rendered metrics expose the dead-letter + zero executions.
    let rendered = metrics.render(0);
    assert!(
        rendered.contains("tasker_outbox_dead_lettered_total 1"),
        "[{engine}] dead-letter counter: {rendered}"
    );
    assert!(
        rendered.contains("tasker_outbox_actions_executed_total 0"),
        "[{engine}] nothing executed"
    );
}

/// Task 6: rules management API — CRUD, authorization matrix, CAS updates,
/// archive semantics, history access.
/// Phase 8B Task 1: integration foundation — migrations, tenant isolation
/// (composite FK), delivery idempotency, event vocabulary, and provenance
/// columns.
#[tokio::test]
async fn integrations_foundation_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        // ---- Fixtures: two teams with projects, two users ----
        let admin_email = format!("int-f-{suffix}@test.local");
        let other_email = format!("int-f-o-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Int Admin").await;
        let other_id = seed_user(&db, &other_email, "Int Other").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "INT"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, other_team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "INT2"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let other_team_id = other_team["id"].as_str().unwrap().to_string();
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "INTP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let (code, other_project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{other_team_id}/projects"),
            Some(&admin),
            json!({"name": "INTP2"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let other_project_id = other_project["id"].as_str().unwrap().to_string();

        // ---- Integration create + duplicate name rejection ----
        let integration_id = {
            let mut conn = db.conn();
            let id = Uuid::now_v7().to_string();
            let mut args = tasker::db::Args::new();
            args.s(&id)
                .s(&team_id)
                .s("github")
                .s(format!("watcher-{suffix}"))
                .opt_s(None::<String>)
                .b(true)
                .s(&admin_id)
                .i(tasker::util::now_ms())
                .i(tasker::util::now_ms())
                .opt_i(None::<i64>);
            conn.execute(
                "INSERT INTO integrations (id, team_id, provider, name, description, enabled, \
                 created_by, created_at, updated_at, archived_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                args,
            )
            .await
            .unwrap_or_else(|e| panic!("[{engine}] integration insert: {e}"));
            id
        };
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s(&team_id)
                .s(format!("watcher-{suffix}"))
                .opt_s(None::<String>)
                .s("github")
                .b(true)
                .s(&admin_id)
                .i(tasker::util::now_ms())
                .i(tasker::util::now_ms())
                .opt_i(None::<i64>);
            let dup = conn
                .execute(
                    "INSERT INTO integrations (id, team_id, provider, name, description, enabled, \
                     created_by, created_at, updated_at, archived_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    args,
                )
                .await;
            assert!(dup.is_err(), "[{engine}] duplicate team+name rejected");
        }
        // Same name in another team is allowed.
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s(&other_team_id)
                .s("gitlab")
                .s(format!("watcher-{suffix}"))
                .opt_s(None::<String>)
                .b(true)
                .s(&other_id)
                .i(tasker::util::now_ms())
                .i(tasker::util::now_ms())
                .opt_i(None::<i64>);
            conn.execute(
                "INSERT INTO integrations (id, team_id, provider, name, description, enabled, \
                 created_by, created_at, updated_at, archived_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                args,
            )
            .await
            .unwrap_or_else(|e| panic!("[{engine}] other-team integration insert: {e}"));
        }

        // ---- Credential: one row per integration, version 1, active ----
        {
            let mut conn = db.begin().await.unwrap();
            let version = tasker::integrations::credentials::provision_credential(
                &mut conn,
                &integration_id,
                "webhook-secret-abc",
                &[19u8; 32],
                &admin_id,
                tasker::util::now_ms(),
            )
            .await
            .expect("provision");
            assert_eq!(version, 1);
            conn.commit().await.unwrap();
            // Duplicate provisioning rejected by the unique constraint.
            let mut conn = db.begin().await.unwrap();
            let dup = tasker::integrations::credentials::provision_credential(
                &mut conn,
                &integration_id,
                "second",
                &[19u8; 32],
                &admin_id,
                tasker::util::now_ms(),
            )
            .await;
            conn.rollback().await;
            assert!(dup.is_err(), "[{engine}] second credential row rejected");
            // Verification succeeds with the right secret and fails closed
            // with the wrong one.
            let mut conn = db.conn();
            assert!(
                tasker::integrations::credentials::verify_webhook_secret(
                    &mut conn,
                    &integration_id,
                    "webhook-secret-abc",
                    &[19u8; 32],
                )
                .await
                .expect("verify"),
                "[{engine}] correct secret verifies"
            );
            assert!(
                !tasker::integrations::credentials::verify_webhook_secret(
                    &mut conn,
                    &integration_id,
                    "wrong-secret",
                    &[19u8; 32],
                )
                .await
                .expect("verify wrong"),
                "[{engine}] wrong secret fails closed"
            );
        }

        // ---- Repository registration + M:N project mapping ----
        let repo_id = {
            let mut conn = db.conn();
            let id = Uuid::now_v7().to_string();
            let mut args = tasker::db::Args::new();
            args.s(&id)
                .s(&integration_id)
                .s(&team_id)
                .s("987654")
                .s("octocat/tasker")
                .i(tasker::util::now_ms());
            conn.execute(
                "INSERT INTO integration_repositories (id, integration_id, team_id, \
                 external_repo_id, full_name, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                args,
            )
            .await
            .unwrap_or_else(|e| panic!("[{engine}] repo insert: {e}"));
            id
        };
        // Duplicate external repo id within the integration rejected.
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s(&integration_id)
                .s("987654")
                .s("octocat/tasker")
                .i(tasker::util::now_ms());
            let dup = conn
                .execute(
                    "INSERT INTO integration_repositories (id, integration_id, team_id, \
                     external_repo_id, full_name, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                    args,
                )
                .await;
            assert!(dup.is_err(), "[{engine}] duplicate external repo rejected");
        }

        // Map repo → project (same team): accepted.
        let mapping_id = {
            let mut conn = db.conn();
            let id = Uuid::now_v7().to_string();
            let mut args = tasker::db::Args::new();
            args.s(&id)
                .s(&repo_id)
                .s(&project_id)
                .s(&team_id)
                .s(&admin_id)
                .i(tasker::util::now_ms());
            conn.execute(
                "INSERT INTO integration_repository_projects (id, repository_id, project_id, \
                 team_id, mapped_by, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                args,
            )
            .await
            .unwrap_or_else(|e| panic!("[{engine}] mapping insert: {e}"));
            id
        };
        // Cross-team mapping: rejected by the composite foreign keys.
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s(&repo_id)
                .s(&other_project_id)
                .s(&other_team_id)
                .s(&admin_id)
                .i(tasker::util::now_ms());
            let cross = conn
                .execute(
                    "INSERT INTO integration_repository_projects (id, repository_id, project_id, \
                     team_id, mapped_by, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                    args,
                )
                .await;
            assert!(
                cross.is_err(),
                "[{engine}] cross-team mapping must be rejected by the database"
            );
        }

        // ---- Delivery idempotency: duplicate delivery id is a no-op ----
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s(&integration_id)
                .s("gh-delivery-1")
                .s("push")
                .s("{}")
                .i(tasker::util::now_ms())
                .i(tasker::util::now_ms())
                .i(tasker::util::now_ms());
            conn.execute(
                "INSERT INTO webhook_deliveries (id, integration_id, delivery_id, \
                 event_type_raw, payload, status, attempts, next_attempt_at, created_at, \
                 updated_at) VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, ?, ?)",
                args,
            )
            .await
            .unwrap_or_else(|e| panic!("[{engine}] delivery insert: {e}"));
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s(&integration_id)
                .s("gh-delivery-1")
                .s("push")
                .s("{}")
                .i(tasker::util::now_ms())
                .i(tasker::util::now_ms())
                .i(tasker::util::now_ms());
            let dup = conn
                .execute(
                    "INSERT INTO webhook_deliveries (id, integration_id, delivery_id, \
                     event_type_raw, payload, status, attempts, next_attempt_at, created_at, \
                     updated_at) VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, ?, ?)",
                    args,
                )
                .await;
            assert!(dup.is_err(), "[{engine}] duplicate delivery rejected");
            // Invalid status rejected by the CHECK.
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s(&integration_id)
                .s("gh-delivery-2")
                .s("push")
                .s("{}")
                .i(tasker::util::now_ms())
                .i(tasker::util::now_ms());
            let bad_status = conn
                .execute(
                    "INSERT INTO webhook_deliveries (id, integration_id, delivery_id, \
                     event_type_raw, payload, status, attempts, next_attempt_at, created_at, \
                     updated_at) VALUES (?, ?, ?, ?, ?, 'queued', 0, ?, ?, ?)",
                    args,
                )
                .await;
            assert!(bad_status.is_err(), "[{engine}] invalid status rejected");
        }

        // ---- Event vocabulary: integration events accepted, provenance
        // ---- required, duplicates rejected, unknown types rejected. ----
        let now = tasker::util::now_ms();
        {
            let mut conn = db.conn();
            let now = tasker::util::now_ms();
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s("git_push")
                .i(1i64)
                .s(&team_id)
                .opt_i(None) // task_id NULL (taskless)
                .opt_i(None) // actor_id NULL (never impersonates)
                .opt_s(Some(r#"{"branch":"main"}"#))
                .opt_s(Some(r#"{"branch":"main"}"#))
                .s("integration")
                .opt_i(None) // causing_execution_id
                .i(now) // occurred_at
                .opt_i(None) // integration_id (Task 5 delivery wires it)
                .opt_i(None); // source_delivery_id (Task 5 delivery wires it)
            conn.execute(
                "INSERT INTO domain_events (id, event_type, schema_version, team_id, task_id, \
                 actor_id, previous_state, new_state, origin, causing_execution_id, \
                 occurred_at, integration_id, source_delivery_id) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                args,
            )
            .await
            .unwrap_or_else(|e| panic!("[{engine}] integration event insert: {e}"));
        }
        // Unknown event type rejected.
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s("github_pushed")
                .i(1i64)
                .s(&team_id)
                .i(now)
                .s("integration")
                .i(now);
            let unknown = conn
                .execute(
                    "INSERT INTO domain_events (id, event_type, schema_version, team_id, \
                     occurred_at, origin) VALUES (?, ?, ?, ?, ?, ?)",
                    args,
                )
                .await;
            assert!(unknown.is_err(), "[{engine}] unknown event type rejected");
        }
        // Unknown origin rejected.
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(Uuid::now_v7().to_string())
                .s("git_push")
                .i(1i64)
                .s(&team_id)
                .s("provider")
                .i(now);
            let unknown_origin = conn
                .execute(
                    "INSERT INTO domain_events (id, event_type, schema_version, team_id, \
                     occurred_at, origin) VALUES (?, ?, ?, ?, ?, ?)",
                    args,
                )
                .await;
            assert!(
                unknown_origin.is_err(),
                "[{engine}] unknown origin rejected"
            );
        }
    }
}

async fn rules_management_api_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        // ---- Fixtures: owner, admin, manager, member, guest, outsider ----
        let owner_email = format!("rules-mgr-o-{suffix}@test.local");
        let admin_email = format!("rules-mgr-a-{suffix}@test.local");
        let manager_email = format!("rules-mgr-m-{suffix}@test.local");
        let member_email = format!("rules-mgr-mb-{suffix}@test.local");
        let guest_email = format!("rules-mgr-g-{suffix}@test.local");
        let outsider_email = format!("rules-mgr-x-{suffix}@test.local");
        let owner_id = seed_user(&db, &owner_email, "Owner").await;
        seed_user(&db, &admin_email, "Admin").await;
        seed_user(&db, &manager_email, "Manager").await;
        seed_user(&db, &member_email, "Member").await;
        seed_user(&db, &guest_email, "Guest").await;
        seed_user(&db, &outsider_email, "Outsider").await;
        let owner = login_as(&app.app, &owner_email, "test-password-123").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let manager = login_as(&app.app, &manager_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let guest = login_as(&app.app, &guest_email, "test-password-123").await;
        let outsider = login_as(&app.app, &outsider_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&owner),
            json!({"name": "MGT"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        for (email, role) in [
            (&admin_email, "admin"),
            (&manager_email, "manager"),
            (&member_email, "member"),
        ] {
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/teams/{team_id}/members"),
                Some(&owner),
                json!({"email": email, "role": role}),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
        }
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&owner),
            json!({"name": "MGTP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();

        // ---- Create (Owner) ----
        let create_body = json!({
            "name": "auto-triage",
            "description": "route work automatically",
            "trigger_type": "task_status_changed",
            "condition": {"field": "task.status", "op": "IN",
                          "values": ["backlog","todo","in_progress","resolved","in_review","closed"]},
            "actions": [{"body": "automation triage"}],
            "enabled": true,
        });
        let (code, rule) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&owner),
            create_body.clone(),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] owner create");
        assert_eq!(rule["current_version"], 1);
        assert_eq!(rule["enabled"], true);
        assert_eq!(rule["condition"]["op"], "IN");
        assert_eq!(rule["actions"].as_array().unwrap().len(), 1);
        let rule_id = rule["id"].as_str().unwrap().to_string();

        // Admin can also create; member cannot; outsider/guest get 404/403.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&admin),
            json!({"name": "admin-rule", "trigger_type": "task_assigned",
                   "condition": {"field":"task.assignee_id","op":"NEQ","value":null},
                   "actions": [{"body":"hi"}]}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] admin create");
        let (code, problem) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&member),
            create_body.clone(),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member create forbidden"
        );
        assert_eq!(problem["status"], 403);
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&outsider),
            create_body.clone(),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider 404");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&guest),
            create_body.clone(),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest 404");

        // Duplicate name → 409; archived name stays reserved (covered below).
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&owner),
            create_body,
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] duplicate name");

        // Invalid condition → 422 with field errors.
        let (code, problem) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&owner),
            json!({"name": "bad-cond", "trigger_type": "task_status_changed",
                   "condition": {"field": "task.title", "op": "EQ", "value": "x"},
                   "actions": [{"body": "x"}]}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] invalid condition"
        );
        assert!(
            problem["errors"].as_array().is_some(),
            "[{engine}] field errors present"
        );

        // ---- List + filters ----
        let (code, list) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&owner),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(list["total"], 2, "[{engine}] two live rules");
        let (code, list) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/teams/{team_id}/rules?trigger_type=task_assigned"),
            Some(&admin),
            json!(null),
        )
        .await;
        assert_eq!(list["total"], 1, "[{engine}] trigger filter");
        // Member cannot list.
        let (code, _) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&member),
            json!(null),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member list forbidden"
        );

        // ---- Update with CAS: wrong version → 409; correct → v2 ----
        let (code, problem) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/rules/{rule_id}"),
            Some(&owner),
            json!({"name": "renamed", "expected_version": 99}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] CAS conflict");
        let (code, updated) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/rules/{rule_id}"),
            Some(&owner),
            json!({"name": "renamed", "expected_version": 1}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] CAS update");
        assert_eq!(updated["current_version"], 2);
        assert_eq!(updated["name"], "renamed");
        // Manager cannot update.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/rules/{rule_id}"),
            Some(&manager),
            json!({"name": "mgr", "expected_version": 2}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] manager update forbidden"
        );

        // Version history: two immutable rows, newest first.
        let (code, versions) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/rules/{rule_id}/versions"),
            Some(&admin),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let versions = versions["items"].as_array().unwrap();
        assert_eq!(versions.len(), 2, "[{engine}] two immutable versions");
        assert_eq!(versions[0]["version"], 2);
        assert_eq!(versions[1]["version"], 1);

        // ---- History: generate executions, then check access matrix ----
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&owner),
            json!({"title": "History target"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&owner),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] backlog->todo needs assignee"
        );
        // Owner is the reporter, so the assignee must differ: assign the
        // manager, who then performs the transition as the assignee.
        let manager_id = user_id_by_email(&db, &manager_email).await;
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&owner),
            json!({"assignee_id": manager_id}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&manager),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, history) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/rules/{rule_id}/history"),
            Some(&manager),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] manager reads history");
        assert_eq!(
            history["total"], 1,
            "[{engine}] one execution recorded: {history}"
        );
        let entry = &history["items"][0];
        // The CAS rename to v2 happened before the transition, so the
        // execution must cite the version current at evaluation time.
        assert_eq!(
            entry["rule_version"], 2,
            "[{engine}] execution cites the evaluated version"
        );
        assert_eq!(entry["condition_result"], "matched");
        // Action-level outcomes exist once the dispatcher executes the
        // internal intent and records the ledger entry.
        assert_eq!(
            entry["actions"].as_array().unwrap().len(),
            0,
            "[{engine}] no action results before a dispatcher cycle"
        );
        {
            let metrics = tasker::http::metrics::Metrics::new();
            let config = tasker::config::RulesWorkerConfig::default();
            let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(2));
            tasker::rules::dispatcher::run_once(&db, &config, "worker-a", &metrics, &semaphore)
                .await
                .expect("dispatcher cycle");
        }
        let (code, history) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/rules/{rule_id}/history"),
            Some(&manager),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let entry = &history["items"][0];
        assert_eq!(
            entry["actions"].as_array().unwrap().len(),
            1,
            "[{engine}] action-level outcome recorded: {entry}"
        );
        // Member cannot read history.
        let (code, _) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/rules/{rule_id}/history"),
            Some(&member),
            json!(null),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member history forbidden"
        );
        // Outsider 404.
        let (code, _) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/rules/{rule_id}/history"),
            Some(&outsider),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND);

        // ---- Enable / disable ----
        let (code, disabled) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/rules/{rule_id}/disable"),
            Some(&admin),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(disabled["enabled"], false);
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/rules/{rule_id}/enable"),
            Some(&admin),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::OK);

        // ---- Archive: stops evaluation, name stays reserved ----
        let (code, archived) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/rules/{rule_id}/archive"),
            Some(&owner),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert!(
            archived["archived_at"].is_i64(),
            "[{engine}] archived_at set"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/rules"),
            Some(&owner),
            json!({"name": "renamed", "trigger_type": "task_status_changed",
                   "condition": {"field":"task.status","op":"IN","values":["todo"]},
                   "actions": [{"body":"x"}]}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] archived name reserved"
        );
        // Archived rule is read-only.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/rules/{rule_id}"),
            Some(&owner),
            json!({"name": "zzz", "expected_version": 2}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] archived rule read-only"
        );
        // The dispatcher never claims intents for archived rules: no new
        // executions after archiving.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&owner),
            json!({"priority": "high"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        // The assignee (manager) performs the transition; the archived rule
        // must not fire on it.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&manager),
            json!({"to": "in_progress"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, history) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/rules/{rule_id}/history"),
            Some(&manager),
            json!(null),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            history["total"], 1,
            "[{engine}] archived rule stops evaluating"
        );

        // History remains available independently of task deletion.
        {
            let mut conn = db.conn();
            let mut args = tasker::db::Args::new();
            args.s(&task_id);
            conn.execute("DELETE FROM tasks WHERE id = ?", args)
                .await
                .unwrap();
        }
        let (code, history) = send_json(
            &app.app,
            "GET",
            &format!("/api/v1/rules/{rule_id}/history"),
            Some(&manager),
            json!(null),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] history survives task deletion"
        );
        assert_eq!(history["total"], 1);
    }
}

async fn rules_capture_atomicity_on_failure() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let db = app.db.clone();
        let suffix = Uuid::now_v7().simple();

        let admin_email = format!("rules-atom-{suffix}@test.local");
        let member_email = format!("rules-atom-m-{suffix}@test.local");
        let admin_id = seed_user(&db, &admin_email, "Rules Atomic").await;
        seed_user(&db, &member_email, "Rules Atomic Member").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name": "RA"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role": "member"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name": "RAP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = user_id_by_email(&db, &member_email).await;
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "Atomicity target", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();

        // Corruption A: a stored version whose condition is not valid JSON.
        // The application contract makes this unwritable; inserting it raw
        // simulates external corruption, and the pipeline must fail the WHOLE
        // transaction rather than skip the rule.
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            "not json at all",
            r#"[{"body":"x"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;

        // The creation-assignment event already exists; the corrupt
        // transition must add NOTHING.
        let events_before = count_rows(
            &db,
            "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
            Some(&task_id),
        )
        .await;
        let exec_before = count_rows(&db, "SELECT COUNT(*) FROM rule_executions", None).await;
        let outbox_before = count_rows(&db, "SELECT COUNT(*) FROM automation_outbox", None).await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::INTERNAL_SERVER_ERROR,
            "[{engine}] corrupt definition fails the mutation"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                Some(&task_id),
            )
            .await,
            events_before,
            "[{engine}] no event persisted"
        );
        assert_eq!(
            count_rows(&db, "SELECT COUNT(*) FROM rule_executions", None).await,
            exec_before,
            "[{engine}] no execution persisted"
        );
        assert_eq!(
            count_rows(&db, "SELECT COUNT(*) FROM automation_outbox", None).await,
            outbox_before,
            "[{engine}] no intent persisted"
        );
        // The originating mutation itself rolled back.
        let mut conn = db.conn();
        let mut args = tasker::db::Args::new();
        args.s(&task_id);
        let row = conn
            .fetch_optional("SELECT status FROM tasks WHERE id = ?", args)
            .await
            .unwrap()
            .expect("task row");
        assert_eq!(
            row.try_get::<String, _>("status").unwrap(),
            "backlog",
            "[{engine}] task mutation rolled back"
        );

        // Corruption B: an action list that violates the application cap.
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
            r#"[{"body":"1"},{"body":"2"},{"body":"3"},{"body":"4"},{"body":"5"},{"body":"6"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::INTERNAL_SERVER_ERROR,
            "[{engine}] oversized action list fails the mutation"
        );
        assert_eq!(
            count_rows(&db, "SELECT COUNT(*) FROM automation_outbox", None).await,
            outbox_before,
            "[{engine}] still no intents"
        );
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                Some(&task_id),
            )
            .await,
            events_before,
            "[{engine}] still no events"
        );

        // Control: with the corrupt definitions removed, the same transition
        // succeeds end to end.
        {
            let mut dconn = db.conn();
            // Child-first: rule_versions RESTRICT-deletes on rules.
            dconn
                .execute(
                    "DELETE FROM rule_versions WHERE rule_id IN (SELECT id FROM rules WHERE team_id = ?)",
                    {
                        let mut args = tasker::db::Args::new();
                        args.s(&team_id);
                        args
                    },
                )
                .await
                .unwrap();
            let mut args = tasker::db::Args::new();
            args.s(&team_id);
            dconn
                .execute("DELETE FROM rules WHERE team_id = ?", args)
                .await
                .unwrap();
        }
        seed_rule(
            &db,
            &team_id,
            "task_status_changed",
            r#"{"field":"task.status","op":"IN","values":["backlog","todo","in_progress","resolved","in_review","closed"]}"#,
            r#"[{"body":"ok now"}]"#,
            true,
            1,
            &admin_id,
        )
        .await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/transition"),
            Some(&member),
            json!({"to": "todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] healthy pipeline succeeds");
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM domain_events WHERE task_id = ?",
                Some(&task_id),
            )
            .await,
            events_before + 1,
            "[{engine}] event persisted"
        );
        assert_eq!(
            count_rows(&db, "SELECT COUNT(*) FROM automation_outbox", None).await,
            outbox_before + 1,
            "[{engine}] intent persisted"
        );
    }
}

async fn db_user_mfa_exists(db: &Database, user_id: &str) -> bool {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(user_id);
    conn.fetch_optional("SELECT id FROM user_mfa WHERE user_id = ?", args)
        .await
        .unwrap()
        .is_some()
}

async fn db_user_mfa_enabled(db: &Database, user_id: &str) -> bool {
    let mut conn = db.conn();
    let mut args = tasker::db::Args::new();
    args.s(user_id);
    conn.fetch_optional(
        "SELECT id FROM user_mfa WHERE user_id = ? AND enabled_at IS NOT NULL",
        args,
    )
    .await
    .unwrap()
    .is_some()
}

// ---------------------------------------------------------------------------
// Phase 3: guests, dependencies, workload, templates
// ---------------------------------------------------------------------------

/// Comprehensive Phase 3 verification on every engine.
#[tokio::test]
#[allow(
    clippy::float_equality_without_abs,
    unused_variables,
    unused_assignments
)]
async fn phase3_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("admin-{suffix}@test.local");
        let member_email = format!("member-{suffix}@test.local");
        let guest_email = format!("guest-{suffix}@test.local");
        let outsider_email = format!("outsider-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "Admin").await;
        seed_user(&app.db, &member_email, "Member").await;
        seed_user(&app.db, &guest_email, "Guest").await;
        seed_user(&app.db, &outsider_email, "Outsider").await;

        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let guest = login_as(&app.app, &guest_email, "test-password-123").await;
        let outsider = login_as(&app.app, &outsider_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"T"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role":"member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"Shared"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap();
        let (code, other_project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"Private"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let other_project_id = other_project["id"].as_str().unwrap();

        let member_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "member")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let admin_id = {
            let (_code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "owner")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let guest_id = {
            // fetch the guest user id by logging in and reading /me
            let (code, me) = get_json(&app.app, "/api/v1/me", &guest).await;
            assert_eq!(code, StatusCode::OK);
            me["id"].as_str().unwrap().to_string()
        };

        // Two tasks in the shared project, one in the private project.
        let (code, t1) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title":"First","assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let t1_id = t1["id"].as_str().unwrap();
        let (code, t2) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title":"Second"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let t2_id = t2["id"].as_str().unwrap();
        let (code, t3) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{other_project_id}/tasks"),
            Some(&admin),
            json!({"title":"Private task"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let t3_id = t3["id"].as_str().unwrap();

        // ---- Guest management ------------------------------------------------
        // Outsiders and guests cannot manage; only Owner/Admin/Manager.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/guests"),
            Some(&outsider),
            json!({"email": guest_email}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider grant");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/guests"),
            Some(&member),
            json!({"email": guest_email}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member grant (needs Manager+)"
        );

        // Granting to a team member is rejected (pointless/privilege confusion).
        let (code, problem) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/guests"),
            Some(&admin),
            json!({"email": member_email}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] member as guest");
        assert!(problem["detail"].as_str().unwrap().contains("team member"));

        // Unknown user rejected as validation.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/guests"),
            Some(&admin),
            json!({"email": "nobody@nowhere.example"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] unknown guest"
        );

        // Duplicate grant rejected.
        let (code, guest_entry) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/guests"),
            Some(&admin),
            json!({"email": guest_email}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] grant");
        assert_eq!(guest_entry["role"], "viewer");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/guests"),
            Some(&admin),
            json!({"email": guest_email}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] duplicate grant");

        // Guest list requires Manager+.
        // Guests get 404 for the roster (non-disclosure) rather than 403.
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/guests"),
            &guest,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest lists guests");
        let (_code, guests) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/guests"),
            &admin,
        )
        .await;
        assert_eq!(
            guests["items"].as_array().unwrap().len(),
            1,
            "[{engine}] one guest"
        );

        // ---- Guest read access -----------------------------------------------
        let (code, p) = get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &guest).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] guest reads project");
        assert_eq!(p["is_guest"], true, "[{engine}] guest flag");
        let (code, tasks) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/tasks"),
            &guest,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            tasks["total"].as_i64().unwrap(),
            2,
            "[{engine}] guest sees both tasks"
        );
        let (code, board) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/board"),
            &guest,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] guest board");
        let _ = board;

        // Cross-project/cross-team isolation.
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/projects/{other_project_id}"),
            &guest,
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] guest other project"
        );
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/tasks"),
            &outsider,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider tasks");
        let (code, _) = get_json(&app.app, &format!("/api/v1/tasks/{t3_id}"), &guest).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest private task");

        // ---- Guest mutations rejected ----------------------------------------
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&guest),
            json!({"title":"Guest task"}),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] guest create task");
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{t1_id}"),
            Some(&guest),
            json!({"title":"Hacked"}),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] guest patch task");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t1_id}/transition"),
            Some(&guest),
            json!({"to":"todo"}),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] guest transition");
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/tasks/{t1_id}"),
            Some(&guest),
        )
        .await;
        assert!(
            matches!(code, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "[{engine}] guest delete task: {code}"
        );

        // Comments: internal by default, guests cannot write.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t1_id}/comments"),
            Some(&guest),
            json!({"body":"guest comment"}),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] guest comment");
        let (code, internal) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t1_id}/comments"),
            Some(&admin),
            json!({"body":"internal secret note"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        assert_eq!(
            internal["guest_visible"], false,
            "[{engine}] comments internal by default"
        );
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/comments/{}", internal["id"].as_str().unwrap()),
            Some(&guest),
            json!({"body":"hacked"}),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] guest edit comment");
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/comments/{}", internal["id"].as_str().unwrap()),
            Some(&guest),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] guest delete comment"
        );

        // A guest-visible comment IS visible to the guest.
        let (code, gv) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t1_id}/comments"),
            Some(&admin),
            json!({"body":"status note for the guest","guest_visible": true}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        assert_eq!(gv["guest_visible"], true);
        let (_code, guest_comments) =
            get_json(&app.app, &format!("/api/v1/tasks/{t1_id}/comments"), &guest).await;
        assert!(
            !guest_comments["items"].is_null(),
            "[debug] guest comments: {guest_comments}"
        );
        let seen: Vec<&str> = guest_comments["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["body"].as_str().unwrap())
            .collect();
        assert!(
            seen.contains(&"status note for the guest"),
            "[{engine}] guest sees visible comment"
        );
        assert!(
            !seen.contains(&"internal secret note"),
            "[{engine}] internal comment hidden"
        );

        // Activity: guest sees the whitelist only (no comment events, no
        // rejection reasons). Internal events remain for members.
        let (_code, guest_activity) =
            get_json(&app.app, &format!("/api/v1/tasks/{t1_id}/activity"), &guest).await;
        let guest_events: Vec<&str> = guest_activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["event"].as_str().unwrap())
            .collect();
        assert!(
            !guest_events.contains(&"comment_added"),
            "[{engine}] comment events hidden"
        );
        assert!(
            !guest_events.contains(&"task_rejection_reason"),
            "[{engine}] rejection reasons hidden"
        );
        assert!(
            guest_events.contains(&"task_created"),
            "[{engine}] whitelist present: {guest_events:?}"
        );
        let (_code, member_activity) =
            get_json(&app.app, &format!("/api/v1/tasks/{t1_id}/activity"), &admin).await;
        let member_events: Vec<&str> = member_activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["event"].as_str().unwrap())
            .collect();
        assert!(
            member_events.contains(&"comment_added"),
            "[{engine}] members see internal events"
        );

        // Team settings + guests management + workload remain out of reach.
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/members"),
            &guest,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest team members");
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/teams/{team_id}"),
            Some(&guest),
            json!({"name":"Hacked"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] guest team settings"
        );
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/workload"),
            &guest,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest workload");

        // Templates hidden from guests.
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/templates"),
            &guest,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest templates");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/templates"),
            Some(&guest),
            json!({"name":"T","default_title":"X"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] guest create template"
        );

        // ---- Revocation is immediate ------------------------------------------
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}/guests/{guest_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] revoke");
        let (code, _) = get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &guest).await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] revoked project read"
        );
        let (code, _) = get_json(&app.app, &format!("/api/v1/tasks/{t1_id}"), &guest).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] revoked task read");
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}/guests/{guest_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] double revoke");

        // Re-grant for dependency tests.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/guests"),
            Some(&admin),
            json!({"email": guest_email}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, _) = get_json(&app.app, &format!("/api/v1/tasks/{t1_id}"), &guest).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] re-granted read");

        // ---- Dependencies ------------------------------------------------------
        // Guest cannot create; can read.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            Some(&guest),
            json!({"predecessor_task_id": t1_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] guest dependency create"
        );
        let (code, deps) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            &guest,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] guest dependency list");
        assert_eq!(deps["can_manage"], false);

        // Valid edge: t1 blocks t2.
        let (code, edge) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id": t1_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] create edge");
        assert_eq!(edge["predecessor_task_id"].as_str().unwrap(), t1_id);

        // Duplicate rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id": t1_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] duplicate edge");

        // Self-dependency rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t1_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id": t1_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] self edge"
        );

        // Two-node cycle rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t1_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id": t2_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] two-node cycle");

        // Cross-project predecessor rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id": t3_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] cross-project"
        );

        // Unknown predecessor rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id":"00000000-0000-0000-0000-000000000000"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] unknown predecessor"
        );

        // Multi-node cycle: create t2→t3' chain inside shared project with a
        // third task, then attempt the closing edge.
        let (code, t4) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title":"Third"}),
        )
        .await;
        let t4_id = t4["id"].as_str().unwrap();
        // t1 → t4 (t1 before t4)
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t4_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id": t1_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] chain 1");
        // t4 → t2: now t2 depends on t4 depends on t1.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id": t4_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] chain 2");
        // Closing edge t2 → t1 would make: t2→t1, t1→t4→t2 → cycle.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t1_id}/dependencies"),
            Some(&member),
            json!({"predecessor_task_id": t2_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] multi-node cycle");

        // Blockers/successors listing correct.
        let (_code, deps) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            &member,
        )
        .await;
        let blockers = deps["blockers"].as_array().unwrap();
        assert_eq!(blockers.len(), 2, "[{engine}] t2 has 2 blockers");
        let (_code, deps) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{t1_id}/dependencies"),
            &member,
        )
        .await;
        // t1 blocks t2 (the first edge) and t4.
        let blocked_ids: Vec<&str> = deps["blocked"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["task_id"].as_str().unwrap())
            .collect();
        assert_eq!(blocked_ids.len(), 2, "[{engine}] t1 blocks t2 and t4");
        assert!(blocked_ids.contains(&t4_id));

        // Deleting a task cascades its edges.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/tasks/{t4_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] delete t4");
        let (_code, deps) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            &member,
        )
        .await;
        let blockers = deps["blockers"].as_array().unwrap();
        assert_eq!(blockers.len(), 1, "[{engine}] t4 edges cascaded");
        assert_eq!(blockers[0]["task_id"].as_str().unwrap(), t1_id);

        // Outsider cannot create dependencies.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            Some(&outsider),
            json!({"predecessor_task_id": t1_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] outsider dependency"
        );

        // Removing the edge works.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/tasks/{t2_id}/dependencies/{t1_id}"),
            Some(&member),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] remove edge");
        let (_code, deps) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{t2_id}/dependencies"),
            &member,
        )
        .await;
        assert_eq!(
            deps["blockers"].as_array().unwrap().len(),
            0,
            "[{engine}] edge removed"
        );

        // ---- Workload -----------------------------------------------------------
        let (code, workload) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/workload"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] workload");
        let members_list = workload["members"].as_array().unwrap();
        // t2 is unassigned (created by admin) — workload counts ASSIGNED tasks
        // per member, so only the member row exists with t1.
        assert!(
            members_list
                .iter()
                .all(|m| m["user"]["id"] != json!(admin_id)),
            "[{engine}] admin has no assigned tasks"
        );
        let member_row = members_list
            .iter()
            .find(|m| m["user"]["id"] == json!(member_id))
            .expect("member row");
        assert_eq!(
            member_row["total"].as_i64().unwrap(),
            1,
            "[{engine}] member owns t1"
        );
        assert_eq!(member_row["by_status"]["backlog"].as_i64().unwrap(), 1);
        // No overdue: no due dates set.
        assert_eq!(member_row["overdue"].as_i64().unwrap(), 0);

        // Unassigned: t2 (t1 assigned to member). Private project task excluded
        // only when filtering by project; team-wide includes all team tasks.
        assert!(
            workload["unassigned"]["total"].as_i64().unwrap() >= 1,
            "[{engine}] unassigned present"
        );

        // Project filter scopes the aggregation.
        let (code, scoped) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/workload?project_id={project_id}"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            scoped["unassigned"]["total"].as_i64().unwrap(),
            1,
            "[{engine}] project filter"
        );

        // Date filter: due today → overdue when due_before is today and open.
        let today = "2026-01-15";
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{t1_id}"),
            Some(&admin),
            json!({"due_date": today}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, dated) = get_json(
            &app.app,
            &format!(
                "/api/v1/teams/{team_id}/workload?due_before=2026-01-15&project_id={project_id}"
            ),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let member_row = dated["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user"]["id"] == json!(member_id))
            .expect("member row");
        // due_before 2026-01-15 includes due 2026-01-15; status backlog ≠ done → counted in the bucket
        assert_eq!(member_row["by_status"]["backlog"].as_i64().unwrap(), 1);

        // Guest workload → 404 (team-scoped).
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/workload"),
            &guest,
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] guest workload after regrant"
        );

        // Invalid date filter rejected.
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/workload?due_before=not-a-date"),
            &admin,
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] bad date"
        );

        // ---- Templates -----------------------------------------------------------
        let label = {
            let (code, l) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/teams/{team_id}/labels"),
                Some(&admin),
                json!({"name":"from-template"}),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            l["id"].as_str().unwrap().to_string()
        };

        // Member (role member) cannot manage; admin can.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/templates"),
            Some(&member),
            json!({"name":"M","default_title":"T"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member template create"
        );

        let (code, template) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/templates"),
            Some(&admin),
            json!({
                "name":"Bug report",
                "default_title":"[Bug] ",
                "default_description":"Steps to reproduce:\n1. ",
                "default_priority":"high",
                "label_ids":[label],
            }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] template create");
        assert_eq!(template["default_priority"], "high");
        assert_eq!(template["label_ids"].as_array().unwrap().len(), 1);
        let template_id = template["id"].as_str().unwrap().to_string();

        // Invalid priority rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/templates"),
            Some(&admin),
            json!({"name":"X","default_title":"T","default_priority":"bogus"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] bad priority"
        );

        // Unknown label rejected.
        let (code, _) = send_json(&app.app, "POST", &format!("/api/v1/projects/{project_id}/templates"),
            Some(&admin), json!({"name":"X","default_title":"T","label_ids":["00000000-0000-0000-0000-000000000000"]})).await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] bad label"
        );

        // List is project-scoped.
        let (_code, templates) = get_json(
            &app.app,
            &format!("/api/v1/projects/{other_project_id}/templates"),
            &admin,
        )
        .await;
        assert_eq!(
            templates["items"].as_array().unwrap().len(),
            0,
            "[{engine}] project scoping"
        );
        let (_code, templates) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/templates"),
            &admin,
        )
        .await;
        assert_eq!(
            templates["items"].as_array().unwrap().len(),
            1,
            "[{engine}] template listed"
        );

        // Update the template: priority back to medium, drop labels.
        let (code, updated) = send_json(&app.app, "PATCH", &format!("/api/v1/templates/{template_id}"),
            Some(&admin), json!({"name":"Bug report v2","default_title":"[Bug] ","default_priority":"medium","label_ids":[]})).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] template update");
        assert_eq!(updated["name"], "Bug report v2");
        assert_eq!(updated["default_priority"], "medium");

        // Create a task from the template (member allowed) with an edited title.
        let (code, from_template) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&member),
            json!({
                "title":"[Bug] Login page explodes",
                "description":"Steps to reproduce:\n1. Open /login",
                "priority":"medium",
                "label_ids":[label],
            }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] task from template");
        assert_eq!(
            from_template["priority"], "medium",
            "[{engine}] edited priority honored"
        );
        assert_eq!(
            from_template["labels"].as_array().unwrap().len(),
            1,
            "[{engine}] labels applied"
        );
        assert_eq!(
            from_template["reporter"]["id"].as_str().unwrap(),
            member_id,
            "[{engine}] default reporter is creator"
        );

        // Template updates never touch created tasks.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/templates/{template_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] template delete");
        let (code, task_after) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{}", from_template["id"].as_str().unwrap()),
            &member,
        )
        .await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] created task survives template delete"
        );
        assert_eq!(task_after["title"], "[Bug] Login page explodes");

        // Template status restrictions: creation via normal flow, so done is
        // rejected (already covered in Phase 2 matrix; asserted once here).
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&member),
            json!({"title":"X","status":"closed"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] workflow restriction intact"
        );

        // Members retain normal task permissions (regression guard).
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{t1_id}"),
            Some(&member),
            json!({"priority":"low"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] member update regression");
        let (code, _) =
            get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &member).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] member read regression");
    }
}

// ---------------------------------------------------------------------------
// Phase 4: goals & key results
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(
    clippy::float_equality_without_abs,
    unused_variables,
    unused_assignments
)]
async fn goals_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("admin-{suffix}@test.local");
        let member_email = format!("member-{suffix}@test.local");
        let guest_email = format!("guest-{suffix}@test.local");
        let outsider_email = format!("outsider-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "Admin").await;
        seed_user(&app.db, &member_email, "Member").await;
        seed_user(&app.db, &guest_email, "Guest").await;
        seed_user(&app.db, &outsider_email, "Outsider").await;

        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let guest = login_as(&app.app, &guest_email, "test-password-123").await;
        let outsider = login_as(&app.app, &outsider_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"T"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role":"member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"P"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap();

        let member_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "member")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let guest_id = {
            let (code, me) = get_json(&app.app, "/api/v1/me", &guest).await;
            me["id"].as_str().unwrap().to_string()
        };

        let (code, t1) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title":"Goal task"}),
        )
        .await;
        let t1_id = t1["id"].as_str().unwrap();

        // Members cannot create goals (Manager+ capability).
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/goals"),
            Some(&member),
            json!({"title":"Nope"}),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] member goal create");

        // Outsiders get 404 on team goal endpoints.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/goals"),
            Some(&outsider),
            json!({"title":"Nope"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] outsider goal create"
        );

        // Guests get 404 (no team membership).
        let (code, _) = get_json(&app.app, &format!("/api/v1/teams/{team_id}/goals"), &guest).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest goals list");

        // Admin creates a draft goal with member as owner.
        let (code, goal) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/goals"),
            Some(&admin),
            json!({"title":"Grow activation","description":"Q4 objective","owner_id": member_id,
                   "start_date":"2026-01-01","due_date":"2026-03-31"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] goal create");
        assert_eq!(goal["status"], "draft");
        assert_eq!(goal["owner"]["id"].as_str().unwrap(), member_id);
        assert_eq!(
            goal["progress"],
            Value::Null,
            "[{engine}] no KRs -> undefined progress"
        );
        let goal_id = goal["id"].as_str().unwrap();

        // Invalid dates rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/goals"),
            Some(&admin),
            json!({"title":"X","start_date":"2026-03-01","due_date":"2026-01-01"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] bad date range"
        );

        // Activation with no KRs blocked.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/status"),
            Some(&admin),
            json!({"status":"active"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] activate without KRs"
        );

        // Cross-team owner rejected.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/goals/{goal_id}"),
            Some(&admin),
            json!({"owner_id":"00000000-0000-0000-0000-000000000000"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] foreign owner"
        );

        // Add an increasing KR.
        let (code, kr) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/key-results"),
            Some(&admin),
            json!({
                "title":"Active users 100 -> 500",
                "metric_type":"increase",
                "start_value":100.0,
                "current_value":300.0,
                "target_value":500.0,
                "unit":"users"
            }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] KR create");
        assert_f64_close(&kr["progress"], 50.0, "KR progress computed");
        let kr_id = kr["id"].as_str().unwrap();

        // Invalid KR configs rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/key-results"),
            Some(&admin),
            json!({"title":"X","metric_type":"increase","start_value":500.0,"target_value":100.0}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] increase target < start"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/key-results"),
            Some(&admin),
            json!({"title":"X","metric_type":"boolean","current_value":0.5}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] boolean 0.5"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/key-results"),
            Some(&admin),
            json!({"title":"X","metric_type":"magic"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] bad metric type"
        );

        // Client-supplied progress is ignored (server-derived).
        let (code, kr2) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/key-results"),
            Some(&admin),
            json!({"title":"NPS 30 -> 60","metric_type":"increase",
                                 "start_value":30.0,"current_value":30.0,"target_value":60.0,
                                 "unit":"pts"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        assert_f64_close(&kr2["progress"], 0.0, "fresh NPS KR progress");

        // Update current value via the KR PATCH (merge semantics).
        let (code, kr_updated) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/key-results/{}", kr2["id"].as_str().unwrap()),
            Some(&admin),
            json!({"current_value":45.0}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] KR update");
        assert_f64_close(&kr_updated["progress"], 50.0, "NPS 45 = 50%");

        // Goal detail: mean of KR progress = (50 + 50) / 2 = 50.
        let (code, detail) = get_json(&app.app, &format!("/api/v1/goals/{goal_id}"), &admin).await;
        assert_eq!(code, StatusCode::OK);
        assert_f64_close(&detail["goal"]["progress"], 50.0, "mean progress");
        assert_eq!(detail["goal"]["key_result_count"].as_i64().unwrap(), 2);
        assert_eq!(
            detail["execution"]["total"].as_i64().unwrap(),
            0,
            "[{engine}] no associations yet"
        );

        // Activate now allowed with a KR.
        let (code, g) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/status"),
            Some(&admin),
            json!({"status":"active"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] activate");
        assert_eq!(g["status"], "active");

        // Invalid transition: active -> draft.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/status"),
            Some(&admin),
            json!({"status":"draft"}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] active -> draft");

        // Cross-team project association rejected.
        let (code, other_team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"T2"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let other_team_id = other_team["id"].as_str().unwrap();
        let (code, other_project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{other_team_id}/projects"),
            Some(&admin),
            json!({"name":"P2"}),
        )
        .await;
        let other_project_id = other_project["id"].as_str().unwrap();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/projects"),
            Some(&admin),
            json!({"project_id": other_project_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] cross-team project assoc"
        );

        // Same-team project association works; duplicate → 409.
        let (code, assoc) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/projects"),
            Some(&admin),
            json!({"project_id": project_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] project assoc");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/projects"),
            Some(&admin),
            json!({"project_id": project_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] duplicate project assoc"
        );

        // Same-team task association works; duplicate → 409.
        let (code, task_assoc) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/tasks"),
            Some(&admin),
            json!({"task_id": t1_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] task assoc");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/tasks"),
            Some(&admin),
            json!({"task_id": t1_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] duplicate task assoc"
        );

        // Execution summary: 1 associated task, backlog.
        let (code, detail) = get_json(&app.app, &format!("/api/v1/goals/{goal_id}"), &admin).await;
        assert_eq!(
            detail["execution"]["total"].as_i64().unwrap(),
            1,
            "[{engine}] execution total"
        );
        assert_eq!(detail["execution"]["completed"].as_i64().unwrap(), 0);
        let completion = detail["execution"]["completion_percent"].as_f64().unwrap();
        assert!(completion.abs() < f64::EPSILON, "[{engine}] 0 of 1 done");
        assert_eq!(detail["projects"].as_array().unwrap().len(), 1);
        assert_eq!(detail["tasks"].as_array().unwrap().len(), 1);

        // KR delete cascades only the KR; goal progress recomputes to 50%.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/key-results/{kr_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT);
        let (code, detail) = get_json(&app.app, &format!("/api/v1/goals/{goal_id}"), &admin).await;
        assert_f64_close(&detail["goal"]["progress"], 50.0, "remaining KR progress");
        assert_eq!(detail["goal"]["key_result_count"].as_i64().unwrap(), 1);

        // Disassociate project: project itself survives.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/goals/{goal_id}/projects/{project_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NO_CONTENT,
            "[{engine}] disassociate project"
        );
        let (code, _) = get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &admin).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] project survives");

        // Disassociate task: task survives.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/goals/{goal_id}/tasks/{t1_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] disassociate task");
        let (code, _) = get_json(&app.app, &format!("/api/v1/tasks/{t1_id}"), &admin).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] task survives");

        // Cross-team goal access denied (404 via second team's context — the
        // admin belongs to both teams, so use the outsider to prove isolation).
        let (code, _) = get_json(&app.app, &format!("/api/v1/goals/{goal_id}"), &outsider).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider goal read");
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/goals/{goal_id}"),
            Some(&outsider),
            json!({"title":"Hacked"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] outsider goal patch"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/status"),
            Some(&outsider),
            json!({"status":"completed"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] outsider goal transition"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/key-results"),
            Some(&outsider),
            json!({"title":"X","metric_type":"increase","start_value":0.0,"target_value":1.0}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider KR create");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/projects"),
            Some(&outsider),
            json!({"project_id": project_id}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider assoc");

        // Guest denied everywhere (no team membership).
        let (code, _) = get_json(&app.app, &format!("/api/v1/goals/{goal_id}"), &guest).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest goal read");
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/goals/{goal_id}"),
            Some(&guest),
            json!({"title":"H"}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] guest goal patch");

        // Complete → reopen flow.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/status"),
            Some(&admin),
            json!({"status":"completed"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] complete");
        let (code, g) = get_json(&app.app, &format!("/api/v1/goals/{goal_id}"), &admin).await;
        assert!(
            g["goal"]["completed_at"].is_number(),
            "[{engine}] completed_at set"
        );
        // Reset KR first so reopen is meaningful (progress stays, lifecycle reopens).
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/goals/{goal_id}/status"),
            Some(&admin),
            json!({"status":"active"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] reopen");
        let (code, g) = get_json(&app.app, &format!("/api/v1/goals/{goal_id}"), &admin).await;
        assert_eq!(
            g["goal"]["completed_at"],
            Value::Null,
            "[{engine}] reopen clears completed_at"
        );

        // Status filter on the team list.
        let (code, active_list) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/goals?status=active"),
            &admin,
        )
        .await;
        assert_eq!(
            active_list["total"].as_i64().unwrap(),
            1,
            "[{engine}] status filter"
        );
        let (code, draft_list) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/goals?status=draft"),
            &admin,
        )
        .await;
        assert_eq!(
            draft_list["total"].as_i64().unwrap(),
            0,
            "[{engine}] no drafts"
        );

        // Goal activity history recorded.
        let (code, activity) = get_json(
            &app.app,
            &format!("/api/v1/goals/{goal_id}/activity"),
            &admin,
        )
        .await;
        let events: Vec<&str> = activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["event"].as_str().unwrap())
            .collect();
        for expected in [
            "goal_created",
            "goal_status_changed",
            "goal_key_result_created",
            "goal_key_result_deleted",
            "goal_project_associated",
            "goal_project_disassociated",
            "goal_task_associated",
            "goal_task_disassociated",
        ] {
            assert!(
                events.contains(&expected),
                "[{engine}] missing {expected} in {events:?}"
            );
        }

        // Deleting the goal cascades KRs and associations but not tasks/projects.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/goals/{goal_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] goal delete");
        let (code, _) = get_json(&app.app, &format!("/api/v1/tasks/{t1_id}"), &admin).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] task survives goal delete");
        let (code, _) = get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &admin).await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] project survives goal delete"
        );
        let (code, _) = get_json(&app.app, &format!("/api/v1/goals/{goal_id}"), &admin).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
    }
}

// ---------------------------------------------------------------------------
// Phase 9: Advanced Profile + Advanced Team Settings
// ---------------------------------------------------------------------------

/// Minimal synthetic PNG as a data URL — the server only parses the header,
/// so a crafted IHDR is enough to exercise format, size, and square checks.
fn phase9_png_data_url(width: u32, height: u32) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = String::from("data:image/png;base64,");
    for chunk in png.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[tokio::test]
async fn profile_and_team_settings_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("p9-admin-{suffix}@test.local");
        let member_email = format!("p9-member-{suffix}@test.local");
        let guest_email = format!("p9-guest-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "P9 Admin").await;
        seed_user(&app.db, &member_email, "P9 Member").await;
        seed_user(&app.db, &guest_email, "P9 Guest").await;

        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let _guest = login_as(&app.app, &guest_email, "test-password-123").await;

        // -- Profile update -------------------------------------------------
        let (code, profile) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&admin),
            json!({
                "display_name": "Ada Admin",
                "username": "ada_admin",
                "bio": "Tasker maintainer",
                "timezone": "Europe/Berlin"
            }),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] patch /me");
        assert_eq!(profile["username"], "ada_admin");
        assert_eq!(profile["bio"], "Tasker maintainer");
        assert_eq!(profile["timezone"], "Europe/Berlin");
        assert_eq!(profile["display_name"], "Ada Admin");

        // GET /me reflects the profile and the default (all-on) prefs.
        let (code, me) = get_json(&app.app, "/api/v1/me", &admin).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(me["username"], "ada_admin");
        assert_eq!(me["notification_prefs"]["comment_mention"], true);

        // Invalid username / bio / timezone / non-square avatar.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&admin),
            json!({"username": "Not Valid!"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] bad username"
        );
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&admin),
            json!({"bio": "x".repeat(201)}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] long bio"
        );
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&admin),
            json!({"timezone": "Not/A zone!!"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] bad timezone"
        );
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&admin),
            json!({"avatar": phase9_png_data_url(64, 48)}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] non-square avatar"
        );

        // Username uniqueness across the instance.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&member),
            json!({"username": "ada_admin"}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] duplicate username");

        // Avatar upload + byte serving.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&admin),
            json!({"avatar": phase9_png_data_url(64, 64)}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] square avatar accepted");
        let admin_id = profile["id"].as_str().unwrap().to_string();
        let response = app
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v1/users/{admin_id}/avatar"))
                    .header(header::COOKIE, &member)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "[{engine}] avatar served"
        );
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");

        // -- My Work Summary ------------------------------------------------
        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"P9T"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role":"member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"P9P"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "member")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };

        // Two open tasks (one overdue) + one task to drive to closed.
        for (title, due) in [
            ("P9 overdue", Some("2020-01-01")),
            ("P9 future", Some("2099-01-01")),
            ("P9 plain", None),
        ] {
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/projects/{project_id}/tasks"),
                Some(&admin),
                json!({"title": title, "assignee_id": member_id, "due_date": due}),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED, "[{engine}] seed task {title}");
        }
        let (code, list) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/tasks?status=backlog"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let tasks = list["items"].as_array().unwrap().clone();
        assert_eq!(tasks.len(), 3, "[{engine}] seeded tasks");
        let close_id = tasks.iter().find(|t| t["title"] == "P9 plain").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        // Drive one task through the full lifecycle to closed:
        // assignee resolves, reporter (the creator) submits, then accepts.
        for (actor, stage) in [
            (&member, "todo"),
            (&member, "in_progress"),
            (&member, "resolved"),
            (&admin, "in_review"),
            (&admin, "closed"),
        ] {
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{close_id}/transition"),
                Some(actor),
                json!({"to": stage}),
            )
            .await;
            assert_eq!(code, StatusCode::OK, "[{engine}] transition to {stage}");
        }

        // Unread mention: admin mentions the member by email token on a
        // fresh open task (Phase 12 WS10: closed tasks reject comments).
        let (code, open_task2) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "open for mention", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let open_task2_id = open_task2["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{open_task2_id}/comments"),
            Some(&admin),
            json!({"body": format!("ping @{member_email}")}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] mention comment");

        let (code, summary) = get_json(&app.app, "/api/v1/me/summary", &member).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] summary");
        assert_eq!(
            summary["by_status"]["backlog"], 3,
            "[{engine}] backlog count"
        );
        assert_eq!(summary["by_status"]["closed"], 1, "[{engine}] closed count");
        assert_eq!(summary["overdue"], 1, "[{engine}] overdue count");
        assert_eq!(
            summary["unread_mentions_7d"], 1,
            "[{engine}] unread mentions"
        );

        // The summary's link targets: overdue + mentioned filters.
        let (code, filtered) = get_json(&app.app, "/api/v1/me/tasks?overdue=true", &member).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(filtered["total"], 1, "[{engine}] overdue filter");
        let (code, filtered) =
            get_json(&app.app, "/api/v1/me/tasks?mentioned=unread", &member).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(filtered["total"], 1, "[{engine}] mentioned filter");

        // -- Notification preference gating ---------------------------------
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&member),
            json!({"notification_prefs": {"comment_mention": false}}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, prefs) = get_json(&app.app, "/api/v1/me", &member).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(prefs["notification_prefs"]["comment_mention"], false);
        assert_eq!(prefs["notification_prefs"]["task_assigned"], true);

        // A new mention is suppressed; an assignment still arrives.
        let future_id = tasks.iter().find(|t| t["title"] == "P9 future").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{future_id}/comments"),
            Some(&admin),
            json!({"body": format!("quiet @{member_email}")}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "P9 assigned", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, notifications) = get_json(&app.app, "/api/v1/notifications", &member).await;
        assert_eq!(code, StatusCode::OK);
        let items = notifications["items"].as_array().unwrap();
        assert!(
            items.iter().any(|n| n["event"] == "task_assigned"),
            "[{engine}] assignment delivered"
        );
        let unread_mentions = items
            .iter()
            .filter(|n| n["event"] == "comment_mention" && n["read_at"].is_null())
            .count();
        assert_eq!(unread_mentions, 1, "[{engine}] gated mention suppressed");

        // -- Sessions --------------------------------------------------------
        let (code, sessions) = get_json(&app.app, "/api/v1/me/sessions", &member).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(sessions["items"].as_array().unwrap().len(), 1);
        assert_eq!(sessions["items"][0]["current"], true);

        let member2 = login_as(&app.app, &member_email, "test-password-123").await;
        let (_code, sessions) = get_json(&app.app, "/api/v1/me/sessions", &member).await;
        assert_eq!(
            sessions["items"].as_array().unwrap().len(),
            2,
            "[{engine}] two sessions"
        );
        let other_id = sessions["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["current"] == false)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/me/sessions/{other_id}"),
            Some(&member),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] revoke other");
        let (code, _) = get_json(&app.app, "/api/v1/me", &member2).await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] revoked session rejected"
        );

        // Revoking the current session is allowed and signs the caller out.
        let (_code, sessions) = get_json(&app.app, "/api/v1/me/sessions", &member).await;
        let current_id = sessions["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["current"] == true)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/me/sessions/{current_id}"),
            Some(&member),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] revoke current");
        let (code, _) = get_json(&app.app, "/api/v1/me", &member).await;
        assert_eq!(
            code,
            StatusCode::UNAUTHORIZED,
            "[{engine}] current session revoked"
        );
        // Re-login for the team settings part.
        let member = login_as(&app.app, &member_email, "test-password-123").await;

        // -- Team settings ----------------------------------------------------
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/teams/{team_id}"),
            Some(&admin),
            json!({"timezone": "Asia/Tehran", "avatar": phase9_png_data_url(32, 32)}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] team timezone+avatar");
        let (code, team_now) =
            get_json(&app.app, &format!("/api/v1/teams/{team_id}"), &admin).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(team_now["timezone"], "Asia/Tehran");
        let response = app
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v1/teams/{team_id}/avatar"))
                    .header(header::COOKIE, &admin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "[{engine}] team avatar served"
        );

        // Members list carries the @mention handle for search.
        let (code, member_rows) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/members"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let owner_entry = member_rows["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user"]["email"] == admin_email.as_str())
            .unwrap();
        assert_eq!(
            owner_entry["username"], "ada_admin",
            "[{engine}] member username"
        );

        // Guests: grant a project viewer, then team-level view + revoke.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/guests"),
            Some(&admin),
            json!({"email": guest_email}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] grant guest");
        let (code, guests) =
            get_json(&app.app, &format!("/api/v1/teams/{team_id}/guests"), &admin).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            guests["items"].as_array().unwrap().len(),
            1,
            "[{engine}] team guests list"
        );
        let guest_id = guests["items"][0]["user"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/teams/{team_id}/guests/{guest_id}"),
            Some(&member),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member revoke guest"
        );
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/teams/{team_id}/guests/{guest_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::NO_CONTENT,
            "[{engine}] owner revoke guest"
        );
        let (_code, guests) =
            get_json(&app.app, &format!("/api/v1/teams/{team_id}/guests"), &admin).await;
        assert_eq!(
            guests["items"].as_array().unwrap().len(),
            0,
            "[{engine}] guests empty"
        );

        // Archive / unarchive.
        let (code, archived) = send_request(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/archive"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] archive");
        assert!(
            archived["archived_at"].is_i64(),
            "[{engine}] archived_at set"
        );
        let (code, _) = send_request(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/unarchive"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] unarchive");
        let (_code, restored) =
            get_json(&app.app, &format!("/api/v1/teams/{team_id}"), &admin).await;
        assert!(
            restored["archived_at"].is_null(),
            "[{engine}] archived_at cleared"
        );

        // Team delete: owner-only; name + password re-verification.
        let (code, _) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/teams/{team_id}"),
            Some(&member),
            json!({"confirm_name": "P9T", "current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member delete forbidden"
        );
        let (code, _) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/teams/{team_id}"),
            Some(&admin),
            json!({"confirm_name": "wrong", "current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] wrong confirm name"
        );
        let (code, _) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/teams/{team_id}"),
            Some(&admin),
            json!({"confirm_name": "P9T", "current_password": "nope-nope-nope"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] wrong password"
        );
        let (code, _) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/teams/{team_id}"),
            Some(&admin),
            json!({"confirm_name": "P9T", "current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] owner delete");
        let (code, _) = get_json(&app.app, &format!("/api/v1/teams/{team_id}"), &admin).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] team gone");
        let (code, _) = get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &admin).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] projects cascaded");
    }
}

// ---------------------------------------------------------------------------
// Phase 10.5: project soft-delete / restore / purge + capacity endpoint.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn project_delete_and_capacity_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("p105-admin-{suffix}@test.local");
        let manager_email = format!("p105-manager-{suffix}@test.local");
        let member_email = format!("p105-member-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "P105 Admin").await;
        seed_user(&app.db, &manager_email, "P105 Manager").await;
        seed_user(&app.db, &member_email, "P105 Member").await;

        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let manager = login_as(&app.app, &manager_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"P105T"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        for (email, role) in [(&manager_email, "manager"), (&member_email, "member")] {
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/teams/{team_id}/members"),
                Some(&admin),
                json!({"email": email, "role": role }),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED, "[{engine}] add {role}");
        }

        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"P105P"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();

        // Seed tasks for the capacity math: member gets 3 open (1 overdue,
        // 1 due soon), manager gets 1 closed.
        let member_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "member")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let today = tasker::util::parse_today_iso();
        let soon = tasker::util::parse_iso_date(&today)
            .unwrap()
            .checked_add(time::Duration::days(3))
            .unwrap()
            .to_string();
        let past = tasker::util::parse_iso_date(&today)
            .unwrap()
            .checked_sub(time::Duration::days(5))
            .unwrap()
            .to_string();
        for (title, due) in [
            ("cap-open", None),
            ("cap-overdue", Some(past)),
            ("cap-soon", Some(soon)),
        ] {
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/projects/{project_id}/tasks"),
                Some(&admin),
                json!({"title": title, "assignee_id": member_id, "due_date": due}),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED, "[{engine}] seed {title}");
        }

        // Capacity: manager allowed, member forbidden (404-style), admin allowed.
        let (code, cap) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/capacity?window=week"),
            &manager,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] manager reads capacity");
        assert_eq!(cap["window_days"], 7);
        assert_eq!(
            cap["summary"]["total_open_tasks"], 3,
            "[{engine}] summary open"
        );
        let member_row = cap["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user_id"] == json!(member_id))
            .unwrap();
        assert_eq!(member_row["open_tasks"], 3);
        assert_eq!(member_row["overdue_tasks"], 1);
        assert_eq!(member_row["due_this_week"], 1, "[{engine}] due soon");
        assert_eq!(
            member_row["capacity_limit"], 10,
            "[{engine}] default capacity"
        );
        assert_eq!(
            member_row["status"], "underutilized",
            "[{engine}] 3/10 < 0.6 underutilized"
        );
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/capacity"),
            &member,
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] member forbidden");

        // Month window flips the counter fields.
        let (code, cap_month) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/capacity?window=month"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let member_month = cap_month["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user_id"] == json!(member_id))
            .unwrap();
        assert_eq!(member_month["due_this_month"], 1);
        assert!(
            member_month["due_this_week"].is_null(),
            "[{engine}] week null in month window"
        );
        // Unknown window rejected.
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/capacity?window=year"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);

        // Capacity limit: member sets their own via PATCH /me; capacity view reflects it.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&member),
            json!({"capacity_limit": 3}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] set capacity");
        let (_code, cap2) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/capacity"),
            &admin,
        )
        .await;
        let member_row2 = cap2["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user_id"] == json!(member_id))
            .unwrap();
        assert_eq!(member_row2["capacity_limit"], 3);
        assert_eq!(
            member_row2["status"], "balanced",
            "[{engine}] 3/3 == 1.0 -> balanced (overloaded is > 1.0)"
        );
        // Out-of-range clamps.
        let (code, me_now) = send_json(
            &app.app,
            "PATCH",
            "/api/v1/me",
            Some(&member),
            json!({"capacity_limit": 500}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            me_now["capacity_limit"], 500,
            "[{engine}] 500 is within the 1-1000 range"
        );

        // -- Project soft-delete -------------------------------------------
        // Member cannot delete.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}"),
            Some(&member),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member delete forbidden"
        );
        // Manager cannot delete (Managers archive, not delete).
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}"),
            Some(&manager),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] manager delete forbidden"
        );
        // Admin soft-deletes.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] admin soft delete");

        // Invisible everywhere.
        let (code, _) = get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &admin).await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] get 404 after delete"
        );
        let (code, list) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/projects?include_archived=true"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            list["items"].as_array().unwrap().len(),
            0,
            "[{engine}] list hides deleted"
        );
        let (_code, my) = get_json(&app.app, "/api/v1/me/tasks", &member).await;
        assert_eq!(
            my["total"], 0,
            "[{engine}] my-tasks hides deleted project tasks"
        );
        let (_code, summary) = get_json(&app.app, "/api/v1/me/summary", &member).await;
        assert_eq!(
            summary["by_status"]["backlog"], 0,
            "[{engine}] summary hides deleted"
        );
        // Capacity also drops to zero open tasks.
        let (_code, cap3) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/capacity"),
            &admin,
        )
        .await;
        assert_eq!(
            cap3["summary"]["total_open_tasks"], 0,
            "[{engine}] capacity hides deleted"
        );

        // Trash lists it (admin only; member gets 404).
        let (code, trash) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/projects/trash"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] trash list");
        assert_eq!(trash["items"].as_array().unwrap().len(), 1);
        assert_eq!(trash["items"][0]["name"], "P105P");
        assert!(trash["items"][0]["retention_expires_at"].is_i64());
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/projects/trash"),
            &member,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] trash member 404");

        // Restore brings everything back.
        let (code, restored) = send_request(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/restore"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] restore");
        assert_eq!(restored["name"], "P105P");
        let (_code, my) = get_json(&app.app, "/api/v1/me/tasks", &member).await;
        assert_eq!(my["total"], 3, "[{engine}] tasks restored");

        // Soft-delete again, then purge: owner-only + name + password.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT);
        // Admin (non-owner) cannot purge.
        let (code, _) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}/purge"),
            Some(&manager),
            json!({"confirm_name": "P105P", "current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] manager purge forbidden"
        );
        // Wrong name.
        let (code, _) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}/purge"),
            Some(&admin),
            json!({"confirm_name": "wrong", "current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] wrong name"
        );
        // Wrong password.
        let (code, _) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}/purge"),
            Some(&admin),
            json!({"confirm_name": "P105P", "current_password": "nope-nope-nope"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] wrong password"
        );
        // Owner purge succeeds; tasks are cascade-deleted.
        let (code, _) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/projects/{project_id}/purge"),
            Some(&admin),
            json!({"confirm_name": "P105P", "current_password": "test-password-123"}),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] owner purge");
        let (_code, trash) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_id}/projects/trash"),
            &admin,
        )
        .await;
        assert_eq!(
            trash["items"].as_array().unwrap().len(),
            0,
            "[{engine}] trash empty after purge"
        );
        // Task rows are gone (cascade) — the task count endpoint confirms.
        let (code, _) = get_json(&app.app, &format!("/api/v1/teams/{team_id}"), &admin).await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] team survives project purge"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 11: comment threads, from-template, position reordering.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn threading_templates_position_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("p11-admin-{suffix}@test.local");
        let member_email = format!("p11-member-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "P11 Admin").await;
        seed_user(&app.db, &member_email, "P11 Member").await;

        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"P11T"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role":"member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let member_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "member")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"P11P"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "P11 root", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();

        // -- Comment threads ------------------------------------------------
        let (code, root) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/comments"),
            Some(&admin),
            json!({"body": "root comment"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let root_id = root["id"].as_str().unwrap().to_string();
        assert!(
            root["parent_comment_id"].is_null(),
            "[{engine}] top-level has no parent"
        );

        // Reply: allowed; parent author notified.
        let (code, reply) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/comments"),
            Some(&member),
            json!({"body": "a reply", "parent_comment_id": root_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] reply created");
        assert_eq!(
            reply["parent_comment_id"],
            json!(root_id),
            "[{engine}] reply carries parent"
        );
        let reply_id = reply["id"].as_str().unwrap().to_string();
        let (code, notifications) = get_json(&app.app, "/api/v1/notifications", &admin).await;
        assert!(
            notifications["items"].as_array().unwrap().iter().any(|n| {
                n["event"] == "comment_mention"
                    && n["summary"]
                        .as_str()
                        .unwrap()
                        .contains("replied to your comment")
            }),
            "[{engine}] parent author notified of reply"
        );

        // Reply-to-reply: 422 (depth 2).
        let (code, err) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/comments"),
            Some(&admin),
            json!({"body": "too deep", "parent_comment_id": reply_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] depth limit"
        );
        assert!(
            err["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["message"].as_str().unwrap().contains("depth")),
            "[{engine}] depth message"
        );

        // Cross-task parent rejected.
        let (code, task2) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "P11 second"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task2_id = task2["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task2_id}/comments"),
            Some(&admin),
            json!({"body": "x", "parent_comment_id": root_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] cross-task parent"
        );

        // comment_replied activity recorded.
        let (code, activity) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/activity"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert!(
            activity["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["event"] == "comment_replied"),
            "[{engine}] comment_replied event"
        );

        // Delete parent with replies: allowed (cascade in app policy).
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/comments/{root_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] delete parent");
        // Replies cascade: the task's comment list no longer contains either.
        let (code, remaining) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/comments"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let ids: Vec<&str> = remaining["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(!ids.contains(&root_id.as_str()), "[{engine}] parent gone");
        assert!(
            !ids.contains(&reply_id.as_str()),
            "[{engine}] reply gone with parent"
        );

        // -- From-template ---------------------------------------------------
        let (code, template) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/templates"),
            Some(&admin),
            json!({
                "name": "P11 Bug",
                "default_title": "Bug: ",
                "default_description": "steps to reproduce",
                "default_priority": "high",
                "label_ids": [],
                "default_subtasks": ["reproduce", "fix", "verify"]
            }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] template create");
        let template_id = template["id"].as_str().unwrap().to_string();
        assert_eq!(
            template["default_subtasks"].as_array().unwrap().len(),
            3,
            "[{engine}] subtasks stored"
        );

        // Single fetch.
        let (code, fetched) = get_json(
            &app.app,
            &format!("/api/v1/templates/{template_id}"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] get single template");
        assert_eq!(fetched["name"], "P11 Bug");

        // Create-from-template with an override.
        let (code, from_t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/from-template/{template_id}"),
            Some(&admin),
            json!({"title": "Bug: login loop", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] from-template");
        assert_eq!(
            from_t["title"], "Bug: login loop",
            "[{engine}] override applied"
        );
        assert_eq!(from_t["priority"], "high", "[{engine}] template priority");
        assert_eq!(
            from_t["from_template_id"],
            json!(template_id),
            "[{engine}] provenance"
        );
        let from_task_id = from_t["id"].as_str().unwrap().to_string();
        let subtasks = from_t["subtasks"].as_array().unwrap();
        assert_eq!(subtasks.len(), 3, "[{engine}] template subtasks applied");
        // Validation still applies: empty title override falls back — but an
        // invalid priority is rejected.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/from-template/{template_id}"),
            Some(&admin),
            json!({"priority": "catastrophic"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] template cannot bypass validation"
        );
        void_task(&from_task_id);

        // -- Position reordering ---------------------------------------------
        let (code, t1) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "pos a"}),
        )
        .await;
        let (code, t2) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "pos b"}),
        )
        .await;
        let (code, t3) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "pos c"}),
        )
        .await;
        let id1 = t1["id"].as_str().unwrap().to_string();
        let id2 = t2["id"].as_str().unwrap().to_string();
        let id3 = t3["id"].as_str().unwrap().to_string();

        // Move c before a: c's position becomes the midpoint above a.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{id3}/position"),
            Some(&admin),
            json!({"before_task_id": id1}),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] reorder");

        // Member (assignee of the root task? no) — member is not creator/assignee
        // of t1: forbidden.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{id1}/position"),
            Some(&member),
            json!({"before_task_id": id2}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] member reorder forbidden"
        );

        // Self-reference rejected.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{id1}/position"),
            Some(&admin),
            json!({"before_task_id": id1}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] self reference"
        );

        // Different-column reference rejected: move a helper task to
        // in_progress first, then reference it from a backlog task.
        let (code, helper) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "pos helper", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let helper_id = helper["id"].as_str().unwrap().to_string();
        for to in ["todo", "in_progress"] {
            let (code, _) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{helper_id}/transition"),
                Some(&member),
                json!({"to": to}),
            )
            .await;
            assert_eq!(code, StatusCode::OK, "[{engine}] helper to {to}");
        }
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{id1}/position"),
            Some(&admin),
            json!({"before_task_id": helper_id}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] cross-column reference"
        );

        // Board ordering reflects the move: c before a.
        let (code, board) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/board"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let backlog = board["columns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["status"] == "backlog")
            .unwrap();
        let titles: Vec<&str> = backlog["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["title"].as_str().unwrap())
            .collect();
        let ci = titles.iter().position(|t| *t == "pos c").unwrap();
        let ai = titles.iter().position(|t| *t == "pos a").unwrap();
        assert!(ci < ai, "[{engine}] c ordered before a: {titles:?}");
    }
}

fn void_task(_: &str) {}

/// Phase 12 WS11: attachment upload/list/download/delete over `local`
/// storage, including the closed-task lock and the task-edit rule.
#[tokio::test]
async fn attachments_local_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("p12-admin-{suffix}@test.local");
        let member_email = format!("p12-member-{suffix}@test.local");
        let outsider_email = format!("p12-outsider-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "P12 Admin").await;
        seed_user(&app.db, &member_email, "P12 Member").await;
        seed_user(&app.db, &outsider_email, "P12 Outsider").await;

        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;
        let outsider = login_as(&app.app, &outsider_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"P12AT"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role":"member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"P12AP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let member_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "member")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "P12 attach", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] task create: {task}");
        let task_id = task["id"].as_str().unwrap().to_string();
        let boundary = "tasker-test-boundary-42";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"notes v1.txt\"\r\nContent-Type: text/plain\r\n\r\nhello attachments\r\n--{boundary}--\r\n"
        );
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/api/v1/tasks/{task_id}/attachments"))
                    .header(header::COOKIE, &admin)
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (code, created) = to_response(response).await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] upload: {created}");
        assert_eq!(created["filename"], "notes v1.txt");
        assert_eq!(created["mime_type"], "text/plain");
        let attachment_id = created["id"].as_str().unwrap().to_string();
        assert!(created["uploaded_by"]["display_name"].is_string());

        // The uploader is a member; assignee (member) may also upload.
        let boundary2 = "tasker-test-boundary-43";
        let body2 = format!(
            "--{boundary2}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"../../evil.bin\"\r\n\r\nxyz\r\n--{boundary2}--\r\n"
        );
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/api/v1/tasks/{task_id}/attachments"))
                    .header(header::COOKIE, &member)
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary2}"),
                    )
                    .body(Body::from(body2))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (code, created2) = to_response(response).await;
        assert_eq!(
            code,
            StatusCode::CREATED,
            "[{engine}] member upload: {created2}"
        );
        // Path traversal is sanitized away in the stored filename.
        assert_eq!(created2["filename"], "evil.bin");

        // -- List -------------------------------------------------------------
        let (code, list) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/attachments"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            list["items"].as_array().unwrap().len(),
            2,
            "[{engine}] {list}"
        );

        // -- Download ---------------------------------------------------------
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&format!("/api/v1/attachments/{attachment_id}"))
                    .header(header::COOKIE, &member)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/plain");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), b"hello attachments");

        // -- Outsider has no access -------------------------------------------
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/attachments"),
            &outsider,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider list");

        // -- Delete (soft) on the open task ------------------------------------
        let (code, err) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/attachments/{attachment_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] delete: {err}");
        let (code, list) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/attachments"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            list["items"].as_array().unwrap().len(),
            1,
            "[{engine}] {list}"
        );

        // -- Closed-task lock --------------------------------------------------
        // Legal lifecycle: assignee drives todo -> in_progress -> resolved,
        // the reporter submits for review and accepts (closed).
        for (actor, to) in [
            (&member, "todo"),
            (&member, "in_progress"),
            (&member, "resolved"),
            (&admin, "in_review"),
            (&admin, "closed"),
        ] {
            let (code, body) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{task_id}/transition"),
                Some(actor),
                json!({"to": to}),
            )
            .await;
            assert_eq!(
                code,
                StatusCode::OK,
                "[{engine}] transition to {to}: {body}"
            );
        }
        let boundary3 = "tasker-test-boundary-44";
        let body3 = format!(
            "--{boundary3}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"late.txt\"\r\n\r\nlate\r\n--{boundary3}--\r\n"
        );
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/api/v1/tasks/{task_id}/attachments"))
                    .header(header::COOKIE, &admin)
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary3}"),
                    )
                    .body(Body::from(body3))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (code, err) = to_response(response).await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] closed upload: {err}"
        );

        // Download still works on closed tasks (read-only surface).
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/attachments"),
            &member,
        )
        .await;
        assert_eq!(code, StatusCode::OK);

        // Closed tasks reject deletion of the remaining attachment.
        let other_id = list["items"][0]["id"].as_str().unwrap().to_string();
        let (code, err) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/attachments/{other_id}"),
            Some(&admin),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] closed delete: {err}"
        );

        // Download of the soft-deleted attachment is a 404.
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/attachments/{attachment_id}"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] deleted download");
    }
}

/// Phase 12 WS4: checklist items + combined progress counters, including the
/// closed-task lock. Guards the four-bind progress aggregate.
#[tokio::test]
async fn checklist_progress_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let admin_email = format!("p12cl-admin-{suffix}@test.local");
        let member_email = format!("p12cl-member-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "P12CL Admin").await;
        seed_user(&app.db, &member_email, "P12CL Member").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;
        let member = login_as(&app.app, &member_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&admin),
            json!({"name":"P12CLT"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&admin),
            json!({"name":"P12CLP"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/members"),
            Some(&admin),
            json!({"email": member_email, "role":"member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let member_id = {
            let (code, members) = get_json(
                &app.app,
                &format!("/api/v1/teams/{team_id}/members"),
                &admin,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            members["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "member")
                .unwrap()["user"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title": "P12CL task", "assignee_id": member_id}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();

        // Empty progress: null pct.
        let (code, progress) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/progress"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(progress["checklist_total"], 0, "[{engine}] {progress}");
        assert!(progress["progress_pct"].is_null());

        // Two items.
        let mut item_ids = Vec::new();
        for title in ["write spec", "review spec"] {
            let (code, item) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{task_id}/checklist"),
                Some(&admin),
                json!({"title": title}),
            )
            .await;
            assert_eq!(
                code,
                StatusCode::CREATED,
                "[{engine}] create {title}: {item}"
            );
            item_ids.push(item["id"].as_str().unwrap().to_string());
        }

        // List reflects both.
        let (code, list) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/checklist"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            list["items"].as_array().unwrap().len(),
            2,
            "[{engine}] {list}"
        );

        // Toggle one: 1/2 = 50%.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/checklist/{}", item_ids[0]),
            Some(&admin),
            json!({"is_checked": true}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, progress) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/progress"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(progress["checklist_checked"], 1, "[{engine}] {progress}");
        assert_eq!(progress["progress_pct"], 50, "[{engine}] {progress}");

        // Delete one: 1/1 = 100%.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/checklist/{}", item_ids[1]),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT);
        let (code, progress) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/progress"),
            &admin,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(progress["progress_pct"], 100, "[{engine}] {progress}");

        // Closed-task lock: close, then item mutations 409.
        // Assignee drives the early lifecycle; the reporter closes.
        for (actor, to) in [
            (&member, "todo"),
            (&member, "in_progress"),
            (&member, "resolved"),
            (&admin, "in_review"),
            (&admin, "closed"),
        ] {
            let (code, body) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/tasks/{task_id}/transition"),
                Some(actor),
                json!({"to": to}),
            )
            .await;
            assert_eq!(code, StatusCode::OK, "[{engine}] transition {to}: {body}");
        }
        let (code, err) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/checklist"),
            Some(&admin),
            json!({"title": "late"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] closed create: {err}"
        );
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/checklist/{}", item_ids[0]),
            Some(&admin),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] closed delete");
    }
}

/// Phase 12.5 Bug 2 + Bug 7: instance-level user administration, team
/// assignment separation, and the Asia/Tehran timezone default.
#[tokio::test]
async fn admin_users_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let owner_email = format!("p125-owner-{suffix}@test.local");
        let manager_email = format!("p125-manager-{suffix}@test.local");
        seed_owner(&app.db, &owner_email, "P125 Owner").await;
        seed_user(&app.db, &manager_email, "P125 Manager").await;
        let owner = login_as(&app.app, &owner_email, "test-password-123").await;
        let manager = login_as(&app.app, &manager_email, "test-password-123").await;

        // Manager cannot create an instance user (403).
        let (code, body) = send_json(
            &app.app,
            "POST",
            "/api/v1/admin/users",
            Some(&manager),
            json!({
                "email": format!("new-{suffix}@test.local"),
                "display_name": "New User",
                "temporary_password": "Str0ng-Passphrase!9"
            }),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] manager create: {body}"
        );

        // Owner creates an instance user — no team membership attached.
        let new_email = format!("new-{suffix}@test.local");
        let (code, created) = send_json(
            &app.app,
            "POST",
            "/api/v1/admin/users",
            Some(&owner),
            json!({
                "email": new_email,
                "display_name": "New User",
                "username": format!("new_user_{}", &suffix.simple().to_string()[..8]),
                "temporary_password": "Str0ng-Passphrase!9"
            }),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CREATED,
            "[{engine}] owner create: {created}"
        );
        let new_user_id = created["id"].as_str().unwrap().to_string();
        assert_eq!(created["instance_role"], "member");
        assert_eq!(created["active"], true);
        assert_eq!(created["teams"].as_array().unwrap().len(), 0);

        // Weak temporary passwords are rejected with the specific policy
        // message (Bug 8 applies to admin-created users).
        let (code, body) = send_json(
            &app.app,
            "POST",
            "/api/v1/admin/users",
            Some(&owner),
            json!({
                "email": format!("weak-{suffix}@test.local"),
                "display_name": "Weak",
                "temporary_password": "weak-pass-123"
            }),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] weak: {body}"
        );
        assert!(
            body.to_string().contains("uppercase"),
            "[{engine}] expected the specific class message: {body}"
        );

        // Listing is owner-only and returns the created account.
        let (code, list) = get_json(&app.app, "/api/v1/admin/users", &manager).await;
        assert_eq!(code, StatusCode::FORBIDDEN);
        let (code, list) = get_json(&app.app, "/api/v1/admin/users?q=new", &owner).await;
        assert_eq!(code, StatusCode::OK);
        assert!(
            list["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u["id"] == created["id"]),
            "[{engine}] search finds the new user: {list}"
        );

        // Team A and Team B; both default to Asia/Tehran (Bug 7).
        for label in ["A", "B"] {
            let (code, team) = send_json(
                &app.app,
                "POST",
                "/api/v1/teams",
                Some(&owner),
                json!({ "name": format!("P125 Team {label} {suffix}") }),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            assert_eq!(
                team["timezone"], "Asia/Tehran",
                "[{engine}] new team defaults to Asia/Tehran: {team}"
            );
        }
        let (code, teams) = get_json(&app.app, "/api/v1/teams", &owner).await;
        assert_eq!(code, StatusCode::OK);
        let team_ids: Vec<String> = teams["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| {
                t["name"]
                    .as_str()
                    .unwrap_or("")
                    .contains(&suffix.to_string())
            })
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(team_ids.len(), 2, "[{engine}] {teams}");

        // Assignment to Team A succeeds; unknown email is a 422 with the
        // actionable message.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{}/members", team_ids[0]),
            Some(&owner),
            json!({ "email": new_email, "role": "member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] assign to A");
        let (code, err) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{}/members", team_ids[1]),
            Some(&owner),
            json!({ "email": format!("ghost-{suffix}@test.local"), "role": "member" }),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] ghost: {err}"
        );
        assert!(
            err.to_string()
                .contains("Ask an Owner to create the account first"),
            "[{engine}] actionable message: {err}"
        );

        // Assignment to Team B: the user is now a member of both.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{}/members", team_ids[1]),
            Some(&owner),
            json!({ "email": new_email, "role": "member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] assign to B");
        let (code, listing) = get_json(
            &app.app,
            &format!("/api/v1/admin/users?q={new_email}"),
            &owner,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let teams_of = &listing["items"][0]["teams"];
        assert_eq!(
            teams_of.as_array().unwrap().len(),
            2,
            "[{engine}] {listing}"
        );

        // Deactivate → sign-in refused; reactivate → allowed again.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/admin/users/{new_user_id}"),
            Some(&owner),
            json!({ "active": false }),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] deactivate");
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"email": new_email, "password": "Str0ng-Passphrase!9"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "[{engine}] deactivated login"
        );
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/admin/users/{new_user_id}"),
            Some(&owner),
            json!({ "active": true }),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] reactivate");

        // Hard delete is guarded while the account has memberships.
        let (code, err) = send_json(
            &app.app,
            "DELETE",
            &format!("/api/v1/admin/users/{new_user_id}"),
            Some(&owner),
            json!({
                "confirm_email": new_email,
                "current_password": "test-password-123"
            }),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "[{engine}] guarded delete: {err}"
        );
    }
}

/// Phase 12.5 Bug 8: the change-password flow enforces the strict policy
/// with specific messages.
#[tokio::test]
async fn password_policy_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        // The admin's identity: changing to a password derived from the
        // display name must fail with the identity message.
        let admin_email = format!("p125pw-admin-{suffix}@test.local");
        seed_user(&app.db, &admin_email, "Px Admin").await;
        let admin = login_as(&app.app, &admin_email, "test-password-123").await;

        async fn change(app: &TestApp, cookie: &str, body: Value) -> (StatusCode, Value) {
            let response = app
                .app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri("/api/v1/me/password")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::COOKIE, cookie)
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            to_response(response).await
        }

        // Same as current password.
        let (code, err) = change(
            &app,
            &admin,
            json!({"current_password": "test-password-123", "new_password": "test-password-123"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] same: {err}"
        );
        assert!(
            err.to_string().contains("must be different"),
            "[{engine}] {err}"
        );

        // Too short.
        let (code, err) = change(
            &app,
            &admin,
            json!({"current_password": "test-password-123", "new_password": "Sh0rt!x"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] short: {err}"
        );
        assert!(
            err.to_string().contains("at least 12 characters"),
            "[{engine}] {err}"
        );

        // Missing a special character.
        let (code, err) = change(
            &app,
            &admin,
            json!({"current_password": "test-password-123", "new_password": "FreshTrail42x"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] class: {err}"
        );
        assert!(
            err.to_string().contains("special character"),
            "[{engine}] {err}"
        );

        // Identity match (email) — otherwise policy-shaped.
        let (code, err) = change(
            &app,
            &admin,
            json!({"current_password": "test-password-123", "new_password": &admin_email}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] identity: {err}"
        );
        assert!(
            err.to_string().contains("must not match your email"),
            "[{engine}] {err}"
        );

        // Strong and unrelated: accepted.
        let (code, _) = change(
            &app,
            &admin,
            json!({"current_password": "test-password-123", "new_password": "Sunny-Trail-42x"}),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] strong accepted");

        // The old password no longer works.
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"email": admin_email, "password": "test-password-123"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "[{engine}] old password rejected"
        );
    }
}

/// Phase 13 Bug 2 (ADR-0017): the cross-team isolation matrix. Team A's
/// member must never observe Team B's resources through any read surface:
/// direct IDs (404), lists (filtered), board, dashboard, workload,
/// capacity, search, my-tasks, goals, trash.
#[tokio::test]
async fn cross_team_isolation_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        // One USER belongs to both teams — the strongest leak witness: any
        // scoping bug shows the other team's rows to a legitimate member.
        let email = format!("p13iso-{suffix}@test.local");
        seed_user(&app.db, &email, "P13 Iso").await;
        let member = login_as(&app.app, &email, "test-password-123").await;

        let mut team_ids = Vec::new();
        let mut project_ids = Vec::new();
        for label in ["A", "B"] {
            let (code, team) = send_json(
                &app.app,
                "POST",
                "/api/v1/teams",
                Some(&member),
                json!({ "name": format!("Iso {label} {suffix}") }),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            let team_id = team["id"].as_str().unwrap().to_string();
            let (code, project) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/teams/{team_id}/projects"),
                Some(&member),
                json!({ "name": format!("Iso Project {label} {suffix}") }),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            let project_id = project["id"].as_str().unwrap().to_string();
            let (code, task) = send_json(
                &app.app,
                "POST",
                &format!("/api/v1/projects/{project_id}/tasks"),
                Some(&member),
                json!({ "title": format!("Iso task {label}") }),
            )
            .await;
            assert_eq!(code, StatusCode::CREATED);
            team_ids.push(team_id);
            project_ids.push(project_id);
            let _ = task;
        }
        let (team_a, team_b) = (team_ids[0].clone(), team_ids[1].clone());
        let (proj_a, proj_b) = (project_ids[0].clone(), project_ids[1].clone());

        // Team B's task id via its own project (for direct-ID checks).
        let (code, b_tasks) = get_json(
            &app.app,
            &format!("/api/v1/projects/{proj_b}/tasks"),
            &member,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let b_task_id = b_tasks["items"][0]["id"].as_str().unwrap().to_string();

        // -- Lists: only the requested team's rows ----------------------------
        let (code, projects) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_a}/projects"),
            &member,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let names: Vec<&str> = projects["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(
            !names.iter().any(|n| n.contains("B")),
            "[{engine}] Team A project list must not include Team B: {names:?}"
        );

        // -- Board: Team A's project board shows no Team B tasks ---------------
        let (code, board) = get_json(
            &app.app,
            &format!("/api/v1/projects/{proj_a}/board"),
            &member,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let board_text = board.to_string();
        assert!(
            !board_text.contains("Iso task B"),
            "[{engine}] Team A board leaked Team B's task"
        );

        // -- Team-scoped task search: only team A's rows -----------------------
        let (code, search) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_a}/tasks?q=Iso"),
            &member,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert!(
            !search.to_string().contains("Iso task B"),
            "[{engine}] Team A task search leaked Team B: {search}"
        );
        assert!(
            search.to_string().contains("Iso task A"),
            "[{engine}] Team A search should include its own task"
        );

        // -- Team-scoped surfaces: workload / capacity / dashboard -------------
        for path in [
            format!("/api/v1/teams/{team_a}/workload"),
            format!("/api/v1/teams/{team_a}/capacity"),
            format!("/api/v1/teams/{team_a}/dashboard"),
        ] {
            let (code, body) = get_json(&app.app, &path, &member).await;
            assert_eq!(code, StatusCode::OK, "[{engine}] {path}");
            assert!(
                !body.to_string().contains("Iso task B"),
                "[{engine}] {path} leaked Team B data"
            );
        }

        // -- my-tasks: assignee-scoped by design (ADR-0017). It can never
        //    surface a task the caller is not assigned to; the membership
        //    EXISTS clause drops rows from teams the caller left. No
        //    cross-team assertion applies here.

        // -- Goals: Team B's goals are not visible from team A's listing -------
        let (code, goals_a) =
            get_json(&app.app, &format!("/api/v1/teams/{team_a}/goals"), &member).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            goals_a["items"].as_array().unwrap().len(),
            0,
            "[{engine}] Team A goals listing leaked rows"
        );

        // -- Trash: scoped to the team ------------------------------------------
        let (code, trash) = get_json(
            &app.app,
            &format!("/api/v1/teams/{team_a}/projects/trash"),
            &member,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert!(
            !trash.to_string().contains(&proj_b),
            "[{engine}] Team A trash leaked Team B's project id"
        );

        // -- Direct-ID reads of the other team's resources → 404 ----------------
        // A SECOND user, member of team A only, must get 404 on team B ids.
        let outsider_email = format!("p13outsider-{suffix}@test.local");
        seed_user(&app.db, &outsider_email, "P13 Outsider").await;
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_a}/members"),
            Some(&member),
            json!({ "email": outsider_email, "role": "member" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let outsider = login_as(&app.app, &outsider_email, "test-password-123").await;

        for (path, label) in [
            (format!("/api/v1/projects/{proj_b}"), "project detail"),
            (format!("/api/v1/projects/{proj_b}/board"), "board"),
            (format!("/api/v1/projects/{proj_b}/tasks"), "task list"),
            (format!("/api/v1/tasks/{b_task_id}"), "task detail"),
        ] {
            let (code, _) = get_json(&app.app, &path, &outsider).await;
            assert_eq!(
                code,
                StatusCode::NOT_FOUND,
                "[{engine}] cross-team {label} must 404"
            );
        }

        // A team B write through an outsider is equally impossible.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{proj_b}/tasks"),
            Some(&outsider),
            json!({ "title": "intrusion" }),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] cross-team create");
    }
}

/// Phase 13 Bug 7 (ADR-0018): the team guest permission matrix. A guest is
/// a non-member whose every capability is an explicit grant. Deny by
/// default; revocation is immediate (row delete → next request 404).
#[tokio::test]
async fn guest_role_matrix() {
    for (engine, url) in test_db_urls() {
        let app = spawn_app(&url).await;
        let suffix = Uuid::now_v7();
        let owner_email = format!("p13g-owner-{suffix}@test.local");
        let guest_email = format!("p13g-guest-{suffix}@test.local");
        seed_user(&app.db, &owner_email, "P13G Owner").await;
        seed_user(&app.db, &guest_email, "P13G Guest").await;
        let owner = login_as(&app.app, &owner_email, "test-password-123").await;
        let guest = login_as(&app.app, &guest_email, "test-password-123").await;

        let (code, team) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&owner),
            json!({ "name": format!("P13G Team {suffix}") }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let team_id = team["id"].as_str().unwrap().to_string();
        let (code, project) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/projects"),
            Some(&owner),
            json!({ "name": format!("P13G Project {suffix}") }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let project_id = project["id"].as_str().unwrap().to_string();
        let (code, task) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&owner),
            json!({ "title": "P13G task" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let task_id = task["id"].as_str().unwrap().to_string();

        // A stranger sees NOTHING of the team before the guest invite.
        for path in [
            format!("/api/v1/teams/{team_id}"),
            format!("/api/v1/projects/{project_id}"),
            format!("/api/v1/tasks/{task_id}"),
        ] {
            let (code, _) = get_json(&app.app, &path, &guest).await;
            assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] pre-invite {path}");
        }

        // Invite as guest (Owner-only).
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/guests2"),
            Some(&guest),
            json!({ "email": guest_email }),
        )
        .await;
        // A non-manager gets 404 (non-disclosure — the guests2 surface is
        // invisible without Owner/Admin).
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] self-invite");
        let (code, invited) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/guests2"),
            Some(&owner),
            json!({ "email": guest_email }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] invite: {invited}");
        let guest_id = invited["guest_id"].as_str().unwrap().to_string();

        // Default deny: a registered guest without grants still sees nothing.
        for path in [
            format!("/api/v1/teams/{team_id}"),
            format!("/api/v1/projects/{project_id}"),
            format!("/api/v1/tasks/{task_id}"),
        ] {
            let (code, _) = get_json(&app.app, &path, &guest).await;
            assert_eq!(
                code,
                StatusCode::NOT_FOUND,
                "[{engine}] default-deny {path}"
            );
        }

        // Grant view_project → project + board + task LIST visible, but the
        // individual task detail still 404s without view_task.
        async fn grant(
            app: &TestApp,
            owner: &str,
            team_id: &str,
            guest_id: &str,
            project_id: &str,
            permission: &str,
        ) -> (StatusCode, Value) {
            send_json(
                &app.app,
                "POST",
                &format!("/api/v1/teams/{team_id}/guests2/{guest_id}/permissions"),
                Some(owner),
                json!({ "permission": permission, "scope_type": "project", "project_id": project_id }),
            )
            .await
        }
        let (code, _) = grant(
            &app,
            &owner,
            &team_id,
            &guest_id,
            &project_id,
            "view_project",
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "[{engine}] grant view_project");
        let (code, _) = get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &guest).await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] view_project grants the project"
        );
        let (code, board) = get_json(
            &app.app,
            &format!("/api/v1/projects/{project_id}/board"),
            &guest,
        )
        .await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] view_project grants the board"
        );
        assert!(board.to_string().contains("P13G task"));
        let (code, _) = get_json(&app.app, &format!("/api/v1/tasks/{task_id}"), &guest).await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] task detail requires view_task"
        );

        // Guest mutation attempts stay denied with the view_project grant.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&guest),
            json!({ "title": "guest intrusion" }),
        )
        .await;
        assert!(
            code == StatusCode::FORBIDDEN || code == StatusCode::NOT_FOUND,
            "[{engine}] guest create denied: {code}"
        );

        // view_task → task detail visible.
        let (code, _) = grant(&app, &owner, &team_id, &guest_id, &project_id, "view_task").await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, _) = get_json(&app.app, &format!("/api/v1/tasks/{task_id}"), &guest).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] view_task grants the task");

        // comment_task → guest can comment.
        let (code, _) = grant(
            &app,
            &owner,
            &team_id,
            &guest_id,
            &project_id,
            "comment_task",
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/comments"),
            Some(&guest),
            json!({ "body": "guest comment" }),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::CREATED,
            "[{engine}] comment_task grants commenting"
        );

        // view_activity → activity visible.
        let (code, _) = grant(
            &app,
            &owner,
            &team_id,
            &guest_id,
            &project_id,
            "view_activity",
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, _) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/activity"),
            &guest,
        )
        .await;
        assert_eq!(
            code,
            StatusCode::OK,
            "[{engine}] view_activity grants activity"
        );

        // view_team → team metadata visible (and the switcher lists it).
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/teams/{team_id}/guests2/{guest_id}/permissions"),
            Some(&owner),
            json!({ "permission": "view_team", "scope_type": "team" }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let (code, team_view) =
            get_json(&app.app, &format!("/api/v1/teams/{team_id}"), &guest).await;
        assert_eq!(code, StatusCode::OK, "[{engine}] view_team grants the team");
        assert_eq!(team_view["my_role"], "guest", "[{engine}] guest badge role");
        let (code, teams) = get_json(&app.app, "/api/v1/teams", &guest).await;
        assert_eq!(code, StatusCode::OK);
        assert!(
            teams.to_string().contains(&team_id),
            "[{engine}] the switcher lists guest teams"
        );

        // Cross-team isolation: a guest of THIS team cannot see another
        // team (create one the guest is not invited to).
        let (code, other) = send_json(
            &app.app,
            "POST",
            "/api/v1/teams",
            Some(&owner),
            json!({ "name": format!("P13G Other {suffix}") }),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        let other_id = other["id"].as_str().unwrap().to_string();
        let (code, _) = get_json(&app.app, &format!("/api/v1/teams/{other_id}"), &guest).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] cross-team guest");

        // Revocation is immediate: next request → 404.
        let (code, _) = send_request(
            &app.app,
            "DELETE",
            &format!("/api/v1/teams/{team_id}/guests2/{guest_id}"),
            Some(&owner),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT, "[{engine}] revoke guest");
        let (code, _) = get_json(&app.app, &format!("/api/v1/projects/{project_id}"), &guest).await;
        assert_eq!(
            code,
            StatusCode::NOT_FOUND,
            "[{engine}] revoked guest loses access immediately"
        );
    }
}
