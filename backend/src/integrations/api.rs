//! Integration management HTTP API (Phase 8B, Task 6).
//!
//! Team-scoped, `Capability::ManageIntegrations`-guarded endpoints for the
//! integration lifecycle built in Tasks 1–5:
//! - **Integrations**: create (webhook secret provisioned atomically and
//!   returned ONCE — it is never retrievable again), list, get, update,
//!   enable/disable, archive.
//! - **Credentials**: rotate (version CAS), revoke, re-provision (requires an
//!   observed revocation). Every lifecycle operation is audited through the
//!   activity trail with **no secret material**.
//! - **Repositories + mappings**: register/list/remove repositories and
//!   manage repo→project mappings (database-enforced tenant isolation).
//! - **Deliveries**: paginated inspection (metadata only — payloads are
//!   never returned), dead-letter visibility, and audited manual replay of
//!   dead-lettered deliveries.
//!
//! Cross-team access is impossible by construction: every route resolves the
//! caller's membership in the path team first, and a non-member sees 404
//! (the same masking as everywhere else).

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{delete, get, post};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::authorization::Capability;
use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::http::metrics::WebhookEvent;
use crate::integrations::store::{self};
use crate::integrations::{Provider, limits};
use crate::state::AppState;
use crate::util;

/// The integration routes: nested under `/api/v1` by `http::build_router`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/teams/{team_id}/integrations",
            get(list_integrations).post(create_integration),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}",
            get(get_integration)
                .patch(update_integration)
                .delete(archive_integration),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/enable",
            post(enable_integration),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/disable",
            post(disable_integration),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/secret/rotate",
            post(rotate_secret),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/secret/revoke",
            post(revoke_secret),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/secret",
            post(reprovision_secret),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/repositories",
            get(list_repositories).post(register_repository),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/repositories/{repository_id}",
            delete(remove_repository),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/repositories/{repository_id}/mappings",
            get(list_mappings).post(create_mapping),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/repositories/{repository_id}/mappings/{mapping_id}",
            delete(delete_mapping),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/deliveries",
            get(list_deliveries),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/deliveries/{delivery_id}",
            get(get_delivery),
        )
        .route(
            "/teams/{team_id}/integrations/{integration_id}/deliveries/{delivery_id}/replay",
            post(replay_delivery),
        )
}

// ---------------------------------------------------------------------------
// Authorization (mirrors rules::api)
// ---------------------------------------------------------------------------

async fn require_manage_integrations(
    conn: &mut Conn<'_>,
    team_id: &str,
    user_id: &str,
) -> Result<(), AppError> {
    let membership = crate::authorization::get_membership(conn, team_id, user_id).await?;
    match membership.map(|m| m.role) {
        Some(role) if role.can(Capability::ManageIntegrations) => Ok(()),
        Some(_) => Err(AppError::Forbidden(
            "your role does not permit managing integrations",
        )),
        None => Err(AppError::NotFound("integration not found")),
    }
}

/// Archived integrations are read-only: metadata and delivery history stay
/// visible, but no mutations are accepted.
fn require_integration_active(integration: &super::Integration) -> Result<(), AppError> {
    if integration.archived_at.is_some() {
        return Err(AppError::Conflict(
            "the integration is archived; archived integrations are read-only".into(),
        ));
    }
    Ok(())
}

/// Resolve the integration in the path team; a non-member (or wrong-team
/// caller) has already been rejected by the membership check, so a missing
/// row here is a plain 404.
async fn require_integration_in_team(
    conn: &mut Conn<'_>,
    team_id: &str,
    integration_id: &str,
) -> Result<super::Integration, AppError> {
    let integration = store::find_integration(conn, integration_id)
        .await?
        .ok_or(AppError::NotFound("integration not found"))?;
    if integration.team_id != team_id {
        // Cross-team probing: same masked 404 as everywhere else.
        return Err(AppError::NotFound("integration not found"));
    }
    Ok(integration)
}

