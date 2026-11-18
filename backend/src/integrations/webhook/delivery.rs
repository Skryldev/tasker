//! Delivery repository (Phase 8B, Task 2): the durable state machine,
//! lease-based claiming, claim-token fencing, failure recording, and
//! auditable manual replay for `webhook_deliveries`.
//!
//! Ownership mapping (mirrors `rules::outbox`): this module is the repository
//! boundary — the polling loop and lifecycle live in `webhook::worker`, the
//! HTTP intake in `webhook::intake`. All state transitions are guarded
//! UPDATEs enforced here; nothing else may mutate `webhook_deliveries`.
//!
//! Concurrency contract:
//! - **Claiming** re-evaluates the eligibility predicate inside a guarded
//!   per-row UPDATE (`status IN ('pending','retryable_failed') AND
//!   next_attempt_at <= now`): a competing worker's UPDATE matches zero rows.
//! - **Completion / failure recording** are fenced by the claim token: a
//!   worker that lost its lease matches zero rows and its whole processing
//!   transaction rolls back with it.
//! - **Lease recovery** (`recover_expired_leases`) returns crashed claims to
//!   `pending` WITHOUT burning an attempt — a crash between claim and
//!   execute must not consume retry budget.
//! - **Duplicate events**: the processing transaction's event insert and the
//!   fenced completion commit atomically; `UNIQUE(source_delivery_id)` on
//!   `domain_events` (migration 0017) is the final boundary, with the same
//!   ledger-style pre-check convergence the outbox uses.

use sqlx::Row;

use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::integrations::Provider;
use crate::util;

use super::{NORMALIZED_STATE_MAX_CHARS, sanitize_occurred_at};

/// How a failed processing attempt should be scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Transient: reschedule with bounded exponential backoff.
    Retryable,
    /// Invalid payload, unavailable adapter, or unsupported content:
    /// dead-letter immediately. Never retried.
    Permanent,
}

/// The outcome of recording one failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDisposition {
    /// Scheduled for another attempt after backoff.
    Rescheduled,
    /// Attempts exhausted, delivery age exceeded, or a permanent failure:
    /// terminal state.
    DeadLettered,
}

/// Why a delivery dead-lettered (bounded diagnostic categories, VARCHAR(40)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadLetterCategory {
    Permanent,
    Exhausted,
    Expired,
}

impl DeadLetterCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permanent => "permanent",
            Self::Exhausted => "exhausted",
            Self::Expired => "expired",
        }
    }
}

/// A delivery successfully claimed by a worker, with everything processing
/// needs: identity, provider, tenant scope, raw payload, and retry state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedDelivery {
    /// `webhook_deliveries.id` — the surrogate row id (`source_delivery_id`
    /// provenance target on the domain event).
    pub delivery_row_id: String,
    pub claim_token: String,
    pub integration_id: String,
    /// Team scope, resolved from the integration row (database truth — never
    /// caller-supplied) inside the claim query.
    pub team_id: String,
    pub provider: Provider,
    pub event_type_raw: String,
    pub payload: String,
    pub attempts: i64,
    pub created_at: i64,
}

/// Pure retry/terminal decision: given the upcoming attempt number, the
/// bounds, the delivery's age, and the failure classification, what happens.
/// Unit-tested; `record_failure` applies it under fencing.
#[must_use]
pub fn failure_disposition(
    attempts_after: i64,
    max_attempts: u32,
    delivery_age_ms: i64,
    max_delivery_age_ms: i64,
    class: FailureClass,
) -> FailureDisposition {
    if class == FailureClass::Permanent {
        return FailureDisposition::DeadLettered;
    }
    if attempts_after >= i64::from(max_attempts) {
        return FailureDisposition::DeadLettered;
    }
    if delivery_age_ms >= max_delivery_age_ms.max(1) {
        return FailureDisposition::DeadLettered;
    }
    FailureDisposition::Rescheduled
}

