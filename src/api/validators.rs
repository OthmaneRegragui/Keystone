use std::collections::HashSet;

// Scopes an *admin* may mint on an API key: full access, including
// administrative and user-management authority.
const ALLOWED_SCOPES: &[&str] = &[
    "files:read",
    "files:write",
    "files:delete",
    "users:read",
    "users:write",
    "admin",
];

// Scopes a *regular user* may mint on their own (self-service) API key. This
// is deliberately restricted to file operations: letting a non-admin mint an
// `admin`/`users:*` key would be a latent privilege-escalation trap if any
// future code path consults key scopes for authorization.
const ALLOWED_USER_SCOPES: &[&str] = &["files:read", "files:write", "files:delete"];

/// Validate scopes an admin is allowed to mint.
pub fn validate_scopes(scopes: &[String]) -> bool {
    if scopes.is_empty() {
        return false;
    }
    let allowed: HashSet<&str> = ALLOWED_SCOPES.iter().copied().collect();
    scopes.iter().all(|s| allowed.contains(s.as_str()))
}

/// Validate scopes a regular user (non-admin) is allowed to self-mint.
pub fn validate_user_scopes(scopes: &[String]) -> bool {
    if scopes.is_empty() {
        return false;
    }
    let allowed: HashSet<&str> = ALLOWED_USER_SCOPES.iter().copied().collect();
    scopes.iter().all(|s| allowed.contains(s.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_scopes() {
        assert!(validate_scopes(&[
            "files:read".to_string(),
            "files:write".to_string()
        ]));
        assert!(validate_scopes(&["admin".to_string()]));
    }

    #[test]
    fn test_invalid_scopes() {
        assert!(!validate_scopes(&["invalid:scope".to_string()]));
        assert!(!validate_scopes(&[]));
    }
}
