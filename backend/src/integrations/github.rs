//! GitHub webhook adapter (Phase 8B, Task 3): the provider-specific half of
//! the webhook pipeline, implementing the provider-neutral
//! [`ProviderAdapter`] contract.
//!
//! Responsibilities (and nothing else — persistence, leases, retries, and
//! worker coordination live in `webhook::delivery`/`webhook::worker`):
//! - **Verification**: `X-Hub-Signature-256` — HMAC-SHA256 over the exact
//!   raw request body (`sha256=<hex>`), constant-time comparison. The secret
//!   is the integration's decrypted webhook credential supplied by the
//!   intake (fail-closed upstream; never logged, never persisted).
//! - **Identity**: `X-GitHub-Delivery` (provider delivery id) and
//!   `X-GitHub-Event` (raw event name), both bounded before persistence.
//! - **Classification**: a CLOSED registry over the current integration
//!   trigger vocabulary. Every other GitHub event — `ping`, `issues`,
//!   `workflow_run`, `check_run`, PR `synchronize`/`edited`/`labeled`, tag
//!   pushes, branch-deletion pushes (the `delete` event covers them), … — is
//!   durably recorded as `unsupported` by the engine: retained history, no
//!   domain event, no automation. Unknown events never silently become
//!   executable domain events.
//! - **Normalization**: bounded, provider-neutral state snapshots using the
//!   `integration.*` condition-namespace keys (`provider`, `event_type`,
//!   `branch`, `repository`, `actor`, `commit_count`) plus bounded event
//!   fields (`head_sha`, `pr_number`, `pr_title`, …), so the Rules Engine
//!   (Task 5) reads one consistent shape for every provider. Strings are
//!   truncated to the condition namespace's scalar bound; external actors
//!   stay external identities.

use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::integrations::Provider;
use crate::integrations::webhook::{
    NormalizedOutcome, ProviderAdapter, RawWebhook, VerificationError, VerifiedDelivery,
};
use crate::rules::events::IntegrationTriggerType;
use crate::util;

const SIGNATURE_HEADER: &str = "x-hub-signature-256";
const DELIVERY_HEADER: &str = "x-github-delivery";
const EVENT_HEADER: &str = "x-github-event";
const SIGNATURE_PREFIX: &str = "sha256=";
/// SHA-256 hex digest length.
const SIGNATURE_HEX_LEN: usize = 64;
/// Bounded length for every string field of a normalized snapshot (mirrors
/// the condition namespace's `Scalar::Text` bound).
const FIELD_MAX_CHARS: usize = 255;

pub struct GitHubAdapter;

fn header_str(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Plain base-16 encoding of a digest (standard table lookup — not
/// cryptography; the cryptography is HMAC-SHA256 itself).
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

fn bounded(s: &str) -> String {
    util::truncate(s, FIELD_MAX_CHARS)
}

/// Parse a GitHub ISO-8601 timestamp into epoch milliseconds; `None` for
/// absent/unparseable values (the engine's sanitization then substitutes the
/// processing time — provider clock skew never poisons ordering).
fn parse_timestamp(raw: Option<&str>) -> Option<i64> {
    let raw = raw?;
    let dt =
        time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok()?;
    Some(dt.unix_timestamp() * 1_000 + i64::from(dt.millisecond()))
}

impl ProviderAdapter for GitHubAdapter {
    fn provider(&self) -> Provider {
        Provider::GitHub
    }

    fn verify(
        &self,
        raw: &RawWebhook<'_>,
        secret: &str,
    ) -> Result<VerifiedDelivery, VerificationError> {
        let sig =
            header_str(raw.headers, SIGNATURE_HEADER).ok_or(VerificationError::MissingSignature)?;
        let hex = sig
            .strip_prefix(SIGNATURE_PREFIX)
            .ok_or(VerificationError::MalformedSignature)?;
        if hex.len() != SIGNATURE_HEX_LEN || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(VerificationError::MalformedSignature);
        }
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(raw.body);
        let tag = mac.finalize().into_bytes();
        let expected_hex: String = hex_encode(&tag);
        // Constant-time comparison via the established Phase 7 helper.
        if !crate::security::token::hashes_equal(&hex.to_ascii_lowercase(), &expected_hex) {
            return Err(VerificationError::InvalidSignature);
        }
        let delivery_id = header_str(raw.headers, DELIVERY_HEADER).ok_or(
            VerificationError::InvalidRequest("missing X-GitHub-Delivery"),
        )?;
        let event_type_raw = header_str(raw.headers, EVENT_HEADER)
            .ok_or(VerificationError::InvalidRequest("missing X-GitHub-Event"))?;
        Ok(VerifiedDelivery {
            delivery_id,
            event_type_raw,
        })
    }

    fn normalize(&self, event_type_raw: &str, payload: &str) -> NormalizedOutcome {
        let parsed: Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => {
                return NormalizedOutcome::Invalid {
                    reason: "payload is not valid JSON",
                };
            }
        };
        match event_type_raw {
            "push" => normalize_push(&parsed),
            "pull_request" => normalize_pull_request(&parsed),
            "create" | "delete" => normalize_ref(event_type_raw, &parsed),
            "release" => normalize_release(&parsed),
            _ => NormalizedOutcome::Unsupported {
                reason: "github event outside the supported vocabulary",
            },
        }
    }
}

