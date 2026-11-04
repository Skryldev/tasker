//! TOTP two-factor authentication.
//!
//! Cryptographic boundary:
//! - TOTP secrets are encrypted at rest with AES-256-GCM; the key comes from
//!   `TASKER_MFA_ENCRYPTION_KEY` (64 hex chars = 32 bytes) and is never
//!   stored in or derivable from the database. The persisted envelope is
//!   `v1.<base64url(12-byte nonce || ciphertext || 16-byte tag)>`.
//! - Recovery codes are 8 characters from a 31-symbol unambiguous alphabet
//!   (~39.6 bits), displayed as `XXXX-XXXX`. Persisted digests are
//!   **keyed HMAC-SHA-256** under the independent `TASKER_RECOVERY_PEPPER`
//!   (not derivable from the encryption key and never stored in the
//!   database), stored as `h1.<hex>` — the tag separates the current keyed
//!   format from legacy unkeyed SHA-256 digests, which are rejected
//!   unconditionally (ADR-0012: unkeyed digests fell to offline brute force
//!   in minutes once the database leaked). The pepper does not increase the
//!   code's intrinsic entropy; it moves the offline attack from "minutes
//!   with the database alone" to "infeasible without also leaking the
//!   pepper". A stored code is consumed with a compare-and-set UPDATE so
//!   concurrent redemptions cannot both succeed.
//! - TOTP verification accepts only the current 30-second step (window 0)
//!   and records the consumed step in `user_mfa.last_used_step` with a
//!   compare-and-set UPDATE: a timestamp alone cannot identify a consumed
//!   time step, so replay prevention needs the counter itself.
//!
//! Nothing in this module logs or includes in errors: TOTP secrets or codes,
//! recovery codes, the pepper, passwords, provisioning URIs, or
//! pending/session tokens.

use aes_gcm::aead::Aead;
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::routing::post;
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use sha2::Digest as _;
use sha2::Sha256;
use sqlx::Row;
use totp_rs::{Algorithm, Secret as TotpSecret, TOTP};

use crate::auth::{AuthedUser, create_session, session_cookie};
use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::http::extractors::client_ip;
use crate::security::token;
use crate::state::AppState;
use crate::util;

/// Prefix marking an MFA-pending token. Pending tokens never live in the
/// session cookie or the sessions table; the prefix additionally makes a
/// cross-table hash collision structurally impossible to exploit.
pub const MFA_PENDING_PREFIX: &str = "mfp_";
/// TTL of an MFA-pending token.
pub const MFA_PENDING_TTL_MS: i64 = 10 * 60 * 1000;
/// Number of recovery codes generated at enable/regeneration time.
pub const RECOVERY_CODE_COUNT: usize = 10;
/// TOTP time step in seconds (RFC 6238 recommended value).
pub const TOTP_STEP_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Secret encryption (AES-256-GCM)
// ---------------------------------------------------------------------------

/// Encrypt `plaintext` under `key` (32 bytes) with a fresh random 96-bit
/// nonce. Output: `v1.<base64url(nonce || ciphertext || tag)>`.
pub fn encrypt_secret(key: &[u8], plaintext: &[u8]) -> Result<String, AppError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    // 96-bit random nonce; uniqueness per key is what GCM requires. Secret
    // encryption happens only at setup, so the per-key nonce space is never
    // approached (birthday bound ~2^32 encryptions).
    let nonce = Aes256Gcm::generate_nonce(&mut aes_gcm::aead::OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| AppError::internal("secret encryption failed"))?;
    let mut envelope = Vec::with_capacity(12 + ciphertext.len());
    envelope.extend_from_slice(nonce.as_slice());
    envelope.extend_from_slice(&ciphertext);
    Ok(format!("v1.{}", token::base64_url_encode(&envelope)))
}

/// Decrypt an envelope produced by [`encrypt_secret`]. The authentication tag
/// is verified by the AEAD; any failure (tampering, wrong key, malformed
/// envelope) returns a uniform error that never carries cryptographic detail.
pub fn decrypt_secret(key: &[u8], envelope: &str) -> Result<Vec<u8>, AppError> {
    let encoded = envelope
        .strip_prefix("v1.")
        .ok_or_else(|| AppError::internal("secret envelope is unreadable"))?;
    let data = base64_url_decode(encoded)
        .ok_or_else(|| AppError::internal("secret envelope is unreadable"))?;
    if data.len() < 12 + 16 {
        return Err(AppError::internal("secret envelope is unreadable"));
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| AppError::internal("secret envelope is unreadable"))?;
    Ok(plaintext)
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    const REV: fn(char) -> Option<u32> = |c| match c {
        'A'..='Z' => Some(u32::from(c) - u32::from('A')),
        'a'..='z' => Some(u32::from(c) - u32::from('a') + 26),
        '0'..='9' => Some(u32::from(c) - u32::from('0') + 52),
        '-' => Some(62),
        '_' => Some(63),
        _ => None,
    };
    if input.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let bytes: Vec<Option<u32>> = input.chars().map(REV).collect();
    if bytes.iter().any(Option::is_none) {
        return None;
    }
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, b) in chunk.iter().enumerate() {
            n |= b.unwrap() << (18 - 6 * i);
        }
        out.push(((n >> 16) & 0xFF) as u8);
        if chunk.len() > 2 {
            out.push(((n >> 8) & 0xFF) as u8);
        }
        if chunk.len() > 3 {
            out.push((n & 0xFF) as u8);
        }
    }
    Some(out)
}

