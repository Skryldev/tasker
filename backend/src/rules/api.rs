//! Rules management and history API (Phase 8A, Task 6).
//!
//! Authorization (accepted Task 6 matrix): Owner/Admin manage rules
//! (create, update, enable, disable, archive, inspect versions); Manager
//! additionally reads execution history; Members and guests have no rules
//! access; outsiders get the standard 404 so cross-team enumeration is
//! impossible. Every query resolves the caller's team membership before
//! touching rule data — team scope is enforced in SQL, not the frontend.
//!
//! Versioning: every update writes an immutable `rule_versions` row and
//! bumps `current_version` under optimistic concurrency control
//! (`WHERE current_version` must equal the caller's `expected_version`);
//! rules keep their name reserved (`UNIQUE(team_id, name)`) and stop
//! evaluating (the engine selection filters `archived_at IS NULL`).
//!
//! Condition and action payloads cross the API as JSON objects and are
//! re-validated through the Task 1 model (`RuleDefinition::from_json`) on
//! every write; stored payloads are canonical.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::authorization::{Capability, Role};
use crate::db::{Args, Conn};
use crate::error::{AppError, FieldError};
use crate::state::AppState;
use crate::util;

use super::events::TriggerType;

/// The priorities a rule may reference, mirrored from `tasks::PRIORITIES`
/// for the `OpenAPI` schema.
pub const PRIORITIES: [&str; 4] = ["low", "medium", "high", "urgent"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/teams/{team_id}/rules", post(create_rule).get(list_rules))
        .route("/rules/{rule_id}", get(get_rule).patch(update_rule))
        .route("/rules/{rule_id}/enable", post(enable_rule))
        .route("/rules/{rule_id}/disable", post(disable_rule))
        .route("/rules/{rule_id}/archive", post(archive_rule))
        .route("/rules/{rule_id}/versions", get(list_rule_versions))
        .route("/rules/{rule_id}/history", get(list_rule_history))
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

fn rule_row_to_json(row: &sqlx::any::AnyRow) -> Result<Value, AppError> {
    let condition_json: String = row.try_get("condition_json")?;
    let actions_json: String = row.try_get("actions_json")?;
    let condition: Value =
        serde_json::from_str(&condition_json).map_err(|e| AppError::internal(e.to_string()))?;
    let actions: Value =
        serde_json::from_str(&actions_json).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(serde_json::json!({
        "id": row.try_get::<String, _>("id")?,
        "team_id": row.try_get::<String, _>("team_id")?,
        "name": row.try_get::<String, _>("name")?,
        "description": row.try_get::<Option<String>, _>("description")?,
        "trigger_type": row.try_get::<String, _>("trigger_type")?,
        "enabled": row.try_get::<i64, _>("enabled").map(|v| v != 0)?,
        "current_version": row.try_get::<i64, _>("current_version")?,
        "condition": condition,
        "actions": actions,
        "created_by": row.try_get::<String, _>("created_by")?,
        "created_at": row.try_get::<i64, _>("created_at")?,
        "updated_at": row.try_get::<i64, _>("updated_at")?,
        "archived_at": row.try_get::<Option<i64>, _>("archived_at")?,
    }))
}

/// Load a rule row; callers resolve membership before/after this.
async fn find_rule(
    conn: &mut Conn<'_>,
    rule_id: &str,
) -> Result<Option<(String, Value)>, AppError> {
    let mut args = Args::new();
    args.s(rule_id);
    let row = conn
        .fetch_optional(
            "SELECT id, team_id, name, description, trigger_type, enabled, \
             current_version, condition_json, actions_json, created_by, created_at, \
             updated_at, archived_at FROM rules WHERE id = ?",
            args,
        )
        .await?;
    let found = match row {
        Some(r) => {
            let team_id = r.try_get::<String, _>("team_id")?;
            let value = rule_row_to_json(&r)?;
            Some((team_id, value))
        }
        None => None,
    };
    Ok(found)
}

/// Resolve the rule's team and the caller's membership role. Outsiders and
/// guests get the standard 404 (no team membership row = no information).
async fn resolve_rule_access(
    conn: &mut Conn<'_>,
    rule_id: &str,
    user_id: &str,
) -> Result<(String, Value, Option<Role>), AppError> {
    let Some((team_id, rule)) = find_rule(conn, rule_id).await? else {
        return Err(AppError::NotFound("rule not found"));
    };
    let membership = crate::authorization::get_membership(conn, &team_id, user_id).await?;
    if membership.is_none() {
        return Err(AppError::NotFound("rule not found"));
    }
    Ok((team_id, rule, membership.map(|m| m.role)))
}

/// History read: Manager and above. A caller with no membership row
/// (outsider/guest) gets the standard 404 — no cross-team enumeration.
fn require_history_access(role: Option<Role>) -> Result<(), AppError> {
    match role {
        Some(role @ (Role::Owner | Role::Admin | Role::Manager)) => {
            let _ = role;
            Ok(())
        }
        Some(_) => Err(AppError::Forbidden(
            "your role does not permit reading rule execution history",
        )),
        None => Err(AppError::NotFound("rule not found")),
    }
}

/// Rule management: Owner/Admin (`Capability::ManageRules`). A caller with
/// no membership row gets 404; a Member gets 403.
fn require_manage_rules(role: Option<Role>) -> Result<(), AppError> {
    match role {
        Some(role) if role.can(Capability::ManageRules) => Ok(()),
        Some(_) => Err(AppError::Forbidden(
            "your role does not permit managing rules",
        )),
        None => Err(AppError::NotFound("rule not found")),
    }
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateRuleRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub trigger_type: String,
    /// The condition DSL tree (strictly validated server-side).
    pub condition: Value,
    /// 1..=5 typed actions.
    pub actions: Value,
    #[serde(default)]
    pub enabled: bool,
}

async fn create_rule(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<CreateRuleRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let mut conn = state.db.begin().await?;
    let membership = crate::authorization::get_membership(&mut conn, &team_id, &user.id).await?;
    require_manage_rules(membership.map(|m| m.role))?;

    let name = super::validate_name(&body.name).map_err(super::RuleError::from)?;
    let description = match &body.description {
        Some(d) => super::validate_description(d).map_err(super::RuleError::from)?,
        None => None,
    };
    let trigger = TriggerType::parse(&body.trigger_type).ok_or_else(|| {
        rule_field_error(
            "trigger_type",
            format!("unknown trigger `{}`", body.trigger_type),
        )
    })?;
    let condition_json =
        serde_json::to_string(&body.condition).map_err(|e| AppError::internal(e.to_string()))?;
    let actions_json =
        serde_json::to_string(&body.actions).map_err(|e| AppError::internal(e.to_string()))?;
    let definition = super::RuleDefinition::from_json(trigger, &condition_json, &actions_json)?;

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&team_id)
        .s(&name)
        .opt_s(description.clone())
        .s(trigger.as_str())
        .b(body.enabled)
        .i(1i64)
        .s(definition.condition_json())
        .s(definition.actions_json())
        .s(&user.id)
        .i(now)
        .i(now)
        .opt_i(None::<i64>);
    // Duplicate names within a team surface as a unique violation (409 via
    // the DB constraint); archived rules keep their name reserved.
    if let Err(err) = conn
        .execute(
            "INSERT INTO rules (id, team_id, name, description, trigger_type, enabled, \
             current_version, condition_json, actions_json, created_by, created_at, \
             updated_at, archived_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await
    {
        if format!("{err}").contains("UNIQUE") || format!("{err}").contains("Duplicate entry") {
            return Err(AppError::Conflict(
                "a rule with this name already exists in the team".into(),
            ));
        }
        return Err(err);
    }
    let mut args = Args::new();
    args.s(&id)
        .i(1i64)
        .s(trigger.as_str())
        .s(definition.condition_json())
        .s(definition.actions_json())
        .s(&user.id)
        .i(now);
    conn.execute(
        "INSERT INTO rule_versions (rule_id, version, trigger_type, condition_json, \
         actions_json, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    activity_record(
        &mut conn,
        &team_id,
        &user.id,
        "rule_created",
        Some(&format!("created rule \"{name}\"")),
    )
    .await?;
    conn.commit().await?;

    let (rule_team, rule) = find_rule(&mut state.db.conn(), &id)
        .await?
        .ok_or_else(|| AppError::internal("rule vanished after create"))?;
    let _ = rule_team;
    Ok((StatusCode::CREATED, Json(rule)))
}

fn rule_field_error(field: &str, message: String) -> AppError {
    AppError::Validation(vec![FieldError::new(field, message)])
}

/// Automation-independent audit trail: rule lifecycle events are recorded in
/// the activity table with no task association.
async fn activity_record(
    conn: &mut Conn<'_>,
    team_id: &str,
    actor_id: &str,
    event: &str,
    summary: Option<&str>,
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
        summary,
    )
    .await
}

