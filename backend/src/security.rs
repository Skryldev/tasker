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
    #[must_use]
    pub fn verify(password: &str, stored_hash: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(stored_hash) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }

    /// Minimum accepted password length for user-chosen passwords
    /// (Phase 12.5 Bug 8 policy: raised from 10).
    pub const MIN_PASSWORD_LEN: usize = 12;

    /// Maximum accepted password length (guards against hashing `DoS`).
    pub const MAX_PASSWORD_LEN: usize = 128;

    /// Identity fields the password must not match (Bug 8). All comparisons
    /// are case-insensitive; the email's local part is compared too.
    pub struct PasswordIdentity<'a> {
        pub email: &'a str,
        pub username: Option<&'a str>,
        pub display_name: &'a str,
    }

    /// Hand-curated blocklist of the most common leaked passwords (Bug 8).
    /// The spec allows a bundled hand-written list instead of the literal
    /// top-10k dump; this covers the classic top-of-the-top entries that
    /// pass length/class checks (e.g. `Password123!`). Matching is exact on
    /// the lowercased password — the class rules make exotic variants rare
    /// enough that character-class checks remain the primary defense.
    const COMMON_PASSWORDS: [&str; 122] = [
        "123456", "password", "123456789", "12345678", "12345", "qwerty", "1234567890", "1234567",
        "111111", "123123", "abc123", "1234", "password1", "iloveyou", "000000", "qwerty123",
        "1q2w3e", "aa12345678", "654321", "555555", "dragon", "qwertyuiop", "monkey", "sunshine",
        "princess", "letmein", "welcome", "admin", "admin123", "administrator", "root", "toor",
        "pass123", "passw0rd", "p@ssw0rd", "p@ssword", "pass@word1", "password!", "password123",
        "password123!", "password1!", "abcd1234", "qwe123", "1qaz2wsx", "zaq12wsx", "qwerty1",
        "qwerty12", "asdfgh", "asdfghjkl", "zxcvbnm", "1q2w3e4r", "1q2w3e4r5t", "qazwsx",
        "qazwsxedc", "trustno1", "hunter2", "hunter", "baseball", "football", "soccer",
        "hockey", "superman", "batman", "spiderman", "pokemon", "samsung", "google", "facebook",
        "nissan", "mercedes", "corvette", "ferrari", "porsche", "mustang", "starwars",
        "master", "master123", "michael", "jennifer", "jordan", "harley", "ranger", "buster",
        "thomas", "tigger", "robert", "soccer1", "jordan23", "harley1", "ranger1", "buster1",
        "summer", "winter", "spring", "autumn", "secret", "freedom", "whatever", "computer",
        "internet", "matrix", "cookie", "lovely", "hello", "hello123", "welcome1", "welcome123",
        "changeme", "changeme1", "letmein123", "test123", "test1234", "guest", "guest123",
        "user", "user123", "default", "example", "sample", "tasker", "tasker123", "zaq1@wsx",
    ];

    /// Strength scoring for the UI meter (0-4): counts satisfied policy
    /// dimensions beyond the hard requirements. UX only — the server policy
    /// is the enforcement point.
    #[must_use]
    pub fn strength(password: &str) -> u8 {
        let mut score = 0u8;
        let len = password.chars().count();
        if len >= MIN_PASSWORD_LEN {
            score += 1;
        }
        if len >= 16 {
            score += 1;
        }
        let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
        let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
        let has_digit = password.chars().any(|c| c.is_ascii_digit());
        let has_special = password
            .chars()
            .any(|c| !c.is_ascii_alphanumeric());
        let classes = u8::from(has_lower)
            + u8::from(has_upper)
            + u8::from(has_digit)
            + u8::from(has_special);
        if classes >= 3 {
            score += 1;
        }
        if classes == 4 {
            score += 1;
        }
        score.min(4)
    }

    fn validate_length(password: &str) -> Result<(), String> {
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

    fn validate_classes(password: &str) -> Result<(), String> {
        if !password.chars().any(|c| c.is_ascii_lowercase()) {
            return Err("password must contain a lowercase letter".into());
        }
        if !password.chars().any(|c| c.is_ascii_uppercase()) {
            return Err("password must contain an uppercase letter".into());
        }
        if !password.chars().any(|c| c.is_ascii_digit()) {
            return Err("password must contain a digit".into());
        }
        if !password.chars().any(|c| !c.is_ascii_alphanumeric()) {
            return Err("password must contain a special character".into());
        }
        Ok(())
    }

    /// Validate a password against the context-free policy (Bug 8):
    /// length → blocklist → character classes. The identity rule (when an
    /// identity is available) runs right after the length checks so its
    /// message is not shadowed by class errors.
    pub fn validate(password: &str) -> Result<(), String> {
        validate_length(password)?;
        let lowered = password.to_lowercase();
        if COMMON_PASSWORDS.contains(&lowered.as_str()) {
            return Err("this password is too common".into());
        }
        validate_classes(password)
    }

    /// Validate a password and additionally reject passwords derived from
    /// the user's identity fields (Bug 8).
    pub fn validate_for_identity(
        password: &str,
        identity: &PasswordIdentity,
    ) -> Result<(), String> {
        validate_length(password)?;
        let lowered = password.to_lowercase();
        let email = identity.email.trim().to_lowercase();
        let email_local = email.split('@').next().unwrap_or("").to_string();
        let username = identity.username.map(str::trim).map(str::to_lowercase);
        let display = identity.display_name.trim().to_lowercase();
        let matches_identity = (!email.is_empty() && lowered == email)
            || (!email_local.is_empty() && lowered == email_local)
            || username.is_some_and(|u| !u.is_empty() && lowered == u)
            || (!display.is_empty() && lowered == display);
        if matches_identity {
            return Err(
                "password must not match your email, username or display name".into(),
            );
        }
        if COMMON_PASSWORDS.contains(&lowered.as_str()) {
            return Err("this password is too common".into());
        }
        validate_classes(password)
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
    #[must_use]
    pub fn base64_url_encode(data: &[u8]) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b0 = u32::from(chunk[0]);
            let b1 = chunk.get(1).map_or(0, |b| u32::from(*b));
            let b2 = chunk.get(2).map_or(0, |b| u32::from(*b));
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
        assert!(password::validate("a-good-length-passphrase").is_err()); // no upper/digit/special
    }

    // Bug 8: the strict policy. Each failure case gets its specific message.
    #[test]
    fn password_policy_rejects_each_failure() {
        assert_eq!(
            password::validate("Sunny-Tr4i!").unwrap_err(),
            "password must be at least 12 characters"
        );
        assert_eq!(
            password::validate("sunny-trail-42x!").unwrap_err(),
            "password must contain an uppercase letter"
        );
        assert_eq!(
            password::validate("SUNNY-TRAIL-42X!").unwrap_err(),
            "password must contain a lowercase letter"
        );
        assert_eq!(
            password::validate("Sunny-Trail-x!").unwrap_err(),
            "password must contain a digit"
        );
        assert_eq!(
            password::validate("SunnyTrail42x").unwrap_err(),
            "password must contain a special character"
        );
        assert_eq!(
            password::validate("Password123!").unwrap_err(),
            "this password is too common"
        );
        // A 12+ password with all classes passes.
        assert!(password::validate("Sunny-Trail-42x").is_ok());
    }

    #[test]
    fn password_policy_rejects_identity_matches() {
        let identity = password::PasswordIdentity {
            email: "ada@tasker.local",
            username: Some("ada-the-admin"),
            display_name: "Ada Administrator",
        };
        // Exact (case-insensitive) matches of the email, username, or
        // display name are rejected with the specific message — the identity
        // rule runs before the class rules so its message is not shadowed.
        for leak in [
            "ada@tasker.local",
            "ADA@TASKER.LOCAL",
            "Ada-The-Admin",
            "ada administrator",
        ] {
            assert_eq!(
                password::validate_for_identity(leak, &identity).unwrap_err(),
                "password must not match your email, username or display name",
                "identity leak: {leak}"
            );
        }
        // But the length rule still wins for short identity strings.
        assert_eq!(
            password::validate_for_identity("ada", &identity).unwrap_err(),
            "password must be at least 12 characters"
        );
        // A strong password unrelated to the identity passes.
        assert!(password::validate_for_identity("Sunny-Trail-42x", &identity).is_ok());
    }

    #[test]
    fn password_strength_meter_scores() {
        assert_eq!(password::strength("short"), 0);
        assert_eq!(password::strength("Sunny-Trail-42x"), 3);
        assert_eq!(password::strength("Sunny-Trail-42x!Extra"), 4);
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
