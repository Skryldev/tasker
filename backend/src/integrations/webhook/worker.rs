//! Webhook delivery worker (Phase 8B, Task 2): polling loop, claim cycle,
//! provider-neutral processing, and graceful lifecycle — the webhook
//! counterpart of `rules::dispatcher`, sharing its proven lease/fencing
//! architecture.
//!
//! Processing atomicity: the normalized domain event insert and the fenced
//! `processed` completion commit in ONE transaction. A crash before commit
//! leaves nothing (lease recovery re-queues without burning an attempt);
//! after commit both exist. `UNIQUE(source_delivery_id)` plus the ledger-
//! style pre-check converge duplicate executions to a single domain event.
//!
//! Transaction ownership per intent: claim (committed) → open a NEW
//! transaction → normalize (pure) + event insert + fenced completion →
//! commit. No transaction is held across anything external; normalization is
//! database-local so the transaction is short by construction.
//!
//! Failure handling: `delivery::classify_processing_failure` sorts errors
//! into Retryable (bounded exponential backoff with full jitter, bounded by
//! `max_attempts` and `max_delivery_age_ms`) and Permanent (immediate
//! `dead_letter`). Failures are recorded through a claim-token-fenced update,
//! so a stale worker cannot schedule anything.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, watch};

use crate::config::WebhookWorkerConfig;
use crate::db::Database;
use crate::error::AppError;
use crate::http::metrics::{Metrics, WebhookEvent};
use crate::integrations::webhook::delivery::{self, ClaimedDelivery, FailureClass};
use crate::integrations::webhook::{
    NORMALIZED_STATE_MAX_CHARS, NormalizedOutcome, ProviderRegistry, sanitize_occurred_at,
};
use crate::util;