async fn activity_record(
    conn: &mut Conn<'_>,
    team_id: &str,
    actor_id: &str,
    event: &str,
    summary: Option<String>,
) -> Result<(), AppError> {
    crate::activity::record(
        conn,
        team_id,
        None,
        None,
        Some(actor_id),
        event,
        None,
        None,
        summary.as_deref(),
    )
    .await
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

fn integration_json(integration: &super::Integration) -> serde_json::Value {
    json!({
        "id": integration.id,
        "team_id": integration.team_id,
        "provider": integration.provider.as_str(),
        "name": integration.name,
        "description": integration.description,
        "enabled": integration.enabled,
        "created_by": integration.created_by,
        "created_at": integration.created_at,
        "updated_at": integration.updated_at,
        "archived_at": integration.archived_at,
        "webhook_url_path": format!(
            "/api/v1/webhooks/{}/{}",
            integration.provider.as_str(),
            integration.id
        ),
    })
}

async fn integration_with_credentials(
    conn: &mut Conn<'_>,
    integration: &super::Integration,
) -> Result<serde_json::Value, AppError> {
    let state = crate::integrations::credentials::credential_state(conn, &integration.id)
        .await?
        .map(|(version, revoked_at)| {
            json!({
                "credential_version": version,
                "revoked_at": revoked_at,
                "active": revoked_at.is_none(),
            })
        })
        .unwrap_or(json!(null));
    let mut body = integration_json(integration);
    body["credential"] = state;
    Ok(body)
}

// ---------------------------------------------------------------------------
// Create (secret provisioned atomically, returned once)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateIntegrationRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub provider: String,
}

async fn create_integration(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
    AuthedUser { id: user, .. }: AuthedUser,
    Json(body): Json<CreateIntegrationRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let provider = Provider::parse(&body.provider).ok_or_else(|| {
        integration_field_error(
            "provider",
            format!(
                "unknown provider `{}`; supported: github, gitlab",
                body.provider
            ),
        )
    })?;
    let name = super::validate_integration_name(&body.name).map_err(AppError::from)?;
    let description =
        super::validate_integration_description(&body.description.unwrap_or_default())
            .map_err(AppError::from)?;
    let key = state.integrations.key.clone().ok_or_else(|| {
        AppError::internal(
            "integration credentials are unavailable: TASKER_INTEGRATIONS_KEY is not configured",
        )
    })?;

    let mut tx = state.db.begin().await?;
    require_manage_integrations(&mut tx, &team_id, &user).await?;
    // One secret per integration, provisioned in the create transaction so
    // the integration and its credential are born atomically. The plaintext
    // is returned in THIS response only — it is never retrievable again.
    let webhook_secret = crate::security::token::generate();
    let integration_id = match store::create_integration(
        &mut tx,
        &team_id,
        &name,
        description.as_deref(),
        provider,
        &user,
    )
    .await
    {
        Ok(id) => id,
        Err(err) => {
            // Unique (team_id, name): a clean 409, never a raw constraint
            // error (dialect-aware markers mirror the rules API).
            let text = format!("{err}");
            if text.contains("UNIQUE")
                || text.contains("Duplicate entry")
                || text.contains("duplicate key")
            {
                return Err(AppError::Conflict(
                    "an integration with this name already exists in the team".into(),
                ));
            }
            return Err(err);
        }
    };
    crate::integrations::credentials::provision_credential(
        &mut tx,
        &integration_id,
        &webhook_secret,
        &key,
        &user,
        util::now_ms(),
    )
    .await?;
    activity_record(
        &mut tx,
        &team_id,
        &user,
        "integration_created",
        Some(format!("created {provider:?} integration \"{name}\"")),
    )
    .await?;
    tx.commit().await?;

    let mut conn = state.db.conn();
    let integration = store::find_integration(&mut conn, &integration_id)
        .await?
        .ok_or_else(|| AppError::internal("integration vanished after create"))?;
    let mut body = integration_with_credentials(&mut conn, &integration).await?;
    // One-time disclosure.
    body["webhook_secret"] = json!(webhook_secret);
    Ok((StatusCode::CREATED, Json(body)))
}

fn integration_field_error(field: &str, message: String) -> AppError {
    AppError::Validation(vec![crate::error::FieldError::new(field, message)])
}

// ---------------------------------------------------------------------------
// List / Get / Update / Enable / Disable / Archive
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

