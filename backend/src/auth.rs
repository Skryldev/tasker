//! Authentication: sessions, login/logout, registration, password change.
//!
//! Session architecture (see docs/security.md):
//! - Opaque 256-bit random token delivered in an `HttpOnly`, SameSite=Lax cookie
//!   (Secure in production). Never stored in browser localStorage.
//! - Only the SHA-256 hash of the token is persisted; raw tokens are not
//!   recoverable from the database.
//! - Fixed TTL (configurable, default 14 days), revoked on logout and after a
//!   password change; expired/revoked sessions are rejected.

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::http::extractors::client_ip;
use crate::state::AppState;
use crate::users::{self, User};
use crate::util;

pub const SESSION_COOKIE: &str = "tasker_session";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/register", post(register))
        .route("/me", get(me))
        .route("/me/password", put(change_password))
        .merge(crate::mfa::routes())
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

pub async fn create_session(
    conn: &mut Conn<'_>,
    user_id: &str,
    ttl_secs: u64,
    ip: &str,
    user_agent: Option<&str>,
) -> Result<String, AppError> {
    let token = crate::security::token::generate();
    let now = util::now_ms();
    let expires_at = now.saturating_add(i64::try_from(ttl_secs).unwrap_or(i64::MAX / 2));
    let mut args = Args::new();
    args.s(util::new_id())
        .s(user_id)
        .s(crate::security::token::hash(&token))
        .i(now)
        .i(expires_at)
        .i(now)
        .opt_s(if ip.is_empty() {
            None
        } else {
            Some(ip.to_string())
        })
        .opt_s(user_agent.map(|ua| util::truncate(ua, 200)));
    conn.execute(
        "INSERT INTO sessions (id, user_id, token_hash, created_at, expires_at, last_seen_at, ip_address, user_agent) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    Ok(token)
}

/// Resolve a valid (non-expired, non-revoked) session to its active user.
async fn find_valid_session(
    conn: &mut Conn<'_>,
    token_hash: &str,
) -> Result<Option<(User, String)>, AppError> {
    let mut args = Args::new();
    args.s(token_hash).i(util::now_ms());
    let row = conn
        .fetch_optional(
            "SELECT u.id, u.email, u.display_name, u.created_at, s.id AS session_id \
             FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = ? AND s.revoked_at IS NULL \
               AND s.expires_at > ? AND u.is_active = TRUE",
            args,
        )
        .await?;
    row.map(|r| {
        Ok((
            User {
                id: r.try_get("id")?,
                email: r.try_get("email")?,
                display_name: r.try_get("display_name")?,
                created_at: r.try_get("created_at")?,
            },
            r.try_get::<String, _>("session_id")?,
        ))
    })
    .transpose()
}

/// Update last-seen at most once per minute per session to bound write load.
async fn touch_session(conn: &mut Conn<'_>, session_id: &str, now: i64) {
    let mut args = Args::new();
    args.i(now).s(session_id).i(now);
    if let Err(e) = conn
        .execute(
            "UPDATE sessions SET last_seen_at = ? WHERE id = ? AND last_seen_at < ? - 60000",
            args,
        )
        .await
    {
        tracing::warn!(error = %e, "failed to touch session");
    }
}

pub async fn revoke_session(conn: &mut Conn<'_>, session_id: &str) -> Result<(), AppError> {
    let mut args = Args::new();
    args.i(util::now_ms()).s(session_id);
    conn.execute("UPDATE sessions SET revoked_at = ? WHERE id = ?", args)
        .await?;
    Ok(())
}

/// Revoke every active session of a user except (optionally) the current one.
pub async fn revoke_other_sessions(
    conn: &mut Conn<'_>,
    user_id: &str,
    except_session_id: Option<&str>,
) -> Result<(), AppError> {
    let now = util::now_ms();
    let mut args = Args::new();
    args.i(now).s(user_id);
    let sql = if except_session_id.is_some() {
        args.opt_s(except_session_id);
        "UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL AND id != ?"
    } else {
        "UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL"
    };
    conn.execute(sql, args).await?;
    Ok(())
}

async fn purge_expired(conn: &mut Conn<'_>) {
    let mut args = Args::new();
    args.i(util::now_ms());
    let _ = conn
        .execute("DELETE FROM sessions WHERE expires_at < ?", args)
        .await;
}

// ---------------------------------------------------------------------------
// Cookie handling
// ---------------------------------------------------------------------------

#[must_use]
pub fn session_cookie(token: &str, secure: bool, max_age_secs: u64) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax{secure_flag}; Max-Age={max_age_secs}"
    )
}

#[must_use]
pub fn clear_session_cookie(secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax{secure_flag}; Max-Age=0")
}

