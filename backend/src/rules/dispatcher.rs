//! Outbox dispatcher (Phase 8A, Task 5): polling loop, claim cycle, internal
//! action dispatch, retry scheduling, and graceful lifecycle.
//!
//! Architecture: `rules::outbox` is the repository boundary (claim, recover,
//! execute, record failure); this module owns the loop and lifecycle only.
//! One poll cycle (`run_once`) is independently callable so the dispatcher is
//! testable without the HTTP server or wall-clock sleeps.
//!
//! Transaction ownership per intent: claim (own transaction, committed) →
//! open a NEW transaction → `outbox::execute_internal_action(tx, id,
//! Some(token))` (effect + ledger + fenced `delivered` completion) → commit.
//! No transaction is held across anything external; internal actions are
//! database-local so the execution transaction is short by construction.
//! `send_message` is never claimed — it stays `pending` for the future
//! external dispatcher (handoff documented in `docs/rules-engine.md`).
//!
//! Failure handling: `outbox::classify_failure` sorts errors into
//! Retryable (bounded exponential backoff with full jitter, bounded by
//! `max_attempts`) and Permanent (immediate `dead_letter`). Failures are
//! recorded through a claim-token-fenced update, so a stale worker cannot
//! schedule anything. Crash recovery: `claim_batch` reaps expired leases
//! first; reaped attempts do not burn retry budget.

use std::time::Duration;

use std::sync::Arc;
use tokio::sync::{Semaphore, watch};

use crate::config::RulesWorkerConfig;
use crate::db::Database;
use crate::http::metrics::{Metrics, OutboxEvent};
use crate::util;

use super::outbox::{ClaimedIntent, classify_failure, execute_internal_action, record_failure};

/// One dispatcher poll cycle: recover expired leases, claim a batch of due
/// internal-action intents, execute each within its own bounded transaction,
/// and record outcomes. Returns the number of intents claimed. Independent
/// of the polling loop — tests drive cycles directly.
///
/// # Errors
/// Propagates claim/lease-recovery database failures; per-intent execution
/// failures are classified and recorded (never propagated — a failing intent
/// must not starve the rest of the batch).
pub async fn run_once(
    db: &Database,
    config: &RulesWorkerConfig,
    worker_id: &str,
    metrics: &Metrics,
    semaphore: &Arc<Semaphore>,
) -> Result<usize, crate::error::AppError> {
    let now = util::now_ms();
    // Recover crashed claims before claiming, with explicit observability:
    // a persistent nonzero recovery count is the crash-loop signal.
    let recovered = {
        let mut conn = db.conn();
        super::outbox::recover_expired_leases(&mut conn, now).await?
    };
    if recovered > 0 {
        metrics.observe_outbox(OutboxEvent::LeaseRecovered(recovered));
        tracing::warn!(
            worker_id,
            recovered = recovered,
            "recovered outbox intents with expired leases"
        );
    }
    let claimed = {
        let mut conn = db.conn();
        super::outbox::claim_batch(
            &mut conn,
            worker_id,
            config.batch_size,
            i64::try_from(config.lease_ms).unwrap_or(60_000),
            now,
        )
        .await?
    };
    if claimed.is_empty() {
        return Ok(0);
    }
    metrics.observe_outbox(OutboxEvent::Claimed(
        u64::try_from(claimed.len()).unwrap_or(u64::MAX),
    ));
    tracing::info!(
        worker_id,
        claimed = claimed.len(),
        "dispatcher claimed outbox intents"
    );

    let mut join_set = tokio::task::JoinSet::new();
    for intent in claimed {
        // Bounded concurrency: acquire before spawning; the permit is held
        // for the whole execution.
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| crate::error::AppError::internal("dispatcher semaphore closed"))?;
        let db = db.clone();
        let metrics = metrics.clone();
        let config = config.clone();
        let worker_id = worker_id.to_string();
        join_set.spawn(async move {
            execute_claimed(db, config, worker_id, metrics, intent).await;
            drop(permit);
        });
    }
    // In-flight work finishes within this cycle; shutdown semantics live in
    // [`spawn`], which stops claiming and then drains.
    while join_set.join_next().await.is_some() {}
    Ok(0)
}

