//! GitLab webhook adapter (Phase 8B, Task 4): the provider-specific half of
//! the webhook pipeline for GitLab, implementing the provider-neutral
//! [`ProviderAdapter`] contract (persistence, leases, retries, and worker
//! coordination live in `webhook::delivery`/`webhook::worker`).
//!
//! Responsibilities:
//! - **Verification**: GitLab sends the webhook secret verbatim in
//!   `X-Gitlab-Token`. Comparison is constant-time over SHA-256 digests
//!   (the Phase 7 helper) — ordinary string equality is never used. The
//!   secret is the integration's decrypted credential supplied by the
//!   intake (fail-closed upstream).
//! - **Identity**: `X-Gitlab-Webhook-UUID` (provider delivery id, GitLab
//!   ≥ 13.x) and `X-Gitlab-Event` (event name), both bounded before
//!   persistence.
//! - **Classification**: a CLOSED registry over the current integration
//!   trigger vocabulary. GitLab reports branch creation/deletion inside the
//!   Push Event (all-zero `before`/`after` SHAs — length-agnostic, SHA-256
//!   repositories send 64 zeros), while tags come as a separate Tag Push
//!   Event. Every other event — Issue, Note, Pipeline, Wiki Page, URL Test
//!   (ping), … — is durably recorded as `unsupported`: retained history, no
//!   domain event, no automation.
//! - **Normalization**: bounded, provider-neutral state snapshots using the
//!   same `integration.*` condition-namespace keys as the GitHub adapter, so
//!   the Rules Engine (Task 5) reads one consistent shape for every
//!   provider.

use axum::http::HeaderMap;
use serde_json::{Value, json};

use crate::integrations::Provider;
use crate::integrations::webhook::{
    NormalizedOutcome, ProviderAdapter, RawWebhook, VerificationError, VerifiedDelivery,
};
use crate::rules::events::IntegrationTriggerType;
use crate::util;

const TOKEN_HEADER: &str = "x-gitlab-token";
const DELIVERY_HEADER: &str = "x-gitlab-webhook-uuid";
const EVENT_HEADER: &str = "x-gitlab-event";
/// Bounded length for every string field of a normalized snapshot (mirrors
/// the condition namespace's `Scalar::Text` bound).
const FIELD_MAX_CHARS: usize = 255;

pub struct GitLabAdapter;

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn bounded(s: &str) -> String {
    util::truncate(s, FIELD_MAX_CHARS)
}

fn is_zero_sha(sha: &str) -> bool {
    !sha.is_empty() && sha.chars().all(|c| c == '0')
}

/// Parse a GitLab ISO-8601 timestamp into epoch milliseconds; `None` for
/// absent/unparseable values (the engine's sanitization then substitutes
/// the processing time).
fn parse_timestamp(raw: Option<&str>) -> Option<i64> {
    let raw = raw?;
    let dt =
        time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok()?;
    Some(dt.unix_timestamp() * 1_000 + i64::from(dt.millisecond()))
}

impl ProviderAdapter for GitLabAdapter {
    fn provider(&self) -> Provider {
        Provider::GitLab
    }

    fn verify(
        &self,
        raw: &RawWebhook<'_>,
        secret: &str,
    ) -> Result<VerifiedDelivery, VerificationError> {
        // Token-comparison verification (GitLab's model): the presented token
        // must equal the credential. Constant-time over SHA-256 digests —
        // never ordinary string equality.
        let presented =
            header_str(raw.headers, TOKEN_HEADER).ok_or(VerificationError::MissingSignature)?;
        let presented_hash = crate::security::token::hash(&presented);
        let stored_hash = crate::security::token::hash(secret);
        if !crate::security::token::hashes_equal(&presented_hash, &stored_hash) {
            return Err(VerificationError::InvalidSignature);
        }
        let delivery_id = header_str(raw.headers, DELIVERY_HEADER).ok_or(
            VerificationError::InvalidRequest("missing X-Gitlab-Webhook-UUID (GitLab ≥ 13.x)"),
        )?;
        let event_type_raw = header_str(raw.headers, EVENT_HEADER)
            .ok_or(VerificationError::InvalidRequest("missing X-Gitlab-Event"))?;
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
            "Push Event" => normalize_push(&parsed),
            "Merge Request Event" => normalize_merge_request(&parsed),
            "Release Event" => normalize_release(&parsed),
            _ => NormalizedOutcome::Unsupported {
                reason: "gitlab event outside the supported vocabulary",
            },
        }
    }
}

