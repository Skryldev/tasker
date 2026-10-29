//! Request-scoped extractors.

use axum::http::HeaderMap;

/// Best-effort client IP for rate limiting and session metadata:
/// `X-Forwarded-For` (first hop, set by our reverse proxy) wins, then
/// `X-Real-IP`, else "unknown". Deployments not behind a trusted proxy should
/// note that X-Forwarded-For is client-controlled.
#[must_use]
pub fn client_ip(headers: &HeaderMap) -> String {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(first) = xff.split(',').next()
    {
        let ip = first.trim();
        if !ip.is_empty() {
            return ip.to_string();
        }
    }
    if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let ip = real.trim();
        if !ip.is_empty() {
            return ip.to_string();
        }
    }
    "unknown".to_string()
}
