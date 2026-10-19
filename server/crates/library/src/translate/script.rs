//! Which writing system a title is in, the languages a listener can ask for, and which titles are
//! worth translating for them.
use unicode_normalization::UnicodeNormalization;

/// The languages Settings → Language offers: (code, English name, script code of the language).
pub const LANGUAGES: &[(&str, &str, &str)] = &[
    ("en", "English", "latn"),
    ("de", "German", "latn"),
    ("fr", "French", "latn"),
    ("es", "Spanish", "latn"),
    ("it", "Italian", "latn"),
    ("pt", "Portuguese", "latn"),
    ("nl", "Dutch", "latn"),
    ("sv", "Swedish", "latn"),
    ("da", "Danish", "latn"),
    ("nb", "Norwegian", "latn"),
    ("fi", "Finnish", "latn"),
    ("pl", "Polish", "latn"),
    ("cs", "Czech", "latn"),
    ("tr", "Turkish", "latn"),
    ("id", "Indonesian", "latn"),
    ("vi", "Vietnamese", "latn"),
    ("ru", "Russian", "cyrl"),
    ("uk", "Ukrainian", "cyrl"),
    ("el", "Greek", "grek"),
    ("ar", "Arabic", "arab"),
    ("he", "Hebrew", "hebr"),
    ("hi", "Hindi", "deva"),
    ("th", "Thai", "thai"),
    ("ja", "Japanese", "ja"),
    ("ko", "Korean", "ko"),
    ("zh", "Chinese", "zh"),
];

/// The target used when a listener picked none (or an unknown one).
pub const DEFAULT_TARGET: &str = "en";

/// A known target language code ("EN-us" → "en"; unknown → "en").
pub fn target(code: &str) -> &'static str {
    let c = code.trim().to_ascii_lowercase();
    let c = c.split(['-', '_']).next().unwrap_or("");
    LANGUAGES.iter().find(|(k, _, _)| *k == c).map(|(k, _, _)| *k).unwrap_or(DEFAULT_TARGET)
}

pub fn language_name(code: &str) -> &'static str {
    LANGUAGES.iter().find(|(k, _, _)| *k == code).map(|(_, n, _)| *n).unwrap_or("English")
}

/// ISO 639-3 code MusicBrainz uses for a release's text language ("en" → "eng").
pub fn iso639_3(code: &str) -> &'static str {
    match code {
        "en" => "eng", "de" => "deu", "fr" => "fra", "es" => "spa", "it" => "ita", "pt" => "por", "nl" => "nld",
        "sv" => "swe", "da" => "dan", "nb" => "nor", "fi" => "fin", "pl" => "pol", "cs" => "ces", "tr" => "tur",
        "id" => "ind", "vi" => "vie", "ru" => "rus", "uk" => "ukr", "el" => "ell", "ar" => "ara", "he" => "heb",
        "hi" => "hin", "th" => "tha", "ja" => "jpn", "ko" => "kor", "zh" => "zho",
        _ => "",
    }
}

/// The script code a language is written in.
pub fn target_script(code: &str) -> &'static str {
    LANGUAGES.iter().find(|(k, _, _)| *k == code).map(|(_, _, s)| *s).unwrap_or("latn")
}

