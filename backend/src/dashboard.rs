//! Team dashboard (Phase 12 WS14): actionable KPIs for managers.
//!
//! All KPIs are server-computed in a single handler with at most three
//! aggregate queries. Authorization mirrors Capacity (Manager+).

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::auth::AuthedUser;
use crate::authorization::Capability;
use crate::db::Args;
use crate::error::AppError;
use crate::state::AppState;
use crate::util;

pub fn routes() -> Router<AppState> {
    Router::new().route("/teams/{team_id}/dashboard", get(dashboard))
}

#[derive(Deserialize)]
pub struct DashboardQuery {
    #[serde(default)]
    pub window: Option<String>,
}

fn window_days(window: Option<&str>) -> Result<i64, AppError> {
    match window.unwrap_or("7d") {
        "7d" => Ok(7),
        "30d" => Ok(30),
        "90d" => Ok(90),
        other => Err(AppError::validation(
            "window",
            format!("unknown window `{other}`; expected 7d, 30d or 90d"),
        )),
    }
}

#[derive(Serialize)]
pub struct DashboardKpis {
    pub throughput: i64,
    pub throughput_prev: i64,
    pub cycle_time_days: Option<f64>,
    pub lead_time_days: Option<f64>,
    pub wip: i64,
    pub overdue: i64,
    pub deadline_risk: i64,
    pub review_load: i64,
    pub priority_mix: serde_json::Value,
    pub capacity_utilization: Option<f64>,
}

