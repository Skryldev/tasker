//! Instance-level user administration (Phase 12.5 Bug 2).
//!
//! Tasker accounts are **instance citizens**: they are created here — Owner
//! only — and then *assigned* to teams by team admins. Team-scoped account
//! creation is gone (ADR-0014): `POST /teams/{id}/members` now only attaches
//! an existing account to a team.
//!
//! Hard delete is deliberately guarded: an account referenced by tasks,
//! comments, or memberships cannot be hard-deleted (the FK graph has NOT NULL
//! creator columns that would either cascade into data loss or violate);
//! the endpoint returns 409 with an explanation. Deactivation is the
//! supported path for leaving users in place.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::db::Args;
use crate::error::AppError;
use crate::state::AppState;
use crate::users;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/users",
            get(list_users).post(create_user),
        )
        .route(
            "/admin/users/{user_id}",
            patch(update_user).delete(delete_user),
        )
}

/// The caller must be an instance Owner for every admin-users operation.
async fn require_owner(state: &AppState, user: &AuthedUser) -> Result<(), AppError> {
    let mut conn = state.db.conn();
    users::require_instance_owner(&mut conn, &user.id).await
}

#[derive(Deserialize)]
pub struct ListUsersQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub page_size: Option<i64>,
}

async fn list_users(
    State(state): State<AppState>,
    user: AuthedUser,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_owner(&state, &user).await?;
    let mut conn = state.db.conn();
    let (items, total) = users::list_admin_users(
        &mut conn,
        query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()),
        query.page.unwrap_or(1),
        query.page_size.unwrap_or(25),
    )
    .await?;
    Ok(Json(serde_json::json!({
        "items": items,
        "page": query.page.unwrap_or(1).max(1),
        "page_size": query.page_size.unwrap_or(25).clamp(1, 100),
        "total": total,
    })))
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub display_name: String,
    /// Optional @mention handle; must be unique (`[a-z0-9_]{3,32}`).
    #[serde(default)]
    pub username: Option<String>,
    /// Must satisfy the Phase 12.5 password policy (Bug 8). Handed to the
    /// new user out-of-band; they are prompted to change it on first login
    /// (follow-up work — out of scope here).
    pub temporary_password: String,
    /// Optional instance role. Only `member` is accepted: instance owners
    /// are minted exclusively by `bootstrap-admin` (the promotion path is
    /// deliberately not exposed over HTTP).
    #[serde(default)]
    pub role: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(body): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    require_owner(&state, &user).await?;
    if let Some(role) = body.role.as_deref().map(str::trim) {
        if !role.is_empty() && role != users::INSTANCE_ROLE_MEMBER {
            return Err(AppError::validation(
                "role",
                "only `member` can be granted over the API; instance owners are minted by bootstrap-admin",
            ));
        }
    }
    let mut conn = state.db.begin().await?;
    let created = users::create_validated(
        &mut conn,
        &body.email,
        &body.display_name,
        body.username.as_deref(),
        &body.temporary_password,
        users::INSTANCE_ROLE_MEMBER,
    )
    .await
    .map_err(|e| match e {
        // Unique violations arrive as 409; the admin form wants the field
        // surfaced as a 422-style problem with the message intact.
        AppError::Conflict(message) => AppError::validation("email", message),
        other => other,
    })?;
    conn.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": created.id,
            "email": created.email,
            "display_name": created.display_name,
            "username": created.username,
            "instance_role": users::INSTANCE_ROLE_MEMBER,
            "active": true,
            "created_at": created.created_at,
            "teams": [],
        })),
    ))
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    /// `null` clears the username, a string sets it (validated + unique).
    #[serde(default)]
    pub username: Option<Option<String>>,
    /// `false` deactivates (sign-in refused, sessions revoked), `true`
    /// reactivates.
    #[serde(default)]
    pub active: Option<bool>,
}

async fn update_user(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(user_id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_owner(&state, &user).await?;
    let mut conn = state.db.begin().await?;

    let mut args = Args::new();
    args.s(&user_id);
    let row = conn
        .fetch_optional(
            "SELECT id, email, display_name, username, \
                    CASE WHEN is_active THEN 1 ELSE 0 END AS active_flag, \
                    instance_role, created_at \
             FROM users WHERE id = ?",
            args,
        )
        .await?
        .ok_or(AppError::NotFound("user not found"))?;
    let mut current = users::AdminUser {
        id: row.try_get("id")?,
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
        username: row.try_get("username")?,
        instance_role: row
            .try_get::<String, _>("instance_role")
            .unwrap_or_else(|_| users::INSTANCE_ROLE_MEMBER.to_string()),
        active: row.try_get::<i64, _>("active_flag").unwrap_or(1) != 0,
        created_at: row.try_get("created_at")?,
        teams: Vec::new(),
    };

    // An owner cannot deactivate or demote themselves: that can strand the
    // instance without any owner.
    if user.id == user_id {
        if body.active == Some(false) {
            return Err(AppError::Conflict(
                "you cannot deactivate your own account".into(),
            ));
        }
        if let Some(None) = &body.username {
            return Err(AppError::Conflict(
                "you cannot clear your own username".into(),
            ));
        }
    }

    if let Some(display_name) = &body.display_name {
        let name = display_name.trim().to_string();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(AppError::validation(
                "display_name",
                "display name must be 1-120 characters",
            ));
        }
        current.display_name = name;
    }
    let mut username_update: Option<Option<String>> = None;
    if let Some(username) = &body.username {
        match username {
            Some(raw) => {
                username_update = Some(Some(crate::profile::validate_username(raw)?));
            }
            None => username_update = Some(None),
        }
    }

    // Reuse the profile persistence path: display_name + username with the
    // unique index mapped to 409.
    users::update_profile(
        &mut conn,
        &user_id,
        &users::ProfileUpdate {
            display_name: Some(current.display_name.as_str()),
            username: username_update.clone(),
            bio: None,
            timezone: None,
            avatar: None,
            notification_prefs: None,
            capacity_limit: None,
        },
    )
    .await?;
    if let Some(active) = body.active {
        if !users::set_active(&mut conn, &user_id, active).await? {
            return Err(AppError::NotFound("user not found"));
        }
        if !active {
            crate::auth::revoke_other_sessions(&mut conn, &user_id, None).await?;
        }
    }
    conn.commit().await?;

    Ok(Json(serde_json::json!({
        "id": current.id,
        "email": current.email,
        "display_name": current.display_name,
        "username": username_update.clone().unwrap_or(current.username),
        "instance_role": current.instance_role,
        "active": body.active.unwrap_or(current.active),
        "created_at": current.created_at,
    })))
}

