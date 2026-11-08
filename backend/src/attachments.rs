//! Task file attachments (Phase 12 WS11).
//!
//! Storage is pluggable via [`crate::config::AttachmentsMode`]:
//!
//! - `disabled` (default): every endpoint fails closed with a clear message;
//!   deployments must opt in explicitly.
//! - `local`: bytes are stored under `TASKER_ATTACHMENTS_DIR`, keyed by
//!   `{task_id}/{attachment_id}/{filename}`.
//! - `s3`: MinIO/S3-compatible storage. Configuration is validated at boot,
//!   but the network driver is not implemented in this build — every storage
//!   operation fails fast with an explicit error rather than pretending to
//!   succeed.
//!
//! Metadata lives in `task_attachments` (migration 0014) with a soft-delete
//! (`deleted_at`). Mutations are guarded by the Phase 12 closed-task lock and
//! the archived-project read-only rule; reads stay available on closed tasks.

use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use sqlx::Row;
use std::path::PathBuf;

use crate::auth::AuthedUser;
use crate::config::{AttachmentsConfig, AttachmentsMode};
use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::state::AppState;
use crate::tasks;
use crate::users::{self, UserBrief};
use crate::util;

pub fn routes(max_upload_bytes: usize) -> Router<AppState> {
    Router::new()
        .route(
            // Per-route body limit: attachments may legitimately exceed the
            // 4 MB JSON default; the global limit still applies everywhere else.
            "/tasks/{task_id}/attachments",
            get(list_attachments).post(upload_attachment),
        )
        .route(
            "/attachments/{attachment_id}",
            get(download_attachment).delete(delete_attachment),
        )
        .layer(DefaultBodyLimit::max(max_upload_bytes))
}

// ---------------------------------------------------------------------------
// Storage backends
// ---------------------------------------------------------------------------

/// Pluggable byte storage. Enum dispatch keeps this dependency-free (no
/// `async_trait`); each arm is a concrete backend.
#[derive(Debug, Clone)]
enum Storage {
    Disabled,
    Local { root: PathBuf },
    S3,
}

