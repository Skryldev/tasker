//! Minimal, bounded-cardinality Prometheus text-format metrics collected in
//! process. No external metrics dependency: counters and a duration histogram
//! keyed by (route template, method, status class) only, plus the Task 5
//! outbox dispatcher counters (fixed label set, no identifiers).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const DURATION_BUCKETS_MS: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0,
];

#[derive(Clone, PartialEq, Eq, Hash)]
struct RouteKey {
    route: String,
    method: String,
    status_class: &'static str,
}

#[derive(Default)]
struct RouteStats {
    count: u64,
    duration_ms_total: f64,
    histogram: [u64; 13],
}

/// Bounded outbox dispatcher counters (Task 5). Fixed label set — no task,
/// rule, or intent identifiers. Atomic so [`Metrics`] stays `Clone + Send`.
#[derive(Default)]
pub struct OutboxStats {
    claimed: AtomicU64,
    executed: AtomicU64,
    already_executed: AtomicU64,
    external_pending: AtomicU64,
    retryable_failed: AtomicU64,
    dead_lettered: AtomicU64,
    lease_recovered: AtomicU64,
    stale_rejected: AtomicU64,
    execution_ms_total: AtomicU64,
    executions: AtomicU64,
}

/// Dispatcher metric events.
#[derive(Debug, Clone, Copy)]
pub enum OutboxEvent {
    Claimed(u64),
    Executed { duration_ms: f64 },
    AlreadyExecuted,
    ExternalPending,
    RetryableFailed,
    DeadLettered,
    LeaseRecovered(u64),
    StaleRejected,
}

/// In-process metrics registry. `Clone` so the dispatcher can fan counters
/// out to its bounded execution tasks.
#[derive(Default, Clone)]
pub struct Metrics {
    routes: Arc<Mutex<HashMap<RouteKey, RouteStats>>>,
    requests_total: Arc<Mutex<u64>>,
    outbox: Arc<OutboxStats>,
}

impl Metrics {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one completed request.
    pub fn observe_request(&self, route: &str, method: &str, status: u16, took_ms: f64) {
        let key = RouteKey {
            route: route.to_string(),
            method: method.to_string(),
            status_class: class_of(status),
        };
        if let Ok(mut routes) = self.routes.lock() {
            let entry = routes.entry(key).or_default();
            entry.count += 1;
            entry.duration_ms_total += took_ms;
            for (idx, bound) in DURATION_BUCKETS_MS.iter().enumerate() {
                if took_ms <= *bound {
                    entry.histogram[idx] += 1;
                }
            }
        }
        if let Ok(mut total) = self.requests_total.lock() {
            *total += 1;
        }
    }

    /// Record one dispatcher event.
    pub fn observe_outbox(&self, event: OutboxEvent) {
        match event {
            OutboxEvent::Claimed(n) => self.outbox.claimed.fetch_add(n, Ordering::Relaxed),
            OutboxEvent::Executed { duration_ms } => {
                self.outbox.executed.fetch_add(1, Ordering::Relaxed);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let ms = duration_ms.max(0.0) as u64; // millisecond-granularity counter
                self.outbox
                    .execution_ms_total
                    .fetch_add(ms, Ordering::Relaxed);
                self.outbox.executions.fetch_add(1, Ordering::Relaxed)
            }
            OutboxEvent::AlreadyExecuted => {
                self.outbox.already_executed.fetch_add(1, Ordering::Relaxed)
            }
            OutboxEvent::ExternalPending => {
                self.outbox.external_pending.fetch_add(1, Ordering::Relaxed)
            }
            OutboxEvent::RetryableFailed => {
                self.outbox.retryable_failed.fetch_add(1, Ordering::Relaxed)
            }
            OutboxEvent::DeadLettered => self.outbox.dead_lettered.fetch_add(1, Ordering::Relaxed),
            OutboxEvent::LeaseRecovered(n) => {
                self.outbox.lease_recovered.fetch_add(n, Ordering::Relaxed)
            }
            OutboxEvent::StaleRejected => {
                self.outbox.stale_rejected.fetch_add(1, Ordering::Relaxed)
            }
        };
    }