/// One worker poll cycle: recover expired leases, claim a batch of due
/// deliveries, process each within its own bounded transaction, and record
/// outcomes. Returns the number of deliveries claimed. Independent of the
/// polling loop — tests drive cycles directly.
///
/// # Errors
/// Propagates claim/lease-recovery database failures; per-delivery
/// processing failures are classified and recorded (never propagated — a
/// failing delivery must not starve the rest of the batch).
pub async fn run_once(
    db: &Database,
    registry: &ProviderRegistry,
    config: &WebhookWorkerConfig,
    worker_id: &str,
    metrics: &Metrics,
    semaphore: &Arc<Semaphore>,
) -> Result<usize, AppError> {
    let now = util::now_ms();
    // Recover crashed claims before claiming, with explicit observability:
    // a persistent nonzero recovery count is the crash-loop signal.
    let recovered = {
        let mut conn = db.conn();
        delivery::recover_expired_leases(&mut conn, now).await?
    };
    if recovered > 0 {
        metrics.observe_webhook(WebhookEvent::LeaseRecovered(recovered));
        tracing::warn!(
            worker_id,
            recovered = recovered,
            "recovered webhook deliveries with expired leases"
        );
    }
    let claimed = {
        let mut conn = db.conn();
        delivery::claim_batch(
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
    metrics.observe_webhook(WebhookEvent::Claimed(
        u64::try_from(claimed.len()).unwrap_or(u64::MAX),
    ));
    tracing::info!(
        worker_id,
        claimed = claimed.len(),
        "worker claimed webhook deliveries"
    );

    let mut join_set = tokio::task::JoinSet::new();
    let claimed_count = claimed.len();
    for claimed_delivery in claimed {
        // Bounded concurrency: acquire before spawning; the permit is held
        // for the whole processing.
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::internal("worker semaphore closed"))?;
        let db = db.clone();
        let metrics = metrics.clone();
        let config = config.clone();
        let registry = registry.clone();
        let worker_id = worker_id.to_string();
        join_set.spawn(async move {
            process_claimed(
                &db,
                &registry,
                &config,
                &worker_id,
                &metrics,
                claimed_delivery,
            )
            .await;
            drop(permit);
        });
    }
    // In-flight work finishes within this cycle; shutdown semantics live in
    // [`run_until_shutdown`], which stops claiming and then drains.
    while join_set.join_next().await.is_some() {}
    // Unlike the Phase 8A dispatcher (whose return value only reports the
    // empty-cycle case), the webhook worker reports the actual claim count:
    // tests and the benchmark assert on it.
    Ok(claimed_count)
}

/// Process (or schedule the failure of) one claimed delivery.
async fn process_claimed(
    db: &Database,
    registry: &ProviderRegistry,
    config: &WebhookWorkerConfig,
    worker_id: &str,
    metrics: &Metrics,
    delivery_claim: ClaimedDelivery,
) {
    let started = Instant::now();
    let Some(adapter) = registry.get(delivery_claim.provider) else {
        // The adapter was unregistered since intake: a configuration
        // regression retrying cannot fix. Dead-letter under the claim.
        record_failure_and_observe(
            db,
            config,
            worker_id,
            metrics,
            &delivery_claim,
            FailureClass::Permanent,
            &AppError::internal("provider adapter unavailable for this delivery's provider"),
        )
        .await;
        return;
    };

    // Normalization is pure: it runs before the transaction and cannot touch
    // the database, so a malformed payload never opens one.
    let outcome = adapter.normalize(&delivery_claim.event_type_raw, &delivery_claim.payload);

    match outcome {
        NormalizedOutcome::Event {
            event_type,
            occurred_at,
            state,
        } => {
            let state_json = match serde_json::to_string(&state) {
                Ok(json) if json.chars().count() <= NORMALIZED_STATE_MAX_CHARS => json,
                Ok(_) => {
                    tx_rollback_record(
                        db,
                        config,
                        worker_id,
                        metrics,
                        delivery_claim,
                        FailureClass::Permanent,
                        &AppError::internal(
                            "normalized state exceeds the documented snapshot bound",
                        ),
                    )
                    .await;
                    return;
                }
                Err(err) => {
                    tx_rollback_record(
                        db,
                        config,
                        worker_id,
                        metrics,
                        delivery_claim,
                        FailureClass::Permanent,
                        &AppError::internal(format!("normalized state is not serializable: {err}")),
                    )
                    .await;
                    return;
                }
            };
            let occurred_at = sanitize_occurred_at(occurred_at, util::now_ms());
            process_event(
                db,
                config,
                worker_id,
                metrics,
                delivery_claim,
                event_type,
                occurred_at,
                &state_json,
                &state,
                started,
            )
            .await;
        }
        NormalizedOutcome::Unsupported { reason } => {
            process_unsupported(db, worker_id, metrics, delivery_claim, reason).await;
        }
        NormalizedOutcome::Invalid { reason } => {
            record_failure_and_observe(
                db,
                config,
                worker_id,
                metrics,
                &delivery_claim,
                FailureClass::Permanent,
                &AppError::BadRequest(reason.to_string()),
            )
            .await;
        }
    }
}

/// Persist the normalized event + fenced completion in one transaction.
#[allow(clippy::too_many_arguments)]
async fn process_event(
    db: &Database,
    config: &WebhookWorkerConfig,
    worker_id: &str,
    metrics: &Metrics,
    delivery_claim: ClaimedDelivery,
    event_type: crate::rules::events::IntegrationTriggerType,
    occurred_at: i64,
    state_json: &str,
    state: &serde_json::Value,
    started: Instant,
) {
    let mut tx = match db.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            record_failure_and_observe(
                db,
                config,
                worker_id,
                metrics,
                &delivery_claim,
                delivery::classify_processing_failure(&err),
                &err,
            )
            .await;
            return;
        }
    };

    // Ledger-style pre-check: one domain event per delivery. The
    // UNIQUE(source_delivery_id) constraint remains the concurrency defense.
    let already =
        match delivery::domain_event_exists(&mut tx, &delivery_claim.delivery_row_id).await {
            Ok(already) => already,
            Err(err) => {
                tx.rollback().await;
                record_failure_and_observe(
                    db,
                    config,
                    worker_id,
                    metrics,
                    &delivery_claim,
                    delivery::classify_processing_failure(&err),
                    &err,
                )
                .await;
                return;
            }
        };

    if already {
        // Converged through provenance: a prior attempt committed the event
        // (and, being atomic, its completion). Complete the fence we still
        // own and observe.
        let complete = delivery::complete_processed(
            &mut tx,
            &delivery_claim.delivery_row_id,
            &delivery_claim.claim_token,
            util::now_ms(),
        )
        .await;
        match complete {
            Ok(()) => {
                // Commit consumes the transaction; a failed commit has
                // already closed it, so no rollback is possible (or needed).
                // The fenced failure recorder converges whichever world is
                // true (landed → stale rejection; not landed → retry).
                match tx.commit().await {
                    Ok(()) => {
                        metrics.observe_webhook(WebhookEvent::AlreadyProcessed);
                        tracing::info!(
                            worker_id,
                            delivery_id = %delivery_claim.delivery_row_id,
                            "duplicate processing converged via provenance uniqueness"
                        );
                        return;
                    }
                    Err(commit_err) => {
                        record_failure_and_observe(
                            db,
                            config,
                            worker_id,
                            metrics,
                            &delivery_claim,
                            delivery::classify_processing_failure(&commit_err),
                            &commit_err,
                        )
                        .await;
                        return;
                    }
                }
            }
            Err(err) => {
                tx.rollback().await;
                handle_post_rollback(db, config, worker_id, metrics, delivery_claim, err).await;
                return;
            }
        }
    }

    let event_id = match delivery::insert_integration_domain_event(
        &mut tx,
        &delivery_claim,
        event_type,
        occurred_at,
        state_json,
    )
    .await
    {
        Ok(event_id) => event_id,
        Err(err) => {
            tx.rollback().await;
            handle_post_rollback(db, config, worker_id, metrics, delivery_claim, err).await;
            return;
        }
    };

    // Shared Rules Engine evaluation (Phase 8B, Task 5), in the SAME
    // transaction: matched integration rules persist executions and action
    // intents atomically with the event and the delivery completion. The
    // snapshot value carries the integration.* condition fields.
    let eval_ctx = crate::rules::engine::IntegrationEventContext {
        team_id: &delivery_claim.team_id,
        event_id: &event_id,
        trigger: event_type,
        state,
    };
    if let Err(err) = crate::rules::engine::evaluate_integration_event(&mut tx, &eval_ctx).await {
        tx.rollback().await;
        handle_post_rollback(db, config, worker_id, metrics, delivery_claim, err).await;
        return;
    }

    match delivery::complete_processed(
        &mut tx,
        &delivery_claim.delivery_row_id,
        &delivery_claim.claim_token,
        util::now_ms(),
    )
    .await
    {
        Ok(()) => {}
        Err(err) => {
            // Lost the lease or a completion race: the whole transaction —
            // event insert included — rolls back; the current owner owns the
            // outcome.
            tx.rollback().await;
            metrics.observe_webhook(WebhookEvent::StaleRejected);
            tracing::warn!(
                worker_id,
                delivery_id = %delivery_claim.delivery_row_id,
                error = %err,
                "processing hit a fenced or completed delivery"
            );
            return;
        }
    }

    match tx.commit().await {
        Ok(()) => {
            metrics.observe_webhook(WebhookEvent::Processed {
                duration_ms: started.elapsed().as_secs_f64() * 1000.0,
            });
            tracing::info!(
                worker_id,
                delivery_id = %delivery_claim.delivery_row_id,
                event_type = %event_type.as_str(),
                "webhook delivery processed; integration event persisted atomically"
            );
        }
        Err(commit_err) => {
            // Commit failure: the transaction rolled back — but we cannot
            // know that from here. Route through the fenced failure
            // recorder: if the commit truly failed, the retry is scheduled;
            // if it actually landed, the fenced update matches zero rows and
            // the provenance pre-check converges on the next claim.
            record_failure_and_observe(
                db,
                config,
                worker_id,
                metrics,
                &delivery_claim,
                delivery::classify_processing_failure(&commit_err),
                &commit_err,
            )
            .await;
        }
    }
}