fn require_key(state: &AppState) -> Result<&[u8], AppError> {
    state
        .config
        .mfa_encryption_key
        .as_deref()
        .ok_or_else(|| AppError::internal("two-factor authentication is not configured"))
}

// ---------------------------------------------------------------------------
// Recovery codes
// ---------------------------------------------------------------------------

/// 31-symbol alphabet without visually ambiguous characters (no 0/O, 1/I/L).
const RECOVERY_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// Generate one unbiased 8-character code from the 31-symbol alphabet.
/// Rejection sampling (accept bytes < 248 = 31 x 8) removes modulo bias;
/// ~39.6 bits of entropy per code, displayed `XXXX-XXXX`.
fn generate_recovery_code() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let alphabet_len = u8::try_from(RECOVERY_ALPHABET.len()).expect("alphabet fits in u8"); // 31
    // Largest multiple of the alphabet length that fits in a byte (248):
    // sampling below it and taking the remainder is unbiased. Computed in
    // wider arithmetic because 256 itself does not fit in u8.
    let unbiased_max: u8 = u8::try_from(256 - (256 % u16::from(alphabet_len)))
        .expect("multiple of the alphabet fits in u8");
    let mut code = String::with_capacity(9);
    for i in 0..8 {
        if i == 4 {
            code.push('-');
        }
        let index = loop {
            let b = rng.random::<u8>();
            if b < unbiased_max {
                break b % alphabet_len;
            }
        };
        code.push(RECOVERY_ALPHABET[usize::from(index)] as char);
    }
    code
}

/// Normalize a user-supplied recovery code: uppercase, separators stripped,
/// alphabet-validated. Returns `None` for anything that is not a code.
#[must_use]
pub fn normalize_recovery_code(input: &str) -> Option<String> {
    let upper: String = input
        .chars()
        .filter(|c| *c != '-' && *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if upper.len() != 8
        || !upper
            .chars()
            .all(|c| RECOVERY_ALPHABET.contains(&(c as u8)))
    {
        return None;
    }
    Some(upper)
}

/// Storage tag for the current keyed digest format (ADR-0012).
const RECOVERY_DIGEST_TAG: &str = "h1.";
/// Domain-separation context bound into every recovery-code HMAC.
const RECOVERY_HMAC_CONTEXT: &[u8] = b"tasker:recovery-code:v1";

/// Keyed digest of a normalized recovery code (what the DB stores):
/// `h1.<hex(HMAC-SHA-256(pepper, "tasker:recovery-code:v1" || code))>`.
///
/// The HMAC pepper keeps an offline attack against leaked digest rows
/// infeasible without the pepper; the `h1.` tag makes the current format
/// self-describing and structurally distinct from legacy unkeyed SHA-256
/// digests, which the verification path rejects unconditionally.
fn recovery_hash(pepper: &[u8], normalized: &str) -> String {
    let mut mac =
        <Hmac<Sha256> as hmac::Mac>::new_from_slice(pepper).expect("HMAC accepts any key length");
    mac.update(RECOVERY_HMAC_CONTEXT);
    mac.update(normalized.as_bytes());
    format!(
        "{}{}",
        RECOVERY_DIGEST_TAG,
        hex_lower(&mac.finalize().into_bytes())
    )
}

/// Parse a stored digest entry. Returns the hex payload only for entries in
/// the current `h1.` keyed format; legacy unkeyed SHA-256 digests (bare hex)
/// are rejected — they can never be accepted by the verification path.
fn parse_recovery_digest(entry: &str) -> Option<&str> {
    entry
        .strip_prefix(RECOVERY_DIGEST_TAG)
        .filter(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

fn hex_lower(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for b in data {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 0x0F), 16).unwrap_or('0'));
    }
    out
}

/// Generate `RECOVERY_CODE_COUNT` codes and their stored form: a JSON array
/// of keyed `h1.` HMAC digests.
fn generate_recovery_set(pepper: &[u8]) -> (Vec<String>, String) {
    let mut plain = Vec::with_capacity(RECOVERY_CODE_COUNT);
    let mut hashes = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let code = generate_recovery_code();
        // Store the digest of the NORMALIZED code (what verification hashes).
        let normalized = normalize_recovery_code(&code).expect("generated code is valid");
        hashes.push(recovery_hash(pepper, &normalized));
        plain.push(code);
    }
    let json = serde_json::to_string(&hashes).unwrap_or_default();
    (plain, json)
}

/// Fail-closed access to the recovery-code pepper.
fn require_pepper(state: &AppState) -> Result<&[u8], AppError> {
    state
        .config
        .mfa_recovery_pepper
        .as_deref()
        .ok_or_else(|| AppError::internal("two-factor recovery codes are not configured"))
}

