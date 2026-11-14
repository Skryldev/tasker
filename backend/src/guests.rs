//! Team guest role (Phase 13 Bug 7, ADR-0018).
//!
//! A Guest is a non-member with an **explicitly granted** permission set.
//! Deny by default: a request from a guest without the matching grant
//! fails with 404 (non-disclosure), identical to a request from a
//! stranger. The model coexists with Phase 3 project viewers
//! (`project_members` rows) — those are read-only companions on one
//! project; guests are team-scoped citizens whose every capability is a
//! row in `team_guest_permissions`.
//!
//! Permission → surface mapping (audited in ADR-0018, granular since
//! Phase 14 WS2 / ADR-0021):
//!   `view_team`              GET /teams/{id} (metadata only; team scope)
//!   `view_project_list`      GET /teams/{id}/projects (team scope)
//!   `view_project`           GET /projects/{id} (project scope)
//!   `view_project_tasks`     GET /projects/{id}/tasks (project scope)
//!   `view_project_board`     GET /projects/{id}/board (project scope)
//!   `view_project_timeline`  list_tasks with `view=timeline` (project scope)
//!   `view_task`              GET /tasks/{id} base fields (project scope)
//!   `view_task_own`          like view_task, but only assigned tasks
//!   `view_task_dates`        due_date/deadline inside task payloads
//!   `view_task_comments`     GET /tasks/{id}/comments (guest-visible only)
//!   `view_task_activity`     GET /tasks/{id}/activity
//!   `comment_task`           POST /tasks/{id}/comments
//!   `view_task_attachments`  GET /tasks/{id}/attachments
//!   `download_task_attachments`  GET /attachments/{id}
//!   `upload_task_attachments`    POST /tasks/{id}/attachments
//!   `invite_guest`           POST /teams/{id}/guests/invite-requests
//!   `view_team_members` / `view_team_activity` / `view_audit_log`
//!                            team-scope model grants (no surface yet —
//!                            ADR-0021 documents the forward compatibility)
//! Everything else keeps `require_membership` — guests fail it with 404.

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
use crate::users::UserBrief;
use crate::util;

/// Granular RBAC v2 permission set (Phase 14 WS2, ADR-0021), grouped by
/// domain. Only `blocks`-style dependency semantics do not apply here;
/// deny-by-default governs everything.
pub const GUEST_PERMISSIONS_TEAM: [&str; 3] = [
    "view_team",
    "view_team_members",
    "view_team_activity",
];
pub const GUEST_PERMISSIONS_PROJECT: [&str; 8] = [
    "view_project_list",
    "view_project",
    "view_project_tasks",
    "view_project_timeline",
    "view_project_board",
    "view_task_dates",
    "view_task_activity",
    "view_task_comments",
];
pub const GUEST_PERMISSIONS_TASK: [&str; 4] = [
    "view_task",
    "view_task_own",
    "comment_task",
    "view_task_attachments",
];
pub const GUEST_PERMISSIONS_ADMIN: [&str; 4] = [
    "download_task_attachments",
    "upload_task_attachments",
    "invite_guest",
    "view_audit_log",
];

/// Every grantable permission (flat view of the domain groups).
pub const GUEST_PERMISSIONS: [&str; 19] = [
    "view_team",
    "view_team_members",
    "view_team_activity",
    "view_project_list",
    "view_project",
    "view_project_tasks",
    "view_project_timeline",
    "view_project_board",
    "view_task_dates",
    "view_task_activity",
    "view_task_comments",
    "view_task",
    "view_task_own",
    "comment_task",
    "view_task_attachments",
    "download_task_attachments",
    "upload_task_attachments",
    "invite_guest",
    "view_audit_log",
];

#[must_use]
pub fn is_valid_permission(p: &str) -> bool {
    GUEST_PERMISSIONS.contains(&p)
}

