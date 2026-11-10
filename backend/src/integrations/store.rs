//! Integration persistence (Phase 8B, Task 1).
//!
//! Portable store contracts for integrations, repositories, repo→project
//! mappings, credentials (lifecycle in `credentials.rs`), and webhook
//! deliveries (schema only in Task 1 — intake/claim/retry arrive in Task 2).
//! All tenant isolation for repo→project mappings is database-enforced via
//! the composite foreign keys introduced in migration 0016.
//!
//! Conventions follow the Phase 8A store patterns: `Args` binds, `?`
//! placeholders adapted per dialect, RESTRICT foreign keys, and guarded
//! updates that rely on unique constraints for idempotency.

use sqlx::Row;

use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::util;

use super::{Integration, Provider, validate_external_repo_id, validate_repo_full_name};

/// A registered external repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub id: String,
    pub integration_id: String,
    pub external_repo_id: String,
    pub full_name: String,
    pub created_at: i64,
}

fn integration_from_row(row: &sqlx::any::AnyRow) -> Result<Integration, AppError> {
    let provider: String = row.try_get("provider")?;
    Ok(Integration {
        id: row.try_get("id")?,
        team_id: row.try_get("team_id")?,
        provider: Provider::parse(&provider)
            .ok_or_else(|| AppError::internal("stored integration provider is invalid"))?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        enabled: row.try_get::<i64, _>("enabled").map(|v| v != 0)?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        archived_at: row.try_get("archived_at")?,
    })
}

const INTEGRATION_COLUMNS: &str = "id, team_id, provider, name, description, enabled, \
     created_by, created_at, updated_at, archived_at";

/// Create an integration for a team. Duplicate names within a team surface
/// as a unique violation (surfaced as 409 by the caller).
pub async fn create_integration(
    conn: &mut Conn<'_>,
    team_id: &str,
    name: &str,
    description: Option<&str>,
    provider: Provider,
    created_by: &str,
) -> Result<String, AppError> {
    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(team_id)
        .s(name)
        .opt_s(description)
        .s(provider.as_str())
        .b(false)
        .s(created_by)
        .i(now)
        .i(now)
        .opt_i(None::<i64>);
    conn.execute(
        "INSERT INTO integrations (id, team_id, provider, name, description, enabled, \
         created_by, created_at, updated_at, archived_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    Ok(id)
}

/// Load one integration by id.
pub async fn find_integration(
    conn: &mut Conn<'_>,
    integration_id: &str,
) -> Result<Option<Integration>, AppError> {
    let mut args = Args::new();
    args.s(integration_id);
    let row = conn
        .fetch_optional(
            &format!("SELECT {INTEGRATION_COLUMNS} FROM integrations WHERE id = ?"),
            args,
        )
        .await?;
    row.map(|r| integration_from_row(&r)).transpose()
}

/// List a team's integrations (bounded by caller pagination).
pub async fn list_integrations(
    conn: &mut Conn<'_>,
    team_id: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Integration>, AppError> {
    let mut args = Args::new();
    args.s(team_id).i(limit).i(offset);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {INTEGRATION_COLUMNS} FROM integrations WHERE team_id = ? \
                 ORDER BY created_at DESC, id LIMIT ? OFFSET ?"
            ),
            args,
        )
        .await?;
    rows.iter().map(integration_from_row).collect()
}

/// Update mutable metadata (name/description); CAS-guarded on `updated_at`
/// for optimistic concurrency.
pub async fn update_integration_metadata(
    conn: &mut Conn<'_>,
    integration_id: &str,
    name: &str,
    description: Option<&str>,
    expected_updated_at: i64,
    now: i64,
) -> Result<bool, AppError> {
    let mut args = Args::new();
    args.s(name)
        .opt_s(description)
        .i(now)
        .s(integration_id)
        .i(expected_updated_at);
    let affected = conn
        .execute(
            "UPDATE integrations SET name = ?, description = ?, updated_at = ? \
             WHERE id = ? AND updated_at = ?",
            args,
        )
        .await?;
    Ok(affected > 0)
}

/// Enable or disable an integration. Archived integrations are read-only.
pub async fn set_integration_enabled(
    conn: &mut Conn<'_>,
    integration_id: &str,
    enabled: bool,
    now: i64,
) -> Result<bool, AppError> {
    let mut args = Args::new();
    args.b(enabled).i(now).s(integration_id);
    let affected = conn
        .execute(
            "UPDATE integrations SET enabled = ?, updated_at = ? \
             WHERE id = ? AND archived_at IS NULL",
            args,
        )
        .await?;
    Ok(affected > 0)
}

