//! Outbox action-intent execution primitives and the idempotency ledger
//! (Phase 8A, Task 4).
//!
//! Ownership mapping (Master Prompt C2): this module owns the durable
//! action-result ledger (`rule_action_results`) coordination and the minimal
//! execution primitive that the Task 5 worker will call. Claim/fencing/lease
//! semantics stay with Task 5; this module's contract is "given a transaction
//! and an outbox intent id, execute the internal action atomically with its
//! durable result".
//!
//! Exactly-once internal effects: the internal side effect (comment /
//! notification rows) and the `rule_action_results` entry commit in the SAME
//! database transaction, together with the outbox row's `delivered`
//! transition. A crash before commit leaves nothing; after commit both
//! exist. The `UNIQUE(outbox_id)` ledger constraint is the final concurrency
//! defense; process-local state is never authoritative.
//!
//! `send_message` is external delivery: this module never performs it, never
//! records a delivery result for it, and leaves the intent `pending` for the
//! Task 5 dispatcher.

use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::util;

use super::actions::{ActionDef, is_internal, provision_principal, validate_stored_action};

/// What executing one intent produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcome {
    /// The internal effect and its ledger entry committed.
    Completed,
    /// A durable result already existed: no effect was re-applied.
    AlreadyExecuted,
    /// The action is external delivery: nothing executed, the intent stays
    /// `pending` for the Task 5 dispatcher.
    ExternalPending,
}

/// The scope chain every intent must satisfy: outbox → execution → event →
/// rule → task must all agree on team/task/rule/version identity. Mismatched
/// references are corruption and fail loudly.
struct IntentScope {
    execution_id: String,
    action_type: String,
    payload: String,
    rule_id: String,
    rule_version: i64,
    task_id: String,
    team_id: String,
}

impl IntentScope {
    fn validate(&self) -> Result<(), AppError> {
        if self.task_id.is_empty() || self.team_id.is_empty() || self.rule_id.is_empty() {
            return Err(AppError::internal(
                "corrupted automation references: incomplete scope chain",
            ));
        }
        Ok(())
    }
}

/// Load one intent with its full scope chain, validating cross-references.
async fn load_intent_scope(conn: &mut Conn<'_>, outbox_id: &str) -> Result<IntentScope, AppError> {
    let mut args = Args::new();
    args.s(outbox_id);
    let row = conn
        .fetch_optional(
            "SELECT o.action_type, o.payload, o.rule_id, o.rule_version, \
                    o.task_id AS intent_task_id, \
                    e.id AS execution_id, e.task_id AS execution_task_id, \
                    e.team_id AS execution_team_id, \
                    e.rule_version AS execution_rule_version, \
                    de.task_id AS event_task_id, de.team_id AS event_team_id, \
                    r.team_id AS rule_team_id \
             FROM automation_outbox o \
             JOIN rule_executions e ON e.id = o.execution_id \
             JOIN domain_events de ON de.id = o.event_id \
             JOIN rules r ON r.id = o.rule_id \
             WHERE o.id = ?",
            args,
        )
        .await?
        .ok_or(AppError::NotFound("outbox intent not found"))?;

    let scope = IntentScope {
        action_type: row.try_get("action_type")?,
        payload: row.try_get("payload")?,
        rule_id: row.try_get("rule_id")?,
        rule_version: row.try_get("rule_version")?,
        task_id: row.try_get("intent_task_id")?,
        execution_id: row.try_get("execution_id")?,
        team_id: row.try_get("execution_team_id")?,
    };
    let execution_task: String = row.try_get("execution_task_id")?;
    let execution_version: i64 = row.try_get("execution_rule_version")?;
    let event_task: String = row.try_get("event_task_id")?;
    let event_team: String = row.try_get("event_team_id")?;
    let rule_team: String = row.try_get("rule_team_id")?;

    if scope.task_id != execution_task
        || scope.task_id != event_task
        || scope.team_id != event_team
        || rule_team != event_team
        || scope.rule_version != execution_version
    {
        return Err(AppError::internal(
            "corrupted automation references: intent, execution, event, and rule do not \
             agree on task/team identity",
        ));
    }
    scope.validate()?;
    Ok(scope)
}