/// Permissions whose rows are team-scoped (`scope_type = "team"`, no
/// scope id). Everything else is project-scoped and requires `project_id`.
pub const TEAM_SCOPED_PERMISSIONS: [&str; 6] = [
    "view_team",
    "view_team_members",
    "view_team_activity",
    "view_project_list",
    "invite_guest",
    "view_audit_log",
];

#[must_use]
pub fn is_team_scoped(p: &str) -> bool {
    TEAM_SCOPED_PERMISSIONS.contains(&p)
}

/// An active guest registration plus its grants.
#[derive(Debug, Clone, Serialize)]
pub struct GuestGrant {
    pub guest_id: String,
    pub team_id: String,
    /// Permissions the guest holds: `("view_team", None)` or
    /// `("view_project", Some(project_id))`.
    pub permissions: Vec<(String, Option<String>)>,
}

/// Resolve the caller's active guest registration for a team, with its
/// grants. `None` = the caller is not an active guest of this team.
pub async fn resolve_guest(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
) -> Result<Option<GuestGrant>, AppError> {
    let mut args = Args::new();
    args.s(team_id).s(user_id);
    let guest = conn
        .fetch_optional(
            "SELECT g.id FROM team_guests g \
             WHERE g.team_id = ? AND g.user_id = ? AND g.revoked_at IS NULL",
            args,
        )
        .await?;
    let Some(guest_row) = guest else {
        return Ok(None);
    };
    let guest_id: String = guest_row.try_get("id")?;

    let mut args = Args::new();
    args.s(&guest_id);
    let rows = conn
        .fetch_all(
            "SELECT permission, scope_type, scope_id FROM team_guest_permissions \
             WHERE guest_id = ?",
            args,
        )
        .await?;
    let permissions = rows
        .iter()
        .map(|row| {
            let scope: String = row.try_get("scope_type")?;
            let scope_id: Option<String> = row.try_get("scope_id")?;
            Ok((
                row.try_get::<String, _>("permission")?,
                if scope == "project" { scope_id } else { None },
            ))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Some(GuestGrant {
        guest_id,
        team_id: team_id.to_string(),
        permissions,
    }))
}

/// Deny-by-default permission check for an active guest.
pub async fn has_permission(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
    permission: &str,
    project_scope: Option<&str>,
) -> Result<bool, AppError> {
    let Some(grant) = resolve_guest(conn, team_id, user_id).await? else {
        return Ok(false);
    };
    Ok(grant.permissions.iter().any(|(p, scope)| {
        p == permission
            && match (permission, scope) {
                // Team-scoped grants carry no project.
                ("view_team", _) => true,
                // Project-scoped grants must match the project (or a
                // team-wide "all projects" row with scope_id NULL).
                (_, None) => true,
                (_, Some(s)) => project_scope.is_some_and(|ps| ps == s),
            }
    }))
}

/// The gate used by audited endpoints: an active guest with the grant
/// passes; everyone else's membership check happens at the call site (this
/// helper only ADDS the guest path — it never widens member access).
pub async fn guest_has(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
    permission: &str,
    project_scope: Option<&str>,
) -> Result<bool, AppError> {
    has_permission(conn, team_id, user_id, permission, project_scope).await
}

// ---------------------------------------------------------------------------
// Management endpoints (Owner/Admin)
// ---------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/teams/{team_id}/guests2",
            get(list_guests).post(invite_guest),
        )
        .route(
            "/teams/{team_id}/guests2/{guest_id}",
            axum::routing::delete(revoke_guest),
        )
        .route(
            "/teams/{team_id}/guests2/{guest_id}/permissions",
            get(list_guest_permissions).post(grant_permission),
        )
        .route(
            "/teams/{team_id}/guests2/{guest_id}/permissions/{permission_id}",
            axum::routing::delete(revoke_permission),
        )
        // Two-step guest invitation (Phase 14 WS2, ADR-0021): a guest with
        // the `invite_guest` grant requests an invitation; Owner/Admin
        // approve (which creates the guest registration) or reject it.
        .route(
            "/teams/{team_id}/guests/invite-requests",
            get(list_invite_requests).post(create_invite_request),
        )
        .route(
            "/teams/{team_id}/guests/invite-requests/{request_id}/approve",
            axum::routing::post(approve_invite_request),
        )
        .route(
            "/teams/{team_id}/guests/invite-requests/{request_id}/reject",
            axum::routing::post(reject_invite_request),
        )
}