/// The shared snapshot keys every supported event carries — exactly the
/// `integration.*` condition-namespace fields the Rules Engine (Task 5)
/// evaluates.
fn base_state<'a>(
    parsed: &'a Value,
    event_type: &str,
    actor_pointer: &'a str,
) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    map.insert("provider".into(), json!("gitlab"));
    map.insert("event_type".into(), json!(event_type));
    if let Some(name) = parsed
        .pointer("/project/path_with_namespace")
        .and_then(Value::as_str)
    {
        map.insert("repository".into(), json!(bounded(name)));
    }
    if let Some(actor) = parsed.pointer(actor_pointer).and_then(Value::as_str) {
        map.insert("actor".into(), json!(bounded(actor)));
    }
    map
}

/// `Push Event`: GitLab reports branch creation and deletion inside pushes
/// (all-zero `before`/`after` SHAs, length-agnostic across SHA-1/SHA-256
/// repositories). Tag pushes arrive as a separate event and stay unsupported.
fn normalize_push(parsed: &Value) -> NormalizedOutcome {
    let full_ref = parsed.pointer("/ref").and_then(Value::as_str).unwrap_or("");
    let Some(branch) = full_ref.strip_prefix("refs/heads/") else {
        return NormalizedOutcome::Unsupported {
            reason: "push to a non-branch ref",
        };
    };
    let before = parsed
        .pointer("/before")
        .and_then(Value::as_str)
        .unwrap_or("");
    let after = parsed
        .pointer("/after")
        .and_then(Value::as_str)
        .unwrap_or("");
    let event_type = if is_zero_sha(after) {
        IntegrationTriggerType::GitBranchDeleted
    } else if is_zero_sha(before) {
        // A brand-new branch: created (no parent commit). When commits ride
        // along on the creating push, it is still a creation event.
        IntegrationTriggerType::GitBranchCreated
    } else {
        IntegrationTriggerType::GitPush
    };
    let mut state = base_state(parsed, event_type.as_str(), "/user_name");
    state.insert("branch".into(), json!(bounded(branch)));
    if event_type == IntegrationTriggerType::GitPush {
        let commit_count = parsed
            .pointer("/total_commits_count")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        state.insert("commit_count".into(), json!(commit_count));
        if let Some(sha) = parsed.pointer("/after").and_then(Value::as_str) {
            state.insert("head_sha".into(), json!(bounded(sha)));
        }
    }
    NormalizedOutcome::Event {
        event_type,
        occurred_at: 0, // GitLab push payloads carry no timestamp
        state: Value::Object(state),
    }
}

