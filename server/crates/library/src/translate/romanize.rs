//! The built-in romanization (tier 3 of translate/tiers.rs): Latin letters for a title in another script, made
//! on this server from tables, without a model or a network call. It fills the `_r` fields only.
//!
//! * Japanese kana: modified Hepburn (romaji.rs); titles with kanji are left as written.
//! * Korean: Revised Romanization from each syllable's letters (hangul.rs).
//! * Cyrillic: a simplified GOST / passport table (ж zh, х kh, ц ts, щ shch, ю yu, я ya), with the Ukrainian
//!   letters (г h, и y, і i, ї yi, є ye) when the title has any.
//! * Greek: simplified ELOT 743 (θ th, χ ch, ψ ps, ου ou, αυ av / af, ευ ev / ef, μπ mp, γγ ng).
//! * Hebrew, Arabic, Thai and Devanagari: deunicode's ASCII spellings, letter by letter (Devanagari's long
//!   vowels written short, च ch: चन्ना मेरेया Channa Mereya). Hebrew and Arabic write few vowels, so these
//!   spellings are rough ("Shlvm" for שלום): they only fill a title that has no romanization yet, and the
//!   local model's replaces them ([`letters_only`], store::Source::Letters).
//! * Chinese: none (a reading needs a dictionary, and the server has none) – left to metadata, DeepL or the
//!   model.
use super::script;
use unicode_normalization::UnicodeNormalization;

/// The scripts (script::detect codes) this tier romanizes.
pub const SCRIPTS: [&str; 8] = ["ja", "ko", "cyrl", "grek", "hebr", "arab", "thai", "deva"];

/// Whether texts in this script get a romanization here.
pub fn supported(script: &str) -> bool {
    SCRIPTS.contains(&script)
}

/// Whether this script's romanization is spelled letter by letter (deunicode): a stand-in that fills a gap,
/// never replacing another source's romanization.
pub fn letters_only(script: &str) -> bool {
    matches!(script, "hebr" | "arab" | "thai" | "deva")
}

/// The romanization of a title, or None when this tier has none for it (Latin text, Chinese, Japanese with
/// kanji, or nothing changed).
pub fn romanize(text: &str) -> Option<String> {
    let text = script::normalize(text);
    let out = match script::detect(&text) {
        "ja" => super::romaji::romanize(&text),
        "ko" => super::hangul::romanize(&text),
        "cyrl" => Some(cyrillic(&text)),
        "grek" => Some(greek(&text)),
        "hebr" | "arab" | "thai" | "deva" => Some(by_deunicode(&text)),
        _ => None,
    }?;
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (!out.is_empty() && !script::same(&out, &text) && out.chars().any(|c| c.is_ascii_alphabetic())).then_some(out)
}

/// The case of a Latin spelling after the letter it stands for: all capitals inside a word in capitals (КИНО →
/// KINO), else a capital first letter (Щука → Shchuka).
fn cased(latin: &str, upper: bool, word_in_caps: bool) -> String {
    if !upper {
        return latin.to_string();
    }
    if word_in_caps {
        return latin.to_uppercase();
    }
    let mut c = latin.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Whether the letter at `i` sits in a word of two or more capitals.
fn in_caps_word(chars: &[char], i: usize) -> bool {
    let up = |j: usize| chars.get(j).is_some_and(|c| c.is_uppercase());
    chars[i].is_uppercase() && (up(i + 1) || (i > 0 && up(i - 1)))
}

/// Cyrillic → Latin: a simplified GOST / passport table; Ukrainian spellings when the text has a letter only
/// Ukrainian uses.
pub fn cyrillic(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let ukrainian = chars.iter().any(|c| matches!(c, 'і' | 'ї' | 'є' | 'ґ' | 'І' | 'Ї' | 'Є' | 'Ґ'));
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let lower = c.to_lowercase().next().unwrap_or(c);
        let latin: Option<&str> = match lower {
            'а' => Some("a"), 'б' => Some("b"), 'в' => Some("v"),
            'г' => Some(if ukrainian { "h" } else { "g" }), 'ґ' => Some("g"),
            'д' => Some("d"), 'е' => Some("e"), 'ё' => Some("yo"), 'є' => Some("ye"), 'ж' => Some("zh"), 'з' => Some("z"),
            'и' => Some(if ukrainian { "y" } else { "i" }), 'і' => Some("i"), 'ї' => Some("yi"), 'й' => Some("y"),
            'к' => Some("k"), 'л' => Some("l"), 'м' => Some("m"), 'н' => Some("n"), 'о' => Some("o"), 'п' => Some("p"),
            'р' => Some("r"), 'с' => Some("s"), 'т' => Some("t"), 'у' => Some("u"), 'ў' => Some("u"), 'ф' => Some("f"),
            'х' => Some("kh"), 'ц' => Some("ts"), 'ч' => Some("ch"), 'ш' => Some("sh"), 'щ' => Some("shch"),
            'ъ' | 'ь' => Some(""), 'ы' => Some("y"), 'э' => Some("e"), 'ю' => Some("yu"), 'я' => Some("ya"),
            // Serbian and Macedonian letters
            'ђ' => Some("dj"), 'ј' => Some("j"), 'љ' => Some("lj"), 'њ' => Some("nj"), 'ћ' => Some("c"), 'џ' => Some("dz"),
            'ѓ' => Some("gj"), 'ќ' => Some("kj"), 'ѕ' => Some("dz"),
            _ => None,
        };
        match latin {
            Some(l) => out.push_str(&cased(l, c.is_uppercase(), in_caps_word(&chars, i))),
            None if (0x0400..=0x052F).contains(&(c as u32)) => out.push_str(deunicode::deunicode_char(c).unwrap_or("")),
            None => out.push(c),
        }
    }
    out
}