/// Archive an integration (irreversible in this phase): stops delivery
/// processing, keeps history and credentials auditable.
pub async fn archive_integration(
    conn: &mut Conn<'_>,
    integration_id: &str,
    now: i64,
) -> Result<bool, AppError> {
    let mut args = Args::new();
    args.i(now).i(now).s(integration_id);
    let affected = conn
        .execute(
            "UPDATE integrations SET archived_at = ?, enabled = FALSE, updated_at = ? \
             WHERE id = ? AND archived_at IS NULL",
            args,
        )
        .await?;
    Ok(affected > 0)
}

/// Register an external repository under an integration. The caller must
/// have validated the ids; duplicates surface via the unique constraint.
pub async fn register_repository(
    conn: &mut Conn<'_>,
    integration_id: &str,
    external_repo_id: &str,
    full_name: &str,
) -> Result<String, AppError> {
    let validated_id =
        validate_external_repo_id(external_repo_id).map_err(crate::error::AppError::from)?;
    let validated_name =
        validate_repo_full_name(full_name).map_err(crate::error::AppError::from)?;
    // The repository's team comes from the integration row (database truth,
    // never a caller-supplied value) and anchors the composite tenant FK.
    let mut args = Args::new();
    args.s(integration_id);
    let team_id: String = conn
        .fetch_optional("SELECT team_id FROM integrations WHERE id = ?", args)
        .await?
        .ok_or(AppError::NotFound("integration not found"))?
        .try_get("team_id")?;
    let id = util::new_id();
    let mut args = Args::new();
    args.s(&id)
        .s(integration_id)
        .s(&team_id)
        .s(&validated_id)
        .s(&validated_name)
        .i(util::now_ms());
    conn.execute(
        "INSERT INTO integration_repositories (id, integration_id, team_id, external_repo_id, \
         full_name, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    Ok(id)
}

/// Map a repository to a Tasker project. **Tenant isolation is enforced by
/// the composite foreign keys**: the mapping row carries `team_id`, and both
/// composite FKs force the repository's team and the project's team to match
/// that value — a cross-team mapping is rejected by the database. The
/// service additionally re-checks project membership so the failure is a
/// clean 404 rather than a raw constraint error.
pub async fn map_repository_to_project(
    conn: &mut Conn<'_>,
    repository_id: &str,
    project_id: &str,
    mapped_by: &str,
) -> Result<String, AppError> {
    // Resolve the repository's team (database truth, not payload data).
    let mut args = Args::new();
    args.s(repository_id);
    let repo_team: String = conn
        .fetch_optional(
            "SELECT team_id FROM integration_repositories WHERE id = ?",
            args,
        )
        .await?
        .ok_or(AppError::NotFound("repository not found"))?
        .try_get("team_id")?;
    // Verify the project belongs to the same team (service-level re-check so
    // the user sees a clean 404 instead of a constraint violation).
    let mut args = Args::new();
    args.s(project_id).s(&repo_team);
    let project_matches = conn
        .fetch_optional("SELECT id FROM projects WHERE id = ? AND team_id = ?", args)
        .await?
        .is_some();
    if !project_matches {
        return Err(AppError::NotFound("project not found for this team"));
    }
    let id = util::new_id();
    let mut args = Args::new();
    args.s(&id)
        .s(repository_id)
        .s(project_id)
        .s(&repo_team)
        .s(mapped_by)
        .i(util::now_ms());
    conn.execute(
        "INSERT INTO integration_repository_projects (id, repository_id, project_id, \
         team_id, mapped_by, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    Ok(id)
}

/// List the project mappings for one repository.
pub async fn list_repository_projects(
    conn: &mut Conn<'_>,
    repository_id: &str,
) -> Result<Vec<String>, AppError> {
    let mut args = Args::new();
    args.s(repository_id);
    let rows = conn
        .fetch_all(
            "SELECT project_id FROM integration_repository_projects \
             WHERE repository_id = ? ORDER BY created_at, id",
            args,
        )
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("project_id").ok())
        .collect())
}

/// Durable webhook delivery record (schema-only in Task 1; intake writes it,
/// the Task 2 worker processes it). Kept here so the intake path and the
/// future worker share one insert contract. Replay protection is the
/// unique constraint: a duplicate delivery id within one integration is a
/// no-op (`Ok(false)`), not an error.
pub async fn insert_delivery(
    conn: &mut Conn<'_>,
    integration_id: &str,
    delivery_id: &str,
    event_type_raw: &str,
    payload: &str,
    now: i64,
) -> Result<bool, AppError> {
    let mut args = Args::new();
    args.s(util::new_id())
        .s(integration_id)
        .s(delivery_id)
        .s(event_type_raw)
        .s(payload)
        .i(now)
        .i(now);
    let affected = conn
        .execute(
            "INSERT INTO webhook_deliveries (id, integration_id, delivery_id, \
             event_type_raw, payload, status, attempts, next_attempt_at, created_at, \
             updated_at) VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, ?, ?)",
            args,
        )
        .await?;
    Ok(affected > 0)
}
