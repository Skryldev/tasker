//! Durable webhook infrastructure (Phase 8B, Task 2) — provider-neutral.
//!
//! This module owns everything that must NOT change per provider:
//! - `intake` — the HTTP endpoint, request-size enforcement, integration
//!   resolution, duplicate-delivery durability, and immediate acknowledgment;
//! - `delivery` — the durable delivery state machine with lease-based
//!   claiming, claim-token fencing, retry scheduling, dead-lettering, and
//!   auditable manual replay;
//! - `worker` — the background processing loop and its lifecycle.
//!
//! Provider-specific behavior is isolated behind [`ProviderAdapter`]:
//! signature verification (raw-body, constant-time) and payload
//! normalization. GitHub (Task 3) and GitLab (Task 4) implement that trait;
//! the engine is shared verbatim. Adapters never own persistence, leases,
//! retry orchestration, or worker coordination.
//!
//! State machine (persisted in `webhook_deliveries.status`, CHECK-constrained
//! since migration 0016):
//!
//! ```text
//!            intake (atomic insert, UNIQUE per (integration, delivery_id))
//!                 │
//!                 ▼
//!              pending ────────────────► processing (claim + lease)
//!                 ▲                        │        │         │
//!   lease expiry  │                        │        │         │
//!   (no attempt   ├────────────────────────┘        │         ├──► processed (terminal)
//!    burn)        │                                 │         ├──► unsupported (terminal)
//!   manual replay │                                 │         └──► dead_letter (terminal)
//!   (audited)     │                    retryable_failed
//!                 │                        ▲  backoff elapsed → claimable again
//!                 └────────────────────────┘
//! ```
//!
//! `retryable_failed` with a future `next_attempt_at` is the persisted form
//! of "retry scheduled"; `unsupported` is the durable form of "rejected"
//! (recognized but outside the supported vocabulary — no domain event, never
//! retried). Intake-rejected requests (bad signature, oversized, unknown
//! integration) are never persisted at all. Terminal states (`processed`,
//! `unsupported`, `dead_letter`) are never silently reopened; `dead_letter`
//! reopens only through the audited [`delivery::replay_dead_letter`].

pub mod delivery;
pub mod intake;
pub mod worker;

use std::collections::HashMap;
use std::sync::Arc;

use crate::integrations::{Provider, limits};
use crate::rules::events::IntegrationTriggerType;

/// An unmodified webhook request handed to a provider verifier. `body` is
/// the exact raw bytes received on the wire — verification is **never**
/// performed against re-serialized JSON, and the body has already passed the
/// 15,000-byte intake limit.
pub struct RawWebhook<'a> {
    pub integration_id: &'a str,
    pub headers: &'a axum::http::HeaderMap,
    pub body: &'a [u8],
}

/// The provider-extracted delivery identity, produced only after successful
/// verification. Bounded by [`validate_delivery_id`] /
/// [`validate_event_type_raw`] before persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDelivery {
    pub delivery_id: String,
    pub event_type_raw: String,
}

/// Why a provider verifier refused a request. Auth failures intentionally
/// collapse into one indistinguishable response class so callers cannot
/// probe which part of the verification chain failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationError {
    /// A required signature header is absent.
    MissingSignature,
    /// A signature header is present but unparseable (wrong scheme/length).
    MalformedSignature,
    /// The signature does not match (constant-time comparison).
    InvalidSignature,
    /// A required delivery/event header is absent or out of bounds.
    InvalidRequest(&'static str),
}

impl VerificationError {
    /// Whether this is an authentication failure (401) rather than a
    /// malformed request (400).
    #[must_use]
    pub fn is_auth_failure(&self) -> bool {
        matches!(
            self,
            Self::MissingSignature | Self::MalformedSignature | Self::InvalidSignature
        )
    }

    /// Bounded metric label for intake rejections (closed set — cardinality
    /// is fixed by construction).
    #[must_use]
    pub fn metric_reason(&self) -> &'static str {
        match self {
            Self::MissingSignature => "missing_signature",
            Self::MalformedSignature => "malformed_signature",
            Self::InvalidSignature => "invalid_signature",
            Self::InvalidRequest(_) => "invalid_request",
        }
    }
}

