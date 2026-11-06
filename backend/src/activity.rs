//! Activity history: transactional event records for significant task and
//! comment changes. Intentionally a plain table (no event sourcing).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::projects::require_project_access;
use crate::state::AppState;
use crate::tasks::require_task_access;
use crate::users::UserBrief;

pub mod events {
    pub const TASK_CREATED: &str = "task_created";
    pub const TASK_UPDATED: &str = "task_updated";
    pub const TASK_STATUS_CHANGED: &str = "task_status_changed";
    pub const TASK_PRIORITY_CHANGED: &str = "task_priority_changed";
    pub const TASK_ASSIGNEE_CHANGED: &str = "task_assignee_changed";
    pub const TASK_DUE_DATE_CHANGED: &str = "task_due_date_changed";
    pub const TASK_REPORTER_CHANGED: &str = "task_reporter_changed";
    pub const TASK_SUBMITTED: &str = "task_submitted";
    pub const TASK_ACCEPTED: &str = "task_accepted";
    pub const TASK_REJECTED: &str = "task_rejected";
    pub const TASK_REJECTION_REASON: &str = "task_rejection_reason";
    pub const TASK_DELETED: &str = "task_deleted";
    pub const COMMENT_ADDED: &str = "comment_added";
    pub const COMMENT_REPLIED: &str = "comment_replied";
    pub const COMMENT_EDITED: &str = "comment_edited";
    pub const COMMENT_DELETED: &str = "comment_deleted";
}

#[derive(Debug, Clone, Serialize)]
pub struct Activity {
    pub id: String,
    pub task_id: Option<String>,
    pub event: String,
    pub field: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub actor: Option<UserBrief>,
    pub created_at: i64,
}

/// Record an activity event. Must be called within the same transaction as
/// the mutation it describes so history stays consistent with state.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    conn: &mut Conn<'_>,
    team_id: &str,
    project_id: Option<&str>,
    task_id: Option<&str>,
    actor_id: Option<&str>,
    event: &str,
    field: Option<&str>,
    old_value: Option<&str>,
    new_value: Option<&str>,
) -> Result<(), AppError> {
    let mut args = Args::new();
    args.s(crate::util::new_id())
        .s(team_id)
        .opt_s(project_id.map(std::string::ToString::to_string))
        .opt_s(task_id.map(std::string::ToString::to_string))
        .opt_s(actor_id.map(std::string::ToString::to_string))
        .s(event)
        .opt_s(field.map(std::string::ToString::to_string))
        .opt_s(old_value.map(std::string::ToString::to_string))
        .opt_s(new_value.map(std::string::ToString::to_string))
        .i(crate::util::now_ms());
    conn.execute(
        "INSERT INTO activities (id, team_id, project_id, task_id, actor_id, event, field, old_value, new_value, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    Ok(())
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/tasks/{task_id}/activity", get(list_task_activity))
}

async fn list_task_activity(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let (task, actor) = require_task_access(&mut conn, &task_id, &user.id).await?;
    require_project_access(&mut conn, &task.project_id, &user.id).await?;

    // Guests see only non-sensitive workflow events. Internal collaboration
    // detail (comment events, description updates, rejection reasons,
    // deletions) stays internal: the whitelist is enforced in SQL.
    let event_filter = if actor.is_guest() {
        " AND a.event IN ('task_created','task_status_changed','task_priority_changed','task_due_date_changed','task_submitted','task_accepted','task_rejected') "
    } else {
        ""
    };
    let mut args = Args::new();
    args.s(&task_id);
    let sql = format!(
        "SELECT a.id, a.task_id, a.event, a.field, a.old_value, a.new_value, a.created_at, \
            u.id AS actor_id, u.display_name AS actor_name, u.email AS actor_email \
         FROM activities a LEFT JOIN users u ON u.id = a.actor_id \
         WHERE a.task_id = ?{event_filter} ORDER BY a.created_at DESC, a.id DESC LIMIT 100"
    );
    let rows = conn.fetch_all(&sql, args).await?;
    let items = rows
        .iter()
        .map(|row| {
            let actor_id: Option<String> = row.try_get("actor_id")?;
            let actor = if let Some(id) = actor_id {
                let display_name: String = row.try_get("actor_name")?;
                let email: Option<String> = row.try_get("actor_email")?;
                Some(UserBrief {
                    id,
                    display_name,
                    email: email.unwrap_or_default(),
                })
            } else {
                None
            };
            Ok(Activity {
                id: row.try_get("id")?,
                task_id: row.try_get("task_id")?,
                event: row.try_get("event")?,
                field: row.try_get("field")?,
                old_value: row.try_get("old_value")?,
                new_value: row.try_get("new_value")?,
                actor,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(json!({ "items": items })))
}