/// One Greek letter without its accents, and whether it had a diaeresis (ϊ ϋ: no diphthong with the letter
/// before it).
fn greek_base(c: char) -> (char, bool) {
    let mut base = c;
    let mut diaeresis = false;
    for (n, d) in c.nfd().enumerate() {
        if n == 0 {
            base = d;
        } else if d == '\u{308}' {
            diaeresis = true;
        }
    }
    (base, diaeresis)
}

/// Greek → Latin: simplified ELOT 743.
pub fn greek(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let letters: Vec<(char, bool, bool)> = chars.iter().map(|&c| {
        let (b, d) = greek_base(c);
        (b.to_lowercase().next().unwrap_or(b), d, c.is_uppercase())
    }).collect();
    let at = |j: usize| letters.get(j).map(|l| l.0);
    let vowel = |c: Option<char>| c.is_some_and(|c| "αεηιουω".contains(c));
    // αυ, ευ, ηυ: v before a vowel or a voiced consonant, f before the rest and at the end
    let voiced = |c: Option<char>| vowel(c) || c.is_some_and(|c| "βγδζλμνρ".contains(c));
    let mut out = String::new();
    let mut i = 0;
    while i < letters.len() {
        let (c, _, upper) = letters[i];
        let next = at(i + 1);
        let next_plain = letters.get(i + 1).is_some_and(|l| !l.1);
        let (latin, used): (Option<String>, usize) = match c {
            'ο' if next == Some('υ') && next_plain => (Some("ou".into()), 2),
            'α' | 'ε' | 'η' if next == Some('υ') && next_plain => {
                let v = if voiced(at(i + 2)) { "v" } else { "f" };
                (Some(format!("{}{v}", match c { 'α' => "a", 'ε' => "e", _ => "i" })), 2)
            }
            'γ' if next == Some('γ') => (Some("ng".into()), 2),
            'γ' if next == Some('ξ') => (Some("nx".into()), 2),
            'γ' if next == Some('χ') => (Some("nch".into()), 2),
            _ => (greek_letter(c).map(String::from), 1),
        };
        match latin {
            Some(l) => out.push_str(&cased(&l, upper, in_caps_word(&chars, i))),
            None => out.push(chars[i]),
        }
        i += used;
    }
    out
}

/// A Greek letter in Latin letters (modern Greek sounds); None for anything else.
fn greek_letter(c: char) -> Option<&'static str> {
    Some(match c {
        'α' => "a", 'β' => "v", 'γ' => "g", 'δ' => "d", 'ε' => "e", 'ζ' => "z", 'η' => "i", 'θ' => "th",
        'ι' => "i", 'κ' => "k", 'λ' => "l", 'μ' => "m", 'ν' => "n", 'ξ' => "x", 'ο' => "o", 'π' => "p",
        'ρ' => "r", 'σ' | 'ς' => "s", 'τ' => "t", 'υ' => "y", 'φ' => "f", 'χ' => "ch", 'ψ' => "ps", 'ω' => "o",
        _ => return None,
    })
}

/// A Devanagari consonant (its inherent vowel a is written unless a vowel sign or the virama follows).
fn devanagari_consonant(c: char) -> bool {
    matches!(c as u32, 0x0915..=0x0939 | 0x0958..=0x095F)
}

/// A Devanagari letter's spelling where deunicode's is not the usual one: the long vowels short, as song
/// titles write them (आ ा aa → a, ई ी ii → i, ऊ ू uu → u), च ch and छ chh.
fn devanagari(c: char, s: &str) -> Option<&'static str> {
    Some(match (c, s) {
        ('च', _) => "ch",
        ('छ', _) => "chh",
        (_, "aa") => "a",
        (_, "ii") => "i",
        (_, "uu") => "u",
        _ => return None,
    })
}

