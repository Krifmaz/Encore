//! Text: escaping into HTML, the folded forms names and titles are compared by, ISO 8601 durations, and
//! the end of a program's output.
//! Each folding below lower-cases first; they differ in what they keep, and every caller keeps the one it
//! always used.
use regex::Regex;
use std::sync::LazyLock;

/// Text into HTML: the five characters that matter (safe in element text and in quoted attributes).
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"), '<' => out.push_str("&lt;"), '>' => out.push_str("&gt;"), '"' => out.push_str("&quot;"), '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Letters and digits only, lower case: "AC/DC" and "ac dc" both become "acdc" (the key the web and phone
/// apps compare names by too).
pub fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// [`norm`] that keeps underscores as well: the older key of the catalogue titles and of For you's caches.
pub fn norm_keep_underscore(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '_').collect()
}

/// Lower-case words: every run of characters that are not letters or digits becomes one space, with none at
/// either end ("Hello,  World!" → "hello world").
pub fn words(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// [`words`] of the plain ASCII letters and digits only: every other character separates words ("Beyoncé"
/// → "beyonc"). The matching of downloads (Soulseek, torrents, tags, picker) has always worked this way.
pub fn ascii_words(s: &str) -> String {
    s.to_lowercase().split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit())).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// An ISO 8601 duration of hours, minutes and seconds as the catalogues write it ("PT3M12S", "PT1H2M5.5S") in
/// seconds; None when the text does not start with "P".
pub fn iso_duration(v: &str) -> Option<f64> {
    static ISO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^P(?:T)?(?:(\d+)H)?(?:(\d+)M)?(?:([\d.]+)S)?").unwrap());
    let c = ISO.captures(v)?;
    let g = |i| c.get(i).and_then(|m: regex::Match| m.as_str().parse::<f64>().ok()).unwrap_or(0.0);
    Some(g(1) * 3600.0 + g(2) * 60.0 + g(3))
}

/// The last `n` lines of `text` that are not blank (a program's error output, shortened for a message).
pub fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_the_five() {
        assert_eq!(super::html_escape(r#"<a href="x">Tom & Jerry's</a>"#), "&lt;a href=&quot;x&quot;&gt;Tom &amp; Jerry&#39;s&lt;/a&gt;");
        assert_eq!(super::html_escape("plain"), "plain");
    }

    #[test]
    fn the_foldings() {
        use super::*;
        assert_eq!(norm("AC/DC – Back_in Black!"), "acdcbackinblack");
        assert_eq!(norm_keep_underscore("AC/DC – Back_in Black!"), "acdcback_inblack");
        assert_eq!(words("  Hello,  World! Beyoncé "), "hello world beyoncé");
        assert_eq!(ascii_words("  Hello,  World! Beyoncé "), "hello world beyonc");
        assert_eq!(ascii_words("--"), "");
        // the regex form the download matching used before ([^a-z0-9]+ → one space, then the words)
        let re = regex::Regex::new(r"[^a-z0-9]+").unwrap();
        for s in ["Guns N' Roses", "Sigur Rós – Ágætis byrjun", "a__b..c", "", "ÆØÅ 123", "İstanbul"] {
            let old = re.replace_all(&s.to_lowercase(), " ").split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(ascii_words(s), old, "{s}");
        }
    }

    #[test]
    fn iso_durations() {
        use super::iso_duration;
        assert_eq!(iso_duration("PT3M12S"), Some(192.0));
        assert_eq!(iso_duration("PT1H2M5.5S"), Some(3725.5));
        assert_eq!(iso_duration("PT45S"), Some(45.0));
        assert_eq!(iso_duration("3:12"), None);
    }

    #[test]
    fn the_last_lines() {
        assert_eq!(super::last_lines("a\n\nb\n  \nc\n", 2), "b\nc");
        assert_eq!(super::last_lines("only", 5), "only");
        assert_eq!(super::last_lines("", 3), "");
    }
}