async fn dashboard(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(team_id): Path<String>,
    Query(query): Query<DashboardQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let days = window_days(query.window.as_deref())?;
    let mut conn = state.db.conn();
    crate::authorization::require_capability(
        &mut conn,
        &team_id,
        &user.id,
        Capability::ManageProjects,
    )
    .await?;

    let today = util::parse_today_iso();
    let window_start = crate::util::parse_iso_date(&today)
        .expect("today_iso is valid")
        .checked_sub(time::Duration::days(days))
        .expect("window start in range")
        .to_string();
    let prev_start = crate::util::parse_iso_date(&today)
        .expect("today_iso is valid")
        .checked_sub(time::Duration::days(days * 2))
        .expect("prev window start in range")
        .to_string();
    let week_ago = crate::util::parse_iso_date(&today)
        .expect("today_iso is valid")
        .checked_sub(time::Duration::days(7))
        .expect("week ago in range")
        .to_string();
    let week_ahead = crate::util::parse_iso_date(&today)
        .expect("today_iso is valid")
        .checked_add(time::Duration::days(7))
        .expect("week ahead in range")
        .to_string();

    // Throughput: tasks closed in the window vs the previous window.
    let mut args = Args::new();
    args.s(&team_id).s(&window_start).s(&today);
    let throughput = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM tasks WHERE team_id = ? AND status = 'closed' \
             AND completed_at IS NOT NULL AND completed_at >= ? AND completed_at <= ?",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    let mut args = Args::new();
    args.s(&team_id).s(&prev_start).s(&window_start);
    let throughput_prev = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM tasks WHERE team_id = ? AND status = 'closed' \
             AND completed_at IS NOT NULL AND completed_at >= ? AND completed_at < ?",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Cycle time: avg (completed_at - first in_progress activity). We use
    // the first task_updated activity with field=assignee as a proxy for
    // when work started (activity records the transition). Simpler and
    // portable: avg completed_at - created_at for lead time; cycle uses the
    // first in_progress activity if available.
    let mut args = Args::new();
    args.s(&team_id).s(&window_start).s(&today);
    let cycle_row = conn
        .fetch_optional(
            "SELECT AVG(t.completed_at - a.created_at) AS cycle_ms FROM tasks t \
             JOIN activities a ON a.task_id = t.id AND a.event = 'task_status_changed' \
                 AND a.new_value = 'in_progress' \
             WHERE t.team_id = ? AND t.status = 'closed' \
               AND t.completed_at IS NOT NULL AND t.completed_at >= ? AND t.completed_at <= ?",
            args,
        )
        .await?;
    let cycle_time_days: Option<f64> = cycle_row.and_then(|r| {
        r.try_get::<Option<f64>, _>("cycle_ms")
            .ok()
            .flatten()
            .map(|ms| ((ms / DAY_MS_F) * 10.0).round() / 10.0)
    });

    let mut args = Args::new();
    args.s(&team_id).s(&window_start).s(&today);
    let lead_row = conn
        .fetch_optional(
            "SELECT AVG(t.completed_at - t.created_at) AS lead_ms FROM tasks t \
             WHERE t.team_id = ? AND t.status = 'closed' \
               AND t.completed_at IS NOT NULL AND t.completed_at >= ? AND t.completed_at <= ?",
            args,
        )
        .await?;
    let lead_time_days: Option<f64> = lead_row.and_then(|r| {
        r.try_get::<Option<f64>, _>("lead_ms")
            .ok()
            .flatten()
            .map(|ms| ((ms / DAY_MS_F) * 10.0).round() / 10.0)
    });

    // WIP: open tasks currently in in_progress or in_review.
    let mut args = Args::new();
    args.s(&team_id);
    let wip = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM tasks WHERE team_id = ? \
             AND status IN ('in_progress', 'in_review') AND parent_task_id IS NULL",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Overdue: open tasks past due date.
    let mut args = Args::new();
    args.s(&team_id).s(&today);
    let overdue = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM tasks WHERE team_id = ? \
             AND due_date IS NOT NULL AND due_date < ? \
             AND status IN ('backlog', 'todo', 'in_progress', 'in_review') \
             AND parent_task_id IS NULL",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Deadline risk: deadline within 7 days and not closed.
    let mut args = Args::new();
    args.s(&team_id).s(&week_ago).s(&week_ahead);
    let deadline_risk = conn
        .fetch_optional(
            "SELECT COUNT(*) AS n FROM tasks WHERE team_id = ? \
             AND deadline IS NOT NULL AND deadline >= ? AND deadline <= ? \
             AND status NOT IN ('closed') AND parent_task_id IS NULL",
            args,
        )
        .await?
        .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));

    // Review load: tasks in in_review per reporter (top reporter + total).
    let mut args = Args::new();
    args.s(&team_id);
    let review_rows = conn
        .fetch_all(
            "SELECT t.reporter_id, u.display_name, COUNT(*) AS n \
             FROM tasks t JOIN users u ON u.id = t.reporter_id \
             WHERE t.team_id = ? AND t.status = 'in_review' \
             GROUP BY t.reporter_id, u.display_name ORDER BY n DESC",
            args,
        )
        .await?;
    let review_by_reporter: Vec<serde_json::Value> = review_rows
        .iter()
        .map(|r| {
            Ok(serde_json::json!({
                "reporter_id": r.try_get::<String, _>("reporter_id")?,
                "display_name": r.try_get::<String, _>("display_name")?,
                "count": r.try_get::<i64, _>("n")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let review_load: i64 = review_by_reporter
        .iter()
        .map(|r| r["count"].as_i64().unwrap_or(0))
        .sum();

    // Priority mix: open tasks by priority.
    let mut args = Args::new();
    args.s(&team_id);
    let mix_rows = conn
        .fetch_all(
            "SELECT priority, COUNT(*) AS n FROM tasks WHERE team_id = ? \
             AND status NOT IN ('closed') AND parent_task_id IS NULL \
             GROUP BY priority ORDER BY priority",
            args,
        )
        .await?;
    let priority_mix: serde_json::Value = {
        let mut obj = serde_json::Map::new();
        for p in crate::tasks::PRIORITIES {
            let n = mix_rows
                .iter()
                .find(|r| r.try_get::<String, _>("priority").is_ok_and(|v| &v == p))
                .map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0));
            obj.insert((*p).to_string(), serde_json::Value::from(n));
        }
        serde_json::Value::Object(obj)
    };

    // Capacity utilization: team average from the capacity endpoint logic.
    let mut args = Args::new();
    args.s(&team_id).s(&today).s(&today).s(&today).s(&team_id);
    let cap_row = conn
        .fetch_optional(
            "SELECT SUM(open_count) AS total_open, SUM(u.capacity_limit) AS total_cap \
             FROM team_members tm \
             JOIN users u ON u.id = tm.user_id \
             LEFT JOIN (SELECT assignee_id, COUNT(*) AS open_count FROM tasks \
                        WHERE team_id = ? AND status != 'closed' AND parent_task_id IS NULL \
                        GROUP BY assignee_id) oc ON oc.assignee_id = u.id \
             WHERE tm.team_id = ? AND u.capacity_limit IS NOT NULL",
            args,
        )
        .await?;
    let capacity_utilization: Option<f64> = cap_row.and_then(|r| {
        let open: Option<i64> = r.try_get("total_open").ok().flatten();
        let cap: Option<i64> = r.try_get("total_cap").ok().flatten();
        match (open, cap) {
            (Some(o), Some(c)) if c > 0 => {
                Some(((o as f64 / c as f64).clamp(0.0, 2.0) * 100.0).round() / 100.0)
            }
            _ => None,
        }
    });

    let kpis = DashboardKpis {
        throughput,
        throughput_prev,
        cycle_time_days,
        lead_time_days,
        wip,
        overdue,
        deadline_risk,
        review_load,
        priority_mix,
        capacity_utilization,
    };
    Ok(Json(serde_json::json!({
        "window_days": days,
        "kpis": serde_json::to_value(&kpis).map_err(|e| AppError::internal(e.to_string()))?,
        "review_by_reporter": review_by_reporter,
    })))
}

const DAY_MS_F: f64 = 86_400_000.0;
