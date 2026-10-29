//! Projects: CRUD, archival, and open-task counts.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::authorization::{Capability, Role};
use crate::db::{Args, Conn};
use crate::error::{AppError, FieldError};
use crate::state::AppState;
use crate::teams::require_membership;
use crate::util;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/teams/{team_id}/projects",
            get(list_projects).post(create_project),
        )
        .route(
            "/projects/{project_id}",
            get(get_project).patch(update_project),
        )
        .route("/projects/{project_id}/archive", post(archive_project))
        .route("/projects/{project_id}/unarchive", post(unarchive_project))
}

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub description: Option<String>,
    /// `active` or `archived`. Archived projects are read-only in the UI.
    pub status: String,
    pub owner_id: String,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_task_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_role: Option<Role>,
}

const PROJECT_COLUMNS: &str =
    "id, team_id, name, description, status, owner_id, created_by, created_at, updated_at";

pub async fn find_project(conn: &mut Conn<'_>, project_id: &str) -> Result<Project, AppError> {
    let mut args = Args::new();
    args.s(project_id);
    let row = conn
        .fetch_optional(
            &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?"),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("project not found"))?;
    row_to_project(&row)
}

fn row_to_project(row: &sqlx::any::AnyRow) -> Result<Project, AppError> {
    Ok(Project {
        id: row.try_get("id")?,
        team_id: row.try_get("team_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        owner_id: row.try_get("owner_id")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        open_task_count: None,
        my_role: None,
    })
}

/// Load the project and verify the caller's membership of its team.
pub async fn require_project_access(
    conn: &mut Conn<'_>,
    project_id: &str,
    user_id: &str,
) -> Result<(Project, crate::authorization::Membership), AppError> {
    let project = find_project(conn, project_id).await?;
    let membership = require_membership(conn, &project.team_id, user_id).await?;
    Ok((project, membership))
}

/// Ensure the referenced owner is a member of the team.
async fn validate_project_owner(
    conn: &mut Conn<'_>,
    team_id: &str,
    owner_id: &str,
) -> Result<(), AppError> {
    let membership = require_membership(conn, team_id, owner_id)
        .await
        .map_err(|_| {
            AppError::validation("owner_id", "the project owner must be a member of the team")
        })?;
    let _ = membership;
    Ok(())
}

fn validate_project_input(
    name: &str,
    description: Option<&str>,
) -> Result<(String, Option<String>), AppError> {
    let mut errors = Vec::new();
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 160 {
        errors.push(FieldError::new(
            "name",
            "project name must be 1-160 characters",
        ));
    }
    let description = description.map(str::trim).filter(|d| !d.is_empty());
    if let Some(d) = description
        && d.chars().count() > 2000
    {
        errors.push(FieldError::new(
            "description",
            "description must be at most 2000 characters",
        ));
    }
    if errors.is_empty() {
        Ok((name, description.map(std::string::ToString::to_string)))
    } else {
        Err(AppError::Validation(errors))
    }
}

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
}

async fn create_project(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    let (name, description) = validate_project_input(&body.name, body.description.as_deref())?;
    let mut conn = state.db.begin().await?;
    require_membership(&mut conn, &team_id, &user.id).await?;
    crate::authorization::require_capability(
        &mut conn,
        &team_id,
        &user.id,
        Capability::ManageProjects,
    )
    .await?;
    let owner_id = body.owner_id.unwrap_or_else(|| user.id.clone());
    validate_project_owner(&mut conn, &team_id, &owner_id).await?;

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&team_id)
        .s(&name)
        .opt_s(description.clone())
        .s("active")
        .s(&owner_id)
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO projects (id, team_id, name, description, status, owner_id, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    conn.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(Project {
            id,
            team_id,
            name,
            description,
            status: "active".to_string(),
            owner_id,
            created_by: user.id,
            created_at: now,
            updated_at: now,
            open_task_count: Some(0),
            my_role: None,
        }),
    ))
}

