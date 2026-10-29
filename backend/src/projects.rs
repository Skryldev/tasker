//! Projects: CRUD, archival, and open-task counts.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
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
        .route(
            "/projects/{project_id}/guests",
            get(list_guests).post(grant_guest),
        )
        .route(
            "/projects/{project_id}/guests/{user_id}",
            delete(revoke_guest),
        )
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
    /// True when the caller reaches this project through a project-guest
    /// grant rather than team membership (read-only).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_guest: bool,
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
        is_guest: false,
    })
}

/// Load the project and verify the caller's membership of its team.
/// How the caller relates to a project: as a team member (full capabilities
/// per the team role) or as a project guest (read-only, explicit grant).
/// Guests hold no team membership, so `can` is false for every capability —
/// mutations are denied by construction and read paths check `can_view`.
#[derive(Debug, Clone)]
pub struct ProjectActor {
    pub membership: Option<crate::authorization::Membership>,
    pub is_guest: bool,
}

impl ProjectActor {
    #[must_use]
    pub fn can(&self, capability: crate::authorization::Capability) -> bool {
        self.membership
            .as_ref()
            .is_some_and(|m| m.role.can(capability))
    }

    #[must_use]
    pub fn is_guest(&self) -> bool {
        self.is_guest
    }

    /// Any read access to the project: a team member or an explicit guest.
    #[must_use]
    pub fn can_view(&self) -> bool {
        self.membership.is_some() || self.is_guest
    }

    /// Guard for internal-only content (templates, guest management, etc.).
    pub fn require_team_member(&self) -> Result<&crate::authorization::Membership, AppError> {
        self.membership
            .as_ref()
            .ok_or(AppError::NotFound("project not found"))
    }
}

/// Resolve the project and the caller's access: team membership first, then
/// the explicit project-guest grant. Outsiders get the standard 404.
pub async fn require_project_access(
    conn: &mut Conn<'_>,
    project_id: &str,
    user_id: &str,
) -> Result<(Project, ProjectActor), AppError> {
    let project = find_project(conn, project_id).await?;
    let actor = if let Some(membership) =
        crate::authorization::get_membership(conn, &project.team_id, user_id).await?
    {
        ProjectActor {
            membership: Some(membership),
            is_guest: false,
        }
    } else {
        let mut args = Args::new();
        args.s(project_id).s(user_id);
        let guest = conn
            .fetch_optional(
                "SELECT user_id FROM project_members WHERE project_id = ? AND user_id = ?",
                args,
            )
            .await?
            .is_some();
        if !guest {
            return Err(AppError::NotFound("project not found"));
        }
        ProjectActor {
            membership: None,
            is_guest: true,
        }
    };
    Ok((project, actor))
}

const PROJECT_MEMBER_COLUMNS: &str = "project_id, user_id, role, created_at, created_by";

pub async fn find_project_member(
    conn: &mut Conn<'_>,
    project_id: &str,
    user_id: &str,
) -> Result<Option<(String, i64, Option<String>)>, AppError> {
    let mut args = Args::new();
    args.s(project_id).s(user_id);
    let row = conn
        .fetch_optional(
            &format!(
                "SELECT {PROJECT_MEMBER_COLUMNS} FROM project_members \
                 WHERE project_id = ? AND user_id = ?"
            ),
            args,
        )
        .await?;
    row.map(|r| {
        let role: String = r.try_get("role")?;
        let created_at: i64 = r.try_get("created_at")?;
        let created_by: Option<String> = r.try_get("created_by")?;
        Ok((role, created_at, created_by))
    })
    .transpose()
}

pub async fn is_project_guest(
    conn: &mut Conn<'_>,
    project_id: &str,
    user_id: &str,
) -> Result<bool, AppError> {
    Ok(find_project_member(conn, project_id, user_id)
        .await?
        .is_some())
}

/// Projects explicitly shared with a user (project-guest grants).
pub async fn list_shared_projects(
    conn: &mut Conn<'_>,
    user_id: &str,
) -> Result<Vec<Project>, AppError> {
    let mut args = Args::new();
    args.s(user_id);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {PROJECT_COLUMNS} FROM projects p \
                 JOIN project_members pm ON pm.project_id = p.id \
                 WHERE pm.user_id = ? ORDER BY p.created_at"
            ),
            args,
        )
        .await?;
    rows.iter().map(row_to_project).collect()
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
            is_guest: false,
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
    let (mut project, actor) = require_project_access(&mut conn, &project_id, &user.id).await?;
    project.is_guest = actor.is_guest();
    project.my_role = actor.membership.map(|m| m.role);
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
        is_guest: project.is_guest,
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