/// Terminal `unsupported` transition (no domain event, no retry).
async fn process_unsupported(
    db: &Database,
    worker_id: &str,
    metrics: &Metrics,
    delivery_claim: ClaimedDelivery,
    reason: &'static str,
) {
    let Ok(mut tx) = db.begin().await else {
        // Cannot even open the transaction: leave the claim; lease
        // recovery re-queues without burning an attempt.
        return;
    };
    let complete = delivery::complete_unsupported(
        &mut tx,
        &delivery_claim.delivery_row_id,
        &delivery_claim.claim_token,
        reason,
        util::now_ms(),
    )
    .await;
    match complete {
        Ok(()) => match tx.commit().await {
            Ok(()) => {
                metrics.observe_webhook(WebhookEvent::Unsupported);
                tracing::info!(
                    worker_id,
                    delivery_id = %delivery_claim.delivery_row_id,
                    reason,
                    "webhook delivery durably recorded as unsupported"
                );
            }
            Err(err) => {
                // The commit consumed the transaction (already closed);
                // infrastructure hiccup: leave the claim for lease recovery,
                // which re-queues without burning an attempt.
                tracing::warn!(
                    worker_id,
                    delivery_id = %delivery_claim.delivery_row_id,
                    error = %err,
                    "unsupported completion commit failed; lease recovery will re-queue"
                );
            }
        },
        Err(err) => {
            tx.rollback().await;
            // Cannot record a failure through the fenced path if the claim
            // was lost; otherwise leave for lease recovery on transient DB
            // errors (do not burn budget on infrastructure hiccups).
            if matches!(err, AppError::Conflict(_)) {
                metrics.observe_webhook(WebhookEvent::StaleRejected);
            }
        }
    }
}