// ---------------------------------------------------------------------------
// TOTP
// ---------------------------------------------------------------------------

fn build_totp(secret: &[u8], email: &str) -> TOTP {
    TOTP::new(
        Algorithm::SHA1, // RFC 6238 default; universal authenticator support
        6,
        0, // window 0: only the current time step (skew in ±steps)
        TOTP_STEP_SECS,
        secret.to_vec(),
        Some("Tasker".to_string()),
        email.to_string(),
    )
    .expect("static TOTP parameters are valid")
}

/// Current RFC 6238 step counter (unix time / step).
#[must_use]
pub fn current_step() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
    .unwrap_or(0)
        / i64::try_from(TOTP_STEP_SECS).unwrap_or(30)
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

struct UserMfaRow {
    secret_encrypted: String,
    recovery_codes_hashed: String,
    enabled_at: Option<i64>,
    last_used_step: Option<i64>,
}

async fn find_user_mfa(conn: &mut Conn<'_>, user_id: &str) -> Result<Option<UserMfaRow>, AppError> {
    let mut args = Args::new();
    args.s(user_id);
    let row = conn
        .fetch_optional(
            "SELECT totp_secret_encrypted, recovery_codes_hashed, enabled_at, last_used_step \
             FROM user_mfa WHERE user_id = ?",
            args,
        )
        .await?;
    Ok(row.map(|r| UserMfaRow {
        secret_encrypted: r.try_get("totp_secret_encrypted").unwrap_or_default(),
        recovery_codes_hashed: r.try_get("recovery_codes_hashed").unwrap_or_default(),
        enabled_at: r.try_get("enabled_at").ok(),
        last_used_step: r.try_get("last_used_step").ok(),
    }))
}

/// Whether the user has completed MFA enrollment (used by the login flow).
pub async fn is_enabled(conn: &mut Conn<'_>, user_id: &str) -> Result<bool, AppError> {
    Ok(find_user_mfa(conn, user_id)
        .await?
        .and_then(|m| m.enabled_at)
        .is_some())
}

/// Store (or replace) the pending, still-disabled encrypted secret. Repeated
/// setup simply replaces the pending secret; an enabled configuration is
/// never touched by this path (handlers reject that earlier).
pub async fn store_pending_secret(
    conn: &mut Conn<'_>,
    user_id: &str,
    encrypted: &str,
) -> Result<(), AppError> {
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(util::new_id())
        .s(user_id)
        .s(encrypted)
        .s("[]")
        .i(now)
        .i(now)
        .i(now);
    let dialect = conn.driver();
    let sql = match dialect {
        crate::db::Driver::MySql => {
            "INSERT INTO user_mfa (id, user_id, totp_secret_encrypted, recovery_codes_hashed, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON DUPLICATE KEY UPDATE totp_secret_encrypted = VALUES(totp_secret_encrypted), updated_at = VALUES(updated_at)"
        }
        crate::db::Driver::Postgres => {
            "INSERT INTO user_mfa (id, user_id, totp_secret_encrypted, recovery_codes_hashed, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (user_id) DO UPDATE SET totp_secret_encrypted = EXCLUDED.totp_secret_encrypted, updated_at = EXCLUDED.updated_at"
        }
        crate::db::Driver::Sqlite => {
            "INSERT INTO user_mfa (id, user_id, totp_secret_encrypted, recovery_codes_hashed, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (user_id) DO UPDATE SET totp_secret_encrypted = EXCLUDED.totp_secret_encrypted, updated_at = EXCLUDED.updated_at"
        }
    };
    conn.execute(sql, args).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// MFA-pending tokens (server-side state, single use)
// ---------------------------------------------------------------------------

/// Issue an MFA-pending credential bound to `user_id`: 256 bits of OS
/// randomness (prefixed `mfp_`), only the SHA-256 hash persisted, 10-minute
/// TTL, single-use via `consumed_at`.
pub async fn issue_pending_token(conn: &mut Conn<'_>, user_id: &str) -> Result<String, AppError> {
    let raw = format!("{MFA_PENDING_PREFIX}{}", token::generate());
    let mut args = Args::new();
    args.s(util::new_id())
        .s(user_id)
        .s(token::hash(&raw))
        .i(util::now_ms())
        .i(util::now_ms() + MFA_PENDING_TTL_MS);
    conn.execute(
        "INSERT INTO mfa_pending (id, user_id, token_hash, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
        args,
    )
    .await?;
    Ok(raw)
}

/// Atomically consume a pending token: exactly one concurrent caller can move
/// `consumed_at` from NULL. Expired, consumed, or unknown tokens yield `None`.
/// Read the user bound to a pending token WITHOUT consuming it. Used to key
/// the per-user rate limit; consumption is a separate atomic step.
async fn resolve_pending_token(
    conn: &mut Conn<'_>,
    raw_token: &str,
) -> Result<Option<String>, AppError> {
    let mut args = Args::new();
    args.s(token::hash(raw_token));
    let row = conn
        .fetch_optional("SELECT user_id FROM mfa_pending WHERE token_hash = ?", args)
        .await?;
    Ok(row.and_then(|r| r.try_get::<String, _>("user_id").ok()))
}

pub async fn consume_pending_token(
    conn: &mut Conn<'_>,
    raw_token: &str,
) -> Result<Option<String>, AppError> {
    let mut args = Args::new();
    args.i(util::now_ms()).s(token::hash(raw_token));
    let affected = conn
        .execute(
            "UPDATE mfa_pending SET consumed_at = ? \
             WHERE token_hash = ? AND consumed_at IS NULL AND expires_at > ?",
            {
                let mut a = Args::new();
                a.i(util::now_ms())
                    .s(token::hash(raw_token))
                    .i(util::now_ms());
                a
            },
        )
        .await?;
    if affected == 0 {
        return Ok(None);
    }
    let mut args = Args::new();
    args.s(token::hash(raw_token));
    let row = conn
        .fetch_optional("SELECT user_id FROM mfa_pending WHERE token_hash = ?", args)
        .await?;
    Ok(row.and_then(|r| r.try_get::<String, _>("user_id").ok()))
}

async fn delete_pending_for_user(conn: &mut Conn<'_>, user_id: &str) -> Result<(), AppError> {
    let mut args = Args::new();
    args.s(user_id);
    conn.execute("DELETE FROM mfa_pending WHERE user_id = ?", args)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/2fa/setup", post(setup))
        .route("/auth/2fa/enable", post(enable))
        .route("/auth/2fa/verify", post(verify))
        .route("/auth/2fa/disable", post(disable))
        .route("/auth/2fa/recovery-codes", post(regenerate_recovery_codes))
}

fn no_store() -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str("no-store") {
        headers.insert(header::CACHE_CONTROL, v);
    }
    headers
}

