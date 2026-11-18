//! HTTP webhook intake (Phase 8B, Task 2).
//!
//! The intake is bounded and fast: it enforces the 15,000-byte raw-body
//! limit **before** buffering, resolves the provider adapter and the
//! integration, delegates authenticity to the provider verifier (constant
//! time, raw-body), persists the delivery atomically under the
//! `UNIQUE(integration_id, delivery_id)` constraint, and acknowledges. No
//! provider parsing, Rules Engine work, or external calls happen here —
//! durable persistence is the boundary between intake and processing.
//!
//! Deterministic responses (provider-compatible; any 2xx stops provider
//! retries):
//!
//! | Situation                              | Status | Body                |
//! |----------------------------------------|--------|---------------------|
//! | First valid delivery                   | `202`  | `{"status":"accepted"}`  |
//! | Duplicate (any prior state)            | `200`  | `{"status":"duplicate"}` |
//! | Authentication failure                 | `401`  | problem details     |
//! | Malformed request (headers, encoding)  | `400`  | problem details     |
//! | Payload beyond the byte limit          | `413`  | problem details     |
//! | Unsupported content type               | `415`  | problem details     |
//! | Unknown/disabled/archived integration, unregistered provider | `404` | problem details |
//! | Database temporarily busy              | `503`  | problem details     |
//!
//! Rejected requests are never persisted. Integration existence is masked:
//! unknown, disabled, archived, and provider-mismatched integrations all
//! return the same `404`.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use serde_json::json;

use crate::error::AppError;
use crate::http::metrics::WebhookEvent;
use crate::integrations::webhook::{self, ProviderRegistry, RawWebhook};
use crate::integrations::{Provider, limits, store};
use crate::state::AppState;

/// Webhook intake routes, with a route-scoped body limit that overrides the
/// application default: enforcement happens during extraction (streaming
/// safe) and again in the handler over the raw bytes — never solely via
/// `Content-Length`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/webhooks/{provider}/{integration_id}",
            post(handle).get(intake_method_not_allowed),
        )
        .layer(DefaultBodyLimit::max(limits::WEBHOOK_BODY_MAX_BYTES))
}

async fn intake_method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        crate::error::AppError::BadRequest("webhook intake accepts POST only".into()),
    )
        .into_response()
}

/// The intake outcome, separated from the HTTP handler for testability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeOutcome {
    /// First valid delivery: durably persisted as `pending`.
    Accepted,
    /// Delivery id already persisted (any state): idempotent no-op.
    Duplicate,
}

/// One authenticated-and-persisted (or rejected) intake. Business rules live
/// here; the HTTP handler stays thin.
// Explicit parameters mirror the intake flow's distinct inputs (path, key,
// headers, raw body) the same way `delivery::record_failure` mirrors its
// configuration knobs.
#[allow(clippy::too_many_arguments)]
pub async fn intake(
    db: &crate::db::Database,
    registry: &ProviderRegistry,
    integration_key: Option<&[u8]>,
    metrics: &crate::http::metrics::Metrics,
    provider_str: &str,
    integration_id: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<IntakeOutcome, IntakeRejection> {
    // ---- Provider resolution (before any body inspection) ----------------
    let Some(provider) = Provider::parse(provider_str) else {
        return Err(reject(metrics, StatusCode::NOT_FOUND, "unknown_provider"));
    };
    let Some(adapter) = registry.get(provider) else {
        // A known provider with no deployed adapter is not yet available.
        return Err(reject(metrics, StatusCode::NOT_FOUND, "unknown_provider"));
    };

    // ---- Integration resolution (tenant/integration state boundary) ------
    // Existence is masked: unknown, disabled, archived, and provider-
    // mismatched integrations all produce the same 404.
    let mut conn = db.conn();
    let _integration = match store::find_integration(&mut conn, integration_id).await {
        Ok(Some(i)) if i.provider == provider && i.enabled && i.archived_at.is_none() => i,
        Ok(_) => {
            return Err(reject(
                metrics,
                StatusCode::NOT_FOUND,
                "unknown_integration",
            ));
        }
        Err(AppError::Busy { retry_after_secs }) => {
            return Err(reject_busy(metrics, retry_after_secs));
        }
        Err(err) => {
            tracing::error!(error = %err, "intake could not read the integration row");
            return Err(reject(
                metrics,
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
            ));
        }
    };
    drop(conn);

    // ---- Content validation ---------------------------------------------
    let content_type_ok = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        });
    if !content_type_ok {
        return Err(reject(
            metrics,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content_type",
        ));
    }
    let payload = match std::str::from_utf8(&body) {
        Ok(text) => text.to_string(),
        Err(_) => return Err(reject(metrics, StatusCode::BAD_REQUEST, "encoding")),
    };

    // ---- Provider authentication over the raw body -----------------------
    // Fail-closed secret resolution: a missing credential, a revoked
    // credential, or a missing/malformed server key all refuse verification
    // (401) — never a fallback, never an unsigned accept.
    let mut conn = db.conn();
    let secret = match crate::integrations::credentials::load_verification_secret(
        &mut conn,
        integration_id,
        integration_key.unwrap_or(&[]),
    )
    .await
    {
        Ok(Some(secret)) => secret,
        Ok(None) => {
            tracing::warn!(integration_id, "intake refused: no active credential");
            return Err(reject(
                metrics,
                StatusCode::UNAUTHORIZED,
                "invalid_signature",
            ));
        }
        Err(err) => {
            // Server-side key/credential problem: refuse without leaking
            // details; the error is logged server-side only.
            tracing::error!(integration_id, error = %err, "verification secret unavailable");
            return Err(reject(
                metrics,
                StatusCode::UNAUTHORIZED,
                "invalid_signature",
            ));
        }
    };
    let raw = RawWebhook {
        integration_id,
        headers,
        body: body.as_ref(),
    };
    let verified = match adapter.verify(&raw, &secret) {
        Ok(v) => v,
        Err(err) => {
            let reason = err.metric_reason();
            let status = if err.is_auth_failure() {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_REQUEST
            };
            return Err(reject(metrics, status, reason));
        }
    };
    // Bounded identity validation: provider ids are correlation keys, never
    // free text.
    let Ok(delivery_id) = webhook::validate_delivery_id(&verified.delivery_id) else {
        return Err(reject(metrics, StatusCode::BAD_REQUEST, "invalid_request"));
    };
    let Ok(event_type_raw) = webhook::validate_event_type_raw(&verified.event_type_raw) else {
        return Err(reject(metrics, StatusCode::BAD_REQUEST, "invalid_request"));
    };

    // ---- Atomic durable persistence --------------------------------------
    // Replay protection: the existence pre-check inside `insert_delivery`
    // converges common duplicates; the UNIQUE constraint is the correctness
    // boundary for true concurrent races (surfacing as a unique violation,
    // which is the duplicate response, not an error).
    match store::insert_delivery(
        &mut conn,
        integration_id,
        &delivery_id,
        &event_type_raw,
        &payload,
        crate::util::now_ms(),
    )
    .await
    {
        Ok(true) => {
            metrics.observe_webhook(WebhookEvent::IntakeAccepted);
            tracing::info!(
                integration_id,
                delivery_id,
                event_type = %event_type_raw,
                "webhook delivery accepted"
            );
            Ok(IntakeOutcome::Accepted)
        }
        Ok(false) => {
            metrics.observe_webhook(WebhookEvent::IntakeDuplicate);
            Ok(IntakeOutcome::Duplicate)
        }
        Err(err) if webhook::is_unique_violation(&err) => {
            metrics.observe_webhook(WebhookEvent::IntakeDuplicate);
            Ok(IntakeOutcome::Duplicate)
        }
        Err(AppError::Busy { retry_after_secs }) => Err(reject_busy(metrics, retry_after_secs)),
        Err(err) => {
            tracing::error!(error = %err, integration_id, "delivery persistence failed");
            Err(reject(
                metrics,
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
            ))
        }
    }
}

