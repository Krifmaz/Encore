//! Version-aware track identity: tells originals, remixes, live takes, covers, edits… apart.
//! Everything that compares two tracks goes through `compare()` so the rules are identical everywhere.
use fancy_regex::Regex as FRegex;
use regex::Regex;
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

// version markers → category. Categories in IGNORED don't change the song's identity.
const VERSION_PATTERNS: [(&str, &str); 15] = [
    (r"\bslowed\b|\breverb\b|\bsped[\s-]*up\b|\bnightcore\b|\b8d\b|\bdaycore\b|\bchopped\b|\bbass[\s-]?boost(?:ed)?\b|\blo[\s-]?fi\b|\bpitched\b|\btik[\s-]?tok version\b", "altered"),
    (r"\bkaraoke\b", "karaoke"),
    (r"\ba[\s-]?cappella\b|\bacapella\b|\bvocals? only\b", "acappella"),
    (r"\binstrumental\b|\boff[\s-]?vocal\b", "instrumental"),
    (r"\bremix\b|\brmx\b|\bre-?work\b|\bbootleg\b|\bflip\b|\bvip\b|\bmash-?up\b|\bdub\b|\bclub mix\b|\b(?<!original )(?<!extended )(?<!radio )(?<!album )(?<!single )mix\b", "remix"),
    (r"\blive\b|\bin concert\b|\bbbc session\b|\bsession\b", "live"),
    (r"\bacoustic\b|\bunplugged\b|\bstripped\b|\bpiano version\b", "acoustic"),
    (r"\bdemo\b", "demo"),
    (r"\bcover\b|\btribute\b", "cover"),
    (r"\bextended\b", "extended"),
    (r"\bradio edit\b|\bsingle edit\b|\bshort edit\b|\b(?<!re-)edit\b|\bradio version\b", "edit"),
    (r"\bre-?recorded\b|\btaylor'?s version\b|\b20\d\d version\b|\bnew version\b", "rerecorded"),
    (r"\borchestral\b|\bsymphonic\b", "orchestral"),
    (r"\bclean\b|\bexplicit\b|\bcensored\b", "explicitness"),
    (r"\bremaster(?:ed)?\b|\bmono\b|\bstereo\b|\bdeluxe\b|\bbonus\b|\banniversary\b|\boriginal mix\b|\balbum version\b|\bsingle version\b|\bfrom .*\b|\bsoundtrack\b|\bofficial\b|\bmusic video\b|\blyric(s| video)?\b|\bvisuali[sz]er\b|\baudio\b|\bvideo\b|\bhd\b|\bhq\b|\b4k\b|\bmv\b", "ignored"),
];
const IGNORED: [&str; 2] = ["ignored", "explicitness"];

static PATS: LazyLock<Vec<(FRegex, FRegex, &'static str)>> = LazyLock::new(|| {
    VERSION_PATTERNS.iter().map(|(p, c)| (FRegex::new(p).unwrap(), FRegex::new(&format!(r"(?i)\s({p})\s*$")).unwrap(), *c)).collect()
});
static FEAT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*[\(\[]?\b(?:feat\.?|ft\.?|featuring|with)\s+[^\)\]]*[\)\]]?").unwrap());
static GROUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[\{]([^\)\]\}]*)[\)\]\}]").unwrap());
static FEAT_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(feat|ft|featuring|with)\b").unwrap());
static REMIXER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?)\s*\b(?:remix|rmx|rework|re-work|bootleg|flip|vip|mix|dub)\b").unwrap());
static REMIXER_CLEAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(official|the|feat.*)\b").unwrap());
static NONWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\s']").unwrap());
static SPLIT_ARTISTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*(?:,|;|/|&|\+|\bx\b|\band\b|\bfeat\.?|\bft\.?|\bfeaturing\b|\bvs\.?|\bwith\b)\s*").unwrap());

