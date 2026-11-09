//! Git Integration platform (Phase 8B).
//!
//! Provider-neutral foundation for receiving external Git events: integration
//! management, credential security, repository→project routing with
//! database-enforced tenant isolation, and the durable webhook delivery
//! schema. Webhook intake and delivery processing arrive in Task 2; provider
//! adapters in Tasks 3–4; core event delivery in Task 5.
//!
//! Boundaries:
//! - `credentials` — AES-256-GCM secret envelope over the Phase 7 primitives,
//!   fail-closed key resolution (`TASKER_INTEGRATIONS_KEY`), rotation and
//!   revocation semantics.
//! - `store` — portable persistence for integrations, repositories,
//!   repo→project mappings (database-enforced tenant isolation via composite
//!   foreign keys), credentials, and deliveries.
//!
//! The Rules Engine never imports this module's provider types: normalized
//! events enter the shared `domain_events` store with
//! `origin = 'integration'` (see `rules::events::EventOrigin`).

pub mod credentials;
pub mod store;

use crate::error::FieldError;

/// Supported Git providers. Closed enum: arbitrary provider strings are
/// rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    GitLab,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "github" => Some(Self::GitHub),
            "gitlab" => Some(Self::GitLab),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
        }
    }
}

/// Limits for the integration subsystem (mirrors `rules::limits` style:
/// application-enforced, database columns sized for the MySQL worst case).
pub mod limits {
    /// Maximum integration name length.
    pub const INTEGRATION_NAME_MAX_CHARS: usize = 120;
    /// Maximum integration description length.
    pub const INTEGRATION_DESCRIPTION_MAX_CHARS: usize = 500;
    /// Maximum external repository full-name length (`owner/repo`).
    pub const REPO_FULL_NAME_MAX_CHARS: usize = 255;
    /// Maximum external repository id length (GitHub numeric id, GitLab id).
    pub const EXTERNAL_REPO_ID_MAX_CHARS: usize = 64;
    /// Maximum raw webhook payload length, measured in **characters** for the
    /// stored column. The HTTP byte limit (15,000 **bytes**) is enforced by
    /// the intake in Task 2; this column bound is the storage-side cap.
    pub const WEBHOOK_PAYLOAD_MAX_CHARS: usize = 15_000;
    /// Maximum provider delivery-id length (GitHub GUID, GitLab numeric id).
    pub const DELIVERY_ID_MAX_CHARS: usize = 64;
    /// Maximum external actor (provider login) length.
    pub const EXTERNAL_ACTOR_MAX_CHARS: usize = 255;
    /// Maximum webhook verification secret length (GitHub secrets up to 255
    /// chars; GitLab tokens similar) — enforced on provisioning/rotation.
    pub const WEBHOOK_SECRET_MAX_CHARS: usize = 255;
}

/// Validate and normalize an integration name (trimmed, 1–120 characters).
pub fn validate_integration_name(name: &str) -> Result<String, FieldError> {
    let trimmed = name.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return Err(FieldError::new("name", "integration name must not be empty"));
    }
    if len > limits::INTEGRATION_NAME_MAX_CHARS {
        return Err(FieldError::new(
            "name",
            format!(
                "integration name must be at most {} characters",
                limits::INTEGRATION_NAME_MAX_CHARS
            ),
        ));
    }
    Ok(trimmed.to_string())
}

/// Validate and normalize an integration description (optional, trimmed, at
/// most 500 characters).
pub fn validate_integration_description(description: &str) -> Result<Option<String>, FieldError> {
    let trimmed = description.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let len = trimmed.chars().count();
    if len > limits::INTEGRATION_DESCRIPTION_MAX_CHARS {
        return Err(FieldError::new(
            "description",
            format!(
                "integration description must be at most {} characters",
                limits::INTEGRATION_DESCRIPTION_MAX_CHARS
            ),
        ));
    }
    Ok(Some(trimmed.to_string()))
}

