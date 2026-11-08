//! Teams: creation, settings, membership management, and labels.

use axum::extract::{Query, Path, State};
use axum::http::{HeaderMap, StatusCode};
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
        .route(
            "/teams/{team_id}",
            get(get_team).patch(update_team).delete(delete_team),
        )
        .route(
            "/teams/{team_id}/archive",
            axum::routing::post(archive_team),
        )
        .route(
            "/teams/{team_id}/unarchive",
            axum::routing::post(unarchive_team),
        )
        .route("/teams/{team_id}/avatar", get(serve_team_avatar))
        .route(
            "/teams/{team_id}/members",
            get(list_members).post(add_member),
        )
        .route(
            "/teams/{team_id}/members/eligible",
            get(eligible_members),
        )
        .route(
            "/teams/{team_id}/members/{user_id}",
            axum::routing::patch(change_member_role).delete(remove_member),
        )
        .route("/teams/{team_id}/guests", get(list_guests))
        .route(
            "/teams/{team_id}/guests/{user_id}",
            axum::routing::delete(revoke_guest),
        )
        .route(
            "/teams/{team_id}/labels",
            get(list_labels).post(create_label),
        )
        .route(
            "/teams/{team_id}/labels/{label_id}",
            axum::routing::patch(update_label).delete(delete_label),
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
    /// Default timezone for the team (Phase 9). Display-only.
    pub timezone: Option<String>,
    /// Set when the team is archived (Phase 9); NULL while active.
    pub archived_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub my_role: Option<Role>,
}

const TEAM_COLUMNS: &str = "id, name, description, timezone, archived_at, created_at, updated_at";

#[derive(Serialize)]
pub struct MemberResponse {
    pub user: UserBrief,
    /// @mention handle (Phase 9): powers the member search and the mention
    /// resolver without changing the embedded user shape.
    pub username: Option<String>,
    pub role: Role,
    pub created_at: i64,
}

#[derive(Serialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: String,
    /// Optional Lucide icon name (Phase 12 WS5), from the app whitelist.
    pub icon: Option<String>,
    pub created_at: i64,
}

/// Lucide icon names the label picker may use (Phase 12 WS5). The frontend
/// resolves these kebab-case names to `lucide-react` components; anything
/// outside this set is rejected so stored icons always render.
pub const LABEL_ICONS: [&str; 48] = [
    "tag",
    "bookmark",
    "bug",
    "wrench",
    "sparkles",
    "rocket",
    "flame",
    "zap",
    "star",
    "heart",
    "circle-alert",
    "triangle-alert",
    "circle-check",
    "circle-x",
    "clock",
    "calendar",
    "flag",
    "target",
    "package",
    "box",
    "layers",
    "folder",
    "file-text",
    "pencil",
    "pen-tool",
    "message-circle",
    "message-square",
    "users",
    "user",
    "git-branch",
    "git-merge",
    "git-pull-request",
    "shield",
    "shield-alert",
    "shield-check",
    "key-round",
    "lock",
    "unlock",
    "eye",
    "search",
    "compass",
    "map-pin",
    "globe",
    "cloud",
    "database",
    "server",
    "cpu",
    "terminal",
];

/// A stored label icon must be exactly one whitelisted Lucide name.
fn validate_label_icon(icon: &str) -> bool {
    LABEL_ICONS.contains(&icon)
}