async fn list_integrations(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
    Query(query): Query<ListQuery>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(25).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    let integrations =
        store::list_integrations(&mut conn, &team_id, i64::from(page_size), offset).await?;
    let mut args = Args::new();
    args.s(&team_id);
    let total = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM integrations WHERE team_id = ?",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));
    let mut items = Vec::with_capacity(integrations.len());
    for integration in &integrations {
        items.push(integration_json(integration));
    }
    Ok(Json(json!({
        "items": items,
        "page": page,
        "page_size": page_size,
        "total": total,
    })))
}

async fn get_integration(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    Ok(Json(
        integration_with_credentials(&mut conn, &integration).await?,
    ))
}

#[derive(Deserialize)]
pub struct UpdateIntegrationRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

async fn update_integration(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
    Json(body): Json<UpdateIntegrationRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    require_integration_active(&integration)?;
    let name = match &body.name {
        Some(n) => Some(super::validate_integration_name(n).map_err(AppError::from)?),
        None => Some(integration.name.clone()),
    };
    let description = match &body.description {
        Some(d) => Some(super::validate_integration_description(d).map_err(AppError::from)?),
        None => Some(integration.description.clone()),
    };
    let name = name.expect("set above");
    let description = description.expect("set above");
    let now = util::now_ms();
    let updated = store::update_integration_metadata(
        &mut conn,
        &integration_id,
        &name,
        description.as_deref(),
        integration.updated_at,
        now,
    )
    .await?;
    if !updated {
        return Err(AppError::Conflict(
            "the integration changed concurrently; reload and retry".into(),
        ));
    }
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "integration_updated",
        Some(format!("updated integration \"{name}\"")),
    )
    .await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    require_integration_active(&integration)?;
    Ok(Json(
        integration_with_credentials(&mut conn, &integration).await?,
    ))
}

async fn enable_integration(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    set_enabled(state, team_id, integration_id, user, true).await
}

async fn disable_integration(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    set_enabled(state, team_id, integration_id, user, false).await
}

async fn set_enabled(
    state: AppState,
    team_id: String,
    integration_id: String,
    user: String,
    enabled: bool,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let changed =
        store::set_integration_enabled(&mut conn, &integration_id, enabled, util::now_ms()).await?;
    if !changed {
        return Err(AppError::Conflict(
            "the integration is archived; archived integrations are read-only".into(),
        ));
    }
    activity_record(
        &mut conn,
        &team_id,
        &user,
        if enabled {
            "integration_enabled"
        } else {
            "integration_disabled"
        },
        Some(format!(
            "{} integration",
            if enabled { "enabled" } else { "disabled" }
        )),
    )
    .await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    require_integration_active(&integration)?;
    Ok(Json(
        integration_with_credentials(&mut conn, &integration).await?,
    ))
}

async fn archive_integration(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    require_integration_active(&integration)?;
    let archived = store::archive_integration(&mut conn, &integration_id, util::now_ms()).await?;
    if !archived {
        return Err(AppError::Conflict(
            "the integration is already archived".into(),
        ));
    }
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "integration_archived",
        Some(format!("archived integration \"{}\"", integration.name)),
    )
    .await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    Ok(Json(
        integration_with_credentials(&mut conn, &integration).await?,
    ))
}

// ---------------------------------------------------------------------------
// Credential lifecycle: rotate / revoke / re-provision (secret shown once)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RotateSecretRequest {
    /// The current credential version (optimistic concurrency): a rotation
    /// racing another rotation loses with 409.
    pub expected_version: i64,
    /// Optional explicit secret (GitLab operators may paste their own token);
    /// defaults to a generated 43-character base64url secret. Bounded by the
    /// credential envelope's size limit.
    #[serde(default)]
    pub new_secret: Option<String>,
}

