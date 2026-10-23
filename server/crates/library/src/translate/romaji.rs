//! Japanese kana → modified Hepburn romaji, from a table: the built-in romanization of Japanese titles
//! (translate/romanize.rs), no model needed.
//!
//! * Long vowels take a macron: the katakana mark ー lengthens the vowel before it (ラーメン → rāmen), and in
//!   hiragana おう / おお → ō, うう → ū, ああ → ā, ええ → ē (えい and いい stay as written). The o of the
//!   particles の and を starts no long vowel (みんなのうた → minnanouta: "〜のうた" is a common title); a verb
//!   that ends in おう (おもう → omō) can't be told from a long vowel by the table.
//! * Sokuon っ doubles the next consonant (ちょっと → chotto), and before ch it is t (まっちゃ → matcha).
//! * Yōon: きゃ → kya, しゃ → sha, ちょ → cho, じゅ → ju; the small vowels of foreign words: ファ → fa,
//!   ティ → ti, ウィ → wi, ヴァ → va, シェ → she, チェ → che.
//! * ん is n, and n' before a vowel or y (きんえん → kin'en).
//! * Particles are left as written: は ha, へ he, を wo (the reading rules need the grammar).
//! * Kanji have no reading here: a title with any is left as it is (None) – its reading has to come from
//!   metadata (MusicBrainz aliases in ja-Latn, an artist's Latin sort name: official.rs) or the model.
//!
//! Each word made from kana starts with a capital letter; Latin letters, digits and symbols stay as they are.
use unicode_normalization::UnicodeNormalization;

/// A kanji (or the marks that stand for one: 々 repeats the kanji before it, 〆): a title with any is left as
/// written.
pub fn is_kanji(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF | 0x3005 | 0x3006)
}

/// Hiragana or katakana (the long mark and the middle dot included).
pub fn is_kana(c: char) -> bool {
    matches!(c as u32, 0x3041..=0x309F | 0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F)
}

/// The romaji of a title in kana, or None when it has kanji or no kana at all.
pub fn romanize(text: &str) -> Option<String> {
    if text.chars().any(is_kanji) || !text.chars().any(is_kana) {
        return None;
    }
    // NFKC: half-width katakana become full width (ﾊﾞ → バ), full-width Latin letters and signs plain ones
    let chars: Vec<char> = text.nfkc().collect();
    let mut w = Writer::default();
    for c in chars {
        w.push(c);
    }
    let out = w.finish();
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (!out.is_empty() && out.chars().any(|c| c.is_ascii_alphabetic())).then_some(out)
}

/// A katakana as its hiragana (ア → あ), so one table serves both; (the hiragana, whether it was katakana).
fn fold(c: char) -> (char, bool) {
    match c as u32 {
        0x30A1..=0x30F6 => (char::from_u32(c as u32 - 0x60).unwrap_or(c), true),
        0x30FD => ('ゝ', true),
        0x30FE => ('ゞ', true),
        0x30F7..=0x30FC => (c, true),
        _ => (c, false),
    }
}

/// The romaji of one (hiragana-folded) kana that stands for a syllable on its own.
fn base(h: char) -> Option<&'static str> {
    Some(match h {
        'あ' => "a", 'い' => "i", 'う' => "u", 'え' => "e", 'お' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        'さ' => "sa", 'し' => "shi", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'ざ' => "za", 'じ' => "ji", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        'た' => "ta", 'ち' => "chi", 'つ' => "tsu", 'て' => "te", 'と' => "to",
        'だ' => "da", 'ぢ' => "ji", 'づ' => "zu", 'で' => "de", 'ど' => "do",
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "hi", 'ふ' => "fu", 'へ' => "he", 'ほ' => "ho",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        'や' => "ya", 'ゆ' => "yu", 'よ' => "yo",
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        'わ' => "wa", 'ゐ' => "i", 'ゑ' => "e", 'を' => "wo", 'ん' => "n",
        'ゔ' => "vu", 'ゕ' => "ka", 'ゖ' => "ke", 'ゎ' => "wa",
        // katakana with no hiragana: ヷ ヸ ヹ ヺ
        'ヷ' => "va", 'ヸ' => "vi", 'ヹ' => "ve", 'ヺ' => "vo",
        _ => return None,
    })
}

/// The vowel of a small ぁぃぅぇぉ.
fn small_vowel(h: char) -> Option<char> {
    Some(match h {
        'ぁ' => 'a',
        'ぃ' => 'i',
        'ぅ' => 'u',
        'ぇ' => 'e',
        'ぉ' => 'o',
        _ => return None,
    })
}