// ---------------------------------------------------------------------------
// Project guests (read-only project-scoped access)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ProjectGuest {
    pub user: crate::users::UserBrief,
    pub role: String,
    pub created_at: i64,
    pub granted_by: Option<crate::users::UserBrief>,
}

fn require_guest_manager(actor: &ProjectActor) -> Result<(), AppError> {
    if actor.is_guest() || !actor.can(Capability::ManageProjects) {
        // Guests and unauthorized members learn nothing about the roster.
        return Err(AppError::Forbidden(
            "managing project guests requires the Manager role or above",
        ));
    }
    Ok(())
}

async fn list_guests(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_project_access(&mut conn, &project_id, &user.id)
        .await?
        .1
        .require_team_member()?;
    // Re-check the capability explicitly (guests are never members, so
    // require_team_member already excludes them; members need ManageProjects).
    let mut args = Args::new();
    args.s(&project_id);
    let rows = conn
        .fetch_all(
            "SELECT pm.user_id, pm.role, pm.created_at, pm.created_by, \
                    u.email, u.display_name, \
                    g.email AS granter_email, g.display_name AS granter_name \
             FROM project_members pm \
             JOIN users u ON u.id = pm.user_id \
             LEFT JOIN users g ON g.id = pm.created_by \
             WHERE pm.project_id = ? ORDER BY pm.created_at",
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(|row| {
            Ok(ProjectGuest {
                user: crate::users::UserBrief {
                    id: row.try_get("user_id")?,
                    display_name: row.try_get("display_name")?,
                    email: row.try_get("email")?,
                },
                role: row.try_get("role")?,
                created_at: row.try_get("created_at")?,
                granted_by: {
                    let granter_email: Option<String> = row.try_get("granter_email")?;
                    granter_email.map(|email| {
                        let granter_id: Option<String> = row.try_get("created_by").unwrap_or(None);
                        let granter_name: Option<String> =
                            row.try_get("granter_name").unwrap_or(None);
                        crate::users::UserBrief {
                            id: granter_id.unwrap_or_default(),
                            display_name: granter_name.unwrap_or_default(),
                            email,
                        }
                    })
                },
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct GrantGuestRequest {
    /// Email of an existing registered user.
    pub email: String,
}

async fn grant_guest(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
    Json(body): Json<GrantGuestRequest>,
) -> Result<(StatusCode, Json<ProjectGuest>), AppError> {
    let mut conn = state.db.begin().await?;
    let (project, actor) = require_project_access(&mut conn, &project_id, &user.id).await?;
    require_guest_manager(&actor)?;
    if project.status == "archived" {
        return Err(AppError::Conflict(
            "archived projects cannot be shared".into(),
        ));
    }

    let email = body.email.trim().to_lowercase();
    let target = crate::users::find_active_by_email(&mut conn, &email)
        .await?
        .ok_or_else(|| AppError::validation("email", "no registered user has this email"))?;

    // A user who cannot see the project at all through their team still gets
    // the explicit viewer row; a team member gains nothing new (their team
    // role already governs them), so member grants are pointless but harmless.
    // Duplicates are rejected for clarity.
    if crate::authorization::get_membership(&mut conn, &project.team_id, &target.0.id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(
            "this user is already a team member and does not need guest access".into(),
        ));
    }
    if is_project_guest(&mut conn, &project_id, &target.0.id).await? {
        return Err(AppError::Conflict(
            "this user already has guest access to the project".into(),
        ));
    }

    let mut args = Args::new();
    args.s(&project_id)
        .s(&target.0.id)
        .s("viewer")
        .i(util::now_ms())
        .s(&user.id);
    conn.execute(
        "INSERT INTO project_members (project_id, user_id, role, created_at, created_by) \
         VALUES (?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    conn.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(ProjectGuest {
            user: crate::users::UserBrief {
                id: target.0.id,
                display_name: target.0.display_name,
                email: target.0.email,
            },
            role: "viewer".to_string(),
            created_at: util::now_ms(),
            granted_by: Some(crate::users::UserBrief {
                id: user.id.clone(),
                display_name: user.display_name.clone(),
                email: user.email.clone(),
            }),
        }),
    ))
}

async fn revoke_guest(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((project_id, target_user_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let (_, actor) = require_project_access(&mut conn, &project_id, &user.id).await?;
    require_guest_manager(&actor)?;
    let mut args = Args::new();
    args.s(&project_id).s(&target_user_id);
    let affected = conn
        .execute(
            "DELETE FROM project_members WHERE project_id = ? AND user_id = ?",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::NotFound("guest not found"));
    }
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