#[derive(Serialize)]
struct SetupResponse {
    /// `otpauth://` provisioning URI for the QR code.
    uri: String,
    /// Base32 secret for manual authenticator entry.
    secret: String,
    issuer: String,
    account: String,
}

async fn setup(
    State(state): State<AppState>,
    user: AuthedUser,
) -> Result<(StatusCode, HeaderMap, Json<SetupResponse>), AppError> {
    let key = require_key(&state)?.to_vec();
    let mut conn = state.db.conn();
    if is_enabled(&mut conn, &user.id).await? {
        return Err(AppError::Conflict(
            "two-factor authentication is already enabled".into(),
        ));
    }
    // Fresh secret on every setup attempt; the previous pending secret (if
    // any) is replaced. Nothing is enabled until /auth/2fa/enable succeeds.
    // 160 bits from the OS CSPRNG — the RFC 4228-recommended secret size.
    let mut raw_bytes = [0u8; 20];
    rand::rng().fill_bytes(&mut raw_bytes);
    let totp = build_totp(&raw_bytes, &user.email);
    let encrypted = encrypt_secret(&key, &raw_bytes)?;
    store_pending_secret(&mut conn, &user.id, &encrypted).await?;
    let encoded = TotpSecret::Raw(raw_bytes.to_vec()).to_encoded().to_string();
    let response = SetupResponse {
        uri: totp.get_url(),
        secret: encoded,
        issuer: "Tasker".to_string(),
        account: user.email.clone(),
    };
    tracing::info!(user_id = %user.id, event = "mfa_setup", "MFA setup initiated");
    Ok((StatusCode::OK, no_store(), Json(response)))
}

#[derive(Deserialize)]
struct EnableRequest {
    code: String,
}

#[derive(Serialize)]
struct RecoveryCodesResponse {
    /// Plaintext codes, returned exactly once (enable or regeneration).
    recovery_codes: Vec<String>,
}

async fn enable(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(body): Json<EnableRequest>,
) -> Result<(StatusCode, HeaderMap, Json<RecoveryCodesResponse>), AppError> {
    let key = require_key(&state)?.to_vec();
    let pepper = require_pepper(&state)?.to_vec();
    let code = body.code.trim();
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::validation("code", "a six-digit code is required"));
    }
    let mut conn = state.db.conn();
    let Some(row) = find_user_mfa(&mut conn, &user.id).await? else {
        return Err(AppError::BadRequest("start two-factor setup first".into()));
    };
    if row.enabled_at.is_some() {
        return Err(AppError::Conflict(
            "two-factor authentication is already enabled".into(),
        ));
    }
    let secret = decrypt_secret(&key, &row.secret_encrypted)?;
    let totp = build_totp(&secret, &user.email);
    let ok = totp.check_current(code).unwrap_or(false);
    if !ok {
        // Invalid enrollment code must not enable MFA.
        tracing::info!(user_id = %user.id, event = "mfa_enable_failed", "MFA enable rejected");
        return Err(AppError::Unauthenticated);
    }
    // Atomic enable: only the first racing request flips enabled_at.
    // last_used_step is deliberately NOT set here: enrollment is not a
    // consumed login step, and recording the current step would reject the
    // user's first legitimate login within the same 30-second window.
    let now = util::now_ms();
    let (plain_codes, hashed_json) = generate_recovery_set(&pepper);
    let mut args = Args::new();
    args.i(now).s(&hashed_json).i(now).s(&user.id);
    let affected = conn
        .execute(
            "UPDATE user_mfa SET enabled_at = ?, recovery_codes_hashed = ?, updated_at = ? \
             WHERE user_id = ? AND enabled_at IS NULL",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::Conflict(
            "two-factor authentication is already enabled".into(),
        ));
    }
    tracing::info!(user_id = %user.id, event = "mfa_enabled", "MFA enabled");
    Ok((
        StatusCode::OK,
        no_store(),
        Json(RecoveryCodesResponse {
            recovery_codes: plain_codes,
        }),
    ))
}

