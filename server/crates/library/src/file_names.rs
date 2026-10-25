//! Which characters the library's file and folder names may hold (the `file_names` setting).
//!
//! Song titles can be emoji or symbols only – Coldplay's "Music of the Spheres" has "♾️", "❤️", "✨" and "🌎"
//! – and not every disk takes them: exFAT, FAT sticks and some NAS shares refuse such a name, and a pooled
//! library (mergerfs) can put the file on such a disk. The setting decides how names are written:
//!
//! * `keep` (the default): every character a name may hold anywhere ([`crate::tagger::clean`]); emoji stay.
//! * `words`: symbols and emoji become their names in words ("♾️" → "Infinity", "❤️" → "Red Heart"), so every
//!   emoji-only title still gets its own name that stays the same on every import; letters of every script
//!   ("Sigur Rós", "ハロー") stay as they are.
//! * `ascii`: every character spelled in plain ASCII ("Sigur Ros", "haro", "Infinity").
//!
//! Only the file names change: the tags and the library keep the real titles. And whatever the setting, a
//! new file whose name its disk refuses is written once more with the `words` name ([`fallback`]).

/// How names are written (the `file_names` setting's values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// names as the tags say (emoji and symbols included)
    Keep,
    /// symbols and emoji in words
    Words,
    /// plain ASCII only
    Ascii,
}

impl Mode {
    /// The mode a setting value names ("keep" for anything unknown).
    pub fn parse(s: &str) -> Mode {
        match s.trim() {
            "words" => Mode::Words,
            "ascii" => Mode::Ascii,
            _ => Mode::Keep,
        }
    }
}

/// Is `c` a symbol or emoji rather than a letter, number or punctuation mark: arrows, maths and technical
/// signs, shapes, the dingbats and miscellaneous symbols (U+2100–U+2BFF, which holds ♾ U+267E, ❤ U+2764 and
/// ✨ U+2728), the emoji planes (U+1F000–U+1FAFF), ©, ® and the few emoji in the CJK blocks (〰 〽 ㊗ ㊙).
pub fn is_symbol(c: char) -> bool {
    matches!(c as u32, 0x2100..=0x2BFF | 0x1F000..=0x1FAFF | 0xA9 | 0xAE | 0x3030 | 0x303D | 0x3297 | 0x3299)
}

/// Is `c` a character that only shapes the one before it in an emoji (a variation selector, the zero-width
/// joiner, a keycap or a tag character): it has no name of its own and goes with its emoji.
fn is_emoji_modifier(c: char) -> bool {
    matches!(c as u32, 0xFE00..=0xFE0F | 0x200D | 0x20E3 | 0xE0020..=0xE007F)
}

/// The name of a symbol in words, each word capitalised ("♾" → "Infinity", "🌎" → "Earth Americas"); a symbol
/// with no known name is written as its code point ("U+1FAE8"), so it still has a name of its own.
fn symbol_words(c: char) -> String {
    let raw = deunicode::deunicode_char(c).unwrap_or("").trim();
    // deunicode writes some symbols as punctuation ("(c)", "->"): those are not words
    if raw.is_empty() || !raw.chars().any(|x| x.is_ascii_alphanumeric()) {
        return format!("U+{:04X}", c as u32);
    }
    raw.split_whitespace().map(|w| {
        let mut cs = w.chars();
        cs.next().map(|f| f.to_ascii_uppercase().to_string() + cs.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

/// `name` as the mode writes it. `Keep` leaves it alone; `Words` puts each symbol's name in words, set apart
/// by spaces from what is around it ("Love❤️You" → "Love Red Heart You"); `Ascii` does that and spells every
/// other character in ASCII as well. No space is doubled next to a symbol, and none is left at either end.
pub fn apply(name: &str, mode: Mode) -> String {
    if mode == Mode::Keep {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len());
    // a symbol's words were just written: a space goes before what follows, unless that is a space or a
    // closing mark ("Infinity.opus", "(Infinity)")
    let mut after_symbol = false;
    for c in name.chars() {
        if is_emoji_modifier(c) {
            continue;
        }
        if is_symbol(c) {
            if !out.is_empty() && !out.ends_with([' ', '(', '[']) {
                out.push(' ');
            }
            out.push_str(&symbol_words(c));
            after_symbol = true;
            continue;
        }
        // a letter of another script in `ascii`: its ASCII spelling (nothing when it has none)
        let piece = if mode == Mode::Ascii && !c.is_ascii() { deunicode::deunicode_char(c).unwrap_or("").to_string() } else { c.to_string() };
        if after_symbol && !piece.is_empty() {
            after_symbol = false;
            if !piece.starts_with([' ', '.', ',', ')', ']']) {
                out.push(' ');
            }
        }
        if piece == " " && out.ends_with(' ') {
            continue;
        }
        out.push_str(&piece);
    }
    out.trim().to_string()
}

/// The name to try once more when the disk refused a new file's name: its file name in [`Mode::Words`] (or
/// in ASCII when that is the same), in the same folder. None when no other name can be made.
pub fn fallback(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let alt = [Mode::Words, Mode::Ascii].into_iter().map(|m| apply(&name, m)).find(|a| !a.is_empty() && *a != name && !a.starts_with('.'))?;
    Some(path.with_file_name(alt))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every emoji title of "Music of the Spheres" gets a name of its own, the same each time.
    #[test]
    fn emoji_titles_get_distinct_words() {
        let titles = ["🪐", "✨", "❤️", "🌎", "♾️"];
        let names: Vec<String> = titles.iter().map(|t| apply(t, Mode::Words)).collect();
        assert_eq!(names, ["Ringed Planet", "Sparkles", "Red Heart", "Earth Americas", "Infinity"]);
        assert_eq!(apply("Coldplay - ♾.opus", Mode::Words), "Coldplay - Infinity.opus");
        assert_eq!(apply("11 - ♾️.flac", Mode::Words), "11 - Infinity.flac");
    }

    /// Letters of every script stay in `words`; `ascii` spells them; `keep` changes nothing.
    #[test]
    fn modes() {
        assert_eq!(apply("Sigur Rós – ハロー", Mode::Words), "Sigur Rós – ハロー");
        assert_eq!(apply("Sigur Rós", Mode::Ascii), "Sigur Ros");
        assert_eq!(apply("Love❤️You", Mode::Words), "Love Red Heart You");
        assert_eq!(apply("♾ (Live)", Mode::Words), "Infinity (Live)");
        assert_eq!(apply("Coldplay - ♾.opus", Mode::Keep), "Coldplay - ♾.opus");
        assert_eq!(Mode::parse("words"), Mode::Words);
        assert_eq!(Mode::parse("ascii"), Mode::Ascii);
        assert_eq!(Mode::parse("anything"), Mode::Keep);
    }

    /// A refused name is tried once more in words; a name with nothing to change has no fallback.
    #[test]
    fn fallback_names() {
        let p = std::path::Path::new("/music/Coldplay/Singles/Coldplay - ♾.opus");
        assert_eq!(fallback(p).unwrap(), std::path::Path::new("/music/Coldplay/Singles/Coldplay - Infinity.opus"));
        assert_eq!(fallback(std::path::Path::new("/music/A/Song.flac")), None);
        assert_eq!(fallback(std::path::Path::new("/music/A/Sigur Rós.flac")).unwrap(), std::path::Path::new("/music/A/Sigur Ros.flac"));
    }
}
