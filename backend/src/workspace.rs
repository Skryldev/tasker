//! Advanced team execution (Phase 3): task dependencies, workload
//! aggregation, project task templates, and shared-with-me project listing.

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
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
        // Dependencies
        .route(
            "/tasks/{task_id}/dependencies",
            get(list_dependencies).post(create_dependency),
        )
        .route(
            "/tasks/{task_id}/dependencies/{predecessor_task_id}",
            axum::routing::delete(remove_dependency),
        )
        // Workload
        .route("/teams/{team_id}/workload", get(workload))
        // Templates
        .route(
            "/projects/{project_id}/templates",
            get(list_templates).post(create_template),
        )
        .route(
            "/templates/{template_id}",
            get(get_template)
                .patch(update_template)
                .delete(delete_template),
        )
        // Shared-with-me
        .route("/me/projects", get(my_projects))
}

fn deny_guest() -> AppError {
    AppError::Forbidden("guests have read-only access")
}

fn today_iso() -> String {
    util::parse_today_iso()
}

// ===========================================================================
// Task dependencies (finish-to-start)
// ===========================================================================

#[derive(Debug, Clone, Serialize)]
pub struct DependencyEdge {
    pub predecessor_task_id: String,
    pub successor_task_id: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyTaskRef {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub due_date: Option<String>,
}

async fn load_edge(
    conn: &mut Conn<'_>,
    predecessor: &str,
    successor: &str,
) -> Result<Option<DependencyEdge>, AppError> {
    let mut args = Args::new();
    args.s(predecessor).s(successor);
    let row = conn
        .fetch_optional(
            "SELECT predecessor_task_id, successor_task_id, created_at \
             FROM task_dependencies WHERE predecessor_task_id = ? AND successor_task_id = ?",
            args,
        )
        .await?;
    row.map(|r| {
        Ok(DependencyEdge {
            predecessor_task_id: r.try_get("predecessor_task_id")?,
            successor_task_id: r.try_get("successor_task_id")?,
            created_at: r.try_get("created_at")?,
        })
    })
    .transpose()
}

/// BFS over predecessor edges starting at `start`: if `target` is reachable,
/// the graph already contains target → … → start, so adding start → target
/// closes a cycle. Terminates via the visited set; depth is bounded by the
/// number of tasks in the project.
async fn reachable(conn: &mut Conn<'_>, start: &str, target: &str) -> Result<bool, AppError> {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(start.to_string());
    let mut frontier = vec![start.to_string()];
    while let Some(node) = frontier.pop() {
        let mut args = Args::new();
        args.s(&node);
        let rows = conn
            .fetch_all(
                "SELECT predecessor_task_id FROM task_dependencies WHERE successor_task_id = ?",
                args,
            )
            .await?;
        for row in rows {
            let pred: String = row.try_get("predecessor_task_id")?;
            if pred == target {
                return Ok(true);
            }
            if visited.insert(pred.clone()) {
                frontier.push(pred);
            }
        }
    }
    Ok(false)
}

#[derive(Deserialize)]
pub struct CreateDependencyRequest {
    /// The task that must finish first.
    pub predecessor_task_id: String,
}

async fn create_dependency(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
    Json(body): Json<CreateDependencyRequest>,
) -> Result<(StatusCode, Json<DependencyEdge>), AppError> {
    let predecessor = body.predecessor_task_id.trim().to_string();
    if predecessor == task_id {
        return Err(AppError::validation(
            "predecessor_task_id",
            "a task cannot depend on itself",
        ));
    }

    let mut conn = state.db.begin().await?;
    // Successor access governs the mutation; guests are read-only.
    let (successor, actor) =
        crate::tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    if actor.is_guest() {
        return Err(deny_guest());
    }
    // Dependency edges are coordination metadata between tasks a team member
    // can already see; guests stay read-only (the security boundary). Tasks
    // must share the project (validated below).
    if crate::projects::find_project(&mut conn, &successor.project_id)
        .await?
        .status
        == "archived"
    {
        return Err(AppError::Conflict("archived projects are read-only".into()));
    }

    // The predecessor must exist in the same project (and therefore team).
    let mut args = Args::new();
    args.s(&predecessor).s(&successor.project_id);
    let pred_row = conn
        .fetch_optional(
            "SELECT team_id FROM tasks WHERE id = ? AND project_id = ?",
            args,
        )
        .await?
        .ok_or_else(|| {
            AppError::validation(
                "predecessor_task_id",
                "the predecessor task does not exist in this project",
            )
        })?;
    if pred_row.try_get::<String, _>("team_id")? != successor.team_id {
        return Err(AppError::internal("project/team mismatch")); // unreachable: project filter
    }

    if load_edge(&mut conn, &predecessor, &task_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("this dependency already exists".into()));
    }
    // A stored reverse edge is a two-task cycle.
    if load_edge(&mut conn, &task_id, &predecessor)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(
            "this dependency would create a cycle with the existing reverse dependency".into(),
        ));
    }

    // Serialize per project so two concurrent requests cannot both pass the
    // cycle check and close a loop together (see db::ProjectDependencyLock).
    {
        let mut lock = crate::db::ProjectDependencyLock::for_driver(conn.driver());
        lock.acquire(&mut conn, &successor.project_id).await;
        // Under the lock: does the successor already transitively depend on
        // the predecessor? Then predecessor → successor closes a loop.
        if reachable(&mut conn, &task_id, &predecessor).await? {
            return Err(AppError::Conflict(
                "this dependency would create a cycle: the predecessor already depends on this task"
                    .into(),
            ));
        }
        let mut args = Args::new();
        args.s(&predecessor)
            .s(&task_id)
            .i(util::now_ms())
            .s(&user.id);
        conn.execute(
            "INSERT INTO task_dependencies (predecessor_task_id, successor_task_id, created_at, created_by) \
             VALUES (?, ?, ?, ?)",
            args,
        )
        .await?;
        lock.release(&mut conn, &successor.project_id).await;
    }
    let edge = load_edge(&mut conn, &predecessor, &task_id)
        .await?
        .ok_or_else(|| AppError::internal("dependency insert did not persist"))?;
    conn.commit().await?;
    Ok((StatusCode::CREATED, Json(edge)))
}