pub fn fold(s: &str) -> String {
    let s: String = s.nfkd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).collect::<String>().to_lowercase();
    let s = s.replace('&', " and ").replace('’', "'").replace('$', "s");
    let s = NONWORD.replace_all(&s, " ").replace('\'', "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, Default)]
pub struct Desc {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub length: Option<f64>,
    pub isrc: Option<String>,
    pub mbid: Option<String>,
    pub base: String,
    pub versions: BTreeSet<&'static str>,
    pub remixer: String,
    pub artists: Vec<String>,
}

pub fn split_artists(artist: &str) -> Vec<String> {
    let out: Vec<String> = SPLIT_ARTISTS.split(artist).map(fold).filter(|p| !p.is_empty()).collect();
    if out.is_empty() { vec![fold(artist)] } else { out }
}

fn is_match(re: &FRegex, s: &str) -> bool {
    re.is_match(s).unwrap_or(false)
}

/// → (base title, version categories, remixer)
pub fn parse_title(title: &str) -> (String, BTreeSet<&'static str>, String) {
    let mut chunks: Vec<String> = GROUP.captures_iter(title).map(|c| c[1].to_string()).collect();
    let mut rest = GROUP.replace_all(title, " ").to_string();
    // "Song - Live at X" / "Song - 2011 Remaster"
    if let Some((head, tail)) = rest.clone().split_once(" - ") {
        chunks.push(tail.to_string());
        rest = head.to_string();
    }
    rest = FEAT.replace_all(&rest, " ").to_string();
    let (mut versions, mut remixer) = (BTreeSet::new(), String::new());
    for ch in &chunks {
        let low = ch.to_lowercase();
        if FEAT_START.is_match(&low) {
            continue;
        }
        let mut found = false;
        for (re, _, cat) in PATS.iter() {
            if is_match(re, &low) {
                found = true;
                if !IGNORED.contains(cat) {
                    versions.insert(*cat);
                }
                if *cat == "remix" && remixer.is_empty() {
                    let r = REMIXER.captures(&low).map(|m| fold(&m[1])).unwrap_or_default();
                    remixer = REMIXER_CLEAN.replace_all(&r, "").trim().to_string();
                }
            }
        }
        if !found {
            // an unknown parenthetical that isn't a version marker (e.g. "(Part 2)") is part of the title
            rest.push(' ');
            rest.push_str(ch);
        }
    }
    // tokens like "Song Remix" without brackets
    for (_, tail, cat) in PATS.iter().take(5) {
        if ["remix", "instrumental", "karaoke", "altered"].contains(cat) {
            let padded = format!(" {rest}");
            if is_match(tail, &padded) {
                versions.insert(*cat);
                rest = tail.replace(&padded, "").to_string();
            }
        }
    }
    (fold(&rest), versions, remixer)
}

/// A download candidate's name (file name, release or video title) carries a version mark the requested
/// song doesn't: live, remix, acoustic, instrumental, a cappella, sped up / slowed / reverb / 8D /
/// nightcore, edit, extended, demo, karaoke, cover … → that mark (the candidate is skipped). Remaster,
/// deluxe, "official video" and the like don't count. The song's own words and the artist's are taken out
/// first, so "Live Forever" or the band Live don't read as live.
pub fn unwanted_version(title: &str, artist: &str, candidate: &str) -> Option<&'static str> {
    let (base, want, _) = parse_title(title);
    let mut rest = format!(" {} ", fold(candidate));
    let whole = format!(" {base} ");
    if !base.is_empty() && rest.contains(&whole) {
        rest = rest.replacen(&whole, " ", 1);
    } else {
        for w in base.split_whitespace() {
            rest = rest.replacen(&format!(" {w} "), " ", 1);
        }
    }
    for w in fold(artist).split_whitespace() {
        while rest.contains(&format!(" {w} ")) {
            rest = rest.replacen(&format!(" {w} "), " ", 1);
        }
    }
    let rest = rest.trim();
    PATS.iter().find(|(re, _, cat)| !IGNORED.contains(cat) && !want.contains(cat) && is_match(re, rest)).map(|(_, _, cat)| *cat)
}