/// `Merge Request Event`: open/reopen/merge/close map onto the vocabulary;
/// `update`, `approved`, and other actions are durably unsupported.
fn normalize_merge_request(parsed: &Value) -> NormalizedOutcome {
    let action = parsed
        .pointer("/object_attributes/action")
        .and_then(Value::as_str)
        .unwrap_or("");
    let event_type = match action {
        "open" | "reopen" => IntegrationTriggerType::GitPullRequestOpened,
        "merge" => IntegrationTriggerType::GitPullRequestMerged,
        "close" => IntegrationTriggerType::GitPullRequestClosed,
        _ => {
            return NormalizedOutcome::Unsupported {
                reason: "merge request action outside the supported vocabulary",
            };
        }
    };
    let mut state = base_state(parsed, event_type.as_str(), "/user/username");
    if let Some(iid) = parsed
        .pointer("/object_attributes/iid")
        .and_then(Value::as_i64)
    {
        state.insert("pr_number".into(), json!(iid));
    }
    if let Some(title) = parsed
        .pointer("/object_attributes/title")
        .and_then(Value::as_str)
    {
        state.insert("pr_title".into(), json!(bounded(title)));
    }
    if let Some(state_name) = parsed
        .pointer("/object_attributes/state")
        .and_then(Value::as_str)
    {
        state.insert("pr_state".into(), json!(bounded(state_name)));
    }
    state.insert(
        "pr_merged".into(),
        json!(event_type == IntegrationTriggerType::GitPullRequestMerged),
    );
    if let Some(author_id) = parsed
        .pointer("/object_attributes/author_id")
        .and_then(Value::as_i64)
    {
        state.insert("pr_author_id".into(), json!(author_id));
    }
    if let Some(branch) = parsed
        .pointer("/object_attributes/source_branch")
        .and_then(Value::as_str)
    {
        state.insert("branch".into(), json!(bounded(branch)));
    }
    if let Some(base) = parsed
        .pointer("/object_attributes/target_branch")
        .and_then(Value::as_str)
    {
        state.insert("base_branch".into(), json!(bounded(base)));
    }
    if let Some(sha) = parsed
        .pointer("/object_attributes/last_commit/id")
        .and_then(Value::as_str)
    {
        state.insert("head_sha".into(), json!(bounded(sha)));
    }
    let occurred_at = ["updated_at", "created_at"]
        .iter()
        .find_map(|key| {
            parse_timestamp(
                parsed
                    .pointer(&format!("/object_attributes/{key}"))
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

/// `Release Event`: only the `publish` action enters the vocabulary.
fn normalize_release(parsed: &Value) -> NormalizedOutcome {
    if parsed
        .pointer("/object_attributes/action")
        .and_then(Value::as_str)
        != Some("publish")
    {
        return NormalizedOutcome::Unsupported {
            reason: "release action outside the supported vocabulary",
        };
    }
    let mut state = base_state(parsed, "git_release_published", "/user/username");
    if let Some(tag) = parsed
        .pointer("/object_attributes/tag")
        .and_then(Value::as_str)
    {
        state.insert("release_tag".into(), json!(bounded(tag)));
    }
    let occurred_at = parse_timestamp(
        parsed
            .pointer("/object_attributes/released_at")
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
// Tests: verification, closed classification registry, bounds
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    const SECRET: &str = "gl-secret";

    fn raw<'a>(headers: &'a HeaderMap, body: &'a [u8]) -> RawWebhook<'a> {
        RawWebhook {
            integration_id: "int-1",
            headers,
            body,
        }
    }

    fn token_headers(delivery: &str, event: &str, secret: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-token", secret.parse().unwrap());
        headers.insert("x-gitlab-webhook-uuid", delivery.parse().unwrap());
        headers.insert("x-gitlab-event", event.parse().unwrap());
        headers
    }

    #[test]
    fn valid_token_verifies_and_extracts_identity() {
        let headers = token_headers("uuid-1", "Push Event", SECRET);
        let verified = GitLabAdapter
            .verify(&raw(&headers, b"{}"), SECRET)
            .expect("valid token");
        assert_eq!(verified.delivery_id, "uuid-1");
        assert_eq!(verified.event_type_raw, "Push Event");
    }

    #[test]
    fn wrong_or_missing_tokens_fail_closed() {
        let headers = token_headers("uuid-1", "Push Event", "other-token");
        assert_eq!(
            GitLabAdapter.verify(&raw(&headers, b"{}"), SECRET),
            Err(VerificationError::InvalidSignature)
        );
        let headers = HeaderMap::new();
        assert_eq!(
            GitLabAdapter.verify(&raw(&headers, b"{}"), SECRET),
            Err(VerificationError::MissingSignature)
        );
        // Missing delivery UUID: GitLab ≥ 13.x requirement, an invalid request.
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-token", SECRET.parse().unwrap());
        headers.insert("x-gitlab-event", "Push Event".parse().unwrap());
        assert!(matches!(
            GitLabAdapter.verify(&raw(&headers, b"{}"), SECRET),
            Err(VerificationError::InvalidRequest(_))
        ));
    }

    #[test]
    fn push_events_split_creation_deletion_and_commits() {
        // Ordinary push.
        let push = json!({
            "ref": "refs/heads/main",
            "before": "1111",
            "after": "2222",
            "total_commits_count": 4,
            "user_name": "gluser",
            "project": {"path_with_namespace": "group/tasker"},
        })
        .to_string();
        let NormalizedOutcome::Event {
            event_type, state, ..
        } = GitLabAdapter.normalize("Push Event", &push)
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitPush);
        assert_eq!(state["provider"], "gitlab");
        assert_eq!(state["branch"], "main");
        assert_eq!(state["commit_count"], 4);
        assert_eq!(state["repository"], "group/tasker");
        assert_eq!(state["actor"], "gluser");
        assert_eq!(state["head_sha"], "2222");

        // Branch creation (all-zero before, length-agnostic).
        let create = json!({
            "ref": "refs/heads/feature/x",
            "before": "0".repeat(64),
            "after": "abc",
            "user_name": "gluser",
            "project": {"path_with_namespace": "g/r"},
        })
        .to_string();
        let NormalizedOutcome::Event {
            event_type, state, ..
        } = GitLabAdapter.normalize("Push Event", &create)
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitBranchCreated);
        assert_eq!(state["branch"], "feature/x");

        // Branch deletion (all-zero after).
        let delete = json!({
            "ref": "refs/heads/feature/x",
            "before": "abc",
            "after": "0".repeat(40),
            "user_name": "gluser",
        })
        .to_string();
        let NormalizedOutcome::Event { event_type, .. } =
            GitLabAdapter.normalize("Push Event", &delete)
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitBranchDeleted);

        // Tag push is a separate GitLab event and stays unsupported; non-branch
        // push refs do too.
        assert!(matches!(
            GitLabAdapter.normalize("Tag Push Event", "{}"),
            NormalizedOutcome::Unsupported { .. }
        ));
        assert!(matches!(
            GitLabAdapter.normalize("Push Event", &json!({"ref": "refs/tags/v1"}).to_string()),
            NormalizedOutcome::Unsupported { .. }
        ));
    }

    #[test]
    fn merge_request_actions_split_without_conflating_merge_and_close() {
        let base = json!({
            "user": {"username": "merger"},
            "project": {"path_with_namespace": "g/r"},
            "object_attributes": {
                "iid": 9,
                "title": "Fix bug",
                "state": "opened",
                "action": "open",
                "source_branch": "fix/bug",
                "target_branch": "main",
                "last_commit": {"id": "def"},
                "author_id": 7,
                "updated_at": "2026-01-02T03:04:05Z",
            },
        });
        let NormalizedOutcome::Event {
            event_type, state, ..
        } = GitLabAdapter.normalize("Merge Request Event", &base.to_string())
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitPullRequestOpened);
        assert_eq!(state["pr_number"], 9);
        assert_eq!(state["branch"], "fix/bug");
        assert_eq!(state["base_branch"], "main");
        assert_eq!(state["head_sha"], "def");
        assert_eq!(state["pr_author_id"], 7);

        for (action, expected) in [
            ("merge", IntegrationTriggerType::GitPullRequestMerged),
            ("close", IntegrationTriggerType::GitPullRequestClosed),
            ("reopen", IntegrationTriggerType::GitPullRequestOpened),
        ] {
            let mut payload = base.clone();
            payload["object_attributes"]["action"] = json!(action);
            let NormalizedOutcome::Event { event_type, .. } =
                GitLabAdapter.normalize("Merge Request Event", &payload.to_string())
            else {
                panic!("expected an event for {action}");
            };
            assert_eq!(event_type, expected, "{action}");
        }

        let mut update = base.clone();
        update["object_attributes"]["action"] = json!("update");
        assert!(matches!(
            GitLabAdapter.normalize("Merge Request Event", &update.to_string()),
            NormalizedOutcome::Unsupported { .. }
        ));
    }

    #[test]
    fn release_publish_normalizes_and_other_actions_do_not() {
        let publish = json!({
            "user": {"username": "gluser"},
            "object_attributes": {
                "action": "publish",
                "tag": "v1.2.0",
                "released_at": "2026-01-02T03:04:05Z",
            },
        })
        .to_string();
        let NormalizedOutcome::Event {
            event_type, state, ..
        } = GitLabAdapter.normalize("Release Event", &publish)
        else {
            panic!("expected an event");
        };
        assert_eq!(event_type, IntegrationTriggerType::GitReleasePublished);
        assert_eq!(state["release_tag"], "v1.2.0");

        let destroy = json!({"object_attributes": {"action": "destroy", "tag": "v1"}}).to_string();
        assert!(matches!(
            GitLabAdapter.normalize("Release Event", &destroy),
            NormalizedOutcome::Unsupported { .. }
        ));
    }

    #[test]
    fn unknown_events_and_invalid_payloads_never_become_events() {
        for event in [
            "URL Test Event",
            "Issue Event",
            "Note Event",
            "Pipeline Event",
            "Wiki Page Event",
        ] {
            assert!(matches!(
                GitLabAdapter.normalize(event, "{}"),
                NormalizedOutcome::Unsupported { .. }
            ));
        }
        assert!(matches!(
            GitLabAdapter.normalize("Push Event", "not json"),
            NormalizedOutcome::Invalid { .. }
        ));
    }

    #[test]
    fn snapshot_fields_are_bounded() {
        let long = "b".repeat(10_000);
        let payload = json!({
            "ref": format!("refs/heads/{long}"),
            "user_name": long,
            "project": {"path_with_namespace": long},
            "total_commits_count": 1,
        })
        .to_string();
        let NormalizedOutcome::Event { state, .. } =
            GitLabAdapter.normalize("Push Event", &payload)
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
}
