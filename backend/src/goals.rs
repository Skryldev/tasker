//! Phase 4: team-level Goals & Key Results.
//!
//! Domain semantics (see ADR-0010):
//! - Goals carry a lifecycle (`draft` → `active` → `completed`/`cancelled`,
//!   with reopen to `active`); measured progress is separate from lifecycle.
//! - Key Results are measurable outcomes of three kinds: increase, decrease,
//!   boolean. Progress is computed server-side by [`kr_progress`] — never
//!   accepted from the client.
//! - Overall goal progress is the arithmetic mean of Key Result progress;
//!   a goal without Key Results has undefined progress (`null`).
//! - Associated projects/tasks contribute an execution summary (task counts)
//!   that is deliberately distinct from outcome progress.
//! - Activation requires at least one Key Result: goals must be measurable
//!   before they go live.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::db::{Args, Conn};
use crate::error::{AppError, FieldError};
use crate::projects::require_project_access;
use crate::state::AppState;
use crate::users::UserBrief;
use crate::util;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/teams/{team_id}/goals", get(list_goals).post(create_goal))
        .route(
            "/goals/{goal_id}",
            get(get_goal).patch(update_goal).delete(delete_goal),
        )
        .route("/goals/{goal_id}/status", post(transition_goal))
        .route("/goals/{goal_id}/activity", get(goal_activity))
        .route(
            "/goals/{goal_id}/key-results",
            get(list_key_results).post(create_key_result),
        )
        .route(
            "/key-results/{key_result_id}",
            get(get_key_result)
                .patch(update_key_result)
                .delete(delete_key_result),
        )
        .route(
            "/goals/{goal_id}/projects",
            get(list_goal_projects).post(associate_project),
        )
        .route(
            "/goals/{goal_id}/projects/{project_id}",
            delete(remove_project_association),
        )
        .route(
            "/goals/{goal_id}/tasks",
            get(list_goal_tasks).post(associate_task),
        )
        .route(
            "/goals/{goal_id}/tasks/{task_id}",
            delete(remove_task_association),
        )
}

pub mod events {
    pub const GOAL_CREATED: &str = "goal_created";
    pub const GOAL_UPDATED: &str = "goal_updated";
    pub const GOAL_STATUS_CHANGED: &str = "goal_status_changed";
    pub const GOAL_DELETED: &str = "goal_deleted";
    pub const KR_CREATED: &str = "goal_key_result_created";
    pub const KR_UPDATED: &str = "goal_key_result_updated";
    pub const KR_DELETED: &str = "goal_key_result_deleted";
    pub const GOAL_PROJECT_ADDED: &str = "goal_project_associated";
    pub const GOAL_PROJECT_REMOVED: &str = "goal_project_disassociated";
    pub const GOAL_TASK_ADDED: &str = "goal_task_associated";
    pub const GOAL_TASK_REMOVED: &str = "goal_task_disassociated";
}

// ---------------------------------------------------------------------------
// Lifecycle + progress domain logic (pure, unit-tested)
// ---------------------------------------------------------------------------

pub const GOAL_STATUSES: [&str; 4] = ["draft", "active", "completed", "cancelled"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Draft,
    Active,
    Completed,
    Cancelled,
}

