//! Event capture and transactional rule evaluation (Phase 8A, Task 3).
//!
//! `capture_and_evaluate` runs **inside the originating task-mutation
//! transaction** and implements the accepted Option-A pipeline:
//!
//! 1. persist the domain event (origin always comes from trusted server
//!    context — handlers hardcode it; no client field exists),
//! 2. apply the Task 1 origin policy (only user-origin events are evaluated),
//! 3. select enabled rules for the trigger, team-scoped, joined with their
//!    authoritative immutable version in one indexed query (no N+1),
//! 4. parse + evaluate each condition with the Task 1 evaluator (pure,
//!    infallible, bounded),
//! 5. persist an execution record per matching rule — idempotent via
//!    `UNIQUE(rule_id, event_id)`,
//! 6. persist one outbox intent per configured action, in declaration order,
//!    keyed by the deterministic idempotency key.
//!
//! Every fallible database operation propagates through `?`, so any failure
//! rolls back the task mutation, the event, the executions, and the intents
//! together. Nothing is swallowed: a stored rule definition that fails
//! Task 1 validation is a data-integrity violation, not a skip. No network
//! I/O occurs here and no action is executed — intents only (Task 4/5
//! execute).

use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::util;

use super::RuleError;
use super::actions::ActionDef;
use super::conditions::{Condition, EvalContext};
use super::events::{DomainEvent, EventOrigin, EventState, TriggerType, should_evaluate};

/// The authoritative task state after the mutation, as read/validated by the
/// caller from server-side data (never from raw request fields).
pub struct TaskState<'a> {
    pub team_id: &'a str,
    pub task_id: &'a str,
    pub status: &'a str,
    pub priority: &'a str,
    pub assignee_id: Option<&'a str>,
    pub reporter_id: Option<&'a str>,
}

/// A captured event: trigger type plus the trigger-relevant state change.
/// `actor_id` is the authenticated human who caused the mutation.
pub struct EventInput {
    pub event_type: TriggerType,
    pub actor_id: Option<String>,
    pub previous: EventState,
    pub new: EventState,
}

fn rule_definition_error(err: &RuleError) -> AppError {
    // A stored definition that no longer passes Task 1 validation means the
    // database was written by something other than the application: fail the
    // transaction loudly instead of silently skipping rules.
    AppError::internal(format!("stored rule definition failed validation: {err}"))
}

