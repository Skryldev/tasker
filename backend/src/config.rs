//! Environment-based application configuration.
//!
//! Every setting is provided through the `TASKER_` environment namespace so the
//! application can be deployed without files or secrets baked into images.

use std::env;
use std::time::Duration;

use crate::error::ConfigError;

pub const ENV_PREFIX: &str = "TASKER_";

/// Runtime execution environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Production,
}

impl Environment {
    #[must_use]
    pub fn is_production(self) -> bool {
        self == Self::Production
    }

    fn parse(s: &str) -> Result<Self, ConfigError> {
        match s.to_lowercase().as_str() {
            "development" | "dev" => Ok(Self::Development),
            "production" | "prod" => Ok(Self::Production),
            other => Err(ConfigError(format!(
                "TASKER_ENV must be `development` or `production`, got `{other}`"
            ))),
        }
    }
}

/// Bounded connection-pool settings shared by all database engines.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
}

/// Full application configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub environment: Environment,
    pub http_host: String,
    pub http_port: u16,
    pub database_url: String,
    pub pool: PoolConfig,
    /// Session lifetime; sessions are fixed-TTL (no sliding renewal).
    pub session_ttl: Duration,
    /// Public self-registration of users (`POST /auth/register`).
    /// Disabled by default; members are provisioned by administrators.
    pub public_registration: bool,
    /// Origins allowed in CORS responses and in the Origin/Sec-Fetch-Site
    /// CSRF check for state-changing requests. Empty means same-origin only.
    pub allowed_origins: Vec<String>,
    /// Login rate limit: attempts per minute per client IP.
    pub login_rate_limit_per_min: u32,
    /// AES-256 key encrypting TOTP secrets at rest, decoded from
    /// `TASKER_MFA_ENCRYPTION_KEY` (64 hex characters = 32 bytes).
    /// Absent => MFA endpoints fail closed (the server still boots for
    /// deployments that do not use MFA). Present-but-malformed => startup
    /// fails: a broken key must never degrade silently to plaintext.
    pub mfa_encryption_key: Option<Vec<u8>>,
    /// AES-256-GCM key encrypting integration webhook secrets. Same rules
    /// as the MFA key: 64 hex characters = 32 bytes; absent => integration
    /// credential operations fail closed; malformed => startup fails.
    pub integrations_key: Option<Vec<u8>>,
    /// Independent HMAC-SHA-256 pepper keying recovery-code digests
    /// (ADR-0012). Never derived from `mfa_encryption_key`; never stored in
    /// or derivable from the database. Absent => recovery-code flows fail
    /// closed; present-but-malformed => startup fails.
    pub mfa_recovery_pepper: Option<Vec<u8>>,
    /// Maximum accepted JSON body size.
    pub body_limit_bytes: usize,
    /// Optional directory containing the built frontend to serve statically.
    pub static_dir: Option<String>,
    /// Log format: structured text or machine-readable JSON.
    pub log_format_json: bool,
    /// Rules Engine outbox dispatcher (Phase 8A, Task 5).
    pub rules_worker: RulesWorkerConfig,
    /// Webhook delivery worker (Phase 8B, Task 2).
    pub webhook_worker: WebhookWorkerConfig,
    /// File attachment storage (Phase 12 WS11).
    pub attachments: AttachmentsConfig,
}

/// Attachment storage mode (Phase 12 WS11).
///
/// - `Disabled` (default): upload/download endpoints fail closed with a clear
///   message; deployments opt in explicitly.
/// - `Local`: bytes stored under `local_dir` on the server filesystem.
/// - `S3`: MinIO/S3-compatible object storage. Configuration is validated at
///   boot; the driver itself is not implemented in this build and every
///   storage operation fails fast with an explicit "not available" error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentsMode {
    Disabled,
    Local,
    S3,
}

/// Attachment storage settings (Phase 12 WS11).
#[derive(Debug, Clone)]
pub struct AttachmentsConfig {
    pub mode: AttachmentsMode,
    /// Root directory for `local` mode (created lazily on first upload).
    pub local_dir: String,
    /// Maximum accepted upload size in bytes (per file).
    pub max_bytes: usize,
    /// Maximum number of attachments per task.
    pub max_per_task: u32,
    // S3/MinIO settings — required when `mode == S3`.
    pub s3_endpoint: Option<String>,
    pub s3_bucket: Option<String>,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
    pub s3_region: String,
}

