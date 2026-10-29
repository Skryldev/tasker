//! Centralized role-based authorization.
//!
//! Roles are ordered (Owner > Admin > Manager > Member) and every capability
//! check is defined here — HTTP handlers and services never make ad-hoc role
//! comparisons. Client-supplied team/project/task IDs are never trusted: the
//! server resolves the resource's owning team and looks up the caller's
//! membership row before any capability is evaluated.

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::db::{Args, Conn};
use crate::error::AppError;

/// Declared smallest-first so the derived ordering matches privilege
/// (Member < Manager < Admin < Owner).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Member,
    Manager,
    Admin,
    Owner,
}

impl Role {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            "manager" => Ok(Self::Manager),
            "member" => Ok(Self::Member),
            other => Err(AppError::validation(
                "role",
                format!("unknown role `{other}`; expected owner, admin, manager or member"),
            )),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Manager => "manager",
            Self::Member => "member",
        }
    }
}

/// Capabilities that can be required of a team member.
#[derive(Debug, Clone, Copy)]
pub enum Capability {
    /// Update team name/description.
    UpdateTeam,
    /// Add members to the team.
    AddMember,
    /// Create/modify/remove projects.
    ManageProjects,
    /// Create tasks.
    CreateTask,
    /// Update any task in the team.
    UpdateAnyTask,
    /// Delete any task in the team.
    DeleteAnyTask,
    /// Create team labels.
    CreateLabel,
    /// Edit any comment.
    EditAnyComment,
    /// Delete any comment.
    DeleteAnyComment,
    /// Accept or reject any task awaiting review (reviewer override). Held by
    /// Owner and Admin only: reviewers may not review their own assigned work,
    /// and acceptance authority stays out of the day-to-day execution roles.
    ReviewAnyTask,
}

impl Role {
    /// The permission matrix. `ManageTeamMembership` (role changes/removals)
    /// has rank-based rules beyond a boolean capability and is handled in
    /// the teams service; everything boolean lives here.
    #[must_use]
    pub fn can(self, capability: Capability) -> bool {
        use Capability::{
            AddMember, CreateLabel, CreateTask, DeleteAnyComment, DeleteAnyTask, EditAnyComment,
            ManageProjects, ReviewAnyTask, UpdateAnyTask, UpdateTeam,
        };
        match self {
            Self::Owner => true,
            Self::Admin => matches!(
                capability,
                UpdateTeam
                    | AddMember
                    | ManageProjects
                    | CreateTask
                    | UpdateAnyTask
                    | DeleteAnyTask
                    | CreateLabel
                    | EditAnyComment
                    | DeleteAnyComment
                    | ReviewAnyTask
            ),
            Self::Manager => matches!(
                capability,
                ManageProjects
                    | CreateTask
                    | UpdateAnyTask
                    | DeleteAnyTask
                    | CreateLabel
                    | DeleteAnyComment
            ),
            Self::Member => matches!(capability, CreateTask),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Membership {
    pub team_id: String,
    pub user_id: String,
    pub role: Role,
}

pub async fn get_membership(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
) -> Result<Option<Membership>, AppError> {
    let mut args = Args::new();
    args.s(team_id).s(user_id);
    let row = conn
        .fetch_optional(
            "SELECT team_id, user_id, role FROM team_members WHERE team_id = ? AND user_id = ?",
            args,
        )
        .await?;
    row.map(|r| {
        Ok(Membership {
            team_id: r.try_get("team_id")?,
            user_id: r.try_get("user_id")?,
            role: Role::parse(&r.try_get::<String, _>("role")?)?,
        })
    })
    .transpose()
}

/// Resolve the caller's membership in a team and require a capability.
pub async fn require_capability(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
    capability: Capability,
) -> Result<Membership, AppError> {
    let membership = get_membership(conn, team_id, user_id)
        .await?
        .ok_or(AppError::Forbidden("you are not a member of this team"))?;
    if !membership.role.can(capability) {
        return Err(AppError::Forbidden("your role does not permit this action"));
    }
    Ok(membership)
}

/// Count of active owners in a team (used to protect the last owner).
pub async fn count_owners(conn: &mut Conn<'_>, team_id: &str) -> Result<i64, AppError> {
    let mut args = Args::new();
    args.s(team_id).s("owner");
    let row = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM team_members WHERE team_id = ? AND role = ?",
            args,
        )
        .await?;
    Ok(row.map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0)))
}

/// Count of owner-role members across all teams (used by admin bootstrap).
pub async fn count_owners_global(conn: &mut Conn<'_>) -> Result<i64, AppError> {
    let args = Args::new();
    let row = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM team_members WHERE role = 'owner'",
            args,
        )
        .await?;
    Ok(row.map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_ordering() {
        assert!(Role::Owner > Role::Admin);
        assert!(Role::Admin > Role::Manager);
        assert!(Role::Manager > Role::Member);
    }

    #[test]
    fn matrix_owner_all() {
        for cap in [
            Capability::UpdateTeam,
            Capability::AddMember,
            Capability::ManageProjects,
            Capability::CreateTask,
            Capability::UpdateAnyTask,
            Capability::DeleteAnyTask,
            Capability::CreateLabel,
            Capability::EditAnyComment,
            Capability::DeleteAnyComment,
            Capability::ReviewAnyTask,
        ] {
            assert!(Role::Owner.can(cap));
        }
    }

    #[test]
    fn matrix_admin() {
        assert!(Role::Admin.can(Capability::UpdateTeam));
        assert!(Role::Admin.can(Capability::AddMember));
        assert!(Role::Admin.can(Capability::EditAnyComment));
        assert!(Role::Admin.can(Capability::ManageProjects));
        assert!(Role::Admin.can(Capability::DeleteAnyTask));
        assert!(Role::Admin.can(Capability::ReviewAnyTask));
    }

    #[test]
    fn matrix_manager() {
        assert!(!Role::Manager.can(Capability::UpdateTeam));
        assert!(!Role::Manager.can(Capability::AddMember));
        assert!(Role::Manager.can(Capability::ManageProjects));
        assert!(Role::Manager.can(Capability::UpdateAnyTask));
        assert!(Role::Manager.can(Capability::DeleteAnyComment));
        assert!(!Role::Manager.can(Capability::EditAnyComment));
        // Acceptance authority stays with Owner/Admin only.
        assert!(!Role::Manager.can(Capability::ReviewAnyTask));
    }

    #[test]
    fn matrix_member() {
        assert!(Role::Member.can(Capability::CreateTask));
        assert!(!Role::Member.can(Capability::UpdateAnyTask));
        assert!(!Role::Member.can(Capability::DeleteAnyTask));
        assert!(!Role::Member.can(Capability::ManageProjects));
        assert!(!Role::Member.can(Capability::AddMember));
    }

    #[test]
    fn role_parse() {
        assert_eq!(Role::parse("owner").unwrap(), Role::Owner);
        assert!(Role::parse("superuser").is_err());
    }
}
