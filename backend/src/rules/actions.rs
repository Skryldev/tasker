//! Typed action model for the Rules Engine.
//!
//! Actions are explicitly typed, validated before any planning can occur,
//! and strictly deserialized (unknown fields rejected). Only the three
//! accepted action types exist:
//!
//! - `add_comment`: automation comment authored by the Automation Principal
//!   (persistence path implemented in Task 4; this module defines the typed
//!   configuration).
//! - `send_notification`: recipients resolved through the existing
//!   notification infrastructure — team-scoped selectors (`assignee`,
//!   `reporter`) and/or explicit team-member ids.
//! - `send_message`: a provider-neutral delivery intent (connection and
//!   destination are opaque references; no URLs, no provider names, no
//!   transport concepts). Phase 8A only ever persists the intent; delivery
//!   belongs to a future integration service.
//!
//! Action bodies are literal text in Phase 8A — no templating. Templated
//! rendering is a deferred capability and must not be silently introduced.
//!
//! JSON grammar (shape-keyed, like the condition DSL):
//!
//! ```json
//! {"body": "text"}
//! {"recipients": {"selectors": ["assignee"], "users": ["<uuid>"]}}
//! {"message": {"connection_ref": "c1", "destination_ref": "d1",
//!              "format": "plain_text", "body": "text"}}
//! ```
//!
//! A rule's action list is a JSON array of 1..=5 entries; execution order is
//! declaration order (deterministic planning).

use serde::Deserialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::error::FieldError;

use super::RuleError;
use super::limits;

// ---------------------------------------------------------------------------
// Typed model
// ---------------------------------------------------------------------------

/// A validated action. Constructed only through [`ActionDef::list_from_json_str`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionDef {
    AddComment { body: String },
    SendNotification { recipients: Recipients },
    SendMessage { message: MessageIntent },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipientSelector {
    Assignee,
    Reporter,
}

impl RecipientSelector {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "assignee" => Some(Self::Assignee),
            "reporter" => Some(Self::Reporter),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assignee => "assignee",
            Self::Reporter => "reporter",
        }
    }
}

/// Notification recipients: team-scoped dynamic selectors and/or explicit
/// team-member ids. Membership is re-verified at execution time; departed
/// members are skipped, not failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipients {
    pub selectors: Vec<RecipientSelector>,
    /// Explicit recipient user ids (normalized canonical UUIDs).
    pub users: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageFormat {
    PlainText,
    MarkdownLike,
}

impl MessageFormat {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "plain_text" => Some(Self::PlainText),
            "markdown_like" => Some(Self::MarkdownLike),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlainText => "plain_text",
            Self::MarkdownLike => "markdown_like",
        }
    }
}

/// A provider-neutral message intent. Connection and destination are opaque
/// references resolved by the future integration service; Tasker Core never
/// resolves them to endpoints (no URL fields exist anywhere in the contract,
/// which is what makes the delivery path SSRF-free by construction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageIntent {
    pub connection_ref: String,
    pub destination_ref: String,
    pub format: MessageFormat,
    pub body: String,
}