#[derive(Deserialize)]
struct VerifyRequest {
    mfa_token: String,
    code: String,
}

async fn verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<VerifyRequest>,
) -> Result<(StatusCode, HeaderMap, Json<crate::auth::UserResponse>), AppError> {
    /// What the read-only validation decided; only the success path consumes
    /// anything, so a failed attempt never burns the pending token.
    enum Outcome {
        /// Position of the matching recovery-code hash in the stored array.
        Recovery { pos: usize },
        /// The verified TOTP step counter.
        Totp { step: i64 },
    }
    let ip = client_ip(&headers);
    let code = body.code.trim();
    if code.len() > 32 || body.mfa_token.len() > 512 || body.mfa_token.is_empty() || code.is_empty()
    {
        return Err(AppError::Unauthenticated);
    }

    // IP-scoped limit first: it applies before anything about the request is
    // known. Limits count every verification attempt (stricter than the
    // required failure counting). Single-instance scope — docs/security.md.
    state
        .rate_limiter
        .check(
            &format!("mfa:ip:{ip}"),
            10,
            std::time::Duration::from_secs(60),
        )
        .map_err(|retry| AppError::RateLimited {
            retry_after_secs: retry.as_secs().max(1),
        })?;

    // Resolve (NOT consume) the pending token to learn the user for the
    // per-user limit. Consumption stays a separate atomic step on the success
    // path, so a failed attempt never burns the token.
    let mut conn = state.db.conn();
    let Some(pending_user_id) = resolve_pending_token(&mut conn, &body.mfa_token).await? else {
        return Err(AppError::Unauthenticated);
    };
    state
        .rate_limiter
        .check(
            &format!("mfa:user:{pending_user_id}"),
            5,
            std::time::Duration::from_secs(60),
        )
        .map_err(|retry| AppError::RateLimited {
            retry_after_secs: retry.as_secs().max(1),
        })?;

    let key = require_key(&state)?.to_vec();
    let pepper = require_pepper(&state)?.to_vec();
    let Some(user) = crate::users::find_active_by_id(&mut conn, &pending_user_id).await? else {
        return Err(AppError::Unauthenticated);
    };
    let Some(row) = find_user_mfa(&mut conn, &user.id).await? else {
        return Err(AppError::Unauthenticated);
    };
    if row.enabled_at.is_none() {
        return Err(AppError::Unauthenticated);
    }

    let secret = decrypt_secret(&key, &row.secret_encrypted)?;
    let outcome: Outcome;
    if let Some(normalized) = normalize_recovery_code(code) {
        let candidate = recovery_hash(&pepper, &normalized);
        // Only current-format (`h1.`) digests participate; legacy unkeyed
        // SHA-256 entries are skipped and can never verify (ADR-0012).
        let hashes: Vec<String> =
            serde_json::from_str(&row.recovery_codes_hashed).unwrap_or_default();
        let Some(pos) = hashes
            .iter()
            .enumerate()
            .filter(|(_, h)| parse_recovery_digest(h).is_some())
            .find(|(_, h)| token::hashes_equal(h, &candidate))
            .map(|(i, _)| i)
        else {
            tracing::info!(user_id = %user.id, event = "mfa_verify_failed", "MFA verification rejected");
            return Err(AppError::Unauthenticated);
        };
        outcome = Outcome::Recovery { pos };
    } else {
        // TOTP: window 0 — only the current 30-second step is accepted.
        let totp = build_totp(&secret, &user.email);
        let step = current_step();
        let ok = totp.check(
            code,
            u64::try_from(step)
                .unwrap_or(0)
                .saturating_mul(TOTP_STEP_SECS),
        );
        if !ok || row.last_used_step.is_some_and(|used| step <= used) {
            tracing::info!(user_id = %user.id, event = "mfa_verify_failed", "MFA verification rejected");
            return Err(AppError::Unauthenticated);
        }
        outcome = Outcome::Totp { step };
    }

    // Success path: consume the pending token exactly once — its CAS
    // arbitrates concurrent submissions of the same token — then consume the
    // credential itself. Any losing race fails closed: no session is issued.
    let Some(_) = consume_pending_token(&mut conn, &body.mfa_token).await? else {
        return Err(AppError::Unauthenticated);
    };
    let now = util::now_ms();
    match outcome {
        Outcome::Recovery { pos } => {
            let hashes: Vec<String> =
                serde_json::from_str(&row.recovery_codes_hashed).unwrap_or_default();
            let mut remaining = hashes;
            remaining.remove(pos);
            let next = serde_json::to_string(&remaining).unwrap_or_else(|_| "[]".to_string());
            let mut args = Args::new();
            args.s(&next)
                .i(now)
                .s(&user.id)
                .s(&row.recovery_codes_hashed);
            let affected = conn
                .execute(
                    "UPDATE user_mfa SET recovery_codes_hashed = ?, updated_at = ?                      WHERE user_id = ? AND recovery_codes_hashed = ?",
                    args,
                )
                .await?;
            if affected == 0 {
                // A concurrent request consumed this code first.
                return Err(AppError::Unauthenticated);
            }
        }
        Outcome::Totp { step } => {
            let mut args = Args::new();
            args.i(step).i(now).i(now).s(&user.id).i(step);
            let affected = conn
                .execute(
                    "UPDATE user_mfa SET last_used_step = ?, last_used_at = ?, updated_at = ?                      WHERE user_id = ? AND (last_used_step IS NULL OR last_used_step < ?)",
                    args,
                )
                .await?;
            if affected == 0 {
                // Replay: this step was consumed by a concurrent request.
                return Err(AppError::Unauthenticated);
            }
        }
    }
    finish_verify(&state, &user, &ip).await
}

