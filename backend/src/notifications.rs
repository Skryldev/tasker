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
    /// Rules Engine automation (Phase 8A): a rule executed an internal
    /// notification action. In-app only; never email/push.
    pub const AUTOMATION_TRIGGERED: &str = "automation_triggered";
}

/// The events a user can turn off in their profile (Phase 9). Every other
/// event type is always delivered.
pub const GATEABLE_EVENTS: [&str; 6] = [
    events::TASK_ASSIGNED,
    events::COMMENT_MENTION,
    events::TASK_SUBMITTED,
    events::TASK_ACCEPTED,
    events::TASK_REJECTED,
    events::TASK_STATUS_CHANGED,
];

/// Per-event notification toggles, persisted as a JSON object in
/// `users.notification_prefs`. Missing rows and missing fields default to
/// `true` — the pre-Phase-9 behavior — so provisioning needs no backfill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
// Six per-event toggles are the product shape (docs/security.md's "per-event
// booleans"); packing them into a bitmask would obscure the JSON contract.
#[allow(clippy::struct_excessive_bools)]
pub struct NotificationPrefs {
    pub task_assigned: bool,
    pub comment_mention: bool,
    pub task_submitted: bool,
    pub task_accepted: bool,
    pub task_rejected: bool,
    pub task_status_changed: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        Self {
            task_assigned: true,
            comment_mention: true,
            task_submitted: true,
            task_accepted: true,
            task_rejected: true,
            task_status_changed: true,
        }
    }
}

impl NotificationPrefs {
    /// Whether `event` may be delivered to this recipient.
    #[must_use]
    pub fn allows(&self, event: &str) -> bool {
        match event {
            events::TASK_ASSIGNED => self.task_assigned,
            events::COMMENT_MENTION => self.comment_mention,
            events::TASK_SUBMITTED => self.task_submitted,
            events::TASK_ACCEPTED => self.task_accepted,
            events::TASK_REJECTED => self.task_rejected,
            events::TASK_STATUS_CHANGED => self.task_status_changed,
            // Unlisted events are always delivered.
            _ => true,
        }
    }

    /// Reject keys outside the closed vocabulary so typo'd clients cannot
    /// silently persist a no-op setting.
    pub fn validate_keys(prefs: &serde_json::Value) -> Result<(), AppError> {
        let Some(map) = prefs.as_object() else {
            return Err(AppError::validation(
                "notification_prefs",
                "notification_prefs must be an object",
            ));
        };
        for key in map.keys() {
            if !matches!(
                key.as_str(),
                "task_assigned"
                    | "comment_mention"
                    | "task_submitted"
                    | "task_accepted"
                    | "task_rejected"
                    | "task_status_changed"
            ) {
                return Err(AppError::validation(
                    "notification_prefs",
                    format!("unknown notification event `{key}`"),
                ));
            }
            if !map.get(key).is_some_and(serde_json::Value::is_boolean) {
                return Err(AppError::validation(
                    "notification_prefs",
                    format!("notification preference `{key}` must be a boolean"),
                ));
            }
        }
        Ok(())
    }
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

/// Load stored preferences for a batch of users (missing rows → defaults).
async fn prefs_for(
    conn: &mut Conn<'_>,
    user_ids: &[String],
) -> Result<std::collections::HashMap<String, NotificationPrefs>, AppError> {
    let mut map = std::collections::HashMap::new();
    if user_ids.is_empty() {
        return Ok(map);
    }
    let placeholders = vec!["?"; user_ids.len()].join(", ");
    let sql = format!("SELECT id, notification_prefs FROM users WHERE id IN ({placeholders})");
    let mut args = Args::new();
    for id in user_ids {
        args.s(id.clone());
    }
    for row in conn.fetch_all(&sql, args).await? {
        let id: String = row.try_get("id")?;
        let raw: Option<String> = row.try_get("notification_prefs")?;
        let prefs = raw
            .as_deref()
            .and_then(|s| serde_json::from_str::<NotificationPrefs>(s).ok())
            .unwrap_or_default();
        map.insert(id, prefs);
    }
    Ok(map)
}

/// Insert a batch of notifications inside the caller's transaction.
/// Duplicates of `(recipient_id, event)` within one batch are collapsed, and
/// self-notifications (recipient == actor) are suppressed here so no call
/// site can forget the rule. Recipients with the event's notification
/// preference turned off do not receive it (Phase 9).
pub async fn record_batch(conn: &mut Conn<'_>, batch: &[NewNotification]) -> Result<(), AppError> {
    let mut seen: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
    // One preferences read per batch: only recipients of gateable events.
    let mut gated: Vec<String> = batch
        .iter()
        .filter(|e| GATEABLE_EVENTS.contains(&e.event))
        .map(|e| e.recipient_id.clone())
        .collect();
    gated.sort();
    gated.dedup();
    let prefs = prefs_for(conn, &gated).await?;
    for entry in batch {
        if entry.recipient_id == entry.actor_id {
            continue;
        }
        if !seen.insert((entry.event, entry.recipient_id.as_str())) {
            continue;
        }
        if GATEABLE_EVENTS.contains(&entry.event) {
            let allowed = match prefs.get(&entry.recipient_id) {
                Some(p) => p.allows(entry.event),
                None => true,
            };
            if !allowed {
                continue;
            }
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
    /// Pinned items surface above the time groups (Phase 14 WS5, ADR-0022).
    pub pinned: bool,
    pub pinned_at: Option<i64>,
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
        pinned: row
            .try_get::<i64, _>("pinned")
            .map(|v| v != 0)
            .unwrap_or(false),
        pinned_at: row.try_get("pinned_at").ok(),
    })
}

const NOTIFICATION_COLUMNS: &str = "n.id, n.event, n.task_id, n.actor_id, n.summary, n.created_at, n.read_at, \
     u.display_name AS actor_name, u.email AS actor_email, \
     CASE WHEN n.pinned THEN 1 ELSE 0 END AS pinned, n.pinned_at";

#[derive(Deserialize)]
pub struct ListNotificationsQuery {
    #[serde(default)]
    pub unread: bool,
    /// Only pinned items (the pinned section of the action center).
    #[serde(default)]
    pub pinned: bool,
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

