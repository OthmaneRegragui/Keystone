use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use crate::error::{AppError, AppResult};
use rand::rngs::OsRng;

pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AppError::Internal(format!("failed to hash password: {e}")))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> AppResult<bool> {
    let parsed_hash = PasswordHash::new(hash)
        .map_err(|e| AppError::BadRequest(format!("invalid password hash format: {e}")))?;
    let valid = argon2::Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok();
    Ok(valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_and_verify() {
        let password = "super_secret_123";
        let hash = hash_password(password).unwrap();
        assert!(verify_password(password, &hash).unwrap());
    }

    #[test]
    fn test_verify_wrong_password() {
        let hash = hash_password("correct_password").unwrap();
        assert!(!verify_password("wrong_password", &hash).unwrap());
    }

    #[test]
    fn test_different_hashes_for_same_password() {
        let hash1 = hash_password("password").unwrap();
        let hash2 = hash_password("password").unwrap();
        assert_ne!(hash1, hash2);
        assert!(verify_password("password", &hash1).unwrap());
        assert!(verify_password("password", &hash2).unwrap());
    }

    #[test]
    fn test_invalid_hash_format() {
        assert!(verify_password("password", "not_a_valid_hash").is_err());
    }
}