/// The shared snapshot keys every supported event carries — exactly the
/// `integration.*` condition-namespace fields the Rules Engine (Task 5)
/// evaluates, so task rules and integration rules read one shape.
fn base_state<'a>(
    parsed: &'a Value,
    event_type: &str,
    actor_pointer: &'a str,
) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    map.insert("provider".into(), json!("github"));
    map.insert("event_type".into(), json!(event_type));
    // Repository identity: the human `full_name` (bounded) when present.
    if let Some(name) = parsed
        .pointer("/repository/full_name")
        .and_then(Value::as_str)
    {
        map.insert("repository".into(), json!(bounded(name)));
    }
    if let Some(actor) = parsed.pointer(actor_pointer).and_then(Value::as_str) {
        map.insert("actor".into(), json!(bounded(actor)));
    }
    map
}

/// `push`: branch pushes onto `git_push`; tag pushes and branch-deletion
/// pushes (the `delete` event covers them durably) stay outside the
/// vocabulary so branch conditions can never mis-match a tag or a deletion.
fn normalize_push(parsed: &Value) -> NormalizedOutcome {
    let full_ref = parsed.pointer("/ref").and_then(Value::as_str).unwrap_or("");
    let Some(branch) = full_ref.strip_prefix("refs/heads/") else {
        return NormalizedOutcome::Unsupported {
            reason: "push to a non-branch ref",
        };
    };
    if parsed.pointer("/deleted").and_then(Value::as_bool) == Some(true) {
        return NormalizedOutcome::Unsupported {
            reason: "branch deletion push (delete event covers it)",
        };
    }
    let mut state = base_state(parsed, "git_push", "/sender/login");
    state.insert("branch".into(), json!(bounded(branch)));
    let commit_count = parsed
        .pointer("/commits")
        .and_then(Value::as_array)
        .map_or(0i64, |commits| {
            i64::try_from(commits.len()).unwrap_or(i64::MAX)
        });
    state.insert("commit_count".into(), json!(commit_count));
    if let Some(sha) = parsed.pointer("/after").and_then(Value::as_str) {
        state.insert("head_sha".into(), json!(bounded(sha)));
    }
    let occurred_at = parse_timestamp(
        parsed
            .pointer("/head_commit/timestamp")
            .and_then(Value::as_str),
    )
    .unwrap_or(0);
    NormalizedOutcome::Event {
        event_type: IntegrationTriggerType::GitPush,
        occurred_at,
        state: Value::Object(state),
    }
}

