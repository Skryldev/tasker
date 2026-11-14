//! Advanced team execution (Phase 3): task dependencies, workload
//! aggregation, project task templates, and shared-with-me project listing.

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
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
            "/tasks/{task_id}/dependencies/{dep_id}",
            axum::routing::delete(remove_dependency),
        )
        .route(
            "/tasks/{task_id}/dependencies/graph",
            get(dependency_graph),
        )
        // Workload
        .route("/teams/{team_id}/workload", get(workload))
        .route("/teams/{team_id}/capacity", get(capacity))
        // Team-wide task search (Phase 12.5 Bug 6 pickers)
        .route("/teams/{team_id}/tasks", get(search_team_tasks))
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
        .route(
            "/tasks/from-template/{template_id}",
            post(create_task_from_template),
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
// Task dependencies (Phase 14 WS6 engine — ADR-0023)
// ===========================================================================

/// The four dependency types. Only `blocks` participates in cycle
/// prevention and scheduling; the rest are informational.
pub const DEPENDENCY_TYPES: [&str; 4] = ["blocks", "relates_to", "duplicates", "causes"];

#[derive(Debug, Clone, Serialize)]
pub struct DependencyEdge {
    pub id: String,
    pub from_task_id: String,
    pub to_task_id: String,
    #[serde(rename = "type")]
    pub dep_type: String,
    pub auto_schedule: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyTaskRef {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub due_date: Option<String>,
    /// The edge id (for removal).
    pub dep_id: String,
    /// The edge type (for grouped rendering).
    #[serde(rename = "type")]
    pub dep_type: String,
}

fn dep_edge_from_row(r: &sqlx::any::AnyRow) -> Result<DependencyEdge, AppError> {
    Ok(DependencyEdge {
        id: r.try_get("id")?,
        from_task_id: r.try_get("from_task_id")?,
        to_task_id: r.try_get("to_task_id")?,
        dep_type: r.try_get::<String, _>("type")?,
        auto_schedule: r
            .try_get::<i64, _>("auto_schedule")
            .map(|v| v != 0)
            .unwrap_or(false),
        created_at: r.try_get("created_at")?,
    })
}

/// Load one edge by (from, to) — the unique business key.
async fn load_edge(
    conn: &mut Conn<'_>,
    from_task: &str,
    to_task: &str,
    dep_type: &str,
) -> Result<Option<DependencyEdge>, AppError> {
    let mut args = Args::new();
    args.s(from_task).s(to_task).s(dep_type);
    let row = conn
        .fetch_optional(
            "SELECT id, from_task_id, to_task_id, type, \
             CASE WHEN auto_schedule THEN 1 ELSE 0 END AS auto_schedule, created_at \
             FROM task_dependencies WHERE from_task_id = ? AND to_task_id = ? AND type = ?",
            args,
        )
        .await?;
    row.map(|r| dep_edge_from_row(&r)).transpose()
}

/// DFS over `blocks` edges starting at `start`: if `target` is reachable,
/// adding start → target closes a cycle. Only `blocks` edges participate
/// (informational types never constrain). Terminates via the visited set.
async fn reachable(conn: &mut Conn<'_>, start: &str, target: &str) -> Result<bool, AppError> {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(start.to_string());
    let mut frontier = vec![start.to_string()];
    while let Some(node) = frontier.pop() {
        let mut args = Args::new();
        args.s(&node);
        let rows = conn
            .fetch_all(
                "SELECT from_task_id FROM task_dependencies WHERE to_task_id = ? AND type = 'blocks'",
                args,
            )
            .await?;
        for row in rows {
            let from: String = row.try_get("from_task_id")?;
            if from == target {
                return Ok(true);
            }
            if visited.insert(from.clone()) {
                frontier.push(from);
            }
        }
    }
    Ok(false)
}

#[derive(Deserialize)]
pub struct CreateDependencyRequest {
    /// Dependency type. Defaults to `blocks`.
    #[serde(default = "default_dep_type")]
    pub dep_type: String,
    /// The task this one depends on (its `from` endpoint).
    pub from_task_id: String,
    /// Opt-in CPM: pull this task's due date forward when the dependency's
    /// due date moves later.
    #[serde(default)]
    pub auto_schedule: bool,
}

fn default_dep_type() -> String {
    "blocks".to_string()
}

async fn create_dependency(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
    Json(body): Json<CreateDependencyRequest>,
) -> Result<(StatusCode, Json<DependencyEdge>), AppError> {
    let dep_type = body.dep_type.as_str();
    if !DEPENDENCY_TYPES.contains(&dep_type) {
        return Err(AppError::validation(
            "type",
            format!("unknown dependency type `{dep_type}`; expected one of {DEPENDENCY_TYPES:?}"),
        ));
    }
    let from_task = body.from_task_id.trim().to_string();
    if from_task == task_id {
        return Err(AppError::validation(
            "from_task_id",
            "a task cannot depend on itself",
        ));
    }

    let mut conn = state.db.begin().await?;
    let (to_task, actor) =
        crate::tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    if actor.is_guest() {
        return Err(deny_guest());
    }
    if crate::projects::find_project(&mut conn, &to_task.project_id)
        .await?
        .status
        == "archived"
    {
        return Err(AppError::Conflict("archived projects are read-only".into()));
    }

    // The dependency task must exist in the same project (and therefore team).
    let mut args = Args::new();
    args.s(&from_task).s(&to_task.project_id);
    let from_row = conn
        .fetch_optional("SELECT team_id FROM tasks WHERE id = ? AND project_id = ?", args)
        .await?
        .ok_or_else(|| {
            AppError::validation(
                "from_task_id",
                "the dependency task does not exist in this project",
            )
        })?;
    if from_row.try_get::<String, _>("team_id")? != to_task.team_id {
        return Err(AppError::internal("project/team mismatch")); // unreachable: project filter
    }

    if load_edge(&mut conn, &from_task, &task_id, dep_type)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("this dependency already exists".into()));
    }
    // Cycle prevention applies only to `blocks` (informational types never
    // constrain). A stored direct pair-edge is a two-task cycle.
    if dep_type == "blocks" {
        if load_edge(&mut conn, &task_id, &from_task, "blocks")
            .await?
            .is_some()
        {
            return Err(AppError::Conflict(
                "this dependency would create a cycle with the existing reverse dependency".into(),
            ));
        }
        // Serialize per project so two concurrent requests cannot both pass
        // the DFS check and close a loop together (db::ProjectDependencyLock).
        {
            let mut lock = crate::db::ProjectDependencyLock::for_driver(conn.driver());
            lock.acquire(&mut conn, &to_task.project_id).await;
            if reachable(&mut conn, &task_id, &from_task).await? {
                lock.release(&mut conn, &to_task.project_id).await;
                return Err(AppError::Conflict(
                    "this dependency would create a circular dependency: the dependency task already depends on this task (directly or transitively)".into(),
                ));
            }
            let id = util::new_id();
            let mut args = Args::new();
            args.s(&id)
                .s(&from_task)
                .s(&task_id)
                .s(dep_type)
                .b(body.auto_schedule)
                .s(&user.id)
                .i(util::now_ms());
            conn.execute(
                "INSERT INTO task_dependencies (id, from_task_id, to_task_id, type, auto_schedule, created_by, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                args,
            )
            .await?;
            lock.release(&mut conn, &to_task.project_id).await;
        }

        // Opt-in CPM: if both tasks carry due dates and this task's due date
        // precedes its dependency's, pull it forward to match.
        if body.auto_schedule {
            let mut args = Args::new();
            args.s(&from_task);
            let dep_due = conn
                .fetch_optional("SELECT due_date FROM tasks WHERE id = ?", args)
                .await?;
            if let Some(row) = dep_due {
                let dep_due_date: Option<String> = row.try_get("due_date").ok();
                if let Some(dep_due) = dep_due_date {
                    if let Some(my_due) = &to_task.due_date {
                        if my_due.as_str() < dep_due.as_str() {
                            let mut a = Args::new();
                            a.s(&dep_due).i(util::now_ms()).s(&task_id);
                            conn.execute(
                                "UPDATE tasks SET due_date = ?, updated_at = ? WHERE id = ?",
                                a,
                            )
                            .await?;
                        }
                    }
                }
            }
        }
    } else {
        // Informational types: no cycle check, no scheduling.
        let id = util::new_id();
        let mut args = Args::new();
        args.s(&id)
            .s(&from_task)
            .s(&task_id)
            .s(dep_type)
            .b(body.auto_schedule)
            .s(&user.id)
            .i(util::now_ms());
        conn.execute(
            "INSERT INTO task_dependencies (id, from_task_id, to_task_id, type, auto_schedule, created_by, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await?;
    }
    let edge = load_edge(&mut conn, &from_task, &task_id, dep_type)
        .await?
        .ok_or_else(|| AppError::internal("dependency insert did not persist"))?;
    conn.commit().await?;
    Ok((StatusCode::CREATED, Json(edge)))
}

async fn remove_dependency(
    State(state): State<AppState>,
    user: AuthedUser,
    Path((task_id, dep_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.db.begin().await?;
    let (_, actor) =
        crate::tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    if actor.is_guest() {
        return Err(deny_guest());
    }
    let mut args = Args::new();
    args.s(&dep_id).s(&task_id);
    let affected = conn
        .execute(
            "DELETE FROM task_dependencies WHERE id = ? AND to_task_id = ?",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::NotFound("dependency not found"));
    }
    conn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Grouped dependency listing: for `task_id`, `blocked_by` = edges whose
/// to_task_id is this task (other tasks this one depends on), `blocking` =
/// edges whose from_task_id is this task (other tasks depending on it).
async fn list_dependencies(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    let (_, actor) = crate::tasks::require_task_access(&mut conn, &task_id, &user.id).await?;
    // Guests see the edges read-only; only members manage them.
    let can_manage = !actor.is_guest();

    let mut args = Args::new();
    args.s(&task_id);
    let blocker_rows = conn
        .fetch_all(
            "SELECT d.id, d.type, t.id AS task_id, t.title, t.status, t.due_date \
             FROM task_dependencies d JOIN tasks t ON t.id = d.from_task_id \
             WHERE d.to_task_id = ? ORDER BY d.created_at",
            args,
        )
        .await?;
    let blocked_by: Vec<serde_json::Value> = blocker_rows
        .iter()
        .map(|r| {
            Ok(serde_json::json!({
                "dep_id": r.try_get::<String, _>("id")?,
                "type": r.try_get::<String, _>("type")?,
                "task_id": r.try_get::<String, _>("task_id")?,
                "title": r.try_get::<String, _>("title")?,
                "status": r.try_get::<String, _>("status")?,
                "due_date": r.try_get::<Option<String>, _>("due_date")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    let mut args = Args::new();
    args.s(&task_id);
    let successor_rows = conn
        .fetch_all(
            "SELECT d.id, d.type, t.id AS task_id, t.title, t.status, t.due_date \
             FROM task_dependencies d JOIN tasks t ON t.id = d.to_task_id \
             WHERE d.from_task_id = ? ORDER BY d.created_at",
            args,
        )
        .await?;
    let blocking: Vec<serde_json::Value> = successor_rows
        .iter()
        .map(|r| {
            Ok(serde_json::json!({
                "dep_id": r.try_get::<String, _>("id")?,
                "type": r.try_get::<String, _>("type")?,
                "task_id": r.try_get::<String, _>("task_id")?,
                "title": r.try_get::<String, _>("title")?,
                "status": r.try_get::<String, _>("status")?,
                "due_date": r.try_get::<Option<String>, _>("due_date")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(Json(serde_json::json!({
        "blocked_by": blocked_by,
        "blocking": blocking,
        "can_manage": can_manage,
    })))
}

/// Subgraph for visualization: the task's `blocks` dependencies and
/// blockers (task refs), expanded one level.
async fn dependency_graph(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    crate::tasks::require_task_access(&mut conn, &task_id, &user.id).await?;

    let mut args = Args::new();
    args.s(&task_id);
    let up_rows = conn
        .fetch_all(
            "SELECT d.from_task_id AS task_id, t.title, t.status, t.due_date \
             FROM task_dependencies d JOIN tasks t ON t.id = d.from_task_id \
             WHERE d.to_task_id = ? AND d.type = 'blocks'",
            args,
        )
        .await?;
    let mut args = Args::new();
    args.s(&task_id);
    let down_rows = conn
        .fetch_all(
            "SELECT d.to_task_id AS task_id, t.title, t.status, t.due_date \
             FROM task_dependencies d JOIN tasks t ON t.id = d.to_task_id \
             WHERE d.from_task_id = ? AND d.type = 'blocks'",
            args,
        )
        .await?;
    let node_json = |rows: &[sqlx::any::AnyRow]| -> Result<Vec<serde_json::Value>, AppError> {
        rows.iter()
            .map(|r| {
                Ok(serde_json::json!({
                    "task_id": r.try_get::<String, _>("task_id")?,
                    "title": r.try_get::<String, _>("title")?,
                    "status": r.try_get::<String, _>("status")?,
                    "due_date": r.try_get::<Option<String>, _>("due_date")?,
                }))
            })
            .collect()
    };
    Ok(Json(serde_json::json!({
        "task_id": task_id,
        "blocked_by": node_json(&up_rows)?,
        "blocking": node_json(&down_rows)?,
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
    /// Subtask titles applied when a task is created from this template
    /// (Phase 11). Stored as a JSON array of strings.
    #[serde(default)]
    pub default_subtasks: Vec<String>,
    pub created_by: Option<UserBrief>,
    pub created_at: i64,
    pub updated_at: i64,
}

const TEMPLATE_COLUMNS: &str = "t.id, t.project_id, t.name, t.default_title, t.default_description, \
     t.default_priority, t.default_subtasks, t.created_by, t.created_at, t.updated_at, \
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
        default_subtasks: row
            .try_get::<Option<String>, _>("default_subtasks")?
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default(),
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

/// Validate a template's default subtask titles: at most 20, each 1-200
/// characters after trimming; empty titles dropped.
fn validate_subtasks(raw: &[String]) -> Result<Vec<String>, AppError> {
    let mut out: Vec<String> = Vec::new();
    for title in raw {
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        if title.chars().count() > 200 {
            return Err(AppError::validation(
                "default_subtasks",
                "subtask titles must be at most 200 characters",
            ));
        }
        out.push(title.to_string());
        if out.len() >= 20 {
            break;
        }
    }
    Ok(out)
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
    /// Subtask titles (Phase 11); bounded to 20, each 1-200 chars.
    #[serde(default)]
    pub default_subtasks: Vec<String>,
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
        .fetch_optional(
            "SELECT team_id FROM projects WHERE id = ? AND deleted_at IS NULL",
            args,
        )
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
    let subtasks_json = serde_json::to_string(&validate_subtasks(&body.default_subtasks)?)
        .map_err(|e| AppError::internal(e.to_string()))?;

    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(&project_id)
        .s(&name)
        .s(&title)
        .opt_s(description.clone())
        .s(priority)
        .s(&subtasks_json)
        .s(&user.id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO task_templates (id, project_id, name, default_title, default_description, default_priority, default_subtasks, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    let subtasks_json = serde_json::to_string(&validate_subtasks(&body.default_subtasks)?)
        .map_err(|e| AppError::internal(e.to_string()))?;

    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&name)
        .s(&title)
        .opt_s(description.clone())
        .s(priority)
        .s(&subtasks_json)
        .i(now)
        .s(&template_id);
    conn.execute(
        "UPDATE task_templates SET name = ?, default_title = ?, default_description = ?, default_priority = ?, default_subtasks = ?, updated_at = ? \
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

// ---------------------------------------------------------------------------
// Capacity (Phase 10.5): forward-looking load per member.
//
// Distinct from Workload (a snapshot of the current distribution): capacity
// answers "how much more can each member take on in the coming window" by
// comparing open load against the member's configurable capacity limit.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CapacityQuery {
    /// Rolling window for the due-soon counters: `week` (7 days, default)
    /// or `month` (30 days).
    #[serde(default)]
    pub window: Option<String>,
}

fn capacity_window_days(window: Option<&str>) -> Result<i64, AppError> {
    match window.unwrap_or("week") {
        "week" => Ok(7),
        "month" => Ok(30),
        other => Err(AppError::validation(
            "window",
            format!("unknown window `{other}`; expected week or month"),
        )),
    }
}

/// Team-wide task search (Phase 12.5 Bug 6): server-paginated picker source
/// for goal associations and any future cross-project surface. Membership
/// only — results carry id/title/status plus the project for disambiguation.
#[derive(Deserialize)]
pub struct TeamTaskSearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub page_size: Option<i64>,
}

async fn search_team_tasks(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Query(query): Query<TeamTaskSearchQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut conn = state.db.conn();
    crate::teams::require_membership(&mut conn, &team_id, &user.id).await?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(25).clamp(1, 100);
    let offset = (page - 1) * page_size;
    let like = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(crate::util::escape_like)
        .map(|q| format!("%{q}%"));

    let total: i64 = match &like {
        Some(pattern) => {
            let mut args = Args::new();
            args.s(&team_id).s(pattern.clone());
            conn.fetch_optional(
                "SELECT COUNT(*) AS n FROM tasks t                  JOIN projects p ON p.id = t.project_id AND p.deleted_at IS NULL                  WHERE t.team_id = ? AND t.title LIKE ? ESCAPE '|'                    AND t.parent_task_id IS NULL",
                args,
            )
            .await?
            .map_or(0, |r| r.try_get("n").unwrap_or(0))
        }
        None => {
            let mut args = Args::new();
            args.s(&team_id);
            conn.fetch_optional(
                "SELECT COUNT(*) AS n FROM tasks t                  JOIN projects p ON p.id = t.project_id AND p.deleted_at IS NULL                  WHERE t.team_id = ? AND t.parent_task_id IS NULL",
                args,
            )
            .await?
            .map_or(0, |r| r.try_get("n").unwrap_or(0))
        }
    };

    let rows = match &like {
        Some(pattern) => {
            let mut args = Args::new();
            args.s(&team_id).s(pattern.clone()).i(page_size).i(offset);
            conn.fetch_all(
                "SELECT t.id, t.title, t.status, t.project_id, p.name AS project_name                  FROM tasks t                  JOIN projects p ON p.id = t.project_id AND p.deleted_at IS NULL                  WHERE t.team_id = ? AND t.title LIKE ? ESCAPE '|'                    AND t.parent_task_id IS NULL                  ORDER BY t.updated_at DESC LIMIT ? OFFSET ?",
                args,
            )
            .await?
        }
        None => {
            let mut args = Args::new();
            args.s(&team_id).i(page_size).i(offset);
            conn.fetch_all(
                "SELECT t.id, t.title, t.status, t.project_id, p.name AS project_name                  FROM tasks t                  JOIN projects p ON p.id = t.project_id AND p.deleted_at IS NULL                  WHERE t.team_id = ? AND t.parent_task_id IS NULL                  ORDER BY t.updated_at DESC LIMIT ? OFFSET ?",
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
                "title": row.try_get::<String, _>("title")?,
                "status": row.try_get::<String, _>("status")?,
                "project_id": row.try_get::<String, _>("project_id")?,
                "project_name": row.try_get::<String, _>("project_name")?,
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

async fn capacity(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Query(query): Query<CapacityQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let window_days = capacity_window_days(query.window.as_deref())?;

    // Team-level capacity data is a management view: Manager and above.
    let mut conn = state.db.conn();
    crate::authorization::require_capability(
        &mut conn,
        &team_id,
        &user.id,
        crate::authorization::Capability::ManageProjects,
    )
    .await?;

    // One pass over open tasks joined to the team roster; every counter is a
    // conditional SUM so the whole response is two queries (roster + tasks).
    let today = today_iso();
    // Window end as an ISO date string: due_date is TEXT ISO, so string
    // comparison is calendar-correct on every engine (portable-SQL rule -
    // no engine-specific date() functions).
    let window_end = crate::util::parse_iso_date(&today)
        .expect("today_iso is a valid ISO date")
        .checked_add(time::Duration::days(window_days))
        .expect("window end stays in range")
        .to_string();
    let mut args = Args::new();
    args.s(&today)
        .s(&today)
        .s(&window_end)
        .s(&team_id)
        .s(&team_id);
    let rows = conn
        .fetch_all(
            "SELECT u.id AS user_id, u.display_name, u.capacity_limit, \
                    COUNT(t.id) AS open_tasks, \
                    COALESCE(SUM(CASE WHEN t.due_date IS NOT NULL AND t.due_date < ? THEN 1 ELSE 0 END), 0) AS overdue_tasks, \
                    COALESCE(SUM(CASE WHEN t.due_date IS NOT NULL AND t.due_date >= ? AND t.due_date < ? THEN 1 ELSE 0 END), 0) AS due_soon \
             FROM team_members tm \
             JOIN users u ON u.id = tm.user_id \
             LEFT JOIN tasks t ON t.assignee_id = u.id AND t.team_id = ? \
                  AND t.status != 'closed' AND t.parent_task_id IS NULL \
                  AND NOT EXISTS (SELECT 1 FROM projects dp
                                  WHERE dp.id = t.project_id AND dp.deleted_at IS NOT NULL) \
             WHERE tm.team_id = ? \
             GROUP BY u.id, u.display_name, u.capacity_limit \
             ORDER BY u.display_name",
            args,
        )
        .await?;

    let mut items = Vec::with_capacity(rows.len());
    let mut total_open: i64 = 0;
    let mut total_capacity: i64 = 0;
    for row in rows {
        let user_id: String = row.try_get("user_id")?;
        let display_name: String = row.try_get("display_name")?;
        let capacity_limit: i64 = row
            .try_get::<i64, _>("capacity_limit")
            .unwrap_or(crate::users::CAPACITY_DEFAULT)
            .clamp(crate::users::CAPACITY_MIN, crate::users::CAPACITY_MAX);
        let open_tasks: i64 = row.try_get("open_tasks")?;
        let overdue_tasks: i64 = row.try_get("overdue_tasks")?;
        let due_soon: i64 = row.try_get("due_soon")?;
        let utilization = (open_tasks as f64 / capacity_limit.max(1) as f64).clamp(0.0, 2.0);
        let status = if utilization > 1.0 {
            "overloaded"
        } else if utilization >= 0.6 {
            "balanced"
        } else {
            "underutilized"
        };
        total_open += open_tasks;
        total_capacity += capacity_limit;
        items.push(serde_json::json!({
            "user_id": user_id,
            "display_name": display_name,
            "avatar_url": format!("/api/v1/users/{user_id}/avatar"),
            "open_tasks": open_tasks,
            "overdue_tasks": overdue_tasks,
            // `due_this_week` / `due_this_month` per the contract, resolved by
            // the requested window (rolling 7 or 30 days from today).
            "due_this_week": serde_json::Value::from(if window_days == 7 { Some(due_soon) } else { None }),
            "due_this_month": serde_json::Value::from(if window_days == 30 { Some(due_soon) } else { None }),
            "due_in_window": due_soon,
            "capacity_limit": capacity_limit,
            "utilization": (utilization * 100.0).round() / 100.0,
            "status": status,
        }));
    }
    Ok(Json(serde_json::json!({
        "window_days": window_days,
        "summary": {
            "total_open_tasks": total_open,
            "total_capacity": total_capacity,
            "utilization": if total_capacity > 0 {
                {
                    let ratio = (total_open as f64 / total_capacity as f64).clamp(0.0, 2.0);
                    (ratio * 100.0).round() / 100.0
                }
            } else {
                0.0
            },
        },
        "items": items,
    })))
}

// ---------------------------------------------------------------------------
// Create-from-template (Phase 11): builds a task (plus its subtasks) from a
// template's defaults, with per-field overrides. All existing task
// validation, workflow constraints, and authorization run unchanged via
// `create_task_inner` — the template cannot bypass any rule.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct FromTemplateRequest {
    /// Overrides the template's default title.
    #[serde(default)]
    pub title: Option<String>,
    /// Overrides the template's default description.
    #[serde(default)]
    pub default_description: Option<String>,
    /// Overrides the template's default priority.
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    /// Overrides the template deadline (must be >= due_date when both set).
    #[serde(default)]
    pub deadline: Option<String>,
    /// Overrides the creation status (backlog/todo/in_progress only).
    #[serde(default)]
    pub status: Option<String>,
    /// Overrides the template's default label set.
    #[serde(default)]
    pub label_ids: Option<Vec<String>>,
    /// Overrides the template's default subtask titles.
    #[serde(default)]
    pub subtasks: Option<Vec<String>>,
}

async fn create_task_from_template(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(template_id): Path<String>,
    Json(overrides): Json<FromTemplateRequest>,
) -> Result<(StatusCode, Json<crate::tasks::Task>), AppError> {
    let mut conn = state.db.conn();
    let (template, actor) = template_project_access(&mut conn, &template_id, &user.id).await?;
    actor.require_team_member()?;
    if actor.is_guest() {
        return Err(deny_guest());
    }

    // Template defaults with per-field overrides. Every field still passes
    // through create_task_inner's validation and authorization.
    let body = crate::tasks::CreateTaskRequest {
        title: overrides
            .title
            .unwrap_or_else(|| template.default_title.clone()),
        description: overrides
            .default_description
            .or_else(|| template.default_description.clone()),
        status: None,
        priority: Some(
            overrides
                .priority
                .unwrap_or_else(|| template.default_priority.clone()),
        ),
        assignee_id: overrides.assignee_id.clone(),
        reporter_id: None,
        due_date: overrides.due_date.clone(),
        deadline: overrides.deadline.clone(),
        parent_task_id: None,
        label_ids: template.label_ids.clone(),
    };

    let task = crate::tasks::create_task_inner(
        state.clone(),
        user.clone(),
        template.project_id.clone(),
        body,
        None,
    )
    .await?;

    // Provenance + subtasks after the main task exists.
    let subtask_titles = overrides
        .subtasks
        .clone()
        .unwrap_or_else(|| template.default_subtasks.clone());
    {
        let mut conn = state.db.begin().await?;
        let mut args = Args::new();
        args.s(&template.id).s(&task.id);
        conn.execute("UPDATE tasks SET from_template_id = ? WHERE id = ?", args)
            .await?;
        conn.commit().await?;
    }
    for title in &subtask_titles {
        crate::tasks::create_task_inner(
            state.clone(),
            user.clone(),
            template.project_id.clone(),
            crate::tasks::CreateTaskRequest {
                title: title.clone(),
                description: None,
                status: None,
                priority: Some(template.default_priority.clone()),
                assignee_id: overrides.assignee_id.clone(),
                reporter_id: None,
                due_date: None,
                deadline: None,
                parent_task_id: Some(task.id.clone()),
                label_ids: Vec::new(),
            },
            Some(task.id.clone()),
        )
        .await?;
    }

    let fresh = crate::tasks::read_task(&state, &user, &task.id).await?;
    Ok((StatusCode::CREATED, Json(fresh)))
}