/// A Soulseek file name from the requested title on ("Hole - Live Through This - 03 - Doll Parts" →
/// "doll parts"): the album and artist parts before it are not version marks. The whole name when the
/// title isn't in it word for word.
pub fn from_title_on(title: &str, candidate: &str) -> String {
    let (base, _, _) = parse_title(title);
    let c = fold(candidate);
    if base.is_empty() {
        return c;
    }
    let padded = format!(" {c} ");
    match padded.rfind(&format!(" {base} ")) {
        Some(i) => padded[i..].trim().to_string(),
        None => c,
    }
}

pub fn describe(title: &str, artist: &str, album: &str, length: Option<f64>, isrc: Option<&str>, mbid: Option<&str>) -> Desc {
    let (base, versions, remixer) = parse_title(title);
    Desc {
        title: title.into(), artist: artist.into(), album: album.into(), length: length.filter(|l| *l > 0.0),
        isrc: isrc.map(|i| i.to_uppercase().replace('-', "")).filter(|i| !i.is_empty()),
        mbid: mbid.filter(|m| !m.is_empty()).map(String::from), base, versions, remixer, artists: split_artists(artist),
    }
}

/// difflib.SequenceMatcher.ratio() (Ratcliff/Obershelp, without the junk heuristic).
pub fn ratio(a: &[char], b: &[char]) -> f64 {
    fn matches(a: &[char], b: &[char]) -> usize {
        if a.is_empty() || b.is_empty() {
            return 0;
        }
        let (mut best, mut bi, mut bj) = (0, 0, 0);
        let mut prev = vec![0usize; b.len() + 1];
        for i in 0..a.len() {
            let mut cur = vec![0usize; b.len() + 1];
            for j in 0..b.len() {
                if a[i] == b[j] {
                    cur[j + 1] = prev[j] + 1;
                    if cur[j + 1] > best {
                        (best, bi, bj) = (cur[j + 1], i + 1 - cur[j + 1], j + 1 - cur[j + 1]);
                    }
                }
            }
            prev = cur;
        }
        if best == 0 {
            return 0;
        }
        best + matches(&a[..bi], &b[..bj]) + matches(&a[bi + best..], &b[bj + best..])
    }
    let total = a.len() + b.len();
    if total == 0 { 1.0 } else { 2.0 * matches(a, b) as f64 / total as f64 }
}

pub fn sim(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b { 1.0 } else { ratio(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>()) }
}

fn artist_score(a: &Desc, b: &Desc) -> f64 {
    let mut best: f64 = 0.0;
    for x in &a.artists {
        for y in &b.artists {
            let contains = !x.is_empty() && !y.is_empty() && (x.contains(y.as_str()) || y.contains(x.as_str())) && x.len().min(y.len()) >= 4;
            best = best.max(sim(x, y)).max(if contains { 1.0 } else { 0.0 });
        }
    }
    best
}

#[derive(Debug, Clone)]
pub struct Verdict {
    pub matched: bool,
    pub score: f64,
    pub reason: String,
}
fn v(m: bool, s: f64, r: impl Into<String>) -> Verdict {
    Verdict { matched: m, score: s, reason: r.into() }
}

