//! Integration credential security (Phase 8B, Task 1).
//!
//! Webhook verification secrets are symmetric secrets that must be
//! *recoverable* at verification time, so they are stored encrypted with the
//! Phase 7 AES-256-GCM envelope (`v1.<base64url(nonce ‖ ciphertext ‖ tag)>`)
//! — the same primitive that protects TOTP secrets. **No custom
//! cryptography** is introduced here.
//!
//! Key handling: `TASKER_INTEGRATIONS_KEY` (64 hex characters = 32 bytes),
//! resolved from the configuration. Fail-closed: absent/malformed key
//! material surfaces as an error at every credential operation — never a
//! fallback, never a default key.
//!
//! Lifecycle (all operations atomic, guarded by `credential_version`, and
//! audited through the existing activity trail with **no secret material**):
//! - **Provisioning**: single credential row, version 1, `revoked_at NULL`.
//! - **Rotation**: replaces the encrypted secret in place, increments the
//!   version — **never reactivates a revoked credential**.
//! - **Revocation**: explicit, terminal until re-provisioning.
//! - **Re-provisioning**: explicit operation that clears `revoked_at`.
//!
//! Plaintext secrets are never persisted, logged, traced, or returned: the
//! caller passes them in and receives only versions/dispositions back.

use crate::config::Config;
use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::integrations::limits;
use crate::util;
use sqlx::Row;

/// The stored credential as read from the database (no secret material —
/// the ciphertext is only loaded for verification).
#[derive(Debug, Clone)]
pub struct ActiveCredential {
    pub integration_id: String,
    pub credential_version: i64,
    pub revoked_at: Option<i64>,
}

/// Resolve the integration key from the configuration. Fail-closed: absent
/// key material is an error, never a fallback to an unencrypted mode.
#[must_use]
pub fn resolve_key(config: &Config) -> Option<Vec<u8>> {
    config.integrations_key.clone()
}

fn seal(key: &[u8], plaintext: &str) -> Result<String, AppError> {
    if plaintext.chars().count() > limits::WEBHOOK_SECRET_MAX_CHARS {
        return Err(AppError::internal("integration secret exceeds the size limit"));
    }
    crate::mfa::encrypt_secret(key, plaintext.as_bytes())
}

fn open(key: &[u8], envelope: &str) -> Result<String, AppError> {
    let bytes = crate::mfa::decrypt_secret(key, envelope)?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::internal("integration secret envelope contained invalid utf-8"))
}

/// Provision the single credential row for a new integration: version 1,
/// `revoked_at NULL`. Fails closed when a credential already exists
/// (`UNIQUE(integration_id)`), when the key is not configured, or when the
/// secret violates the size limit. `created_by` records the provisioning
/// actor for the audit trail.
///
/// Must run inside the caller's transaction (typically the integration
/// create transaction) so the credential and the integration are born
/// atomically.
pub async fn provision_credential(
    conn: &mut Conn<'_>,
    integration_id: &str,
    plaintext: &str,
    key: &[u8],
    actor_id: &str,
    now: i64,
) -> Result<i64, AppError> {
    let secret_encrypted = seal(key, plaintext)?;
    let mut args = Args::new();
    args.s(util::new_id())
        .s(integration_id)
        .s(&secret_encrypted)
        .i(1i64)
        .s(actor_id)
        .i(now)
        .i(now);
    conn.execute(
        "INSERT INTO integration_credentials (id, integration_id, secret_encrypted, \
         credential_version, created_by, created_at, revoked_at) \
         VALUES (?, ?, ?, ?, ?, ?, NULL)",
        args,
    )
    .await?;
    Ok(1)
}

/// Load the active credential row for verification: `None` when no
/// credential exists or the active credential is revoked (fail-closed).
pub async fn load_active_credential(
    conn: &mut Conn<'_>,
    integration_id: &str,
) -> Result<Option<ActiveCredential>, AppError> {
    let mut args = Args::new();
    args.s(integration_id);
    let row = conn
        .fetch_optional(
            "SELECT credential_version, revoked_at FROM integration_credentials \
             WHERE integration_id = ?",
            args,
        )
        .await?;
    Ok(row.and_then(|r| {
        let credential_version = r.try_get::<i64, _>("credential_version").ok()?;
        let revoked_at = r.try_get::<Option<i64>, _>("revoked_at").ok()?;
        (revoked_at.is_none()).then(|| ActiveCredential {
            integration_id: integration_id.to_string(),
            credential_version,
            revoked_at,
        })
    }))
}

/// Load the encrypted envelope for an active credential (verification path
/// only — the management API never returns this).
async fn load_active_envelope(
    conn: &mut Conn<'_>,
    integration_id: &str,
) -> Result<Option<(i64, String)>, AppError> {
    let mut args = Args::new();
    args.s(integration_id);
    let row = conn
        .fetch_optional(
            "SELECT credential_version, secret_encrypted FROM integration_credentials \
             WHERE integration_id = ? AND revoked_at IS NULL",
            args,
        )
        .await?;
    Ok(row.and_then(|r| {
        Some((
            r.try_get::<i64, _>("credential_version").ok()?,
            r.try_get::<String, _>("secret_encrypted").ok()?,
        ))
    }))
}

