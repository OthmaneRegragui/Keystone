//! Cross-platform-safe name validation for folders and files.
//!
//! A single strict rule set that works on Windows, Linux and macOS so any name
//! created through Keystone behaves identically when browsed, opened through the
//! `?dir=` URL, downloaded, renamed, or exported to ZIP — and can never break
//! out of a path or poison an HTTP header.

/// Validate a username for account registration.
///
/// Complements the DTO length limits (3..=50 chars) with a strict charset so
/// usernames cannot smuggle control characters into logs or rendered pages.
/// Returns `Err(reason)` with a human-readable reason when rejected.
pub fn validate_username(username: &str) -> Result<(), String> {
    if username.is_empty() {
        return Err("username cannot be empty".to_string());
    }

    // Control characters enable log injection and terminal escape tricks.
    if username.chars().any(|c| c.is_control()) {
        return Err("username contains control characters which are not allowed".to_string());
    }

    // Whitespace (incl. unicode) breaks URLs, headers and login flows.
    if username.chars().any(char::is_whitespace) {
        return Err("username cannot contain whitespace".to_string());
    }

    // Restrict to a portable safe charset (unicode letters/digits included).
    if !username
        .chars()
        .all(|c| c.is_alphanumeric() || "_-.".contains(c))
    {
        return Err(
            "username may only contain letters, digits, and '_', '-', '.'".to_string(),
        );
    }

    Ok(())
}

/// Validate a folder or file name (a single path component, already trimmed).
///
/// Returns `Err(reason)` with a human-readable reason when the name would cause
/// problems on any of Windows / Linux / macOS.
pub fn validate_component_name(name: &str) -> Result<(), String> {
    // 1. Non-empty.
    if name.is_empty() {
        return Err("name cannot be empty".to_string());
    }

    // 2. "." and ".." are reserved (path traversal).
    if name == "." || name == ".." {
        return Err("'.' and '..' are reserved and cannot be used as a name".to_string());
    }

    // 3. Control characters are invalid on every OS and break URLs/headers.
    if name.chars().any(|c| c.is_control()) {
        return Err(
            "name contains control characters (newline, tab, etc.) which are not allowed"
                .to_string(),
        );
    }

    // 4. Characters Windows forbids in file/folder names (also path separators).
    const FORBIDDEN: &str = r#"\/:*?"<>|"#;
    if let Some(c) = name.chars().find(|c| FORBIDDEN.contains(*c)) {
        return Err(format!(
            "name contains the forbidden character '{c}'. These characters are not \
             allowed anywhere: \\ / : * ? \" < > |"
        ));
    }

    // 5. Leading/trailing spaces and trailing dots: Windows and macOS strip or
    //    normalise them, so the same name would silently differ between OSes.
    if name.starts_with(' ') {
        return Err("name cannot start with a space".to_string());
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Err("name cannot end with a space or a dot".to_string());
    }

    // 6. Windows reserved device names (with or without an extension).
    let stem = name.split('.').next().unwrap_or("");
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.to_uppercase().as_str()) {
        return Err(format!(
            "'{stem}' is a reserved device name on Windows and cannot be used"
        ));
    }

    // 7. Length: 255 bytes is the safe common limit (NTFS/FAT/exFAT and most
    //    filesystems, including on Linux and macOS).
    if name.len() > 255 {
        return Err("name is too long (maximum 255 bytes)".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_names() {
        for n in [
            "file.txt",
            "My Folder",
            "photo-2024.jpg",
            "Tést 文档.pdf",
            ".env",
            ".gitignore",
            "a",
        ] {
            assert!(validate_component_name(n).is_ok(), "should accept {n:?}");
        }
    }

    #[test]
    fn rejects_windows_forbidden_characters() {
        for n in ["a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b"] {
            assert!(validate_component_name(n).is_err(), "should reject {n:?}");
        }
    }

    #[test]
    fn rejects_reserved_device_names() {
        for n in ["CON", "con.txt", "PRN", "NUL", "aux.pdf", "COM1", "LPT9", "con.log"] {
            assert!(validate_component_name(n).is_err(), "should reject {n:?}");
        }
    }

    #[test]
    fn rejects_dots_spaces_and_traversal() {
        for n in [".", "..", "trailing.", "leading ", "trailing ", " middle"] {
            assert!(validate_component_name(n).is_err(), "should reject {n:?}");
        }
    }

    #[test]
    fn rejects_control_characters() {
        for n in ["a\nb", "a\tb", "a\r\nb", "a\u{1f}b"] {
            assert!(validate_component_name(n).is_err(), "should reject {n:?}");
        }
    }

    #[test]
    fn rejects_overlong_names() {
        let max_ok = "x".repeat(255);
        assert!(validate_component_name(&max_ok).is_ok());
        let too_long = "x".repeat(256);
        assert!(validate_component_name(&too_long).is_err());
    }
}