/// Classify a processing failure. Malformed payload shapes and vanished
/// references are permanent (they cannot become valid by waiting); database
/// unavailability and unknown shapes are retryable — safe because the failed
/// transaction rolled back atomically with any event insert, attempts burn
/// only when `record_failure` lands, and the provenance uniqueness makes a
/// mis-retried already-committed event converge via the pre-check.
/// `Conflict` here means a lost claim; `record_failure` is itself fenced and
/// matches zero rows in that case, so classification remains safe.
#[must_use]
pub fn classify_processing_failure(err: &AppError) -> FailureClass {
    use AppError as E;
    let text = err.to_string();
    let permanent_markers = [
        // The adapter for the delivery's provider is not registered: a
        // configuration regression that retrying cannot fix.
        "provider adapter unavailable",
        // Normalization produced a snapshot beyond the documented bound.
        "normalized state exceeds",
        // The integration row vanished between claim and process: corruption.
        "integration vanished",
    ];
    if matches!(err, E::BadRequest(_) | E::Validation(_) | E::NotFound(_))
        || permanent_markers.iter().any(|m| text.contains(m))
    {
        return FailureClass::Permanent;
    }
    FailureClass::Retryable
}

/// Recover deliveries whose lease expired while `processing`: back to
/// `pending`, claim fields cleared, **attempt count deliberately NOT
/// incremented** (a worker crash between claim and execute must not burn
/// retry budget). Returns the number of recovered rows.
pub async fn recover_expired_leases(conn: &mut Conn<'_>, now: i64) -> Result<u64, AppError> {
    let mut args = Args::new();
    args.i(now).i(now);
    let affected = conn
        .execute(
            "UPDATE webhook_deliveries SET status = 'pending', claim_token = NULL, \
             worker_id = NULL, claimed_at = NULL, lease_expires_at = NULL, updated_at = ? \
             WHERE status = 'processing' AND lease_expires_at < ?",
            args,
        )
        .await?;
    Ok(affected)
}

/// Claim up to `batch` eligible deliveries for one worker. Eligibility: due
/// (`next_attempt_at <= now`) and either fresh (`pending`) or a retry whose
/// backoff elapsed (`retryable_failed`). Accepted deliveries are processed
/// regardless of the integration's *current* enabled state: they were
/// authenticated and durably accepted at intake, so an operator disabling an
/// integration must not strand already-accepted deliveries (new deliveries
/// are refused at intake instead). Lease recovery is the caller's
/// responsibility (`run_once` performs it first with observability).
pub async fn claim_batch(
    conn: &mut Conn<'_>,
    worker_id: &str,
    batch: u32,
    lease_ms: i64,
    now: i64,
) -> Result<Vec<ClaimedDelivery>, AppError> {
    let mut args = Args::new();
    args.i(now).i(i64::from(batch));
    let candidates = conn
        .fetch_all(
            "SELECT wd.id FROM webhook_deliveries wd \
             WHERE wd.next_attempt_at <= ? \
             AND wd.status IN ('pending', 'retryable_failed') \
             ORDER BY wd.next_attempt_at, wd.id LIMIT ?",
            args,
        )
        .await?;
    let mut claimed = Vec::new();
    for row in candidates {
        let delivery_row_id: String = row.try_get("id")?;
        let token = crate::security::token::generate();
        // Seven placeholders, seven binds: SET (5) + id + the due predicate.
        let mut args = Args::new();
        args.s(&token)
            .s(worker_id)
            .i(now)
            .i(now.saturating_add(lease_ms))
            .i(now)
            .s(&delivery_row_id)
            .i(now);
        let affected = conn
            .execute(
                "UPDATE webhook_deliveries SET status = 'processing', claim_token = ?, \
                 worker_id = ?, claimed_at = ?, lease_expires_at = ?, updated_at = ? \
                 WHERE id = ? AND next_attempt_at <= ? \
                 AND status IN ('pending', 'retryable_failed')",
                args,
            )
            .await?;
        if affected != 1 {
            continue;
        }
        // Re-read with the tenant/provider scope from the integration row.
        let mut args = Args::new();
        args.s(&delivery_row_id);
        let row = conn
            .fetch_optional(
                "SELECT wd.integration_id, wd.event_type_raw, wd.payload, wd.attempts, \
                        wd.created_at, i.team_id, i.provider \
                 FROM webhook_deliveries wd \
                 JOIN integrations i ON i.id = wd.integration_id \
                 WHERE wd.id = ?",
                args,
            )
            .await?
            .ok_or_else(|| AppError::internal("claimed delivery has no integration row"))?;
        let provider_str: String = row.try_get("provider")?;
        let Some(provider) = Provider::parse(&provider_str) else {
            // CHECK-constrained data cannot reach here; a corrupted row
            // must not loop through lease recovery forever — dead-letter
            // it under the claim we hold.
            let mut args = Args::new();
            args.i(now)
                .i(now)
                .s("provider adapter unavailable: corrupted provider value")
                .s(&delivery_row_id)
                .s(&token);
            conn.execute(
                "UPDATE webhook_deliveries SET status = 'dead_letter', \
                 claim_token = NULL, completed_at = ?, updated_at = ?, \
                 last_error_category = 'permanent', last_error_detail = ? \
                 WHERE id = ? AND claim_token = ?",
                args,
            )
            .await?;
            continue;
        };
        claimed.push(ClaimedDelivery {
            delivery_row_id,
            claim_token: token,
            integration_id: row.try_get("integration_id")?,
            team_id: row.try_get("team_id")?,
            provider,
            event_type_raw: row.try_get("event_type_raw")?,
            payload: row.try_get("payload")?,
            attempts: row.try_get("attempts")?,
            created_at: row.try_get("created_at")?,
        });
    }
    Ok(claimed)
}