/// Execute (or schedule the failure of) one claimed intent.
async fn execute_claimed(
    db: Database,
    config: RulesWorkerConfig,
    worker_id: String,
    metrics: Metrics,
    intent: ClaimedIntent,
) {
    let started = std::time::Instant::now();
    let mut tx = match db.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            // The claim transaction could not even start: treat as retryable
            // through the same fenced path (the claim is still ours).
            record_failure_and_observe(
                &db,
                &config,
                &worker_id,
                &metrics,
                &intent,
                classify_failure(&err),
                &err,
            )
            .await;
            return;
        }
    };
    match execute_internal_action(&mut tx, &intent.outbox_id, Some(&intent.claim_token)).await {
        Ok(super::outbox::ExecutionOutcome::Completed) => {
            let commit_result = tx.commit().await;
            if let Err(commit_err) = commit_result {
                // Commit failure: the transaction rolled back - but we cannot
                // know that from here. Route through the fenced failure
                // recorder: if the commit truly failed, the retry is
                // scheduled and the attempt burns; if it actually landed
                // (delivered, token cleared), the fenced update matches zero
                // rows and the ledger pre-check converges on the next claim.
                // Either way a deterministic commit failure cannot strand the
                // record in a retry loop.
                record_failure_and_observe(
                    &db,
                    &config,
                    &worker_id,
                    &metrics,
                    &intent,
                    classify_failure(&commit_err),
                    &commit_err,
                )
                .await;
                return;
            }
            metrics.observe_outbox(OutboxEvent::Executed {
                duration_ms: started.elapsed().as_secs_f64() * 1000.0,
            });
            tracing::info!(
                worker_id,
                outbox_id = %intent.outbox_id,
                action_type = %intent.action_type,
                "automation action executed"
            );
        }
        Ok(super::outbox::ExecutionOutcome::AlreadyExecuted) => {
            // Converged through the ledger: complete the fence we still own
            // (the effect was committed by an earlier attempt that died
            // before completing the row) and observe.
            let now = util::now_ms();
            let complete = super::outbox::complete_intent(
                &mut tx,
                &intent.outbox_id,
                Some(&intent.claim_token),
                now,
            )
            .await;
            match complete {
                Ok(()) => {
                    if tx.commit().await.is_ok() {
                        metrics.observe_outbox(OutboxEvent::AlreadyExecuted);
                        tracing::info!(
                            worker_id,
                            outbox_id = %intent.outbox_id,
                            "duplicate execution converged via ledger"
                        );
                        return;
                    }
                }
                Err(err) => {
                    // Stale claim or concurrent completion: roll back; the
                    // current owner owns the row.
                    tx.rollback().await;
                    metrics.observe_outbox(OutboxEvent::StaleRejected);
                    tracing::warn!(
                        worker_id,
                        outbox_id = %intent.outbox_id,
                        error = %err,
                        "duplicate execution hit a fenced or completed intent"
                    );
                    return;
                }
            }
            metrics.observe_outbox(OutboxEvent::RetryableFailed);
        }
        Ok(super::outbox::ExecutionOutcome::ExternalPending) => {
            // Unreachable: external intents are never claimed. Observe and
            // release defensively without consuming retry budget.
            tx.rollback().await;
            metrics.observe_outbox(OutboxEvent::ExternalPending);
        }
        Err(err) => {
            tx.rollback().await;
            let class = classify_failure(&err);
            record_failure_and_observe(&db, &config, &worker_id, &metrics, &intent, class, &err)
                .await;
        }
    }
}