/// Is b the same recording/version as a?
pub fn compare(a: &Desc, b: &Desc, strict_duration: bool) -> Verdict {
    if a.mbid.is_some() && a.mbid == b.mbid {
        return v(true, 1.0, "same MusicBrainz recording");
    }
    if a.isrc.is_some() && a.isrc == b.isrc {
        return v(true, 0.98, "same ISRC");
    }
    let tsim = sim(&a.base, &b.base);
    if tsim < 0.85 {
        return v(false, tsim * 0.5, "different title");
    }
    if a.versions != b.versions {
        let diff: Vec<&str> = a.versions.symmetric_difference(&b.versions).copied().collect();
        return v(false, 0.3, format!("different version ({})", diff.join(", ")));
    }
    if a.versions.contains("remix") && !a.remixer.is_empty() && !b.remixer.is_empty() && sim(&a.remixer, &b.remixer) < 0.7 {
        return v(false, 0.35, "different remix");
    }
    let asc = artist_score(a, b);
    if asc < 0.75 {
        return v(false, 0.4, "different artist (cover?)");
    }
    let mut score = 0.55 * tsim + 0.35 * asc;
    match (a.length, b.length) {
        (Some(x), Some(y)) => {
            let d = (x - y).abs();
            if d > if strict_duration { 12.0 } else { 30.0 } {
                return v(false, 0.45, format!("length differs by {}s", d as i64));
            }
            score += if d <= 4.0 { 0.1 } else { 0.05 };
        }
        _ => score += 0.05,
    }
    v(score >= 0.85, (score * 1000.0).round() / 1000.0, "metadata match")
}

/// Grouping key – same key ⇒ same song & version (modulo fuzzy title).
pub fn key(d: &Desc) -> String {
    [d.artists.first().cloned().unwrap_or_default(), d.base.clone(), d.versions.iter().copied().collect::<Vec<_>>().join(","), d.remixer.clone()].join("|")
}

/// In-memory index of library tracks for fast matching.
pub struct LibraryIndex {
    pub rows: Vec<crate::db::Row>,
    by_mbid: HashMap<String, usize>,
    by_isrc: HashMap<String, usize>,
    by_word: HashMap<String, Vec<(Desc, usize)>>,
}

impl LibraryIndex {
    pub fn new(rows: Vec<crate::db::Row>) -> Self {
        use crate::db::RowExt;
        let mut ix = LibraryIndex { rows: vec![], by_mbid: HashMap::new(), by_isrc: HashMap::new(), by_word: HashMap::new() };
        for (i, r) in rows.iter().enumerate() {
            let d = describe(&r.s("title"), &r.s("artist"), &r.s("album"), Some(r.f("duration")), Some(&r.s("isrc")), Some(&r.s("mb_trackid")));
            if let Some(m) = &d.mbid {
                ix.by_mbid.entry(m.clone()).or_insert(i);
            }
            if let Some(m) = &d.isrc {
                ix.by_isrc.entry(m.clone()).or_insert(i);
            }
            let word = d.base.split_whitespace().next().unwrap_or("").to_string();
            ix.by_word.entry(word).or_default().push((d, i));
        }
        ix.rows = rows;
        ix
    }

    pub fn load(db: &crate::db::Db) -> Self {
        Self::new(db.query("SELECT id, path, title, artist, album, albumartist, album_id, duration, isrc, mb_trackid, format, bitrate FROM tracks", crate::p![]).unwrap_or_default())
    }