async fn rotate_secret(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
    Json(body): Json<RotateSecretRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = state.integrations.key.clone().ok_or_else(|| {
        AppError::internal(
            "integration credentials are unavailable: TASKER_INTEGRATIONS_KEY is not configured",
        )
    })?;
    let new_secret = match &body.new_secret {
        Some(s) => {
            if s.chars().count() > limits::WEBHOOK_SECRET_MAX_CHARS {
                return Err(integration_field_error(
                    "new_secret",
                    format!(
                        "secret must be at most {} characters",
                        limits::WEBHOOK_SECRET_MAX_CHARS
                    ),
                ));
            }
            s.clone()
        }
        None => crate::security::token::generate(),
    };
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let new_version = crate::integrations::credentials::rotate_credential(
        &mut conn,
        &integration_id,
        &new_secret,
        &key,
        body.expected_version,
    )
    .await?;
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "webhook_secret_rotated",
        Some(format!(
            "rotated the webhook secret (version {} → {new_version})",
            body.expected_version
        )),
    )
    .await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    require_integration_active(&integration)?;
    let mut body = integration_with_credentials(&mut conn, &integration).await?;
    body["webhook_secret"] = json!(new_secret);
    Ok(Json(body))
}

async fn revoke_secret(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let Some((version, _)) =
        crate::integrations::credentials::credential_state(&mut conn, &integration_id).await?
    else {
        return Err(AppError::NotFound("credential not found"));
    };
    let revoked = crate::integrations::credentials::revoke_credential(
        &mut conn,
        &integration_id,
        version,
        util::now_ms(),
    )
    .await?;
    if !revoked {
        return Err(AppError::Conflict(
            "the credential is already revoked".into(),
        ));
    }
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "webhook_secret_revoked",
        Some(format!("revoked the webhook secret (version {version})")),
    )
    .await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    require_integration_active(&integration)?;
    Ok(Json(
        integration_with_credentials(&mut conn, &integration).await?,
    ))
}

async fn reprovision_secret(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = state.integrations.key.clone().ok_or_else(|| {
        AppError::internal(
            "integration credentials are unavailable: TASKER_INTEGRATIONS_KEY is not configured",
        )
    })?;
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let Some((version, revoked_at)) =
        crate::integrations::credentials::credential_state(&mut conn, &integration_id).await?
    else {
        return Err(AppError::NotFound("credential not found"));
    };
    if revoked_at.is_none() {
        return Err(AppError::Conflict(
            "re-provisioning requires a revoked credential; rotate an active one instead".into(),
        ));
    }
    let new_secret = crate::security::token::generate();
    let new_version = crate::integrations::credentials::re_provision_credential(
        &mut conn,
        &integration_id,
        &new_secret,
        &key,
        version,
        &user,
        util::now_ms(),
    )
    .await?;
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "webhook_secret_reprovisioned",
        Some(format!(
            "re-provisioned the webhook secret (version {version} → {new_version})"
        )),
    )
    .await?;
    let integration = require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    require_integration_active(&integration)?;
    let mut body = integration_with_credentials(&mut conn, &integration).await?;
    body["webhook_secret"] = json!(new_secret);
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// Repositories + project mappings
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RegisterRepositoryRequest {
    pub external_repo_id: String,
    pub full_name: String,
}

async fn register_repository(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
    Json(body): Json<RegisterRepositoryRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let repository_id = match store::register_repository(
        &mut conn,
        &integration_id,
        &body.external_repo_id,
        &body.full_name,
    )
    .await
    {
        Ok(id) => id,
        Err(err) => {
            let text = format!("{err}");
            if text.contains("UNIQUE")
                || text.contains("Duplicate entry")
                || text.contains("duplicate key")
            {
                return Err(AppError::Conflict(
                    "this repository is already registered on the integration".into(),
                ));
            }
            return Err(err);
        }
    };
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "repository_registered",
        Some(format!("registered repository \"{}\"", body.full_name)),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": repository_id,
            "integration_id": integration_id,
            "external_repo_id": body.external_repo_id,
            "full_name": body.full_name,
        })),
    ))
}

#[derive(Deserialize)]
pub struct ListRepositoriesQuery {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

async fn list_repositories(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    Query(query): Query<ListRepositoriesQuery>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(25).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let rows =
        store::list_repositories(&mut conn, &integration_id, i64::from(page_size), offset).await?;
    let mut args = Args::new();
    args.s(&integration_id);
    let total = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM integration_repositories WHERE integration_id = ?",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|repo| {
            json!({
                "id": repo.id,
                "integration_id": repo.integration_id,
                "external_repo_id": repo.external_repo_id,
                "full_name": repo.full_name,
                "created_at": repo.created_at,
            })
        })
        .collect();
    Ok(Json(json!({
        "items": items,
        "page": page,
        "page_size": page_size,
        "total": total,
    })))
}