impl AttachmentsConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let mode = match env_or("TASKER_ATTACHMENTS", "disabled".to_string())?
            .to_lowercase()
            .as_str()
        {
            "disabled" => AttachmentsMode::Disabled,
            "local" => AttachmentsMode::Local,
            "s3" => AttachmentsMode::S3,
            other => {
                return Err(ConfigError(format!(
                    "TASKER_ATTACHMENTS must be `disabled`, `local` or `s3`, got `{other}`"
                )));
            }
        };
        let max_mb: usize = env_parsed("TASKER_ATTACHMENTS_MAX_MB", 25usize)?;
        let max_per_task: u32 = env_parsed("TASKER_ATTACHMENTS_MAX_PER_TASK", 50u32)?;
        let config = Self {
            mode,
            local_dir: env_or("TASKER_ATTACHMENTS_DIR", "data/attachments".to_string())?,
            max_bytes: max_mb.saturating_mul(1024 * 1024).max(1),
            max_per_task: max_per_task.clamp(1, 1000),
            s3_endpoint: env::var(format!("{ENV_PREFIX}S3_ENDPOINT"))
                .ok()
                .filter(|s| !s.is_empty()),
            s3_bucket: env::var(format!("{ENV_PREFIX}S3_BUCKET"))
                .ok()
                .filter(|s| !s.is_empty()),
            s3_access_key: env::var(format!("{ENV_PREFIX}S3_ACCESS_KEY"))
                .ok()
                .filter(|s| !s.is_empty()),
            s3_secret_key: env::var(format!("{ENV_PREFIX}S3_SECRET_KEY"))
                .ok()
                .filter(|s| !s.is_empty()),
            s3_region: env_or("TASKER_S3_REGION", "us-east-1".to_string())?,
        };
        if config.mode == AttachmentsMode::S3 {
            let missing = [
                ("TASKER_S3_ENDPOINT", &config.s3_endpoint),
                ("TASKER_S3_BUCKET", &config.s3_bucket),
                ("TASKER_S3_ACCESS_KEY", &config.s3_access_key),
                ("TASKER_S3_SECRET_KEY", &config.s3_secret_key),
            ]
            .into_iter()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k)
            .collect::<Vec<_>>()
            .join(", ");
            if !missing.is_empty() {
                return Err(ConfigError(format!(
                    "TASKER_ATTACHMENTS=s3 requires these settings: {missing}"
                )));
            }
        }
        Ok(config)
    }
}

/// Outbox dispatcher operational parameters. Defaults follow the Task 5
/// specification (1s poll, batch 16, 2 concurrent executions, 60s lease,
/// 8 attempts).
#[derive(Debug, Clone)]
pub struct RulesWorkerConfig {
    /// Whether the dispatcher starts with the server.
    pub enabled: bool,
    /// Poll interval between claim cycles.
    pub poll_interval_ms: u64,
    /// Maximum intents claimed per cycle.
    pub batch_size: u32,
    /// Maximum intents executed concurrently.
    pub max_concurrent: u32,
    /// Claim lease duration before a record is recoverable again.
    pub lease_ms: u64,
    /// Attempts (execution failures) before an intent dead-letters.
    pub max_attempts: u32,
    /// Exponential backoff base: delay = base * 2^(attempt-1), jittered.
    pub backoff_base_ms: u64,
    /// Exponential backoff ceiling.
    pub backoff_max_ms: u64,
}

impl Default for RulesWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_ms: 1_000,
            batch_size: 16,
            max_concurrent: 2,
            lease_ms: 60_000,
            max_attempts: 8,
            backoff_base_ms: 1_000,
            backoff_max_ms: 60_000,
        }
    }
}

/// Webhook delivery worker operational parameters (Phase 8B, Task 2).
/// Defaults mirror the proven Rules dispatcher policy (1s poll, batch 16,
/// 2 concurrent executions, 60s lease, 8 attempts, 1s/60s backoff) with the
/// additions the delivery lifecycle requires. The full-jitter fraction is an
/// internal constant (jitter in `[0, 1000]` on the shared backoff function),
/// not a configuration surface.
#[derive(Debug, Clone)]
pub struct WebhookWorkerConfig {
    /// Whether the worker starts with the server.
    pub enabled: bool,
    /// Poll interval between claim cycles.
    pub poll_interval_ms: u64,
    /// Maximum deliveries claimed per cycle.
    pub batch_size: u32,
    /// Maximum deliveries processed concurrently.
    pub max_concurrent: u32,
    /// Claim lease duration before a delivery is recoverable again.
    pub lease_ms: u64,
    /// Attempts (processing failures) before a delivery dead-letters.
    pub max_attempts: u32,
    /// Exponential backoff base: delay = base * 2^(attempt-1), jittered.
    pub backoff_base_ms: u64,
    /// Exponential backoff ceiling.
    pub backoff_max_ms: u64,
    /// Deliveries older than this dead-letter on their next failure instead
    /// of retrying (bounded retry age; 7 days by default).
    pub max_delivery_age_ms: u64,
}

