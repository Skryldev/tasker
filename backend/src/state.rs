//! Shared application state.

use std::sync::Arc;
use std::time::Instant;

use crate::config::Config;
use crate::db::Database;
use crate::http::metrics::Metrics;
use crate::integrations::webhook::ProviderRegistry;
use crate::security::RateLimiter;

/// Integration runtime pieces (Phase 8B, Task 2): the provider adapter
/// registry (immutable after startup — GitHub/GitLab adapters register in
/// Tasks 3–4; tests inject deterministic adapters) and the resolved
/// integration encryption key (`TASKER_INTEGRATIONS_KEY`; `None` means every
/// credential operation fails closed).
#[derive(Clone)]
pub struct IntegrationRuntime {
    pub registry: ProviderRegistry,
    pub key: Option<Vec<u8>>,
}

pub struct AppStateInner {
    pub db: Database,
    pub config: Config,
    pub rate_limiter: RateLimiter,
    pub metrics: Metrics,
    pub started_at: Instant,
    pub integrations: IntegrationRuntime,
}

#[derive(Clone)]
pub struct AppState(pub Arc<AppStateInner>);

impl AppState {
    #[must_use]
    pub fn new(db: Database, config: Config) -> Self {
        Self::with_registry(db, config, ProviderRegistry::default())
    }

    /// Build state with an explicit provider adapter registry (tests inject
    /// deterministic adapters; production registration arrives with the
    /// provider adapter tasks).
    #[must_use]
    pub fn with_registry(db: Database, config: Config, registry: ProviderRegistry) -> Self {
        let key = config.integrations_key.clone();
        Self(Arc::new(AppStateInner {
            db,
            config,
            rate_limiter: RateLimiter::new(),
            metrics: Metrics::new(),
            started_at: Instant::now(),
            integrations: IntegrationRuntime { registry, key },
        }))
    }
}

impl std::ops::Deref for AppState {
    type Target = AppStateInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