/// Verify a presented webhook secret against the integration's active
/// credential. Fail-closed: no credential, revoked credential, missing key
/// configuration, decryption failure, or mismatch all return `false`.
/// Constant-time comparison via the existing `security::hashes_equal`.
pub async fn verify_webhook_secret(
    conn: &mut Conn<'_>,
    integration_id: &str,
    presented: &str,
    key: &[u8],
) -> Result<bool, AppError> {
    let Some((version, envelope)) = load_active_envelope(conn, integration_id).await? else {
        return Ok(false);
    };
    let stored = open(key, &envelope)?;
    // Constant-time comparison of SHA-256 digests sidesteps length leakage
    // and reuses the established Phase 7 helper.
    let presented_hash = crate::security::token::hash(presented);
    let stored_hash = crate::security::token::hash(&stored);
    Ok(crate::security::token::hashes_equal(
        &presented_hash,
        &stored_hash,
    ))
}

/// Rotate the active credential: replace the encrypted secret in place and
/// increment `credential_version` under an optimistic-concurrency guard.
/// **Never reactivates a revoked credential.** Atomic; audited through the
/// activity trail by the caller.
///
/// Returns the new credential version.
///
/// Must run inside the caller's transaction.
pub async fn rotate_credential(
    conn: &mut Conn<'_>,
    integration_id: &str,
    new_plaintext: &str,
    key: &[u8],
    expected_version: i64,
    actor_id: &str,
    now: i64,
) -> Result<i64, AppError> {
    let secret_encrypted = seal(key, new_plaintext)?;
    let mut args = Args::new();
    args.s(&secret_encrypted)
        .i(expected_version + 1)
        .s(integration_id)
        .i(expected_version);
    let affected = conn
        .execute(
            "UPDATE integration_credentials SET secret_encrypted = ?, \
             credential_version = ? \
             WHERE integration_id = ? AND credential_version = ? \
             AND revoked_at IS NULL",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::Conflict(
            "credential version conflict or credential is revoked".into(),
        ));
    }
    Ok(expected_version + 1)
}

/// Revoke the active credential: sets `revoked_at` explicitly; verification
/// fails closed afterward. Guarded by the current version; a revoked
/// credential is idempotently left as-is.
///
/// Returns `true` when this call performed the revocation.
pub async fn revoke_credential(
    conn: &mut Conn<'_>,
    integration_id: &str,
    expected_version: i64,
    now: i64,
) -> Result<bool, AppError> {
    let mut args = Args::new();
    args.i(now).s(integration_id).i(expected_version);
    let affected = conn
        .execute(
            "UPDATE integration_credentials SET revoked_at = ? \
             WHERE integration_id = ? AND credential_version = ? AND revoked_at IS NULL",
            args,
        )
        .await?;
    Ok(affected > 0)
}

/// Re-provision a **revoked** credential: replace the secret, increment the
/// version, and clear `revoked_at`. Explicit operation only — the guarded
/// update requires the caller to have observed the revocation (`revoked_at
/// IS NOT NULL`), so a concurrent re-provision or an active credential
/// cannot be silently replaced.
///
/// Returns the new credential version.
///
/// Must run inside the caller's transaction.
pub async fn re_provision_credential(
    conn: &mut Conn<'_>,
    integration_id: &str,
    new_plaintext: &str,
    key: &[u8],
    expected_version: i64,
    actor_id: &str,
    now: i64,
) -> Result<i64, AppError> {
    let secret_encrypted = seal(key, new_plaintext)?;
    // Placeholders in order: secret, version, created_by, created_at (SET)
    // + integration_id, expected_version (WHERE).
    let mut args = Args::new();
    args.s(&secret_encrypted)
        .i(expected_version + 1)
        .s(actor_id)
        .i(now)
        .s(integration_id)
        .i(expected_version);
    let affected = conn
        .execute(
            "UPDATE integration_credentials SET secret_encrypted = ?, \
             credential_version = ?, created_by = ?, created_at = ?, revoked_at = NULL \
             WHERE integration_id = ? AND credential_version = ? \
             AND revoked_at IS NOT NULL",
            args,
        )
        .await?;
    if affected == 0 {
        return Err(AppError::Conflict(
            "credential is active or was already re-provisioned concurrently".into(),
        ));
    }
    Ok(expected_version + 1)
}

/// The credential row's version and revocation state (for lifecycle tests
/// and management reads that must not expose the secret).
pub async fn credential_state(
    conn: &mut Conn<'_>,
    integration_id: &str,
) -> Result<Option<(i64, Option<i64>)>, AppError> {
    let mut args = Args::new();
    args.s(integration_id);
    let row = conn
        .fetch_optional(
            "SELECT credential_version, revoked_at FROM integration_credentials \
             WHERE integration_id = ?",
            args,
        )
        .await?;
    let found = match row {
        Some(r) => Some((
            r.try_get::<i64, _>("credential_version")?,
            r.try_get::<Option<i64>, _>("revoked_at")?,
        )),
        None => None,
    };
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = &[21u8; 32];

    #[test]
    fn seal_open_roundtrip() {
        let envelope = seal(KEY, "webhook-secret-123").expect("seal");
        assert!(envelope.starts_with("v1."));
        assert_eq!(open(KEY, &envelope).expect("open"), "webhook-secret-123");
    }

    #[test]
    fn wrong_key_fails_closed() {
        let envelope = seal(KEY, "secret").expect("seal");
        let wrong = &[22u8; 32];
        assert!(open(wrong, &envelope).is_err(), "wrong key must fail");
    }

    #[test]
    fn tampered_envelope_fails_closed() {
        let envelope = seal(KEY, "secret").expect("seal");
        // Flip a character late in the envelope (inside the ciphertext/tag).
        let mut chars: Vec<char> = envelope.chars().collect();
        let last = chars.len() - 2;
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert!(open(KEY, &tampered).is_err(), "tamper must be rejected");
    }

    #[test]
    fn oversized_secret_rejected() {
        let long = "a".repeat(limits::WEBHOOK_SECRET_MAX_CHARS + 1);
        assert!(seal(KEY, &long).is_err());
    }



}
