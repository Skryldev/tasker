//! Task checklists (Phase 12 WS4): lightweight, non-task checklist entries
//! with a checkbox, title, and ordering. Distinct from subtasks (which are
//! full tasks with `parent_task_id`).
//!
//! Progress semantics (WS4): a task's progress combines closed subtasks and
//! checked checklist items over their totals — `GET /tasks/{id}/progress`.
//!
//! Closed tasks (Phase 12 WS10) are immutable: every mutating handler here
//! rejects with 409 via `ensure_task_mutable`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::state::AppState;
use crate::tasks;
use crate::util;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/tasks/{task_id}/checklist",
            get(list_items).post(create_item),
        )
        .route(
            "/checklist/{item_id}",
            axum::routing::patch(update_item).delete(delete_item),
        )
        .route("/tasks/{task_id}/progress", get(task_progress))
}

#[derive(Debug, Clone, Serialize)]
pub struct ChecklistItem {
    pub id: String,
    pub task_id: String,
    pub title: String,
    pub is_checked: bool,
    pub position: f64,
    pub created_at: i64,
    pub updated_at: i64,
}

const CHECKLIST_COLUMNS: &str = "id, task_id, title, is_checked, position, created_at, updated_at";

fn row_to_item(row: &sqlx::any::AnyRow) -> Result<ChecklistItem, AppError> {
    Ok(ChecklistItem {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        title: row.try_get("title")?,
        // `is_checked` is normalized to an integer in SQL: PG returns a real
        // BOOLEAN while MySQL/SQLite return 0/1 through the portable layer.
        is_checked: row.try_get::<i64, _>("is_checked")? != 0,
        position: row.try_get("position")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_items(conn: &mut Conn<'_>, task_id: &str) -> Result<Vec<ChecklistItem>, AppError> {
    let mut args = Args::new();
    args.s(task_id);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {CHECKLIST_COLUMNS} FROM task_checklist_items \
                 WHERE task_id = ? ORDER BY position, created_at, id"
            ),
            args,
        )
        .await?;
    rows.iter().map(row_to_item).collect()
}

#[derive(Serialize)]
pub struct TaskProgress {
    pub subtasks_total: i64,
    pub subtasks_closed: i64,
    pub checklist_total: i64,
    pub checklist_checked: i64,
    /// Combined completion 0-100, rounded; `None` when there is nothing to
    /// track (no subtasks and no checklist items).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_pct: Option<i64>,
}

async fn compute_progress(conn: &mut Conn<'_>, task_id: &str) -> Result<TaskProgress, AppError> {
    // Four placeholders, so the task id binds four times.
    let mut args = Args::new();
    args.s(task_id).s(task_id).s(task_id).s(task_id);
    let row = conn
        .fetch_optional(
            "SELECT \
                 (SELECT COUNT(*) FROM tasks WHERE parent_task_id = ?) AS subtasks_total, \
                 (SELECT COUNT(*) FROM tasks WHERE parent_task_id = ? AND status = 'closed') AS subtasks_closed, \
                 (SELECT COUNT(*) FROM task_checklist_items WHERE task_id = ?) AS checklist_total, \
                 (SELECT COUNT(*) FROM task_checklist_items WHERE task_id = ? AND is_checked = TRUE) AS checklist_checked",
            args,
        )
        .await?
        .ok_or_else(|| AppError::internal("progress aggregate returned no row"))?;
    let subtasks_total: i64 = row.try_get("subtasks_total")?;
    let subtasks_closed: i64 = row.try_get("subtasks_closed")?;
    let checklist_total: i64 = row.try_get("checklist_total")?;
    let checklist_checked: i64 = row.try_get("checklist_checked")?;
    let done = subtasks_closed + checklist_checked;
    let total = subtasks_total + checklist_total;
    let pct = if total == 0 {
        None
    } else {
        Some(((done as f64 / total as f64) * 100.0).round() as i64)
    };
    Ok(TaskProgress {
        subtasks_total,
        subtasks_closed,
        checklist_total,
        checklist_checked,
        progress_pct: pct,
    })
}

async fn list_items(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    let items = load_items(&mut conn, &task_id).await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

async fn task_progress(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<Json<TaskProgress>, AppError> {
    let mut conn = state.db.conn();
    tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    Ok(Json(compute_progress(&mut conn, &task_id).await?))
}

#[derive(Deserialize)]
pub struct CreateChecklistItemRequest {
    pub title: String,
    #[serde(default)]
    pub position: Option<f64>,
}

async fn create_item(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
    Json(body): Json<CreateChecklistItemRequest>,
) -> Result<(StatusCode, Json<ChecklistItem>), AppError> {
    let title = body.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(AppError::validation(
            "title",
            "checklist item title must be 1-200 characters",
        ));
    }
    let mut conn = state.db.begin().await?;
    let (task, _) = tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    tasks::ensure_task_mutable(&task)?;
    let project = crate::projects::find_project(&mut conn, &task.project_id).await?;
    if project.status == "archived" {
        return Err(AppError::Conflict("archived projects are read-only".into()));
    }

    let position = if let Some(p) = body.position {
        p
    } else {
        let mut args = Args::new();
        args.s(&task_id);
        let row = conn
            .fetch_optional(
                "SELECT COALESCE(MAX(position), 0) AS max_pos FROM task_checklist_items WHERE task_id = ?",
                args,
            )
            .await?;
        row.map_or(1024.0, |r| {
            r.try_get::<f64, _>("max_pos").unwrap_or(0.0) + 1024.0
        })
    };

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&task_id)
        .s(&title)
        .b(false)
        .f(position)
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO task_checklist_items (id, task_id, title, is_checked, position, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    conn.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(ChecklistItem {
            id,
            task_id: task_id.clone(),
            title,
            is_checked: false,
            position,
            created_at: now,
            updated_at: now,
        }),
    ))
}

