//! Profile (Phase 9): work identity, notification preferences, the
//! server-computed My Work Summary, session management, and avatar serving.
//!
//! All operations are scoped to the authenticated caller; nothing here grants
//! or checks team capabilities. Task visibility in the summary mirrors
//! `my_tasks` exactly (same WHERE clause) so the counts can never disagree
//! with the lists they link to.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::avatar;
use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::notifications::NotificationPrefs;
use crate::state::AppState;
use crate::users::{self, ProfileUpdate, User};
use crate::util;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me", axum::routing::patch(update_me))
        .route("/me/summary", get(my_summary))
        .route("/me/sessions", get(list_sessions))
        .route("/me/sessions/{session_id}", delete(revoke_session))
        .route("/users", get(list_users_brief))
        .route("/users/{user_id}/avatar", get(serve_avatar))
}

// ---------------------------------------------------------------------------
// Validation helpers (shared with team settings)
// ---------------------------------------------------------------------------

/// Validate a username: `[a-z0-9_]{3,32}`, per the product contract used for
/// @mention resolution.
pub fn validate_username(raw: &str) -> Result<String, AppError> {
    let username = raw.trim().to_lowercase();
    let ok = (3..=32).contains(&username.chars().count())
        && username
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(username)
    } else {
        Err(AppError::validation(
            "username",
            "username must be 3-32 characters of a-z, 0-9 or _",
        ))
    }
}

/// Validate an IANA timezone name. The full registry lives in the browser
/// (`Intl.supportedValuesOf`), so the server enforces the shape only: 2-64
/// characters, `Area/Location` segments of letters/digits/`_+-`, or `UTC`.
pub fn validate_timezone(raw: &str) -> Result<String, AppError> {
    let tz = raw.trim();
    let valid = tz == "UTC"
        || ((2..=64).contains(&tz.len())
            && !tz.starts_with('/')
            && !tz.ends_with('/')
            && tz.split('/').all(|seg| {
                !seg.is_empty()
                    && seg
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
            }));
    if valid {
        Ok(tz.to_string())
    } else {
        Err(AppError::validation(
            "timezone",
            "timezone must be a valid IANA name such as Europe/Berlin",
        ))
    }
}

// ---------------------------------------------------------------------------
// PATCH /me
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct UpdateProfileRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    /// `null` clears the username, a string sets it.
    #[serde(default)]
    pub username: Option<Option<String>>,
    #[serde(default)]
    pub bio: Option<Option<String>>,
    #[serde(default)]
    pub timezone: Option<Option<String>>,
    #[serde(default)]
    pub avatar: Option<Option<String>>,
    #[serde(default)]
    pub notification_prefs: Option<serde_json::Value>,
    /// Forward-looking capacity unit (1-100); clamped server-side.
    #[serde(default)]
    pub capacity_limit: Option<i64>,
}

/// Merge a field-scoped validation error into an aggregate list.
fn push_field_errors(errors: &mut Vec<crate::error::FieldError>, err: AppError) {
    if let AppError::Validation(fields) = err {
        errors.extend(fields);
    }
}