/// Owner/Admin gate for guest management (matches the member-management
/// rule; Managers cannot mint guests).
async fn require_guest_manager(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
) -> Result<crate::authorization::Membership, AppError> {
    let membership = crate::teams::require_membership(conn, team_id, user_id).await?;
    if !matches!(
        membership.role,
        crate::authorization::Role::Owner | crate::authorization::Role::Admin
    ) {
        return Err(AppError::NotFound("team not found"));
    }
    Ok(membership)
}

#[derive(Serialize)]
pub struct TeamGuestRow {
    pub guest_id: String,
    pub user: UserBrief,
    pub created_at: i64,
    pub permissions: Vec<GuestPermissionRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GuestPermissionRow {
    pub id: String,
    pub permission: String,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub created_at: i64,
}

async fn list_guests(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    let mut args = Args::new();
    args.s(&team_id);
    let rows = conn
        .fetch_all(
            "SELECT g.id AS guest_id, g.created_at, u.id AS user_id, u.email, u.display_name \
             FROM team_guests g \
             JOIN users u ON u.id = g.user_id \
             WHERE g.team_id = ? AND g.revoked_at IS NULL \
             ORDER BY g.created_at",
            args,
        )
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let guest_id: String = row.try_get("guest_id")?;
        let mut args = Args::new();
        args.s(&guest_id);
        let perm_rows = conn
            .fetch_all(
                "SELECT id, permission, scope_type, scope_id, created_at \
                 FROM team_guest_permissions WHERE guest_id = ? ORDER BY created_at",
                args,
            )
            .await?;
        items.push(TeamGuestRow {
            guest_id: guest_id.clone(),
            user: UserBrief {
                id: row.try_get("user_id")?,
                email: row.try_get("email")?,
                display_name: row.try_get("display_name")?,
            },
            created_at: row.try_get("created_at")?,
            permissions: perm_rows
                .iter()
                .map(|p| {
                    Ok::<GuestPermissionRow, AppError>(GuestPermissionRow {
                        id: p.try_get("id")?,
                        permission: p.try_get("permission")?,
                        scope_type: p.try_get("scope_type")?,
                        scope_id: p.try_get("scope_id")?,
                        created_at: p.try_get("created_at")?,
                    })
                })
                .collect::<Result<Vec<_>, AppError>>()?,
        });
    }
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct InviteGuestRequest {
    /// An existing instance user (Bug 2 model — guests are accounts too).
    pub email: String,
}

async fn invite_guest(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<InviteGuestRequest>,
) -> Result<(StatusCode, Json<TeamGuestRow>), AppError> {
    let mut conn = state.db.begin().await?;
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    let email = body.email.trim().to_lowercase();
    let Some((invitee, _)) = crate::users::find_active_by_email(&mut conn, &email).await? else {
        return Err(AppError::validation(
            "email",
            "No Tasker user with this email. Ask an Owner to create the account first.",
        ));
    };
    // A guest must not be a member (membership supersedes guest status).
    if crate::authorization::get_membership(&mut conn, &team_id, &invitee.id)
        .await?
        .is_some()
    {
        return Err(AppError::validation(
            "email",
            "this user is already a team member",
        ));
    }

    let now = util::now_ms();
    let id = insert_team_guest(&mut conn, &team_id, &invitee.id, &user.id, now).await?;
    conn.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(TeamGuestRow {
            guest_id: id,
            user: UserBrief {
                id: invitee.id,
                email: invitee.email,
                display_name: invitee.display_name,
            },
            created_at: now,
            permissions: Vec::new(),
        }),
    ))
}

