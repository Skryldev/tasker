//! Teams: creation, settings, membership management, and labels.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::authorization::{self, Capability, Role};
use crate::db::{Args, Conn, map_unique_violation};
use crate::error::{AppError, FieldError};
use crate::state::AppState;
use crate::users::{self, UserBrief};
use crate::util;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/teams", get(list_teams).post(create_team))
        .route("/teams/{team_id}", get(get_team).patch(update_team))
        .route(
            "/teams/{team_id}/members",
            get(list_members).post(add_member),
        )
        .route(
            "/teams/{team_id}/members/{user_id}",
            axum::routing::patch(change_member_role).delete(remove_member),
        )
        .route(
            "/teams/{team_id}/labels",
            get(list_labels).post(create_label),
        )
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub my_role: Option<Role>,
}

const TEAM_COLUMNS: &str = "id, name, description, created_at, updated_at";

#[derive(Serialize)]
pub struct MemberResponse {
    pub user: UserBrief,
    pub role: Role,
    pub created_at: i64,
}

#[derive(Serialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: String,
    pub created_at: i64,
}

fn row_to_team(row: &sqlx::any::AnyRow, my_role: Option<Role>) -> Result<Team, AppError> {
    Ok(Team {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        my_role,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub async fn find_team(conn: &mut Conn<'_>, team_id: &str) -> Result<Team, AppError> {
    let mut args = Args::new();
    args.s(team_id);
    let row = conn
        .fetch_optional(
            &format!("SELECT {TEAM_COLUMNS} FROM teams WHERE id = ?"),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("team not found"))?;
    row_to_team(&row, None)
}

/// Resolve the caller's membership for a team in the URL, 404 when absent.
pub async fn require_membership(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
) -> Result<authorization::Membership, AppError> {
    authorization::get_membership(conn, team_id, user_id)
        .await?
        .ok_or(AppError::NotFound("team not found"))
}

fn validate_team_input(
    name: &str,
    description: Option<&str>,
) -> Result<(String, Option<String>), AppError> {
    let mut errors = Vec::new();
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 {
        errors.push(FieldError::new(
            "name",
            "team name must be 1-120 characters",
        ));
    }
    let description = description.map(str::trim).filter(|d| !d.is_empty());
    if let Some(d) = description {
        if d.chars().count() > 1000 {
            errors.push(FieldError::new(
                "description",
                "description must be at most 1000 characters",
            ));
        }
    }
    if errors.is_empty() {
        Ok((name, description.map(std::string::ToString::to_string)))
    } else {
        Err(AppError::Validation(errors))
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

async fn create_team(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(body): Json<CreateTeamRequest>,
) -> Result<(StatusCode, Json<Team>), AppError> {
    let (name, description) = validate_team_input(&body.name, body.description.as_deref())?;
    let mut conn = state.db.begin().await?;

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&name)
        .opt_s(description.clone())
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO teams (id, name, description, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.s(&id).s(&user.id).s(Role::Owner.as_str()).i(now);
    conn.execute(
        "INSERT INTO team_members (team_id, user_id, role, created_at) VALUES (?, ?, ?, ?)",
        args,
    )
    .await?;
    conn.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(Team {
            id,
            name,
            description,
            created_at: now,
            updated_at: now,
            my_role: Some(Role::Owner),
        }),
    ))
}

async fn list_teams(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let mut args = Args::new();
    args.s(&user.id);
    let rows = conn
        .fetch_all(
            "SELECT t.id, t.name, t.description, t.created_at, t.updated_at, tm.role \
             FROM teams t JOIN team_members tm ON tm.team_id = t.id \
             WHERE tm.user_id = ? ORDER BY t.created_at",
            args,
        )
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(serde_json::json!({
            "id": row.try_get::<String, _>("id")?,
            "name": row.try_get::<String, _>("name")?,
            "description": row.try_get::<Option<String>, _>("description")?,
            "created_at": row.try_get::<i64, _>("t_created")?,
            "updated_at": row.try_get::<i64, _>("t_updated")?,
            "my_role": Role::parse(&row.try_get::<String, _>("role")?)?,
        }));
    }
    Ok(Json(serde_json::json!({ "items": items })))
}

async fn get_team(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
) -> Result<Json<Team>, AppError> {
    let mut conn = state.db.conn();
    let membership = require_membership(&mut conn, &team_id, &user.id).await?;
    let mut team = find_team(&mut conn, &team_id).await?;
    team.my_role = Some(membership.role);
    Ok(Json(team))
}

#[derive(Deserialize)]
pub struct UpdateTeamRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

async fn update_team(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<UpdateTeamRequest>,
) -> Result<Json<Team>, AppError> {
    let mut conn = state.db.begin().await?;
    require_membership(&mut conn, &team_id, &user.id).await?;
    authorization::require_capability(&mut conn, &team_id, &user.id, Capability::UpdateTeam)
        .await?;
    let existing = find_team(&mut conn, &team_id).await?;
    let (name, description) = validate_team_input(
        body.name.as_deref().unwrap_or(&existing.name),
        body.description
            .as_deref()
            .or(existing.description.as_deref()),
    )?;
    let mut args = Args::new();
    args.s(&name)
        .opt_s(description.clone())
        .i(util::now_ms())
        .s(&team_id);
    conn.execute(
        "UPDATE teams SET name = ?, description = ?, updated_at = ? WHERE id = ?",
        args,
    )
    .await?;
    conn.commit().await?;
    Ok(Json(Team {
        id: team_id,
        name,
        description,
        created_at: existing.created_at,
        updated_at: util::now_ms(),
        my_role: existing.my_role,
    }))
}

async fn list_members(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_membership(&mut conn, &team_id, &user.id).await?;
    let mut args = Args::new();
    args.s(&team_id);
    let rows = conn
        .fetch_all(
            "SELECT tm.role, tm.created_at, u.id, u.email, u.display_name \
             FROM team_members tm JOIN users u ON u.id = tm.user_id \
             WHERE tm.team_id = ? ORDER BY tm.created_at",
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(|row| {
            Ok(MemberResponse {
                user: UserBrief {
                    id: row.try_get("id")?,
                    email: row.try_get("email")?,
                    display_name: row.try_get("display_name")?,
                },
                role: Role::parse(&row.try_get::<String, _>("role")?)?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub role: String,
    /// Required only when the user does not exist yet (admin-assisted
    /// provisioning; the administrator hands credentials to the member
    /// out-of-band). Email invitations are planned for a later phase.
    #[serde(default)]
    pub temp_password: Option<String>,
}

async fn add_member(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<AddMemberRequest>,
) -> Result<(StatusCode, Json<MemberResponse>), AppError> {
    let role = Role::parse(&body.role)?;
    if role == Role::Owner {
        // Ownership is granted only by an existing owner promoting a member.
        let mut conn = state.db.conn();
        let membership = require_membership(&mut conn, &team_id, &user.id).await?;
        if membership.role != Role::Owner {
            return Err(AppError::Forbidden(
                "only the team owner can grant the owner role",
            ));
        }
    } else {
        let mut conn = state.db.conn();
        require_membership(&mut conn, &team_id, &user.id).await?;
        authorization::require_capability(&mut conn, &team_id, &user.id, Capability::AddMember)
            .await?;
    }

    let email = body.email.trim().to_lowercase();
    let mut conn = state.db.begin().await?;

    let now = util::now_ms();
    let (user_id, display_name) = match users::find_active_by_email(&mut conn, &email).await? {
        Some((existing, _)) => (existing.id, existing.display_name),
        None => {
            let temp_password = body.temp_password.clone().unwrap_or_default();
            let new_user = users::create_validated(
                &mut conn,
                &email,
                body.display_name
                    .as_deref()
                    .unwrap_or_else(|| email.split('@').next().unwrap_or("member")),
                &temp_password,
            )
            .await?;
            (new_user.id, new_user.display_name)
        }
    };

    let mut args = Args::new();
    args.s(&team_id).s(&user_id).s(role.as_str()).i(now);
    if let Err(e) = conn
        .execute(
            "INSERT INTO team_members (team_id, user_id, role, created_at) VALUES (?, ?, ?, ?)",
            args,
        )
        .await
    {
        return Err(map_unique_violation(
            e,
            "this user is already a team member".into(),
        ));
    }
    conn.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(MemberResponse {
            user: UserBrief {
                id: user_id,
                email,
                display_name,
            },
            role,
            created_at: now,
        }),
    ))
}

#[derive(Deserialize)]
pub struct ChangeRoleRequest {
    pub role: String,
}

async fn change_member_role(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, target_user_id)): Path<(String, String)>,
    Json(body): Json<ChangeRoleRequest>,
) -> Result<Json<MemberResponse>, AppError> {
    let new_role = Role::parse(&body.role)?;
    let mut conn = state.db.begin().await?;
    let actor = require_membership(&mut conn, &team_id, &user.id).await?;

    let is_authorized = match actor.role {
        Role::Owner => true,
        Role::Admin => true,
        _ => false,
    };
    if !is_authorized {
        return Err(AppError::Forbidden("your role does not permit this action"));
    }

    let mut args = Args::new();
    args.s(&team_id).s(&target_user_id);
    let target_row = conn
        .fetch_optional(
            "SELECT role, created_at, user_id FROM team_members WHERE team_id = ? AND user_id = ?",
            args,
        )
        .await?
        .ok_or(AppError::NotFound("member not found"))?;
    let current_role = Role::parse(&target_row.try_get::<String, _>("role")?)?;
    let member_since: i64 = target_row.try_get("created_at")?;

    if target_user_id == user.id {
        return Err(AppError::BadRequest(
            "you cannot change your own role".into(),
        ));
    }
    if actor.role != Role::Owner {
        if current_role == Role::Owner || new_role == Role::Owner {
            return Err(AppError::Forbidden(
                "only the team owner can manage owner-level roles",
            ));
        }
    }
    if current_role == Role::Owner && new_role != Role::Owner {
        let owners = authorization::count_owners(&mut conn, &team_id).await?;
        if owners <= 1 {
            return Err(AppError::Conflict(
                "cannot demote the last owner of the team".into(),
            ));
        }
    }

    let mut args = Args::new();
    args.s(new_role.as_str()).s(&team_id).s(&target_user_id);
    conn.execute(
        "UPDATE team_members SET role = ? WHERE team_id = ? AND user_id = ?",
        args,
    )
    .await?;
    conn.commit().await?;

    let mut conn = state.db.conn();
    let target = users::find_by_id(&mut conn, &target_user_id).await?;
    Ok(Json(MemberResponse {
        user: target.map_or_else(
            || UserBrief {
                id: target_user_id.clone(),
                email: String::new(),
                display_name: String::new(),
            },
            |u| UserBrief {
                id: u.id,
                email: u.email,
                display_name: u.display_name,
            },
        ),
        role: new_role,
        created_at: member_since,
    }))
}

async fn remove_member(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, target_user_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let actor = require_membership(&mut conn, &team_id, &user.id).await?;
    let mut args = Args::new();
    args.s(&team_id).s(&target_user_id);
    let target_row = conn
        .fetch_optional(
            "SELECT role FROM team_members WHERE team_id = ? AND user_id = ?",
            args,
        )
        .await?
        .ok_or(AppError::NotFound("member not found"))?;
    let target_role = Role::parse(&target_row.try_get::<String, _>("role")?)?;

    if target_user_id == user.id {
        return Err(AppError::BadRequest(
            "you cannot remove yourself; ask another admin or the owner".into(),
        ));
    }
    let authorized = match actor.role {
        Role::Owner => true,
        Role::Admin => !matches!(target_role, Role::Owner | Role::Admin),
        _ => false,
    };
    if !authorized {
        return Err(AppError::Forbidden("your role does not permit this action"));
    }
    if target_role == Role::Owner {
        let owners = authorization::count_owners(&mut conn, &team_id).await?;
        if owners <= 1 {
            return Err(AppError::Conflict(
                "cannot remove the last owner of the team".into(),
            ));
        }
    }
    let mut args = Args::new();
    args.s(&team_id).s(&target_user_id);
    conn.execute(
        "DELETE FROM team_members WHERE team_id = ? AND user_id = ?",
        args,
    )
    .await?;
    // Tasks assigned to the removed member are unassigned so work stays visible.
    let mut args = Args::new();
    args.opt_i(None::<i64>).s(&team_id).s(&target_user_id);
    conn.execute(
        "UPDATE tasks SET assignee_id = NULL, updated_at = updated_at WHERE team_id = ? AND assignee_id = ?",
        args,
    )
    .await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

async fn list_labels(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_membership(&mut conn, &team_id, &user.id).await?;
    let mut args = Args::new();
    args.s(&team_id);
    let rows = conn
        .fetch_all(
            "SELECT id, name, color, created_at FROM labels WHERE team_id = ? ORDER BY name",
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(|row| {
            Ok(Label {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                color: row.try_get("color")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct CreateLabelRequest {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}

fn validate_label_color(color: &str) -> bool {
    let bytes = color.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

async fn create_label(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<CreateLabelRequest>,
) -> Result<(StatusCode, Json<Label>), AppError> {
    let mut conn = state.db.begin().await?;
    require_membership(&mut conn, &team_id, &user.id).await?;
    authorization::require_capability(&mut conn, &team_id, &user.id, Capability::CreateLabel)
        .await?;

    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(AppError::validation(
            "name",
            "label name must be 1-80 characters",
        ));
    }
    let color = body.color.unwrap_or_else(|| "#6366f1".to_string());
    if !validate_label_color(&color) {
        return Err(AppError::validation(
            "color",
            "color must be a hex value like #6366f1",
        ));
    }

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id).s(&team_id).s(&name).s(&color).i(now);
    if let Err(e) = conn
        .execute(
            "INSERT INTO labels (id, team_id, name, color, created_at) VALUES (?, ?, ?, ?, ?)",
            args,
        )
        .await
    {
        return Err(map_unique_violation(
            e,
            "a label with this name already exists".into(),
        ));
    }
    conn.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(Label {
            id,
            name,
            color,
            created_at: now,
        }),
    ))
}