impl Storage {
    fn from_config(config: &AttachmentsConfig) -> Self {
        match config.mode {
            AttachmentsMode::Disabled => Self::Disabled,
            AttachmentsMode::Local => Self::Local {
                root: PathBuf::from(&config.local_dir),
            },
            AttachmentsMode::S3 => Self::S3,
        }
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), AppError> {
        match self {
            Self::Disabled => Err(AppError::Conflict(
                "attachments are disabled on this deployment".into(),
            )),
            Self::Local { root } => {
                let path = root.join(key);
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| AppError::internal(format!("storage mkdir failed: {e}")))?;
                }
                tokio::fs::write(&path, bytes)
                    .await
                    .map_err(|e| AppError::internal(format!("storage write failed: {e}")))?;
                Ok(())
            }
            Self::S3 => Err(AppError::internal(
                "s3 attachment storage is not available in this build; configure `local` storage",
            )),
        }
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, AppError> {
        match self {
            Self::Disabled => Err(AppError::Conflict(
                "attachments are disabled on this deployment".into(),
            )),
            Self::Local { root } => tokio::fs::read(root.join(key))
                .await
                .map_err(|_| AppError::internal("attachment bytes missing from storage")),
            Self::S3 => Err(AppError::internal(
                "s3 attachment storage is not available in this build; configure `local` storage",
            )),
        }
    }

    /// Best-effort removal: a missing file is not an error (the metadata row
    /// is soft-deleted either way).
    async fn delete(&self, key: &str) {
        if let Self::Local { root } = self {
            let _ = tokio::fs::remove_file(root.join(key)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Attachment {
    pub id: String,
    pub task_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub uploaded_by: Option<UserBrief>,
    pub created_at: i64,
}

const ATTACHMENT_COLUMNS: &str = "a.id, a.task_id, a.filename, a.mime_type, a.size_bytes, \
     a.uploaded_by, a.created_at, u.email AS uploader_email, u.display_name AS uploader_name";

fn row_to_attachment(row: &sqlx::any::AnyRow) -> Result<Attachment, AppError> {
    Ok(Attachment {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        filename: row.try_get("filename")?,
        mime_type: row.try_get("mime_type")?,
        size_bytes: row.try_get("size_bytes")?,
        uploaded_by: row
            .try_get::<Option<String>, _>("uploaded_by")?
            .map(|id| {
                Ok::<UserBrief, AppError>(UserBrief {
                    id,
                    display_name: row
                        .try_get::<Option<String>, _>("uploader_name")?
                        .unwrap_or_default(),
                    email: row
                        .try_get::<Option<String>, _>("uploader_email")?
                        .unwrap_or_default(),
                })
            })
            .transpose()?,
        created_at: row.try_get("created_at")?,
    })
}

async fn load_attachment(
    conn: &mut Conn<'_>,
    attachment_id: &str,
) -> Result<(Attachment, String, String), AppError> {
    let mut args = Args::new();
    args.s(attachment_id);
    let row = conn
        .fetch_optional(
            &format!(
                "SELECT {ATTACHMENT_COLUMNS}, a.storage_key, t.status AS task_status \
                 FROM task_attachments a \
                 JOIN tasks t ON t.id = a.task_id \
                 LEFT JOIN users u ON u.id = a.uploaded_by \
                 WHERE a.id = ? AND a.deleted_at IS NULL"
            ),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("attachment not found"))?;
    let attachment = row_to_attachment(&row)?;
    let storage_key: String = row.try_get("storage_key")?;
    let task_status: String = row.try_get("task_status")?;
    Ok((attachment, storage_key, task_status))
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

async fn list_attachments(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    let mut args = Args::new();
    args.s(&task_id);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {ATTACHMENT_COLUMNS} \
                 FROM task_attachments a \
                 LEFT JOIN users u ON u.id = a.uploaded_by \
                 WHERE a.task_id = ? AND a.deleted_at IS NULL \
                 ORDER BY a.created_at, a.id"
            ),
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(row_to_attachment)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

async fn upload_attachment(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
    body: Multipart,
) -> Result<(StatusCode, Json<Attachment>), AppError> {
    let config = state.config.attachments.clone();
    let max_bytes = config.max_bytes;
    let mut conn = state.db.begin().await?;
    let (task, actor) = tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    tasks::ensure_task_mutable(&task)?;
    // Attachments follow the task-edit rule: task editors only.
    let can_update_any = actor.can(crate::authorization::Capability::UpdateAnyTask);
    let is_own = task.created_by == user.id || task.assignee_id.as_deref() == Some(&user.id);
    if !can_update_any && !is_own {
        return Err(AppError::Forbidden(
            "you can only attach files to tasks you created or are assigned to",
        ));
    }
    let project = crate::projects::find_project(&mut conn, &task.project_id).await?;
    if project.status == "archived" {
        return Err(AppError::Conflict("archived projects are read-only".into()));
    }

    let mut args = Args::new();
    args.s(&task_id);
    let count = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM task_attachments WHERE task_id = ? AND deleted_at IS NULL",
            args,
        )
        .await?
        .map_or(0i64, |r| r.try_get::<i64, _>("n").unwrap_or(0));
    if count >= i64::from(config.max_per_task) {
        return Err(AppError::validation(
            "file",
            format!("at most {} attachments per task", config.max_per_task),
        ));
    }

    // Exactly one `file` part is accepted.
    let mut multipart = body;
    let mut file: Option<(String, String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::validation("file", &format!("invalid multipart body: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = sanitize_filename(
            field
                .file_name()
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|| "file".to_string())
                .as_str(),
        );
        let mime_type = field
            .content_type()
            .filter(|m| {
                !m.is_empty()
                    && m.len() <= 127
                    && !m
                        .bytes()
                        .any(|b| b == b'\r' || b == b'\n' || b.is_ascii_whitespace())
            })
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::validation("file", format!("could not read upload: {e}")))?;
        if bytes.is_empty() {
            return Err(AppError::validation("file", "the uploaded file is empty"));
        }
        if bytes.len() > max_bytes {
            return Err(AppError::validation(
                "file",
                format!(
                    "the uploaded file exceeds the {} MB limit",
                    max_bytes / (1024 * 1024)
                ),
            ));
        }
        file = Some((filename, mime_type, bytes.to_vec()));
        break;
    }
    let (filename, mime_type, bytes) = file.ok_or_else(|| {
        AppError::validation(
            "file",
            "multipart field `file` with a file part is required",
        )
    })?;

    let storage = Storage::from_config(&config);
    let id = util::new_id();
    let storage_key = format!("{task_id}/{id}/{filename}");
    storage.put(&storage_key, &bytes).await?;

    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&task_id)
        .s(&filename)
        .s(&mime_type)
        .i(i64::try_from(bytes.len()).unwrap_or(i64::MAX))
        .s(&storage_key)
        .s(&user.id)
        .i(now);
    if let Err(e) = conn
        .execute(
            "INSERT INTO task_attachments (id, task_id, filename, mime_type, size_bytes, storage_key, uploaded_by, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
    {
        // Keep storage and database consistent: the row failed, drop the bytes.
        storage.delete(&storage_key).await;
        return Err(e);
    }
    let uploader = users::find_many_brief(&mut conn, std::slice::from_ref(&user.id))
        .await?
        .into_values()
        .next();
    conn.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(Attachment {
            id,
            task_id,
            filename,
            mime_type,
            size_bytes: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            uploaded_by: uploader,
            created_at: now,
        }),
    ))
}

