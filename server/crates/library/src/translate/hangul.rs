//! Korean Hangul → Revised Romanization, worked out from each syllable's letters (initial, vowel, final): the
//! built-in romanization of Korean titles (translate/romanize.rs), no model needed.
//!
//! The spelling follows the letters, with the rules that need no knowledge of the words:
//! * a final consonant is k / t / p / l / n / m / ng before another consonant and at the end (꽃 kkot, 밥 bap),
//! * before a syllable that starts with ㅇ (no sound) it is read as that syllable's first letter (한국어
//!   hangugeo, 좋아 joa, 앉아 anja); of a double final only the second letter moves, the first is read as a
//!   final (없어 eopseo, 넋이 neoksi),
//! * a final ㅎ before ㄱ, ㄷ or ㅈ makes them k, t, ch and is not written itself (어떻게 eotteoke, 좋다 jota;
//!   ㄶ and ㅀ keep their n and l: 많다 manta, 싫다 silta),
//! * ㄹㄹ is ll (물론 mullon); ㄹ is r at the start of a syllable and l at its end.
//!
//! The sound changes between syllables (assimilation: 신라 Silla, 독립 dongnip, 같이 gachi) are not applied, as
//! they need the pronunciation of each word: the spelling stays the letters' (Sinra, dokrip, gati). Official
//! spellings come from metadata (MusicBrainz ko-Latn aliases) when there are any.
//!
//! Each word made from Hangul starts with a capital letter; other characters stay as they are.

/// The initial consonants in Unicode order, romanized (ㅇ silent).
const INITIALS: [&str; 19] = ["g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p", "h"];
/// The vowels in Unicode order, romanized.
const VOWELS: [&str; 21] = ["a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we", "wi", "yu", "eu", "ui", "i"];
/// The final consonant before another consonant or at the end (index 0: none).
const FINALS: [&str; 28] = ["", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m", "p", "p", "t", "t", "ng", "t", "t", "k", "t", "p", "t"];
/// The final consonant before a syllable that starts with ㅇ: read as its first letter (ㅎ is silent there).
const FINALS_LINKED: [&str; 28] = ["", "g", "kk", "ks", "n", "nj", "n", "d", "r", "lg", "lm", "lb", "ls", "lt", "lp", "r", "m", "b", "ps", "s", "ss", "ng", "j", "ch", "k", "t", "p", ""];
/// The letters on their own (compatibility jamo ㄱ … ㅎ, then ㅏ … ㅣ).
const JAMO_CONSONANTS: [&str; 30] = ["g", "kk", "gs", "n", "nj", "nh", "d", "tt", "r", "lg", "lm", "lb", "ls", "lt", "lp", "lh", "m", "b", "pp", "bs", "s", "ss", "", "j", "jj", "ch", "k", "t", "p", "h"];

/// ㄹ as an initial, ㅇ as an initial.
const RIEUL: usize = 5;
/// The index of ㅇ among the initials.
const IEUNG: usize = 11;

/// The finals with ㅎ (ㄶ, ㅀ, ㅎ): before ㄱ, ㄷ or ㅈ they make it k, t or ch, and leave what they write
/// otherwise (n, l, nothing).
fn h_final(fin: usize) -> Option<&'static str> {
    match fin {
        6 => Some("n"),
        15 => Some("l"),
        27 => Some(""),
        _ => None,
    }
}

/// ㄱ, ㄷ, ㅈ as an initial after a final with ㅎ: k, t, ch.
fn aspirated(ini: usize) -> Option<&'static str> {
    match ini {
        0 => Some("k"),
        3 => Some("t"),
        12 => Some("ch"),
        _ => None,
    }
}

/// A Hangul syllable as (initial, vowel, final) indexes.
fn split(c: char) -> Option<(usize, usize, usize)> {
    let u = c as u32;
    if !(0xAC00..=0xD7A3).contains(&u) {
        return None;
    }
    let s = (u - 0xAC00) as usize;
    Some((s / (21 * 28), (s % (21 * 28)) / 28, s % 28))
}

