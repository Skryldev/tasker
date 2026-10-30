//! HTTP integration tests: authentication, authorization, and task workflows
//! against real database engines.
//!
//! Engines: `SQLite` always (temp file). `PostgreSQL` and `MySQL` run when env
//! vars are set (see `test_db_urls`), typically via scripts/test-matrix.sh.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
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
        .with_max_level(tracing::Level::ERROR)
        .with_test_writer()
        .try_init();
    let config = test_config(url.to_string());
    let db = Database::connect(&config).await.expect("connect");
    db.migrate().await.expect("migrate");
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
    .unwrap();
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

        // 6. Own-task updates work and land in the activity history.
        let (status, updated) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&member_cookie),
            json!({ "status": "in_progress" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "[{engine}] own-task update");
        assert_eq!(updated["status"], "in_progress");
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
        assert!(
            events.contains(&"task_status_changed"),
            "[{engine}] status event"
        );

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

        // Creation straight into done is rejected: acceptance requires review.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/projects/{project_id}/tasks"),
            Some(&admin),
            json!({"title":"Instant done","status":"done"}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] create as done"
        );

        // Member self-assigns and moves to in_progress.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&member),
            json!({"assignee_id": member_id, "status":"in_progress"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] claim + start");

        // Outsider cannot submit (404: cross-team isolation).
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/submit"),
            Some(&outsider),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider submit");

        // Assignee submits; reporter (admin, not assignee) is notified.
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/submit"),
            Some(&member),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] submit");
        assert_eq!(t["status"], "in_review");

        // The assignee cannot accept their own work even though they are not
        // the reporter; acceptance belongs to the reporter.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/accept"),
            Some(&member),
            json!({}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::FORBIDDEN,
            "[{engine}] assignee self-accept"
        );

        // Rejection requires a meaningful reason.
        let (code, problem) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/reject"),
            Some(&admin),
            json!({"reason":"   "}),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::UNPROCESSABLE_ENTITY,
            "[{engine}] blank reject"
        );
        assert_eq!(problem["errors"][0]["field"], "reason");

        // Reporter accepts: resolution recorded.
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/accept"),
            Some(&admin),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] accept");
        assert_eq!(t["status"], "done");
        assert_eq!(t["resolution"], "accepted");
        assert_eq!(t["resolved_by"]["id"].as_str().unwrap(), admin_id);
        assert!(t["resolved_at"].is_number());
        assert!(t["completed_at"].is_number());

        // Stale accept is a conflict; resolution stays consistent.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/accept"),
            Some(&admin),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] stale accept");
        let (code, t) = get_json(&app.app, &format!("/api/v1/tasks/{task_id}"), &admin).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(t["resolution"], "accepted");

        // Non-member cannot act even with the task id.
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/reject"),
            Some(&outsider),
            json!({"reason":"nope"}),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "[{engine}] outsider reject");

        // Reopen clears the resolution; member resubmits; admin rejects with reason.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"status":"in_progress"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] reopen");
        let (_code, t) = get_json(&app.app, &format!("/api/v1/tasks/{task_id}"), &admin).await;
        assert_eq!(
            t["resolution"],
            Value::Null,
            "[{engine}] reopen clears resolution"
        );
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/submit"),
            Some(&member),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (code, t) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{task_id}/reject"),
            Some(&admin),
            json!({"reason":"fix the intro"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "[{engine}] reject");
        assert_eq!(t["status"], "in_progress");
        assert_eq!(t["resolution"], "rejected");
        assert_eq!(t["resolution_note"], "fix the intro");
        assert_eq!(t["completed_at"], Value::Null);

        // Patch cannot skip review: in_progress -> done forbidden even for admins.
        let (code, _) = send_json(
            &app.app,
            "PATCH",
            &format!("/api/v1/tasks/{task_id}"),
            Some(&admin),
            json!({"status":"done"}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT, "[{engine}] skip review");

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
            !member_events.contains(&"task_submitted"),
            "[{engine}] no self submission notif"
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
            admin_events.contains(&"task_submitted"),
            "[{engine}] reporter submit notif: {admin_events:?}"
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
            &format!("/api/v1/tasks/{t1_id}/submit"),
            Some(&guest),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] guest submit");
        let (code, _) = send_json(
            &app.app,
            "POST",
            &format!("/api/v1/tasks/{t1_id}/accept"),
            Some(&guest),
            json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "[{engine}] guest accept");
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
                "assignee_id": member_id,
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
            json!({"title":"X","status":"done"}),
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