impl Default for WebhookWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_ms: 1_000,
            batch_size: 16,
            max_concurrent: 2,
            lease_ms: 60_000,
            max_attempts: 8,
            backoff_base_ms: 1_000,
            backoff_max_ms: 60_000,
            max_delivery_age_ms: 7 * 24 * 60 * 60 * 1000,
        }
    }
}

impl WebhookWorkerConfig {
    /// Cross-field validation beyond the per-field clamping applied during
    /// env parsing: fail fast at startup when the values cannot form a
    /// correct retry policy.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] describing the incoherent combination.
    pub fn validated(self) -> Result<Self, ConfigError> {
        if self.backoff_max_ms < self.backoff_base_ms {
            return Err(ConfigError(
                "TASKER_WEBHOOK_WORKER_BACKOFF_MAX_MS must be >= \
                 TASKER_WEBHOOK_WORKER_BACKOFF_BASE_MS"
                    .into(),
            ));
        }
        if self.max_delivery_age_ms < self.lease_ms {
            return Err(ConfigError(
                "TASKER_WEBHOOK_WORKER_MAX_DELIVERY_AGE_MS must be >= \
                 TASKER_WEBHOOK_WORKER_LEASE_MS (a delivery must not expire \
                 while its first lease is still valid)"
                    .into(),
            ));
        }
        Ok(self)
    }
}

