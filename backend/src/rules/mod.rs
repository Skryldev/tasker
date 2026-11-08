//! Rules Engine (Phase 8A).
//!
//! A secure, deterministic, database-portable automation engine inside the
//! existing modular monolith. Boundaries:
//!
//! - [`limits`] — every engine limit, centralized.
//! - [`conditions`] — typed condition DSL: strict parsing, validation, pure
//!   evaluator.
//! - [`actions`] — typed action model: add-comment, send-notification,
//!   provider-neutral send-message intent.
//! - [`events`] — domain events, trigger matrix, loop prevention.
//!
//! Persistence (`rule_executions`, outbox), the worker, and the management
//! API land in Tasks 2-6 and extend this module tree; nothing here depends
//! on them. The engine never touches authentication, sessions, or MFA
//! internals: authorization happens at the API edge via the existing
//! capability model, and automation writes use the dedicated Automation
//! Principal (Task 4).

pub mod actions;
pub mod api;
pub mod conditions;
pub mod dispatcher;
pub mod engine;
pub mod events;
pub mod limits;
pub mod outbox;

use crate::error::FieldError;

pub use actions::{ActionDef, MessageFormat, MessageIntent, RecipientSelector, Recipients};
pub use conditions::{CmpOp, Compare, Condition, EvalContext, Field, Operand, Scalar};
pub use events::{DomainEvent, EventOrigin, EventState, TriggerType};

/// Validation failure(s) for rule input. Field errors follow the repo's
/// `FieldError` convention so API handlers can surface them in the
/// problem+json `errors[]` extension without translation.
#[derive(Debug, Clone)]
pub struct RuleError(pub Vec<FieldError>);

impl From<FieldError> for RuleError {
    fn from(fe: FieldError) -> Self {
        Self(vec![fe])
    }
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let messages: Vec<String> = self
            .0
            .iter()
            .map(|fe| format!("{}: {}", fe.field, fe.message))
            .collect();
        write!(f, "{}", messages.join("; "))
    }
}

/// The rule's versioned definition: trigger, condition, and actions.
/// Persisted immutably per version (`rule_versions`, Task 2); the `rules`
/// row carries only the current version number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDefinition {
    pub trigger: TriggerType,
    pub condition: Condition,
    pub actions: Vec<ActionDef>,
}

impl RuleDefinition {
    /// Parse a definition from its two serialized parts. Both size gates and
    /// all structural validations run here; a definition that parses has
    /// bounded evaluation cost by construction.
    pub fn from_json(
        trigger: TriggerType,
        condition_json: &str,
        actions_json: &str,
    ) -> Result<Self, RuleError> {
        let condition = Condition::from_json_str(condition_json)?;
        let actions = ActionDef::list_from_json_str(actions_json)?;
        Ok(Self {
            trigger,
            condition,
            actions,
        })
    }

    /// Canonical serialized condition for storage.
    #[must_use]
    pub fn condition_json(&self) -> String {
        self.condition.to_json_str()
    }

    /// Canonical serialized actions for storage.
    #[must_use]
    pub fn actions_json(&self) -> String {
        ActionDef::list_to_json_str(&self.actions)
    }
}

/// A rule as an aggregate: management metadata plus the current definition.
/// Versions live in `rule_versions` (Task 2); `version` here is the current
/// number, bumped with optimistic concurrency control on every edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub version: u32,
    pub definition: RuleDefinition,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}

/// Validate and normalize a rule name (trimmed, 1-120 characters).
pub fn validate_name(name: &str) -> Result<String, FieldError> {
    let trimmed = name.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return Err(FieldError::new("name", "rule name must not be empty"));
    }
    if len > limits::RULE_NAME_MAX_CHARS {
        return Err(FieldError::new(
            "name",
            format!(
                "rule name must be at most {} characters",
                limits::RULE_NAME_MAX_CHARS
            ),
        ));
    }
    Ok(trimmed.to_string())
}

/// Validate and normalize a rule description (optional, trimmed, at most 500
/// characters).
pub fn validate_description(description: &str) -> Result<Option<String>, FieldError> {
    let trimmed = description.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let len = trimmed.chars().count();
    if len > limits::RULE_DESCRIPTION_MAX_CHARS {
        return Err(FieldError::new(
            "description",
            format!(
                "rule description must be at most {} characters",
                limits::RULE_DESCRIPTION_MAX_CHARS
            ),
        ));
    }
    Ok(Some(trimmed.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const CONDITION: &str = "{\"field\":\"task.status\",\"op\":\"EQ\",\"value\":\"todo\"}";
    const ACTIONS: &str = "[{\"body\":\"triage\"}]";

    #[test]
    fn parses_a_full_definition() {
        let def = RuleDefinition::from_json(TriggerType::TaskStatusChanged, CONDITION, ACTIONS)
            .expect("valid definition");
        assert_eq!(def.trigger, TriggerType::TaskStatusChanged);
        assert_eq!(def.actions.len(), 1);
        // Canonical round trip.
        let reparsed =
            RuleDefinition::from_json(def.trigger, &def.condition_json(), &def.actions_json())
                .expect("canonical reparses");
        assert_eq!(def, reparsed);
    }

    #[test]
    fn condition_errors_are_field_errors() {
        let err = RuleDefinition::from_json(
            TriggerType::TaskStatusChanged,
            "{\"field\":\"task.title\",\"op\":\"EQ\",\"value\":\"x\"}",
            ACTIONS,
        )
        .unwrap_err();
        assert_eq!(err.0[0].field, "condition");
        assert!(err.to_string().contains("condition"));
    }

    #[test]
    fn action_errors_are_field_errors() {
        let err =
            RuleDefinition::from_json(TriggerType::TaskAssigned, CONDITION, "[]").unwrap_err();
        assert_eq!(err.0[0].field, "actions");
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("  rotate triage  ").is_ok());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("").is_err());
        let long = "a".repeat(limits::RULE_NAME_MAX_CHARS + 1);
        assert!(validate_name(&long).is_err());
        let exact = "a".repeat(limits::RULE_NAME_MAX_CHARS);
        assert!(validate_name(&exact).is_ok());
    }

    #[test]
    fn description_validation() {
        assert_eq!(validate_description("  ").expect("empty is None"), None);
        assert_eq!(
            validate_description("auto-triage").expect("some"),
            Some("auto-triage".to_string())
        );
        let long = "a".repeat(limits::RULE_DESCRIPTION_MAX_CHARS + 1);
        assert!(validate_description(&long).is_err());
        let exact = "a".repeat(limits::RULE_DESCRIPTION_MAX_CHARS);
        assert!(validate_description(&exact).is_ok());
    }

    #[test]
    fn unicode_char_counts_not_bytes() {
        // 60 two-byte characters = 120 bytes but only 60 chars: within cap.
        let multibyte = "é".repeat(60);
        assert!(validate_name(&multibyte).is_ok());
        // 121 one-byte characters exceed the char cap.
        let ascii_long = "a".repeat(limits::RULE_NAME_MAX_CHARS + 1);
        assert!(validate_name(&ascii_long).is_err());
    }
}
