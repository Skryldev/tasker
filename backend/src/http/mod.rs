//! HTTP transport: router assembly, middleware, and health endpoints.
//!
//! Handlers stay thin; business rules live in the domain modules
//! (`teams`, `projects`, `tasks`, `comments`, `activity`, `auth`).

pub mod extractors;
pub mod metrics;

use std::time::Instant;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Serialize;
use tower_http::services::fs::{ServeDir, ServeFile};
use tower_http::timeout::TimeoutLayer;
use tracing::Instrument;

use crate::error::AppError;
use crate::state::AppState;
use crate::{activity, auth, comments, projects, tasks, teams};

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Assemble the complete application router.
pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/openapi.json", get(openapi))
        .merge(auth::routes())
        .merge(teams::routes())
        .merge(projects::routes())
        .merge(tasks::routes())
        .merge(comments::routes())
        .merge(activity::routes())
        .merge(crate::notifications::routes());

    let mut app = Router::new()
        .nest("/api/v1", api)
        .route("/metrics", get(metrics_endpoint))
        .layer(DefaultBodyLimit::max(state.config.body_limit_bytes))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            metrics_middleware,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            origin_check_middleware,
        ));

    if !state.config.allowed_origins.is_empty() {
        app = app.layer(cors_layer(&state.config.allowed_origins));
    }

    app = app.layer(middleware::from_fn(request_context_middleware));

    if let Some(dir) = state.config.static_dir.clone() {
        let index = std::path::Path::new(&dir).join("index.html");
        app = app.fallback_service(
            ServeDir::new(&dir)
                .append_index_html_on_directories(true)
                .not_found_service(ServeFile::new(index)),
        );
    }

    app.with_state(state)
}

fn cors_layer(allowed_origins: &[String]) -> tower_http::cors::CorsLayer {
    let origins: Vec<HeaderValue> = allowed_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();
    tower_http::cors::CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            axum::http::HeaderName::from_static("x-request-id"),
        ])
        .allow_credentials(true)
        .max_age(std::time::Duration::from_secs(3600))
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Request correlation + structured request logging.
///
/// The span is attached with `Instrument` instead of `span.enter()`: holding
/// an `Entered` guard across `.await` can exit the span on a different worker
/// and panics the tracing registry under load.
async fn request_context_middleware(request: Request, next: Next) -> Response {
    let request_id = crate::util::new_id();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let start = Instant::now();

    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %method,
        path = %path,
    );

    let mut response = next.run(request).instrument(span).await;
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request_id).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );
    let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!(
        status = %response.status(),
        duration_ms = elapsed_ms,
        "request completed"
    );
    response
}

/// Prometheus metrics with bounded cardinality (route template, method,
/// status class). Applied via `route_layer` so `MatchedPath` is available.
async fn metrics_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", |mp| mp.as_str())
        .to_string();
    let method = request.method().to_string();
    let start = Instant::now();
    let response = next.run(request).await;
    state.metrics.observe_request(
        &route,
        &method,
        response.status().as_u16(),
        start.elapsed().as_secs_f64() * 1000.0,
    );
    response
}

/// CSRF defense for cookie authentication: on state-changing requests, when a
/// browser-provided `Origin` header is present it must match an allowed
/// origin; when `Sec-Fetch-Site` is present it must be same-origin or none.
/// Combined with SameSite=Lax cookies this blocks cross-site request forgery
/// without needing per-request tokens (see docs/security.md). Non-browser
/// clients (curl, scripts) send neither header and are unaffected.
async fn origin_check_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    const SAFE_METHODS: &[Method] = &[Method::GET, Method::HEAD, Method::OPTIONS];

    if !SAFE_METHODS.contains(request.method()) {
        let origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let sec_fetch_site = request
            .headers()
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        if let Some(origin) = origin {
            let normalized = origin.trim_end_matches('/').to_lowercase();
            let allowed = state
                .config
                .allowed_origins
                .iter()
                .any(|a| a.trim_end_matches('/').to_lowercase() == normalized);
            // Same-origin requests carry the server's own origin: allow when
            // the request host matches the origin's host+port.
            if !allowed && !origin_matches_host(&request, &normalized) {
                return Err(AppError::Forbidden(
                    "request origin is not allowed for state-changing operations",
                ));
            }
        } else if let Some(site) = sec_fetch_site
            && !matches!(site.as_str(), "same-origin" | "none")
        {
            return Err(AppError::Forbidden(
                "cross-site state-changing requests are not allowed",
            ));
        }
    }
    Ok(next.run(request).await)
}

/// Same-origin fallback check: allow an unlisted Origin whose host matches the
/// request's own Host header (covers direct deployments without configured
/// origins).
fn origin_matches_host(request: &Request, origin: &str) -> bool {
    let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
    else {
        return false;
    };
    origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

// ---------------------------------------------------------------------------
// Health & ops endpoints
// ---------------------------------------------------------------------------

async fn live() -> Json<LiveResponse> {
    Json(LiveResponse {
        status: "ok".to_string(),
    })
}

#[derive(Serialize)]
struct LiveResponse {
    status: String,
}

#[derive(Serialize)]
struct ReadyResponse {
    status: String,
    database: String,
}

async fn ready(State(state): State<AppState>) -> Response {
    match state.db.ping().await {
        Ok(()) => (
            StatusCode::OK,
            Json(ReadyResponse {
                status: "ok".to_string(),
                database: state.db.driver().name().to_string(),
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "readiness check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ReadyResponse {
                    status: "degraded".to_string(),
                    database: "unavailable".to_string(),
                }),
            )
                .into_response()
        }
    }
}

async fn metrics_endpoint(State(state): State<AppState>) -> Response {
    let uptime = state.started_at.elapsed().as_secs();
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(uptime),
    )
        .into_response()
}

async fn openapi() -> Response {
    let body = include_str!("../../../api/openapi.json");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}
