use rand::Rng;
use sha2::{Digest, Sha256};

const BASE62: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

pub fn generate_api_key() -> (String, String, String) {
    let mut rng = rand::thread_rng();
    let suffix: String = (0..40)
        .map(|_| BASE62[rng.gen_range(0..BASE62.len())] as char)
        .collect();

    let full_key = format!("ks_live_{}", suffix);
    let prefix = full_key[..12].to_string();
    let hash = hash_api_key(&full_key);

    (full_key, prefix, hash)
}

pub fn hash_api_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn validate_api_key(key: &str, expected_hash: &str) -> bool {
    let computed = hash_api_key(key);
    let computed = computed.as_bytes();
    let expected = expected_hash.as_bytes();
    if computed.len() != expected.len() {
        return false;
    }
    // Constant-time comparison (no early exit): both values are SHA-256 hex
    // (always 64 bytes) so the length check leaks nothing.
    let mut diff = 0u8;
    for (a, b) in computed.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn parse_key_prefix(key: &str) -> Option<&str> {
    // `get` instead of byte-slicing: never panics on a multi-byte character
    // boundary (attacker-supplied keys may be arbitrary UTF-8).
    key.get(..12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_api_key_format() {
        let (full_key, prefix, hash) = generate_api_key();
        assert!(full_key.starts_with("ks_live_"));
        assert_eq!(full_key.len(), 48); // "ks_live_" (8) + 40 chars
        assert_eq!(prefix.len(), 12);
        assert_eq!(prefix, &full_key[..12]);
        assert_eq!(hash.len(), 64); // SHA256 hex = 64 chars
    }

    #[test]
    fn test_hash_api_key_deterministic() {
        let hash1 = hash_api_key("ks_live_test123");
        let hash2 = hash_api_key("ks_live_test123");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_validate_api_key() {
        let (full_key, _, hash) = generate_api_key();
        assert!(validate_api_key(&full_key, &hash));
        assert!(!validate_api_key("wrong_key", &hash));
    }

    #[test]
    fn test_parse_key_prefix() {
        let (full_key, _, _) = generate_api_key();
        let prefix = parse_key_prefix(&full_key).unwrap();
        assert_eq!(prefix, &full_key[..12]);
    }

    #[test]
    fn test_parse_key_prefix_short_key() {
        assert!(parse_key_prefix("short").is_none());
    }

    #[test]
    fn test_different_keys_different_hashes() {
        let (_, _, hash1) = generate_api_key();
        let (_, _, hash2) = generate_api_key();
        assert_ne!(hash1, hash2);
    }
}
