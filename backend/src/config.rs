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
    /// Maximum accepted JSON body size.
    pub body_limit_bytes: usize,
    /// Optional directory containing the built frontend to serve statically.
    pub static_dir: Option<String>,
    /// Log format: structured text or machine-readable JSON.
    pub log_format_json: bool,
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
        let body_limit_bytes: usize = env_parsed("TASKER_BODY_LIMIT_BYTES", 1_048_576usize)?;

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
            body_limit_bytes,
            static_dir,
            log_format_json,
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