#[derive(Deserialize, Default)]
pub struct DeleteUserRequest {
    /// Required for hard delete: the account's email, typed verbatim.
    #[serde(default)]
    pub confirm_email: Option<String>,
    /// Required for hard delete: the requesting owner's password.
    #[serde(default)]
    pub current_password: Option<String>,
}

async fn delete_user(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(user_id): Path<String>,
    body: Option<Json<DeleteUserRequest>>,
) -> Result<StatusCode, AppError> {
    require_owner(&state, &user).await?;
    let confirm = body.map(|Json(b)| b).unwrap_or_default();

    // An owner cannot delete their own account.
    if user.id == user_id {
        return Err(AppError::Conflict(
            "you cannot delete your own account".into(),
        ));
    }

    let hard = confirm.confirm_email.is_some() || confirm.current_password.is_some();
    let mut conn = state.db.begin().await?;

    let mut args = Args::new();
    args.s(&user_id);
    let row = conn
        .fetch_optional("SELECT email FROM users WHERE id = ?", args)
        .await?
        .ok_or(AppError::NotFound("user not found"))?;
    let email: String = row.try_get("email")?;

    if !hard {
        // Soft path: deactivate + revoke sessions. Reversible via PATCH.
        if !users::set_active(&mut conn, &user_id, false).await? {
            return Err(AppError::NotFound("user not found"));
        }
        crate::auth::revoke_other_sessions(&mut conn, &user_id, None).await?;
        conn.commit().await?;
        return Ok(StatusCode::NO_CONTENT);
    }

    // Hard delete: both confirmations, case-insensitive email match.
    let typed_email = confirm
        .confirm_email
        .as_deref()
        .map(|e| e.trim().to_lowercase())
        .unwrap_or_default();
    if typed_email != email.to_lowercase() {
        return Err(AppError::validation(
            "confirm_email",
            "the typed email does not match this account",
        ));
    }
    let typed_password = confirm.current_password.clone().unwrap_or_default();
    // Verify against the REQUESTING owner's hash (not the target account's).
    let mut hash_args = Args::new();
    hash_args.s(&user.id);
    let hash_row = conn
        .fetch_optional(
            "SELECT password_hash FROM users WHERE id = ?",
            hash_args,
        )
        .await?;
    let stored_hash: String = hash_row
        .map(|r| r.try_get("password_hash").unwrap_or_default())
        .unwrap_or_default();
    if !crate::security::password::verify(&typed_password, &stored_hash) {
        return Err(AppError::validation(
            "current_password",
            "the password is incorrect",
        ));
    }

    // Guard: refuse when the account still owns data. FK graph: tasks
    // reference users via NOT NULL creator columns; deleting would either
    // cascade into data loss or violate constraints depending on engine.
    let mut args = Args::new();
    args.s(&user_id);
    let counts = conn
        .fetch_optional(
            "SELECT \
                (SELECT COUNT(*) FROM team_members WHERE user_id = ?) AS memberships, \
                (SELECT COUNT(*) FROM tasks WHERE created_by = ?) AS created_tasks, \
                (SELECT COUNT(*) FROM comments WHERE author_id = ?) AS comments",
            {
                let mut a = Args::new();
                a.s(&user_id).s(&user_id).s(&user_id);
                a
            },
        )
        .await?;
    let memberships: i64 = counts.as_ref().map_or(0, |r| r.try_get("memberships").unwrap_or(0));
    let created_tasks: i64 = counts.as_ref().map_or(0, |r| r.try_get("created_tasks").unwrap_or(0));
    let comments: i64 = counts.as_ref().map_or(0, |r| r.try_get("comments").unwrap_or(0));
    if memberships > 0 || created_tasks > 0 || comments > 0 {
        return Err(AppError::Conflict(format!(
            "this account still has {memberships} team membership(s), {created_tasks} created task(s) and {comments} comment(s); deactivate it instead, or reassign and remove its data first"
        )));
    }

    crate::auth::revoke_other_sessions(&mut conn, &user_id, None).await?;
    let mut args = Args::new();
    args.s(&user_id);
    let deleted = conn
        .execute("DELETE FROM users WHERE id = ?", args)
        .await?;
    if deleted == 0 {
        return Err(AppError::NotFound("user not found"));
    }
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
