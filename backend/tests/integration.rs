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

async fn seed_user(db: &Database, email: &str, display: &str) -> String {
    let mut conn = db.begin().await.unwrap();
    let hash = password::hash("test-password-123").unwrap();
    let user = tasker::users::insert(
        &mut conn,
        &tasker::users::NewUser {
            email,
            display_name: display,
            password_hash: &hash,
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
            json!({ "email": member_email, "role": "member", "temp_password": "member-temp-pw" }),
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
            json!({"email": member_email, "role":"member", "temp_password":"member-temp-pw"}),
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
        let (code, _) = send_json(&app.app, "POST", &format!("/api/v1/tasks/{task_id}/comments"),
            Some(&admin),
            json!({"body": format!("hey @{member_email} and again @{member_email} — ping @ghost@nowhere.example")})).await;
        assert_eq!(code, StatusCode::CREATED);
        let (_code, n) = get_json(&app.app, "/api/v1/notifications?unread=true", &member).await;
        assert_eq!(
            mention_count(&n),
            before + 1,
            "[{engine}] mention dedup + team scoping"
        );

        // Editing without adding new mentions does not re-notify.
        let (_code, comments) = get_json(
            &app.app,
            &format!("/api/v1/tasks/{task_id}/comments"),
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
        {
            let mut conn = db.conn();
            let expected = match engine.as_str() {
                // MySQL: 0008 re-declares TEXT → VARCHAR(4096); the wire
                // reports VARCHAR as DATA_TYPE='varchar'.
                "mysql" => "varchar",
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
            } else if engine == "mysql" {
                // information_schema.DATA_TYPE materializes as NULL for every
                // row of this dictionary view when read over server-side
                // prepared statements on MySQL 8.4 (verified on CI: 9 rows,
                // every data_type empty) — SHOW FULL COLUMNS reports the
                // concrete type instead. Columns: Field, Type, ...
                conn.fetch_all("SHOW FULL COLUMNS FROM user_mfa", tasker::db::Args::new())
                    .await
                    .unwrap()
                    .iter()
                    .filter_map(|r| {
                        let field: String = r.try_get(0).ok()?;
                        let ty: String = r.try_get(1).ok()?;
                        (field == "recovery_codes_hashed").then_some(ty)
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
                observed.iter().any(|t| {
                    let t = t.to_ascii_lowercase();
                    t == expected || t.starts_with(&format!("{expected}("))
                }),
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
            json!({"email": member_email, "role":"member", "temp_password":"member-temp-pw"}),
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
            json!({"email": member_email, "role":"member", "temp_password":"member-temp-pw"}),
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