#[derive(Deserialize)]
pub struct ListProjectsQuery {
    #[serde(default)]
    pub include_archived: bool,
}

async fn list_projects(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Query(query): Query<ListProjectsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let membership = require_membership(&mut conn, &team_id, &user.id).await?;
    let mut args = Args::new();
    args.s(&team_id).b(query.include_archived);
    let rows = conn
        .fetch_all(
            "SELECT p.id, p.team_id, p.name, p.description, p.status, p.owner_id, p.created_by, \
                    p.created_at, p.updated_at, \
                    (SELECT COUNT(*) FROM tasks t WHERE t.project_id = p.id AND t.status != 'done') AS open_task_count \
             FROM projects p \
             WHERE p.team_id = ? AND (p.status = 'active' OR ? = TRUE) \
             ORDER BY p.created_at",
            args,
        )
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let mut project = row_to_project(&row)?;
        project.open_task_count = Some(row.try_get("open_task_count")?);
        project.my_role = Some(membership.role);
        items.push(project);
    }
    Ok(Json(json!({ "items": items })))
}

async fn get_project(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
) -> Result<Json<Project>, AppError> {
    let mut conn = state.db.conn();
    let (mut project, membership) =
        require_project_access(&mut conn, &project_id, &user.id).await?;
    project.my_role = Some(membership.role);
    Ok(Json(project))
}

#[derive(Deserialize)]
pub struct UpdateProjectRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
}

async fn update_project(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
    Json(body): Json<UpdateProjectRequest>,
) -> Result<Json<Project>, AppError> {
    let mut conn = state.db.begin().await?;
    let (project, _) = require_project_access(&mut conn, &project_id, &user.id).await?;
    crate::authorization::require_capability(
        &mut conn,
        &project.team_id,
        &user.id,
        Capability::ManageProjects,
    )
    .await?;
    if project.status == "archived" {
        return Err(AppError::Conflict(
            "archived projects cannot be modified; unarchive first".into(),
        ));
    }
    let (name, description) = validate_project_input(
        body.name.as_deref().unwrap_or(&project.name),
        body.description
            .as_deref()
            .or(project.description.as_deref()),
    )?;
    let owner_id = body.owner_id.unwrap_or_else(|| project.owner_id.clone());
    validate_project_owner(&mut conn, &project.team_id, &owner_id).await?;

    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&name)
        .opt_s(description.clone())
        .s(&owner_id)
        .i(now)
        .s(&project_id);
    conn.execute(
        "UPDATE projects SET name = ?, description = ?, owner_id = ?, updated_at = ? WHERE id = ?",
        args,
    )
    .await?;
    conn.commit().await?;
    Ok(Json(Project {
        id: project_id,
        team_id: project.team_id,
        name,
        description,
        status: project.status,
        owner_id,
        created_by: project.created_by,
        created_at: project.created_at,
        updated_at: now,
        open_task_count: project.open_task_count,
        my_role: project.my_role,
    }))
}

async fn archive_project(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
) -> Result<Json<Project>, AppError> {
    set_project_status(state, user, project_id, "archived").await
}

async fn unarchive_project(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
) -> Result<Json<Project>, AppError> {
    set_project_status(state, user, project_id, "active").await
}

async fn set_project_status(
    state: AppState,
    user: AuthedUser,
    project_id: String,
    status: &str,
) -> Result<Json<Project>, AppError> {
    let mut conn = state.db.begin().await?;
    let (project, _) = require_project_access(&mut conn, &project_id, &user.id).await?;
    crate::authorization::require_capability(
        &mut conn,
        &project.team_id,
        &user.id,
        Capability::ManageProjects,
    )
    .await?;
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(status).i(now).s(&project_id);
    conn.execute(
        "UPDATE projects SET status = ?, updated_at = ? WHERE id = ?",
        args,
    )
    .await?;
    conn.commit().await?;
    Ok(Json(Project {
        status: status.to_string(),
        updated_at: now,
        ..project
    }))
}