/// Record a classified failure on the fenced claim and observe it.
async fn record_failure_and_observe(
    db: &Database,
    config: &RulesWorkerConfig,
    worker_id: &str,
    metrics: &Metrics,
    intent: &ClaimedIntent,
    class: super::outbox::FailureClass,
    err: &crate::error::AppError,
) {
    let mut conn = db.conn();
    let jitter = rand_jitter();
    match record_failure(
        &mut conn,
        intent,
        class,
        err,
        config.max_attempts,
        i64::try_from(config.backoff_base_ms).unwrap_or(1_000),
        i64::try_from(config.backoff_max_ms).unwrap_or(60_000),
        jitter,
        util::now_ms(),
    )
    .await
    {
        Ok(super::outbox::FailureDisposition::Rescheduled) => {
            metrics.observe_outbox(OutboxEvent::RetryableFailed);
            tracing::warn!(
                worker_id,
                outbox_id = %intent.outbox_id,
                attempt = intent.attempts + 1,
                classification = "retryable",
                "automation action failed; rescheduled with backoff"
            );
        }
        Ok(super::outbox::FailureDisposition::DeadLettered) => {
            metrics.observe_outbox(OutboxEvent::DeadLettered);
            tracing::error!(
                worker_id,
                outbox_id = %intent.outbox_id,
                attempt = intent.attempts + 1,
                classification = if matches!(class, super::outbox::FailureClass::Permanent) {
                    "permanent"
                } else {
                    "exhausted"
                },
                "automation action dead-lettered"
            );
        }
        Err(lost @ crate::error::AppError::Conflict(_)) => {
            // The claim was lost (reaped + re-claimed): observe a stale
            // rejection. The current owner owns the outcome.
            metrics.observe_outbox(OutboxEvent::StaleRejected);
            tracing::warn!(
                worker_id,
                outbox_id = %intent.outbox_id,
                error = %lost,
                "failure recording lost its claim"
            );
        }
        Err(db_err) => {
            // The failure record itself could not be written (infrastructure
            // down): the row stays `processing`, the lease expires, and the
            // intent is retried without burning an attempt.
            tracing::error!(
                worker_id,
                outbox_id = %intent.outbox_id,
                error = %db_err,
                "could not persist automation failure record; lease recovery will re-queue"
            );
        }
    }
}

/// Process-local full-jitter source in `[0, 1000]`. Determinism is not
/// required here; bounds are (verified by unit tests on the pure function).
fn rand_jitter() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0u64, |d| u64::from(d.subsec_nanos()));
    // nanos < 10^6, far below i64::MAX
    #[allow(clippy::cast_possible_wrap)]
    let jitter = (nanos % 1001) as i64;
    jitter
}

/// Run the dispatcher until shutdown. Claims and executes in cycles; stops
/// claiming as soon as shutdown is signalled and drains in-flight work.
/// Claiming never blocks the runtime: every cycle is bounded by `batch_size`
/// and `max_concurrent`.
pub async fn run_until_shutdown(
    db: Database,
    config: RulesWorkerConfig,
    metrics: Metrics,
    mut shutdown: watch::Receiver<bool>,
) {
    let worker_id = util::new_id();
    let semaphore = Arc::new(Semaphore::new(
        usize::try_from(config.max_concurrent.max(1)).unwrap_or(1),
    ));
    tracing::info!(
        worker_id,
        poll_interval_ms = config.poll_interval_ms,
        batch_size = config.batch_size,
        max_concurrent = config.max_concurrent,
        lease_ms = config.lease_ms,
        "outbox dispatcher started"
    );
    let mut ticker = tokio::time::interval(Duration::from_millis(
        config.poll_interval_ms.clamp(50, 3_600_000),
    ));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // First tick fires immediately: drain any backlog right after boot.
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if *shutdown.borrow() {
                    break;
                }
                if run_once(&db, &config, &worker_id, &metrics, &semaphore)
                    .await
                    .is_err()
                {
                    // Claim-cycle DB failure: keep polling; the next cycle
                    // retries recovery. Back off by one interval implicitly.
                    tracing::warn!(worker_id, "dispatcher claim cycle failed; will retry");
                }
            }
            _ = shutdown.changed() => {
                break;
            }
        }
    }
    // Drain: in-flight executions (spawned in run_once's JoinSet) have
    // already completed because run_once awaits its own set before
    // returning; nothing else is in flight here.
    tracing::info!(
        worker_id,
        "outbox dispatcher stopped; no claims accepted after shutdown"
    );
}

/// Spawn the dispatcher as a tracked task wired to the server's shutdown
/// signal. Returns the join handle so main can await a clean stop.
#[must_use]
pub fn spawn(
    db: Database,
    config: RulesWorkerConfig,
    metrics: Metrics,
    shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_until_shutdown(db, config, metrics, shutdown))
}