/// The vowel of a small ゃゅょ.
fn small_y(h: char) -> Option<char> {
    Some(match h {
        'ゃ' => 'a',
        'ゅ' => 'u',
        'ょ' => 'o',
        _ => return None,
    })
}

/// The vowel with a macron (o → ō), or None when it is not a plain vowel.
fn long(c: char) -> Option<char> {
    Some(match c {
        'a' => 'ā',
        'i' => 'ī',
        'u' => 'ū',
        'e' => 'ē',
        'o' => 'ō',
        _ => return None,
    })
}

/// What the other marks become.
fn sign(c: char) -> Option<&'static str> {
    Some(match c {
        '・' => " ",
        '、' => ", ",
        '。' => ". ",
        '「' | '」' | '『' | '』' => "\"",
        '〜' | '～' => "~",
        '【' | '〔' => "(",
        '】' | '〕' => ")",
        _ => return None,
    })
}

/// Writes kana as Hepburn romaji, syllable by syllable.
#[derive(Default)]
struct Writer {
    /// the romaji so far
    out: String,
    /// where the last syllable starts in `out` (a small kana changes it)
    syl: usize,
    /// a small っ waits for the next syllable
    sokuon: bool,
    /// the last character was a kana, and whether it was written in hiragana; the last one was ん
    after_kana: bool,
    /// the last kana was hiragana
    hira: bool,
    /// the last kana was ん
    after_n: bool,
    /// the last kana written as a syllable of its own (folded to hiragana), '\0' after anything else
    prev: char,
    /// where a word made from kana starts in `out`: those get the capital letter
    caps: Vec<usize>,
}

impl Writer {
    /// Whether text pushed now starts a word: nothing before it, or a space or a sign (not a letter, a digit or
    /// the apostrophe of n').
    fn word_start(&self) -> bool {
        match self.out.chars().next_back() {
            None => true,
            Some(c) => c.is_whitespace() || !(c.is_alphanumeric() || c == '\''),
        }
    }

    /// Write a syllable; `after_n`: the kana before it was ん.
    fn syllable(&mut self, r: &str, hira: bool, after_n: bool) {
        let mut r = r.to_string();
        if self.sokuon {
            self.sokuon = false;
            if let Some(first) = r.chars().next().filter(|c| c.is_ascii_alphabetic() && !"aiueo".contains(*c)) {
                r = if r.starts_with("ch") { format!("t{r}") } else { format!("{first}{r}") };
            }
        }
        if after_n && r.starts_with(['a', 'i', 'u', 'e', 'o', 'y']) {
            self.out.push('\'');
        }
        if self.word_start() {
            self.caps.push(self.out.len());
        }
        self.syl = self.out.len();
        self.out.push_str(&r);
        self.after_kana = true;
        self.hira = hira;
    }

    /// The last syllable as written so far, in lower case.
    fn last(&self) -> String {
        self.out[self.syl..].to_lowercase()
    }

    /// Replace the end of the last syllable: drop `cut` characters, add `add`.
    fn reshape(&mut self, cut: usize, add: &str) {
        for _ in 0..cut {
            self.out.pop();
        }
        self.out.push_str(add);
    }