/// Deterministic delivery-intent key (sha-256 hex, 64 chars) over the
/// (event, rule version, action index) tuple — the dedup anchor for the
/// outbox and, later, for provider-side idempotency.
fn idempotency_key(event_id: &str, rule_id: &str, rule_version: i64, index: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(event_id.as_bytes());
    hasher.update(b"|");
    hasher.update(rule_id.as_bytes());
    hasher.update(b"|");
    hasher.update(rule_version.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(index.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Compact, bounded summary of the planned actions for the execution record:
/// `[{"index":0,"action_type":"add_comment"}, ...]` (≤5 entries).
fn planned_actions_json(actions: &[ActionDef]) -> String {
    let items: Vec<serde_json::Value> = actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let action_type = match action {
                ActionDef::AddComment { .. } => "add_comment",
                ActionDef::SendNotification { .. } => "send_notification",
                ActionDef::SendMessage { .. } => "send_message",
            };
            json!({ "index": index, "action_type": action_type })
        })
        .collect();
    serde_json::to_string(&serde_json::Value::Array(items))
        .expect("planned-actions summary serialization cannot fail")
}

/// Capture a domain event for the committed mutation and evaluate matching
/// rules. **Must be called inside the mutation's transaction**, after the
/// mutation and its activity/notification writes, before the caller commits.
pub async fn capture_and_evaluate(
    conn: &mut Conn<'_>,
    task: TaskState<'_>,
    event: EventInput,
    origin: EventOrigin,
) -> Result<(), AppError> {
    // ---- 1. Persist the domain event -------------------------------------
    let event_id = util::new_id();
    let now = util::now_ms();
    // Event state is bounded by construction (closed enums + UUIDs); the
    // `to_json_str` cap check is defense-in-depth on the persistence path.
    let previous_json = event
        .previous
        .to_json_str()
        .map_err(|e| rule_definition_error(&e))?;
    let new_json = event
        .new
        .to_json_str()
        .map_err(|e| rule_definition_error(&e))?;
    let mut args = Args::new();
    args.s(&event_id)
        .s(event.event_type.as_str())
        .i(i64::from(DomainEvent::SCHEMA_VERSION))
        .s(task.team_id)
        .s(task.task_id)
        .opt_s(event.actor_id.clone())
        .opt_s(Some(previous_json))
        .opt_s(Some(new_json))
        .s(origin.as_str())
        .opt_i(None::<i64>) // causing_execution_id: set only on automation-origin events
        .i(now);
    conn.execute(
        "INSERT INTO domain_events (id, event_type, schema_version, team_id, task_id, \
         actor_id, previous_state, new_state, origin, causing_execution_id, occurred_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;

    // ---- 2. Origin policy (Task 1): user-origin events only ---------------
    if !should_evaluate(origin) {
        return Ok(());
    }

    // ---- 3. Rule selection + version loading (two bounded queries) --------
    // An active rule whose current_version row is missing is schema-level
    // corruption (the application always writes both atomically): it must
    // fail the transaction loudly, never silently omit the rule.
    let mut args = Args::new();
    args.s(event.event_type.as_str()).s(task.team_id);
    let rule_rows = conn
        .fetch_all(
            "SELECT id, current_version FROM rules \
             WHERE trigger_type = ? AND enabled = TRUE AND team_id = ?",
            args,
        )
        .await?;
    let mut selected: Vec<(String, i64)> = Vec::with_capacity(rule_rows.len());
    for row in &rule_rows {
        selected.push((row.try_get("id")?, row.try_get("current_version")?));
    }
    let definitions = if selected.is_empty() {
        Vec::new()
    } else {
        let placeholders = vec!["?"; selected.len()].join(", ");
        let mut args = Args::new();
        for (rule_id, _) in &selected {
            args.s(rule_id.clone());
        }
        conn.fetch_all(
            &format!(
                "SELECT rule_id, version, condition_json, actions_json \
                 FROM rule_versions WHERE rule_id IN ({placeholders})"
            ),
            args,
        )
        .await?
    };
    let mut rules: Vec<(String, i64, String, String)> = Vec::with_capacity(selected.len());
    for (rule_id, current_version) in &selected {
        let definition = definitions
            .iter()
            .find(|row| {
                row.try_get::<String, _>("rule_id").unwrap_or_default() == *rule_id
                    && row.try_get::<i64, _>("version").unwrap_or_default() == *current_version
            })
            .ok_or_else(|| {
                AppError::internal(format!(
                    "active rule {rule_id} references missing rule version {current_version}"
                ))
            })?;
        rules.push((
            rule_id.clone(),
            *current_version,
            definition.try_get("condition_json")?,
            definition.try_get("actions_json")?,
        ));
    }

    for (rule_id, rule_version, condition_json, actions_json) in rules {
        // ---- 4. Parse the immutable definition and evaluate ---------------
        let condition =
            Condition::from_json_str(&condition_json).map_err(|e| rule_definition_error(&e))?;
        let actions =
            ActionDef::list_from_json_str(&actions_json).map_err(|e| rule_definition_error(&e))?;
        let ctx = EvalContext {
            task_status: task.status,
            task_priority: task.priority,
            task_assignee_id: task.assignee_id,
            task_reporter_id: task.reporter_id,
            event_actor_id: event.actor_id.as_deref(),
            event_previous_status: event.previous.status,
            event_new_status: event.new.status,
            event_previous_priority: event.previous.priority,
            event_new_priority: event.new.priority,
            event_trigger: event.event_type,
            event_origin: origin,
            event_causing_execution_id: None,
        };
        if !condition.evaluate(&ctx) {
            // Non-matching rules produce no records (Task 3 spec §5).
            continue;
        }

        // ---- 5. Execution record (idempotent per (rule, event)) -----------
        // Duplicate detection is an explicit existence check, NOT
        // rows_affected: sqlx-mysql always sets CLIENT_FOUND_ROWS
        // (connection/stream.rs), under which `ON DUPLICATE KEY UPDATE
        // id = id` reports 1 (found) - never 0 - for a duplicate, so
        // affected-row counting cannot distinguish a fresh insert from a
        // no-op on MySQL. The plain INSERT keeps every other constraint
        // violation loud; the UNIQUE constraint stays the final defense.
        let already_processed: i64 = {
            let mut args = Args::new();
            args.s(&rule_id).s(&event_id);
            let row = conn
                .fetch_optional(
                    "SELECT 1 AS one FROM rule_executions WHERE rule_id = ? AND event_id = ?",
                    args,
                )
                .await?;
            row.map_or(0, |r| r.try_get::<i64, _>("one").unwrap_or(0))
        };
        if already_processed == 1 {
            // This (rule, event) pair was already processed: the earlier pass
            // created the execution and its intents. Skip without duplicating.
            continue;
        }
        let execution_id = util::new_id();
        let mut args = Args::new();
        args.s(&execution_id)
            .s(&rule_id)
            .i(rule_version)
            .s(&event_id)
            .s(task.team_id)
            .s(task.task_id)
            .s(event.event_type.as_str())
            .s("matched")
            .s("planned")
            .opt_s(Some(planned_actions_json(&actions)))
            .i(now);
        conn.execute(
            "INSERT INTO rule_executions (id, rule_id, rule_version, event_id, team_id, \
             task_id, trigger_type, condition_result, status, planned_actions, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await?;

        // ---- 6. Action intents, deterministic declaration order -----------
        for (index, action) in actions.iter().enumerate() {
            let payload = serde_json::to_string(&action.to_json_value())
                .expect("canonical action serialization cannot fail");
            let idempotency_key = idempotency_key(&event_id, &rule_id, rule_version, index);
            // Same rationale as the execution pre-check above: an explicit
            // existence probe, never affected-row counting.
            let already_queued: i64 = {
                let mut args = Args::new();
                args.s(&idempotency_key);
                let row = conn
                    .fetch_optional(
                        "SELECT 1 AS one FROM automation_outbox WHERE idempotency_key = ?",
                        args,
                    )
                    .await?;
                row.map_or(0, |r| r.try_get::<i64, _>("one").unwrap_or(0))
            };
            if already_queued == 1 {
                continue;
            }
            let mut args = Args::new();
            args.s(util::new_id())
                .s(&execution_id)
                .s(&event_id)
                .s(&rule_id)
                .i(rule_version)
                .s(task.task_id)
                .s(action_type_of(action))
                .i(i64::from(DomainEvent::SCHEMA_VERSION))
                .s(&payload)
                .s(&idempotency_key)
                .s("pending")
                .i(0i64)
                .i(now) // next_attempt_at: eligible for the Task 5 worker
                .i(now)
                .i(now);
            conn.execute(
                "INSERT INTO automation_outbox (id, execution_id, event_id, rule_id, \
                 rule_version, task_id, action_type, schema_version, payload, \
                 idempotency_key, status, attempts, next_attempt_at, created_at, \
                 updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                args,
            )
            .await?;
        }
    }
    Ok(())
}

fn action_type_of(action: &ActionDef) -> &'static str {
    match action {
        ActionDef::AddComment { .. } => "add_comment",
        ActionDef::SendNotification { .. } => "send_notification",
        ActionDef::SendMessage { .. } => "send_message",
    }
}