async fn remove_repository(
    State(state): State<AppState>,
    Path((team_id, integration_id, repository_id)): Path<(String, String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    // Mappings RESTRICT the delete: surface a clean 409 instead of a raw
    // constraint error, directing the operator to remove mappings first.
    let mut args = Args::new();
    args.s(&repository_id);
    let mapping_count = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM integration_repository_projects WHERE repository_id = ?",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));
    if mapping_count > 0 {
        return Err(AppError::Conflict(
            "the repository has project mappings; remove them first".into(),
        ));
    }
    let removed = store::delete_repository(&mut conn, &integration_id, &repository_id).await?;
    if !removed {
        return Err(AppError::NotFound("repository not found"));
    }
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "repository_removed",
        Some("removed a registered repository".to_string()),
    )
    .await?;
    Ok(Json(json!({"status": "removed"})))
}

#[derive(Deserialize)]
pub struct CreateMappingRequest {
    pub project_id: String,
}

async fn create_mapping(
    State(state): State<AppState>,
    Path((team_id, integration_id, repository_id)): Path<(String, String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
    Json(body): Json<CreateMappingRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    // The store re-checks project membership (clean 404) and the composite
    // foreign keys make cross-team mappings unrepresentable.
    let mapping_id =
        store::map_repository_to_project(&mut conn, &repository_id, &body.project_id, &user)
            .await?;
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "repository_mapped",
        Some(format!(
            "mapped repository {repository_id} to project {}",
            body.project_id
        )),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": mapping_id,
            "repository_id": repository_id,
            "project_id": body.project_id,
        })),
    ))
}

