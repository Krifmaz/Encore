//! Files: when a file last changed, and names that are safe as file names on every system.
use std::path::Path;
use std::time::SystemTime;

/// When the file at `path` last changed (None when it can't be read).
pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// `s` with the characters a file system refuses (`\ / : * ? " < > |` and control characters) replaced by
/// `_`, without spaces at either end. Callers that keep names short or drop leading dots do so on top.
pub fn safe_file_name(s: &str) -> String {
    s.chars().map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn safe_names() {
        assert_eq!(super::safe_file_name(" AC/DC: Back in Black?.flac "), "AC_DC_ Back in Black_.flac");
        assert_eq!(super::safe_file_name("a\tb\u{7}c"), "a_b_c");
        assert_eq!(super::safe_file_name("...dots stay..."), "...dots stay...");
        assert!(super::mtime(std::path::Path::new("/definitely/not/here")).is_none());
    }
}