/// `pull_request`: opened/reopened/closed map onto the vocabulary
/// (`closed` splits by `merged` — never conflated); every other PR action
/// (`synchronize`, `edited`, `labeled`, `review_requested`, …) is durably
/// recorded as unsupported until the vocabulary is extended.
fn normalize_pull_request(parsed: &Value) -> NormalizedOutcome {
    let action = parsed
        .pointer("/action")
        .and_then(Value::as_str)
        .unwrap_or("");
    let event_type = match action {
        "opened" | "reopened" => IntegrationTriggerType::GitPullRequestOpened,
        "closed" => {
            if parsed
                .pointer("/pull_request/merged")
                .and_then(Value::as_bool)
                == Some(true)
            {
                IntegrationTriggerType::GitPullRequestMerged
            } else {
                IntegrationTriggerType::GitPullRequestClosed
            }
        }
        _ => {
            return NormalizedOutcome::Unsupported {
                reason: "pull_request action outside the supported vocabulary",
            };
        }
    };
    let mut state = base_state(parsed, event_type.as_str(), "/sender/login");
    if let Some(number) = parsed
        .pointer("/pull_request/number")
        .and_then(Value::as_i64)
    {
        state.insert("pr_number".into(), json!(number));
    }
    if let Some(title) = parsed
        .pointer("/pull_request/title")
        .and_then(Value::as_str)
    {
        state.insert("pr_title".into(), json!(bounded(title)));
    }
    if let Some(pr_state) = parsed
        .pointer("/pull_request/state")
        .and_then(Value::as_str)
    {
        state.insert("pr_state".into(), json!(bounded(pr_state)));
    }
    state.insert(
        "pr_merged".into(),
        json!(
            parsed
                .pointer("/pull_request/merged")
                .and_then(Value::as_bool)
                == Some(true)
        ),
    );
    if let Some(author) = parsed
        .pointer("/pull_request/user/login")
        .and_then(Value::as_str)
    {
        state.insert("pr_author".into(), json!(bounded(author)));
    }
    if let Some(branch) = parsed
        .pointer("/pull_request/head/ref")
        .and_then(Value::as_str)
    {
        state.insert("branch".into(), json!(bounded(branch)));
    }
    if let Some(base) = parsed
        .pointer("/pull_request/base/ref")
        .and_then(Value::as_str)
    {
        state.insert("base_branch".into(), json!(bounded(base)));
    }
    if let Some(sha) = parsed
        .pointer("/pull_request/head/sha")
        .and_then(Value::as_str)
    {
        state.insert("head_sha".into(), json!(bounded(sha)));
    }
    let occurred_at = ["merged_at", "closed_at", "updated_at", "created_at"]
        .iter()
        .find_map(|key| {
            parse_timestamp(
                parsed
                    .pointer(&format!("/pull_request/{key}"))
                    .and_then(Value::as_str),
            )
        })
        .unwrap_or(0);
    NormalizedOutcome::Event {
        event_type,
        occurred_at,
        state: Value::Object(state),
    }
}

/// `create`/`delete`: branch refs onto the branch vocabulary; tag refs stay
/// outside it (tag creation is release-adjacent, not branch activity).
fn normalize_ref(event_type_raw: &str, parsed: &Value) -> NormalizedOutcome {
    if parsed.pointer("/ref_type").and_then(Value::as_str) != Some("branch") {
        return NormalizedOutcome::Unsupported {
            reason: "non-branch ref create/delete",
        };
    }
    let event_type = match event_type_raw {
        "create" => IntegrationTriggerType::GitBranchCreated,
        _ => IntegrationTriggerType::GitBranchDeleted,
    };
    let mut state = base_state(parsed, event_type.as_str(), "/sender/login");
    if let Some(branch) = parsed.pointer("/ref").and_then(Value::as_str) {
        state.insert("branch".into(), json!(bounded(branch)));
    }
    NormalizedOutcome::Event {
        event_type,
        occurred_at: 0, // create/delete carry no timestamp; processing time applies
        state: Value::Object(state),
    }
}

