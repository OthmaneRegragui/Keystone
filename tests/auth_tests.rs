mod helpers;

use keystone::utils::auth::password::{hash_password, verify_password};
use keystone::utils::auth::jwt::JwtService;
use keystone::utils::auth::api_keys::{generate_api_key, validate_api_key};

#[test]
fn test_password_hash_and_verify() {
    let password = "secure_password_123!";
    let hash = hash_password(password).expect("Failed to hash password");

    assert!(verify_password(password, &hash).expect("Failed to verify password"));
    assert!(!verify_password("wrong_password", &hash).expect("Failed to verify password"));
}

#[test]
fn test_jwt_create_and_validate() {
    let secret = "test-secret-key-for-jwt";
    let jwt_service = JwtService::new(secret, 60);

    let user_id = uuid::Uuid::new_v4();
    let token = jwt_service
        .create_token(user_id, "user")
        .expect("Failed to create token");

    let claims = jwt_service
        .validate_token(&token)
        .expect("Failed to validate token");

    assert_eq!(claims.sub, user_id.to_string());
    assert_eq!(claims.role, "user");
}

#[test]
fn test_jwt_invalid_token() {
    let secret = "test-secret-key-for-jwt";
    let jwt_service = JwtService::new(secret, 60);

    let result = jwt_service.validate_token("invalid.token.here");
    assert!(result.is_err());
}

#[test]
fn test_api_key_generation() {
    let (full_key, prefix, hash) = generate_api_key();

    assert!(full_key.starts_with("ks_live_"));
    assert!(prefix.starts_with("ks_live_"));
    assert!(prefix.len() < full_key.len());
    assert!(validate_api_key(&full_key, &hash));
    assert!(!validate_api_key("wrong_key", &hash));
}
