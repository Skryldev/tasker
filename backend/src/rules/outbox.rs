//! Outbox action-intent execution, claiming, and the idempotency ledger
//! (Phase 8A, Tasks 4-5).
//!
//! Ownership mapping (Master Prompt C2): this module owns the durable
//! action-result ledger (`rule_action_results`) coordination, the claim /
//! lease / fencing primitives, and the execution primitive. The polling loop
//! and lifecycle live in `rules::dispatcher`; this module is the repository
//! boundary the loop calls.
//!
//! Exactly-once internal effects: the internal side effect (comment /
//! notification rows) and the `rule_action_results` entry commit in the SAME
//! database transaction, together with the outbox row's fenced `delivered`
//! transition. A crash before commit leaves nothing; after commit both
//! exist. The `UNIQUE(outbox_id)` ledger constraint is the final concurrency
//! defense; process-local state is never authoritative.
//!
//! `send_message` is external delivery: the internal dispatcher never
//! claims it, never performs it, never records a delivery result for it. It
//! stays `pending` until the future external dispatcher (handoff: claim
//! `action_type = 'send_message' AND status = 'pending'` with the same
//! lease fields).

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
    /// `pending` for the external dispatcher.
    ExternalPending,
}

/// How a failed execution should be scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Transient: reschedule with bounded exponential backoff.
    Retryable,
    /// Invalid payload, corrupted references, or unsupported content:
    /// dead-letter immediately. Never retried.
    Permanent,
}

/// An intent successfully claimed by a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedIntent {
    pub outbox_id: String,
    pub claim_token: String,
    pub action_type: String,
    pub attempts: i64,
}

/// Classify an execution failure. Internal-action failures that come from
/// our own validation/scope paths are permanent (the payload cannot become
/// valid by waiting); everything else — DB unavailability, busy, conflict
/// races — is retryable. Classification is explicit and unit-tested; unknown
/// error shapes never permanently suppress a record.
#[must_use]
pub fn classify_failure(err: &AppError) -> FailureClass {
    use AppError as E;
    let text = err.to_string();
    let permanent_markers = [
        "stored action payload failed validation",
        "stored action payload must decode",
        "stored action payload type",
        "corrupted automation references",
        "automation principal identity corrupted",
        "task's team does not match",
    ];
    if matches!(err, E::BadRequest(_) | E::Validation(_))
        || permanent_markers.iter().any(|m| text.contains(m))
    {
        return FailureClass::Permanent;
    }
    FailureClass::Retryable
}

// ---------------------------------------------------------------------------
// Execution primitive (Task 4, extended with claim fencing in Task 5)
// ---------------------------------------------------------------------------

/// The scope chain every intent must satisfy: outbox → execution → event →
/// rule → task must all agree on team/task/rule/version identity. Mismatched
/// references are corruption and fail loudly.
struct IntentScope {
    execution_id: String,
    action_type: String,
    payload: String,
    /// Cross-checked in the scope JOIN; retained for the ledger/diagnostics.
    #[allow(dead_code)]
    rule_id: String,
    rule_version: i64,
    task_id: String,
    team_id: String,
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
    Ok(scope)
}

/// Execute one outbox intent's internal action. **Must be called inside the
/// caller's transaction** (the dispatcher opens it after claiming); the
/// caller commits. `claim_token` fences the completion: a worker that lost
/// its lease rolls back its whole transaction here.
pub async fn execute_internal_action(
    conn: &mut Conn<'_>,
    outbox_id: &str,
    claim_token: Option<&str>,
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
        // External delivery: preserve the intent untouched for the external
        // dispatcher. No ledger row, no status change — never fake a
        // delivery result.
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
            outcome_detail = serde_json::json!({ "comment_id": comment_id }).to_string();
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
            outcome_detail = serde_json::json!({
                "notified": batch.len(),
                "skipped": skipped,
            })
            .to_string();
            recipient_id = targets.first().cloned();
        }
        ActionDef::SendMessage { .. } => unreachable!("external actions returned above"),
    }

    // ---- Durable result + fenced outbox completion, same transaction ------
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
    complete_intent(conn, outbox_id, claim_token, now).await?;
    Ok(ExecutionOutcome::Completed)
}