fn row_to_team(row: &sqlx::any::AnyRow, my_role: Option<Role>) -> Result<Team, AppError> {
    Ok(Team {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        timezone: row.try_get("timezone")?,
        archived_at: row.try_get("archived_at")?,
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
    if let Some(d) = description
        && d.chars().count() > 1000
    {
        errors.push(FieldError::new(
            "description",
            "description must be at most 1000 characters",
        ));
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

/// Bug 7: the product's primary market anchors the team timezone; every new
/// team defaults to Asia/Tehran and migration 0015 backfilled existing NULLs.
pub const DEFAULT_TEAM_TIMEZONE: &str = "Asia/Tehran";

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
        .s(DEFAULT_TEAM_TIMEZONE)
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO teams (id, name, description, timezone, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
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
            timezone: Some(DEFAULT_TEAM_TIMEZONE.to_string()),
            archived_at: None,
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
            "SELECT t.id, t.name, t.description, t.timezone, t.archived_at, t.created_at, t.updated_at, tm.role \
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
            "timezone": row.try_get::<Option<String>, _>("timezone")?,
            "archived_at": row.try_get::<Option<i64>, _>("archived_at")?,
            "created_at": row.try_get::<i64, _>("created_at")?,
            "updated_at": row.try_get::<i64, _>("updated_at")?,
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
    #[serde(default)]
    pub timezone: Option<String>,
    /// `null` clears the team avatar; a data URL sets it. Never echoed back —
    /// the image is served by `GET /teams/{team_id}/avatar`.
    #[serde(default)]
    pub avatar: Option<Option<String>>,
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

    let timezone = match &body.timezone {
        Some(raw) => Some(crate::profile::validate_timezone(raw)?),
        None => existing.timezone.clone(),
    };
    let avatar_update = if let Some(value) = &body.avatar {
        match value {
            Some(raw) => {
                let validated = crate::avatar::validate_data_url(raw)?;
                Some(format!(
                    "data:{};base64,{}",
                    validated.content_type,
                    crate::profile::encode_avatar_base64(&validated.bytes)
                ))
            }
            None => Some(String::new()), // clear
        }
    } else {
        None
    };

    let mut args = Args::new();
    args.s(&name).opt_s(description.clone()).i(util::now_ms());
    let mut sql = String::from("UPDATE teams SET name = ?, description = ?, updated_at = ?");
    if let Some(tz) = &timezone {
        sql.push_str(", timezone = ?");
        args.s(tz.clone());
    }
    if let Some(encoded) = &avatar_update {
        // An empty string means "clear" (NULL): the portable layer stores
        // NULL so absence stays uniform across engines.
        if encoded.is_empty() {
            sql.push_str(", avatar = NULL");
        } else {
            sql.push_str(", avatar = ?");
            args.s(encoded.clone());
        }
    }
    sql.push_str(" WHERE id = ?");
    args.s(&team_id);
    conn.execute(&sql, args).await?;
    conn.commit().await?;
    Ok(Json(Team {
        id: team_id,
        name,
        description,
        timezone,
        archived_at: existing.archived_at,
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
            "SELECT tm.role, tm.created_at, u.id, u.email, u.display_name, u.username \
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
                username: row.try_get("username")?,
                role: Role::parse(&row.try_get::<String, _>("role")?)?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    /// Email of an **existing instance user** (Bug 2). Tasker accounts are
    /// created by an instance Owner via `POST /admin/users`; team membership
    /// is a separate assignment step.
    pub email: String,
    pub role: String,
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

    // Assignment only: the account must already exist instance-wide. A 422
    // with an actionable message replaces the old team-scoped account
    // creation (ADR-0014).
    let Some((existing, _)) = users::find_active_by_email(&mut conn, &email).await? else {
        return Err(AppError::validation(
            "email",
            "No Tasker user with this email. Ask an Owner to create the account first.",
        ));
    };
    let user_id = existing.id;
    let display_name = existing.display_name;
    let username = existing.username;

    let mut args = Args::new();
    let now = util::now_ms();
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
            username,
            role,
            created_at: now,
        }),
    ))
}

/// Search instance users eligible to join this team (Phase 12.5 Bug 2):
/// existing accounts only, excluding current members. Team Owner/Admin —
/// the same gate as assignment itself.
#[derive(Deserialize)]
pub struct EligibleMembersQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub page_size: Option<i64>,
}

async fn eligible_members(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Query(query): Query<EligibleMembersQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_membership(&mut conn, &team_id, &user.id).await?;
    authorization::require_capability(&mut conn, &team_id, &user.id, Capability::AddMember)
        .await?;

    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * page_size;
    let like = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(crate::util::escape_like)
        .map(|q| format!("%{q}%"));

    let sql_where = "FROM users u          WHERE u.is_active            AND NOT EXISTS (SELECT 1 FROM team_members tm                            WHERE tm.team_id = ? AND tm.user_id = u.id)";
    let total: i64 = match &like {
        Some(pattern) => {
            let mut args = Args::new();
            args.s(&team_id).s(pattern.clone());
            conn.fetch_optional(
                &format!("SELECT COUNT(*) AS n {sql_where}                           AND (u.email LIKE ? ESCAPE '|' OR u.display_name LIKE ? ESCAPE '|' OR u.username LIKE ? ESCAPE '|')"),
                {
                    let mut a = Args::new();
                    a.s(&team_id).s(pattern.clone()).s(pattern.clone()).s(pattern.clone());
                    a
                },
            )
            .await?
            .map_or(0, |r| r.try_get("n").unwrap_or(0))
        }
        None => {
            let mut args = Args::new();
            args.s(&team_id);
            conn.fetch_optional(&format!("SELECT COUNT(*) AS n {sql_where}"), args)
                .await?
                .map_or(0, |r| r.try_get("n").unwrap_or(0))
        }
    };

    let rows = match &like {
        Some(pattern) => {
            let mut args = Args::new();
            args.s(&team_id).s(pattern.clone()).s(pattern.clone()).s(pattern.clone());
            args.i(page_size).i(offset);
            conn.fetch_all(
                &format!(
                    "SELECT u.id, u.email, u.display_name, u.username {sql_where}                      AND (u.email LIKE ? ESCAPE '|' OR u.display_name LIKE ? ESCAPE '|' OR u.username LIKE ? ESCAPE '|')                      ORDER BY u.display_name LIMIT ? OFFSET ?"
                ),
                args,
            )
            .await?
        }
        None => {
            let mut args = Args::new();
            args.s(&team_id).i(page_size).i(offset);
            conn.fetch_all(
                &format!(
                    "SELECT u.id, u.email, u.display_name, u.username {sql_where}                      ORDER BY u.display_name LIMIT ? OFFSET ?"
                ),
                args,
            )
            .await?
        }
    };

    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            Ok(serde_json::json!({
                "id": row.try_get::<String, _>("id")?,
                "email": row.try_get::<String, _>("email")?,
                "display_name": row.try_get::<String, _>("display_name")?,
                "username": row.try_get::<Option<String>, _>("username")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(Json(serde_json::json!({
        "items": items,
        "page": page,
        "page_size": page_size,
        "total": total,
    })))
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

    if !matches!(actor.role, Role::Owner | Role::Admin) {
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
    if actor.role != Role::Owner && (current_role == Role::Owner || new_role == Role::Owner) {
        return Err(AppError::Forbidden(
            "only the team owner can manage owner-level roles",
        ));
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
        user: target.clone().map_or_else(
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
        username: target.and_then(|u| u.username),
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
// Archive / delete (Phase 9)
// ---------------------------------------------------------------------------

/// Archive is reversible; the flag only marks the team, it does not alter any
/// workflow or authorization behavior.
async fn set_team_archived(
    state: &AppState,
    user: &AuthedUser,
    team_id: &str,
    archived: bool,
) -> Result<Json<Team>, AppError> {
    let mut conn = state.db.begin().await?;
    require_membership(&mut conn, team_id, &user.id).await?;
    authorization::require_capability(&mut conn, team_id, &user.id, Capability::UpdateTeam).await?;
    let existing = find_team(&mut conn, team_id).await?;
    let mut args = Args::new();
    args.opt_i(if archived { Some(util::now_ms()) } else { None })
        .i(util::now_ms())
        .s(team_id);
    conn.execute(
        "UPDATE teams SET archived_at = ?, updated_at = ? WHERE id = ?",
        args,
    )
    .await?;
    conn.commit().await?;
    Ok(Json(Team {
        archived_at: if archived { Some(util::now_ms()) } else { None },
        ..existing
    }))
}

async fn archive_team(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
) -> Result<Json<Team>, AppError> {
    set_team_archived(&state, &user, &team_id, true).await
}

async fn unarchive_team(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
) -> Result<Json<Team>, AppError> {
    set_team_archived(&state, &user, &team_id, false).await
}

#[derive(Deserialize)]
pub struct DeleteTeamRequest {
    /// The team name, typed by the caller as a deliberate confirmation.
    pub confirm_name: String,
    /// The caller's current password (re-verification for an irreversible act).
    pub current_password: String,
}

/// Hard-delete a team. Owner-only, and only after re-verifying the caller's
/// password and the typed team name. The rules-engine rows are removed first
/// because migration 0009 intentionally RESTRICTs their team references.
///
/// Deletion order (single transaction; every step is team-scoped):
/// 1. detach the `domain_events.causing_execution_id` cycle edge,
/// 2. `rule_action_results`, `automation_outbox`, `rule_executions`,
/// 3. `domain_events`, `rule_versions`, `rules`,
/// 4. the team row — engine-level CASCADEs remove members, projects, labels,
///    tasks (with comments, notifications, dependencies, templates), and the
///    goal links (goals reference teams with ON DELETE CASCADE).
async fn delete_team(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<DeleteTeamRequest>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let membership = require_membership(&mut conn, &team_id, &user.id).await?;
    if membership.role != Role::Owner {
        return Err(AppError::Forbidden(
            "only the team owner can delete the team",
        ));
    }
    let team = find_team(&mut conn, &team_id).await?;
    if body.confirm_name.trim() != team.name {
        return Err(AppError::validation(
            "confirm_name",
            "the typed name does not match the team name",
        ));
    }

    let found = users::find_active_by_email(&mut conn, &user.email).await?;
    let Some((_, password_hash)) = found else {
        return Err(AppError::Unauthenticated);
    };
    if !crate::security::password::verify(&body.current_password, &password_hash) {
        return Err(AppError::validation(
            "current_password",
            "current password is incorrect",
        ));
    }

    // Args is positional and single-use; each statement gets a fresh binding.
    let mut args = Args::new();
    args.s(&team_id);
    conn.execute(
        "UPDATE domain_events SET causing_execution_id = NULL WHERE team_id = ?",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.s(&team_id);
    conn.execute(
        "DELETE FROM rule_action_results WHERE execution_id IN \
         (SELECT id FROM rule_executions WHERE team_id = ?)",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.s(&team_id);
    conn.execute(
        "DELETE FROM automation_outbox WHERE execution_id IN \
         (SELECT id FROM rule_executions WHERE team_id = ?)",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.s(&team_id);
    conn.execute("DELETE FROM rule_executions WHERE team_id = ?", args)
        .await?;
    let mut args = Args::new();
    args.s(&team_id);
    conn.execute("DELETE FROM domain_events WHERE team_id = ?", args)
        .await?;
    let mut args = Args::new();
    args.s(&team_id);
    conn.execute(
        "DELETE FROM rule_versions WHERE rule_id IN (SELECT id FROM rules WHERE team_id = ?)",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.s(&team_id);
    conn.execute("DELETE FROM rules WHERE team_id = ?", args)
        .await?;
    let mut args = Args::new();
    args.s(&team_id);
    conn.execute("DELETE FROM teams WHERE id = ?", args).await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Guests (team-wide view over project_members viewers)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct TeamGuest {
    pub user: UserBrief,
    pub project_id: String,
    pub project_name: String,
    pub created_at: i64,
}

async fn list_guests(
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
            "SELECT pm.user_id, pm.created_at, u.email, u.display_name, p.id AS project_id, p.name AS project_name \
             FROM project_members pm \
             JOIN projects p ON p.id = pm.project_id \
             JOIN users u ON u.id = pm.user_id \
             WHERE p.team_id = ? AND pm.role = 'viewer' \
             ORDER BY p.name, u.display_name",
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(|row| {
            Ok(TeamGuest {
                user: UserBrief {
                    id: row.try_get("user_id")?,
                    display_name: row.try_get("display_name")?,
                    email: row.try_get("email")?,
                },
                project_id: row.try_get("project_id")?,
                project_name: row.try_get("project_name")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// Revoke a guest from every project in the team (Manager capability or
/// above, matching the project-level guest manager rule).
async fn revoke_guest(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, target_user_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    require_membership(&mut conn, &team_id, &user.id).await?;
    authorization::require_capability(&mut conn, &team_id, &user.id, Capability::ManageProjects)
        .await?;
    let mut args = Args::new();
    args.s(&target_user_id).s(&team_id);
    let affected = conn
        .execute(
            "DELETE FROM project_members WHERE user_id = ? AND role = 'viewer' \
             AND project_id IN (SELECT id FROM projects WHERE team_id = ?)",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::NotFound("guest not found in this team"));
    }
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Serve the team avatar as image bytes (same contract as user avatars).
async fn serve_team_avatar(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
) -> Result<(StatusCode, HeaderMap, Vec<u8>), AppError> {
    let mut conn = state.db.conn();
    require_membership(&mut conn, &team_id, &user.id).await?;
    // MEDIUMTEXT on MySQL decodes as Blob through sqlx Any; CONVERT yields
    // a wire VARCHAR (Text kind). Same rationale as users::find_avatar.
    let sql = match conn.driver() {
        crate::db::Driver::MySql => {
            "SELECT CONVERT(avatar USING utf8mb4) AS avatar FROM teams WHERE id = ?"
        }
        _ => "SELECT avatar FROM teams WHERE id = ?",
    };
    let mut args = Args::new();
    args.s(&team_id);
    let row = conn
        .fetch_optional(sql, args)
        .await?
        .ok_or(AppError::NotFound("team not found"))?;
    let raw: Option<String> = row.try_get("avatar")?;
    let Some(raw) = raw else {
        return Err(AppError::NotFound("avatar not found"));
    };
    let validated = crate::avatar::validate_data_url(&raw)
        .map_err(|_| AppError::NotFound("avatar not found"))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        validated
            .content_type
            .parse()
            .map_err(|_| AppError::internal("invalid content type"))?,
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::header::HeaderValue::from_static("private, max-age=300"),
    );
    Ok((StatusCode::OK, headers, validated.bytes))
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
            "SELECT id, name, color, icon, created_at FROM labels WHERE team_id = ? ORDER BY name",
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
                icon: row.try_get("icon")?,
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
    #[serde(default)]
    pub icon: Option<String>,
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
    if let Some(icon) = &body.icon {
        if !validate_label_icon(icon) {
            return Err(AppError::validation("icon", "unknown label icon"));
        }
    }

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&team_id)
        .s(&name)
        .s(&color)
        .opt_s(body.icon.clone())
        .i(now);
    if let Err(e) = conn
        .execute(
            "INSERT INTO labels (id, team_id, name, color, icon, created_at) VALUES (?, ?, ?, ?, ?, ?)",
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
            icon: body.icon,
            created_at: now,
        }),
    ))
}

/// Update a team label (name/color/icon). Reuses the CreateLabel capability:
/// label management is an owner/admin action.
#[derive(Deserialize)]
pub struct UpdateLabelRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    /// `null` clears the icon; a whitelisted name sets it.
    #[serde(default)]
    pub icon: Option<Option<String>>,
}

async fn update_label(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, label_id)): Path<(String, String)>,
    Json(body): Json<UpdateLabelRequest>,
) -> Result<Json<Label>, AppError> {
    let mut conn = state.db.begin().await?;
    require_membership(&mut conn, &team_id, &user.id).await?;
    authorization::require_capability(&mut conn, &team_id, &user.id, Capability::CreateLabel)
        .await?;

    let mut args = Args::new();
    args.s(&label_id).s(&team_id);
    let row = conn
        .fetch_optional(
            "SELECT id, name, color, icon, created_at FROM labels WHERE id = ? AND team_id = ?",
            args,
        )
        .await?
        .ok_or(AppError::NotFound("label not found"))?;
    let mut label = Label {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        color: row.try_get("color")?,
        icon: row.try_get("icon")?,
        created_at: row.try_get("created_at")?,
    };

    if let Some(name) = &body.name {
        let name = name.trim().to_string();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(AppError::validation(
                "name",
                "label name must be 1-80 characters",
            ));
        }
        label.name = name;
    }
    if let Some(color) = &body.color {
        if !validate_label_color(color) {
            return Err(AppError::validation(
                "color",
                "color must be a hex value like #6366f1",
            ));
        }
        label.color = color.clone();
    }
    if let Some(icon) = &body.icon {
        if let Some(icon) = icon {
            if !validate_label_icon(icon) {
                return Err(AppError::validation("icon", "unknown label icon"));
            }
            label.icon = Some(icon.clone());
        } else {
            label.icon = None;
        }
    }

    let mut args = Args::new();
    args.s(&label.name)
        .s(&label.color)
        .opt_s(label.icon.clone())
        .s(&label_id);
    conn.execute(
        "UPDATE labels SET name = ?, color = ?, icon = ? WHERE id = ?",
        args,
    )
    .await?;
    conn.commit().await?;
    Ok(Json(label))
}

/// Delete a team label. `task_labels` rows cascade via FK, so tasks simply
/// lose the label — no task data is touched.
async fn delete_label(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, label_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    require_membership(&mut conn, &team_id, &user.id).await?;
    authorization::require_capability(&mut conn, &team_id, &user.id, Capability::CreateLabel)
        .await?;
    let mut args = Args::new();
    args.s(&label_id).s(&team_id);
    let deleted = conn
        .execute("DELETE FROM labels WHERE id = ? AND team_id = ?", args)
        .await?;
    if deleted == 0 {
        return Err(AppError::NotFound("label not found"));
    }
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
