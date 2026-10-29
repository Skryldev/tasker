//! Minimal, bounded-cardinality Prometheus text-format metrics collected in
//! process. No external metrics dependency: counters and a duration histogram
//! keyed by (route template, method, status class) only.

use std::collections::HashMap;
use std::sync::Mutex;

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

/// In-process metrics registry.
#[derive(Default)]
pub struct Metrics {
    routes: Mutex<HashMap<RouteKey, RouteStats>>,
    requests_total: Mutex<u64>,
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