    /// Write one character.
    fn push(&mut self, c: char) {
        let (h, kata) = fold(c);
        let was_n = std::mem::take(&mut self.after_n);
        let prev = std::mem::replace(&mut self.prev, '\0');
        // the sokuon
        if h == 'っ' {
            self.sokuon = true;
            self.after_kana = true;
            self.hira = !kata;
            return;
        }
        // the long mark lengthens the vowel before it
        if h == 'ー' {
            if self.after_kana {
                // (what kana make is lower case until `finish`)
                if let Some(l) = self.out.chars().next_back().and_then(long) {
                    self.out.pop();
                    self.out.push(l);
                }
            }
            self.sokuon = false;
            return;
        }
        // yōon: きゃ → kya, しゃ → sha
        if let Some(v) = small_y(h) {
            let s = self.last();
            if !self.after_kana || s.is_empty() {
                self.syllable(&format!("y{v}"), !kata, was_n);
            } else if s.ends_with("shi") || s.ends_with("chi") || s.ends_with("ji") {
                self.reshape(1, &v.to_string());
            } else if s.ends_with("te") || s.ends_with("de") {
                // テュ → tyu
                self.reshape(1, &format!("y{v}"));
            } else if (s.ends_with("fu") || s.ends_with("vu")) || (s.ends_with('i') && s.len() >= 2) {
                self.reshape(1, &format!("y{v}"));
            } else {
                self.syllable(&format!("y{v}"), !kata, was_n);
            }
            return;
        }
        // the small vowels of foreign words: ファ → fa, ティ → ti, ウィ → wi; otherwise the vowel again (ねぇ → nee)
        if let Some(v) = small_vowel(h) {
            let s = self.last();
            if !self.after_kana || s.is_empty() {
                self.syllable(&v.to_string(), !kata, was_n);
                return;
            }
            let ends = |x: &str| s.ends_with(x);
            if (ends("fu") || ends("vu") || ends("tsu")) && v != 'u' {
                self.reshape(1, &v.to_string());
            } else if (ends("te") || ends("de")) && v == 'i' {
                self.reshape(1, "i");
            } else if (ends("to") || ends("do")) && v == 'u' {
                self.reshape(1, "u");
            } else if (ends("su") || ends("zu")) && v == 'i' {
                self.reshape(1, "i");
            } else if (ends("shi") || ends("chi") || ends("ji")) && v == 'e' {
                self.reshape(1, "e");
            } else if (ends("ku") || ends("gu") || s == "u") && v != 'u' {
                // クァ → kwa, ウィ → wi
                self.reshape(1, &format!("w{v}"));
            } else if s.ends_with('i') && v == 'e' {
                // イェ → ye, キェ → kye
                self.reshape(1, "ye");
            } else {
                self.out.push(v);
            }
            return;
        }
        // the iteration marks repeat the syllable before
        if h == 'ゝ' || h == 'ゞ' {
            let s = self.last();
            if self.after_kana && !s.is_empty() {
                self.syllable(&s, !kata, was_n);
            }
            return;
        }
        if let Some(r) = base(h) {
            let hira = !kata;
            // hiragana long vowels: おう / おお → ō, うう → ū, ああ → ā, ええ → ē (not after the particles の, を)
            if hira && self.hira && self.after_kana && !self.sokuon && !was_n && !matches!(prev, 'の' | 'を') {
                let last = self.out.chars().next_back();
                let merged = match (last, h) {
                    (Some('o'), 'う' | 'お') | (Some('u'), 'う') | (Some('a'), 'あ') | (Some('e'), 'え') => last.and_then(long),
                    _ => None,
                };
                if let Some(l) = merged {
                    self.out.pop();
                    self.out.push(l);
                    self.prev = h;
                    return;
                }
            }
            self.syllable(r, hira, was_n);
            self.prev = h;
            if h == 'ん' {
                self.after_n = true;
            }
            return;
        }
        // anything else: the marks become Latin ones, the rest stays as it is
        self.sokuon = false;
        self.after_kana = false;
        match sign(c) {
            Some(s) => self.out.push_str(s),
            None => self.out.push(c),
        }
    }