// ---------------------------------------------------------------------------
// List / Get
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListRulesQuery {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub trigger_type: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

async fn list_rules(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Query(query): Query<ListRulesQuery>,
) -> Result<Json<Value>, AppError> {
    let mut conn = state.db.conn();
    let membership = crate::authorization::get_membership(&mut conn, &team_id, &user.id).await?;
    require_manage_rules(membership.map(|m| m.role))?;

    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(25).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);

    let mut where_clause = String::from(" WHERE team_id = ? AND archived_at IS NULL");
    let mut count_args = Args::new();
    count_args.s(&team_id);
    if let Some(trigger) = &query.trigger_type {
        TriggerType::parse(trigger).ok_or_else(|| {
            rule_field_error("trigger_type", format!("unknown trigger `{trigger}`"))
        })?;
        where_clause.push_str(" AND trigger_type = ?");
        count_args.s(trigger.clone());
    }
    if let Some(enabled) = query.enabled {
        where_clause.push_str(" AND enabled = ?");
        count_args.b(enabled);
    }
    let total = conn
        .fetch_optional(
            &format!("SELECT COUNT(*) AS n FROM rules{where_clause}"),
            count_args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Page query binds in the same order as the count query: team,
    // [trigger], [enabled], page_size, offset.
    let trigger_filter = if query.trigger_type.is_some() {
        " AND trigger_type = ?"
    } else {
        ""
    };
    let enabled_filter = if query.enabled.is_some() {
        " AND enabled = ?"
    } else {
        ""
    };
    let mut args = Args::new();
    args.s(&team_id);
    if let Some(trigger) = &query.trigger_type {
        args.s(trigger.clone());
    }
    if let Some(enabled) = query.enabled {
        args.b(enabled);
    }
    args.i(i64::from(page_size)).i(offset);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT id, team_id, name, description, trigger_type, enabled, \
                 current_version, condition_json, actions_json, created_by, created_at, \
                 updated_at, archived_at FROM rules \
                 WHERE team_id = ? AND archived_at IS NULL{trigger_filter}{enabled_filter} \
                 ORDER BY created_at DESC, id LIMIT ? OFFSET ?"
            ),
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(rule_row_to_json)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({
        "items": items,
        "page": page,
        "page_size": page_size,
        "total": total,
    })))
}