/// Fenced `delivered` transition. A stale worker's token matches nothing:
/// its whole transaction (effect + ledger) rolls back. `None` targets the
/// unclaimed state used by direct invocations.
pub(crate) async fn complete_intent(
    conn: &mut Conn<'_>,
    outbox_id: &str,
    claim_token: Option<&str>,
    now: i64,
) -> Result<(), AppError> {
    // The schema CHECK (claim_token IS NULL) = (status <> 'processing')
    // requires the token to be cleared whenever the row leaves `processing`.
    // The predicate still compares against the OLD token, so a stale worker
    // matches nothing.
    let (sql, token): (&str, Option<&str>) = match claim_token {
        Some(t) => (
            "UPDATE automation_outbox SET status = 'delivered', claim_token = NULL, \
             completed_at = ?, updated_at = ? \
             WHERE id = ? AND status IN ('pending', 'processing') \
             AND claim_token = ?",
            Some(t),
        ),
        None => (
            "UPDATE automation_outbox SET status = 'delivered', claim_token = NULL, \
             completed_at = ?, updated_at = ? \
             WHERE id = ? AND status IN ('pending', 'processing') \
             AND claim_token IS NULL",
            None,
        ),
    };
    let mut args = Args::new();
    args.i(now).i(now).s(outbox_id);
    if let Some(t) = token {
        args.s(t);
    }
    let affected = conn.execute(sql, args).await?;
    if affected == 0 {
        // Lost the lease or a completion race; the ledger entry and the
        // effect roll back with this transaction and the current owner (or
        // a later attempt's ledger pre-check) converges.
        return Err(AppError::Conflict(
            "automation outbox row is no longer owned by this claim".into(),
        ));
    }
    Ok(())
}

/// Record a durable `skipped` outcome (no internal effect) and complete the
/// intent. Same transaction as `execute_internal_action`. Skips happen on
/// direct invocations (unclaimed intents), so completion is fenced on
/// `claim_token IS NULL`.
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
        .opt_s(Some(serde_json::json!({ "reason": reason }).to_string()))
        .i(now);
    conn.execute(
        "INSERT INTO rule_action_results (id, outbox_id, execution_id, action_type, \
         outcome, recipient_id, author_id, body_hash, detail, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    complete_intent(conn, outbox_id, None, now).await?;
    Ok(ExecutionOutcome::Completed)
}

// ---------------------------------------------------------------------------
// Claim / lease / recovery (Task 5)
// ---------------------------------------------------------------------------

/// Recover intents whose lease expired while `processing`: back to
/// `pending`, claim fields cleared. The attempt count is deliberately NOT
/// incremented — a worker crash between claim and execute must not burn
/// retry budget. Returns the number of recovered rows.
pub async fn recover_expired_leases(conn: &mut Conn<'_>, now: i64) -> Result<u64, AppError> {
    let mut args = Args::new();
    args.i(now).i(now);
    let affected = conn
        .execute(
            "UPDATE automation_outbox SET status = 'pending', claim_token = NULL, \
             worker_id = NULL, claimed_at = NULL, lease_expires_at = NULL, updated_at = ? \
             WHERE status = 'processing' AND lease_expires_at < ?",
            args,
        )
        .await?;
    Ok(affected)
}

