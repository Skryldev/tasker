//! Consistent error representation mapped to RFC 9457 Problem Details
//! (`application/problem+json`) responses.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// A single field-level validation error.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

/// Application error type. Variants map 1:1 to HTTP problem responses.
/// Internal error details are logged server-side and never returned to clients.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("authentication required")]
    Unauthenticated,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    BadRequest(String),
    #[error("validation failed")]
    Validation(Vec<FieldError>),
    #[error("{0}")]
    Conflict(String),
    #[error("too many requests")]
    RateLimited { retry_after_secs: u64 },
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = %err, "database error");
        match err {
            sqlx::Error::RowNotFound => AppError::NotFound("resource not found"),
            other => AppError::Internal(other.to_string()),
        }
    }
}

impl AppError {
    /// Problem type slug (`https://tasker.dev/problems/<slug>`).
    #[must_use]
    pub fn slug(&self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound(_) => "not-found",
            Self::BadRequest(_) => "bad-request",
            Self::Validation(_) => "validation-failed",
            Self::Conflict(_) => "conflict",
            Self::RateLimited { .. } => "rate-limited",
            Self::Internal(_) => "internal-error",
        }
    }

    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Convenience constructor for a single-field validation error.
    pub fn validation(field: &str, message: impl Into<String>) -> Self {
        Self::Validation(vec![FieldError::new(field, message)])
    }

    /// Logs the internal detail and returns a sanitized internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        let message = message.into();
        tracing::error!(detail = %message, "internal error");
        Self::Internal(message)
    }
}

/// Problem Details body (RFC 9457) with an extension for field errors.
#[derive(Serialize)]
struct Problem {
    #[serde(rename = "type")]
    problem_type: String,
    title: String,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<Vec<FieldError>>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        // Internal error details are logged but never returned to clients.
        let detail = match &self {
            Self::Internal(_) => Some("an unexpected error occurred".to_string()),
            Self::Unauthenticated => Some("authentication is required".to_string()),
            Self::Forbidden(d) => Some((*d).to_string()),
            Self::NotFound(d) => Some((*d).to_string()),
            Self::BadRequest(d) => Some(d.clone()),
            Self::Conflict(d) => Some(d.clone()),
            Self::RateLimited { .. } => Some("request rate limit exceeded".to_string()),
            Self::Validation(_) => Some("request validation failed".to_string()),
        };
        let errors = match &self {
            Self::Validation(fields) => Some(fields.clone()),
            _ => None,
        };
        let problem = Problem {
            problem_type: format!("https://tasker.dev/problems/{}", self.slug()),
            title: status.canonical_reason().unwrap_or("error").to_string(),
            status: status.as_u16(),
            detail,
            errors,
        };

        let mut response = (status, Json(problem)).into_response();
        if let Self::RateLimited { retry_after_secs } = &self {
            if let Ok(value) = retry_after_secs.to_string().parse() {
                response.headers_mut().insert("retry-after", value);
            }
        }
        response
    }
}

/// Non-HTTP bootstrap failure (startup configuration, database init).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);
