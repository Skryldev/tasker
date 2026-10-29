//! Tasks: CRUD, subtasks, labels, filtering/sorting/pagination, board view,
//! and My Tasks. Status changes and field updates are recorded as activity
//! events in the same transaction as the mutation.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use crate::activity::{self, events};
use crate::auth::AuthedUser;
use crate::authorization::{Capability, Membership};
use crate::db::{Args, Conn};
use crate::error::{AppError, FieldError};
use crate::projects::require_project_access;
use crate::state::AppState;
use crate::users::{self, UserBrief};
use crate::util;

/// Canonical status workflow. Centralized and ordered so a configurable
/// workflow can replace it later without reshaping the domain.
pub const STATUSES: [&str; 5] = ["backlog", "todo", "in_progress", "in_review", "done"];

/// Canonical priorities, ascending importance.
pub const PRIORITIES: [&str; 4] = ["low", "medium", "high", "urgent"];

pub fn validate_status(s: &str) -> Result<&'static str, AppError> {
    STATUSES
        .iter()
        .find(|candidate| **candidate == s)
        .copied()
        .ok_or_else(|| {
            AppError::validation(
                "status",
                format!("unknown status `{s}`; expected one of: {STATUSES:?}"),
            )
        })
}

pub fn validate_priority(p: &str) -> Result<&'static str, AppError> {
    PRIORITIES
        .iter()
        .find(|candidate| **candidate == p)
        .copied()
        .ok_or_else(|| {
            AppError::validation(
                "priority",
                format!("unknown priority `{p}`; expected one of: {PRIORITIES:?}"),
            )
        })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project_id}/tasks",
            get(list_tasks).post(create_task),
        )
        .route("/projects/{project_id}/board", get(board))
        .route(
            "/tasks/{task_id}",
            get(get_task).patch(update_task).delete(delete_task),
        )
        .route("/tasks/{task_id}/subtasks", post(create_subtask))
        .route("/me/tasks", get(my_tasks))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct LabelBrief {
    pub id: String,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: String,
    pub team_id: String,
    pub project_id: String,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub assignee: Option<UserBrief>,
    pub created_by: Option<UserBrief>,
    pub due_date: Option<String>,
    pub position: f64,
    pub completed_at: Option<i64>,
    pub labels: Vec<LabelBrief>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtasks: Option<Vec<Task>>,
}