async fn remove_dependency(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((task_id, predecessor_task_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let (_successor, actor) =
        crate::tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    if actor.is_guest() {
        return Err(deny_guest());
    }
    let mut args = Args::new();
    args.s(&predecessor_task_id).s(&task_id);
    let affected = conn
        .execute(
            "DELETE FROM task_dependencies WHERE predecessor_task_id = ? AND successor_task_id = ?",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::NotFound("dependency not found"));
    }
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_dependencies(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let (_, actor) = crate::tasks::require_task_access(&mut conn, &task_id, &user.id).await?;

    let mut args = Args::new();
    args.s(&task_id);
    let blocker_rows = conn
        .fetch_all(
            "SELECT t.id, t.title, t.status, t.due_date \
             FROM task_dependencies d JOIN tasks t ON t.id = d.predecessor_task_id \
             WHERE d.successor_task_id = ? ORDER BY d.created_at",
            args,
        )
        .await?;
    let blockers = blocker_rows
        .iter()
        .map(|r| {
            Ok(DependencyTaskRef {
                task_id: r.try_get("id")?,
                title: r.try_get("title")?,
                status: r.try_get("status")?,
                due_date: r.try_get("due_date")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    let mut args = Args::new();
    args.s(&task_id);
    let successor_rows = conn
        .fetch_all(
            "SELECT t.id, t.title, t.status, t.due_date \
             FROM task_dependencies d JOIN tasks t ON t.id = d.successor_task_id \
             WHERE d.predecessor_task_id = ? ORDER BY d.created_at",
            args,
        )
        .await?;
    let blocked = successor_rows
        .iter()
        .map(|r| {
            Ok(DependencyTaskRef {
                task_id: r.try_get("id")?,
                title: r.try_get("title")?,
                status: r.try_get("status")?,
                due_date: r.try_get("due_date")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(Json(json!({
        "blockers": blockers,
        "blocked": blocked,
        "can_manage": !actor.is_guest(),
    })))
}

// ===========================================================================
// Workload (team-scoped SQL aggregation)
// ===========================================================================

#[derive(Deserialize)]
pub struct WorkloadQuery {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub due_before: Option<String>,
    #[serde(default)]
    pub due_after: Option<String>,
}

/// Filters shared by both aggregation queries so status and priority counts
/// always describe the same task set.
struct WorkloadFilters {
    project_id: Option<String>,
    due_before: Option<String>,
    due_after: Option<String>,
}

impl WorkloadFilters {
    fn parse(query: &WorkloadQuery) -> Result<Self, AppError> {
        for (field, value) in [
            ("due_before", query.due_before.as_deref()),
            ("due_after", query.due_after.as_deref()),
        ] {
            if let Some(v) = value.filter(|s| !s.is_empty()) {
                util::parse_iso_date(v).map_err(|e| AppError::validation(field, e))?;
            }
        }
        Ok(Self {
            project_id: query
                .project_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            due_before: query
                .due_before
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            due_after: query
                .due_after
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
    }

    fn clause(&self) -> String {
        let mut sql = String::new();
        if self.project_id.is_some() {
            sql.push_str(" AND t.project_id = ?");
        }
        if self.due_before.is_some() {
            sql.push_str(" AND t.due_date IS NOT NULL AND t.due_date <= ?");
        }
        if self.due_after.is_some() {
            sql.push_str(" AND t.due_date IS NOT NULL AND t.due_date >= ?");
        }
        sql
    }

    fn bind(&self, args: &mut Args<'_>) {
        if let Some(pid) = &self.project_id {
            args.s(pid.clone());
        }
        if let Some(d) = &self.due_before {
            args.s(d.clone());
        }
        if let Some(d) = &self.due_after {
            args.s(d.clone());
        }
    }
}

struct MemberRow {
    display_name: String,
    email: String,
    by_status: std::collections::BTreeMap<String, i64>,
    by_priority: std::collections::BTreeMap<String, i64>,
    overdue: i64,
    total: i64,
}
fn ensure<'a>(
    members: &'a mut std::collections::BTreeMap<String, MemberRow>,
    id: &str,
    name: &str,
    email: &str,
) -> &'a mut MemberRow {
    members.entry(id.to_string()).or_insert_with(|| MemberRow {
        display_name: name.to_string(),
        email: email.to_string(),
        by_status: std::collections::BTreeMap::new(),
        by_priority: std::collections::BTreeMap::new(),
        overdue: 0,
        total: 0,
    })
}

async fn workload(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Query(query): Query<WorkloadQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Team-wide aggregates require team membership; project guests are not
    // members and cannot see team-level numbers (their access is 404 here).
    {
        let mut conn = state.db.conn();
        crate::teams::require_membership(&mut conn, &team_id, &user.id).await?;
        if let Some(pid) = query.project_id.as_deref().filter(|s| !s.is_empty()) {
            crate::projects::require_project_access(&mut conn, pid, &user.id).await?;
        }
    }
    let filters = WorkloadFilters::parse(&query)?;
    let today = today_iso();

    let mut conn = state.db.conn();

    // One grouped query per dimension over the same filtered task set; both
    // hit the (team_id, project_id) indexes.
    let status_sql = format!(
        "SELECT t.assignee_id, u.display_name, u.email, t.status, COUNT(*) AS n \
         FROM tasks t JOIN users u ON u.id = t.assignee_id \
         WHERE t.team_id = ? AND t.parent_task_id IS NULL AND t.assignee_id IS NOT NULL{} \
         GROUP BY t.assignee_id, u.display_name, u.email, t.status",
        filters.clause()
    );
    let mut status_args = Args::new();
    status_args.s(team_id.clone());
    filters.bind(&mut status_args);
    let status_rows = conn.fetch_all(&status_sql, status_args).await?;

    let priority_sql = format!(
        "SELECT t.assignee_id, u.display_name, u.email, t.priority, COUNT(*) AS n \
         FROM tasks t JOIN users u ON u.id = t.assignee_id \
         WHERE t.team_id = ? AND t.parent_task_id IS NULL AND t.assignee_id IS NOT NULL{} \
         GROUP BY t.assignee_id, u.display_name, u.email, t.priority",
        filters.clause()
    );
    let mut priority_args = Args::new();
    priority_args.s(team_id.clone());
    filters.bind(&mut priority_args);
    let priority_rows = conn.fetch_all(&priority_sql, priority_args).await?;

    let mut members: std::collections::BTreeMap<String, MemberRow> =
        std::collections::BTreeMap::new();
    for row in &status_rows {
        let id: String = row.try_get("assignee_id")?;
        let name: String = row.try_get("display_name")?;
        let email: String = row.try_get("email")?;
        let status: String = row.try_get("status")?;
        let n: i64 = row.try_get("n")?;
        let entry = ensure(&mut members, &id, &name, &email);
        *entry.by_status.entry(status).or_insert(0) += n;
        entry.total += n;
    }
    for row in &priority_rows {
        let id: String = row.try_get("assignee_id")?;
        let name: String = row.try_get("display_name")?;
        let email: String = row.try_get("email")?;
        let priority: String = row.try_get("priority")?;
        let n: i64 = row.try_get("n")?;
        let entry = ensure(&mut members, &id, &name, &email);
        *entry.by_priority.entry(priority).or_insert(0) += n;
    }
    // Overdue counts: dedicated grouped count query — no boolean decoding
    // (PG returns COUNT as BIGINT on every engine; portable).
    let overdue_sql = format!(
        "SELECT t.assignee_id, COUNT(*) AS n \
         FROM tasks t \
         WHERE t.team_id = ? AND t.parent_task_id IS NULL AND t.assignee_id IS NOT NULL \
           AND t.due_date IS NOT NULL AND t.due_date < '{today}' AND t.status != 'closed'{} \
         GROUP BY t.assignee_id",
        filters.clause()
    );
    let mut overdue_args = Args::new();
    overdue_args.s(team_id.clone());
    filters.bind(&mut overdue_args);
    for row in conn.fetch_all(&overdue_sql, overdue_args).await? {
        let id: String = row.try_get("assignee_id")?;
        let n: i64 = row.try_get("n")?;
        let entry = ensure(&mut members, &id, "", "");
        entry.overdue += n;
    }

    let items: Vec<serde_json::Value> = members
        .into_iter()
        .map(|(id, m)| {
            json!({
                "user": { "id": id, "display_name": m.display_name, "email": m.email },
                "by_status": m.by_status,
                "by_priority": m.by_priority,
                "overdue": m.overdue,
                "total": m.total,
            })
        })
        .collect();

    // Unassigned work: same filters, grouped by status + priority; overdue via
    // a dedicated count (portable — no boolean columns).
    let unassigned_sql = format!(
        "SELECT t.status, t.priority, COUNT(*) AS n \
         FROM tasks t \
         WHERE t.team_id = ? AND t.parent_task_id IS NULL AND t.assignee_id IS NULL{} \
         GROUP BY t.status, t.priority",
        filters.clause()
    );
    let mut unassigned_args = Args::new();
    unassigned_args.s(team_id.clone());
    filters.bind(&mut unassigned_args);
    let unassigned_rows = conn.fetch_all(&unassigned_sql, unassigned_args).await?;
    let mut ua_by_status = std::collections::BTreeMap::<String, i64>::new();
    let mut ua_by_priority = std::collections::BTreeMap::<String, i64>::new();
    let mut ua_total = 0i64;
    for row in &unassigned_rows {
        let status: String = row.try_get("status")?;
        let priority: String = row.try_get("priority")?;
        let n: i64 = row.try_get("n")?;
        *ua_by_status.entry(status).or_insert(0) += n;
        *ua_by_priority.entry(priority).or_insert(0) += n;
        ua_total += n;
    }
    let unassigned_overdue_sql = format!(
        "SELECT COUNT(*) AS n FROM tasks t \
         WHERE t.team_id = ? AND t.parent_task_id IS NULL AND t.assignee_id IS NULL \
           AND t.due_date IS NOT NULL AND t.due_date < '{today}' AND t.status != 'closed'{}",
        filters.clause()
    );
    let mut ua_overdue_args = Args::new();
    ua_overdue_args.s(team_id.clone());
    filters.bind(&mut ua_overdue_args);
    let ua_overdue = conn
        .fetch_optional(&unassigned_overdue_sql, ua_overdue_args)
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    Ok(Json(json!({
        "members": items,
        "unassigned": {
            "by_status": ua_by_status,
            "by_priority": ua_by_priority,
            "overdue": ua_overdue,
            "total": ua_total,
        },
        "generated_at": util::now_ms(),
    })))
}

// ===========================================================================
// Project task templates
// ===========================================================================

#[derive(Debug, Clone, Serialize)]
pub struct TaskTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub default_title: String,
    pub default_description: Option<String>,
    pub default_priority: String,
    pub label_ids: Vec<String>,
    pub created_by: Option<UserBrief>,
    pub created_at: i64,
    pub updated_at: i64,
}

const TEMPLATE_COLUMNS: &str = "t.id, t.project_id, t.name, t.default_title, t.default_description, \
     t.default_priority, t.created_by, t.created_at, t.updated_at, \
     u.email AS creator_email, u.display_name AS creator_name";

fn row_to_template(row: &sqlx::any::AnyRow) -> Result<TaskTemplate, AppError> {
    let creator_email: Option<String> = row.try_get("creator_email")?;
    Ok(TaskTemplate {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        default_title: row.try_get("default_title")?,
        default_description: row.try_get("default_description")?,
        default_priority: row.try_get("default_priority")?,
        label_ids: Vec::new(),
        created_by: creator_email.map(|email| {
            let creator_id: Option<String> = row.try_get("created_by").unwrap_or(None);
            let creator_name: Option<String> = row.try_get("creator_name").unwrap_or(None);
            UserBrief {
                id: creator_id.unwrap_or_default(),
                display_name: creator_name.unwrap_or_default(),
                email,
            }
        }),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_template_labels(
    conn: &mut Conn<'_>,
    template_id: &str,
) -> Result<Vec<String>, AppError> {
    let mut args = Args::new();
    args.s(template_id);
    let rows = conn
        .fetch_all(
            "SELECT label_id FROM task_template_labels WHERE template_id = ? ORDER BY label_id",
            args,
        )
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("label_id").ok())
        .collect())
}

async fn find_template(conn: &mut Conn<'_>, template_id: &str) -> Result<TaskTemplate, AppError> {
    let mut args = Args::new();
    args.s(template_id);
    let row = conn
        .fetch_optional(
            &format!(
                "SELECT {TEMPLATE_COLUMNS} FROM task_templates t \
                 JOIN users u ON u.id = t.created_by WHERE t.id = ?"
            ),
            args,
        )
        .await?
        .ok_or(AppError::NotFound("template not found"))?;
    let mut template = row_to_template(&row)?;
    template.label_ids = load_template_labels(conn, template_id).await?;
    Ok(template)
}

#[derive(Deserialize)]
pub struct UpsertTemplateRequest {
    pub name: String,
    pub default_title: String,
    #[serde(default)]
    pub default_description: Option<String>,
    #[serde(default)]
    pub default_priority: Option<String>,
    #[serde(default)]
    pub label_ids: Vec<String>,
}

fn validate_template(
    body: &UpsertTemplateRequest,
) -> Result<(String, String, Option<String>, String), AppError> {
    let mut errors = Vec::new();
    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 160 {
        errors.push(FieldError::new(
            "name",
            "template name must be 1-160 characters",
        ));
    }
    let title = body.default_title.trim().to_string();
    if title.is_empty() || title.chars().count() > 250 {
        errors.push(FieldError::new(
            "default_title",
            "default title must be 1-250 characters",
        ));
    }
    let description = body
        .default_description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    if let Some(d) = description
        && d.chars().count() > 10_000
    {
        errors.push(FieldError::new(
            "default_description",
            "default description must be at most 10000 characters",
        ));
    }
    let priority: String = match body.default_priority.as_deref().unwrap_or("medium") {
        "low" | "medium" | "high" | "urgent" => body
            .default_priority
            .as_deref()
            .unwrap_or("medium")
            .to_string(),
        other => {
            return Err(AppError::Validation(vec![FieldError::new(
                "default_priority",
                format!("unknown priority `{other}`"),
            )]));
        }
    };
    if errors.is_empty() {
        Ok((
            name,
            title,
            description.map(std::string::ToString::to_string),
            priority,
        ))
    } else {
        Err(AppError::Validation(errors))
    }
}

async fn team_of(conn: &mut Conn<'_>, project_id: &str) -> Result<String, AppError> {
    let mut args = Args::new();
    args.s(project_id);
    let row = conn
        .fetch_optional("SELECT team_id FROM projects WHERE id = ?", args)
        .await?
        .ok_or(AppError::NotFound("project not found"))?;
    Ok(row.try_get("team_id")?)
}

async fn require_template_manager(
    conn: &mut Conn<'_>,
    project_id: &str,
    user_id: &str,
) -> Result<(), AppError> {
    let (_, actor) = require_project_access(conn, project_id, user_id).await?;
    if actor.is_guest() || !actor.can(crate::authorization::Capability::ManageProjects) {
        return Err(AppError::Forbidden(
            "managing templates requires the Manager role or above",
        ));
    }
    Ok(())
}

async fn list_templates(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let (_, actor) = require_project_access(&mut conn, &project_id, &user.id).await?;
    // Templates are internal collaboration defaults: not exposed to guests.
    actor.require_team_member()?;
    let mut args = Args::new();
    args.s(&project_id);
    let rows = conn
        .fetch_all(
            &format!(
                "SELECT {TEMPLATE_COLUMNS} FROM task_templates t \
                 JOIN users u ON u.id = t.created_by \
                 WHERE t.project_id = ? ORDER BY t.name"
            ),
            args,
        )
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let mut template = row_to_template(&row)?;
        template.label_ids = load_template_labels(&mut conn, &template.id).await?;
        items.push(template);
    }
    Ok(Json(json!({ "items": items })))
}

async fn create_template(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(project_id): Path<String>,
    Json(body): Json<UpsertTemplateRequest>,
) -> Result<(StatusCode, Json<TaskTemplate>), AppError> {
    let mut conn = state.db.begin().await?;
    let (_, actor) = require_project_access(&mut conn, &project_id, &user.id).await?;
    if actor.is_guest() {
        return Err(deny_guest());
    }
    if !actor.can(crate::authorization::Capability::ManageProjects) {
        return Err(AppError::Forbidden(
            "managing templates requires the Manager role or above",
        ));
    }
    let (name, title, description, priority) = validate_template(&body)?;
    let team_id = team_of(&mut conn, &project_id).await?;
    crate::tasks::validate_label_ids(&mut conn, &team_id, &body.label_ids).await?;

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&project_id)
        .s(&name)
        .s(&title)
        .opt_s(description.clone())
        .s(priority)
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO task_templates (id, project_id, name, default_title, default_description, default_priority, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    for label_id in &body.label_ids {
        let mut label_args = Args::new();
        label_args.s(&id).s(label_id);
        conn.execute(
            "INSERT INTO task_template_labels (template_id, label_id) VALUES (?, ?)",
            label_args,
        )
        .await?;
    }
    conn.commit().await?;

    let mut conn = state.db.conn();
    let template = find_template(&mut conn, &id).await?;
    Ok((StatusCode::CREATED, Json(template)))
}

async fn template_project_access(
    conn: &mut Conn<'_>,
    template_id: &str,
    user_id: &str,
) -> Result<(TaskTemplate, crate::projects::ProjectActor), AppError> {
    let mut template = find_template(conn, template_id).await?;
    let (_, actor) = require_project_access(conn, &template.project_id, user_id).await?;
    template.label_ids = load_template_labels(conn, &template.id).await?;
    Ok((template, actor))
}

async fn get_template(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(template_id): Path<String>,
) -> Result<Json<TaskTemplate>, AppError> {
    let mut conn = state.db.conn();
    let (template, actor) = template_project_access(&mut conn, &template_id, &user.id).await?;
    actor.require_team_member()?;
    Ok(Json(template))
}

async fn update_template(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(template_id): Path<String>,
    Json(body): Json<UpsertTemplateRequest>,
) -> Result<Json<TaskTemplate>, AppError> {
    let mut conn = state.db.begin().await?;
    let (template, _) = template_project_access(&mut conn, &template_id, &user.id).await?;
    require_template_manager(&mut conn, &template.project_id, &user.id).await?;
    let (name, title, description, priority) = validate_template(&body)?;
    let team_id = team_of(&mut conn, &template.project_id).await?;
    crate::tasks::validate_label_ids(&mut conn, &team_id, &body.label_ids).await?;

    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&name)
        .s(&title)
        .opt_s(description.clone())
        .s(priority)
        .i(now)
        .s(&template_id);
    conn.execute(
        "UPDATE task_templates SET name = ?, default_title = ?, default_description = ?, default_priority = ?, updated_at = ? \
         WHERE id = ?",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.s(&template_id);
    conn.execute(
        "DELETE FROM task_template_labels WHERE template_id = ?",
        args,
    )
    .await?;
    for label_id in &body.label_ids {
        let mut label_args = Args::new();
        label_args.s(&template_id).s(label_id);
        conn.execute(
            "INSERT INTO task_template_labels (template_id, label_id) VALUES (?, ?)",
            label_args,
        )
        .await?;
    }
    conn.commit().await?;

    let mut conn = state.db.conn();
    Ok(Json(find_template(&mut conn, &template_id).await?))
}

async fn delete_template(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(template_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let (template, _) = template_project_access(&mut conn, &template_id, &user.id).await?;
    require_template_manager(&mut conn, &template.project_id, &user.id).await?;
    let mut args = Args::new();
    args.s(&template_id);
    conn.execute("DELETE FROM task_templates WHERE id = ?", args)
        .await?;
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ===========================================================================
// Shared-with-me projects
// ===========================================================================

async fn my_projects(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let projects = crate::projects::list_shared_projects(&mut conn, &user.id).await?;
    Ok(Json(json!({ "items": projects })))
}