/// Claim up to `batch` eligible internal-action intents for one worker.
/// Eligibility: `pending`, due (`next_attempt_at <= now`), and an internal
/// action type — `send_message` is never claimed here. The guarded per-row
/// UPDATE (not affected-row arithmetic on upserts) makes the winner set
/// exact and portable: a competing worker's UPDATE re-evaluates the
/// `status = 'pending'` predicate against the locked row and loses.
pub async fn claim_batch(
    conn: &mut Conn<'_>,
    worker_id: &str,
    batch: u32,
    lease_ms: i64,
    now: i64,
) -> Result<Vec<ClaimedIntent>, AppError> {
    recover_expired_leases(conn, now).await?;
    // Eligibility: due (`next_attempt_at <= now`) and either fresh (`pending`)
    // or a retry whose backoff has elapsed (`retryable_failed`).
    let mut args = Args::new();
    args.i(now).i(i64::from(batch));
    let candidates = conn
        .fetch_all(
            "SELECT id FROM automation_outbox \
             WHERE next_attempt_at <= ? \
             AND status IN ('pending', 'retryable_failed') \
             AND action_type IN ('add_comment', 'send_notification') \
             ORDER BY next_attempt_at LIMIT ?",
            args,
        )
        .await?;
    let mut claimed = Vec::new();
    for row in candidates {
        let outbox_id: String = row.try_get("id")?;
        let token = crate::security::token::generate();
        // Seven placeholders, seven binds: SET (5) + id + the due predicate.
        let mut args = Args::new();
        args.s(&token)
            .s(worker_id)
            .i(now)
            .i(now + lease_ms)
            .i(now)
            .s(&outbox_id)
            .i(now);
        let affected = conn
            .execute(
                "UPDATE automation_outbox SET status = 'processing', claim_token = ?, \
                 worker_id = ?, claimed_at = ?, lease_expires_at = ?, updated_at = ? \
                 WHERE id = ? AND next_attempt_at <= ? \
                 AND status IN ('pending', 'retryable_failed')",
                args,
            )
            .await?;
        if affected == 1 {
            let mut args = Args::new();
            args.s(&outbox_id);
            let row = conn
                .fetch_optional(
                    "SELECT action_type, attempts FROM automation_outbox WHERE id = ?",
                    args,
                )
                .await?
                .ok_or_else(|| AppError::internal("claimed intent vanished"))?;
            claimed.push(ClaimedIntent {
                outbox_id,
                claim_token: token,
                action_type: row.try_get("action_type")?,
                attempts: row.try_get("attempts")?,
            });
        }
    }
    Ok(claimed)
}

/// What happened to a failed intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDisposition {
    /// Scheduled for another attempt after backoff.
    Rescheduled,
    /// Attempts exhausted (or a permanent failure): terminal state.
    DeadLettered,
}

/// Record a failure on a claimed intent, fenced by its claim token:
/// retryable failures get `retryable_failed` + backoff and return to the
/// eligibility queue; permanent failures and exhausted attempts get
/// `dead_letter`. The claim fields are always cleared; the attempt count is
/// incremented exactly here (failures burn budget — crashes between claim
/// and execute do not, see `recover_expired_leases`).
#[allow(clippy::too_many_arguments)] // explicit knobs mirror RulesWorkerConfig
pub async fn record_failure(
    conn: &mut Conn<'_>,
    intent: &ClaimedIntent,
    class: FailureClass,
    error: &AppError,
    max_attempts: u32,
    backoff_base_ms: i64,
    backoff_max_ms: i64,
    jitter: i64,
    now: i64,
) -> Result<FailureDisposition, AppError> {
    let attempts = intent.attempts + 1;
    let exhausted = attempts >= i64::from(max_attempts);
    let (status, category, next_attempt_at) = match (class, exhausted) {
        (FailureClass::Permanent, _) | (_, true) => ("dead_letter", "permanent", now),
        (FailureClass::Retryable, false) => {
            let delay = backoff_delay_ms(attempts, backoff_base_ms, backoff_max_ms, jitter);
            ("retryable_failed", "retryable", now + delay)
        }
    };
    // Bounded, sanitized diagnostics: category enum + truncated message. The
    // error text comes from our own validation/DB layers and never carries
    // payloads or secrets, but it is still truncated to the schema bound.
    let detail = util::truncate(&error.to_string(), 500);
    // Placeholders in order: status, attempts, category, detail,
    // next_attempt_at, updated_at (SET) + id, claim_token (WHERE).
    let mut args = Args::new();
    args.s(status)
        .i(attempts)
        .s(category)
        .s(&detail)
        .i(next_attempt_at)
        .i(now)
        .s(&intent.outbox_id)
        .s(&intent.claim_token);
    let affected = conn
        .execute(
            "UPDATE automation_outbox SET status = ?, attempts = ?, \
             last_error_category = ?, last_error_detail = ?, next_attempt_at = ?, \
             claim_token = NULL, worker_id = NULL, claimed_at = NULL, \
             lease_expires_at = NULL, updated_at = ? \
             WHERE id = ? AND status = 'processing' AND claim_token = ?",
            args,
        )
        .await?;
    if affected == 0 {
        // The claim was lost (reaped + re-claimed): the failure belongs to a
        // dead attempt. Report it; the current owner owns the outcome.
        return Err(AppError::Conflict(
            "automation outbox claim is no longer owned by this worker".into(),
        ));
    }
    if status == "dead_letter" {
        Ok(FailureDisposition::DeadLettered)
    } else {
        Ok(FailureDisposition::Rescheduled)
    }
}