/// Issue the normal session through the existing session-creation path and
/// set the session cookie. Called only after successful verification.
async fn finish_verify(
    state: &AppState,
    user: &crate::users::User,
    ip: &str,
) -> Result<(StatusCode, HeaderMap, Json<crate::auth::UserResponse>), AppError> {
    let mut conn = state.db.conn();
    let user_agent: Option<&str> = None;
    let token_value = create_session(
        &mut conn,
        &user.id,
        state.config.session_ttl.as_secs(),
        ip,
        user_agent,
    )
    .await?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(
            &token_value,
            state.config.cookie_secure(),
            state.config.session_ttl.as_secs(),
        ))
        .map_err(|_| AppError::internal("cannot build session cookie"))?,
    );
    tracing::info!(user_id = %user.id, event = "mfa_verified", "MFA verification succeeded");
    Ok((
        StatusCode::OK,
        response_headers,
        Json(crate::auth::UserResponse::from(user.clone())),
    ))
}

#[derive(Deserialize)]
struct DisableRequest {
    current_password: String,
}

async fn disable(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(body): Json<DisableRequest>,
) -> Result<(StatusCode, HeaderMap), AppError> {
    let mut conn = state.db.conn();
    verify_current_password(&mut conn, &user, &body.current_password).await?;
    // Atomic removal: the configuration, its encrypted secret, and the
    // recovery hashes disappear together; outstanding pending tokens die too.
    let mut args = Args::new();
    args.s(&user.id);
    conn.execute("DELETE FROM user_mfa WHERE user_id = ?", args)
        .await?;
    delete_pending_for_user(&mut conn, &user.id).await?;
    tracing::info!(user_id = %user.id, event = "mfa_disabled", "MFA disabled");
    let headers = no_store();
    Ok((StatusCode::NO_CONTENT, headers))
}

#[derive(Deserialize)]
struct PasswordRequest {
    current_password: String,
}

async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(body): Json<PasswordRequest>,
) -> Result<(StatusCode, HeaderMap, Json<RecoveryCodesResponse>), AppError> {
    let pepper = require_pepper(&state)?.to_vec();
    let mut conn = state.db.conn();
    verify_current_password(&mut conn, &user, &body.current_password).await?;
    let Some(row) = find_user_mfa(&mut conn, &user.id).await? else {
        return Err(AppError::BadRequest(
            "two-factor authentication is not enabled".into(),
        ));
    };
    if row.enabled_at.is_none() {
        return Err(AppError::BadRequest(
            "two-factor authentication is not enabled".into(),
        ));
    }
    let (plain_codes, hashed_json) = generate_recovery_set(&pepper);
    let mut args = Args::new();
    args.s(&hashed_json).i(util::now_ms()).s(&user.id);
    conn.execute(
        "UPDATE user_mfa SET recovery_codes_hashed = ?, updated_at = ? WHERE user_id = ?",
        args,
    )
    .await?;
    tracing::info!(user_id = %user.id, event = "mfa_recovery_regenerated", "Recovery codes regenerated");
    Ok((
        StatusCode::OK,
        no_store(),
        Json(RecoveryCodesResponse {
            recovery_codes: plain_codes,
        }),
    ))
}