#[derive(Deserialize)]
pub struct UpdateChecklistItemRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub is_checked: Option<bool>,
}

async fn update_item(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(item_id): Path<String>,
    Json(body): Json<UpdateChecklistItemRequest>,
) -> Result<Json<ChecklistItem>, AppError> {
    let mut conn = state.db.begin().await?;
    let mut args = Args::new();
    args.s(&item_id);
    let row = conn
        .fetch_optional(
            &format!("SELECT {CHECKLIST_COLUMNS} FROM task_checklist_items WHERE id = ?"),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("checklist item not found"))?;
    let item = row_to_item(&row)?;
    // Authorization + closed-lock + archived-project guards.
    let (task, _) = tasks::require_task_access(&mut conn, &item.task_id, &user.id).await?;
    tasks::ensure_task_mutable(&task)?;
    let project = crate::projects::find_project(&mut conn, &task.project_id).await?;
    if project.status == "archived" {
        return Err(AppError::Conflict("archived projects are read-only".into()));
    }

    let title = body
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(&item.title);
    if title.chars().count() > 200 {
        return Err(AppError::validation(
            "title",
            "checklist item title must be at most 200 characters",
        ));
    }
    let is_checked = body.is_checked.unwrap_or(item.is_checked);

    let mut args = Args::new();
    args.s(title).b(is_checked).i(util::now_ms()).s(&item_id);
    conn.execute(
        "UPDATE task_checklist_items SET title = ?, is_checked = ?, updated_at = ? WHERE id = ?",
        args,
    )
    .await?;
    conn.commit().await?;

    Ok(Json(ChecklistItem {
        title: title.to_string(),
        is_checked,
        ..item
    }))
}

async fn delete_item(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(item_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let mut args = Args::new();
    args.s(&item_id);
    let row = conn
        .fetch_optional(
            &format!("SELECT {CHECKLIST_COLUMNS} FROM task_checklist_items WHERE id = ?"),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("checklist item not found"))?;
    let item = row_to_item(&row)?;
    // Authorization + closed-lock + archived-project guards.
    let (task, _) = tasks::require_task_access(&mut conn, &item.task_id, &user.id).await?;
    tasks::ensure_task_mutable(&task)?;
    let project = crate::projects::find_project(&mut conn, &task.project_id).await?;
    if project.status == "archived" {
        return Err(AppError::Conflict("archived projects are read-only".into()));
    }
    let mut args = Args::new();
    args.s(&item_id);
    conn.execute("DELETE FROM task_checklist_items WHERE id = ?", args)
        .await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