    /// The romaji, with the capitals put in.
    fn finish(mut self) -> String {
        // capitals from the end, so the positions before stay where they are
        self.caps.sort_unstable();
        self.caps.dedup();
        for &at in self.caps.iter().rev() {
            if let Some(c) = self.out[at..].chars().next() {
                let up: String = c.to_uppercase().collect();
                self.out.replace_range(at..at + c.len_utf8(), &up);
            }
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forty song titles in kana (and two artists), with the romaji a reader would expect in modified Hepburn.
    const TITLES: &[(&str, &str)] = &[
        ("ハルジオン", "Harujion"),                         // YOASOBI
        ("アイドル", "Aidoru"),                             // YOASOBI
        ("ヨルシカ", "Yorushika"),                          // the band
        ("うっせぇわ", "Usseewa"),                          // Ado
        ("ドライフラワー", "Doraifurawā"),                  // Yuuri
        ("マリーゴールド", "Marīgōrudo"),                   // Aimyon
        ("ありがとう", "Arigatō"),
        ("さくらんぼ", "Sakuranbo"),                        // Ai Otsuka
        ("シャルル", "Sharuru"),                            // Balloon
        ("ロキ", "Roki"),                                   // Mikito-P
        ("チョコレイト・ディスコ", "Chokoreito Disuko"),    // Perfume
        ("ポリリズム", "Poririzumu"),                       // Perfume
        ("ヒロイン", "Hiroin"),                             // back number
        ("クリスマスソング", "Kurisumasusongu"),            // back number
        ("ハッピーエンド", "Happīendo"),                    // back number
        ("ファンファーレ", "Fanfāre"),                      // sumika
        ("ウィーアー!", "Wīā!"),                            // Hiroshi Kitadani
        ("ヴァンパイア", "Vanpaia"),                        // DECO*27
        ("ゴーストルール", "Gōsutorūru"),                   // DECO*27
        ("ラビットホール", "Rabittohōru"),                  // DECO*27
        ("ヒバナ", "Hibana"),                               // DECO*27
        ("シャンティ", "Shanti"),                           // wotaku
        ("ブリキノダンス", "Burikinodansu"),                // Hinata Electric Works
        ("かくれんぼ", "Kakurenbo"),                        // AliA
        ("いーあるふぁんくらぶ", "Īarufankurabu"),          // Mikito-P
        ("マトリョシカ", "Matoryoshika"),                   // Hachi
        ("パンダヒーロー", "Pandahīrō"),                    // Hachi
        ("メルト", "Meruto"),                               // ryo
        ("ワールドイズマイン", "Wārudoizumain"),            // ryo
        ("ブラック★ロックシューター", "Burakku★Rokkushūtā"), // supercell
        ("ヘビーローテーション", "Hebīrōtēshon"),           // AKB48
        ("フライングゲット", "Furaingugetto"),              // AKB48
        ("ポニーテールとシュシュ", "Ponītērutoshushu"),     // AKB48
        ("ギンガムチェック", "Gingamuchekku"),              // AKB48
        ("リンダリンダ", "Rindarinda"),                     // The Blue Hearts
        ("おどるポンポコリン", "Odoruponpokorin"),          // B.B.Queens
        ("さんぽ", "Sanpo"),                                // from My Neighbor Totoro
        ("にんげんっていいな", "Ningentteiina"),
        ("ほんとうのきもち", "Hontōnokimochi"),
        ("きょうもハレバレ", "Kyōmoharebare"),
        ("さよならエレジー", "Sayonaraerejī"),              // Masaki Suda
        ("ジョン・レノン", "Jon Renon"),
    ];

    #[test]
    fn forty_song_titles() {
        assert!(TITLES.len() >= 40);
        let mut wrong = vec![];
        for (kana, want) in TITLES {
            let got = romanize(kana);
            if got.as_deref() != Some(*want) {
                wrong.push(format!("{kana}: {got:?}, expected {want}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn the_rules() {
        let r = |s: &str| romanize(s).unwrap_or_default();
        // sokuon before ch, and at the end of a word (left out)
        assert_eq!(r("まっちゃ"), "Matcha");
        assert_eq!(r("キッチン"), "Kitchin");
        assert_eq!(r("あっ"), "A");
        // n' before a vowel or y
        assert_eq!(r("きんえん"), "Kin'en");
        assert_eq!(r("こんや"), "Kon'ya");
        assert_eq!(r("しんぱい"), "Shinpai");
        // particles as written
        assert_eq!(r("こんにちは"), "Konnichiha");
        assert_eq!(r("ぼくをみて"), "Bokuwomite");
        // long vowels in hiragana; えい and いい stay
        assert_eq!(r("とうきょう"), "Tōkyō");
        assert_eq!(r("おおきな"), "Ōkina");
        assert_eq!(r("おねえさん"), "Onēsan");
        assert_eq!(r("せんせい"), "Sensei");
        assert_eq!(r("いいね"), "Iine");
        // katakana doubles stay as written; only ー makes the macron
        assert_eq!(r("バレエ"), "Baree");
        // foreign sounds
        assert_eq!(r("フィーリング"), "Fīringu");
        assert_eq!(r("ウォーター"), "Wōtā");
        assert_eq!(r("ツァイト"), "Tsaito");
        assert_eq!(r("デュエット"), "Dyuetto");
        assert_eq!(r("トゥナイト"), "Tunaito");
        assert_eq!(r("シェイク"), "Sheiku");
        assert_eq!(r("イェーイ"), "Yēi");
        // half-width katakana, full-width signs, Latin parts and spaces
        assert_eq!(r("ﾊﾙｼﾞｵﾝ"), "Harujion");
        assert_eq!(r("ハルジオン (Remix)"), "Harujion (Remix)");
        assert_eq!(r("「ヒカリへ」"), "\"Hikarihe\"");
        assert_eq!(r("ハレ　ハレ！"), "Hare Hare!");
        assert_eq!(r("Lemonのはな"), "Lemonnohana");
        // the particle の starts no long vowel: "〜のうた"
        assert_eq!(r("のうた"), "Nouta");
        assert_eq!(r("みんなのうた"), "Minnanouta");
        assert_eq!(r("かえるのうた"), "Kaerunouta");
        assert_eq!(r("ぼくをおもう"), "Bokuwoomō");
    }

    #[test]
    fn kanji_and_other_scripts_are_left_as_written() {
        assert_eq!(romanize("夜に駆ける"), None);
        assert_eq!(romanize("東京フラッシュ"), None);
        assert_eq!(romanize("人々のうた"), None);
        assert_eq!(romanize("Lemon"), None);
        assert_eq!(romanize("봄날"), None);
        assert_eq!(romanize("ー"), None);
    }
}