    /// Render the Prometheus text exposition format.
    #[must_use]
    pub fn render(&self, uptime_secs: u64) -> String {
        let mut out = String::with_capacity(4096);
        out.push_str("# HELP tasker_uptime_seconds Process uptime in seconds.\n");
        out.push_str("# TYPE tasker_uptime_seconds gauge\n");
        out.push_str(&format!("tasker_uptime_seconds {uptime_secs}\n"));

        out.push_str("# HELP tasker_requests_total Total HTTP requests.\n");
        out.push_str("# TYPE tasker_requests_total counter\n");
        if let Ok(total) = self.requests_total.lock() {
            out.push_str(&format!("tasker_requests_total {total}\n"));
        }

        out.push_str("# HELP tasker_request_duration_seconds HTTP request duration by route.\n");
        out.push_str("# TYPE tasker_request_duration_seconds histogram\n");
        if let Ok(routes) = self.routes.lock() {
            let mut keys: Vec<&RouteKey> = routes.keys().collect();
            keys.sort_by(|a, b| (&a.route, &a.method).cmp(&(&b.route, &b.method)));
            for key in keys {
                let stats = &routes[key];
                let route = sanitize_label(&key.route);
                let method = sanitize_label(&key.method);
                let base = &format!(
                    "tasker_request_duration_seconds_bucket{{route=\"{route}\",method=\"{method}\",status_class=\"{}\"",
                    key.status_class
                );
                let mut cumulative = 0u64;
                for (idx, bound) in DURATION_BUCKETS_MS.iter().enumerate() {
                    cumulative += stats.histogram[idx];
                    out.push_str(&format!(
                        "{base},le=\"{}\"}} {cumulative}\n",
                        bound / 1000.0
                    ));
                }
                out.push_str(&format!("{base},le=\"+Inf\"}} {}\n", stats.count));
                out.push_str(&format!(
                    "tasker_request_duration_seconds_sum{{route=\"{route}\",method=\"{method}\",status_class=\"{}\"}} {:.6}\n",
                    key.status_class, stats.duration_ms_total / 1000.0
                ));
                out.push_str(&format!(
                    "tasker_request_duration_seconds_count{{route=\"{route}\",method=\"{method}\",status_class=\"{}\"}} {}\n",
                    key.status_class, stats.count
                ));
            }
        }

        let counter = |out: &mut String, name: &str, help: &str, value: u64| {
            out.push_str(&format!("# HELP {name} {help}.\n"));
            out.push_str(&format!("# TYPE {name} counter\n"));
            out.push_str(&format!("{name} {value}\n"));
        };
        counter(
            &mut out,
            "tasker_outbox_intents_claimed_total",
            "Outbox intents claimed by the dispatcher",
            self.outbox.claimed.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "tasker_outbox_actions_executed_total",
            "Internal automation actions executed successfully",
            self.outbox.executed.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "tasker_outbox_already_executed_total",
            "Duplicate executions converged through the idempotency ledger",
            self.outbox.already_executed.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "tasker_outbox_external_pending_total",
            "send_message intents left pending for the external dispatcher",
            self.outbox.external_pending.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "tasker_outbox_retryable_failures_total",
            "Retryable execution failures recorded",
            self.outbox.retryable_failed.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "tasker_outbox_dead_lettered_total",
            "Intents moved to the terminal failure state",
            self.outbox.dead_lettered.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "tasker_outbox_leases_recovered_total",
            "Expired claims recovered after worker crashes",
            self.outbox.lease_recovered.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "tasker_outbox_stale_rejections_total",
            "Completions rejected because the claim token was stale",
            self.outbox.stale_rejected.load(Ordering::Relaxed),
        );
        out.push_str(
            "# HELP tasker_outbox_execution_duration_seconds Internal action execution duration.\n",
        );
        out.push_str("# TYPE tasker_outbox_execution_duration_seconds summary\n");
        // Counter totals stay far below f64's 2^52 exact-integer bound.
        #[allow(clippy::cast_precision_loss)]
        let total_seconds = self.outbox.execution_ms_total.load(Ordering::Relaxed) as f64 / 1000.0;
        out.push_str(&format!(
            "tasker_outbox_execution_duration_seconds_sum {total_seconds:.6}\n"
        ));
        out.push_str(&format!(
            "tasker_outbox_execution_duration_seconds_count {}\n",
            self.outbox.executions.load(Ordering::Relaxed)
        ));
        out
    }
}

fn class_of(status: u16) -> &'static str {
    match status {
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

fn sanitize_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}