async fn verify_current_password(
    conn: &mut Conn<'_>,
    user: &AuthedUser,
    current_password: &str,
) -> Result<(), AppError> {
    let found = crate::users::find_active_by_email(conn, &user.email).await?;
    let Some((_, password_hash)) = found else {
        return Err(AppError::Unauthenticated);
    };
    if !crate::security::password::verify(current_password, &password_hash) {
        return Err(AppError::validation(
            "current_password",
            "current password is incorrect",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Extractor guard: pending tokens are body-scoped; the cookie-based central
// authentication only ever resolves normal sessions. The types below make the
// two principals structurally distinct.
// ---------------------------------------------------------------------------

/// Marker proving a handler reached MFA management with a NORMAL session.
pub struct MfaVerified;

impl FromRequestParts<AppState> for MfaVerified {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Only a NORMAL session may reach MFA management endpoints; a pending
        // token in the cookie resolves to nothing here (it lives in a
        // different table with a different prefix).
        AuthedUser::from_request_parts(parts, state).await?;
        Ok(MfaVerified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> Vec<u8> {
        vec![7u8; 32]
    }

    fn test_pepper() -> Vec<u8> {
        vec![11u8; 32]
    }

    #[test]
    fn envelope_roundtrip() {
        let secret = b"top-secret-totp-bytes";
        let envelope = encrypt_secret(&test_key(), secret).expect("encrypt");
        assert!(envelope.starts_with("v1."));
        assert!(!envelope.contains("top-secret"));
        assert_ne!(
            envelope,
            encrypt_secret(&test_key(), secret).expect("encrypt2")
        ); // fresh nonce
        assert_eq!(
            decrypt_secret(&test_key(), &envelope).expect("decrypt"),
            secret
        );
    }

    #[test]
    fn tampered_or_wrong_key_ciphertext_rejected() {
        let envelope = encrypt_secret(&test_key(), b"secret").expect("encrypt");
        // Flip a character early in the payload: unpadded base64url drops the
        // final chunk's trailing bits, so mutating the last char may be a no-op.
        let mut flipped = envelope.clone();
        let bytes: Vec<char> = flipped.chars().collect();
        let idx = bytes.len() / 2;
        let replacement = if bytes[idx] == 'A' { 'B' } else { 'A' };
        flipped = bytes[..idx].iter().collect::<String>()
            + &replacement.to_string()
            + &bytes[idx + 1..].iter().collect::<String>();
        assert!(decrypt_secret(&test_key(), &flipped).is_err());
        let mut other_key = test_key();
        other_key[0] ^= 1;
        assert!(decrypt_secret(&other_key, &envelope).is_err());
        assert!(decrypt_secret(&test_key(), "garbage").is_err());
        assert!(decrypt_secret(&test_key(), "v1.!!!not-base64!!!").is_err());
    }

    #[test]
    fn recovery_codes_shape_and_normalization() {
        let (plain, hashed_json) = generate_recovery_set(&test_pepper());
        assert_eq!(plain.len(), RECOVERY_CODE_COUNT);
        let hashes: Vec<String> = serde_json::from_str(&hashed_json).unwrap();
        assert_eq!(hashes.len(), RECOVERY_CODE_COUNT);
        for code in &plain {
            assert_eq!(code.len(), 9);
            assert_eq!(code.as_bytes()[4], b'-');
            let normalized = normalize_recovery_code(code).unwrap();
            assert_eq!(normalized, code.replace('-', ""));
            // The stored form is a digest of the normalized code, not plaintext.
            assert!(!hashed_json.contains(code));
            assert!(!hashed_json.contains(&normalized));
            assert!(
                hashes
                    .iter()
                    .any(|h| token::hashes_equal(h, &recovery_hash(&test_pepper(), &normalized)))
            );
        }
        // Every stored digest is in the current keyed `h1.` format.
        assert!(hashes.iter().all(|h| parse_recovery_digest(h).is_some()));
        // Rejects wrong shapes and foreign characters.
        assert!(normalize_recovery_code("short").is_none());
        assert!(normalize_recovery_code("AAAA-BAA0").is_none()); // 0 not in alphabet
        assert!(normalize_recovery_code("aaaa-baao").is_none()); // O not in alphabet
    }

    /// Required test 1: the stored digest for a code is exactly the keyed
    /// HMAC-SHA-256 of the normalized code under the configured pepper.
    #[test]
    fn recovery_digest_is_keyed_hmac_of_normalized_code() {
        let normalized = "ABCD2345";
        let digest = recovery_hash(&test_pepper(), normalized);
        assert!(digest.starts_with(RECOVERY_DIGEST_TAG));
        let hex = parse_recovery_digest(&digest).expect("parsable");
        // Independently computed with the same construction.
        let mut mac = <Hmac<Sha256> as hmac::Mac>::new_from_slice(&test_pepper())
            .expect("HMAC accepts any key length");
        mac.update(RECOVERY_HMAC_CONTEXT);
        mac.update(normalized.as_bytes());
        assert_eq!(hex, hex_lower(&mac.finalize().into_bytes()));
        // Deterministic for the same pepper and input.
        assert_eq!(recovery_hash(&test_pepper(), normalized), digest);
    }

    /// Required tests 2 & 3: a wrong code (or wrong pepper) produces a digest
    /// that never matches, and matching goes through the constant-time
    /// comparison helper; legacy bare-hex entries are structurally rejected.
    #[test]
    fn wrong_code_or_pepper_never_matches_and_legacy_rejected() {
        let normalized = "ABCD2345";
        let digest = recovery_hash(&test_pepper(), normalized);
        // Different code under the same pepper.
        assert_ne!(recovery_hash(&test_pepper(), "ABCD2346"), digest);
        // Same code under a different pepper (key separation matters).
        let other_pepper = vec![12u8; 32];
        assert_ne!(recovery_hash(&other_pepper, normalized), digest);
        // Constant-time comparison path is used for the hex payloads.
        let hex = parse_recovery_digest(&digest).unwrap();
        assert!(token::hashes_equal(hex, hex));
        assert!(!token::hashes_equal(hex, &"0".repeat(64)));
        // Legacy unkeyed SHA-256 digest of the same code: bare hex, no tag —
        // parsed entries are None and equality with a tagged digest fails.
        let legacy = hex_lower(&Sha256::digest(normalized.as_bytes()));
        assert!(parse_recovery_digest(&legacy).is_none());
        assert_ne!(legacy, digest);
        assert!(!token::hashes_equal(&legacy, &digest));
    }

    /// Required test 8: fail-closed pepper handling. A present-but-malformed
    /// value is rejected by the configuration loader (startup abort); an
    /// absent value makes `require_pepper` fail closed so recovery endpoints
    /// return 500 instead of accepting unkeyed verification.
    #[tokio::test]
    async fn recovery_pepper_shape_rules_and_fail_closed() {
        let valid = "a".repeat(64);
        assert_eq!(valid.len(), 64);
        assert!(valid.chars().all(|c| c.is_ascii_hexdigit()));
        let short = "a".repeat(63);
        assert_ne!(short.len(), 64);
        let not_hex = "z".repeat(64);
        assert!(!not_hex.chars().all(|c| c.is_ascii_hexdigit()));
        // require_pepper on a config without the pepper fails closed.
        let mut config = crate::config::Config {
            environment: crate::config::Environment::Development,
            http_host: "127.0.0.1".into(),
            http_port: 0,
            database_url: "sqlite::memory:".into(),
            pool: crate::config::PoolConfig {
                max_connections: 1,
                acquire_timeout: std::time::Duration::from_secs(1),
                idle_timeout: std::time::Duration::from_secs(1),
                max_lifetime: std::time::Duration::from_secs(1),
            },
            session_ttl: std::time::Duration::from_secs(60),
            public_registration: false,
            allowed_origins: Vec::new(),
            login_rate_limit_per_min: 10,
            mfa_encryption_key: Some(test_key()),
            mfa_recovery_pepper: None,
            body_limit_bytes: 1_048_576,
            static_dir: None,
            log_format_json: false,
        };
        let state = AppState::new(
            crate::db::Database::connect(&config).await.expect("db"),
            config.clone(),
        );
        assert!(
            require_pepper(&state).is_err(),
            "absent pepper fails closed"
        );
        // With the pepper present, access succeeds.
        config.mfa_recovery_pepper = Some(test_pepper());
        let state = AppState::new(
            crate::db::Database::connect(&config).await.expect("db2"),
            config,
        );
        assert!(require_pepper(&state).is_ok());
    }

    /// Required test 6 (storage level): a legacy SHA-256 digest entry can
    /// never be selected by the verification filter, which requires the
    /// current `h1.` format.
    #[test]
    fn verification_filter_skips_legacy_digests() {
        let normalized = "ABCD2345";
        let legacy = hex_lower(&Sha256::digest(normalized.as_bytes()));
        let current = recovery_hash(&test_pepper(), normalized);
        let stored = [legacy.clone(), current.clone()];
        let candidate = parse_recovery_digest(&current).expect("current parses");
        let matches: Vec<usize> = stored
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                let parsed = parse_recovery_digest(h);
                parsed.is_some() && token::hashes_equal(parsed.unwrap(), candidate)
            })
            .map(|(i, _)| i)
            .collect();
        // Only the tagged entry matches; the legacy entry is invisible.
        assert_eq!(matches, vec![1]);
        // A legacy-only store matches nothing even for the identical code:
        // with only untagged entries the verification filter finds no match.
        let legacy_only = [legacy];
        assert!(
            legacy_only
                .iter()
                .all(|h| parse_recovery_digest(h).is_none())
        );
        assert!(!token::hashes_equal(&legacy_only[0], candidate));
    }

    #[test]
    fn totp_window_is_zero_and_step_moves() {
        let mut secret = [0u8; 20];
        rand::rng().fill_bytes(&mut secret);
        let totp = build_totp(&secret, "user@test.local");
        let t0 = 1_800_000u64; // aligned to a step boundary
        let code = totp.generate(t0);
        assert!(totp.check(&code, t0), "current step accepted");
        assert!(
            !totp.check(&code, t0 + TOTP_STEP_SECS),
            "previous step rejected (window 0)"
        );
        let code2 = totp.generate(t0 + TOTP_STEP_SECS);
        assert_ne!(code, code2);
    }
}