/// The Revised Romanization of a text with Hangul, or None when it has none.
pub fn romanize(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if !chars.iter().any(|c| split(*c).is_some() || (0x3131..=0x3163).contains(&(*c as u32))) {
        return None;
    }
    let mut out = String::new();
    let mut caps: Vec<usize> = vec![];
    let starts_word = |out: &str| out.chars().next_back().is_none_or(|c| !c.is_alphanumeric() && c != '-');
    for (i, &c) in chars.iter().enumerate() {
        if let Some((ini, vow, fin)) = split(c) {
            let next = chars.get(i + 1).and_then(|n| split(*n));
            let prev = i.checked_sub(1).and_then(|p| chars.get(p)).and_then(|p| split(*p));
            if starts_word(&out) {
                caps.push(out.len());
            }
            // the initial: silent ㅇ; ㄹ after a final read as l is l (물론 mullon); ㄱ ㄷ ㅈ after ㅎ k t ch
            let initial = match prev {
                _ if ini == IEUNG => "",
                Some((_, _, pf)) if ini == RIEUL && FINALS[pf] == "l" => "l",
                Some((_, _, pf)) if h_final(pf).is_some() && aspirated(ini).is_some() => aspirated(ini).unwrap_or(""),
                _ => INITIALS[ini],
            };
            out.push_str(initial);
            out.push_str(VOWELS[vow]);
            if fin != 0 {
                let linked = matches!(next, Some((n_ini, _, _)) if n_ini == IEUNG);
                let before_h = next.and_then(|(n_ini, _, _)| aspirated(n_ini)).and(h_final(fin));
                out.push_str(match before_h {
                    Some(h) => h,
                    None if linked => FINALS_LINKED[fin],
                    None => FINALS[fin],
                });
            }
            continue;
        }
        let u = c as u32;
        if (0x3131..=0x314E).contains(&u) || (0x314F..=0x3163).contains(&u) {
            if starts_word(&out) {
                caps.push(out.len());
            }
            out.push_str(if u <= 0x314E { JAMO_CONSONANTS[(u - 0x3131) as usize] } else { VOWELS[(u - 0x314F) as usize] });
            continue;
        }
        out.push(c);
    }
    caps.sort_unstable();
    caps.dedup();
    for &at in caps.iter().rev() {
        if let Some(c) = out[at..].chars().next().filter(|c| c.is_ascii_lowercase()) {
            out.replace_range(at..at + 1, &c.to_ascii_uppercase().to_string());
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out.chars().any(|c| c.is_ascii_alphabetic()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twenty titles and names with their Revised Romanization; the last ones are spelled without the sound
    /// changes (official spelling in the comment).
    const CASES: &[(&str, &str)] = &[
        ("봄날", "Bomnal"),                                  // BTS
        ("아이유", "Aiyu"),                                  // IU
        ("소녀시대", "Sonyeosidae"),                         // Girls' Generation
        ("방탄소년단", "Bangtansonyeondan"),                 // BTS
        ("피 땀 눈물", "Pi Ttam Nunmul"),                    // BTS
        ("사랑", "Sarang"),
        ("빨간 맛", "Ppalgan Mat"),                          // Red Velvet
        ("블랙핑크", "Beullaekpingkeu"),                     // BLACKPINK: ㄹㄹ is ll
        ("트와이스", "Teuwaiseu"),                           // TWICE
        ("뉴진스", "Nyujinseu"),                             // NewJeans
        ("한국어", "Hangugeo"),                              // a final before ㅇ is read with the next syllable
        ("좋아", "Joa"),                                     // ㅎ is silent before ㅇ
        ("괜찮아", "Gwaenchana"),                            // ㄶ before ㅇ: n
        ("앉아", "Anja"),                                    // ㄵ before ㅇ: nj
        ("의사", "Uisa"),
        ("물론", "Mullon"),
        ("읽다", "Ikda"),                                    // ㄺ before a consonant: k
        ("같이", "Gati"),                                    // without assimilation; official gachi
        ("신라", "Sinra"),                                   // without assimilation; official Silla
        ("독립", "Dokrip"),                                  // without assimilation; official dongnip
        ("거짓말", "Geojitmal"),                             // BIGBANG; without assimilation; official geojinmal
        ("첫눈처럼 너에게 가겠다", "Cheotnuncheoreom Neoege Gagetda"), // EXO; official Cheonnuncheoreom
        ("없어", "Eopseo"),                                  // ㅄ before ㅇ: ps
        ("값이", "Gapsi"),
        ("넋이", "Neoksi"),                                  // ㄳ before ㅇ: ks
        ("어떻게", "Eotteoke"),                              // ㅎ before ㄱ: k
        ("그렇게", "Geureoke"),
        ("좋다", "Jota"),                                    // ㅎ before ㄷ: t
        ("많다", "Manta"),                                   // ㄶ before ㄷ: n, t
        ("싫지", "Silchi"),                                  // ㅀ before ㅈ: l, ch
        ("어떻게 이별까지 사랑하겠어", "Eotteoke Ibyeolkkaji Saranghagesseo"), // AKMU
    ];

    #[test]
    fn twenty_cases() {
        assert!(CASES.len() >= 20);
        let mut wrong = vec![];
        for (hangul, want) in CASES {
            let got = romanize(hangul);
            if got.as_deref() != Some(*want) {
                wrong.push(format!("{hangul}: {got:?}, expected {want}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn other_characters_stay() {
        assert_eq!(romanize("봄날 (Spring Day)").as_deref(), Some("Bomnal (Spring Day)"));
        assert_eq!(romanize("Dynamite"), None);
        assert_eq!(romanize("夜に駆ける"), None);
        assert_eq!(romanize("ㅋㅋ").as_deref(), Some("Kk"));
        // ㄹ at the start of a word is r, after another final too
        assert_eq!(romanize("라면").as_deref(), Some("Ramyeon"));
        assert_eq!(romanize("종로").as_deref(), Some("Jongro"));
    }
}