async fn download_attachment(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(attachment_id): Path<String>,
) -> Result<Response, AppError> {
    let mut conn = state.db.conn();
    let (attachment, storage_key, _) = load_attachment(&mut conn, &attachment_id).await?;
    tasks::require_task_access(&mut conn, &attachment.task_id, &user.id).await?;
    drop(conn);

    let storage = Storage::from_config(&state.config.attachments);
    let bytes = storage.get(&storage_key).await?;

    let ascii_filename: String = attachment
        .filename
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let disposition = format!("attachment; filename=\"{ascii_filename}\"");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&attachment.mime_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok((StatusCode::OK, headers, bytes).into_response())
}

async fn delete_attachment(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(attachment_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let (attachment, storage_key, _) = load_attachment(&mut conn, &attachment_id).await?;
    let (task, actor) =
        tasks::require_task_access(&mut conn, &attachment.task_id, &user.id).await?;
    tasks::ensure_task_mutable(&task)?;
    // Attachments follow the task-edit rule: task editors only.
    let can_update_any = actor.can(crate::authorization::Capability::UpdateAnyTask);
    let is_own = task.created_by == user.id || task.assignee_id.as_deref() == Some(&user.id);
    if !can_update_any && !is_own {
        return Err(AppError::Forbidden(
            "you can only remove attachments from tasks you created or are assigned to",
        ));
    }
    let project = crate::projects::find_project(&mut conn, &task.project_id).await?;
    if project.status == "archived" {
        return Err(AppError::Conflict("archived projects are read-only".into()));
    }

    // Soft delete: the uploader (or a task editor) removes an attachment.
    let mut args = Args::new();
    args.i(util::now_ms()).s(&attachment_id);
    conn.execute(
        "UPDATE task_attachments SET deleted_at = ? WHERE id = ?",
        args,
    )
    .await?;
    conn.commit().await?;

    Storage::from_config(&state.config.attachments)
        .delete(&storage_key)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Keep only a safe basename: strip directories, replace anything outside the
/// allowed punctuation with `_`, cap at 255 chars. Storage keys are built from
/// this, so `../` tricks must not survive.
fn sanitize_filename(raw: &str) -> String {
    let basename = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric()
                || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')' | '[' | ']')
            {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = basename.trim().trim_matches('.').to_string();
    let safe = if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed
    };
    if safe.chars().count() > 255 {
        safe.chars().take(255).collect()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_filename;

    #[test]
    fn strips_paths() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\temp\\evil.exe"), "evil.exe");
        assert_eq!(
            sanitize_filename("a/b/c report final.txt"),
            "c report final.txt"
        );
    }

    #[test]
    fn replaces_unsafe_chars() {
        assert_eq!(sanitize_filename("a\nb"), "a_b");
        assert_eq!(sanitize_filename("100%_file?.pdf"), "100__file_.pdf");
    }

    #[test]
    fn empty_becomes_file() {
        assert_eq!(sanitize_filename(""), "file");
        assert_eq!(sanitize_filename(".."), "file");
        assert_eq!(sanitize_filename("///"), "file");
    }
}