/// deunicode's ASCII spelling of each letter of the script (other characters stay), with a capital first
/// letter per word and small letters after it: these scripts have no capitals of their own (deunicode's
/// capitals tell apart letters such as Arabic ح H and ه h). Devanagari consonants get their inherent a before
/// another consonant (नमस्ते namaste; none at the end of a word, as Hindi says it); the signs deunicode writes
/// as ` (Thai อ, Arabic ع) are left out, and so is the ' of a hamza at the start of a word (أنت Nt).
fn by_deunicode(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut word_start = true;
    for (i, &c) in chars.iter().enumerate() {
        let in_script = matches!(c as u32, 0x0590..=0x05FF | 0x0600..=0x06FF | 0x0750..=0x077F | 0x0E00..=0x0E7F | 0x0900..=0x097F);
        if in_script {
            let mut s: String = deunicode::deunicode_char(c).unwrap_or("").trim().chars().filter(|x| *x != '`').collect();
            if let Some(d) = devanagari(c, &s) {
                s = d.to_string();
            }
            if devanagari_consonant(c) && chars.get(i + 1).is_some_and(|n| devanagari_consonant(*n)) {
                s.push('a');
            }
            let starts = word_start && out.chars().next_back().is_none_or(|p| !p.is_alphanumeric());
            if starts {
                s = s.trim_start_matches('\'').to_string();
            }
            if s.is_empty() {
                continue;
            }
            let s = s.to_lowercase();
            if starts {
                let mut it = s.chars();
                if let Some(f) = it.next() {
                    out.extend(f.to_uppercase());
                    out.push_str(it.as_str());
                }
            } else {
                out.push_str(&s);
            }
            word_start = false;
        } else {
            out.push(c);
            word_start = !c.is_alphanumeric();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_script() {
        let r = |s: &str| romanize(s);
        assert_eq!(r("ハルジオン").as_deref(), Some("Harujion"));
        assert_eq!(r("봄날").as_deref(), Some("Bomnal"));
        assert_eq!(r("Кино").as_deref(), Some("Kino"));
        assert_eq!(r("Группа крови").as_deref(), Some("Gruppa krovi"));
        assert_eq!(r("КИНО").as_deref(), Some("KINO"));
        assert_eq!(r("Щука и ёж").as_deref(), Some("Shchuka i yozh"));
        assert_eq!(r("Хочешь?").as_deref(), Some("Khochesh?"));
        assert_eq!(r("Обійми").as_deref(), Some("Obiymy"), "Ukrainian и is y");
        assert_eq!(r("Червона рута і").as_deref(), Some("Chervona ruta i"));
        assert_eq!(r("Σ' αγαπώ").as_deref(), Some("S' agapo"));
        assert_eq!(r("Ευτυχία").as_deref(), Some("Eftychia"));
        assert_eq!(r("Αύριο").as_deref(), Some("Avrio"));
        assert_eq!(r("Ούλα").as_deref(), Some("Oula"));
        assert_eq!(r("Άγγελος").as_deref(), Some("Angelos"));
        assert_eq!(r("Μπουζούκι").as_deref(), Some("Mpouzouki"));
        assert!(r("שלום").is_some_and(|s| s.is_ascii() && s.starts_with('S')));
        assert!(r("حبيبي").is_some_and(|s| s.is_ascii()));
        assert!(r("สวัสดี").is_some_and(|s| s.is_ascii()));
        assert_eq!(r("नमस्ते").as_deref(), Some("Namaste"));
        assert_eq!(r("तुम ही हो").as_deref(), Some("Tum Hi Ho"), "long vowels written short");
        assert_eq!(r("चन्ना मेरेया").as_deref(), Some("Channa Mereya"));
        assert_eq!(r("ขอบคุณ").as_deref(), Some("Khbkhun"), "no backtick for อ");
        // no capitals inside a word, no apostrophe before one
        assert_eq!(r("بحبك يا لبنان").as_deref(), Some("Bhbk Y Lbnn"));
        assert_eq!(r("أنت عمري").as_deref(), Some("Nt Mry"));
        // these only fill a gap
        assert!(["hebr", "arab", "thai", "deva"].iter().all(|s| letters_only(s)) && !letters_only("ja") && !letters_only("cyrl"));
    }

    #[test]
    fn nothing_for_latin_chinese_and_kanji() {
        assert_eq!(romanize("Yesterday"), None);
        assert_eq!(romanize("东风破"), None);
        assert_eq!(romanize("夜に駆ける"), None);
        assert_eq!(romanize("1989"), None);
        assert!(!supported("zh") && !supported("latn") && supported("ja") && supported("cyrl"));
    }
}