/// Fenced success transition. A stale worker's token matches nothing and the
/// caller's transaction (event insert included) rolls back.
pub async fn complete_processed(
    conn: &mut Conn<'_>,
    delivery_row_id: &str,
    claim_token: &str,
    now: i64,
) -> Result<(), AppError> {
    let mut args = Args::new();
    args.i(now).i(now).s(delivery_row_id).s(claim_token);
    let affected = conn
        .execute(
            "UPDATE webhook_deliveries SET status = 'processed', claim_token = NULL, \
             completed_at = ?, updated_at = ? \
             WHERE id = ? AND status = 'processing' AND claim_token = ?",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::Conflict(
            "webhook delivery claim is no longer owned by this worker".into(),
        ));
    }
    Ok(())
}

/// Fenced terminal transition for events outside the supported vocabulary:
/// durably recorded, never a domain event, never retried.
pub async fn complete_unsupported(
    conn: &mut Conn<'_>,
    delivery_row_id: &str,
    claim_token: &str,
    reason: &str,
    now: i64,
) -> Result<(), AppError> {
    let detail = util::truncate(reason, 500);
    let mut args = Args::new();
    args.i(now)
        .i(now)
        .s(&detail)
        .s(delivery_row_id)
        .s(claim_token);
    let affected = conn
        .execute(
            "UPDATE webhook_deliveries SET status = 'unsupported', claim_token = NULL, \
             completed_at = ?, updated_at = ?, \
             last_error_category = 'unsupported_event', last_error_detail = ? \
             WHERE id = ? AND status = 'processing' AND claim_token = ?",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::Conflict(
            "webhook delivery claim is no longer owned by this worker".into(),
        ));
    }
    Ok(())
}

