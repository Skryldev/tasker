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
