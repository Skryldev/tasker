//! Tasker — self-hosted team work management platform (backend).
//!
//! Modular monolith: HTTP transport (`http`), application services
//! (`auth`, `teams`, `projects`, `tasks`, `comments`, `activity`),
//! authorization (`authorization`), and infrastructure (`db`, `security`).

pub mod activity;
pub mod attachments;
pub mod auth;
pub mod authorization;
pub mod avatar;
pub mod checklists;
pub mod comments;
pub mod config;
pub mod dashboard;
pub mod db;
pub mod error;
pub mod goals;
pub mod http;
pub mod mfa;
pub mod notifications;
pub mod profile;
pub mod projects;
pub mod rules;
pub mod security;
pub mod state;
pub mod tasks;
pub mod teams;
pub mod users;
pub mod workspace;

pub mod util {
    //! Small shared helpers (time, ids, validation).

    use std::time::{SystemTime, UNIX_EPOCH};
    use time::macros::format_description;
    use uuid::Uuid;

    /// Current unix epoch milliseconds (UTC).
    #[must_use]
    pub fn now_ms() -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
            Err(_) => 0,
        }
    }

    /// Generate a new `UUIDv7` identifier (time-ordered) as a hyphenated string.
    #[must_use]
    pub fn new_id() -> String {
        Uuid::now_v7().to_string()
    }

    /// Validate an ISO-8601 calendar date (`YYYY-MM-DD`).
    pub fn parse_iso_date(s: &str) -> Result<time::Date, String> {
        time::Date::parse(s, &format_description!("[year]-[month]-[day]"))
            .map_err(|_| format!("invalid date `{s}`, expected YYYY-MM-DD"))
    }

    /// Validate an email address (pragmatic Phase 1 check: one `@`, non-empty
    /// local part and domain, no whitespace, bounded length). Full RFC 5322
    /// validation is intentionally out of scope.
    pub fn validate_email(email: &str) -> Result<(), String> {
        let email = email.trim().to_lowercase();
        // The Automation Principal identity (Phase 8A) must never be held by
        // a real account: registration, admin provisioning, and bootstrap all
        // route through this choke point.
        if email == crate::rules::actions::AUTOMATION_PRINCIPAL_EMAIL {
            return Err("this email address is reserved".into());
        }
        if email.len() < 3 || email.len() > 255 {
            return Err("email must be 3-255 characters".into());
        }
        let Some((local, domain)) = email.split_once('@') else {
            return Err("email must contain an `@`".into());
        };
        if local.is_empty() || domain.is_empty() || domain.split('.').any(str::is_empty) {
            return Err("email local part and domain must be non-empty".into());
        }
        if email.chars().any(char::is_whitespace) {
            return Err("email must not contain whitespace".into());
        }
        Ok(())
    }

    /// Escape user input for use inside a `LIKE` pattern. The escape char is
    /// `|` — deliberately *not* backslash, which `MySQL` treats as a string
    /// escape even inside literals.
    #[must_use]
    pub fn escape_like(input: &str) -> String {
        let mut out = String::with_capacity(input.len() + 2);
        for c in input.chars() {
            if matches!(c, '|' | '%' | '_') {
                out.push('|');
            }
            out.push(c);
        }
        out
    }

    /// Today's date as an ISO `YYYY-MM-DD` string (UTC), for overdue checks
    /// against the TEXT `due_date` column.
    #[must_use]
    pub fn parse_today_iso() -> String {
        let now = time::OffsetDateTime::now_utc().date();
        format!(
            "{:04}-{:02}-{:02}",
            now.year(),
            u8::from(now.month()),
            now.day()
        )
    }

    /// Truncate a string at a char boundary (used to bound stored metadata).
    #[must_use]
    pub fn truncate(s: &str, max_chars: usize) -> String {
        s.chars().take(max_chars).collect()
    }
}