/// Execute one outbox intent's internal action. **Must be called inside the
/// caller's transaction** (the Task 5 worker opens it after claiming); the
/// caller commits. See the module docs for the exactly-once contract.
pub async fn execute_internal_action(
    conn: &mut Conn<'_>,
    outbox_id: &str,
) -> Result<ExecutionOutcome, AppError> {
    let scope = load_intent_scope(conn, outbox_id).await?;

    // Ledger pre-check: the authoritative duplicate guard. The
    // UNIQUE(outbox_id) constraint remains the concurrency defense.
    let mut args = Args::new();
    args.s(outbox_id);
    if conn
        .fetch_optional(
            "SELECT id FROM rule_action_results WHERE outbox_id = ?",
            args,
        )
        .await?
        .is_some()
    {
        return Ok(ExecutionOutcome::AlreadyExecuted);
    }

    // ---- Payload validation at execution time -----------------------------
    let action = validate_stored_action(&scope.action_type, &scope.payload)?;
    if !is_internal(&action) {
        // External delivery: preserve the intent untouched for Task 5. No
        // ledger row, no status change — never fake a delivery result.
        return Ok(ExecutionOutcome::ExternalPending);
    }

    // ---- Task existence / scope revalidation ------------------------------
    let mut args = Args::new();
    args.s(&scope.task_id);
    let task = conn
        .fetch_optional(
            "SELECT team_id, project_id, title, status, priority, assignee_id, reporter_id \
             FROM tasks WHERE id = ?",
            args,
        )
        .await?;
    let Some(task) = task else {
        // Hard-deleted task: a durable `skipped` result prevents pointless
        // retries while recording why. No internal effect is possible.
        return record_skipped(conn, outbox_id, &scope, "task_deleted").await;
    };
    let task_team: String = task.try_get("team_id")?;
    let task_project: String = task.try_get("project_id")?;
    if task_team != scope.team_id {
        return Err(AppError::internal(
            "corrupted automation references: task's team does not match the rule's team",
        ));
    }
    // Archived projects are read-only for everyone, including automation.
    let mut args = Args::new();
    args.s(&task_project);
    let project = conn
        .fetch_optional("SELECT status FROM projects WHERE id = ?", args)
        .await?;
    if project
        .and_then(|r| r.try_get::<String, _>("status").ok())
        .as_deref()
        == Some("archived")
    {
        return record_skipped(conn, outbox_id, &scope, "project_archived").await;
    }

    // The Automation Principal is provisioned inside this transaction: every
    // effect's author/actor FK needs the row, and provisioning is idempotent.
    let principal_id = provision_principal(conn).await?;

    let outcome_detail;
    let mut recipient_id: Option<String> = None;
    let mut author_id: Option<String> = None;
    let mut body_hash: Option<String> = None;
    match &action {
        ActionDef::AddComment { body } => {
            let comment_id = crate::comments::create_automation_comment(
                conn,
                &scope.team_id,
                &task_project,
                &scope.task_id,
                &principal_id,
                body,
            )
            .await?;
            let mut hasher = Sha256::new();
            hasher.update(body.as_bytes());
            body_hash = Some(format!("{:x}", hasher.finalize()));
            author_id = Some(principal_id.clone());
            outcome_detail = json!({ "comment_id": comment_id }).to_string();
        }
        ActionDef::SendNotification { recipients } => {
            // Resolve selectors against the CURRENT task state; explicit
            // recipients are revalidated as team members right here.
            // Departed/ineligible recipients are skipped (Task 1 contract).
            let assignee: Option<String> = task.try_get("assignee_id")?;
            let reporter: Option<String> = task.try_get("reporter_id")?;
            let mut targets: Vec<String> = Vec::new();
            let mut skipped = 0usize;
            for selector in &recipients.selectors {
                let candidate = match selector {
                    super::actions::RecipientSelector::Assignee => assignee.as_ref(),
                    super::actions::RecipientSelector::Reporter => reporter.as_ref(),
                };
                match candidate {
                    Some(id) if !targets.contains(id) => targets.push(id.clone()),
                    _ => skipped += 1,
                }
            }
            for user_id in &recipients.users {
                let mut args = Args::new();
                args.s(&scope.team_id).s(user_id);
                let member = conn
                    .fetch_optional(
                        "SELECT 1 AS one FROM team_members WHERE team_id = ? AND user_id = ?",
                        args,
                    )
                    .await?;
                if member.is_some() && !targets.contains(user_id) {
                    targets.push(user_id.clone());
                } else {
                    skipped += 1;
                }
            }
            let title: String = task.try_get("title")?;
            let mut batch: Vec<crate::notifications::NewNotification> = Vec::new();
            for recipient in &targets {
                batch.push(crate::notifications::NewNotification {
                    event: crate::notifications::events::AUTOMATION_TRIGGERED,
                    recipient_id: recipient.clone(),
                    task_id: scope.task_id.clone(),
                    actor_id: principal_id.clone(),
                    summary: format!("Automation rule triggered on \"{title}\""),
                });
            }
            crate::notifications::record_batch(conn, &batch).await?;
            outcome_detail = json!({
                "notified": batch.len(),
                "skipped": skipped,
            })
            .to_string();
            recipient_id = targets.first().cloned();
        }
        ActionDef::SendMessage { .. } => unreachable!("external actions returned above"),
    }

    // ---- Durable result + outbox completion, same transaction -------------
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(util::new_id())
        .s(outbox_id)
        .s(&scope.execution_id)
        .s(&scope.action_type)
        .s("completed")
        .opt_s(recipient_id)
        .opt_s(author_id)
        .opt_s(body_hash)
        .opt_s(Some(outcome_detail))
        .i(now);
    conn.execute(
        "INSERT INTO rule_action_results (id, outbox_id, execution_id, action_type, \
         outcome, recipient_id, author_id, body_hash, detail, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.i(now).i(now).s(outbox_id);
    let affected = conn
        .execute(
            "UPDATE automation_outbox SET status = 'delivered', completed_at = ?, \
             updated_at = ? WHERE id = ? AND status IN ('pending', 'processing')",
            args,
        )
        .await?;
    if affected == 0 {
        // Lost a completion race; the ledger entry rolls back with this
        // transaction and the next attempt's pre-check converges.
        return Err(AppError::Conflict(
            "automation outbox row was completed concurrently".into(),
        ));
    }
    Ok(ExecutionOutcome::Completed)
}

/// Record a durable `skipped` outcome (no internal effect) and complete the
/// intent. Same transaction as `execute_internal_action`.
async fn record_skipped(
    conn: &mut Conn<'_>,
    outbox_id: &str,
    scope: &IntentScope,
    reason: &str,
) -> Result<ExecutionOutcome, AppError> {
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(util::new_id())
        .s(outbox_id)
        .s(&scope.execution_id)
        .s(&scope.action_type)
        .s("skipped")
        .opt_s(None::<String>)
        .opt_s(None::<String>)
        .opt_s(None::<String>)
        .opt_s(Some(json!({ "reason": reason }).to_string()))
        .i(now);
    conn.execute(
        "INSERT INTO rule_action_results (id, outbox_id, execution_id, action_type, \
         outcome, recipient_id, author_id, body_hash, detail, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    let mut args = Args::new();
    args.i(now).i(now).s(outbox_id);
    conn.execute(
        "UPDATE automation_outbox SET status = 'delivered', completed_at = ?, \
         updated_at = ? WHERE id = ? AND status IN ('pending', 'processing')",
        args,
    )
    .await?;
    Ok(ExecutionOutcome::Completed)
}