/// Record a failure on a claimed delivery, fenced by its claim token:
/// rescheduled failures get `retryable_failed` + backoff and return to the
/// eligibility queue; permanent failures, exhausted attempts, and deliveries
/// beyond the maximum age get `dead_letter` with a bounded category. The
/// attempt count is incremented exactly here (failures burn budget — crashes
/// between claim and execute do not; see `recover_expired_leases`).
#[allow(clippy::too_many_arguments)] // explicit knobs mirror WebhookWorkerConfig
pub async fn record_failure(
    conn: &mut Conn<'_>,
    claimed: &ClaimedDelivery,
    class: FailureClass,
    error: &AppError,
    max_attempts: u32,
    backoff_base_ms: i64,
    backoff_max_ms: i64,
    jitter: i64,
    max_delivery_age_ms: i64,
    now: i64,
) -> Result<FailureDisposition, AppError> {
    let attempts = claimed.attempts + 1;
    let age = now.saturating_sub(claimed.created_at);
    // Backoff reuses the Phase 8A pure function (identical policy, one
    // implementation — no duplicated infrastructure).
    let disposition = failure_disposition(attempts, max_attempts, age, max_delivery_age_ms, class);
    let (status, category, next_attempt_at) = match disposition {
        FailureDisposition::DeadLettered => {
            let category = match class {
                FailureClass::Permanent => DeadLetterCategory::Permanent,
                FailureClass::Retryable if age >= max_delivery_age_ms.max(1) => {
                    DeadLetterCategory::Expired
                }
                FailureClass::Retryable => DeadLetterCategory::Exhausted,
            };
            ("dead_letter", category.as_str(), now)
        }
        FailureDisposition::Rescheduled => {
            let delay = backoff_delay_ms(attempts, backoff_base_ms, backoff_max_ms, jitter);
            ("retryable_failed", "retryable", now + delay)
        }
    };
    // Bounded, sanitized diagnostics: category enum + truncated message. The
    // error text comes from our own validation/DB layers and never carries
    // payloads or secrets, but is truncated to the schema bound anyway.
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
        .s(&claimed.delivery_row_id)
        .s(&claimed.claim_token);
    let affected = conn
        .execute(
            "UPDATE webhook_deliveries SET status = ?, attempts = ?, \
             last_error_category = ?, last_error_detail = ?, next_attempt_at = ?, \
             claim_token = NULL, worker_id = NULL, claimed_at = NULL, \
             lease_expires_at = NULL, updated_at = ? \
             WHERE id = ? AND status = 'processing' AND claim_token = ?",
            args,
        )
        .await?;
    if affected == 0 {
        // The claim was lost (reaped + re-claimed): the failure belongs to a
        // dead attempt. The current owner owns the outcome.
        return Err(AppError::Conflict(
            "webhook delivery claim is no longer owned by this worker".into(),
        ));
    }
    Ok(disposition)
}

/// Whether a domain event already exists for this delivery. Called inside
/// the processing transaction before the insert; the
/// `UNIQUE(source_delivery_id)` index (migration 0017) is the constraint
/// backstop against a racing duplicate.
pub async fn domain_event_exists(
    conn: &mut Conn<'_>,
    delivery_row_id: &str,
) -> Result<bool, AppError> {
    let mut args = Args::new();
    args.s(delivery_row_id);
    Ok(conn
        .fetch_optional(
            "SELECT id FROM domain_events WHERE source_delivery_id = ?",
            args,
        )
        .await?
        .is_some())
}