async fn update_me(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(body): Json<UpdateProfileRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut field_errors: Vec<crate::error::FieldError> = Vec::new();

    let mut display_name: Option<String> = None;
    if let Some(name) = &body.display_name {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            field_errors.push(crate::error::FieldError::new(
                "display_name",
                "display name must be 1-120 characters",
            ));
        } else {
            display_name = Some(name.to_string());
        }
    }

    let mut username: Option<Option<String>> = None;
    if let Some(value) = &body.username {
        match value {
            Some(raw) => match validate_username(raw) {
                Ok(v) => username = Some(Some(v)),
                Err(e) => push_field_errors(&mut field_errors, e),
            },
            None => username = Some(None),
        }
    }

    let mut bio: Option<Option<String>> = None;
    if let Some(value) = &body.bio {
        match value {
            Some(text) if text.chars().count() > 200 => field_errors.push(
                crate::error::FieldError::new("bio", "bio must be at most 200 characters"),
            ),
            Some(text) => bio = Some(Some(text.trim().to_string())),
            None => bio = Some(None),
        }
    }

    let mut timezone: Option<Option<String>> = None;
    if let Some(value) = &body.timezone {
        match value {
            Some(raw) => match validate_timezone(raw) {
                Ok(v) => timezone = Some(Some(v)),
                Err(e) => push_field_errors(&mut field_errors, e),
            },
            None => timezone = Some(None),
        }
    }

    let mut avatar_update: Option<Option<String>> = None;
    if let Some(value) = &body.avatar {
        match value {
            Some(raw) => match avatar::validate_data_url(raw) {
                Ok(v) => {
                    let encoded = format!(
                        "data:{};base64,{}",
                        v.content_type,
                        encode_avatar_base64(&v.bytes)
                    );
                    avatar_update = Some(Some(encoded));
                }
                Err(e) => push_field_errors(&mut field_errors, e),
            },
            None => avatar_update = Some(None),
        }
    }

    let mut prefs_json: Option<String> = None;
    if let Some(prefs) = &body.notification_prefs {
        NotificationPrefs::validate_keys(prefs)?;
        // Materialize a full object: unspecified events keep the default.
        let mut resolved = NotificationPrefs::default();
        if let Some(map) = prefs.as_object() {
            if let Some(v) = map
                .get("task_assigned")
                .and_then(serde_json::Value::as_bool)
            {
                resolved.task_assigned = v;
            }
            if let Some(v) = map
                .get("comment_mention")
                .and_then(serde_json::Value::as_bool)
            {
                resolved.comment_mention = v;
            }
            if let Some(v) = map
                .get("task_submitted")
                .and_then(serde_json::Value::as_bool)
            {
                resolved.task_submitted = v;
            }
            if let Some(v) = map
                .get("task_accepted")
                .and_then(serde_json::Value::as_bool)
            {
                resolved.task_accepted = v;
            }
            if let Some(v) = map
                .get("task_rejected")
                .and_then(serde_json::Value::as_bool)
            {
                resolved.task_rejected = v;
            }
            if let Some(v) = map
                .get("task_status_changed")
                .and_then(serde_json::Value::as_bool)
            {
                resolved.task_status_changed = v;
            }
        }
        prefs_json =
            Some(serde_json::to_string(&resolved).map_err(|e| AppError::internal(e.to_string()))?);
    }

    if !field_errors.is_empty() {
        return Err(AppError::Validation(field_errors));
    }

    let mut conn = state.db.conn();
    users::update_profile(
        &mut conn,
        &user.id,
        &ProfileUpdate {
            display_name: display_name.as_deref(),
            username,
            bio,
            timezone,
            avatar: avatar_update,
            notification_prefs: prefs_json,
            capacity_limit: body.capacity_limit,
        },
    )
    .await?;

    let updated = users::find_by_id(&mut conn, &user.id)
        .await?
        .ok_or(AppError::Unauthenticated)?;
    Ok(Json(profile_response(&mut conn, &updated).await?))
}

/// The full profile payload: user fields plus the resolved notification
/// preferences (never the avatar itself — that is served as image bytes).
pub async fn profile_response(
    conn: &mut Conn<'_>,
    user: &User,
) -> Result<serde_json::Value, AppError> {
    let prefs = load_prefs(conn, &user.id).await?;
    Ok(serde_json::json!({
        "id": user.id,
        "email": user.email,
        "display_name": user.display_name,
        "username": user.username,
        "bio": user.bio,
        "timezone": user.timezone,
        "capacity_limit": user.capacity_limit.unwrap_or(crate::users::CAPACITY_DEFAULT),
        "created_at": user.created_at,
        "notification_prefs": prefs,
    }))
}

/// Stored preferences for a user, with defaults for unset rows/fields.
pub async fn load_prefs(conn: &mut Conn<'_>, user_id: &str) -> Result<NotificationPrefs, AppError> {
    let mut args = Args::new();
    args.s(user_id);
    let row = conn
        .fetch_optional("SELECT notification_prefs FROM users WHERE id = ?", args)
        .await?;
    let raw: Option<String> = row
        .and_then(|r| r.try_get::<Option<String>, _>("notification_prefs").ok())
        .flatten();
    Ok(raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<NotificationPrefs>(s).ok())
        .unwrap_or_default())
}

/// Standard base64 encoding (no new crate: only used for already-validated
/// avatar payloads). Shared with team settings.
#[must_use]
pub fn encode_avatar_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
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

// ---------------------------------------------------------------------------
// GET /me/summary — server-computed My Work Summary
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct MyWorkSummary {
    pub by_status: StatusCounts,
    pub overdue: i64,
    pub unread_mentions_7d: i64,
}

#[derive(Serialize)]
pub struct StatusCounts {
    pub backlog: i64,
    pub todo: i64,
    pub in_progress: i64,
    pub in_review: i64,
    pub resolved: i64,
    pub closed: i64,
}

/// The exact visibility filter used by `my_tasks`: assigned to the caller,
/// top-level, in an active project of one of their teams.
fn my_tasks_where() -> &'static str {
    "assignee_id = ? AND parent_task_id IS NULL \
     AND EXISTS (SELECT 1 FROM team_members tm WHERE tm.team_id = tasks.team_id AND tm.user_id = ?) \
     AND EXISTS (SELECT 1 FROM projects p WHERE p.id = tasks.project_id AND p.status = 'active' AND p.deleted_at IS NULL)"
}