/// `release`: only `published` enters the vocabulary; edits and deletions
/// are durably recorded as unsupported.
fn normalize_release(parsed: &Value) -> NormalizedOutcome {
    if parsed.pointer("/action").and_then(Value::as_str) != Some("published") {
        return NormalizedOutcome::Unsupported {
            reason: "release action outside the supported vocabulary",
        };
    }
    let mut state = base_state(parsed, "git_release_published", "/sender/login");
    if let Some(tag) = parsed.pointer("/release/tag_name").and_then(Value::as_str) {
        state.insert("release_tag".into(), json!(bounded(tag)));
    }
    if let Some(name) = parsed.pointer("/release/name").and_then(Value::as_str) {
        state.insert("release_name".into(), json!(bounded(name)));
    }
    state.insert(
        "prerelease".into(),
        json!(
            parsed
                .pointer("/release/prerelease")
                .and_then(Value::as_bool)
                == Some(true)
        ),
    );
    let occurred_at = parse_timestamp(
        parsed
            .pointer("/release/published_at")
            .and_then(Value::as_str),
    )
    .unwrap_or(0);
    NormalizedOutcome::Event {
        event_type: IntegrationTriggerType::GitReleasePublished,
        occurred_at,
        state: Value::Object(state),
    }
}

// ---------------------------------------------------------------------------
// Tests: verification matrix, closed classification registry, bounds
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    const SECRET: &str = "gh-secret";

    fn raw<'a>(headers: &'a HeaderMap, body: &'a [u8]) -> RawWebhook<'a> {
        RawWebhook {
            integration_id: "int-1",
            headers,
            body,
        }
    }

    fn signed_headers(delivery: &str, event: &str, secret: &str, body: &[u8]) -> HeaderMap {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let tag = mac.finalize().into_bytes();
        let hex: String = hex_encode(&tag);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hub-signature-256",
            format!("sha256={hex}").parse().unwrap(),
        );
        headers.insert("x-github-delivery", delivery.parse().unwrap());
        headers.insert("x-github-event", event.parse().unwrap());
        headers
    }

    #[test]
    fn valid_signature_verifies_and_extracts_identity() {
        let body = br#"{"zen":"keep it simple"}"#;
        let headers = signed_headers("guid-1", "ping", SECRET, body);
        let verified = GitHubAdapter
            .verify(&raw(&headers, body), SECRET)
            .expect("valid signature");
        assert_eq!(verified.delivery_id, "guid-1");
        assert_eq!(verified.event_type_raw, "ping");
    }

    #[test]
    fn tampered_body_and_wrong_secret_fail_closed() {
        let body = br#"{"a":1}"#;
        let headers = signed_headers("g", "push", SECRET, body);
        // Tampered body (signature over different bytes).
        assert_eq!(
            GitHubAdapter.verify(&raw(&headers, br#"{"a":2}"#), SECRET),
            Err(VerificationError::InvalidSignature)
        );
        // Wrong secret.
        assert_eq!(
            GitHubAdapter.verify(&raw(&headers, body), "other-secret"),
            Err(VerificationError::InvalidSignature)
        );
    }

    #[test]
    fn missing_malformed_signatures_are_classified() {
        let body = br"{}";
        // Missing entirely.
        let headers = HeaderMap::new();
        assert_eq!(
            GitHubAdapter.verify(&raw(&headers, body), SECRET),
            Err(VerificationError::MissingSignature)
        );
        // Wrong scheme.
        let mut headers = HeaderMap::new();
        headers.insert("x-hub-signature-256", "sha1=abc".parse().unwrap());
        headers.insert("x-github-delivery", "g".parse().unwrap());
        headers.insert("x-github-event", "push".parse().unwrap());
        assert_eq!(
            GitHubAdapter.verify(&raw(&headers, body), SECRET),
            Err(VerificationError::MalformedSignature)
        );
        // Bad length / non-hex.
        let mut headers = HeaderMap::new();
        headers.insert("x-hub-signature-256", "sha256=zz".parse().unwrap());
        headers.insert("x-github-delivery", "g".parse().unwrap());
        headers.insert("x-github-event", "push".parse().unwrap());
        assert_eq!(
            GitHubAdapter.verify(&raw(&headers, body), SECRET),
            Err(VerificationError::MalformedSignature)
        );
        // Missing identity headers after a valid signature.
        let good = signed_headers("g", "push", SECRET, body);
        let mut no_identity = good.clone();
        no_identity.remove("x-github-delivery");
        assert!(matches!(
            GitHubAdapter.verify(&raw(&no_identity, body), SECRET),
            Err(VerificationError::InvalidRequest(_))
        ));
    }

    #[test]
    fn push_normalization_carries_namespace_keys() {
        let payload = json!({
            "ref": "refs/heads/feature/login",
            "after": "abc123",
            "deleted": false,
            "sender": {"login": "octocat"},
            "repository": {"full_name": "octocat/tasker"},
            "commits": [{"id": "1"}, {"id": "2"}],
            "head_commit": {"timestamp": "2026-01-02T03:04:05Z"},
        })
        .to_string();
        let outcome = GitHubAdapter.normalize("push", &payload);
        let NormalizedOutcome::Event {
            event_type,
            occurred_at,
            state,
        } = outcome
        else {
            panic!("expected an event, got {outcome:?}");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitPush);
        // 2026-01-02T03:04:05Z
        assert_eq!(occurred_at, 1_767_323_045_000);
        assert_eq!(state["provider"], "github");
        assert_eq!(state["event_type"], "git_push");
        assert_eq!(state["branch"], "feature/login");
        assert_eq!(state["repository"], "octocat/tasker");
        assert_eq!(state["actor"], "octocat");
        assert_eq!(state["commit_count"], 2);
        assert_eq!(state["head_sha"], "abc123");
    }

    #[test]
    fn tag_pushes_and_branch_deletions_stay_outside_the_vocabulary() {
        let tag = json!({"ref": "refs/tags/v1.0"}).to_string();
        assert!(matches!(
            GitHubAdapter.normalize("push", &tag),
            NormalizedOutcome::Unsupported { .. }
        ));
        let deleted = json!({"ref": "refs/heads/x", "deleted": true}).to_string();
        assert!(matches!(
            GitHubAdapter.normalize("push", &deleted),
            NormalizedOutcome::Unsupported { .. }
        ));
    }

    #[test]
    fn pull_request_actions_split_without_conflating_close_and_merge() {
        let base = json!({
            "sender": {"login": "merger"},
            "repository": {"full_name": "o/r"},
            "pull_request": {
                "number": 7,
                "title": "Add login",
                "state": "closed",
                "merged": false,
                "user": {"login": "author"},
                "head": {"ref": "feature", "sha": "abc"},
                "base": {"ref": "main"},
                "closed_at": "2026-01-02T03:04:05Z",
            },
        });
        let mut closed = base.clone();
        closed["action"] = json!("closed");
        let NormalizedOutcome::Event { event_type, .. } =
            GitHubAdapter.normalize("pull_request", &closed.to_string())
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitPullRequestClosed);

        let mut merged = base.clone();
        merged["action"] = json!("closed");
        merged["pull_request"]["merged"] = json!(true);
        merged["pull_request"]["merged_at"] = json!("2026-01-02T04:04:05Z");
        let NormalizedOutcome::Event {
            event_type, state, ..
        } = GitHubAdapter.normalize("pull_request", &merged.to_string())
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitPullRequestMerged);
        assert_eq!(state["pr_merged"], true);
        assert_eq!(state["pr_number"], 7);
        assert_eq!(state["pr_title"], "Add login");
        assert_eq!(state["pr_author"], "author");
        assert_eq!(state["branch"], "feature");
        assert_eq!(state["base_branch"], "main");

        let mut opened = base.clone();
        opened["action"] = json!("opened");
        opened["pull_request"]["state"] = json!("open");
        let NormalizedOutcome::Event { event_type, .. } =
            GitHubAdapter.normalize("pull_request", &opened.to_string())
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitPullRequestOpened);

        // Recognized but outside the vocabulary: durable unsupported record.
        let mut synced = base.clone();
        synced["action"] = json!("synchronize");
        assert!(matches!(
            GitHubAdapter.normalize("pull_request", &synced.to_string()),
            NormalizedOutcome::Unsupported { .. }
        ));
    }

    #[test]
    fn branch_refs_classify_and_tags_do_not() {
        let create = json!({"ref_type": "branch", "ref": "feature/x", "sender": {"login": "a"}, "repository": {"full_name": "o/r"}}).to_string();
        let NormalizedOutcome::Event {
            event_type, state, ..
        } = GitHubAdapter.normalize("create", &create)
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitBranchCreated);
        assert_eq!(state["branch"], "feature/x");

        let delete =
            json!({"ref_type": "branch", "ref": "feature/x", "sender": {"login": "a"}}).to_string();
        let NormalizedOutcome::Event { event_type, .. } =
            GitHubAdapter.normalize("delete", &delete)
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitBranchDeleted);

        let tag = json!({"ref_type": "tag", "ref": "v1.0"}).to_string();
        assert!(matches!(
            GitHubAdapter.normalize("create", &tag),
            NormalizedOutcome::Unsupported { .. }
        ));
    }

    #[test]
    fn release_published_normalizes_and_edits_do_not() {
        let payload = json!({
            "action": "published",
            "sender": {"login": "releaser"},
            "repository": {"full_name": "o/r"},
            "release": {
                "tag_name": "v2.0.0",
                "name": "Second release",
                "prerelease": true,
                "published_at": "2026-01-02T03:04:05Z",
            },
        })
        .to_string();
        let NormalizedOutcome::Event {
            event_type, state, ..
        } = GitHubAdapter.normalize("release", &payload)
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitReleasePublished);
        assert_eq!(state["release_tag"], "v2.0.0");
        assert_eq!(state["release_name"], "Second release");
        assert_eq!(state["prerelease"], true);

        let edited = json!({"action": "edited", "release": {"tag_name": "v2"}}).to_string();
        assert!(matches!(
            GitHubAdapter.normalize("release", &edited),
            NormalizedOutcome::Unsupported { .. }
        ));
    }

    #[test]
    fn unknown_events_and_invalid_payloads_never_become_events() {
        // Recognized GitHub events outside the vocabulary.
        for event in ["ping", "issues", "workflow_run", "check_run", "star"] {
            assert!(matches!(
                GitHubAdapter.normalize(event, "{}"),
                NormalizedOutcome::Unsupported { .. }
            ));
        }
        // A supported event with a malformed payload is a permanent failure,
        // not an event and not an unsupported record.
        assert!(matches!(
            GitHubAdapter.normalize("push", "not json"),
            NormalizedOutcome::Invalid { .. }
        ));
    }

    #[test]
    fn snapshot_fields_are_bounded() {
        let long_branch = "b".repeat(10_000);
        let payload = json!({
            "ref": format!("refs/heads/{long_branch}"),
            "sender": {"login": "a".repeat(10_000)},
            "repository": {"full_name": "r".repeat(10_000)},
            "commits": [],
        })
        .to_string();
        let NormalizedOutcome::Event { state, .. } = GitHubAdapter.normalize("push", &payload)
        else {
            panic!("expected an event");
        };
        assert_eq!(
            state["branch"].as_str().unwrap().chars().count(),
            FIELD_MAX_CHARS
        );
        assert_eq!(
            state["actor"].as_str().unwrap().chars().count(),
            FIELD_MAX_CHARS
        );
        assert_eq!(
            state["repository"].as_str().unwrap().chars().count(),
            FIELD_MAX_CHARS
        );
    }

    #[test]
    fn timestamps_parse_and_broken_ones_are_rejected() {
        assert_eq!(
            parse_timestamp(Some("2026-01-02T03:04:05Z")),
            Some(1_767_323_045_000)
        );
        assert_eq!(parse_timestamp(Some("not a time")), None);
        assert_eq!(parse_timestamp(None), None);
    }
}
