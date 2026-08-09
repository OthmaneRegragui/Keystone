use std::collections::HashSet;

const ALLOWED_SCOPES: &[&str] = &[
    "files:read",
    "files:write",
    "files:delete",
    "users:read",
    "users:write",
    "admin",
];

pub fn validate_scopes(scopes: &[String]) -> bool {
    if scopes.is_empty() {
        return false;
    }
    let allowed: HashSet<&str> = ALLOWED_SCOPES.iter().copied().collect();
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
