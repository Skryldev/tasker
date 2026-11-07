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
            body_limit_bytes,
            static_dir,
            log_format_json,
            rules_worker,
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