#[derive(Debug, Clone)]
pub struct TaskCore {
    pub id: String,
    pub team_id: String,
    pub project_id: String,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub assignee_id: Option<String>,
    pub created_by: String,
    pub due_date: Option<String>,
    pub position: f64,
    pub completed_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

const TASK_COLUMNS: &str = "id, team_id, project_id, parent_task_id, title, description, status, \
     priority, assignee_id, created_by, due_date, position, completed_at, created_at, updated_at";

fn row_to_task_core(row: &sqlx::any::AnyRow) -> Result<TaskCore, AppError> {
    Ok(TaskCore {
        id: row.try_get("id")?,
        team_id: row.try_get("team_id")?,
        project_id: row.try_get("project_id")?,
        parent_task_id: row.try_get("parent_task_id")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        priority: row.try_get("priority")?,
        assignee_id: row.try_get("assignee_id")?,
        created_by: row.try_get("created_by")?,
        due_date: row.try_get("due_date")?,
        position: row.try_get("position")?,
        completed_at: row.try_get("completed_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn core_to_task(
    core: TaskCore,
    users_by_id: &HashMap<String, UserBrief>,
    labels: Vec<LabelBrief>,
) -> Task {
    Task {
        id: core.id,
        team_id: core.team_id,
        project_id: core.project_id,
        parent_task_id: core.parent_task_id,
        title: core.title,
        description: core.description,
        status: core.status,
        priority: core.priority,
        assignee: core
            .assignee_id
            .as_ref()
            .and_then(|id| users_by_id.get(id).cloned()),
        created_by: users_by_id.get(&core.created_by).cloned(),
        due_date: core.due_date,
        position: core.position,
        completed_at: core.completed_at,
        labels,
        created_at: core.created_at,
        updated_at: core.updated_at,
        subtasks: None,
    }
}

/// Resolve a task and the caller's team membership; outsiders get a 404 so
/// task existence is not disclosed across team boundaries.
pub async fn require_task_access(
    conn: &mut Conn<'_>,
    task_id: &str,
    user_id: &str,
) -> Result<(TaskCore, Membership), AppError> {
    let mut args = Args::new();
    args.s(task_id);
    let row = conn
        .fetch_optional(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?"),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("task not found"))?;
    let task = row_to_task_core(&row)?;
    let membership = crate::teams::require_membership(conn, &task.team_id, user_id).await?;
    Ok((task, membership))
}

/// Attach labels, assignee, and creator to a batch of task cores.
async fn embed_extras(conn: &mut Conn<'_>, cores: Vec<TaskCore>) -> Result<Vec<Task>, AppError> {
    if cores.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = cores.iter().map(|t| t.id.clone()).collect();
    let mut user_ids: Vec<String> = cores
        .iter()
        .flat_map(|t| [t.assignee_id.clone(), Some(t.created_by.clone())])
        .flatten()
        .collect();
    user_ids.sort();
    user_ids.dedup();
    let users_by_id = users::find_many_brief(conn, &user_ids).await?;

    let placeholders = vec!["?"; ids.len()].join(", ");
    let mut args = Args::new();
    for id in &ids {
        args.s(id.clone());
    }
    let label_rows = conn
        .fetch_all(
            &format!(
                "SELECT tl.task_id, l.id, l.name, l.color \
                 FROM task_labels tl JOIN labels l ON l.id = tl.label_id \
                 WHERE tl.task_id IN ({placeholders}) ORDER BY l.name"
            ),
            args,
        )
        .await?;
    let mut labels_by_task: HashMap<String, Vec<LabelBrief>> = HashMap::new();
    for row in label_rows {
        let task_id: String = row.try_get("task_id")?;
        labels_by_task.entry(task_id).or_default().push(LabelBrief {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            color: row.try_get("color")?,
        });
    }

    Ok(cores
        .into_iter()
        .map(|core| {
            let labels = labels_by_task.remove(&core.id).unwrap_or_default();
            core_to_task(core, &users_by_id, labels)
        })
        .collect())
}

async fn task_from_core(state: &AppState, core: TaskCore) -> Result<Task, AppError> {
    let mut conn = state.db.conn();
    let mut tasks = embed_extras(&mut conn, vec![core]).await?;
    tasks
        .pop()
        .ok_or_else(|| AppError::internal("task disappeared after insert"))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

struct TaskInput {
    title: String,
    description: Option<String>,
    status: &'static str,
    priority: &'static str,
    due_date: Option<String>,
}

fn validate_task_fields(
    title: &str,
    description: Option<&str>,
    status: &str,
    priority: &str,
    due_date: Option<&str>,
) -> Result<TaskInput, AppError> {
    let mut errors = Vec::new();
    let title = title.trim().to_string();
    if title.is_empty() || title.chars().count() > 250 {
        errors.push(FieldError::new("title", "title must be 1-250 characters"));
    }
    let description = description.map(str::trim).filter(|d| !d.is_empty());
    if let Some(d) = description
        && d.chars().count() > 10_000
    {
        errors.push(FieldError::new(
            "description",
            "description must be at most 10000 characters",
        ));
    }
    let status = match validate_status(status) {
        Ok(s) => s,
        Err(e) => {
            if let AppError::Validation(fields) = &e {
                errors.extend(fields.iter().cloned());
            }
            "backlog"
        }
    };
    let priority = match validate_priority(priority) {
        Ok(p) => p,
        Err(e) => {
            if let AppError::Validation(fields) = &e {
                errors.extend(fields.iter().cloned());
            }
            "medium"
        }
    };
    let due_date = due_date.map(str::trim).filter(|d| !d.is_empty());
    if let Some(d) = due_date
        && util::parse_iso_date(d).is_err()
    {
        errors.push(FieldError::new(
            "due_date",
            "due date must be a valid YYYY-MM-DD date",
        ));
    }
    if errors.is_empty() {
        Ok(TaskInput {
            title,
            description: description.map(std::string::ToString::to_string),
            status,
            priority,
            due_date: due_date.map(std::string::ToString::to_string),
        })
    } else {
        Err(AppError::Validation(errors))
    }
}

/// Ensure the assignee is a member of the task's team.
async fn validate_assignee(
    conn: &mut Conn<'_>,
    team_id: &str,
    assignee_id: &str,
) -> Result<(), AppError> {
    crate::teams::require_membership(conn, team_id, assignee_id)
        .await
        .map_err(|_| {
            AppError::validation("assignee_id", "the assignee must be a member of the team")
        })?;
    Ok(())
}

/// Validate a subtask parent: must exist in the same project and be a root
/// task (single nesting level).
async fn validate_parent(
    conn: &mut Conn<'_>,
    project_id: &str,
    parent_task_id: &str,
) -> Result<(), AppError> {
    let mut args = Args::new();
    args.s(parent_task_id);
    let row = conn
        .fetch_optional(
            "SELECT project_id, parent_task_id FROM tasks WHERE id = ?",
            args,
        )
        .await?
        .ok_or_else(|| AppError::validation("parent_task_id", "parent task does not exist"))?;
    let parent_project: String = row.try_get("project_id")?;
    let grandparent: Option<String> = row.try_get("parent_task_id")?;
    if parent_project != project_id {
        return Err(AppError::validation(
            "parent_task_id",
            "a subtask must belong to the same project as its parent",
        ));
    }
    if grandparent.is_some() {
        return Err(AppError::validation(
            "parent_task_id",
            "subtasks cannot have their own subtasks (one nesting level)",
        ));
    }
    Ok(())
}

async fn validate_label_ids(
    conn: &mut Conn<'_>,
    team_id: &str,
    label_ids: &[String],
) -> Result<(), AppError> {
    if label_ids.len() > 20 {
        return Err(AppError::validation(
            "label_ids",
            "at most 20 labels per task",
        ));
    }
    for label_id in label_ids {
        let mut args = Args::new();
        args.s(label_id).s(team_id);
        let exists = conn
            .fetch_optional("SELECT id FROM labels WHERE id = ? AND team_id = ?", args)
            .await?
            .is_some();
        if !exists {
            return Err(AppError::validation(
                "label_ids",
                format!("label {label_id} does not exist in this team"),
            ));
        }
    }
    Ok(())
}

async fn insert_task_labels(
    conn: &mut Conn<'_>,
    task_id: &str,
    label_ids: &[String],
) -> Result<(), AppError> {
    for label_id in label_ids {
        let mut args = Args::new();
        args.s(task_id).s(label_id);
        conn.execute(
            "INSERT INTO task_labels (task_id, label_id) VALUES (?, ?)",
            args,
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub label_ids: Vec<String>,
}

async fn create_task(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
    Json(body): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<Task>), AppError> {
    let task = create_task_inner(state, user, project_id, body, None).await?;
    Ok((StatusCode::CREATED, Json(task)))
}

async fn create_subtask(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(parent_task_id): Path<String>,
    Json(body): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<Task>), AppError> {
    {
        let mut conn = state.db.conn();
        let (parent, _) = require_task_access(&mut conn, &parent_task_id, &user.id).await?;
        if parent.parent_task_id.is_some() {
            return Err(AppError::validation(
                "parent_task_id",
                "subtasks cannot have their own subtasks (one nesting level)",
            ));
        }
        create_task_inner(state, user, parent.project_id, body, Some(parent.id)).await
    }
    .map(|task| (StatusCode::CREATED, Json(task)))
}

async fn create_task_inner(
    state: AppState,
    user: AuthedUser,
    project_id: String,
    body: CreateTaskRequest,
    parent_task_id: Option<String>,
) -> Result<Task, AppError> {
    let input = validate_task_fields(
        &body.title,
        body.description.as_deref(),
        body.status.as_deref().unwrap_or("backlog"),
        body.priority.as_deref().unwrap_or("medium"),
        body.due_date.as_deref(),
    )?;

    let mut conn = state.db.begin().await?;
    let (project, membership) = require_project_access(&mut conn, &project_id, &user.id).await?;
    if !membership.role.can(Capability::CreateTask) {
        return Err(AppError::Forbidden(
            "your role does not permit creating tasks",
        ));
    }
    if project.status == "archived" {
        return Err(AppError::Conflict(
            "cannot create tasks in an archived project".into(),
        ));
    }

    let assignee_id = match body.assignee_id.as_deref() {
        Some(id) if !id.is_empty() => {
            validate_assignee(&mut conn, &project.team_id, id).await?;
            Some(id.to_string())
        }
        _ => None,
    };
    if let Some(parent_id) = &parent_task_id {
        validate_parent(&mut conn, &project_id, parent_id).await?;
    }
    validate_label_ids(&mut conn, &project.team_id, &body.label_ids).await?;

    // Position at the bottom of the target status column.
    let mut args = Args::new();
    args.s(&project_id).s(input.status);
    let max_row = conn
        .fetch_optional(
            "SELECT COALESCE(MAX(position), 0) AS max_pos FROM tasks WHERE project_id = ? AND status = ?",
            args,
        )
        .await?;
    let position = max_row.map_or(0.0, |r| r.try_get::<f64, _>("max_pos").unwrap_or(0.0)) + 1024.0;

    let id = util::new_id();
    let now = util::now_ms();
    let completed_at = if input.status == "done" {
        Some(now)
    } else {
        None
    };
    let mut args = Args::new();
    args.s(&id)
        .s(&project.team_id)
        .s(&project_id)
        .opt_s(parent_task_id.clone())
        .s(&input.title)
        .opt_s(input.description.clone())
        .s(input.status)
        .s(input.priority)
        .opt_s(assignee_id.clone())
        .s(&user.id)
        .opt_s(input.due_date.clone())
        .f(position)
        .opt_i(completed_at)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO tasks (id, team_id, project_id, parent_task_id, title, description, status, priority, assignee_id, created_by, due_date, position, completed_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    insert_task_labels(&mut conn, &id, &body.label_ids).await?;
    activity::record(
        &mut conn,
        &project.team_id,
        Some(&project_id),
        Some(&id),
        Some(&user.id),
        events::TASK_CREATED,
        None,
        None,
        Some(&input.title),
    )
    .await?;
    conn.commit().await?;

    let core = TaskCore {
        id,
        team_id: project.team_id,
        project_id,
        parent_task_id,
        title: input.title,
        description: input.description,
        status: input.status.to_string(),
        priority: input.priority.to_string(),
        assignee_id,
        created_by: user.id,
        due_date: input.due_date,
        position,
        completed_at,
        created_at: now,
        updated_at: now,
    };
    task_from_core(&state, core).await
}

// ---------------------------------------------------------------------------
// Read (detail + board + lists)
// ---------------------------------------------------------------------------

async fn get_task(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<Json<Task>, AppError> {
    let mut conn = state.db.conn();
    let (core, _) = require_task_access(&mut conn, &task_id, &user.id).await?;
    let mut task = embed_extras(&mut conn, vec![core])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::internal("task disappeared"))?;

    let mut args = Args::new();
    args.s(&task_id);
    let subtask_rows = conn
        .fetch_all(
            &format!(
                "SELECT {TASK_COLUMNS} FROM tasks WHERE parent_task_id = ? ORDER BY position, created_at, id"
            ),
            args,
        )
        .await?;
    let subtask_cores = subtask_rows
        .iter()
        .map(row_to_task_core)
        .collect::<Result<Vec<_>, AppError>>()?;
    task.subtasks = Some(embed_extras(&mut conn, subtask_cores).await?);
    Ok(Json(task))
}

#[derive(Deserialize)]
pub struct ListTasksQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub label_id: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub due_before: Option<String>,
    #[serde(default)]
    pub due_after: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

#[derive(Serialize)]
pub struct PaginatedTasks {
    pub items: Vec<Task>,
    pub page: u32,
    pub page_size: u32,
    pub total: i64,
}

fn pagination(query_page: Option<u32>, query_page_size: Option<u32>) -> (u32, u32, i64) {
    let page = query_page.unwrap_or(1).max(1);
    let page_size = query_page_size.unwrap_or(25).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);
    (page, page_size, offset)
}

/// CSV filter values, validated against a canonical list.
fn csv_filter(
    raw: Option<&String>,
    allowed: &[&str],
    field: &str,
) -> Result<Vec<String>, AppError> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(|s| {
            let s = s.trim();
            if allowed.contains(&s) {
                Ok(s.to_string())
            } else {
                Err(AppError::validation(field, format!("unknown value `{s}`")))
            }
        })
        .collect()
}

const SORT_CREATED_AT: &str = "created_at";
const SORT_UPDATED_AT: &str = "updated_at";
const SORT_DUE_DATE: &str = "due_date";
const SORT_PRIORITY: &str = "priority";
const SORT_TITLE: &str = "title";

fn sort_expression(sort: Option<&str>) -> Result<&'static str, AppError> {
    match sort {
        None | Some(SORT_CREATED_AT) => Ok("t.created_at"),
        Some(SORT_UPDATED_AT) => Ok("t.updated_at"),
        Some(SORT_DUE_DATE) => Ok("t.due_date"),
        Some(SORT_PRIORITY) => Ok(
            "CASE t.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END",
        ),
        Some(SORT_TITLE) => Ok("LOWER(t.title)"),
        Some(other) => Err(AppError::validation(
            "sort",
            format!(
                "unknown sort `{other}`; expected created_at, updated_at, due_date, priority or title"
            ),
        )),
    }
}

fn sort_direction(order: Option<&str>) -> Result<&'static str, AppError> {
    match order {
        None | Some("desc") => Ok("DESC"),
        Some("asc") => Ok("ASC"),
        Some(other) => Err(AppError::validation(
            "order",
            format!("unknown order `{other}`; expected asc or desc"),
        )),
    }
}

/// A bound filter value retained so the identical predicate set can be
/// re-bound for the COUNT and page queries.
enum FilterValue {
    S(String),
}

/// Build the shared WHERE clause and its bind values for task listing.
fn build_list_filters(
    project_id: &str,
    query: &ListTasksQuery,
) -> Result<(String, Vec<FilterValue>), AppError> {
    let statuses = csv_filter(query.status.as_ref(), &STATUSES, "status")?;
    let priorities = csv_filter(query.priority.as_ref(), &PRIORITIES, "priority")?;
    if let Some(d) = query.due_before.as_deref().filter(|s| !s.is_empty()) {
        util::parse_iso_date(d).map_err(|e| AppError::validation("due_before", e))?;
    }
    if let Some(d) = query.due_after.as_deref().filter(|s| !s.is_empty()) {
        util::parse_iso_date(d).map_err(|e| AppError::validation("due_after", e))?;
    }
    sort_expression(query.sort.as_deref())?;
    sort_direction(query.order.as_deref())?;

    let mut where_clause = String::from("t.project_id = ? AND t.parent_task_id IS NULL");
    let mut values = vec![FilterValue::S(project_id.to_string())];

    if !statuses.is_empty() {
        let placeholders = vec!["?"; statuses.len()].join(", ");
        where_clause.push_str(&format!(" AND t.status IN ({placeholders})"));
        for s in &statuses {
            values.push(FilterValue::S(s.clone()));
        }
    }
    if !priorities.is_empty() {
        let placeholders = vec!["?"; priorities.len()].join(", ");
        where_clause.push_str(&format!(" AND t.priority IN ({placeholders})"));
        for p in &priorities {
            values.push(FilterValue::S(p.clone()));
        }
    }
    if let Some(assignee) = query.assignee_id.as_deref().filter(|s| !s.is_empty()) {
        if assignee == "none" {
            where_clause.push_str(" AND t.assignee_id IS NULL");
        } else {
            where_clause.push_str(" AND t.assignee_id = ?");
            values.push(FilterValue::S(assignee.to_string()));
        }
    }
    if let Some(label_id) = query.label_id.as_deref().filter(|s| !s.is_empty()) {
        where_clause.push_str(
            " AND EXISTS (SELECT 1 FROM task_labels tl WHERE tl.task_id = t.id AND tl.label_id = ?)",
        );
        values.push(FilterValue::S(label_id.to_string()));
    }
    if let Some(q) = query.q.as_deref().filter(|s| !s.is_empty()) {
        let pattern = format!("%{}%", util::escape_like(&q.trim().to_lowercase()));
        where_clause.push_str(
            " AND (LOWER(t.title) LIKE ? ESCAPE '|' OR LOWER(COALESCE(t.description, '')) LIKE ? ESCAPE '|')",
        );
        values.push(FilterValue::S(pattern.clone()));
        values.push(FilterValue::S(pattern));
    }
    if let Some(d) = query.due_before.as_deref().filter(|s| !s.is_empty()) {
        where_clause.push_str(" AND t.due_date IS NOT NULL AND t.due_date <= ?");
        values.push(FilterValue::S(d.to_string()));
    }
    if let Some(d) = query.due_after.as_deref().filter(|s| !s.is_empty()) {
        where_clause.push_str(" AND t.due_date IS NOT NULL AND t.due_date >= ?");
        values.push(FilterValue::S(d.to_string()));
    }
    Ok((where_clause, values))
}

fn bind_values(args: &mut Args<'_>, values: &[FilterValue]) {
    for value in values {
        match value {
            FilterValue::S(s) => {
                args.s(s.clone());
            }
        }
    }
}

async fn list_tasks(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
    Query(query): Query<ListTasksQuery>,
) -> Result<Json<PaginatedTasks>, AppError> {
    let mut conn = state.db.conn();
    require_project_access(&mut conn, &project_id, &user.id).await?;

    let (where_clause, values) = build_list_filters(&project_id, &query)?;
    let sort_sql = sort_expression(query.sort.as_deref())?;
    let dir_sql = sort_direction(query.order.as_deref())?;
    let (page, page_size, offset) = pagination(query.page, query.page_size);

    // Total count for pagination metadata.
    let count_sql = format!("SELECT COUNT(*) AS n FROM tasks t WHERE {where_clause}");
    let mut count_args = Args::new();
    bind_values(&mut count_args, &values);
    let total = conn
        .fetch_optional(&count_sql, count_args)
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Page of ids, then full rows preserving order.
    let page_sql = format!(
        "SELECT t.id FROM tasks t WHERE {where_clause} ORDER BY {sort_sql} {dir_sql}, t.id ASC LIMIT ? OFFSET ?"
    );
    let mut page_args = Args::new();
    bind_values(&mut page_args, &values);
    page_args.i(i64::from(page_size)).i(offset);
    let id_rows = conn.fetch_all(&page_sql, page_args).await?;
    let ids: Vec<String> = id_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("id").ok())
        .collect();

    let items = if ids.is_empty() {
        Vec::new()
    } else {
        let placeholders = vec!["?"; ids.len()].join(", ");
        let mut args = Args::new();
        for id in &ids {
            args.s(id.clone());
        }
        let rows = conn
            .fetch_all(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id IN ({placeholders})"),
                args,
            )
            .await?;
        let mut by_id: HashMap<String, TaskCore> = HashMap::with_capacity(rows.len());
        for row in rows {
            let core = row_to_task_core(&row)?;
            by_id.insert(core.id.clone(), core);
        }
        let cores: Vec<TaskCore> = ids.iter().filter_map(|id| by_id.remove(id)).collect();
        embed_extras(&mut conn, cores).await?
    };

    Ok(Json(PaginatedTasks {
        items,
        page,
        page_size,
        total,
    }))
}

#[derive(Deserialize)]
pub struct BoardQuery {
    #[serde(default)]
    pub page_size: Option<u32>,
}

async fn board(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
    Query(query): Query<BoardQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let (project, _) = require_project_access(&mut conn, &project_id, &user.id).await?;
    let page_size = query.page_size.unwrap_or(50).clamp(1, 100);

    let mut args = Args::new();
    args.s(&project_id);
    let count_rows = conn
        .fetch_all(
            "SELECT status, COUNT(*) AS n FROM tasks WHERE project_id = ? AND parent_task_id IS NULL GROUP BY status",
            args,
        )
        .await?;
    let mut totals: HashMap<String, i64> = HashMap::new();
    for row in count_rows {
        totals.insert(row.try_get("status")?, row.try_get("n")?);
    }

    let mut columns = Vec::with_capacity(STATUSES.len());
    for status in STATUSES {
        let mut args = Args::new();
        args.s(&project_id).s(status).i(i64::from(page_size));
        let rows = conn
            .fetch_all(
                &format!(
                    "SELECT {TASK_COLUMNS} FROM tasks t \
                     WHERE t.project_id = ? AND t.status = ? AND t.parent_task_id IS NULL \
                     ORDER BY t.position, t.created_at, t.id LIMIT ?"
                ),
                args,
            )
            .await?;
        let cores = rows
            .iter()
            .map(row_to_task_core)
            .collect::<Result<Vec<_>, AppError>>()?;
        let items = embed_extras(&mut conn, cores).await?;
        columns.push(json!({
            "status": status,
            "total": totals.get(status).copied().unwrap_or(0),
            "items": items,
        }));
    }
    Ok(Json(
        json!({ "project_id": project.id, "columns": columns }),
    ))
}

// ---------------------------------------------------------------------------
// Update & delete
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct UpdateTaskRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub label_ids: Option<Vec<String>>,
}

async fn update_task(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
    Json(body): Json<UpdateTaskRequest>,
) -> Result<Json<Task>, AppError> {
    let mut conn = state.db.begin().await?;
    let (task, membership) = require_task_access(&mut conn, &task_id, &user.id).await?;
    let can_update_any = membership.role.can(Capability::UpdateAnyTask);
    let is_own = task.created_by == user.id || task.assignee_id.as_deref() == Some(&user.id);
    if !can_update_any && !is_own {
        return Err(AppError::Forbidden(
            "you can only update tasks you created or are assigned to",
        ));
    }

    let project = crate::projects::find_project(&mut conn, &task.project_id).await?;
    if project.status == "archived" {
        return Err(AppError::Conflict("archived projects are read-only".into()));
    }

    let input = validate_task_fields(
        body.title.as_deref().unwrap_or(&task.title),
        body.description.as_deref().or(task.description.as_deref()),
        body.status.as_deref().unwrap_or(&task.status),
        body.priority.as_deref().unwrap_or(&task.priority),
        body.due_date.as_deref().or(task.due_date.as_deref()),
    )?;

    // Assignee rules: privileged roles assign freely; plain members may only
    // claim a task for themselves. Clearing an assignment is allowed when the
    // task is currently assigned to the caller or the caller created it.
    let assignee_id = match body.assignee_id.as_deref() {
        Some("") => {
            if !can_update_any
                && task.assignee_id.as_deref() != Some(&user.id)
                && task.created_by != user.id
            {
                return Err(AppError::Forbidden(
                    "you can only clear assignments on your own tasks",
                ));
            }
            None
        }
        Some(id) => {
            if !can_update_any && id != user.id {
                return Err(AppError::Forbidden("you can only assign tasks to yourself"));
            }
            validate_assignee(&mut conn, &task.team_id, id).await?;
            Some(id.to_string())
        }
        None => task.assignee_id.clone(),
    };

    let label_ids = match &body.label_ids {
        Some(ids) => {
            if !can_update_any {
                return Err(AppError::Forbidden(
                    "your role does not permit editing task labels",
                ));
            }
            validate_label_ids(&mut conn, &task.team_id, ids).await?;
            Some(ids.clone())
        }
        None => None,
    };

    // Diff against current state for activity records.
    let now = util::now_ms();
    let mut changes: Vec<(&'static str, Option<String>, Option<String>)> = Vec::new();
    if input.title != task.title {
        changes.push(("title", Some(task.title.clone()), Some(input.title.clone())));
    }
    if input.description != task.description {
        changes.push((
            "description",
            task.description.clone(),
            input.description.clone(),
        ));
    }
    if input.status != task.status {
        changes.push((
            "status",
            Some(task.status.clone()),
            Some(input.status.to_string()),
        ));
    }
    if input.priority != task.priority {
        changes.push((
            "priority",
            Some(task.priority.clone()),
            Some(input.priority.to_string()),
        ));
    }
    if assignee_id != task.assignee_id {
        changes.push(("assignee_id", task.assignee_id.clone(), assignee_id.clone()));
    }
    if input.due_date != task.due_date {
        changes.push(("due_date", task.due_date.clone(), input.due_date.clone()));
    }

    let completed_at = match (task.status.as_str(), input.status) {
        ("done", "done") => task.completed_at,
        (_, "done") => Some(now),
        (_, _) => None,
    };

    let mut args = Args::new();
    args.s(&input.title)
        .opt_s(input.description.clone())
        .s(input.status)
        .s(input.priority)
        .opt_s(assignee_id.clone())
        .opt_s(input.due_date.clone())
        .opt_i(completed_at)
        .i(now)
        .s(&task_id);
    conn.execute(
        "UPDATE tasks SET title = ?, description = ?, status = ?, priority = ?, assignee_id = ?, \
         due_date = ?, completed_at = ?, updated_at = ? WHERE id = ?",
        args,
    )
    .await?;

    if let Some(ids) = &label_ids {
        let mut args = Args::new();
        args.s(&task_id);
        conn.execute("DELETE FROM task_labels WHERE task_id = ?", args)
            .await?;
        insert_task_labels(&mut conn, &task_id, ids).await?;
    }

    for (field, old, new) in &changes {
        let event = match *field {
            "status" => events::TASK_STATUS_CHANGED,
            "priority" => events::TASK_PRIORITY_CHANGED,
            "assignee_id" => events::TASK_ASSIGNEE_CHANGED,
            "due_date" => events::TASK_DUE_DATE_CHANGED,
            _ => events::TASK_UPDATED,
        };
        activity::record(
            &mut conn,
            &task.team_id,
            Some(&task.project_id),
            Some(&task.id),
            Some(&user.id),
            event,
            Some(field),
            old.as_deref(),
            new.as_deref(),
        )
        .await?;
    }
    conn.commit().await?;

    let core = TaskCore {
        id: task.id,
        team_id: task.team_id,
        project_id: task.project_id,
        parent_task_id: task.parent_task_id,
        title: input.title,
        description: input.description,
        status: input.status.to_string(),
        priority: input.priority.to_string(),
        assignee_id,
        created_by: task.created_by,
        due_date: input.due_date,
        position: task.position,
        completed_at,
        created_at: task.created_at,
        updated_at: now,
    };
    task_from_core(&state, core).await.map(Json)
}

async fn delete_task(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let (task, membership) = require_task_access(&mut conn, &task_id, &user.id).await?;
    let can_delete_any = membership.role.can(Capability::DeleteAnyTask);
    let is_creator = task.created_by == user.id;
    if !can_delete_any && !is_creator {
        return Err(AppError::Forbidden("you can only delete tasks you created"));
    }

    // The task row (and its subtasks, via FK cascade) disappear, so the
    // deletion event is anchored to the project rather than the task.
    activity::record(
        &mut conn,
        &task.team_id,
        Some(&task.project_id),
        None,
        Some(&user.id),
        events::TASK_DELETED,
        Some("title"),
        None,
        Some(&task.title),
    )
    .await?;

    let mut args = Args::new();
    args.s(&task_id);
    conn.execute("DELETE FROM tasks WHERE id = ?", args).await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// My Tasks
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct MyTasksQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

#[derive(Serialize)]
pub struct MyTaskItem {
    #[serde(flatten)]
    pub task: Task,
    pub team_name: String,
    pub project_name: String,
}

#[derive(Serialize)]
pub struct PaginatedMyTasks {
    pub items: Vec<MyTaskItem>,
    pub page: u32,
    pub page_size: u32,
    pub total: i64,
}

/// Tasks assigned to the authenticated user across all of their teams,
/// excluding archived projects. Due-soon tasks sort first by default.
async fn my_tasks(
    State(state): State<AppState>,
    user: AuthedUser,
    Query(query): Query<MyTasksQuery>,
) -> Result<Json<PaginatedMyTasks>, AppError> {
    let mut conn = state.db.conn();
    let statuses = csv_filter(query.status.as_ref(), &STATUSES, "status")?;
    let priorities = csv_filter(query.priority.as_ref(), &PRIORITIES, "priority")?;
    let sort_sql = sort_expression(query.sort.as_deref())?;
    let dir_sql = sort_direction(query.order.as_deref())?;
    let (page, page_size, offset) = pagination(query.page, query.page_size);

    let mut where_clause = String::from(
        "t.assignee_id = ? AND t.parent_task_id IS NULL \
         AND EXISTS (SELECT 1 FROM team_members tm WHERE tm.team_id = t.team_id AND tm.user_id = ?) \
         AND EXISTS (SELECT 1 FROM projects p WHERE p.id = t.project_id AND p.status = 'active')",
    );
    let mut values: Vec<FilterValue> = vec![
        FilterValue::S(user.id.clone()),
        FilterValue::S(user.id.clone()),
    ];
    if !statuses.is_empty() {
        let placeholders = vec!["?"; statuses.len()].join(", ");
        where_clause.push_str(&format!(" AND t.status IN ({placeholders})"));
        for s in &statuses {
            values.push(FilterValue::S(s.clone()));
        }
    }
    if !priorities.is_empty() {
        let placeholders = vec!["?"; priorities.len()].join(", ");
        where_clause.push_str(&format!(" AND t.priority IN ({placeholders})"));
        for p in &priorities {
            values.push(FilterValue::S(p.clone()));
        }
    }

    let count_sql = format!("SELECT COUNT(*) AS n FROM tasks t WHERE {where_clause}");
    let mut count_args = Args::new();
    bind_values(&mut count_args, &values);
    let total = conn
        .fetch_optional(&count_sql, count_args)
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Unset due dates sort after all set dates on every engine.
    let order_sql = if sort_sql == "t.due_date" {
        format!("(t.due_date IS NULL) ASC, t.due_date {dir_sql}, t.id ASC")
    } else {
        format!("{sort_sql} {dir_sql}, t.id ASC")
    };
    let page_sql = format!(
        "SELECT t.id FROM tasks t WHERE {where_clause} ORDER BY {order_sql} LIMIT ? OFFSET ?"
    );
    let mut page_args = Args::new();
    bind_values(&mut page_args, &values);
    page_args.i(i64::from(page_size)).i(offset);
    let id_rows = conn.fetch_all(&page_sql, page_args).await?;
    let ids: Vec<String> = id_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("id").ok())
        .collect();

    let mut items: Vec<MyTaskItem> = Vec::with_capacity(ids.len());
    if !ids.is_empty() {
        let placeholders = vec!["?"; ids.len()].join(", ");
        let mut args = Args::new();
        for id in &ids {
            args.s(id.clone());
        }
        let rows = conn
            .fetch_all(
                &format!(
                    "SELECT t.*, tm.name AS team_name, p.name AS project_name \
                     FROM tasks t \
                     JOIN teams tm ON tm.id = t.team_id \
                     JOIN projects p ON p.id = t.project_id \
                     WHERE t.id IN ({placeholders})"
                ),
                args,
            )
            .await?;
        let mut by_id: HashMap<String, (TaskCore, String, String)> = HashMap::new();
        for row in rows {
            let core = TaskCore {
                id: row.try_get("id")?,
                team_id: row.try_get("team_id")?,
                project_id: row.try_get("project_id")?,
                parent_task_id: row.try_get("parent_task_id")?,
                title: row.try_get("title")?,
                description: row.try_get("description")?,
                status: row.try_get("status")?,
                priority: row.try_get("priority")?,
                assignee_id: row.try_get("assignee_id")?,
                created_by: row.try_get("created_by")?,
                due_date: row.try_get("due_date")?,
                position: row.try_get("position")?,
                completed_at: row.try_get("completed_at")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            };
            let team_name: String = row.try_get("team_name")?;
            let project_name: String = row.try_get("project_name")?;
            by_id.insert(core.id.clone(), (core, team_name, project_name));
        }
        let cores: Vec<(TaskCore, String, String)> =
            ids.iter().filter_map(|id| by_id.remove(id)).collect();
        let task_cores: Vec<TaskCore> = cores.iter().map(|(c, _, _)| c.clone()).collect();
        let tasks = embed_extras(&mut conn, task_cores).await?;
        for (task, (_, team_name, project_name)) in tasks.into_iter().zip(cores) {
            items.push(MyTaskItem {
                task,
                team_name,
                project_name,
            });
        }
    }

    Ok(Json(PaginatedMyTasks {
        items,
        page,
        page_size,
        total,
    }))
}