#[must_use]
fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=')
            && k == name
            && !v.is_empty()
        {
            return Some(v.to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Authentication core
// ---------------------------------------------------------------------------

/// Validate the request's session cookie and return the authenticated user.
pub async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<AuthedUser, AppError> {
    let Some(token) = read_cookie(headers, SESSION_COOKIE) else {
        return Err(AppError::Unauthenticated);
    };
    let token_hash = crate::security::token::hash(&token);
    let mut conn = state.db.conn();
    let Some((user, session_id)) = find_valid_session(&mut conn, &token_hash).await? else {
        return Err(AppError::Unauthenticated);
    };
    let now = util::now_ms();
    touch_session(&mut conn, &session_id, now).await;
    Ok(AuthedUser {
        id: user.id,
        email: user.email,
        display_name: user.display_name,
        session_id,
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Login outcome: a normal session (non-MFA users, unchanged contract) or an
/// MFA-pending challenge. The pending token lives only in the response body —
/// it is never a cookie and cannot authorize any other endpoint.
#[derive(Serialize)]
#[serde(untagged)]
pub enum LoginResponse {
    User(UserResponse),
    MfaRequired {
        mfa_required: bool,
        mfa_token: String,
    },
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub created_at: i64,
    /// Whether TOTP two-factor authentication is enabled for this account.
    pub mfa_enabled: bool,
}

impl From<User> for UserResponse {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            display_name: u.display_name,
            created_at: u.created_at,
            mfa_enabled: false,
        }
    }
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<(StatusCode, HeaderMap, Json<LoginResponse>), AppError> {
    let ip = client_ip(&headers);
    let rate_key = format!("login:{ip}");
    if let Err(retry_after) = state.rate_limiter.check(
        &rate_key,
        state.config.login_rate_limit_per_min,
        std::time::Duration::from_secs(60),
    ) {
        return Err(AppError::RateLimited {
            retry_after_secs: retry_after.as_secs().max(1),
        });
    }

    if body.email.trim().is_empty() || body.password.is_empty() {
        return Err(AppError::BadRequest(
            "email and password are required".into(),
        ));
    }

    let mut conn = state.db.conn();
    let found = users::find_active_by_email(&mut conn, &body.email).await?;
    let Some((user, password_hash)) = found else {
        // Equalize response time for unknown users (runs a verification).
        let _ = crate::security::password::verify(
            &body.password,
            "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        return Err(AppError::Unauthenticated);
    };
    if !crate::security::password::verify(&body.password, &password_hash) {
        return Err(AppError::Unauthenticated);
    }

    // Two-factor users do not get a session yet: password possession alone
    // yields a short-lived, single-use MFA-pending credential.
    if crate::mfa::is_enabled(&mut conn, &user.id).await? {
        let mfa_token = crate::mfa::issue_pending_token(&mut conn, &user.id).await?;
        return Ok((
            StatusCode::OK,
            HeaderMap::new(),
            Json(LoginResponse::MfaRequired {
                mfa_required: true,
                mfa_token,
            }),
        ));
    }

    purge_expired(&mut conn).await;
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string);
    let token = create_session(
        &mut conn,
        &user.id,
        state.config.session_ttl.as_secs(),
        &ip,
        user_agent.as_deref(),
    )
    .await?;

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(
            &token,
            state.config.cookie_secure(),
            state.config.session_ttl.as_secs(),
        ))
        .map_err(|_| AppError::internal("cannot build session cookie"))?,
    );
    Ok((
        StatusCode::OK,
        response_headers,
        Json(LoginResponse::User(UserResponse::from(user))),
    ))
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, HeaderMap), AppError> {
    if let Ok(user) = authenticate(&state, &headers).await {
        let mut conn = state.db.conn();
        revoke_session(&mut conn, &user.session_id).await?;
    }
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&clear_session_cookie(state.config.cookie_secure()))
            .map_err(|_| AppError::internal("cannot build cookie"))?,
    );
    Ok((StatusCode::NO_CONTENT, response_headers))
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub display_name: String,
    pub password: String,
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<UserResponse>), AppError> {
    if !state.config.public_registration {
        return Err(AppError::Forbidden(
            "public registration is disabled on this deployment",
        ));
    }
    let mut conn = state.db.conn();
    let user =
        users::create_validated(&mut conn, &body.email, &body.display_name, &body.password).await?;
    Ok((StatusCode::CREATED, Json(UserResponse::from(user))))
}

async fn me(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<Json<UserResponse>, AppError> {
    let mut conn = state.db.conn();
    let full = users::find_by_id(&mut conn, &user.id)
        .await?
        .ok_or(AppError::Unauthenticated)?;
    let mut response = UserResponse::from(full);
    response.mfa_enabled = crate::mfa::is_enabled(&mut conn, &user.id).await?;
    Ok(Json(response))
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<StatusCode, AppError> {
    crate::security::password::validate(&body.new_password)
        .map_err(|m| AppError::validation("new_password", m))?;
    let mut conn = state.db.conn();
    let found = users::find_active_by_email(&mut conn, &user.email).await?;
    let Some((_, password_hash)) = found else {
        return Err(AppError::Unauthenticated);
    };
    if !crate::security::password::verify(&body.current_password, &password_hash) {
        return Err(AppError::validation(
            "current_password",
            "current password is incorrect",
        ));
    }
    let new_hash =
        crate::security::password::hash(&body.new_password).map_err(AppError::internal)?;
    users::update_password(&mut conn, &user.id, &new_hash).await?;
    revoke_other_sessions(&mut conn, &user.id, Some(&user.session_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Extractor
// ---------------------------------------------------------------------------

/// Session-authenticated user injected into handlers; rejects with 401 when
/// the session cookie is missing, expired, revoked, or the user is inactive.
#[derive(Debug, Clone)]
pub struct AuthedUser {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub session_id: String,
}

impl FromRequestParts<AppState> for AuthedUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(state, &parts.headers).await
    }
}