/// Shared `team_guests` insert (direct invite + invite-request approval).
/// Unique violations map to a friendly 409.
async fn insert_team_guest(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
    granted_by: &str,
    now: i64,
) -> Result<String, AppError> {
    let id = util::new_id();
    let mut args = Args::new();
    args.s(&id).s(team_id).s(user_id).s(granted_by).i(now);
    if let Err(e) = conn
        .execute(
            "INSERT INTO team_guests (id, team_id, user_id, granted_by, created_at) \
             VALUES (?, ?, ?, ?, ?)",
            args,
        )
        .await
    {
        return Err(crate::db::map_unique_violation(
            e,
            "this user is already a guest of the team".into(),
        ));
    }
    Ok(id)
}

async fn revoke_guest(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, guest_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    let mut args = Args::new();
    args.i(util::now_ms()).s(&guest_id).s(&team_id);
    let updated = conn
        .execute(
            "UPDATE team_guests SET revoked_at = ? \
             WHERE id = ? AND team_id = ? AND revoked_at IS NULL",
            args,
        )
        .await?;
    if updated == 0 {
        return Err(AppError::NotFound("guest not found in this team"));
    }
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_guest_permissions(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, guest_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    ensure_guest_of_team(&mut conn, &guest_id, &team_id).await?;
    let mut args = Args::new();
    args.s(&guest_id);
    let rows = conn
        .fetch_all(
            "SELECT id, permission, scope_type, scope_id, created_at \
             FROM team_guest_permissions WHERE guest_id = ? ORDER BY created_at",
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(|p| {
            Ok::<GuestPermissionRow, AppError>(GuestPermissionRow {
                id: p.try_get("id")?,
                permission: p.try_get("permission")?,
                scope_type: p.try_get("scope_type")?,
                scope_id: p.try_get("scope_id")?,
                created_at: p.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

async fn ensure_guest_of_team(
    conn: &mut Conn<'_>,
    guest_id: &str,
    team_id: &str,
) -> Result<(), AppError> {
    let mut args = Args::new();
    args.s(guest_id).s(team_id);
    let active = conn
        .fetch_optional(
            "SELECT id FROM team_guests \
             WHERE id = ? AND team_id = ? AND revoked_at IS NULL",
            args,
        )
        .await?;
    if active.is_none() {
        return Err(AppError::NotFound("guest not found in this team"));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct GrantPermissionRequest {
    pub permission: String,
    /// `team` for team-scoped permissions; `project` (with `project_id`)
    /// for project-scoped ones (ADR-0021).
    pub scope_type: String,
    #[serde(default)]
    pub project_id: Option<String>,
}

async fn grant_permission(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, guest_id)): Path<(String, String)>,
    Json(body): Json<GrantPermissionRequest>,
) -> Result<(StatusCode, Json<GuestPermissionRow>), AppError> {
    let mut conn = state.db.begin().await?;
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    ensure_guest_of_team(&mut conn, &guest_id, &team_id).await?;

    if !is_valid_permission(&body.permission) {
        return Err(AppError::validation("permission", "unknown permission"));
    }
    let (scope_type, scope_id) = match body.scope_type.as_str() {
        "team" if is_team_scoped(&body.permission) => ("team", None),
        "project" if !is_team_scoped(&body.permission) => {
            let project_id = body.project_id.clone().ok_or_else(|| {
                AppError::validation("project_id", "a project scope requires project_id")
            })?;
            // The project must belong to this team (no cross-team grants).
            let project = crate::projects::find_project(&mut conn, &project_id).await?;
            if project.team_id != team_id {
                return Err(AppError::NotFound("project not found"));
            }
            ("project", Some(project_id))
        }
        _ => {
            return Err(AppError::validation(
                "scope_type",
                if is_team_scoped(&body.permission) {
                    "this permission is team-scoped"
                } else {
                    "this permission is project-scoped and requires project_id"
                },
            ));
        }
    };

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id).s(&guest_id).s(&body.permission).s(scope_type);
    args.opt_s(scope_id.clone()).s(&user.id).i(now);
    if let Err(e) = conn
        .execute(
            "INSERT INTO team_guest_permissions \
             (id, guest_id, permission, scope_type, scope_id, granted_by, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
    {
        return Err(crate::db::map_unique_violation(
            e,
            "this permission is already granted".into(),
        ));
    }
    conn.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(GuestPermissionRow {
            id,
            permission: body.permission,
            scope_type: scope_type.to_string(),
            scope_id,
            created_at: now,
        }),
    ))
}

async fn revoke_permission(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, guest_id, permission_id)): Path<(String, String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    ensure_guest_of_team(&mut conn, &guest_id, &team_id).await?;
    let mut args = Args::new();
    args.s(&permission_id).s(&guest_id);
    let deleted = conn
        .execute(
            "DELETE FROM team_guest_permissions WHERE id = ? AND guest_id = ?",
            args,
        )
        .await?;
    if deleted == 0 {
        return Err(AppError::NotFound("permission not found"));
    }
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Guest invitation requests (Phase 14 WS2, ADR-0021)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct GuestInvitationRow {
    pub id: String,
    pub team_id: String,
    pub invitee_email: String,
    pub status: String,
    pub requested_by: Option<UserBrief>,
    pub decided_by: Option<UserBrief>,
    pub decided_at: Option<i64>,
    pub created_at: i64,
}

const INVITATION_COLUMNS: &str = "i.id, i.team_id, i.invitee_email, i.status, i.decided_at, \
     i.created_at, rb.id AS rb_id, rb.email AS rb_email, rb.display_name AS rb_name, \
     db.id AS db_id, db.email AS db_email, db.display_name AS db_name";

fn invitation_from_row(row: &sqlx::any::AnyRow) -> Result<GuestInvitationRow, AppError> {
    Ok(GuestInvitationRow {
        id: row.try_get("id")?,
        team_id: row.try_get("team_id")?,
        invitee_email: row.try_get("invitee_email")?,
        status: row.try_get("status")?,
        requested_by: row
            .try_get::<Option<String>, _>("rb_id")?
            .map(|id| {
                Ok::<UserBrief, AppError>(UserBrief {
                    id,
                    email: row.try_get("rb_email")?,
                    display_name: row.try_get("rb_name")?,
                })
            })
            .transpose()?,
        decided_by: row
            .try_get::<Option<String>, _>("db_id")?
            .map(|id| {
                Ok::<UserBrief, AppError>(UserBrief {
                    id,
                    email: row.try_get("db_email")?,
                    display_name: row.try_get("db_name")?,
                })
            })
            .transpose()?,
        decided_at: row.try_get("decided_at")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Load one invitation request; 404 when it belongs to another team.
async fn find_invitation(
    conn: &mut Conn<'_>,
    team_id: &str,
    request_id: &str,
) -> Result<GuestInvitationRow, AppError> {
    let mut args = Args::new();
    args.s(request_id).s(team_id);
    let row = conn
        .fetch_optional(
            &format!(
                "SELECT {INVITATION_COLUMNS} \
                 FROM guest_invitations i \
                 LEFT JOIN users rb ON rb.id = i.requested_by \
                 LEFT JOIN users db ON db.id = i.decided_by \
                 WHERE i.id = ? AND i.team_id = ?"
            ),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("invitation request not found"))?;
    invitation_from_row(&row)
}

#[derive(Deserialize)]
pub struct CreateInviteRequest {
    pub email: String,
}

/// Step 1: an active guest holding the team-scoped `invite_guest` grant
/// requests an invitation for an email address. The request starts
/// `pending`; nobody gains access until an Owner/Admin approves.
async fn create_invite_request(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<GuestInvitationRow>), AppError> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(AppError::validation("email", "a valid email is required"));
    }
    let mut conn = state.db.begin().await?;
    // The requester must be an active guest with the invite_guest grant.
    // Members use the member-management surface; strangers get 404.
    let grant = resolve_guest(&mut conn, &team_id, &user.id)
        .await?
        .filter(|g| g.permissions.iter().any(|(p, _)| p == "invite_guest"));
    let Some(grant) = grant else {
        return Err(AppError::NotFound("team not found"));
    };

    // One pending request per (team, email).
    let mut args = Args::new();
    args.s(&team_id).s(&email);
    let open = conn
        .fetch_optional(
            "SELECT id FROM guest_invitations \
             WHERE team_id = ? AND invitee_email = ? AND status = 'pending'",
            args,
        )
        .await?;
    if open.is_some() {
        return Err(AppError::Conflict(
            "an invitation request for this email is already pending".into(),
        ));
    }

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&team_id)
        .s(&grant.guest_id)
        .s(&email)
        .s(&user.id)
        .i(now);
    conn.execute(
        "INSERT INTO guest_invitations \
         (id, team_id, guest_id, invitee_email, status, requested_by, created_at) \
         VALUES (?, ?, ?, ?, 'pending', ?, ?)",
        args,
    )
    .await?;
    let row = find_invitation(&mut conn, &team_id, &id).await?;
    conn.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// Step 2a (Owner/Admin): list the team's invitation requests.
async fn list_invite_requests(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    let mut args = Args::new();
    args.s(&team_id);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {INVITATION_COLUMNS} \
                 FROM guest_invitations i \
                 LEFT JOIN users rb ON rb.id = i.requested_by \
                 LEFT JOIN users db ON db.id = i.decided_by \
                 WHERE i.team_id = ? \
                 ORDER BY CASE i.status WHEN 'pending' THEN 0 ELSE 1 END, i.created_at DESC"
            ),
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(invitation_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// Step 2b (Owner/Admin): approve a pending request — this creates the
/// actual guest registration (same rules as a direct invite).
async fn approve_invite_request(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, request_id)): Path<(String, String)>,
) -> Result<Json<GuestInvitationRow>, AppError> {
    let mut conn = state.db.begin().await?;
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    let request = find_invitation(&mut conn, &team_id, &request_id).await?;
    if request.status != "pending" {
        return Err(AppError::Conflict(
            "this invitation request was already decided".into(),
        ));
    }
    let Some((invitee, _)) =
        crate::users::find_active_by_email(&mut conn, &request.invitee_email).await?
    else {
        return Err(AppError::validation(
            "email",
            "No Tasker user with this email. Ask an Owner to create the account first.",
        ));
    };
    // A guest must not be a member (membership supersedes guest status).
    if crate::authorization::get_membership(&mut conn, &team_id, &invitee.id)
        .await?
        .is_some()
    {
        return Err(AppError::validation(
            "email",
            "this user is already a team member",
        ));
    }
    let now = util::now_ms();
    insert_team_guest(&mut conn, &team_id, &invitee.id, &user.id, now).await?;
    let mut args = Args::new();
    args.s(&user.id).i(now).s(&request_id);
    conn.execute(
        "UPDATE guest_invitations SET status = 'approved', decided_by = ?, decided_at = ? \
         WHERE id = ?",
        args,
    )
    .await?;
    let row = find_invitation(&mut conn, &team_id, &request_id).await?;
    conn.commit().await?;
    Ok(Json(row))
}

/// Step 2c (Owner/Admin): reject a pending request.
async fn reject_invite_request(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((team_id, request_id)): Path<(String, String)>,
) -> Result<Json<GuestInvitationRow>, AppError> {
    let mut conn = state.db.begin().await?;
    require_guest_manager(&mut conn, &team_id, &user.id).await?;
    let request = find_invitation(&mut conn, &team_id, &request_id).await?;
    if request.status != "pending" {
        return Err(AppError::Conflict(
            "this invitation request was already decided".into(),
        ));
    }
    let mut args = Args::new();
    args.s(&user.id).i(util::now_ms()).s(&request_id);
    conn.execute(
        "UPDATE guest_invitations SET status = 'rejected', decided_by = ?, decided_at = ? \
         WHERE id = ?",
        args,
    )
    .await?;
    let row = find_invitation(&mut conn, &team_id, &request_id).await?;
    conn.commit().await?;
    Ok(Json(row))
}