/// Rollback helper for pre-transaction normalization failures: no
/// transaction was opened (normalization is pure), so only the fenced
/// failure record is needed.
#[allow(clippy::too_many_arguments)]
async fn tx_rollback_record(
    db: &Database,
    config: &WebhookWorkerConfig,
    worker_id: &str,
    metrics: &Metrics,
    delivery_claim: ClaimedDelivery,
    class: FailureClass,
    err: &AppError,
) {
    record_failure_and_observe(db, config, worker_id, metrics, &delivery_claim, class, err).await;
}

/// After a rollback: classify and record the failure. A lost-claim Conflict
/// is a stale rejection (the current owner owns the outcome), not a retry.
async fn handle_post_rollback(
    db: &Database,
    config: &WebhookWorkerConfig,
    worker_id: &str,
    metrics: &Metrics,
    delivery_claim: ClaimedDelivery,
    err: AppError,
) {
    if matches!(err, AppError::Conflict(_)) {
        metrics.observe_webhook(WebhookEvent::StaleRejected);
        tracing::warn!(
            worker_id,
            delivery_id = %delivery_claim.delivery_row_id,
            error = %err,
            "stale claim rejected; the current owner owns the delivery"
        );
        return;
    }
    record_failure_and_observe(
        db,
        config,
        worker_id,
        metrics,
        &delivery_claim,
        delivery::classify_processing_failure(&err),
        &err,
    )
    .await;
}