// ---------------------------------------------------------------------------
// Raw (strict) deserialization
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddCommentRaw {
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipientsRaw {
    #[serde(default)]
    selectors: Vec<String>,
    #[serde(default)]
    users: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendNotificationRaw {
    recipients: RecipientsRaw,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageIntentRaw {
    connection_ref: String,
    destination_ref: String,
    format: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendMessageRaw {
    message: MessageIntentRaw,
}

/// Untagged: each variant's shape is its own discriminator; unknown keys
/// fail every variant (strict). Note: serde's `deny_unknown_fields` does not
/// compose with internally-tagged enums, which is why the grammar is
/// shape-keyed rather than tagged with a `type` field.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ActionRaw {
    AddComment(AddCommentRaw),
    SendNotification(SendNotificationRaw),
    SendMessage(SendMessageRaw),
}

// ---------------------------------------------------------------------------
// Validation (raw -> typed)
// ---------------------------------------------------------------------------

fn validate_body(body: &str) -> Result<String, FieldError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(FieldError::new("actions", "action body must not be empty"));
    }
    if trimmed.chars().count() > limits::AUTOMATION_BODY_MAX_CHARS {
        return Err(FieldError::new(
            "actions",
            format!(
                "action body exceeds the maximum of {} characters",
                limits::AUTOMATION_BODY_MAX_CHARS
            ),
        ));
    }
    // Stored as authored; persistence-level validation re-trims.
    Ok(body.to_string())
}

fn validate_ref(value: &str, what: &str) -> Result<String, FieldError> {
    let valid = !value.is_empty()
        && value.chars().count() <= limits::OPAQUE_REF_MAX_CHARS
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    if !valid {
        return Err(FieldError::new(
            "actions",
            format!(
                "{what} must be 1-{} characters of [A-Za-z0-9._-]",
                limits::OPAQUE_REF_MAX_CHARS
            ),
        ));
    }
    Ok(value.to_string())
}

fn validate_recipients(raw: &RecipientsRaw) -> Result<Recipients, FieldError> {
    let fe = |message: String| FieldError::new("actions", message);
    let mut selectors = Vec::new();
    for s in &raw.selectors {
        let parsed = RecipientSelector::parse(s)
            .ok_or_else(|| fe(format!("unknown recipient selector `{s}`")))?;
        if selectors.contains(&parsed) {
            return Err(fe(format!("duplicate recipient selector `{s}`")));
        }
        selectors.push(parsed);
    }
    let mut users = Vec::new();
    for u in &raw.users {
        let normalized = Uuid::parse_str(u)
            .map(|id| id.to_string())
            .map_err(|_| fe(format!("malformed recipient UUID `{u}`")))?;
        if users.contains(&normalized) {
            return Err(fe(format!("duplicate recipient UUID `{u}`")));
        }
        users.push(normalized);
    }
    if users.len() > limits::NOTIFICATION_USERS_MAX {
        return Err(fe(format!(
            "notification action exceeds the maximum of {} explicit users",
            limits::NOTIFICATION_USERS_MAX
        )));
    }
    if selectors.is_empty() && users.is_empty() {
        return Err(fe(
            "notification action requires at least one selector or explicit user".to_string(),
        ));
    }
    Ok(Recipients { selectors, users })
}

fn validate_message(raw: &MessageIntentRaw) -> Result<MessageIntent, FieldError> {
    let connection_ref = validate_ref(&raw.connection_ref, "connection_ref")?;
    let destination_ref = validate_ref(&raw.destination_ref, "destination_ref")?;
    let format = MessageFormat::parse(&raw.format).ok_or_else(|| {
        FieldError::new(
            "actions",
            format!("unknown message format `{}`", raw.format),
        )
    })?;
    let body = validate_body(&raw.body)?;
    Ok(MessageIntent {
        connection_ref,
        destination_ref,
        format,
        body,
    })
}

fn validate_action(raw: ActionRaw) -> Result<ActionDef, FieldError> {
    match raw {
        ActionRaw::AddComment(r) => {
            validate_body(&r.body).map(|body| ActionDef::AddComment { body })
        }
        ActionRaw::SendNotification(r) => validate_recipients(&r.recipients)
            .map(|recipients| ActionDef::SendNotification { recipients }),
        ActionRaw::SendMessage(r) => {
            validate_message(&r.message).map(|message| ActionDef::SendMessage { message })
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

impl ActionDef {
    /// Parse and fully validate a serialized action list (a JSON array of
    /// 1..=[`limits::ACTIONS_MAX`] entries). The serialized-size gate runs
    /// before parsing.
    pub fn list_from_json_str(json: &str) -> Result<Vec<Self>, RuleError> {
        if json.chars().count() > limits::ACTIONS_JSON_MAX_CHARS {
            return Err(RuleError(vec![FieldError::new(
                "actions",
                format!(
                    "actions JSON exceeds the maximum serialized size of {} characters",
                    limits::ACTIONS_JSON_MAX_CHARS
                ),
            )]));
        }
        let raw: Vec<ActionRaw> = serde_json::from_str(json)
            .map_err(|e| RuleError(vec![FieldError::new("actions", e.to_string())]))?;
        if raw.is_empty() {
            return Err(RuleError(vec![FieldError::new(
                "actions",
                "a rule requires at least one action",
            )]));
        }
        if raw.len() > limits::ACTIONS_MAX {
            return Err(RuleError(vec![FieldError::new(
                "actions",
                format!("a rule allows at most {} actions", limits::ACTIONS_MAX),
            )]));
        }
        raw.into_iter()
            .map(validate_action)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| RuleError(vec![e]))
    }

    /// Canonical JSON representation used for storage and rule versions.
    #[must_use]
    pub fn to_json_value(&self) -> Value {
        match self {
            Self::AddComment { body } => Value::Object(single("body", Value::String(body.clone()))),
            Self::SendNotification { recipients } => {
                let mut inner = Map::new();
                inner.insert(
                    "selectors".into(),
                    Value::Array(
                        recipients
                            .selectors
                            .iter()
                            .map(|s| Value::String(s.as_str().into()))
                            .collect(),
                    ),
                );
                if !recipients.users.is_empty() {
                    inner.insert(
                        "users".into(),
                        Value::Array(
                            recipients
                                .users
                                .iter()
                                .map(|u| Value::String(u.clone()))
                                .collect(),
                        ),
                    );
                }
                Value::Object(single("recipients", Value::Object(inner)))
            }
            Self::SendMessage { message } => {
                let mut inner = Map::new();
                inner.insert(
                    "connection_ref".into(),
                    Value::String(message.connection_ref.clone()),
                );
                inner.insert(
                    "destination_ref".into(),
                    Value::String(message.destination_ref.clone()),
                );
                inner.insert(
                    "format".into(),
                    Value::String(message.format.as_str().into()),
                );
                inner.insert("body".into(), Value::String(message.body.clone()));
                Value::Object(single("message", Value::Object(inner)))
            }
        }
    }

    /// Canonical JSON for the whole list (storage form).
    #[must_use]
    pub fn list_to_json_str(actions: &[Self]) -> String {
        let value = Value::Array(actions.iter().map(ActionDef::to_json_value).collect());
        serde_json::to_string(&value).expect("serializing validated actions cannot fail")
    }
}

fn single(key: &str, value: Value) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert(key.into(), value);
    map
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(body: &str) -> String {
        format!("{{\"body\":\"{body}\"}}")
    }

    fn parse_list(json: &str) -> Vec<ActionDef> {
        ActionDef::list_from_json_str(json).expect("valid actions")
    }

    /// Parse a single action object wrapped into the required array form.
    fn one(json: &str) -> Vec<ActionDef> {
        parse_list(&format!("[{json}]"))
    }

    #[test]
    fn parses_add_comment() {
        let actions = one(&comment("triage this"));
        assert_eq!(
            actions,
            vec![ActionDef::AddComment {
                body: "triage this".into()
            }]
        );
    }

    #[test]
    fn parses_notification_with_selectors_and_users() {
        let json = "{\"recipients\":{\"selectors\":[\"assignee\",\"reporter\"],\
                    \"users\":[\"00000000-0000-0000-0000-00000000000A\"]}}";
        let actions = one(json);
        assert_eq!(
            actions,
            vec![ActionDef::SendNotification {
                recipients: Recipients {
                    selectors: vec![RecipientSelector::Assignee, RecipientSelector::Reporter],
                    users: vec!["00000000-0000-0000-0000-00000000000a".into()],
                },
            }]
        );
    }

    #[test]
    fn parses_send_message_intent() {
        let json = "{\"message\":{\"connection_ref\":\"conn-1\",\
                    \"destination_ref\":\"dest.1_a\",\"format\":\"markdown_like\",\
                    \"body\":\"deploy done\"}}";
        let actions = one(json);
        assert_eq!(
            actions,
            vec![ActionDef::SendMessage {
                message: MessageIntent {
                    connection_ref: "conn-1".into(),
                    destination_ref: "dest.1_a".into(),
                    format: MessageFormat::MarkdownLike,
                    body: "deploy done".into(),
                },
            }]
        );
    }

    #[test]
    fn rejects_unknown_variant_shapes_and_keys() {
        for json in [
            "{\"kind\":\"add_comment\"}",
            "{\"body\":\"x\",\"junk\":1}",
            "{\"recipients\":{},\"junk\":1}",
            "{\"message\":{\"connection_ref\":\"c\",\"destination_ref\":\"d\",\"format\":\"plain_text\",\"body\":\"b\",\"junk\":1}}",
            "[{\"body\":\"x\"},{\"unknown\":1}]",
        ] {
            assert!(ActionDef::list_from_json_str(json).is_err(), "{json}");
        }
    }

    #[test]
    fn rejects_empty_body_and_oversized_body() {
        // Whitespace-only body: per-item validation rejects it (small
        // serialized form passes the gate).
        let err = ActionDef::list_from_json_str(&format!("[{}]", comment("   ")))
            .expect_err("whitespace body rejected");
        assert!(err.0[0].message.contains("must not be empty"));
        // Oversized body: the serialized gate fires first.
        let long = "a".repeat(limits::AUTOMATION_BODY_MAX_CHARS + 1);
        let err = ActionDef::list_from_json_str(&format!("[{}]", comment(&long)))
            .expect_err("oversized body rejected");
        assert!(err.0[0].message.contains("maximum serialized size"));
    }

    #[test]
    fn body_cap_composes_with_serialized_cap() {
        // A 1900-char body fits the aggregate serialized cap.
        let body = "a".repeat(1_900);
        assert_eq!(one(&comment(&body)).len(), 1);
        // A maximum-length body cannot fit: the array envelope pushes the
        // serialized form past the 2000-char cap, which is enforced before
        // per-item validation. The per-item cap guards render paths
        // (Task 4+), not parsing.
        let exact = "a".repeat(limits::AUTOMATION_BODY_MAX_CHARS);
        let wrapped = format!("[{}]", comment(&exact));
        assert!(wrapped.chars().count() > limits::ACTIONS_JSON_MAX_CHARS);
        let err = ActionDef::list_from_json_str(&wrapped).expect_err("aggregate cap rejected");
        assert!(err.0[0].message.contains("maximum serialized size"));
    }

    #[test]
    fn rejects_invalid_recipients() {
        for json in [
            // no recipients at all
            "{\"recipients\":{\"selectors\":[],\"users\":[]}}",
            // unknown selector
            "{\"recipients\":{\"selectors\":[\"owner\"]}}",
            // duplicate selector
            "{\"recipients\":{\"selectors\":[\"assignee\",\"assignee\"]}}",
            // malformed uuid
            "{\"recipients\":{\"users\":[\"not-a-uuid\"]}}",
            // duplicate uuid
            "{\"recipients\":{\"users\":[\"00000000-0000-0000-0000-00000000000a\",\"00000000-0000-0000-0000-00000000000A\"]}}",
        ] {
            assert!(ActionDef::list_from_json_str(json).is_err(), "{json}");
        }
    }

    #[test]
    fn rejects_oversized_recipient_list() {
        let users: Vec<String> = (0..=10)
            .map(|i| {
                let hex = format!("{i:012x}");
                format!("\"00000000-0000-0000-0000-{hex}\"")
            })
            .collect();
        let json = format!("{{\"recipients\":{{\"users\":[{}]}}}}", users.join(","));
        assert!(ActionDef::list_from_json_str(&json).is_err());
    }

    #[test]
    fn rejects_invalid_message_refs_and_format() {
        for json in [
            "{\"message\":{\"connection_ref\":\"\",\"destination_ref\":\"d\",\"format\":\"plain_text\",\"body\":\"b\"}}",
            "{\"message\":{\"connection_ref\":\"c d\",\"destination_ref\":\"d\",\"format\":\"plain_text\",\"body\":\"b\"}}",
            "{\"message\":{\"connection_ref\":\"c\",\"destination_ref\":\"d\",\"format\":\"html\",\"body\":\"b\"}}",
            // 65-character ref
            "{\"message\":{\"connection_ref\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"destination_ref\":\"d\",\"format\":\"plain_text\",\"body\":\"b\"}}",
        ] {
            assert!(ActionDef::list_from_json_str(json).is_err(), "{json}");
        }
    }

    #[test]
    fn enforces_action_count_and_serialized_cap() {
        let one = comment("x");
        assert!(ActionDef::list_from_json_str(&format!("[{one}]")).is_ok());
        // 0 actions
        assert!(ActionDef::list_from_json_str("[]").is_err());
        // 6 actions
        let list: Vec<&str> = (0..6).map(|_| one.as_str()).collect();
        assert!(ActionDef::list_from_json_str(&format!("[{}]", list.join(","))).is_err());
        // serialized cap
        let long = "a".repeat(limits::ACTIONS_JSON_MAX_CHARS);
        assert!(ActionDef::list_from_json_str(&format!("[{long}]")).is_err());
    }

    #[test]
    fn canonical_serialization_round_trips() {
        let json = "[{\"body\": \"hello\"}, {\"recipients\": {\"selectors\": [\"assignee\"]}}, {\"message\": {\"connection_ref\": \"c1\", \"destination_ref\": \"d1\", \"format\": \"plain_text\", \"body\": \"b\"}}]";
        let actions = parse_list(json);
        let canonical = ActionDef::list_to_json_str(&actions);
        let reparsed = ActionDef::list_from_json_str(&canonical).expect("canonical reparses");
        assert_eq!(actions, reparsed);
        // Users omitted when empty; canonical form is compact.
        assert!(!canonical.contains("\"users\""));
        assert!(canonical.chars().count() < json.chars().count());
    }

    #[test]
    fn declaration_order_is_preserved() {
        let json = "[{\"body\":\"first\"},{\"recipients\":{\"selectors\":[\"reporter\"]}}]";
        let actions = parse_list(json);
        assert!(matches!(actions[0], ActionDef::AddComment { .. }));
        assert!(matches!(actions[1], ActionDef::SendNotification { .. }));
        let canonical = ActionDef::list_to_json_str(&actions);
        let body_pos = canonical.find("\"body\"").expect("body present");
        let sel_pos = canonical.find("\"selectors\"").expect("selectors present");
        assert!(body_pos < sel_pos, "order must round-trip");
    }
}
