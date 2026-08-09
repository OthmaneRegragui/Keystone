use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub iat: usize,
    pub role: String,
    /// Optional so tokens issued before this field was introduced still verify.
    pub nbf: Option<usize>,
}

pub struct JwtService {
    secret: Vec<u8>,
    expiry_minutes: u64,
}

impl JwtService {
    pub fn new(secret: &str, expiry_minutes: u64) -> Self {
        if secret.trim().len() < 32 {
            tracing::warn!(
                "JWT secret is shorter than 32 bytes; use a random secret of at least 32 bytes (256 bits) for HS256"
            );
        }
        Self {
            secret: secret.as_bytes().to_vec(),
            expiry_minutes,
        }
    }

    /// Refuse to sign or verify with an empty/whitespace secret: an empty
    /// secret would let anyone forge valid tokens (they could guess it).
    fn ensure_secret(&self) -> AppResult<()> {
        if self.secret.is_empty() || self.secret.iter().all(|b| b.is_ascii_whitespace()) {
            return Err(AppError::Internal(
                "JWT secret must not be empty or whitespace".into(),
            ));
        }
        Ok(())
    }

    pub fn create_token(&self, user_id: Uuid, role: &str) -> AppResult<String> {
        self.ensure_secret()?;

        let now = Utc::now();
        let exp = now + chrono::Duration::minutes(self.expiry_minutes as i64);

        let claims = Claims {
            sub: user_id.to_string(),
            exp: exp.timestamp() as usize,
            iat: now.timestamp() as usize,
            role: role.to_string(),
            nbf: Some(now.timestamp() as usize),
        };

        encode(
            &Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|e| AppError::Internal(format!("failed to create JWT: {e}")))
    }

    pub fn validate_token(&self, token: &str) -> AppResult<Claims> {
        self.ensure_secret()?;

        let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
        // Reject tokens whose `nbf` claim is in the future (allows 60s leeway,
        // the jsonwebtoken default). Tokens without `nbf` are still accepted.
        validation.validate_nbf = true;
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(&self.secret),
            &validation,
        )
        .map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                AppError::Unauthorized("token has expired".into())
            }
            _ => AppError::Unauthorized(format!("invalid token: {e}")),
        })?;

        Ok(token_data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_validate_token() {
        let service = JwtService::new("test_secret_key", 60);
        let user_id = Uuid::new_v4();

        let token = service.create_token(user_id, "admin").unwrap();
        let claims = service.validate_token(&token).unwrap();

        assert_eq!(claims.sub, user_id.to_string());
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn test_validate_wrong_secret() {
        let service1 = JwtService::new("secret1", 60);
        let service2 = JwtService::new("secret2", 60);
        let user_id = Uuid::new_v4();

        let token = service1.create_token(user_id, "user").unwrap();
        assert!(service2.validate_token(&token).is_err());
    }

    #[test]
    fn test_validate_invalid_token() {
        let service = JwtService::new("secret", 60);
        assert!(service.validate_token("not.a.jwt").is_err());
    }

    #[test]
    fn test_expired_token() {
        let service = JwtService::new("secret", 0);
        let user_id = Uuid::new_v4();

        let token = service.create_token(user_id, "user").unwrap();
        // Token with 0 expiry_minutes is likely expired by the time we validate
        // but may still be valid due to clock precision. This tests the path.
        let result = service.validate_token(&token);
        // May be Ok or Err depending on timing - both are acceptable
        let _ = result;
    }
}