async fn get_rule(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(rule_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let mut conn = state.db.conn();
    let (_, rule_doc, role) = resolve_rule_access(&mut conn, rule_id.as_str(), &user.id).await?;
    require_manage_rules(role)?;
    Ok(Json(rule_doc))
}

// ---------------------------------------------------------------------------
// Update (immutable versions + optimistic concurrency)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct UpdateRuleRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub trigger_type: Option<String>,
    #[serde(default)]
    pub condition: Option<Value>,
    #[serde(default)]
    pub actions: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Optimistic concurrency: the caller's known `current_version`.
    pub expected_version: i64,
}

async fn update_rule(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(rule_id): Path<String>,
    Json(body): Json<UpdateRuleRequest>,
) -> Result<Json<Value>, AppError> {
    let rule_id = rule_id.clone();
    let mut conn = state.db.begin().await?;
    let (team_id, rule_doc, role) =
        resolve_rule_access(&mut conn, rule_id.as_str(), &user.id).await?;
    require_manage_rules(role)?;
    if rule_doc["archived_at"].as_i64().is_some() {
        return Err(AppError::Conflict("archived rules are read-only".into()));
    }
    let current_version = rule_doc["current_version"].as_i64().unwrap_or(1);
    if body.expected_version != current_version {
        return Err(AppError::Conflict(format!(
            "version conflict: expected {expected}, current is {current_version}",
            expected = body.expected_version,
        )));
    }

    // Field-level validation through the Task 1 model.
    let name = match &body.name {
        Some(n) => super::validate_name(n).map_err(super::RuleError::from)?,
        None => rule_doc["name"].as_str().unwrap_or_default().to_string(),
    };
    let description = match &body.description {
        Some(d) => super::validate_description(d).map_err(super::RuleError::from)?,
        None => rule_doc["description"].as_str().map(str::to_string),
    };
    let trigger_str = body.trigger_type.clone().unwrap_or_else(|| {
        rule_doc["trigger_type"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    });
    let trigger = TriggerType::parse(&trigger_str).ok_or_else(|| {
        rule_field_error("trigger_type", format!("unknown trigger `{trigger_str}`"))
    })?;
    let condition_json = if let Some(v) = &body.condition {
        serde_json::to_string(v).map_err(|e| AppError::internal(e.to_string()))?
    } else {
        let stored = rule_doc["condition"].clone();
        serde_json::to_string(&stored).map_err(|e| AppError::internal(e.to_string()))?
    };
    let actions_json = if let Some(v) = &body.actions {
        serde_json::to_string(v).map_err(|e| AppError::internal(e.to_string()))?
    } else {
        let stored = rule_doc["actions"].clone();
        serde_json::to_string(&stored).map_err(|e| AppError::internal(e.to_string()))?
    };
    let enabled = body
        .enabled
        .unwrap_or(rule_doc["enabled"].as_bool().unwrap_or(false));
    let definition = super::RuleDefinition::from_json(trigger, &condition_json, &actions_json)?;
    let new_version = current_version + 1;
    let now = util::now_ms();

    // Immutable version first, then the guarded current-version bump: a lost
    // CAS race rolls the version row back with the transaction.
    let mut args = Args::new();
    args.s(&rule_id)
        .i(new_version)
        .s(trigger.as_str())
        .s(definition.condition_json())
        .s(definition.actions_json())
        .s(&user.id)
        .i(now);
    conn.execute(
        "INSERT INTO rule_versions (rule_id, version, trigger_type, condition_json, \
         actions_json, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.s(&name)
        .opt_s(description.clone())
        .s(trigger.as_str())
        .b(enabled)
        .i(new_version)
        .s(definition.condition_json())
        .s(definition.actions_json())
        .i(now)
        .s(&rule_id)
        .i(current_version);
    let affected = conn
        .execute(
            "UPDATE rules SET name = ?, description = ?, trigger_type = ?, enabled = ?, \
             current_version = ?, condition_json = ?, actions_json = ?, updated_at = ? \
             WHERE id = ? AND current_version = ? AND archived_at IS NULL",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::Conflict(
            "version conflict: the rule was modified concurrently".into(),
        ));
    }
    activity_record(
        &mut conn,
        &team_id,
        &user.id,
        "rule_updated",
        Some(&format!("updated rule \"{name}\" to version {new_version}")),
    )
    .await?;
    conn.commit().await?;

    let (_, updated) = find_rule(&mut state.db.conn(), &rule_id)
        .await?
        .ok_or_else(|| AppError::internal("rule vanished after update"))?;
    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// Enable / disable / archive
// ---------------------------------------------------------------------------

async fn set_enabled(
    state: &AppState,
    user: &AuthedUser,
    rule_id: &str,
    enabled: bool,
    event: &str,
) -> Result<Json<Value>, AppError> {
    let mut conn = state.db.begin().await?;
    let (team_id, _rule, role) = resolve_rule_access(&mut conn, rule_id, &user.id).await?;
    require_manage_rules(role)?;
    if enabled {
        // Archived rules cannot be re-enabled; unarchive is out of Phase 8A.
        let (_, rule_doc) = find_rule(&mut conn, rule_id)
            .await?
            .ok_or(AppError::NotFound("rule not found"))?;
        if rule_doc["archived_at"].as_i64().is_some() {
            return Err(AppError::Conflict("archived rules are read-only".into()));
        }
    }
    let now = util::now_ms();
    let mut args = Args::new();
    args.b(enabled).i(now).s(rule_id);
    conn.execute(
        "UPDATE rules SET enabled = ?, updated_at = ? WHERE id = ? AND archived_at IS NULL",
        args,
    )
    .await?;
    activity_record(&mut conn, team_id.as_str(), &user.id, event, None).await?;
    conn.commit().await?;
    let (_, rule_doc) = find_rule(&mut state.db.conn(), rule_id)
        .await?
        .ok_or(AppError::NotFound("rule not found"))?;
    Ok(Json(rule_doc))
}

async fn enable_rule(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(rule_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    set_enabled(&state, &user, &rule_id, true, "rule_enabled").await
}

async fn disable_rule(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(rule_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    set_enabled(&state, &user, &rule_id, false, "rule_disabled").await
}

async fn archive_rule(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(rule_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let rule_id = rule_id.clone();
    let mut conn = state.db.begin().await?;
    let (team_id, _rule, role) = resolve_rule_access(&mut conn, rule_id.as_str(), &user.id).await?;
    require_manage_rules(role)?;
    let now = util::now_ms();
    let mut args = Args::new();
    args.i(now).i(now).b(false).s(&rule_id);
    conn.execute(
        "UPDATE rules SET archived_at = ?, updated_at = ?, enabled = ? \
         WHERE id = ? AND archived_at IS NULL",
        args,
    )
    .await?;
    activity_record(&mut conn, &team_id, &user.id, "rule_archived", None).await?;
    conn.commit().await?;
    let (_, rule_doc) = find_rule(&mut state.db.conn(), &rule_id)
        .await?
        .ok_or(AppError::NotFound("rule not found"))?;
    Ok(Json(rule_doc))
}

// ---------------------------------------------------------------------------
// Version history
// ---------------------------------------------------------------------------

async fn list_rule_versions(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(rule_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let mut conn = state.db.conn();
    let (_, _rule_doc, role) = resolve_rule_access(&mut conn, rule_id.as_str(), &user.id).await?;
    require_manage_rules(role)?;
    let mut args = Args::new();
    args.s(&rule_id);
    let rows = conn
        .fetch_all(
            "SELECT version, trigger_type, condition_json, actions_json, created_by, \
             created_at FROM rule_versions WHERE rule_id = ? ORDER BY version DESC",
            args,
        )
        .await?;
    let items = rows
        .iter()
        .map(|r| {
            let condition: Value = serde_json::from_str(&r.try_get::<String, _>("condition_json")?)
                .map_err(|e| AppError::internal(e.to_string()))?;
            let actions: Value = serde_json::from_str(&r.try_get::<String, _>("actions_json")?)
                .map_err(|e| AppError::internal(e.to_string()))?;
            Ok(serde_json::json!({
                "version": r.try_get::<i64, _>("version")?,
                "trigger_type": r.try_get::<String, _>("trigger_type")?,
                "condition": condition,
                "actions": actions,
                "created_by": r.try_get::<String, _>("created_by")?,
                "created_at": r.try_get::<i64, _>("created_at")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(serde_json::json!({ "items": items })))
}

// ---------------------------------------------------------------------------
// Execution history
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListHistoryQuery {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

async fn list_rule_history(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(rule_id): Path<String>,
    Query(query): Query<ListHistoryQuery>,
) -> Result<Json<Value>, AppError> {
    let mut conn = state.db.conn();
    let (_, _rule_doc, role) = resolve_rule_access(&mut conn, rule_id.as_str(), &user.id).await?;
    require_history_access(role)?;

    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(25).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);

    let mut args = Args::new();
    args.s(&rule_id);
    let total = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM rule_executions WHERE rule_id = ?",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    let mut args = Args::new();
    args.s(&rule_id).i(i64::from(page_size)).i(offset);
    let rows = conn
        .fetch_all(
            "SELECT e.id, e.event_id, e.rule_version, e.trigger_type, e.condition_result, \
                    e.status, e.planned_actions, e.error_category, e.error_detail, \
                    e.created_at, e.completed_at, de.event_type, de.occurred_at, \
                    de.actor_id, de.origin \
             FROM rule_executions e \
             JOIN domain_events de ON de.id = e.event_id \
             WHERE e.rule_id = ? \
             ORDER BY e.created_at DESC, e.id LIMIT ? OFFSET ?",
            args,
        )
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let execution_id: String = row.try_get("id")?;
        // Action-level outcomes for this page's executions (bounded second
        // query, not N+1: one query per page).
        let mut args = Args::new();
        args.s(&execution_id);
        let actions = conn
            .fetch_all(
                "SELECT ar.action_type, ar.outcome, ar.detail, o.status AS outbox_status \
                 FROM rule_action_results ar \
                 JOIN automation_outbox o ON o.id = ar.outbox_id \
                 WHERE ar.execution_id = ?",
                args,
            )
            .await?;
        let action_items: Vec<Value> = actions
            .iter()
            .map(|a| {
                Ok(serde_json::json!({
                    "action_type": a.try_get::<String, _>("action_type")?,
                    "outcome": a.try_get::<String, _>("outcome")?,
                    "outbox_status": a.try_get::<String, _>("outbox_status")?,
                    "detail": a.try_get::<Option<String>, _>("detail")?,
                }))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        items.push(serde_json::json!({
            "id": execution_id,
            "rule_version": row.try_get::<i64, _>("rule_version")?,
            "event_id": row.try_get::<String, _>("event_id")?,
            "event_type": row.try_get::<String, _>("event_type")?,
            "event_origin": row.try_get::<String, _>("origin")?,
            "occurred_at": row.try_get::<i64, _>("occurred_at")?,
            "actor_id": row.try_get::<Option<String>, _>("actor_id")?,
            "trigger_type": row.try_get::<String, _>("trigger_type")?,
            "condition_result": row.try_get::<String, _>("condition_result")?,
            "status": row.try_get::<String, _>("status")?,
            "planned_actions": row.try_get::<Option<String>, _>("planned_actions")?,
            "error_category": row.try_get::<Option<String>, _>("error_category")?,
            "error_detail": row.try_get::<Option<String>, _>("error_detail")?,
            "created_at": row.try_get::<i64, _>("created_at")?,
            "completed_at": row.try_get::<Option<i64>, _>("completed_at")?,
            "actions": action_items,
        }));
    }
    Ok(Json(serde_json::json!({
        "items": items,
        "page": page,
        "page_size": page_size,
        "total": total,
    })))
}
