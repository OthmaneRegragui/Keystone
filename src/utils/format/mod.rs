/// Format a byte count into a human-readable string.
///
/// Uses SI units (1 KB = 1000 bytes) with two decimal places.
///
/// # Examples
///
/// ```
/// use keystone::utils::format::format_file_size;
///
/// assert_eq!(format_file_size(0), "0 B");
/// assert_eq!(format_file_size(500), "500 B");
/// assert_eq!(format_file_size(1_500), "1.50 KB");
/// assert_eq!(format_file_size(1_500_000), "1.50 MB");
/// assert_eq!(format_file_size(2_000_000_000), "2.00 GB");
/// ```
pub fn format_file_size(bytes: i64) -> String {
    if bytes < 0 {
        return format!("-{}", format_file_size(-bytes));
    }
    if bytes == 0 {
        return "0 B".to_string();
    }

    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let base: f64 = 1000.0;

    let i = (bytes as f64).ln() / base.ln();
    let i = (i.floor() as usize).min(units.len() - 1);

    let value = bytes as f64 / base.powi(i as i32);

    if i == 0 {
        format!("{} B", value)
    } else if value >= 100.0 {
        format!("{:.0} {}", value, units[i])
    } else {
        format!("{:.2} {}", value, units[i])
    }
}

/// Format an optional MIME type into a display string.
///
/// Returns `"unknown"` when `None`, or the MIME type string when `Some`.
pub fn format_mime_type(mime: &Option<String>) -> String {
    match mime {
        Some(m) => m.clone(),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_file_size_bytes() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(1), "1 B");
        assert_eq!(format_file_size(999), "999 B");
    }

    #[test]
    fn test_format_file_size_kilobytes() {
        assert_eq!(format_file_size(1_000), "1.00 KB");
        assert_eq!(format_file_size(1_500), "1.50 KB");
        assert_eq!(format_file_size(99_900), "99.90 KB");
    }

    #[test]
    fn test_format_file_size_megabytes() {
        assert_eq!(format_file_size(1_000_000), "1.00 MB");
        assert_eq!(format_file_size(1_500_000), "1.50 MB");
    }

    #[test]
    fn test_format_file_size_gigabytes() {
        assert_eq!(format_file_size(1_000_000_000), "1.00 GB");
        assert_eq!(format_file_size(2_500_000_000), "2.50 GB");
    }

    #[test]
    fn test_format_file_size_terabytes() {
        assert_eq!(format_file_size(1_000_000_000_000), "1.00 TB");
    }

    #[test]
    fn test_format_file_size_petabytes() {
        assert_eq!(format_file_size(1_000_000_000_000_000), "1.00 PB");
    }

    #[test]
    fn test_format_file_size_negative() {
        assert_eq!(format_file_size(-1_500_000), "-1.50 MB");
    }

    #[test]
    fn test_format_mime_type_none() {
        assert_eq!(format_mime_type(&None), "unknown");
    }

    #[test]
    fn test_format_mime_type_some() {
        assert_eq!(
            format_mime_type(&Some("application/pdf".to_string())),
            "application/pdf"
        );
    }
}