    pub fn find(&self, want: &Desc) -> Option<(&crate::db::Row, Verdict)> {
        if let Some(i) = want.mbid.as_ref().and_then(|m| self.by_mbid.get(m)) {
            return Some((&self.rows[*i], v(true, 1.0, "same MusicBrainz recording")));
        }
        if let Some(i) = want.isrc.as_ref().and_then(|m| self.by_isrc.get(m)) {
            return Some((&self.rows[*i], v(true, 0.98, "same ISRC")));
        }
        let word = want.base.split_whitespace().next().unwrap_or("");
        let mut best: Option<(usize, Verdict)> = None;
        for (d, i) in self.by_word.get(word).map(|v| v.as_slice()).unwrap_or(&[]) {
            let vv = compare(want, d, true);
            if vv.matched && best.as_ref().is_none_or(|(_, b)| vv.score > b.score) {
                best = Some((*i, vv));
            }
        }
        best.map(|(i, vv)| (&self.rows[i], vv))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(t: &str, a: &str) -> Desc {
        describe(t, a, "", None, None, None)
    }
    #[test]
    fn versions() {
        assert!(compare(&d("Blinding Lights", "The Weeknd"), &d("Blinding Lights (2020 Remaster)", "The Weeknd"), true).matched);
        assert!(!compare(&d("Blinding Lights", "The Weeknd"), &d("Blinding Lights (Chromatics Remix)", "The Weeknd"), true).matched);
        assert!(compare(&d("Song (Original Mix)", "X Y Z"), &d("Song", "X Y Z"), true).matched);
        assert!(!compare(&d("Song - Live at Wembley", "Band"), &d("Song", "Band"), true).matched);
        assert!(compare(&d("Héllo feat. Someone", "Adele & Someone"), &d("Hello", "Adele"), true).matched);
        assert!(!compare(&d("Song", "Band"), &d("Song (Radio Edit)", "Band"), true).matched);
        assert_eq!(parse_title("Track (Part 2)").0, "track part 2");
        assert_eq!(parse_title("Track (Foo Remix)").2, "foo");
        assert!(parse_title("Track Slowed").1.contains("altered"));
        assert!((sim("abcd", "bcde") - 0.75).abs() < 1e-9);
        assert!(parse_title("Song (Bass Boosted)").1.contains("altered"));
        assert!(parse_title("Song (Lo-Fi)").1.contains("altered"));
    }

    #[test]
    fn candidates_of_another_version_are_skipped() {
        let u = |title: &str, cand: &str| unwanted_version(title, "Travis Scott", cand);
        for (cand, mark) in [
            ("Travis Scott - 90210 (Live at Rolling Loud)", "live"),
            ("05 - 90210 (Live)", "live"),
            ("Travis Scott - 90210 (Chopnotslop Remix) [FLAC]", "remix"),
            ("90210 (Acoustic)", "acoustic"),
            ("90210 (Instrumental)", "instrumental"),
            ("90210 (A Cappella)", "acappella"),
            ("Travis Scott - 90210 (Sped Up)", "altered"),
            ("90210 slowed + reverb", "altered"),
            ("90210 (8D Audio)", "altered"),
            ("90210 nightcore", "altered"),
            ("90210 (Radio Edit)", "edit"),
            ("90210 (Extended Mix)", "extended"),
            ("90210 (Demo)", "demo"),
            ("Travis Scott - 90210 (Karaoke Version) [FLAC]", "karaoke"),
            ("90210 (Cover)", "cover"),
        ] {
            assert_eq!(u("90210", cand), Some(mark), "{cand}");
        }
        for cand in ["Travis Scott - 90210 [FLAC] [WEB]", "04 - 90210", "Travis Scott - 90210 (Official Audio)", "90210 (2015 Remaster)",
            "Travis Scott - Rodeo (Deluxe) incl 90210 [FLAC]", "90210 (feat. Kacy Hill)", "Travis Scott - 90210 (Explicit)"] {
            assert_eq!(u("90210", cand), None, "{cand}");
        }
        // the requested song's own mark is fine, and its words are not marks
        assert_eq!(u("90210 (Live)", "Travis Scott - 90210 (Live)"), None);
        assert_eq!(u("90210 - Acoustic", "90210 (Acoustic Version)"), None);
        assert_eq!(unwanted_version("Live Forever", "Oasis", "Oasis - Live Forever (Remastered)"), None);
        assert_eq!(unwanted_version("Live Forever", "Oasis", "Oasis - Live Forever (Live at Knebworth)"), Some("live"));
        assert_eq!(unwanted_version("Selling the Drama", "Live", "Live - Selling the Drama"), None);
        assert_eq!(unwanted_version("Remix", "X", "X - Remix"), None);
        // an album name before the title is not a mark (Soulseek "Artist - Album - NN - Title")
        let tail = from_title_on("Doll Parts", "Hole - Live Through This - 03 - Doll Parts");
        assert_eq!(tail, "doll parts");
        assert_eq!(unwanted_version("Doll Parts", "Hole", &tail), None);
        let tail = from_title_on("Doll Parts", "Hole - Live Through This - 03 - Doll Parts (Live)");
        assert_eq!(unwanted_version("Doll Parts", "Hole", &tail), Some("live"));
    }
}