/// Persist the normalized integration event with full provenance. **Must run
/// inside the processing transaction** together with the fenced delivery
/// completion: a crash before commit leaves nothing, after commit both the
/// event and the `processed` delivery exist. Taskless by design
/// (`task_id NULL`), external-actor-safe (`actor_id NULL` — external actors
/// never become Tasker users), and traceable to its delivery.
pub async fn insert_integration_domain_event(
    conn: &mut Conn<'_>,
    claimed: &ClaimedDelivery,
    event_type: crate::rules::events::IntegrationTriggerType,
    occurred_at: i64,
    state_json: &str,
) -> Result<(), AppError> {
    if state_json.chars().count() > NORMALIZED_STATE_MAX_CHARS {
        return Err(AppError::internal(
            "normalized state exceeds the documented snapshot bound",
        ));
    }
    // Placeholders in order (13): id, event_type, schema_version, team_id,
    // task_id, actor_id, previous_state, new_state, origin,
    // causing_execution_id, occurred_at, integration_id, source_delivery_id.
    let mut args = Args::new();
    args.s(util::new_id())
        .s(event_type.as_str())
        .i(1i64)
        .s(&claimed.team_id)
        .opt_i(None::<i64>) // task_id: integration events are taskless
        .opt_i(None::<i64>) // actor_id: external actors never impersonate
        .opt_s(None::<String>) // previous_state: no prior Tasker state
        .opt_s(Some(state_json))
        .s("integration")
        .opt_i(None::<i64>) // causing_execution_id
        .i(sanitize_occurred_at(occurred_at, util::now_ms()))
        .opt_s(Some(claimed.integration_id.clone()))
        .opt_s(Some(claimed.delivery_row_id.clone()));
    conn.execute(
        "INSERT INTO domain_events (id, event_type, schema_version, team_id, task_id, \
         actor_id, previous_state, new_state, origin, causing_execution_id, \
         occurred_at, integration_id, source_delivery_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    Ok(())
}

/// Replay a dead-lettered delivery back into the queue. **Must run inside
/// the caller's transaction.** Guarded by `status = 'dead_letter'`: a
/// concurrent replay loses the CAS and returns `Ok(false)`; terminal states
/// other than `dead_letter` are never reopened; the delivery's diagnostics
/// at replay time are snapshotted into an append-only
/// `webhook_delivery_replays` audit row (who, when, why) so reopening never
/// destroys history. Attempts reset to zero — the replayed delivery gets a
/// full retry budget.
pub async fn replay_dead_letter(
    conn: &mut Conn<'_>,
    delivery_row_id: &str,
    replayed_by: &str,
    reason: Option<&str>,
    now: i64,
) -> Result<bool, AppError> {
    // Snapshot the pre-replay diagnostics inside the same transaction.
    let mut args = Args::new();
    args.s(delivery_row_id);
    let prior = conn
        .fetch_optional(
            "SELECT status, attempts, last_error_category, last_error_detail \
             FROM webhook_deliveries WHERE id = ?",
            args,
        )
        .await?
        .ok_or(AppError::NotFound("delivery not found"))?;
    let prior_status: String = prior.try_get("status")?;
    let prior_attempts: i64 = prior.try_get("attempts")?;
    let prior_error_category: Option<String> = prior.try_get("last_error_category").ok();
    let prior_error_detail: Option<String> = prior.try_get("last_error_detail").ok();
    if prior_status != "dead_letter" {
        return Ok(false);
    }
    // CAS reopen: only from `dead_letter`; a concurrent replay matches zero.
    let mut args = Args::new();
    args.i(now).i(now).s(delivery_row_id);
    let affected = conn
        .execute(
            "UPDATE webhook_deliveries SET status = 'pending', attempts = 0, \
             next_attempt_at = ?, claim_token = NULL, worker_id = NULL, \
             claimed_at = NULL, lease_expires_at = NULL, completed_at = NULL, \
             last_error_category = NULL, last_error_detail = NULL, updated_at = ? \
             WHERE id = ? AND status = 'dead_letter'",
            args,
        )
        .await?;
    if affected == 0 {
        return Ok(false);
    }
    let category = prior_error_category
        .as_deref()
        .map(|c| util::truncate(c, 40));
    let detail = prior_error_detail
        .as_deref()
        .map(|d| util::truncate(d, 500));
    let reason = reason.map(|r| util::truncate(r, 500));
    let mut args = Args::new();
    args.s(util::new_id())
        .s(delivery_row_id)
        .s(replayed_by)
        .i(prior_attempts)
        .opt_s(category)
        .opt_s(detail)
        .opt_s(reason)
        .i(now);
    conn.execute(
        "INSERT INTO webhook_delivery_replays (id, delivery_id, replayed_by, \
         prior_attempts, prior_error_category, prior_error_detail, reason, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    Ok(true)
}

/// Full-jitter backoff: imported from the Phase 8A outbox policy so both
/// dispatchers share one implementation (doubling, capping, jitter bounds,
/// and overflow behavior are unit-tested there).
use crate::rules::outbox::backoff_delay_ms;

#[cfg(test)]
mod tests {
    use super::*;

    const AGE_BOUND: i64 = 7 * 24 * 60 * 60 * 1000; // 7 days in ms

    #[test]
    fn disposition_is_permanent_immediately() {
        assert_eq!(
            failure_disposition(1, 8, 0, AGE_BOUND, FailureClass::Permanent),
            FailureDisposition::DeadLettered
        );
    }

    #[test]
    fn disposition_reschedules_within_bounds() {
        assert_eq!(
            failure_disposition(1, 8, 0, AGE_BOUND, FailureClass::Retryable),
            FailureDisposition::Rescheduled
        );
        assert_eq!(
            failure_disposition(7, 8, AGE_BOUND - 1, AGE_BOUND, FailureClass::Retryable),
            FailureDisposition::Rescheduled
        );
    }

    #[test]
    fn disposition_exhausts_attempts() {
        assert_eq!(
            failure_disposition(8, 8, 0, AGE_BOUND, FailureClass::Retryable),
            FailureDisposition::DeadLettered
        );
        assert_eq!(
            failure_disposition(9, 8, 0, AGE_BOUND, FailureClass::Retryable),
            FailureDisposition::DeadLettered
        );
    }

    #[test]
    fn disposition_expires_old_deliveries() {
        assert_eq!(
            failure_disposition(2, 8, AGE_BOUND, AGE_BOUND, FailureClass::Retryable),
            FailureDisposition::DeadLettered
        );
        assert_eq!(
            failure_disposition(2, 8, AGE_BOUND + 1, AGE_BOUND, FailureClass::Retryable),
            FailureDisposition::DeadLettered
        );
    }

    #[test]
    fn classification_marks_payload_and_config_failures_permanent() {
        let permanent = [
            AppError::BadRequest("malformed".into()),
            AppError::Validation(vec![crate::error::FieldError::new("payload", "bad")]),
            AppError::NotFound("integration vanished"),
            AppError::internal("provider adapter unavailable for delivery"),
            AppError::internal("normalized state exceeds the documented snapshot bound"),
        ];
        for err in &permanent {
            assert_eq!(
                classify_processing_failure(err),
                FailureClass::Permanent,
                "{err}"
            );
        }
    }

    #[test]
    fn classification_policy_for_unknown_and_transient_errors() {
        // Documented unknown-failure policy: unrecognized shapes are
        // Retryable — the processing transaction rolled back atomically,
        // attempts burn only when the fenced record lands, and the
        // provenance uniqueness converges any mis-retried committed event.
        let retryable = [
            AppError::Busy {
                retry_after_secs: 1,
            },
            AppError::internal("database temporarily unavailable"),
            AppError::Conflict("completed concurrently".into()),
            AppError::internal("UNIQUE constraint failed: domain_events.source_delivery_id"),
        ];
        for err in &retryable {
            assert_eq!(
                classify_processing_failure(err),
                FailureClass::Retryable,
                "{err}"
            );
        }
    }

    #[test]
    fn shared_backoff_policy_doubles_and_caps() {
        // The webhook path delegates to the Phase 8A pure function; assert a
        // few anchors to prove the delegation is live.
        assert_eq!(backoff_delay_ms(1, 1_000, 60_000, 1_000), 1_000);
        assert_eq!(backoff_delay_ms(3, 1_000, 60_000, 1_000), 4_000);
        assert_eq!(backoff_delay_ms(20, 1_000, 60_000, 1_000), 60_000);
    }

    #[test]
    fn occurred_at_validation_replaces_implausible_values() {
        let now = 1_700_000_000_000;
        assert_eq!(super::sanitize_occurred_at(now - 1, now), now - 1);
        assert_eq!(super::sanitize_occurred_at(now + 61_000, now), now);
        assert_eq!(super::sanitize_occurred_at(0, now), now);
    }
}
