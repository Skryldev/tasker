//! Task comments with an explicit ownership/permission policy:
//! - Any team member can comment.
//! - Authors can edit and delete their own comments.
//! - Owner/Admin can edit any comment; Owner/Admin/Manager can delete any.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use crate::activity::{self, events};
use crate::auth::AuthedUser;
use crate::authorization::Capability;
use crate::db::{Args, Conn};
use crate::error::{AppError, FieldError};
use crate::state::AppState;
use crate::tasks::require_task_access;
use crate::users::UserBrief;
use crate::util;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/tasks/{task_id}/comments",
            get(list_comments).post(create_comment),
        )
        .route(
            "/comments/{comment_id}",
            axum::routing::patch(update_comment).delete(delete_comment),
        )
}

#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    pub id: String,
    pub task_id: String,
    pub author: UserBrief,
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
}

const COMMENT_COLUMNS: &str = "c.id, c.task_id, c.author_id, c.body, c.created_at, c.updated_at, u.email AS author_email, u.display_name AS author_name";

fn row_to_comment(row: &sqlx::any::AnyRow) -> Result<Comment, AppError> {
    Ok(Comment {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        author: UserBrief {
            id: row.try_get("author_id")?,
            display_name: row.try_get("author_name")?,
            email: row.try_get("author_email")?,
        },
        body: row.try_get("body")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn find_comment(conn: &mut Conn<'_>, comment_id: &str) -> Result<Comment, AppError> {
    let mut args = Args::new();
    args.s(comment_id);
    let row = conn
        .fetch_optional(
            &format!(
                "SELECT {COMMENT_COLUMNS} FROM comments c JOIN users u ON u.id = c.author_id WHERE c.id = ?"
            ),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("comment not found"))?;
    row_to_comment(&row)
}

fn validate_body(body: &str) -> Result<String, AppError> {
    let body = body.trim().to_string();
    if body.is_empty() {
        return Err(AppError::Validation(vec![FieldError::new(
            "body",
            "comment body must not be empty",
        )]));
    }
    if body.chars().count() > 10_000 {
        return Err(AppError::Validation(vec![FieldError::new(
            "body",
            "comment body must be at most 10000 characters",
        )]));
    }
    Ok(body)
}

#[derive(Deserialize)]
pub struct ListCommentsQuery {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

async fn list_comments(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
    Query(query): Query<ListCommentsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_task_access(&mut conn, &task_id, &user.id).await?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);

    let mut args = Args::new();
    args.s(&task_id);
    let total = conn
        .fetch_optional("SELECT COUNT(*) AS n FROM comments WHERE task_id = ?", args)
        .await?
        .map(|r| r.try_get::<i64, _>("n").unwrap_or(0))
        .unwrap_or(0);

    let mut args = Args::new();
    args.s(&task_id).i(i64::from(page_size)).i(offset);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {COMMENT_COLUMNS} FROM comments c JOIN users u ON u.id = c.author_id \
                 WHERE c.task_id = ? ORDER BY c.created_at, c.id LIMIT ? OFFSET ?"
            ),
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(row_to_comment)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(
        json!({ "items": items, "page": page, "page_size": page_size, "total": total }),
    ))
}

#[derive(Deserialize)]
pub struct CreateCommentRequest {
    pub body: String,
}

async fn create_comment(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
    Json(payload): Json<CreateCommentRequest>,
) -> Result<(StatusCode, Json<Comment>), AppError> {
    let body = validate_body(&payload.body)?;
    let mut conn = state.db.begin().await?;
    let (task, _) = require_task_access(&mut conn, &task_id, &user.id).await?;

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id).s(&task_id).s(&user.id).s(&body).i(now).i(now);
    conn.execute(
        "INSERT INTO comments (id, task_id, author_id, body, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    activity::record(
        &mut conn,
        &task.team_id,
        Some(&task.project_id),
        Some(&task_id),
        Some(&user.id),
        events::COMMENT_ADDED,
        None,
        None,
        Some(&util::truncate(&body, 120)),
    )
    .await?;
    conn.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(Comment {
            id,
            task_id,
            author: UserBrief {
                id: user.id,
                display_name: user.display_name,
                email: user.email,
            },
            body,
            created_at: now,
            updated_at: now,
        }),
    ))
}

#[derive(Deserialize)]
pub struct UpdateCommentRequest {
    pub body: String,
}

async fn update_comment(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(comment_id): Path<String>,
    Json(payload): Json<UpdateCommentRequest>,
) -> Result<Json<Comment>, AppError> {
    let body = validate_body(&payload.body)?;
    let mut conn = state.db.begin().await?;
    let comment = find_comment(&mut conn, &comment_id).await?;
    let (task, membership) = require_task_access(&mut conn, &comment.task_id, &user.id).await?;

    let can_edit_any = membership.role.can(Capability::EditAnyComment);
    if comment.author.id != user.id && !can_edit_any {
        return Err(AppError::Forbidden("you can only edit your own comments"));
    }

    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&body).i(now).s(&comment_id);
    conn.execute(
        "UPDATE comments SET body = ?, updated_at = ? WHERE id = ?",
        args,
    )
    .await?;
    activity::record(
        &mut conn,
        &task.team_id,
        Some(&task.project_id),
        Some(&task.id),
        Some(&user.id),
        events::COMMENT_EDITED,
        Some("body"),
        Some(&util::truncate(&comment.body, 120)),
        Some(&util::truncate(&body, 120)),
    )
    .await?;
    conn.commit().await?;

    Ok(Json(Comment {
        body,
        updated_at: now,
        ..comment
    }))
}

async fn delete_comment(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(comment_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let comment = find_comment(&mut conn, &comment_id).await?;
    let (task, membership) = require_task_access(&mut conn, &comment.task_id, &user.id).await?;

    let can_delete_any = membership.role.can(Capability::DeleteAnyComment);
    if comment.author.id != user.id && !can_delete_any {
        return Err(AppError::Forbidden("you can only delete your own comments"));
    }

    activity::record(
        &mut conn,
        &task.team_id,
        Some(&task.project_id),
        Some(&task.id),
        Some(&user.id),
        events::COMMENT_DELETED,
        Some("body"),
        Some(&util::truncate(&comment.body, 120)),
        None,
    )
    .await?;

    let mut args = Args::new();
    args.s(&comment_id);
    conn.execute("DELETE FROM comments WHERE id = ?", args)
        .await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