/// Validate an external repository id (provider-assigned; digits for both
/// GitHub and GitLab today, kept permissive to bounded printable values).
pub fn validate_external_repo_id(raw: &str) -> Result<String, FieldError> {
    let trimmed = raw.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return Err(FieldError::new(
            "external_repo_id",
            "external repository id must not be empty",
        ));
    }
    if len > limits::EXTERNAL_REPO_ID_MAX_CHARS {
        return Err(FieldError::new(
            "external_repo_id",
            format!(
                "external repository id must be at most {} characters",
                limits::EXTERNAL_REPO_ID_MAX_CHARS
            ),
        ));
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(FieldError::new(
            "external_repo_id",
            "external repository id must be alphanumeric with `-`/`_` only",
        ));
    }
    Ok(trimmed.to_string())
}

/// Validate an external repository full name (`owner/repo` shape for GitHub,
/// `namespace/project` for GitLab — both bounded to printable non-whitespace
/// segments around `/`).
pub fn validate_repo_full_name(raw: &str) -> Result<String, FieldError> {
    let trimmed = raw.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return Err(FieldError::new(
            "full_name",
            "repository full name must not be empty",
        ));
    }
    if len > limits::REPO_FULL_NAME_MAX_CHARS {
        return Err(FieldError::new(
            "full_name",
            format!(
                "repository full name must be at most {} characters",
                limits::REPO_FULL_NAME_MAX_CHARS
            ),
        ));
    }
    if trimmed
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(FieldError::new(
            "full_name",
            "repository full name must not contain whitespace or control characters",
        ));
    }
    Ok(trimmed.to_string())
}

/// Integration aggregate as persisted (Task 1: managed at the store level;
/// HTTP endpoints arrive with the management API work).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integration {
    pub id: String,
    pub team_id: String,
    pub provider: Provider,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_enum_is_closed() {
        assert_eq!(Provider::parse("github"), Some(Provider::GitHub));
        assert_eq!(Provider::parse("gitlab"), Some(Provider::GitLab));
        assert_eq!(Provider::parse("gitea"), None);
        assert_eq!(Provider::parse("GitHub"), None, "strict lowercase");
    }

    #[test]
    fn name_validation() {
        assert!(validate_integration_name("  repo watcher  ").is_ok());
        assert!(validate_integration_name("").is_err());
        assert!(validate_integration_name("   ").is_err());
        let long = "a".repeat(limits::INTEGRATION_NAME_MAX_CHARS + 1);
        assert!(validate_integration_name(&long).is_err());
        let exact = "a".repeat(limits::INTEGRATION_NAME_MAX_CHARS);
        assert!(validate_integration_name(&exact).is_ok());
    }

    #[test]
    fn description_validation() {
        assert_eq!(
            validate_integration_description("  ").expect("empty is None"),
            None
        );
        assert!(validate_integration_description("watches CI").is_ok());
        let long = "a".repeat(limits::INTEGRATION_DESCRIPTION_MAX_CHARS + 1);
        assert!(validate_integration_description(&long).is_err());
    }

    #[test]
    fn external_repo_id_validation() {
        assert!(validate_external_repo_id("1234567").is_ok());
        assert!(validate_external_repo_id("repo-1_x").is_ok());
        assert!(validate_external_repo_id("").is_err());
        assert!(validate_external_repo_id("has space").is_err());
        let long = "a".repeat(limits::EXTERNAL_REPO_ID_MAX_CHARS + 1);
        assert!(validate_external_repo_id(&long).is_err());
    }

    #[test]
    fn repo_full_name_validation() {
        assert!(validate_repo_full_name("octocat/hello-world").is_ok());
        assert!(validate_repo_full_name("group/sub/project").is_ok());
        assert!(validate_repo_full_name("has space").is_err());
        assert!(validate_repo_full_name("").is_err());
    }
}