    let mut filters = String::new();
    if query.unread {
        filters.push_str(" AND n.read_at IS NULL");
    }
    if query.pinned {
        filters.push_str(" AND n.pinned = TRUE");
    }

    let mut count_args = Args::new();
    count_args.s(&user.id);
    let total = conn
        .fetch_optional(
            &format!("SELECT COUNT(*) AS n FROM notifications n WHERE n.recipient_id = ?{filters}"),
            count_args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Pinned first, then time — the client renders the pinned section from
    // the same payload.
    let mut args = Args::new();
    args.s(&user.id).i(i64::from(page_size)).i(offset);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {NOTIFICATION_COLUMNS} FROM notifications n \
                 LEFT JOIN users u ON u.id = n.actor_id \
                 WHERE n.recipient_id = ?{filters} \
                 ORDER BY n.pinned DESC, n.pinned_at DESC, n.created_at DESC, n.id DESC LIMIT ? OFFSET ?"
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

/// Per-tab totals for the action center (Phase 14 WS5, ADR-0022):
/// `mentions` = comment_mention events, `assigned` = task_assigned,
/// `activity` = every other task event. Each tab carries its total and
/// unread count.
async fn tab_counts(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let mut args = Args::new();
    args.s(&user.id);
    let rows = conn
        .fetch_all(
            "SELECT event, COUNT(*) AS total, \
             COUNT(CASE WHEN read_at IS NULL THEN 1 END) AS unread \
             FROM notifications WHERE recipient_id = ? GROUP BY event",
            args,
        )
        .await?;
    let mut tabs = json!({
        "all": { "total": 0, "unread": 0 },
        "mentions": { "total": 0, "unread": 0 },
        "assigned": { "total": 0, "unread": 0 },
        "activity": { "total": 0, "unread": 0 },
    });
    for row in &rows {
        let event: String = row.try_get("event")?;
        let total: i64 = row.try_get("total")?;
        let unread: i64 = row.try_get("unread").unwrap_or(0);
        let mut bump = |tab: &str| {
            let t = &mut tabs[tab];
            t["total"] = serde_json::Value::from(t["total"].as_i64().unwrap_or(0) + total);
            t["unread"] = serde_json::Value::from(t["unread"].as_i64().unwrap_or(0) + unread);
        };
        bump("all");
        bump(match event.as_str() {
            "comment_mention" => "mentions",
            "task_assigned" => "assigned",
            _ => "activity",
        });
    }
    Ok(Json(tabs))
}

/// Pin toggle (Phase 14 WS5, ADR-0022). Pinned notifications float above
/// the time groups; an explicit `{ "pinned": bool }` body wins, otherwise
/// the request flips the current state.
#[derive(Deserialize, Default)]
pub struct PinRequest {
    #[serde(default)]
    pub pinned: Option<bool>,
}

async fn toggle_pin(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(notification_id): Path<String>,
    body: Option<Json<PinRequest>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let mut args = Args::new();
    args.s(&notification_id).s(&user.id);
    let row = conn
        .fetch_optional(
            "SELECT CASE WHEN pinned THEN 1 ELSE 0 END AS pinned, read_at \
             FROM notifications WHERE id = ? AND recipient_id = ?",
            args,
        )
        .await?
        .ok_or(AppError::NotFound("notification not found"))?;
    let current = row.try_get::<i64, _>("pinned").unwrap_or(0) != 0;
    let target = body.and_then(|Json(b)| b.pinned).unwrap_or(!current);
    if target != current {
        let mut args = Args::new();
        args.b(target).i(util::now_ms()).s(&notification_id).s(&user.id);
        conn.execute(
            "UPDATE notifications SET pinned = ?, pinned_at = ? \
             WHERE id = ? AND recipient_id = ?",
            args,
        )
        .await?;
    }
    Ok(Json(json!({ "pinned": target })))
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
        .route("/notifications/counts", get(tab_counts))
        .route("/notifications/read-all", post(mark_all_read))
        .route("/notifications/{notification_id}/read", post(mark_read))
        .route(
            "/notifications/{notification_id}/pin",
            post(toggle_pin),
        )
}
