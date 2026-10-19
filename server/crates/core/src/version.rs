//! Version numbers of programs and releases, compared by their numeric dot parts.

/// The numeric dot parts of a version ("v2.4.10" → [2, 4, 10], "rip, version 2.4.9" → [2, 4, 9]).
pub fn parts(v: &str) -> Vec<u64> {
    let Some(s) = extract_version(v) else { return vec![] };
    s.split('.').map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0)).collect()
}

/// The first dotted version number in a tool's output ("rip, version 2.4.9" → "2.4.9", "v2.5.0" → "2.5.0").
pub fn extract_version(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"\d+(?:\.\d+)+").ok()?;
    re.find(text).map(|m| m.as_str().to_string()).or_else(|| {
        let t = text.trim().trim_start_matches(['v', 'V']);
        (!t.is_empty() && t.chars().all(|c| c.is_ascii_digit())).then(|| t.to_string())
    })
}

/// Compare two versions by their numeric dot parts (missing parts count as 0).
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    let (x, y) = (parts(a), parts(b));
    for i in 0..x.len().max(y.len()) {
        let o = x.get(i).unwrap_or(&0).cmp(y.get(i).unwrap_or(&0));
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    std::cmp::Ordering::Equal
}

/// `latest` is a version and a higher one than `installed`.
pub fn newer(latest: &str, installed: &str) -> bool {
    !parts(latest).is_empty() && compare(latest, installed) == std::cmp::Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(compare("2026.10.09", "2026.9.30"), Greater);
        assert_eq!(compare("2.4.9", "2.4.10"), Less);
        assert_eq!(compare("v2.5.0", "2.4.10"), Greater);
        assert_eq!(compare("v2.5.0", "2.5"), Equal);
        assert!(newer("2026.10.14", "2026.10.09"));
        assert!(!newer("2.4.9", "2.4.9"));
        assert!(!newer("", "2.4.9"));
        assert_eq!(extract_version("rip, version 2.4.9\n").as_deref(), Some("2.4.9"));
        assert_eq!(extract_version("v2.5.0").as_deref(), Some("2.5.0"));
        assert_eq!(extract_version("Python 3.14.0").as_deref(), Some("3.14.0"));
    }
}