async fn list_mappings(
    State(state): State<AppState>,
    Path((team_id, integration_id, repository_id)): Path<(String, String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let mappings = store::list_mappings_with_meta(&mut conn, &repository_id).await?;
    Ok(Json(json!({ "items": mappings })))
}

async fn delete_mapping(
    State(state): State<AppState>,
    Path((team_id, integration_id, repository_id, mapping_id)): Path<(
        String,
        String,
        String,
        String,
    )>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let deleted = store::delete_mapping(&mut conn, &repository_id, &mapping_id).await?;
    if !deleted {
        return Err(AppError::NotFound("mapping not found"));
    }
    activity_record(
        &mut conn,
        &team_id,
        &user,
        "repository_unmapped",
        Some(format!("removed mapping {mapping_id}")),
    )
    .await?;
    Ok(Json(json!({"status": "removed"})))
}

// ---------------------------------------------------------------------------
// Delivery inspection + audited replay
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListDeliveriesQuery {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub status: Option<String>,
}

/// The delivery list/detail projection: bounded metadata only — **the raw
/// payload is never returned** (provider payloads may contain sensitive
/// content; the durable record keeps them for replay processing only).
async fn delivery_json(
    conn: &mut Conn<'_>,
    integration_id: &str,
    delivery_row_id: &str,
) -> Result<Option<serde_json::Value>, AppError> {
    let mut args = Args::new();
    args.s(integration_id).s(delivery_row_id);
    let row = conn
        .fetch_optional(
            "SELECT id, delivery_id, event_type_raw, status, attempts, next_attempt_at, \
                    created_at, updated_at, completed_at, last_error_category, \
                    last_error_detail, claim_token, worker_id \
             FROM webhook_deliveries WHERE integration_id = ? AND id = ?",
            args,
        )
        .await?;
    Ok(row.map(|r| {
        json!({
            "id": r.try_get::<String, _>("id").unwrap_or_default(),
            "delivery_id": r.try_get::<String, _>("delivery_id").unwrap_or_default(),
            "event_type": r.try_get::<String, _>("event_type_raw").unwrap_or_default(),
            "status": r.try_get::<String, _>("status").unwrap_or_default(),
            "attempts": r.try_get::<i64, _>("attempts").unwrap_or_default(),
            "next_attempt_at": r.try_get::<i64, _>("next_attempt_at").unwrap_or_default(),
            "created_at": r.try_get::<i64, _>("created_at").unwrap_or_default(),
            "updated_at": r.try_get::<i64, _>("updated_at").unwrap_or_default(),
            "completed_at": r.try_get::<Option<i64>, _>("completed_at").unwrap_or_default(),
            "last_error_category": r.try_get::<Option<String>, _>("last_error_category").unwrap_or_default(),
            "last_error_detail": r.try_get::<Option<String>, _>("last_error_detail").unwrap_or_default(),
            "claim_token": r.try_get::<Option<String>, _>("claim_token").unwrap_or_default(),
            "worker_id": r.try_get::<Option<String>, _>("worker_id").unwrap_or_default(),
        })
    }))
}

async fn list_deliveries(
    State(state): State<AppState>,
    Path((team_id, integration_id)): Path<(String, String)>,
    Query(query): Query<ListDeliveriesQuery>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(25).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);
    let status = query.status.as_deref().filter(|s| !s.is_empty());
    // The status filter is validated against the closed state machine.
    if let Some(status) = status {
        const STATUSES: &[&str] = &[
            "pending",
            "processing",
            "processed",
            "retryable_failed",
            "dead_letter",
            "unsupported",
        ];
        if !STATUSES.contains(&status) {
            return Err(integration_field_error(
                "status",
                format!("unknown status `{status}`"),
            ));
        }
    }
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    let mut args = Args::new();
    args.s(&integration_id).i(i64::from(page_size)).i(offset);
    let filter_sql = if status.is_some() {
        " AND status = ?"
    } else {
        ""
    };
    // Placeholder order: integration_id, page_size, offset [, status].
    if let Some(status) = status {
        args.s(status);
    }
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT id FROM webhook_deliveries WHERE integration_id = ?{filter_sql} \
                 ORDER BY created_at DESC, id LIMIT ? OFFSET ?"
            ),
            args,
        )
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let id: String = row.try_get("id")?;
        if let Some(delivery) = delivery_json(&mut conn, &integration_id, &id).await? {
            items.push(delivery);
        }
    }
    let mut count_args = Args::new();
    count_args.s(&integration_id);
    if let Some(status) = status {
        count_args.s(status);
    }
    let total = conn
        .fetch_optional(
            &format!(
                "SELECT COUNT(*) AS n FROM webhook_deliveries \
                 WHERE integration_id = ?{filter_sql}"
            ),
            count_args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));
    Ok(Json(json!({
        "items": items,
        "page": page,
        "page_size": page_size,
        "total": total,
    })))
}

async fn get_delivery(
    State(state): State<AppState>,
    Path((team_id, integration_id, delivery_id)): Path<(String, String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_manage_integrations(&mut conn, &team_id, &user).await?;
    require_integration_in_team(&mut conn, &team_id, &integration_id).await?;
    delivery_json(&mut conn, &integration_id, &delivery_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("delivery not found"))
}

#[derive(Deserialize)]
pub struct ReplayDeliveryRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

async fn replay_delivery(
    State(state): State<AppState>,
    Path((team_id, integration_id, delivery_id)): Path<(String, String, String)>,
    AuthedUser { id: user, .. }: AuthedUser,
    Json(body): Json<ReplayDeliveryRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db.begin().await?;
    require_manage_integrations(&mut tx, &team_id, &user).await?;
    require_integration_in_team(&mut tx, &team_id, &integration_id).await?;
    let replayed = super::webhook::delivery::replay_dead_letter(
        &mut tx,
        &delivery_id,
        &user,
        body.reason.as_deref(),
        util::now_ms(),
    )
    .await?;
    if !replayed {
        return Err(AppError::Conflict(
            "only dead-lettered deliveries can be replayed".into(),
        ));
    }
    crate::activity::record(
        &mut tx,
        &team_id,
        None,
        None,
        Some(&user),
        "delivery_replayed",
        None,
        None,
        Some(&format!("replayed delivery {delivery_id}")),
    )
    .await?;
    tx.commit().await?;
    state.metrics.observe_webhook(WebhookEvent::Replayed);
    Ok(Json(json!({"status": "replayed"})))
}
