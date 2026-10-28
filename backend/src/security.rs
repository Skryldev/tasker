//! Security primitives: password hashing, session token generation,
//! and the in-memory login rate limiter.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub mod password {
    //! Argon2id password hashing (OWASP-recommended default parameters:
    //! 19 MiB memory, 2 iterations, parallelism 1).

    use argon2::Argon2;
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

    /// Hash a password with Argon2id. Never returns the input in errors.
    pub fn hash(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| format!("password hashing failed: {e}"))
    }

    /// Verify a password against a stored PHC-format hash in constant time.
    pub fn verify(password: &str, stored_hash: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(stored_hash) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }

    /// Minimum accepted password length for user-chosen passwords.
    pub const MIN_PASSWORD_LEN: usize = 10;

    /// Maximum accepted password length (guards against hashing DoS).
    pub const MAX_PASSWORD_LEN: usize = 128;

    pub fn validate(password: &str) -> Result<(), String> {
        if password.len() < MIN_PASSWORD_LEN {
            return Err(format!(
                "password must be at least {MIN_PASSWORD_LEN} characters"
            ));
        }
        if password.len() > MAX_PASSWORD_LEN {
            return Err(format!(
                "password must be at most {MAX_PASSWORD_LEN} characters"
            ));
        }
        Ok(())
    }
}

pub mod token {
    //! Session token generation and hashing.
    //!
    //! Tokens are 256 bits of OS randomness, base64url-encoded. Only the
    //! SHA-256 hash is persisted; the raw token exists solely in the cookie.

    use rand::RngCore;
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;

    /// Generate a new cryptographically random session token.
    #[must_use]
    pub fn generate() -> String {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        base64_url_encode(&bytes)
    }

    /// SHA-256 hex digest of a session token (what the database stores).
    #[must_use]
    pub fn hash(token: &str) -> String {
        let digest = Sha256::digest(token.as_bytes());
        hex_encode(&digest)
    }

    /// Constant-time comparison of two token hashes.
    #[must_use]
    pub fn hashes_equal(a: &str, b: &str) -> bool {
        a.as_bytes().ct_eq(b.as_bytes()).into()
    }

    /// Base64url encoding without padding (RFC 4648 §5): trailing bits for a
    /// partial final chunk are dropped, and no `=` padding is emitted.
    pub fn base64_url_encode(data: &[u8]) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).map_or(0, |b| *b as u32);
            let b2 = chunk.get(2).map_or(0, |b| *b as u32);
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(CHARS[(n >> 18) as usize & 63] as char);
            out.push(CHARS[(n >> 12) as usize & 63] as char);
            if chunk.len() > 1 {
                out.push(CHARS[(n >> 6) as usize & 63] as char);
            }
            if chunk.len() > 2 {
                out.push(CHARS[n as usize & 63] as char);
            }
        }
        out
    }

    fn hex_encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len() * 2);
        for b in data {
            out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(b & 0x0F), 16).unwrap_or('0'));
        }
        out
    }
}

/// Fixed-window in-memory rate limiter keyed by arbitrary strings
/// (e.g. `login:203.0.113.9`). Single-instance scope by design; horizontal
/// deployments should front the service with a shared limiter.
pub struct RateLimiter {
    windows: Mutex<HashMap<String, Window>>,
}

struct Window {
    started: Instant,
    count: u32,
}

impl RateLimiter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Allow one request if the fixed window for `key` is below `limit`.
    /// Returns the retry delay when the limit is exceeded.
    pub fn check(&self, key: &str, limit: u32, window: Duration) -> Result<(), Duration> {
        let now = Instant::now();
        let mut windows = self.windows.lock().expect("rate limiter poisoned");
        // Opportunistic sweep to bound memory.
        if windows.len() > 10_000 {
            windows.retain(|_, w| now.duration_since(w.started) < window);
        }
        let entry = windows.entry(key.to_string()).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.duration_since(entry.started) >= window {
            entry.started = now;
            entry.count = 0;
        }
        if entry.count >= limit {
            let elapsed = now.duration_since(entry.started);
            return Err(window.saturating_sub(elapsed).max(Duration::from_secs(1)));
        }
        entry.count += 1;
        Ok(())
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_and_verify_roundtrip() {
        let hash = password::hash("correct horse battery").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(password::verify("correct horse battery", &hash));
        assert!(!password::verify("wrong password", &hash));
    }

    #[test]
    fn password_validation_bounds() {
        assert!(password::validate("short").is_err());
        assert!(password::validate(&"a".repeat(200)).is_err());
        assert!(password::validate("a-good-length-passphrase").is_ok());
    }

    #[test]
    fn tokens_are_random_and_hashable() {
        let t1 = token::generate();
        let t2 = token::generate();
        assert_ne!(t1, t2);
        assert_eq!(t1.len(), 43); // 32 bytes -> 43 base64url chars (unpadded)
        let h = token::hash(&t1);
        assert_eq!(h.len(), 64);
        assert!(token::hashes_equal(&h, &token::hash(&t1)));
        assert!(!token::hashes_equal(&h, &token::hash(&t2)));
    }

    #[test]
    fn rate_limiter_blocks_after_limit() {
        let rl = RateLimiter::new();
        for _ in 0..3 {
            assert!(
                rl.check("login:1.2.3.4", 3, Duration::from_secs(60))
                    .is_ok()
            );
        }
        assert!(
            rl.check("login:1.2.3.4", 3, Duration::from_secs(60))
                .is_err()
        );
        assert!(
            rl.check("login:5.6.7.8", 3, Duration::from_secs(60))
                .is_ok()
        );
    }
}
