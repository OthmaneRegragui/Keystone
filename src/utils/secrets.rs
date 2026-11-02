//! Encryption of stored secrets (currently FTP passwords) at rest.
//!
//! The `ENCRYPTION_TOKEN` set at deployment is stretched to an AES-256-GCM key
//! with SHA-256 (the token must be stable across restarts; rotating it makes
//! previously-encrypted values undecryptable, exactly as the documentation
//! states). Ciphertext is stored inline in the `connections.config` JSON as an
//! `enc:v1:<base64(nonce|ciphertext)>` envelope, so any value that does not
//! carry the envelope prefix is treated as legacy plaintext and passed
//! through unchanged on read.
//!
//! A build that never initialised a token (unit tests, tooling) gets a
//! pass-through instance so legacy plaintext flows keep working.

use std::sync::OnceLock;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

/// Marker prefix stored in front of every encrypted value. Values without this
/// prefix are legacy plaintext written by older builds and are returned as-is.
pub const ENVELOPE_PREFIX: &str = "enc:v1:";
const NONCE_LEN: usize = 12;

/// Loaded once from `settings.security.encryption_token` at startup
/// (see `main.rs`). Absent in unit tests, where the pass-through constructor
/// is used instead.
static SECRETS: OnceLock<Secrets> = OnceLock::new();

pub struct Secrets {
    cipher: Option<Aes256Gcm>,
}

impl Secrets {
    /// Stretch the deployment token into an AES-256 key. A token of any
    /// length is accepted here; the minimum length is enforced at startup in
    /// production.
    pub fn from_token(token: &str) -> Self {
        if token.chars().all(|c| c.is_whitespace()) || token == "change-me-in-production" {
            tracing::warn!("ENCRYPTION_TOKEN is unset or the default value - secrets at rest are NOT encrypted");
            return Self { cipher: None };
        }
        let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let cipher = Aes256Gcm::new((&key).into());
        Self { cipher: Some(cipher) }
    }

    /// Pass-through instance: encrypt/decrypt are identity operations. Used by
    /// unit tests and tooling that has no configuration.
    pub fn disabled() -> Self {
        Self { cipher: None }
    }

    /// Install the process-wide instance. Must be called once at startup.
    pub fn install_global(self) {
        let _ = SECRETS.set(self);
    }

    /// The installed global instance, or a pass-through when never installed.
    pub fn global() -> &'static Self {
        SECRETS.get().unwrap_or(&FALLBACK)
    }

    /// Encrypt `plaintext` into the `enc:v1:` envelope. Empty and already-
    /// encrypted values pass through unchanged (idempotent).
    pub fn encrypt(&self, plaintext: &str) -> AppResult<String> {
        if plaintext.is_empty() || plaintext.starts_with(ENVELOPE_PREFIX) {
            return Ok(plaintext.to_string());
        }
        let Some(cipher) = &self.cipher else {
            return Ok(plaintext.to_string());
        };
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, Payload { msg: plaintext.as_bytes(), aad: &[] })
            .map_err(|e| AppError::Internal(format!("failed to encrypt secret: {e}")))?;
        let mut out = nonce_bytes.to_vec();
        out.extend_from_slice(&ciphertext);
        Ok(format!("{}{}", ENVELOPE_PREFIX, base64::engine::general_purpose::STANDARD.encode(out)))
    }

    /// Decrypt an `enc:v1:` envelope. Values without the prefix are legacy
    /// plaintext from older builds and pass through untouched, so rotating
    /// encryption on can be done without a data migration.
    pub fn decrypt(&self, stored: &str) -> AppResult<String> {
        let Some(pkg) = stored.strip_prefix(ENVELOPE_PREFIX) else {
            return Ok(stored.to_string());
        };
        let Some(cipher) = &self.cipher else {
            // Encrypted value but no usable key: this can only mean the token
            // changed or was cleared after the value was written. Fail loudly
            // rather than handing back ciphertext as a "password".
            return Err(AppError::Internal(
                "stored secret is encrypted but no encryption key is configured".into(),
            ));
        };
        let raw = base64::engine::general_purpose::STANDARD
            .decode(pkg)
            .map_err(|e| AppError::Internal(format!("stored secret has invalid encoding: {e}")))?;
        if raw.len() <= NONCE_LEN {
            return Err(AppError::Internal("stored secret has invalid envelope".into()));
        }
        let (nonce_bytes, ciphertext) = raw.split_at(NONCE_LEN);
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce_bytes), Payload { msg: ciphertext, aad: &[] })
            .map_err(|_| AppError::Internal(
                "stored secret failed to decrypt - the ENCRYPTION_TOKEN likely changed".into(),
            ))?;
        String::from_utf8(plaintext)
            .map_err(|_| AppError::Internal("stored secret is not valid UTF-8".into()))
    }
}

static FALLBACK: Secrets = Secrets { cipher: None };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Secrets::from_token("test-token-very-long-enough-1234");
        let enc = s.encrypt("hunter2").unwrap();
        assert!(enc.starts_with(ENVELOPE_PREFIX));
        assert_ne!(enc, "hunter2");
        // Two encryptions must differ (fresh nonce).
        let enc2 = s.encrypt("hunter2").unwrap();
        assert_ne!(enc, enc2);
        assert_eq!(s.decrypt(&enc).unwrap(), "hunter2");
        assert_eq!(s.decrypt(&enc2).unwrap(), "hunter2");
    }

    #[test]
    fn empty_and_already_encrypted_pass_through() {
        let s = Secrets::from_token("test-token-very-long-enough-1234");
        assert_eq!(s.encrypt("").unwrap(), "");
        let once = s.encrypt("x").unwrap();
        assert_eq!(s.encrypt(&once).unwrap(), once);
    }

    #[test]
    fn legacy_plaintext_decrypts_as_is() {
        let s = Secrets::disabled();
        assert_eq!(s.decrypt("hunter2").unwrap(), "hunter2");
        assert_eq!(s.encrypt("hunter2").unwrap(), "hunter2");
    }

    #[test]
    fn no_key_rejects_envelope() {
        let s = Secrets::disabled();
        assert!(s.decrypt("enc:v1:AAAA").is_err());
    }

    #[test]
    fn wrong_token_fails_closed() {
        let a = Secrets::from_token("token-one");
        let enc = a.encrypt("hunter2").unwrap();
        let b = Secrets::from_token("token-two");
        assert!(b.decrypt(&enc).is_err());
    }
}
