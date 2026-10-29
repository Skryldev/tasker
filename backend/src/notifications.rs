//! Persistent in-app notifications.
//!
//! Notifications are plain rows written **in the same transaction** as the
//! business event that produced them — no broker, no fire-and-forget. The
//! module owns persistence and the recipient-scoped query API; event/recipient
//! selection lives in the emitting modules (`tasks`, `comments`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::state::AppState;
use crate::users::UserBrief;
use crate::util;

pub mod events {
    pub const TASK_ASSIGNED: &str = "task_assigned";
    pub const COMMENT_MENTION: &str = "comment_mention";
    pub const TASK_SUBMITTED: &str = "task_submitted";
    pub const TASK_ACCEPTED: &str = "task_accepted";
    pub const TASK_REJECTED: &str = "task_rejected";
    pub const TASK_STATUS_CHANGED: &str = "task_status_changed";
}

/// A pending notification produced by one operation. Recipient/event pairs
/// are deduplicated when the batch is written.
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub event: &'static str,
    pub recipient_id: String,
    pub task_id: String,
    pub actor_id: String,
    pub summary: String,
}

/// Insert a batch of notifications inside the caller's transaction.
/// Duplicates of `(recipient_id, event)` within one batch are collapsed, and
/// self-notifications (recipient == actor) are suppressed here so no call
/// site can forget the rule.
pub async fn record_batch(conn: &mut Conn<'_>, batch: &[NewNotification]) -> Result<(), AppError> {
    let mut seen: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
    for entry in batch {
        if entry.recipient_id == entry.actor_id {
            continue;
        }
        if !seen.insert((entry.event, entry.recipient_id.as_str())) {
            continue;
        }
        let mut args = Args::new();
        args.s(util::new_id())
            .s(&entry.recipient_id)
            .s(entry.event)
            .s(&entry.task_id)
            .s(&entry.actor_id)
            .s(util::truncate(&entry.summary, 300))
            .i(util::now_ms());
        conn.execute(
            "INSERT INTO notifications (id, recipient_id, event, task_id, actor_id, summary, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub id: String,
    pub event: String,
    pub task_id: Option<String>,
    pub actor: Option<UserBrief>,
    pub summary: String,
    pub created_at: i64,
    pub read_at: Option<i64>,
}

fn row_to_notification(row: &sqlx::any::AnyRow) -> Result<Notification, AppError> {
    let actor_id: Option<String> = row.try_get("actor_id")?;
    let actor = if let Some(id) = actor_id {
        let display_name: Option<String> = row.try_get("actor_name")?;
        let email: Option<String> = row.try_get("actor_email")?;
        Some(UserBrief {
            id,
            display_name: display_name.unwrap_or_default(),
            email: email.unwrap_or_default(),
        })
    } else {
        None
    };
    Ok(Notification {
        id: row.try_get("id")?,
        event: row.try_get("event")?,
        task_id: row.try_get("task_id")?,
        actor,
        summary: row.try_get("summary")?,
        created_at: row.try_get("created_at")?,
        read_at: row.try_get("read_at")?,
    })
}

const NOTIFICATION_COLUMNS: &str = "n.id, n.event, n.task_id, n.actor_id, n.summary, n.created_at, n.read_at, \
     u.display_name AS actor_name, u.email AS actor_email";

#[derive(Deserialize)]
pub struct ListNotificationsQuery {
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

async fn list_notifications(
    State(state): State<AppState>,
    user: AuthedUser,
    Query(query): Query<ListNotificationsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);

    let filter = if query.unread {
        " AND n.read_at IS NULL"
    } else {
        ""
    };

    let mut count_args = Args::new();
    count_args.s(&user.id);
    let total = conn
        .fetch_optional(
            &format!("SELECT COUNT(*) AS n FROM notifications n WHERE n.recipient_id = ?{filter}"),
            count_args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    let mut args = Args::new();
    args.s(&user.id).i(i64::from(page_size)).i(offset);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {NOTIFICATION_COLUMNS} FROM notifications n \
                 LEFT JOIN users u ON u.id = n.actor_id \
                 WHERE n.recipient_id = ?{filter} \
                 ORDER BY n.created_at DESC, n.id DESC LIMIT ? OFFSET ?"
            ),
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(row_to_notification)
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(Json(
        json!({ "items": items, "page": page, "page_size": page_size, "total": total }),
    ))
}

async fn unread_count(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let mut args = Args::new();
    args.s(&user.id);
    let count = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM notifications WHERE recipient_id = ? AND read_at IS NULL",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));
    Ok(Json(json!({ "count": count })))
}

/// Idempotent, recipient-scoped mark-as-read. A foreign or unknown id is a
/// 404 that does not reveal whether the notification exists for others.
async fn mark_read(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(notification_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.conn();
    let mut args = Args::new();
    args.i(util::now_ms()).s(&notification_id).s(&user.id);
    let affected = conn
        .execute(
            "UPDATE notifications SET read_at = ? \
             WHERE id = ? AND recipient_id = ? AND read_at IS NULL",
            args,
        )
        .await?;
    if affected == 0 {
        // Either already read (fine) or foreign/unknown — distinguish safely.
        let mut args = Args::new();
        args.s(&notification_id).s(&user.id);
        let owned = conn
            .fetch_optional(
                "SELECT id FROM notifications WHERE id = ? AND recipient_id = ?",
                args,
            )
            .await?
            .is_some();
        if !owned {
            return Err(AppError::NotFound("notification not found"));
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn mark_all_read(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.conn();
    let mut args = Args::new();
    args.i(util::now_ms()).s(&user.id);
    conn.execute(
        "UPDATE notifications SET read_at = ? WHERE recipient_id = ? AND read_at IS NULL",
        args,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/notifications", get(list_notifications))
        .route("/notifications/unread-count", get(unread_count))
        .route("/notifications/read-all", post(mark_all_read))
        .route("/notifications/{notification_id}/read", post(mark_read))
}