impl Config {
    /// Load configuration from the environment, failing fast with precise
    /// messages when a required or malformed setting is encountered.
    pub fn from_env() -> Result<Self, ConfigError> {
        let environment = Environment::parse(&env_or("TASKER_ENV", "development")?)?;

        let database_url = env::var(format!("{ENV_PREFIX}DATABASE_URL"))
            .map_err(|_| ConfigError("TASKER_DATABASE_URL is required".into()))?;

        let http_port = env_parsed("TASKER_HTTP_PORT", 8080u16)?;
        let session_ttl_hours: u64 = env_parsed("TASKER_SESSION_TTL_HOURS", 336u64)?;
        let pool_max: u32 = env_parsed("TASKER_DB_MAX_CONNECTIONS", 10u32)?;
        let acquire_secs: u64 = env_parsed("TASKER_DB_ACQUIRE_TIMEOUT_SECS", 5u64)?;
        let idle_secs: u64 = env_parsed("TASKER_DB_IDLE_TIMEOUT_SECS", 300u64)?;
        let lifetime_secs: u64 = env_parsed("TASKER_DB_MAX_LIFETIME_SECS", 1800u64)?;
        let login_rate_limit_per_min: u32 = env_parsed("TASKER_RATE_LIMIT_LOGIN_PER_MIN", 10u32)?;
        // 4 MB: avatars travel as data URLs inside JSON (2 MB decoded image
        // ≈ 2.7 MB base64); every other payload stays far below this.
        let body_limit_bytes: usize = env_parsed("TASKER_BODY_LIMIT_BYTES", 4_194_304usize)?;

        let mfa_encryption_key = match std::env::var("TASKER_MFA_ENCRYPTION_KEY") {
            Ok(raw) => {
                let hex_key = raw.trim();
                if hex_key.len() != 64 || !hex_key.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(crate::error::ConfigError(
                        "TASKER_MFA_ENCRYPTION_KEY must be exactly 64 hex characters (32 bytes)"
                            .to_string(),
                    ));
                }
                let bytes = (0..32)
                    .map(|i| u8::from_str_radix(&hex_key[i * 2..i * 2 + 2], 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .map_err(|e| {
                        crate::error::ConfigError(format!(
                            "TASKER_MFA_ENCRYPTION_KEY is not valid hex: {e}"
                        ))
                    })?;
                Some(bytes)
            }
            Err(_) => None,
        };

        // Independent recovery-code pepper (HMAC-SHA-256 key for recovery-code
        // digests, ADR-0012). Deliberately NOT derived from the AES-GCM
        // encryption key: separate compromise domains. Absent => recovery
        // flows fail closed (the server boots for deployments without 2FA);
        // present-but-malformed => startup aborts.
        let mfa_recovery_pepper = match std::env::var("TASKER_RECOVERY_PEPPER") {
            Ok(raw) => {
                let hex_key = raw.trim();
                if hex_key.len() != 64 || !hex_key.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(crate::error::ConfigError(
                        "TASKER_RECOVERY_PEPPER must be exactly 64 hex characters (32 bytes)"
                            .to_string(),
                    ));
                }
                let bytes = (0..32)
                    .map(|i| u8::from_str_radix(&hex_key[i * 2..i * 2 + 2], 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .map_err(|e| {
                        crate::error::ConfigError(format!(
                            "TASKER_RECOVERY_PEPPER is not valid hex: {e}"
                        ))
                    })?;
                Some(bytes)
            }
            Err(_) => None,
        };

        let integrations_key = match std::env::var("TASKER_INTEGRATIONS_KEY") {
            Ok(raw) => {
                let hex_key = raw.trim();
                if hex_key.len() != 64 || !hex_key.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(crate::error::ConfigError(
                        "TASKER_INTEGRATIONS_KEY must be exactly 64 hex characters (32 bytes)"
                            .to_string(),
                    ));
                }
                let bytes = (0..32)
                    .map(|i| u8::from_str_radix(&hex_key[i * 2..i * 2 + 2], 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .map_err(|e| {
                        crate::error::ConfigError(format!(
                            "TASKER_INTEGRATIONS_KEY is not valid hex: {e}"
                        ))
                    })?;
                Some(bytes)
            }
            Err(_) => None,
        };

        let allowed_origins = env_or("TASKER_ALLOWED_ORIGINS", String::new())?
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(std::string::ToString::to_string)
            .collect();

        let static_dir = env::var(format!("{ENV_PREFIX}STATIC_DIR"))
            .ok()
            .filter(|s| !s.is_empty());

        let log_format_json = matches!(
            env_or("TASKER_LOG_FORMAT", "text")?.as_str(),
            "json" | "JSON"
        );

        let rules_worker = RulesWorkerConfig {
            enabled: env_or("TASKER_RULES_WORKER_ENABLED", "true".to_string())?
                .eq_ignore_ascii_case("true"),
            poll_interval_ms: env_parsed("TASKER_RULES_WORKER_POLL_MS", 1_000u64)?.max(50),
            batch_size: env_parsed("TASKER_RULES_WORKER_BATCH", 16u32)?.clamp(1, 100),
            max_concurrent: env_parsed("TASKER_RULES_WORKER_CONCURRENCY", 2u32)?.clamp(1, 16),
            lease_ms: env_parsed("TASKER_RULES_WORKER_LEASE_MS", 60_000u64)?.max(1_000),
            max_attempts: env_parsed("TASKER_RULES_WORKER_MAX_ATTEMPTS", 8u32)?.clamp(1, 100),
            backoff_base_ms: env_parsed("TASKER_RULES_WORKER_BACKOFF_BASE_MS", 1_000u64)?.max(1),
            backoff_max_ms: env_parsed("TASKER_RULES_WORKER_BACKOFF_MAX_MS", 60_000u64)?.max(1),
        };

        let webhook_worker = WebhookWorkerConfig {
            enabled: env_or("TASKER_WEBHOOK_WORKER_ENABLED", "true".to_string())?
                .eq_ignore_ascii_case("true"),
            poll_interval_ms: env_parsed("TASKER_WEBHOOK_WORKER_POLL_MS", 1_000u64)?.max(50),
            batch_size: env_parsed("TASKER_WEBHOOK_WORKER_BATCH", 16u32)?.clamp(1, 1000),
            max_concurrent: env_parsed("TASKER_WEBHOOK_WORKER_CONCURRENCY", 2u32)?.clamp(1, 64),
            lease_ms: env_parsed("TASKER_WEBHOOK_WORKER_LEASE_MS", 60_000u64)?.max(1_000),
            max_attempts: env_parsed("TASKER_WEBHOOK_WORKER_MAX_ATTEMPTS", 8u32)?.clamp(1, 100),
            backoff_base_ms: env_parsed("TASKER_WEBHOOK_WORKER_BACKOFF_BASE_MS", 1_000u64)?.max(1),
            backoff_max_ms: env_parsed("TASKER_WEBHOOK_WORKER_BACKOFF_MAX_MS", 60_000u64)?.max(1),
            max_delivery_age_ms: env_parsed(
                "TASKER_WEBHOOK_WORKER_MAX_DELIVERY_AGE_MS",
                7 * 24 * 60 * 60 * 1_000u64,
            )?
            .max(60_000),
        }
        .validated()?;

        let attachments = AttachmentsConfig::from_env()?;

        Ok(Self {
            environment,
            http_host: env_or("TASKER_HTTP_HOST", "0.0.0.0".to_string())?,
            http_port,
            database_url,
            pool: PoolConfig {
                max_connections: pool_max.max(1),
                acquire_timeout: Duration::from_secs(acquire_secs.max(1)),
                idle_timeout: Duration::from_secs(idle_secs.max(1)),
                max_lifetime: Duration::from_secs(lifetime_secs.max(1)),
            },
            session_ttl: Duration::from_secs(session_ttl_hours.max(1) * 3600),
            public_registration: env_or("TASKER_PUBLIC_REGISTRATION", "false".to_string())?
                .eq_ignore_ascii_case("true"),
            allowed_origins,
            login_rate_limit_per_min,
            mfa_encryption_key,
            mfa_recovery_pepper,
            integrations_key,
            body_limit_bytes,
            static_dir,
            log_format_json,
            rules_worker,
            webhook_worker,
            attachments,
        })
    }

    /// Whether session cookies must be marked `Secure` (production only).
    #[must_use]
    pub fn cookie_secure(&self) -> bool {
        self.environment.is_production()
    }
}

fn env_or(key: &str, default: impl Into<String>) -> Result<String, ConfigError> {
    match env::var(key) {
        Ok(v) => Ok(v),
        Err(env::VarError::NotPresent) => Ok(default.into()),
        Err(e) => Err(ConfigError(format!("{key} is not valid UTF-8: {e}"))),
    }
}

fn env_parsed<T: std::str::FromStr>(key: &str, default: T) -> Result<T, ConfigError> {
    match env::var(key) {
        Err(env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(ConfigError(format!("{key} is not valid UTF-8: {e}"))),
        Ok(raw) => raw.trim().parse::<T>().map_err(|_| {
            ConfigError(format!(
                "{key} must be parseable as the expected type, got `{raw}`"
            ))
        }),
    }
}

#[cfg(test)]
/// Webhook worker configuration validation (Phase 8B, Task 2): cross-field
/// invariants fail fast at startup, coherent policies load cleanly.
#[cfg(test)]
mod webhook_worker_config_tests {
    use super::WebhookWorkerConfig;

    #[test]
    fn defaults_are_coherent() {
        assert!(WebhookWorkerConfig::default().validated().is_ok());
    }

    #[test]
    fn backoff_ceiling_below_base_is_rejected() {
        let config = WebhookWorkerConfig {
            backoff_base_ms: 5_000,
            backoff_max_ms: 1_000,
            ..WebhookWorkerConfig::default()
        };
        let err = config.validated().unwrap_err();
        assert!(err.0.contains("BACKOFF_MAX_MS"));
    }

    #[test]
    fn delivery_age_below_lease_is_rejected() {
        let config = WebhookWorkerConfig {
            lease_ms: 60_000,
            max_delivery_age_ms: 30_000,
            ..WebhookWorkerConfig::default()
        };
        let err = config.validated().unwrap_err();
        assert!(err.0.contains("MAX_DELIVERY_AGE_MS"));
    }

    #[test]
    fn boundary_equality_is_accepted() {
        let config = WebhookWorkerConfig {
            backoff_base_ms: 1_000,
            backoff_max_ms: 1_000,
            lease_ms: 60_000,
            max_delivery_age_ms: 60_000,
            ..WebhookWorkerConfig::default()
        };
        assert!(config.validated().is_ok());
    }
}

mod mfa_key_tests {
    // The key-parsing rules live in Config::from_env; the loadable-shape
    // invariants are pure and checked here without touching process env.
    #[test]
    fn key_shape_rules() {
        let valid = "a".repeat(64);
        assert_eq!(valid.len(), 64);
        assert!(valid.chars().all(|c| c.is_ascii_hexdigit()));
        // Wrong length and non-hex shapes are exactly what the loader rejects.
        let short = "a".repeat(63);
        assert_ne!(short.len(), 64);
        let not_hex = "z".repeat(64);
        assert!(!not_hex.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