/// What normalizing one stored delivery produced. Pure: adapters run inside
/// the processing transaction and must not touch the database.
#[derive(Debug, Clone, PartialEq)]
pub enum NormalizedOutcome {
    /// The payload maps onto the integration trigger vocabulary.
    Event {
        event_type: IntegrationTriggerType,
        /// Provider event timestamp, validated by the engine
        /// (`delivery::sanitize_occurred_at`).
        occurred_at: i64,
        /// Provider-neutral state snapshot persisted on the domain event
        /// (`new_state`). Bounded by `NORMALIZED_STATE_MAX_CHARS`.
        state: serde_json::Value,
    },
    /// A recognized provider event outside the supported vocabulary: the
    /// delivery is durably recorded as `unsupported` (terminal) and never
    /// becomes a domain event.
    Unsupported { reason: &'static str },
    /// A payload malformed for an otherwise-supported event type: permanent
    /// failure — retrying can never fix the bytes.
    Invalid { reason: &'static str },
}

/// Provider-specific webhook adapter. The durable engine never changes per
/// provider: adapters implement verification and normalization only. Task 2
/// ships this contract plus the deterministic test adapter used by the test
/// suite; no production adapter is registered until Tasks 3–4.
pub trait ProviderAdapter: Send + Sync {
    fn provider(&self) -> Provider;

    /// Verify authenticity over the raw body. Implementations must use
    /// constant-time comparisons, must never accept unsigned requests, and
    /// must never leak the secret through errors.
    fn verify(
        &self,
        raw: &RawWebhook<'_>,
        secret: &str,
    ) -> Result<VerifiedDelivery, VerificationError>;

    /// Normalize a stored delivery's payload for processing.
    fn normalize(&self, event_type_raw: &str, payload: &str) -> NormalizedOutcome;
}

/// Maximum serialized size of a normalized state snapshot persisted on the
/// domain event (`new_state`). Integration state snapshots are small by
/// design (branch, SHA, counts); larger payloads indicate a normalization
/// bug and fail as a permanent error rather than bloating `domain_events`.
pub const NORMALIZED_STATE_MAX_CHARS: usize = 2_000;

/// Adapters registered at startup. Immutable after construction: the intake
/// resolves the adapter for the request's provider, and the worker resolves
/// the adapter for a delivery's provider. Tests inject deterministic
/// adapters; production registration happens with Tasks 3–4.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    adapters: Arc<HashMap<Provider, Arc<dyn ProviderAdapter>>>,
}

impl ProviderRegistry {
    #[must_use]
    pub fn new(adapters: HashMap<Provider, Arc<dyn ProviderAdapter>>) -> Self {
        Self {
            adapters: Arc::new(adapters),
        }
    }

    #[must_use]
    pub fn get(&self, provider: Provider) -> Option<Arc<dyn ProviderAdapter>> {
        self.adapters.get(&provider).cloned()
    }
}

/// Bounded provider delivery-id validation (GitHub GUIDs, GitLab numeric
/// ids — both fit). Non-empty, at most 64 characters, no whitespace or
/// control characters: delivery ids are correlation keys, not free text.
pub fn validate_delivery_id(raw: &str) -> Result<String, &'static str> {
    validate_bounded_id(raw, limits::DELIVERY_ID_MAX_CHARS, "delivery_id")
}

/// Bounded provider event-header validation (e.g. GitHub `X-GitHub-Event`).
pub fn validate_event_type_raw(raw: &str) -> Result<String, &'static str> {
    validate_bounded_id(raw, 64, "event_type_raw")
}

fn validate_bounded_id(raw: &str, max: usize, field: &'static str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("must not be empty");
    }
    if trimmed.chars().count() > max {
        return Err("exceeds the length bound");
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("must not contain whitespace or control characters");
    }
    let _ = field;
    Ok(trimmed.to_string())
}

/// Whether a database error is a unique-constraint violation. Dialect-aware
/// because sqlx Any surfaces driver text. Used by the intake to converge
/// concurrent duplicate deliveries on the `UNIQUE(integration_id,
/// delivery_id)` constraint — application-level pre-checks are only an
/// efficiency optimization; the database constraint is the correctness
/// boundary.
#[must_use]
pub fn is_unique_violation(err: &crate::error::AppError) -> bool {
    let text = err.to_string();
    text.contains("UNIQUE constraint failed")            // SQLite
        || text.contains("duplicate key value violates") // PostgreSQL
        || text.contains("Duplicate entry") // MySQL
}