/// A rejection with its bounded metric reason. Payloads, signatures, and
/// secrets never enter logs or responses.
#[derive(Debug)]
pub struct IntakeRejection {
    pub status: StatusCode,
    pub reason: &'static str,
}

fn reject(
    metrics: &crate::http::metrics::Metrics,
    status: StatusCode,
    reason: &'static str,
) -> IntakeRejection {
    metrics.observe_webhook(WebhookEvent::IntakeRejected(reason));
    IntakeRejection { status, reason }
}

fn reject_busy(metrics: &crate::http::metrics::Metrics, _retry_after_secs: u64) -> IntakeRejection {
    // Busy is transient infrastructure state, not a rejection class; the
    // provider will retry. Counted under `busy` for observability.
    metrics.observe_webhook(WebhookEvent::IntakeRejected("busy"));
    IntakeRejection {
        status: StatusCode::SERVICE_UNAVAILABLE,
        reason: "busy",
    }
}

/// HTTP handler: thin adapter over [`intake`]. The body is read with an
/// explicit byte cap (streaming-safe; not solely reliant on Content-Length),
/// so oversized requests are refused before full buffering.
async fn handle(
    State(state): State<AppState>,
    Path((provider_str, integration_id)): Path<(String, String)>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    // Fast-path Content-Length enforcement (browsers/providers that declare
    // the size): reject before touching the body stream.
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<usize>().ok())
        .is_some_and(|len| len > limits::WEBHOOK_BODY_MAX_BYTES)
    {
        let rejection = reject(&state.metrics, StatusCode::PAYLOAD_TOO_LARGE, "too_large");
        return rejection.into_response();
    }
    let body = match axum::body::to_bytes(request.into_body(), limits::WEBHOOK_BODY_MAX_BYTES).await
    {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::debug!(error = %err, "webhook body exceeded the intake limit");
            let rejection = reject(&state.metrics, StatusCode::PAYLOAD_TOO_LARGE, "too_large");
            return rejection.into_response();
        }
    };

    let outcome = intake(
        &state.db,
        &state.integrations.registry,
        state.integrations.key.as_deref(),
        &state.metrics,
        &provider_str,
        &integration_id,
        &headers,
        body,
    )
    .await;

    match outcome {
        Ok(IntakeOutcome::Accepted) => {
            (StatusCode::ACCEPTED, Json(json!({"status": "accepted"}))).into_response()
        }
        Ok(IntakeOutcome::Duplicate) => {
            (StatusCode::OK, Json(json!({"status": "duplicate"}))).into_response()
        }
        Err(rejection) => rejection.into_response(),
    }
}

impl IntoResponse for IntakeRejection {
    fn into_response(self) -> Response {
        // Problem-details-shaped body with a stable `reason` class; no
        // payload, signature, or secret material ever appears here.
        (
            self.status,
            Json(json!({
                "title": "webhook delivery rejected",
                "reason": self.reason,
            })),
        )
            .into_response()
    }
}