impl GoalStatus {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "draft" => Ok(Self::Draft),
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(AppError::validation(
                "status",
                format!("unknown goal status `{other}`; expected one of: {GOAL_STATUSES:?}"),
            )),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Allowed lifecycle transitions. Reopening (completed/cancelled →
    /// active) is an explicitly authorized transition.
    #[must_use]
    pub fn can_transition_to(self, to: GoalStatus) -> bool {
        matches!(
            (self, to),
            (Self::Draft, Self::Active | Self::Cancelled)
                | (Self::Active, Self::Completed | Self::Cancelled)
                | (Self::Completed | Self::Cancelled, Self::Active)
        )
    }
}

pub const KR_METRIC_TYPES: [&str; 3] = ["increase", "decrease", "boolean"];

/// Validate a Key Result metric configuration and compute progress.
/// One centralized, deterministic function: progress is derived from the
/// metric values, never accepted from the client.
/// Returns `(progress, normalized_current)` so boolean KRs store 0/1.
pub fn kr_progress(
    metric_type: &str,
    start_value: Option<f64>,
    current_value: f64,
    target_value: Option<f64>,
) -> Result<(f64, f64), AppError> {
    let finite = |v: f64, field: &str| -> Result<f64, AppError> {
        if v.is_finite() {
            Ok(v)
        } else {
            Err(AppError::validation(field, "value must be a finite number"))
        }
    };
    match metric_type {
        "boolean" => {
            let current = finite(current_value, "current_value")?;
            if (current - 0.0).abs() > f64::EPSILON && (current - 1.0).abs() > f64::EPSILON {
                return Err(AppError::validation(
                    "current_value",
                    "boolean key results accept only 0 (not achieved) or 1 (achieved)",
                ));
            }
            Ok((current * 100.0, current))
        }
        "increase" => {
            let start = start_value.ok_or_else(|| {
                AppError::validation("start_value", "increase metrics require a start value")
            })?;
            let target = target_value.ok_or_else(|| {
                AppError::validation("target_value", "increase metrics require a target value")
            })?;
            let start = finite(start, "start_value")?;
            let target = finite(target, "target_value")?;
            let current = finite(current_value, "current_value")?;
            if target <= start {
                return Err(AppError::validation(
                    "target_value",
                    "increase metrics require target_value > start_value",
                ));
            }
            let progress = (current - start) / (target - start) * 100.0;
            Ok((progress.clamp(0.0, 100.0), current))
        }
        "decrease" => {
            let start = start_value.ok_or_else(|| {
                AppError::validation("start_value", "decrease metrics require a start value")
            })?;
            let target = target_value.ok_or_else(|| {
                AppError::validation("target_value", "decrease metrics require a target value")
            })?;
            let start = finite(start, "start_value")?;
            let target = finite(target, "target_value")?;
            let current = finite(current_value, "current_value")?;
            if target >= start {
                return Err(AppError::validation(
                    "target_value",
                    "decrease metrics require target_value < start_value",
                ));
            }
            let progress = (start - current) / (start - target) * 100.0;
            Ok((progress.clamp(0.0, 100.0), current))
        }
        other => Err(AppError::validation(
            "metric_type",
            format!("unknown metric type `{other}`; expected one of: {KR_METRIC_TYPES:?}"),
        )),
    }
}

/// Overall goal progress: arithmetic mean of Key Result progress, or `None`
/// when the goal has no Key Results (undefined, not 0%).
#[must_use]
pub fn goal_progress(kr_progresses: &[f64]) -> Option<f64> {
    if kr_progresses.is_empty() {
        return None;
    }
    let sum: f64 = kr_progresses.iter().sum();
    Some(sum / f64::from(u32::try_from(kr_progresses.len()).unwrap_or(u32::MAX)))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Goal {
    pub id: String,
    pub team_id: String,
    pub title: String,
    pub description: Option<String>,
    pub owner: Option<UserBrief>,
    pub status: String,
    pub start_date: Option<String>,
    pub due_date: Option<String>,
    pub created_by: Option<UserBrief>,
    pub completed_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Mean Key Result progress (0-100); null while the goal has no KRs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_result_count: Option<i64>,
}

const FIND_GOAL_SQL: &str = "SELECT g.id, g.team_id, g.title, g.description, g.owner_id, g.status, g.start_date, g.due_date, g.completed_at, g.created_by, g.created_at, g.updated_at, u.email AS owner_email, u.display_name AS owner_name, c.email AS creator_email, c.display_name AS creator_name FROM goals g JOIN users u ON u.id = g.owner_id JOIN users c ON c.id = g.created_by WHERE g.id = ?";

fn row_to_goal(row: &sqlx::any::AnyRow) -> Result<Goal, AppError> {
    let owner_email: String = row.try_get("owner_email")?;
    let creator_email: Option<String> = row.try_get("creator_email")?;
    Ok(Goal {
        id: row.try_get("id")?,
        team_id: row.try_get("team_id")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        owner: Some(UserBrief {
            id: row.try_get("owner_id")?,
            display_name: row.try_get("owner_name")?,
            email: owner_email,
        }),
        status: row.try_get("status")?,
        start_date: row.try_get("start_date")?,
        due_date: row.try_get("due_date")?,
        created_by: creator_email.map(|email| {
            let id: Option<String> = row.try_get("created_by").unwrap_or(None);
            let name: Option<String> = row.try_get("creator_name").unwrap_or(None);
            UserBrief {
                id: id.unwrap_or_default(),
                display_name: name.unwrap_or_default(),
                email,
            }
        }),
        completed_at: row.try_get("completed_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        progress: None,
        key_result_count: None,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyResult {
    pub id: String,
    pub goal_id: String,
    pub title: String,
    pub description: Option<String>,
    pub metric_type: String,
    pub start_value: Option<f64>,
    pub current_value: f64,
    pub target_value: Option<f64>,
    pub unit: Option<String>,
    /// Computed server-side; 0-100 clamped. 100% means the metric target was
    /// reached — it does NOT complete the goal.
    pub progress: f64,
    pub position: f64,
    pub created_at: i64,
    pub updated_at: i64,
}

const LIST_KR_SQL: &str = "SELECT id, goal_id, title, description, metric_type, start_value, current_value, target_value, unit, progress, position, created_at, updated_at FROM key_results WHERE goal_id = ? ORDER BY position, created_at, id";

const FIND_KR_SQL: &str = "SELECT id, goal_id, title, description, metric_type, start_value, current_value, target_value, unit, progress, position, created_at, updated_at FROM key_results WHERE id = ?";

fn row_to_key_result(row: &sqlx::any::AnyRow) -> Result<KeyResult, AppError> {
    Ok(KeyResult {
        id: row.try_get("id")?,
        goal_id: row.try_get("goal_id")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        metric_type: row.try_get("metric_type")?,
        start_value: row.try_get("start_value")?,
        current_value: row.try_get("current_value")?,
        target_value: row.try_get("target_value")?,
        unit: row.try_get("unit")?,
        progress: row.try_get("progress")?,
        position: row.try_get("position")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

async fn find_goal(conn: &mut Conn<'_>, goal_id: &str) -> Result<Goal, AppError> {
    let mut args = Args::new();
    args.s(goal_id);
    let row = conn
        .fetch_optional(FIND_GOAL_SQL, args)
        .await?
        .ok_or(AppError::NotFound("goal not found"))?;
    row_to_goal(&row)
}

/// Resolve a goal + caller membership. Team-scoped: guests have no team
/// membership and receive the standard 404.
async fn require_goal_access(
    conn: &mut Conn<'_>,
    goal_id: &str,
    user_id: &str,
) -> Result<(Goal, crate::authorization::Membership), AppError> {
    let goal = find_goal(conn, goal_id).await?;
    let membership = crate::teams::require_membership(conn, &goal.team_id, user_id).await?;
    Ok((goal, membership))
}

async fn load_key_results(conn: &mut Conn<'_>, goal_id: &str) -> Result<Vec<KeyResult>, AppError> {
    let mut args = Args::new();
    args.s(goal_id);
    let rows = conn.fetch_all(LIST_KR_SQL, args).await?;
    rows.iter().map(row_to_key_result).collect()
}

fn set_goal_progress_fields(goal: &mut Goal, krs: &[KeyResult]) {
    goal.key_result_count = Some(i64::try_from(krs.len()).unwrap_or(i64::MAX));
    goal.progress = goal_progress(&krs.iter().map(|kr| kr.progress).collect::<Vec<_>>());
}

/// Record a goal-scoped activity event using the nullable `goal_id` column on
/// the existing activities table (`task_id` NULL) — task history and the guest
/// whitelist are untouched.
async fn record_goal_activity(
    conn: &mut Conn<'_>,
    goal: &Goal,
    actor_id: &str,
    event: &str,
    field: Option<&str>,
    old_value: Option<&str>,
    new_value: Option<&str>,
) -> Result<(), AppError> {
    let sql = "INSERT INTO activities (id, team_id, project_id, task_id, actor_id, event, field, old_value, new_value, created_at, goal_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    let mut args = Args::new();
    args.s(util::new_id())
        .s(&goal.team_id)
        .opt_s(None::<String>)
        .opt_s(None::<String>)
        .opt_s(Some(actor_id))
        .s(event)
        .opt_s(field)
        .opt_s(old_value)
        .opt_s(new_value)
        .i(util::now_ms())
        .opt_s(Some(goal.id.clone()));
    conn.execute(sql, args).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Goal CRUD + lifecycle
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListGoalsQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

async fn list_goals(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Query(query): Query<ListGoalsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    crate::teams::require_membership(&mut conn, &team_id, &user.id).await?;
    let status = match query.status.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => {
            GoalStatus::parse(s)?;
            Some(s.to_string())
        }
        None => None,
    };
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(25).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);

    let filter = if status.is_some() {
        " AND g.status = ?"
    } else {
        ""
    };
    let count_sql = format!("SELECT COUNT(*) AS n FROM goals g WHERE g.team_id = ?{filter}");
    let mut count_args = Args::new();
    count_args.s(&team_id);
    if let Some(s) = &status {
        count_args.s(s.clone());
    }
    let total = conn
        .fetch_optional(&count_sql, count_args)
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    let list_sql = format!(
        "SELECT g.id, g.team_id, g.title, g.description, g.owner_id, g.status, g.start_date, g.due_date, g.completed_at, g.created_by, g.created_at, g.updated_at, u.email AS owner_email, u.display_name AS owner_name, c.email AS creator_email, c.display_name AS creator_name FROM goals g JOIN users u ON u.id = g.owner_id JOIN users c ON c.id = g.created_by WHERE g.team_id = ?{filter} ORDER BY g.created_at DESC, g.id LIMIT ? OFFSET ?"
    );
    let mut args = Args::new();
    args.s(&team_id);
    if let Some(s) = &status {
        args.s(s.clone());
    }
    args.i(i64::from(page_size)).i(offset);
    let rows = conn.fetch_all(&list_sql, args).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let mut goal = row_to_goal(&row)?;
        let krs = load_key_results(&mut conn, &goal.id).await?;
        set_goal_progress_fields(&mut goal, &krs);
        items.push(goal);
    }
    Ok(Json(
        json!({ "items": items, "page": page, "page_size": page_size, "total": total }),
    ))
}

#[derive(Deserialize)]
pub struct CreateGoalRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Team member responsible for the outcome. Defaults to the caller.
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
}

fn parse_date(errors: &mut Vec<FieldError>, field: &str, raw: Option<&str>) -> Option<String> {
    let raw = raw.map(str::trim).filter(|d| !d.is_empty())?;
    match util::parse_iso_date(raw) {
        Ok(_) => Some(raw.to_string()),
        Err(e) => {
            errors.push(FieldError::new(field, e));
            None
        }
    }
}

fn validate_goal_input(
    title: &str,
    description: Option<&str>,
    start_date: Option<&str>,
    due_date: Option<&str>,
    errors: &mut Vec<FieldError>,
) -> (String, Option<String>, Option<String>, Option<String>) {
    let title = title.trim().to_string();
    if title.is_empty() || title.chars().count() > 250 {
        errors.push(FieldError::new(
            "title",
            "goal title must be 1-250 characters",
        ));
    }
    if description.is_some_and(|d| d.chars().count() > 5000) {
        errors.push(FieldError::new(
            "description",
            "description must be at most 5000 characters",
        ));
    }
    let start = parse_date(errors, "start_date", start_date);
    let due = parse_date(errors, "due_date", due_date);
    if start
        .as_ref()
        .is_some_and(|s| due.as_ref().is_some_and(|d| d < s))
    {
        errors.push(FieldError::new(
            "due_date",
            "due_date must not precede start_date",
        ));
    }
    (
        title,
        description.map(std::string::ToString::to_string),
        start,
        due,
    )
}

async fn create_goal(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Json(body): Json<CreateGoalRequest>,
) -> Result<(StatusCode, Json<Goal>), AppError> {
    let mut conn = state.db.begin().await?;
    let membership = crate::teams::require_membership(&mut conn, &team_id, &user.id).await?;
    if !membership
        .role
        .can(crate::authorization::Capability::ManageGoals)
    {
        return Err(AppError::Forbidden(
            "managing goals requires the Manager role or above",
        ));
    }
    let mut validation_errors = Vec::new();
    let (title, description, start_date, due_date) = validate_goal_input(
        &body.title,
        body.description.as_deref(),
        body.start_date.as_deref(),
        body.due_date.as_deref(),
        &mut validation_errors,
    );
    if !validation_errors.is_empty() {
        return Err(AppError::Validation(validation_errors));
    }

    // Owner defaults to the caller; must be a team member.
    let owner_id = match body.owner_id.as_deref() {
        Some(id) if !id.is_empty() => {
            crate::teams::require_membership(&mut conn, &team_id, id)
                .await
                .map_err(|_| {
                    AppError::validation("owner_id", "the owner must be a member of the team")
                })?;
            id.to_string()
        }
        _ => user.id.clone(),
    };

    let id = util::new_id();
    let now = util::now_ms();
    let insert_sql = "INSERT INTO goals (id, team_id, title, description, owner_id, status, start_date, due_date, completed_at, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    let mut args = Args::new();
    args.s(&id)
        .s(&team_id)
        .s(&title)
        .opt_s(description.clone())
        .s(&owner_id)
        .s("draft")
        .opt_s(start_date.clone())
        .opt_s(due_date.clone())
        .opt_i(None::<i64>)
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(insert_sql, args).await?;
    let goal = find_goal(&mut conn, &id).await?;
    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::GOAL_CREATED,
        None,
        None,
        Some(&title),
    )
    .await?;
    conn.commit().await?;
    Ok((StatusCode::CREATED, Json(goal)))
}

#[derive(Deserialize)]
pub struct UpdateGoalRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
}

async fn update_goal(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
    Json(body): Json<UpdateGoalRequest>,
) -> Result<Json<Goal>, AppError> {
    let mut conn = state.db.begin().await?;
    let (goal, membership) = require_goal_access(&mut conn, &goal_id, &user.id).await?;
    if !membership
        .role
        .can(crate::authorization::Capability::ManageGoals)
    {
        return Err(AppError::Forbidden(
            "managing goals requires the Manager role or above",
        ));
    }

    let title = body.title.as_deref().unwrap_or(&goal.title);
    let description = body.description.as_deref().or(goal.description.as_deref());
    let start = body.start_date.as_deref().or(goal.start_date.as_deref());
    let due = body.due_date.as_deref().or(goal.due_date.as_deref());
    let mut validation_errors = Vec::new();
    let (title, description, start_date, due_date) =
        validate_goal_input(title, description, start, due, &mut validation_errors);
    if !validation_errors.is_empty() {
        return Err(AppError::Validation(validation_errors));
    }

    let owner_id = match body.owner_id.as_deref() {
        Some(id) if !id.is_empty() => {
            crate::teams::require_membership(&mut conn, &goal.team_id, id)
                .await
                .map_err(|_| {
                    AppError::validation("owner_id", "the owner must be a member of the team")
                })?;
            id.to_string()
        }
        _ => goal.owner.as_ref().expect("owner joined").id.clone(),
    };

    let now = util::now_ms();
    let update_sql = "UPDATE goals SET title = ?, description = ?, owner_id = ?, start_date = ?, due_date = ?, updated_at = ? WHERE id = ?";
    let mut args = Args::new();
    args.s(&title)
        .opt_s(description.clone())
        .s(&owner_id)
        .opt_s(start_date.clone())
        .opt_s(due_date.clone())
        .i(now)
        .s(&goal_id);
    conn.execute(update_sql, args).await?;

    // Metadata edits never change status.
    let updated = find_goal(&mut conn, &goal_id).await?;
    if updated.title != goal.title || updated.description != goal.description {
        record_goal_activity(
            &mut conn,
            &updated,
            &user.id,
            events::GOAL_UPDATED,
            Some("metadata"),
            Some(&goal.title),
            Some(&updated.title),
        )
        .await?;
    }
    let old_owner = goal.owner.as_ref().map(|o| o.id.clone());
    let new_owner = updated.owner.as_ref().map(|o| o.id.clone());
    if old_owner != new_owner {
        record_goal_activity(
            &mut conn,
            &updated,
            &user.id,
            events::GOAL_UPDATED,
            Some("owner_id"),
            old_owner.as_deref(),
            new_owner.as_deref(),
        )
        .await?;
    }
    conn.commit().await?;
    Ok(Json(updated))
}

async fn delete_goal(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let goal = require_goal_manager(&mut conn, &goal_id, &user.id).await?;
    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::GOAL_DELETED,
        Some("title"),
        None,
        Some(&goal.title),
    )
    .await?;
    // KRs and association rows cascade via FK; tasks and projects survive.
    let delete_sql = "DELETE FROM goals WHERE id = ?";
    let mut args = Args::new();
    args.s(&goal_id);
    conn.execute(delete_sql, args).await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct TransitionGoalRequest {
    pub status: String,
}

async fn transition_goal(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
    Json(body): Json<TransitionGoalRequest>,
) -> Result<Json<Goal>, AppError> {
    let to = GoalStatus::parse(&body.status)?;
    let mut conn = state.db.begin().await?;
    let (goal, membership) = require_goal_access(&mut conn, &goal_id, &user.id).await?;
    if !membership
        .role
        .can(crate::authorization::Capability::ManageGoals)
    {
        return Err(AppError::Forbidden(
            "managing goals requires the Manager role or above",
        ));
    }
    let from = GoalStatus::parse(&goal.status)?;
    if from == to {
        return Err(AppError::Conflict(
            "the goal is already in this status".into(),
        ));
    }
    if !from.can_transition_to(to) {
        return Err(AppError::Conflict(format!(
            "cannot transition a {} goal to {}",
            from.as_str(),
            to.as_str()
        )));
    }
    // A goal must be measurable before it goes live.
    if to == GoalStatus::Active {
        let krs = load_key_results(&mut conn, &goal_id).await?;
        if krs.is_empty() {
            return Err(AppError::Conflict(
                "add at least one key result before activating the goal".into(),
            ));
        }
    }

    let now = util::now_ms();
    let completed_at = if to == GoalStatus::Completed {
        Some(now)
    } else {
        None
    };
    let transition_sql =
        "UPDATE goals SET status = ?, completed_at = ?, updated_at = ? WHERE id = ?";
    let mut args = Args::new();
    args.s(to.as_str()).opt_i(completed_at).i(now).s(&goal_id);
    conn.execute(transition_sql, args).await?;
    let updated = find_goal(&mut conn, &goal_id).await?;
    record_goal_activity(
        &mut conn,
        &updated,
        &user.id,
        events::GOAL_STATUS_CHANGED,
        Some("status"),
        Some(from.as_str()),
        Some(to.as_str()),
    )
    .await?;
    conn.commit().await?;
    Ok(Json(updated))
}

async fn get_goal(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let (goal, _) = require_goal_access(&mut conn, &goal_id, &user.id).await?;
    let krs = load_key_results(&mut conn, &goal_id).await?;

    // Task execution summary: one grouped query over associated tasks.
    let exec_sql = "SELECT t.status, COUNT(*) AS n FROM goal_tasks gt JOIN tasks t ON t.id = gt.task_id WHERE gt.goal_id = ? GROUP BY t.status";
    let mut args = Args::new();
    args.s(&goal_id);
    let exec_rows = conn.fetch_all(exec_sql, args).await?;
    let mut by_status = BTreeMap::new();
    let mut total = 0i64;
    for row in &exec_rows {
        let status: String = row.try_get("status")?;
        let n: i64 = row.try_get("n")?;
        *by_status.entry(status).or_insert(0) += n;
        total += n;
    }
    let done = *by_status.get("closed").unwrap_or(&0);
    #[allow(clippy::cast_precision_loss)]
    let completion = if total > 0 {
        Some((done as f64) / (total as f64) * 100.0)
    } else {
        None
    };

    let project_sql = "SELECT p.id, p.name, p.status FROM goal_projects gp JOIN projects p ON p.id = gp.project_id WHERE gp.goal_id = ? ORDER BY p.created_at";
    let mut args = Args::new();
    args.s(&goal_id);
    let project_rows = conn.fetch_all(project_sql, args).await?;
    let projects = project_rows
        .iter()
        .map(|r| {
            Ok(json!({
                "id": r.try_get::<String, _>("id")?,
                "name": r.try_get::<String, _>("name")?,
                "status": r.try_get::<String, _>("status")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    let task_sql = "SELECT t.id, t.title, t.status FROM goal_tasks gt JOIN tasks t ON t.id = gt.task_id WHERE gt.goal_id = ? ORDER BY t.created_at";
    let mut args = Args::new();
    args.s(&goal_id);
    let task_rows = conn.fetch_all(task_sql, args).await?;
    let tasks = task_rows
        .iter()
        .map(|r| {
            Ok(json!({
                "id": r.try_get::<String, _>("id")?,
                "title": r.try_get::<String, _>("title")?,
                "status": r.try_get::<String, _>("status")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    let mut goal = goal;
    set_goal_progress_fields(&mut goal, &krs);
    Ok(Json(json!({
        "goal": goal,
        "key_results": krs,
        "projects": projects,
        "tasks": tasks,
        "execution": {
            "total": total,
            "by_status": by_status,
            "completed": done,
            "completion_percent": completion,
        },
    })))
}

async fn goal_activity(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_goal_access(&mut conn, &goal_id, &user.id).await?;
    let activity_sql = "SELECT a.id, a.event, a.field, a.old_value, a.new_value, a.created_at, u.id AS actor_id, u.display_name AS actor_name, u.email AS actor_email FROM activities a LEFT JOIN users u ON u.id = a.actor_id WHERE a.goal_id = ? ORDER BY a.created_at DESC, a.id DESC LIMIT 100";
    let mut args = Args::new();
    args.s(&goal_id);
    let rows = conn.fetch_all(activity_sql, args).await?;
    let items = rows
        .iter()
        .map(|row| {
            let actor_id: Option<String> = row.try_get("actor_id")?;
            let actor = match actor_id {
                Some(id) => {
                    let display_name: Option<String> = row.try_get("actor_name")?;
                    let email: Option<String> = row.try_get("actor_email")?;
                    Some(UserBrief {
                        id,
                        display_name: display_name.unwrap_or_default(),
                        email: email.unwrap_or_default(),
                    })
                }
                None => None,
            };
            Ok(json!({
                "id": row.try_get::<String, _>("id")?,
                "event": row.try_get::<String, _>("event")?,
                "field": row.try_get::<Option<String>, _>("field")?,
                "old_value": row.try_get::<Option<String>, _>("old_value")?,
                "new_value": row.try_get::<Option<String>, _>("new_value")?,
                "actor": actor,
                "created_at": row.try_get::<i64, _>("created_at")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(json!({ "items": items })))
}

// ---------------------------------------------------------------------------
// Key Results
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct UpsertKeyResultRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub metric_type: String,
    #[serde(default)]
    pub start_value: Option<f64>,
    #[serde(default)]
    pub current_value: Option<f64>,
    #[serde(default)]
    pub target_value: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
}

struct ValidatedKr {
    title: String,
    description: Option<String>,
    metric_type: String,
    start_value: Option<f64>,
    current_value: f64,
    target_value: Option<f64>,
    unit: Option<String>,
    progress: f64,
}

fn validate_kr(body: &UpsertKeyResultRequest) -> Result<ValidatedKr, AppError> {
    let mut errors = Vec::new();
    let title = body.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 250 {
        errors.push(FieldError::new(
            "title",
            "key result title must be 1-250 characters",
        ));
    }
    let description = body
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    if description.is_some_and(|d| d.chars().count() > 5000) {
        errors.push(FieldError::new(
            "description",
            "description must be at most 5000 characters",
        ));
    }
    let metric_type = match body.metric_type.as_str() {
        "increase" | "decrease" | "boolean" => body.metric_type.clone(),
        other => {
            return Err(AppError::Validation(vec![FieldError::new(
                "metric_type",
                format!("unknown metric type `{other}`"),
            )]));
        }
    };
    let unit = body
        .unit
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    if unit.as_ref().is_some_and(|u| u.chars().count() > 30) {
        errors.push(FieldError::new(
            "unit",
            "unit must be at most 30 characters",
        ));
    }
    // Boolean KRs: start 0, target 1, current 0/1.
    let (start, current, target) = if metric_type == "boolean" {
        (Some(0.0), body.current_value.unwrap_or(0.0), Some(1.0))
    } else {
        (
            body.start_value,
            body.current_value
                .unwrap_or(body.start_value.unwrap_or(0.0)),
            body.target_value,
        )
    };
    if !errors.is_empty() {
        return Err(AppError::Validation(errors));
    }
    // Full metric validation (ranges, finiteness) in one place.
    let (progress, current) = kr_progress(&metric_type, start, current, target)?;
    Ok(ValidatedKr {
        title,
        description: description.map(std::string::ToString::to_string),
        metric_type,
        start_value: start,
        current_value: current,
        target_value: target,
        unit,
        progress,
    })
}

async fn require_goal_manager(
    conn: &mut Conn<'_>,
    goal_id: &str,
    user_id: &str,
) -> Result<Goal, AppError> {
    let (goal, membership) = require_goal_access(conn, goal_id, user_id).await?;
    if !membership
        .role
        .can(crate::authorization::Capability::ManageGoals)
    {
        return Err(AppError::Forbidden(
            "managing goals requires the Manager role or above",
        ));
    }
    Ok(goal)
}

async fn list_key_results(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_goal_access(&mut conn, &goal_id, &user.id).await?;
    let krs = load_key_results(&mut conn, &goal_id).await?;
    Ok(Json(json!({ "items": krs })))
}

async fn get_key_result(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(key_result_id): Path<String>,
) -> Result<Json<KeyResult>, AppError> {
    let mut conn = state.db.conn();
    let kr = find_key_result(&mut conn, &key_result_id).await?;
    require_goal_access(&mut conn, &kr.goal_id, &user.id).await?;
    Ok(Json(kr))
}

async fn find_key_result(conn: &mut Conn<'_>, kr_id: &str) -> Result<KeyResult, AppError> {
    let mut args = Args::new();
    args.s(kr_id);
    let row = conn
        .fetch_optional(FIND_KR_SQL, args)
        .await?
        .ok_or(AppError::NotFound("key result not found"))?;
    row_to_key_result(&row)
}

async fn create_key_result(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
    Json(body): Json<UpsertKeyResultRequest>,
) -> Result<(StatusCode, Json<KeyResult>), AppError> {
    let mut conn = state.db.begin().await?;
    let goal = require_goal_manager(&mut conn, &goal_id, &user.id).await?;
    let validated = validate_kr(&body)?;

    let pos_sql = "SELECT COALESCE(MAX(position), 0) AS max_pos FROM key_results WHERE goal_id = ?";
    let mut args = Args::new();
    args.s(&goal.id);
    let max_row = conn.fetch_optional(pos_sql, args).await?;
    let position = max_row.map_or(0.0, |r| r.try_get::<f64, _>("max_pos").unwrap_or(0.0)) + 1024.0;

    let id = util::new_id();
    let now = util::now_ms();
    let insert_sql = "INSERT INTO key_results (id, goal_id, title, description, metric_type, start_value, current_value, target_value, unit, progress, position, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    let mut args = Args::new();
    args.s(&id)
        .s(&goal.id)
        .s(&validated.title)
        .opt_s(validated.description.clone())
        .s(&validated.metric_type)
        .opt_f(validated.start_value)
        .f(validated.current_value)
        .opt_f(validated.target_value)
        .opt_s(validated.unit.clone())
        .f(validated.progress)
        .f(position)
        .i(now)
        .i(now);
    conn.execute(insert_sql, args).await?;
    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::KR_CREATED,
        Some("title"),
        None,
        Some(&validated.title),
    )
    .await?;
    conn.commit().await?;

    let mut conn = state.db.conn();
    let kr = find_key_result(&mut conn, &id).await?;
    Ok((StatusCode::CREATED, Json(kr)))
}

#[derive(Deserialize)]
pub struct UpdateKeyResultRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metric_type: Option<String>,
    #[serde(default)]
    pub start_value: Option<f64>,
    #[serde(default)]
    pub current_value: Option<f64>,
    #[serde(default)]
    pub target_value: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
}

async fn update_key_result(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(key_result_id): Path<String>,
    Json(body): Json<UpdateKeyResultRequest>,
) -> Result<Json<KeyResult>, AppError> {
    let mut conn = state.db.begin().await?;
    let existing = find_key_result(&mut conn, &key_result_id).await?;
    let goal = require_goal_manager(&mut conn, &existing.goal_id, &user.id).await?;

    let merged = UpsertKeyResultRequest {
        title: body.title.unwrap_or_else(|| existing.title.clone()),
        description: body.description.or_else(|| existing.description.clone()),
        metric_type: body
            .metric_type
            .unwrap_or_else(|| existing.metric_type.clone()),
        start_value: body.start_value.or(existing.start_value),
        current_value: Some(body.current_value.unwrap_or(existing.current_value)),
        target_value: body.target_value.or(existing.target_value),
        unit: body.unit.or_else(|| existing.unit.clone()),
    };
    let validated = validate_kr(&merged)?;

    let now = util::now_ms();
    let update_sql = "UPDATE key_results SET title = ?, description = ?, metric_type = ?, start_value = ?, current_value = ?, target_value = ?, unit = ?, progress = ?, updated_at = ? WHERE id = ?";
    let mut args = Args::new();
    args.s(&validated.title)
        .opt_s(validated.description.clone())
        .s(&validated.metric_type)
        .opt_f(validated.start_value)
        .f(validated.current_value)
        .opt_f(validated.target_value)
        .opt_s(validated.unit.clone())
        .f(validated.progress)
        .i(now)
        .s(&key_result_id);
    conn.execute(update_sql, args).await?;

    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::KR_UPDATED,
        Some("title"),
        Some(&existing.title),
        Some(&validated.title),
    )
    .await?;
    conn.commit().await?;

    let mut conn = state.db.conn();
    Ok(Json(find_key_result(&mut conn, &key_result_id).await?))
}

async fn delete_key_result(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(key_result_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let existing = find_key_result(&mut conn, &key_result_id).await?;
    let goal = require_goal_manager(&mut conn, &existing.goal_id, &user.id).await?;
    let delete_sql = "DELETE FROM key_results WHERE id = ?";
    let mut args = Args::new();
    args.s(&key_result_id);
    conn.execute(delete_sql, args).await?;
    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::KR_DELETED,
        Some("title"),
        Some(&existing.title),
        None,
    )
    .await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Associations
// ---------------------------------------------------------------------------

async fn list_goal_projects(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_goal_access(&mut conn, &goal_id, &user.id).await?;
    let assoc_sql = "SELECT p.id, p.name, p.status FROM goal_projects gp JOIN projects p ON p.id = gp.project_id WHERE gp.goal_id = ? ORDER BY p.created_at";
    let mut args = Args::new();
    args.s(&goal_id);
    let rows = conn.fetch_all(assoc_sql, args).await?;
    let items = rows
        .iter()
        .map(|r| {
            Ok(json!({
                "id": r.try_get::<String, _>("id")?,
                "name": r.try_get::<String, _>("name")?,
                "status": r.try_get::<String, _>("status")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(json!({ "items": items })))
}

async fn list_goal_tasks(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    require_goal_access(&mut conn, &goal_id, &user.id).await?;
    let assoc_sql = "SELECT t.id, t.title, t.status FROM goal_tasks gt JOIN tasks t ON t.id = gt.task_id WHERE gt.goal_id = ? ORDER BY t.created_at";
    let mut args = Args::new();
    args.s(&goal_id);
    let rows = conn.fetch_all(assoc_sql, args).await?;
    let items = rows
        .iter()
        .map(|r| {
            Ok(json!({
                "id": r.try_get::<String, _>("id")?,
                "title": r.try_get::<String, _>("title")?,
                "status": r.try_get::<String, _>("status")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct AssociateProjectRequest {
    pub project_id: String,
}

async fn associate_project(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
    Json(body): Json<AssociateProjectRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let mut conn = state.db.begin().await?;
    let goal = require_goal_manager(&mut conn, &goal_id, &user.id).await?;
    let (project, _) = require_project_access(&mut conn, &body.project_id, &user.id).await?;
    if project.team_id != goal.team_id {
        return Err(AppError::validation(
            "project_id",
            "goals can only be associated with projects in the same team",
        ));
    }
    let exists_sql = "SELECT goal_id FROM goal_projects WHERE goal_id = ? AND project_id = ?";
    let mut args = Args::new();
    args.s(&goal_id).s(&body.project_id);
    let exists = conn.fetch_optional(exists_sql, args).await?.is_some();
    if exists {
        return Err(AppError::Conflict(
            "this project is already associated with the goal".into(),
        ));
    }
    let insert_sql = "INSERT INTO goal_projects (goal_id, project_id, created_at, created_by) VALUES (?, ?, ?, ?)";
    let mut args = Args::new();
    args.s(&goal_id)
        .s(&body.project_id)
        .i(util::now_ms())
        .s(&user.id);
    conn.execute(insert_sql, args).await?;
    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::GOAL_PROJECT_ADDED,
        Some("project"),
        None,
        Some(&project.name),
    )
    .await?;
    conn.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": project.id, "name": project.name, "status": project.status })),
    ))
}

async fn remove_project_association(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((goal_id, project_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let goal = require_goal_manager(&mut conn, &goal_id, &user.id).await?;
    let delete_sql = "DELETE FROM goal_projects WHERE goal_id = ? AND project_id = ?";
    let mut args = Args::new();
    args.s(&goal_id).s(&project_id);
    let affected = conn.execute(delete_sql, args).await?;
    if affected == 0 {
        return Err(AppError::NotFound("association not found"));
    }
    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::GOAL_PROJECT_REMOVED,
        Some("project"),
        None,
        Some(&project_id),
    )
    .await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct AssociateTaskRequest {
    pub task_id: String,
}

async fn associate_task(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(goal_id): Path<String>,
    Json(body): Json<AssociateTaskRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let mut conn = state.db.begin().await?;
    let goal = require_goal_manager(&mut conn, &goal_id, &user.id).await?;
    let (task, _) = crate::tasks::require_task_access(&mut conn, &body.task_id, &user.id).await?;
    if task.team_id != goal.team_id {
        return Err(AppError::validation(
            "task_id",
            "goals can only be associated with tasks in the same team",
        ));
    }
    let exists_sql = "SELECT goal_id FROM goal_tasks WHERE goal_id = ? AND task_id = ?";
    let mut args = Args::new();
    args.s(&goal_id).s(&body.task_id);
    let exists = conn.fetch_optional(exists_sql, args).await?.is_some();
    if exists {
        return Err(AppError::Conflict(
            "this task is already associated with the goal".into(),
        ));
    }
    let insert_sql =
        "INSERT INTO goal_tasks (goal_id, task_id, created_at, created_by) VALUES (?, ?, ?, ?)";
    let mut args = Args::new();
    args.s(&goal_id)
        .s(&body.task_id)
        .i(util::now_ms())
        .s(&user.id);
    conn.execute(insert_sql, args).await?;
    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::GOAL_TASK_ADDED,
        Some("task"),
        None,
        Some(&task.title),
    )
    .await?;
    conn.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": task.id, "title": task.title, "status": task.status })),
    ))
}

async fn remove_task_association(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((goal_id, task_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let goal = require_goal_manager(&mut conn, &goal_id, &user.id).await?;
    let delete_sql = "DELETE FROM goal_tasks WHERE goal_id = ? AND task_id = ?";
    let mut args = Args::new();
    args.s(&goal_id).s(&task_id);
    let affected = conn.execute(delete_sql, args).await?;
    if affected == 0 {
        return Err(AppError::NotFound("association not found"));
    }
    record_goal_activity(
        &mut conn,
        &goal,
        &user.id,
        events::GOAL_TASK_REMOVED,
        Some("task"),
        None,
        Some(&task_id),
    )
    .await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[allow(clippy::float_equality_without_abs, clippy::float_cmp)]
mod domain_tests {
    // These tests assert on exactly representable results (50.0, 100.0, 0.0
    // from halving/doubling), so direct equality is intentional.
    use super::*;

    #[test]
    fn increase_progress() {
        let (p, _) = kr_progress("increase", Some(100.0), 300.0, Some(500.0)).unwrap();
        assert_eq!(p, 50.0);
    }

    #[test]
    fn decrease_progress() {
        let (p, _) = kr_progress("decrease", Some(400.0), 275.0, Some(150.0)).unwrap();
        assert_eq!(p, 50.0);
    }

    #[test]
    fn boolean_progress() {
        let (p0, c0) = kr_progress("boolean", None, 0.0, None).unwrap();
        assert_eq!((p0, c0), (0.0, 0.0));
        let (p1, c1) = kr_progress("boolean", None, 1.0, None).unwrap();
        assert_eq!((p1, c1), (100.0, 1.0));
    }

    #[test]
    fn progress_clamps_beyond_range() {
        let (over, _) = kr_progress("increase", Some(0.0), 900.0, Some(100.0)).unwrap();
        assert_eq!(over, 100.0);
        let (under, _) = kr_progress("increase", Some(100.0), 0.0, Some(500.0)).unwrap();
        assert_eq!(under, 0.0);
        let (dec_over, _) = kr_progress("decrease", Some(400.0), 0.0, Some(150.0)).unwrap();
        assert_eq!(dec_over, 100.0);
    }

    #[test]
    fn invalid_configurations_rejected() {
        assert!(kr_progress("increase", Some(100.0), 0.0, Some(100.0)).is_err());
        assert!(kr_progress("increase", Some(100.0), 0.0, Some(50.0)).is_err());
        assert!(kr_progress("decrease", Some(100.0), 0.0, Some(100.0)).is_err());
        assert!(kr_progress("decrease", Some(50.0), 0.0, Some(150.0)).is_err());
        assert!(kr_progress("increase", None, 0.0, None).is_err());
        assert!(kr_progress("decrease", Some(100.0), 0.0, None).is_err());
        assert!(kr_progress("increase", Some(0.0), f64::NAN, Some(10.0)).is_err());
        assert!(kr_progress("increase", Some(0.0), f64::INFINITY, Some(10.0)).is_err());
        assert!(kr_progress("magic", Some(0.0), 0.0, Some(10.0)).is_err());
        assert!(kr_progress("boolean", None, 0.5, None).is_err());
    }

    #[test]
    fn goal_progress_is_mean_or_none() {
        assert_eq!(goal_progress(&[]), None);
        assert_eq!(goal_progress(&[50.0]), Some(50.0));
        assert_eq!(goal_progress(&[0.0, 100.0]), Some(50.0));
    }

    #[test]
    fn lifecycle_transitions() {
        use GoalStatus::*;
        // Allowed
        assert!(Draft.can_transition_to(Active) && Draft.can_transition_to(Cancelled));
        assert!(Active.can_transition_to(Completed) && Active.can_transition_to(Cancelled));
        assert!(Completed.can_transition_to(Active) && Cancelled.can_transition_to(Active));
        // Forbidden
        assert!(!Draft.can_transition_to(Completed));
        assert!(!Draft.can_transition_to(Draft));
        assert!(!Completed.can_transition_to(Cancelled));
        assert!(!Cancelled.can_transition_to(Completed));
        assert!(!Active.can_transition_to(Active));
        assert!(!Completed.can_transition_to(Draft));
    }

    #[test]
    fn status_parse() {
        assert!(matches!(GoalStatus::parse("draft"), Ok(GoalStatus::Draft)));
        assert!(GoalStatus::parse("bogus").is_err());
    }
}