/// Sanitize a provider event timestamp: implausible values (non-positive,
/// or more than one minute in the future — clock skew) are replaced with the
/// processing time. Unit-tested; documented in `delivery.rs`.
#[must_use]
pub fn sanitize_occurred_at(occurred_at: i64, now: i64) -> i64 {
    if occurred_at <= 0 || occurred_at > now + 60_000 {
        now
    } else {
        occurred_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_id_bounds() {
        assert!(validate_delivery_id(" 550e8400-e29b-41d4 ").is_ok());
        assert!(validate_delivery_id("12345").is_ok());
        assert!(validate_delivery_id("").is_err());
        assert!(validate_delivery_id("   ").is_err());
        let long = "d".repeat(limits::DELIVERY_ID_MAX_CHARS + 1);
        assert!(validate_delivery_id(&long).is_err());
        assert!(validate_delivery_id("has space").is_err());
        assert!(
            validate_delivery_id("a\u{7}b").is_err(),
            "control chars rejected"
        );
    }

    #[test]
    fn event_type_bounds() {
        assert!(validate_event_type_raw("push").is_ok());
        assert!(validate_event_type_raw("pull_request").is_ok());
        assert!(validate_event_type_raw("").is_err());
        let long = "e".repeat(65);
        assert!(validate_event_type_raw(&long).is_err());
    }

    #[test]
    fn auth_failure_classification_is_collapsed() {
        // Every authentication failure classifies as auth; none of the three
        // are distinguishable at the HTTP layer.
        for err in [
            VerificationError::MissingSignature,
            VerificationError::MalformedSignature,
            VerificationError::InvalidSignature,
        ] {
            assert!(err.is_auth_failure());
        }
        assert!(!VerificationError::InvalidRequest("x").is_auth_failure());
    }

    #[test]
    fn unique_violation_markers_cover_all_dialects() {
        for text in [
            "UNIQUE constraint failed: webhook_deliveries.integration_id",
            "duplicate key value violates unique constraint \"uq_webhook_deliveries\"",
            "Duplicate entry 'abc' for key 'uq_webhook_deliveries'",
        ] {
            assert!(is_unique_violation(&crate::error::AppError::internal(text)));
        }
        assert!(!is_unique_violation(&crate::error::AppError::internal(
            "CHECK constraint failed: webhook_deliveries_status"
        )));
    }

    #[test]
    fn occurred_at_sanitization() {
        let now = 1_700_000_000_000;
        assert_eq!(sanitize_occurred_at(now - 5_000, now), now - 5_000);
        // Clock skew beyond one minute: processing time wins.
        assert_eq!(sanitize_occurred_at(now + 120_000, now), now);
        // Non-positive timestamps are invalid, not "the epoch".
        assert_eq!(sanitize_occurred_at(0, now), now);
        assert_eq!(sanitize_occurred_at(-1, now), now);
        // Just inside the skew window is preserved.
        assert_eq!(sanitize_occurred_at(now + 60_000, now), now + 60_000);
    }

    /// Minimal structural check of the registry: lookup by provider, absent
    /// providers return `None`, and the registry is `Clone` (cheap `Arc`).
    #[test]
    fn registry_lookup() {
        use crate::integrations::Provider;
        struct Nop;
        impl ProviderAdapter for Nop {
            fn provider(&self) -> Provider {
                Provider::GitHub
            }
            fn verify(
                &self,
                _raw: &RawWebhook<'_>,
                _secret: &str,
            ) -> Result<VerifiedDelivery, VerificationError> {
                Err(VerificationError::MissingSignature)
            }
            fn normalize(&self, _event: &str, _payload: &str) -> NormalizedOutcome {
                NormalizedOutcome::Unsupported { reason: "nop" }
            }
        }
        let mut map = HashMap::new();
        map.insert(Provider::GitHub, Arc::new(Nop) as Arc<dyn ProviderAdapter>);
        let registry = ProviderRegistry::new(map);
        assert!(registry.get(Provider::GitHub).is_some());
        assert!(registry.get(Provider::GitLab).is_none());
    }
}