/// Record a classified failure on the fenced claim and observe it.
async fn record_failure_and_observe(
    db: &Database,
    config: &WebhookWorkerConfig,
    worker_id: &str,
    metrics: &Metrics,
    delivery_claim: &ClaimedDelivery,
    class: FailureClass,
    err: &AppError,
) {
    let mut conn = db.conn();
    let jitter = rand_jitter();
    match delivery::record_failure(
        &mut conn,
        delivery_claim,
        class,
        err,
        config.max_attempts,
        i64::try_from(config.backoff_base_ms).unwrap_or(1_000),
        i64::try_from(config.backoff_max_ms).unwrap_or(60_000),
        jitter,
        i64::try_from(config.max_delivery_age_ms).unwrap_or(604_800_000),
        util::now_ms(),
    )
    .await
    {
        Ok(delivery::FailureDisposition::Rescheduled) => {
            metrics.observe_webhook(WebhookEvent::RetryableFailed);
            tracing::warn!(
                worker_id,
                delivery_id = %delivery_claim.delivery_row_id,
                attempt = delivery_claim.attempts + 1,
                classification = "retryable",
                "webhook delivery processing failed; rescheduled with backoff"
            );
        }
        Ok(delivery::FailureDisposition::DeadLettered) => {
            metrics.observe_webhook(WebhookEvent::DeadLettered);
            tracing::error!(
                worker_id,
                delivery_id = %delivery_claim.delivery_row_id,
                attempt = delivery_claim.attempts + 1,
                classification = if matches!(class, FailureClass::Permanent) {
                    "permanent"
                } else {
                    "exhausted-or-expired"
                },
                "webhook delivery dead-lettered"
            );
        }
        Err(lost @ AppError::Conflict(_)) => {
            // The claim was lost (reaped + re-claimed): observe a stale
            // rejection. The current owner owns the outcome.
            metrics.observe_webhook(WebhookEvent::StaleRejected);
            tracing::warn!(
                worker_id,
                delivery_id = %delivery_claim.delivery_row_id,
                error = %lost,
                "failure recording lost its claim"
            );
        }
        Err(db_err) => {
            // The failure record itself could not be written (infrastructure
            // down): the row stays `processing`, the lease expires, and the
            // delivery is retried without burning an attempt.
            tracing::error!(
                worker_id,
                delivery_id = %delivery_claim.delivery_row_id,
                error = %db_err,
                "could not persist delivery failure record; lease recovery will re-queue"
            );
        }
    }
}

/// Process-local full-jitter source in `[0, 1000]`. Shared implementation
/// with the Phase 8A dispatcher (same bounds, same unit-test coverage).
fn rand_jitter() -> i64 {
    crate::rules::dispatcher::rand_jitter()
}

/// Run the worker until shutdown. Claims and processes in cycles; stops
/// claiming as soon as shutdown is signalled and drains in-flight work.
/// Claiming never blocks the runtime: every cycle is bounded by `batch_size`
/// and `max_concurrent`.
pub async fn run_until_shutdown(
    db: Database,
    registry: ProviderRegistry,
    config: WebhookWorkerConfig,
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
        max_attempts = config.max_attempts,
        "webhook delivery worker started"
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
                if run_once(&db, &registry, &config, &worker_id, &metrics, &semaphore)
                    .await
                    .is_err()
                {
                    // Claim-cycle DB failure: keep polling; the next cycle
                    // retries recovery. Back off by one interval implicitly.
                    tracing::warn!(worker_id, "webhook claim cycle failed; will retry");
                }
            }
            _ = shutdown.changed() => {
                break;
            }
        }
    }
    // Drain: in-flight processing (spawned in run_once's JoinSet) has
    // already completed because run_once awaits its own set before
    // returning; nothing else is in flight here.
    tracing::info!(
        worker_id,
        "webhook delivery worker stopped; no claims accepted after shutdown"
    );
}

/// Spawn the worker as a tracked task wired to the server's shutdown signal.
/// Returns the join handle so main can await a clean stop.
#[must_use]
pub fn spawn(
    db: Database,
    registry: ProviderRegistry,
    config: WebhookWorkerConfig,
    metrics: Metrics,
    shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_until_shutdown(db, registry, config, metrics, shutdown))
}