/// The writing system of a text, as a short code: "ja" (any kana), "ko" (hangul), "zh" (Han
/// characters without kana), "cyrl", "grek", "arab", "hebr", "thai", "deva", "latn", or "" when the
/// text has no letters at all (numbers, symbols). It doubles as the source language of the cache key:
/// the text alone decides it, so the same title always lands on the same row.
pub fn detect(text: &str) -> &'static str {
    let (mut kana, mut hangul, mut han, mut latin) = (0usize, 0usize, 0usize, 0usize);
    let mut other: [(usize, &'static str); 6] = [(0, "cyrl"), (0, "grek"), (0, "arab"), (0, "hebr"), (0, "thai"), (0, "deva")];
    for c in text.chars() {
        let u = c as u32;
        match u {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => kana += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF => han += 1,
            0x0400..=0x052F => other[0].0 += 1,
            0x0370..=0x03FF | 0x1F00..=0x1FFF => other[1].0 += 1,
            0x0600..=0x06FF | 0x0750..=0x077F => other[2].0 += 1,
            0x0590..=0x05FF => other[3].0 += 1,
            0x0E00..=0x0E7F => other[4].0 += 1,
            0x0900..=0x097F => other[5].0 += 1,
            _ if c.is_alphabetic() => latin += 1,
            _ => {}
        }
    }
    if kana > 0 {
        return "ja";
    }
    if hangul > 0 {
        return "ko";
    }
    let best = other.iter().max_by_key(|(n, _)| *n).copied().unwrap_or((0, ""));
    if han > 0 && han >= best.0 {
        return "zh";
    }
    if best.0 > 0 {
        return best.1;
    }
    if latin > 0 { "latn" } else { "" }
}

/// Whether a text is worth translating for a listener reading `target`: its script differs from the
/// one the language is written in. Han characters alone count as readable for a Japanese reader.
/// Latin titles for a Latin-script language (Spanish for an English reader) are left alone: telling
/// those languages apart needs more than the script, and most of them are names. A manual
/// correction works for any text.
pub fn candidate(text: &str, target: &str) -> bool {
    let src = detect(text);
    let ts = target_script(target);
    if src.is_empty() || src == ts {
        return false;
    }
    !(ts == "ja" && src == "zh")
}

/// The text as stored and looked up: NFC, trimmed, inner runs of white space made one space.
pub fn normalize(text: &str) -> String {
    let nfc: String = text.nfc().collect();
    nfc.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether two texts say the same thing (case, width and spacing ignored): a translation that only
/// repeats the original is not shown.
pub fn same(a: &str, b: &str) -> bool {
    let f = |s: &str| s.nfkc().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()).collect::<String>();
    f(a) == f(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts() {
        assert_eq!(detect("夜に駆ける"), "ja");
        assert_eq!(detect("ヨルシカ"), "ja");
        assert_eq!(detect("东风破"), "zh");
        assert_eq!(detect("東京"), "zh");
        assert_eq!(detect("봄날"), "ko");
        assert_eq!(detect("Кино"), "cyrl");
        assert_eq!(detect("Группа крови (Remastered)"), "cyrl");
        assert_eq!(detect("Σ' αγαπώ"), "grek");
        assert_eq!(detect("Beyoncé"), "latn");
        assert_eq!(detect("1989"), "");
        assert_eq!(detect("夜に駆ける (Remix)"), "ja");
    }

    #[test]
    fn candidates_by_script() {
        assert!(candidate("夜に駆ける", "en"));
        assert!(candidate("Кино", "de"));
        assert!(!candidate("Yesterday", "en"));
        assert!(!candidate("Despacito", "en"), "Latin for a Latin reader is left alone");
        assert!(candidate("Yesterday", "ja"));
        assert!(!candidate("東京", "ja"), "kanji are Japanese too");
        assert!(candidate("夜に駆ける", "zh"), "kana for a Chinese reader");
        assert!(!candidate("1989", "en"));
        assert!(!candidate("Кино", "ru"));
    }

    #[test]
    fn targets_and_codes() {
        assert_eq!(target("EN-us"), "en");
        assert_eq!(target("ja_JP"), "ja");
        assert_eq!(target("xx"), "en");
        assert_eq!(target(""), "en");
        assert_eq!(iso639_3("ja"), "jpn");
        assert_eq!(language_name("de"), "German");
        assert!(LANGUAGES.iter().all(|(c, _, _)| !iso639_3(c).is_empty()));
    }

    #[test]
    fn normalizing() {
        assert_eq!(normalize("  夜に  駆ける \n"), "夜に 駆ける");
        // NFC: a decomposed é becomes one character, so both spellings share a row
        assert_eq!(normalize("Beyonce\u{301}"), "Beyoncé");
        assert!(same("Racing Into The Night", "racing into the  night"));
        assert!(same("ＡＢＣ", "abc"));
        assert!(!same("Night", "Day"));
    }
}