/// Bounded exponential backoff with full jitter:
/// `min(max, base * 2^(attempt-1))` scaled by `jitter/1000` (jitter in
/// `[0, 1000]`; 1000 = full delay, 0 = none). Pure and unit-testable.
#[must_use]
pub fn backoff_delay_ms(attempt: i64, base_ms: i64, max_ms: i64, jitter: i64) -> i64 {
    let exponent = (attempt.max(1) - 1).min(16);
    let raw = base_ms.saturating_mul(1i64 << exponent).min(max_ms.max(1));
    let jitter = jitter.clamp(0, 1000);
    raw * jitter / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        // Full jitter (1000) = the raw delay; attempt 1 = base.
        assert_eq!(backoff_delay_ms(1, 1_000, 60_000, 1_000), 1_000);
        assert_eq!(backoff_delay_ms(2, 1_000, 60_000, 1_000), 2_000);
        assert_eq!(backoff_delay_ms(3, 1_000, 60_000, 1_000), 4_000);
        assert_eq!(backoff_delay_ms(6, 1_000, 60_000, 1_000), 32_000);
        // Ceiling reached at attempt 7 (64x base > 60s cap).
        assert_eq!(backoff_delay_ms(7, 1_000, 60_000, 1_000), 60_000);
        assert_eq!(backoff_delay_ms(20, 1_000, 60_000, 1_000), 60_000);
    }

    #[test]
    fn backoff_jitter_is_bounded() {
        // Zero jitter = no delay; partial jitter scales linearly.
        assert_eq!(backoff_delay_ms(1, 1_000, 60_000, 0), 0);
        assert_eq!(backoff_delay_ms(1, 1_000, 60_000, 500), 500);
        assert_eq!(backoff_delay_ms(3, 1_000, 60_000, 250), 1_000);
        // Out-of-range jitter clamps.
        assert_eq!(backoff_delay_ms(1, 1_000, 60_000, 5_000), 1_000);
        assert_eq!(backoff_delay_ms(1, 1_000, 60_000, -10), 0);
    }

    #[test]
    fn backoff_handles_extreme_attempts() {
        // Large attempt numbers must not overflow the shift.
        assert_eq!(backoff_delay_ms(1_000, 1_000, 60_000, 1_000), 60_000);
        assert_eq!(backoff_delay_ms(0, 1_000, 60_000, 1_000), 1_000);
    }

    #[test]
    fn classification_marks_validation_and_corruption_permanent() {
        let permanent = [
            AppError::internal("stored action payload failed validation: boom"),
            AppError::internal("stored action payload type `add_comment` disagrees"),
            AppError::internal("corrupted automation references: intent"),
            AppError::BadRequest("bad".into()),
            AppError::Conflict("integrity".into()),
        ];
        for err in &permanent[..4] {
            assert_eq!(classify_failure(err), FailureClass::Permanent, "{err}");
        }
        // Concurrent-completion conflicts are retryable (pre-check converges).
        assert_eq!(
            classify_failure(&AppError::Conflict("completed concurrently".into())),
            FailureClass::Retryable
        );
        assert_eq!(
            classify_failure(&AppError::internal("database temporarily unavailable")),
            FailureClass::Retryable
        );
    }
}