async fn my_summary(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<Json<MyWorkSummary>, AppError> {
    let mut conn = state.db.conn();
    let where_clause = my_tasks_where();

    let mut args = Args::new();
    args.s(&user.id).s(&user.id);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT status, COUNT(*) AS n FROM tasks WHERE {where_clause} GROUP BY status"
            ),
            args,
        )
        .await?;
    let mut counts = StatusCounts {
        backlog: 0,
        todo: 0,
        in_progress: 0,
        in_review: 0,
        resolved: 0,
        closed: 0,
    };
    for row in rows {
        let status: String = row.try_get("status")?;
        let n: i64 = row.try_get("n")?;
        match status.as_str() {
            "backlog" => counts.backlog = n,
            "todo" => counts.todo = n,
            "in_progress" => counts.in_progress = n,
            "in_review" => counts.in_review = n,
            "resolved" => counts.resolved = n,
            "closed" => counts.closed = n,
            _ => {}
        }
    }

    // Overdue: open work whose due date has passed (TEXT ISO dates compare
    // correctly as strings; done statuses are excluded).
    let mut args = Args::new();
    args.s(&user.id).s(&user.id).s(util::parse_today_iso());
    let overdue = conn
        .fetch_optional(
            &format!(
                "SELECT COUNT(*) AS n FROM tasks WHERE {where_clause} \
                 AND due_date IS NOT NULL AND due_date < ? \
                 AND status IN ('backlog', 'todo', 'in_progress', 'in_review')"
            ),
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Unread mentions in the trailing 7 days.
    let mut args = Args::new();
    args.s(&user.id).i(util::now_ms() - 7 * 24 * 60 * 60 * 1000);
    let unread_mentions_7d = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM notifications \
             WHERE recipient_id = ? AND event = 'comment_mention' \
               AND read_at IS NULL AND created_at > ?",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    Ok(Json(MyWorkSummary {
        by_status: counts,
        overdue,
        unread_mentions_7d,
    }))
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub current: bool,
}

async fn list_sessions(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let mut args = Args::new();
    args.s(&user.id).i(util::now_ms());
    let rows = conn
        .fetch_all(
            "SELECT id, created_at, last_seen_at, ip_address, user_agent FROM sessions \
             WHERE user_id = ? AND revoked_at IS NULL AND expires_at > ? \
             ORDER BY last_seen_at DESC",
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(|row| {
            Ok(SessionInfo {
                id: row.try_get("id")?,
                created_at: row.try_get("created_at")?,
                last_seen_at: row.try_get("last_seen_at")?,
                ip_address: row.try_get("ip_address")?,
                user_agent: row.try_get("user_agent")?,
                current: row.try_get::<String, _>("id")? == user.session_id,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// Revoke one of the caller's own sessions. Revoking the current session is
/// allowed: the next request 401s and the client signs the tab out.
async fn revoke_session(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.conn();
    let mut args = Args::new();
    args.i(util::now_ms()).s(&session_id).s(&user.id);
    let affected = conn
        .execute(
            "UPDATE sessions SET revoked_at = ? \
             WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::NotFound("session not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// GET /users?ids= — batch identity lookup for hover cards & @mentions
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct UserInfo {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub username: Option<String>,
    pub bio: Option<String>,
}

/// Resolve a comma-separated id list (bounded to 100) to identity cards used
/// by avatar hover cards and the @mention resolver.
async fn list_users_brief(
    State(state): State<AppState>,
    user: AuthedUser,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ids: Vec<String> = params
        .get("ids")
        .map(|v| v.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        return Ok(Json(serde_json::json!({ "items": [] })));
    }
    let ids: Vec<String> = ids.into_iter().take(100).collect();
    let mut conn = state.db.conn();
    let _ = user; // authentication required; any session may resolve identities

    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT id, email, display_name, username, bio FROM users WHERE id IN ({placeholders})"
    );
    let mut args = Args::new();
    for id in &ids {
        args.s(id.clone());
    }
    let rows = conn.fetch_all(&sql, args).await?;
    let items = rows
        .iter()
        .map(|row| {
            Ok(UserInfo {
                id: row.try_get("id")?,
                display_name: row.try_get("display_name")?,
                email: row.try_get("email")?,
                username: row.try_get("username")?,
                bio: row.try_get("bio")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

// ---------------------------------------------------------------------------
// GET /users/{user_id}/avatar
// ---------------------------------------------------------------------------

/// Serve a user's avatar as image bytes. Avatars are never embedded in JSON
/// payloads; the UI builds this URL and falls back to initials on 404.
async fn serve_avatar(
    State(state): State<AppState>,
    _user: AuthedUser,
    Path(user_id): Path<String>,
) -> Result<(StatusCode, HeaderMap, Vec<u8>), AppError> {
    let mut conn = state.db.conn();
    let raw = users::find_avatar(&mut conn, &user_id).await?;
    let Some(raw) = raw else {
        return Err(AppError::NotFound("avatar not found"));
    };
    let validated =
        avatar::validate_data_url(&raw).map_err(|_| AppError::NotFound("avatar not found"))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        validated
            .content_type
            .parse()
            .map_err(|_| AppError::internal("invalid content type"))?,
    );
    // Private cache: teammates' avatars change rarely, but must not live in
    // shared caches.
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, max-age=300"),
    );
    Ok((StatusCode::OK, headers, validated.bytes))
}
