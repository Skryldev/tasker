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
