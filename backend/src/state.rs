//! Shared application state.

use std::sync::Arc;
use std::time::Instant;

use crate::config::Config;
use crate::db::Database;
use crate::http::metrics::Metrics;
use crate::security::RateLimiter;

pub struct AppStateInner {
    pub db: Database,
    pub config: Config,
    pub rate_limiter: RateLimiter,
    pub metrics: Metrics,
    pub started_at: Instant,
}

#[derive(Clone)]
pub struct AppState(pub Arc<AppStateInner>);

impl AppState {
    #[must_use]
    pub fn new(db: Database, config: Config) -> Self {
        Self(Arc::new(AppStateInner {
            db,
            config,
            rate_limiter: RateLimiter::new(),
            metrics: Metrics::new(),
            started_at: Instant::now(),
        }))
    }
}

impl std::ops::Deref for AppState {
    type Target = AppStateInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
