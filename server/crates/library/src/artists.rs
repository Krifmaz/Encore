//! Library artists. There is no artists table: an artist is a name in `tracks.albumartist`. This module
//! turns those names into real artists and keeps the result in memory (rebuilt when the tracks change):
//! - spellings with the same key ("The Black Eyed Peas" / "black eyed peas") or the same MusicBrainz id
//!   are one artist, shown with the spelling that has the most songs;
//! - collaborations ("Kanye West & XXXTENTACION") are split into their artists, and then count under each
//!   part: by the artists the files' ARTISTS / ALBUMARTISTS tags or MusicBrainz credit (one credited artist
//!   keeps "Simon & Garfunkel" whole), else when every part is an artist in the library (an album artist, a
//!   song's artist, a credited artist or a known alias), and always around "feat.";
//! - aliases from MusicBrainz (Ye ↔ Kanye West), looked up in the background after scans.
//!
//! It also ranks searches: exact > starts with > a word starts with > contains, case- and accent-blind.
use encore_core::audio::LOSSLESS;
use encore_core::ids::is_mbid;
use encore_db::{now, Conn, Db, RowExt};
use encore_db::p;
use crate::Ctx;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use regex::Regex;
use std::time::{Duration, Instant};
use unicode_normalization::UnicodeNormalization;

// ---- names ---------------------------------------------------------------------------------------

/// Lowercase, accents dropped, "&" read as "and", punctuation as spaces, spaces collapsed.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfkd() {
        if unicode_normalization::char::is_combining_mark(c) || c == '\'' || c == '’' {
            continue;
        }
        if c == '&' {
            out.push_str(" and ");
        } else if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A folded name without a leading "the " (unless that is all of it).
pub(crate) fn strip_the(f: &str) -> &str {
    match f.strip_prefix("the ") {
        Some(rest) if !rest.is_empty() => rest,
        _ => f,
    }
}

/// The identity of an artist name: `fold` without a leading "the". A name of punctuation or symbols
/// only ("!!!") keeps its own text, marked with "#" (which `fold` never makes) so it can't meet a folded key.
pub fn key(s: &str) -> String {
    let k = strip_the(&fold(s)).to_string();
    if k.is_empty() {
        let t = s.trim();
        if !t.is_empty() {
            return format!("#{}", t.to_lowercase());
        }
    }
    k
}

/// Separators between the artists of a collaboration (matched case-insensitively). " · " and "; " are how
/// streaming services and multi-value tags join artists ("20syl · Lord Apex"), "×" is the multiplication sign
/// some releases use for "x".
const SEPARATORS: &[&str] = &[" featuring ", " feat. ", " feat ", " ft. ", " ft ", " vs. ", " vs ", " with ", " and ", " & ", ", ", " x ", " × ", " / ", " · ", "; ", " + "];

/// Pieces of `s` between separators, as byte ranges.
fn segments(s: &str) -> Vec<(usize, usize)> {
    let low = s.to_ascii_lowercase();
    let b = low.as_bytes();
    let mut out = vec![];
    let (mut start, mut i) = (0, 0);
    while i < b.len() {
        if let Some(sep) = SEPARATORS.iter().find(|sep| b[i..].starts_with(sep.as_bytes())) {
            out.push((start, i));
            i += sep.len();
            start = i;
        } else {
            i += 1;
        }
    }
    out.push((start, b.len()));
    out
}

/// Splits a collaboration into the names of its artists when every part is `known` (by key); the finest
/// split wins. None when the name is one artist.
pub fn split(s: &str, known: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let segs = segments(s);
    if segs.len() < 2 || segs.len() > 24 {
        return None;
    }
    fn go(s: &str, segs: &[(usize, usize)], i: usize, known: &dyn Fn(&str) -> bool, out: &mut Vec<String>) -> bool {
        if i == segs.len() {
            return out.len() >= 2;
        }
        for j in i..segs.len() {
            if i == 0 && j == segs.len() - 1 {
                break; // the whole name is not a split
            }
            let part = s[segs[i].0..segs[j].1].trim();
            let k = key(part);
            if !k.is_empty() && known(&k) {
                out.push(part.to_string());
                if go(s, segs, j + 1, known, out) {
                    return true;
                }
                out.pop();
            }
        }
        false
    }
    let mut out = vec![];
    go(s, &segs, 0, known, &mut out).then_some(out)
}

/// Separators that always part two artists, known or not: "feat." and the joins no artist name is made of
/// (" · " from streaming services, "; " and " / " from multi-value tags written as one text).
const HARD: &[&str] = &[" featuring ", " feat. ", " feat ", " ft. ", " ft ", " · ", "; ", " / "];

/// The pieces of `s` between the separators in `seps` (matched case-insensitively), trimmed, empty ones left out.
fn cut(s: &str, seps: &[&str]) -> Vec<String> {
    let low = s.to_ascii_lowercase();
    let b = low.as_bytes();
    let (mut start, mut i, mut out) = (0, 0, vec![]);
    while i < b.len() {
        if let Some(sep) = seps.iter().find(|sep| b[i..].starts_with(sep.as_bytes())) {
            // every separator starts and ends on an ASCII byte, so both cuts fall on character boundaries
            out.push(s[start..i].trim().to_string());
            i += sep.len();
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(s[start..].trim().to_string());
    out.retain(|p| !p.is_empty());
    out
}

/// A collaboration whose parts aren't all in the library, split anyway when it plainly is one: some part of it
/// is a known artist ("3R2 & t+pazolite" with t+pazolite in the library), or it is a list of three or more
/// names joined by commas only ("111nightshift, c4rp3tg4r, Moe Shop"). Known runs of pieces stay together
/// (the longest one wins, "Tyler, The Creator & Ye" → "Tyler, The Creator", "Ye"), each unknown piece becomes
/// an artist of its own. The whole text never counts as known here: that is the name being split. None when
/// it reads as one name ("Earth, Wind & Fire", "Simon & Garfunkel" with neither half known).
pub fn loose_split(s: &str, known: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let segs = segments(s);
    if segs.len() < 2 || segs.len() > 24 {
        return None;
    }
    let whole = key(s);
    let piece = |i: usize, j: usize| s[segs[i].0..segs[j].1].trim();
    let is_known = |p: &str| {
        let k = key(p);
        !k.is_empty() && k != whole && known(&k)
    };
    // greedy, left to right: the longest known run from here, else one piece
    let (mut out, mut any_known, mut i) = (vec![], false, 0);
    while i < segs.len() {
        let run = (i..segs.len()).rev().find(|&j| is_known(piece(i, j)));
        let j = run.unwrap_or(i);
        any_known |= run.is_some();
        let p = piece(i, j);
        if !p.is_empty() {
            out.push(p.to_string());
        }
        i = j + 1;
    }
    // "A, B, C": only commas between the pieces (the text between two segments is the separator)
    let commas_only = segs.windows(2).all(|w| s[w[0].1..w[1].0].trim() == ",");
    ((any_known || (segs.len() >= 3 && commas_only)) && out.len() >= 2).then_some(out)
}

/// The artists a collaboration text names, for telling a collaboration from an artist: split at the hard
/// separators (`HARD`) always; each piece then stays whole when it is a known artist (another than the whole
/// text), splits when all its parts are known (`split`), and otherwise splits loosely (`loose_split`). A text
/// that reads as one artist comes back as itself alone.
pub fn collab_parts(s: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return vec![];
    }
    let whole = key(s);
    cut(s, HARD).into_iter().flat_map(|part| {
        let k = key(&part);
        if !k.is_empty() && k != whole && known(&k) {
            return vec![part];
        }
        split(&part, known).or_else(|| loose_split(&part, known)).unwrap_or_else(|| vec![part])
    }).collect()
}

/// The artist names in a song's artist text, in order: the whole text when it is one known artist, else its
/// parts as `collab_parts` reads them ("Travis Scott feat. The Weeknd" and "20syl · Lord Apex" → both, known or
/// not; "A & B" when both are known, or when one is).
pub fn credit_names(s: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return vec![];
    }
    // a library artist whose own name has " ft " / " feat " in it stays whole
    let whole = key(s);
    if !whole.is_empty() && known(&whole) {
        return vec![s.to_string()];
    }
    collab_parts(s, known)
}

/// The artists a credit names, in order: (name, MusicBrainz id or "").
pub(crate) type Credit = Vec<(String, String)>;

/// A credit (`tracks.artist_credit`, `mb_credits.credit`: JSON `[{name, mbid?}]`) for the artist text `text`,
/// kept only when it fits that text: one name that is the whole text (one artist, never split), or several
/// that each appear in it as words. Anything else (a credit under other names, a stale tag) is left out.
pub(crate) fn credit_for(text: &str, json: &str) -> Option<Credit> {
    if json.len() < 4 {
        return None; // NULL, "" or "[]"
    }
    let v: Value = serde_json::from_str(json).ok()?;
    let list: Credit = v.as_array()?.iter().filter_map(|c| {
        let n = c["name"].as_str()?.trim();
        let m = c["mbid"].as_str().filter(|m| is_mbid(m)).unwrap_or("");
        (!n.is_empty()).then(|| (n.to_string(), m.to_string()))
    }).collect();
    match list.len() {
        0 => None,
        1 => (key(&list[0].0) == key(text)).then_some(list),
        _ => {
            let whole = format!(" {} ", fold(text));
            list.iter().all(|(n, _)| {
                let f = fold(n);
                !f.is_empty() && whole.contains(&format!(" {f} "))
            }).then_some(list)
        }
    }
}

/// The artists a credit (JSON `[{name, mbid?}]`) names beyond the artist text `text`, when it is the text's own
/// credit with more in it: at least one of its names is in the text as words ("Coldplay" with ARTISTS
/// "Coldplay; Selena Gomez" → Selena Gomez). Empty when the credit is about other artists, or adds nobody.
pub(crate) fn beyond_text(text: &str, json: &str) -> Credit {
    if json.len() < 4 {
        return vec![];
    }
    let Ok(v) = serde_json::from_str::<Value>(json) else { return vec![] };
    let list: Credit = v.as_array().into_iter().flatten().filter_map(|c| {
        let n = c["name"].as_str()?.trim();
        let m = c["mbid"].as_str().filter(|m| is_mbid(m)).unwrap_or("");
        (!n.is_empty()).then(|| (n.to_string(), m.to_string()))
    }).collect();
    let whole = format!(" {} ", fold(text));
    let in_text = |n: &str| {
        let f = fold(n);
        !f.is_empty() && whole.contains(&format!(" {f} "))
    };
    if list.len() < 2 || !list.iter().any(|(n, _)| in_text(n)) {
        return vec![];
    }
    list.into_iter().filter(|(n, _)| !in_text(n)).collect()
}

/// A featured-artist note in a title: "(feat. X)", "[ft. X & Y]", "(with X)", or " feat. X" to the end.
static TITLE_FEAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[\(\[]\s*(feat\.?|ft\.?|featuring|with)\s+([^\)\]]+)[\)\]]|\s(?:feat\.|ft\.|featuring)\s+(.+)$").unwrap()
});

/// The artists a title features ("Let Somebody Go (feat. Selena Gomez)" → Selena Gomez; "(feat. A & B)" → both
/// when either is known, see `collab_parts`). A known artist's whole name stays one ("feat. Simon & Garfunkel").
pub fn title_features(title: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for c in TITLE_FEAT.captures_iter(title) {
        let Some(m) = c.get(2).or_else(|| c.get(3)) else { continue };
        let clause = m.as_str().trim();
        let k = key(clause);
        let mut names = if !k.is_empty() && known(&k) { vec![clause.to_string()] } else { collab_parts(clause, known) };
        // "(with …)" is also how titles say "(With Love)": only artists the library knows count there
        if c.get(1).is_some_and(|w| w.as_str().eq_ignore_ascii_case("with")) {
            names.retain(|n| known(&key(n)));
        }
        for n in names {
            if !key(&n).is_empty() && !out.iter().any(|x| key(x) == key(&n)) {
                out.push(n);
            }
        }
    }
    out
}

// ---- matching ------------------------------------------------------------------------------------

/// A search query, folded once.
pub struct Q {
    /// the query folded
    f: String,
    /// folded, without a leading "the "
    k: String,
    /// folded, with a space in front (a word start)
    word: String,
    /// folded, without spaces
    compact: String,
    /// the query as typed, lower case
    raw: String,
}

impl Q {
    /// Fold a query once.
    pub fn new(q: &str) -> Q {
        let f = fold(q);
        let k = strip_the(&f).to_string();
        Q { word: format!(" {f}"), compact: f.replace(' ', ""), k, f, raw: q.trim().to_lowercase() }
    }

    /// 0 exact, 1 starts with, 2 a word starts with, 3 contains; None: no match. `h` is `fold` of the text.
    pub fn tier(&self, raw: &str, h: &str) -> Option<u8> {
        if self.f.is_empty() {
            // a query of punctuation only ("!!!"): plain text
            let r = raw.trim().to_lowercase();
            return if self.raw.is_empty() { None } else if r == self.raw { Some(0) } else if r.contains(&self.raw) { Some(3) } else { None };
        }
        if h.is_empty() {
            return None;
        }
        let hk = strip_the(h);
        if h == self.f || hk == self.k {
            Some(0)
        } else if h.starts_with(&self.f) || hk.starts_with(&self.k) {
            Some(1)
        } else if h.contains(&self.word) {
            Some(2)
        } else if h.contains(&self.f) || (self.compact.len() >= 3 && h.replace(' ', "").contains(&self.compact)) {
            Some(3)
        } else {
            None
        }
    }

    /// Like `tier`, without the space-blind fallback (cheap enough for every song in the library).
    fn tier_fast(&self, raw: &str, h: &str) -> Option<u8> {
        if self.f.is_empty() {
            return self.tier(raw, h);
        }
        if h.is_empty() {
            return None;
        }
        let hk = strip_the(h);
        if h == self.f || hk == self.k {
            Some(0)
        } else if h.starts_with(&self.f) || hk.starts_with(&self.k) {
            Some(1)
        } else if h.contains(&self.word) {
            Some(2)
        } else if h.contains(&self.f) {
            Some(3)
        } else {
            None
        }
    }
}

/// The better (lower) of two tiers.
fn best(a: Option<u8>, b: Option<u8>) -> Option<u8> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

// ---- the index -----------------------------------------------------------------------------------

/// How `Index::list` compares two artists for one sort column.
type ArtistCmp = Box<dyn Fn(&Artist, &Artist) -> Ordering>;

/// One artist of the library (an albumartist and its other spellings, aliases and collaborations).
#[derive(Clone, Debug, Default)]
pub struct Artist {
    /// the name shown
    pub name: String,
    /// its key ([`key`] of the name)
    pub key: String,
    /// its MusicBrainz id, when the files carry one
    pub mbid: String,
    /// albumartist spellings of this artist itself
    pub spellings: Vec<String>,
    /// other names (MusicBrainz aliases)
    pub aliases: Vec<String>,
    /// songs
    pub tracks: i64,
    /// albums
    pub albums: i64,
    /// bytes of its files
    pub size: i64,
    /// when a song of it was last added, Unix seconds
    pub added: f64,
    /// the album its picture comes from
    pub cover_album: String,
    /// every albumartist value whose albums belong here (own spellings and collaborations)
    pub aa_names: Vec<String>,
    /// every track artist value whose songs belong here
    pub artist_names: Vec<String>,
    /// every name it matches by: (as written, folded, is an alias)
    pub(crate) folds: Vec<(String, String, bool)>, // (raw, fold, is alias)
    /// songs it is featured on without being named in their artist text ("Let Somebody Go" by Coldplay,
    /// featuring Selena Gomez in its title or its ARTISTS / MusicBrainz credit): their row ids
    pub feat_tracks: Vec<i64>,
}

/// A name as it appears in the tracks table, with the artists it stands for.
pub(crate) struct Name {
    /// the value as written
    pub(crate) raw: String,
    /// lower case, for sorting
    lower: String,
    /// folded, for matching
    pub(crate) fold: String,
    /// the artists it stands for (indexes into `artists`)
    pub(crate) groups: Vec<usize>,
}

/// One song as the index holds it.
pub struct Track {
    /// its row id
    pub id: i64,
    /// its title
    pub(crate) title: String,
    /// its title, folded
    pub(crate) title_f: String,
    /// its title, lower case
    title_l: String,
    /// its album, folded
    pub(crate) album_f: String,
    /// its album, lower case
    album_l: String,
    /// its album as written
    pub(crate) album_raw: String,
    /// its artist value (index into `names`)
    pub(crate) artist_ix: usize,
    /// its albumartist value (index into `names`)
    pub(crate) aa_ix: usize,
    /// its disc number
    disc: i64,
    /// its track number
    track: i64,
    /// its year
    year: String,
    /// its length, seconds
    duration: f64,
    /// when it was added, Unix seconds
    added: f64,
    /// the file is lossless
    lossless: bool,
    /// its bitrate, kbps
    bitrate: i64,
}

/// One album as the index holds it.
pub struct Album {
    /// its album id
    pub id: String,
    /// its title
    pub(crate) title: String,
    /// its title, folded
    pub(crate) title_f: String,
    /// `title_f` without "(Deluxe)", "\[Live\]" or " - Remastered" (search::base_of), worked out once per index
    pub(crate) base_f: String,
    /// its albumartist value (index into `names`)
    pub(crate) aa_ix: usize,
    /// the artists it belongs to
    pub(crate) artist_ixs: Vec<usize>,
}

/// The library's artists, names, songs and albums, built from the database for searching and listing.
#[derive(Default)]
pub struct Index {
    /// the artists
    pub artists: Vec<Artist>,
    /// artists by key
    by_key: HashMap<String, usize>,
    /// every albumartist and artist value, one entry each
    pub(crate) names: Vec<Name>, // albumartist and artist values (one entry per distinct value)
    /// every song
    pub(crate) tracks: Vec<Track>,
    /// every album
    pub(crate) albums: Vec<Album>,
    /// artist texts with a credit that fits them (from the files' tags, or MusicBrainz): see `credit_for`
    credit_of: HashMap<String, Credit>,
    /// each song's featured artists that its artist text doesn't name (see `featured_of`): row id → (its
    /// title, the names, each with its MusicBrainz id or "")
    feat_of: HashMap<i64, (String, Credit)>,
}

/// Union-find over artist keys (collaborations and spellings join one artist).
struct Uf(HashMap<String, String>);
impl Uf {
    /// The group `k` is in (with path compression).
    fn find(&mut self, k: &str) -> String {
        let mut cur = k.to_string();
        while let Some(p) = self.0.get(&cur) {
            if *p == cur {
                break;
            }
            cur = p.clone();
        }
        // path compression
        let mut x = k.to_string();
        while let Some(p) = self.0.get(&x).cloned() {
            if p == x {
                break;
            }
            self.0.insert(x, cur.clone());
            x = p;
        }
        cur
    }
    /// Join the groups of `a` and `b` (the smaller key leads).
    fn union(&mut self, a: &str, b: &str) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0.insert(hi, lo);
        }
    }
}

/// What one albumartist value adds up to.
#[derive(Default)]
struct RawAa {
    /// songs
    tracks: i64,
    /// bytes
    size: i64,
    /// the latest added time
    added: f64,
    /// the MusicBrainz id its songs carry
    mbid: String,
    /// its album ids
    albums: BTreeSet<String>,
}

impl Index {
    /// Build the index from the database.
    pub fn build(db: &Db) -> anyhow::Result<Index> {
        // a copy of a song the library has from MusicBrainz is that song (crate::twins): searched and counted once
        let rows = db.query(
            &format!("SELECT id, title, artist, album, albumartist, album_id, disc, track, year, duration, format, bitrate, size, added_at, mb_albumartistid,
                    mb_trackid, mb_albumid, artist_credit, albumartist_credit FROM tracks WHERE {}", crate::twins::SHOWN),
            p![],
        )?;
        let aliases = db.query("SELECT artist_key, alias, alias_key FROM artist_alias", p![])?;
        let mb_credits: HashMap<String, String> = db.query("SELECT mbid, credit FROM mb_credits", p![])?.into_iter().map(|r| (r.s("mbid"), r.s("credit"))).collect();

        // the credits that fit their texts: a song's own tags first, then what MusicBrainz said for its
        // recording (the song's artist) or release (the album artist)
        let mut credit_of: HashMap<String, Credit> = HashMap::new();
        for r in &rows {
            for (text, tag, id) in [("artist", "artist_credit", "mb_trackid"), ("albumartist", "albumartist_credit", "mb_albumid")] {
                let text = r.s(text);
                if text.trim().is_empty() || credit_of.contains_key(&text) {
                    continue;
                }
                let c = credit_for(&text, &r.s(tag)).or_else(|| mb_credits.get(&r.s(id)).and_then(|j| credit_for(&text, j)));
                if let Some(c) = c {
                    credit_of.insert(text, c);
                }
            }
        }
        // the artists credits name beside others (key → name, MusicBrainz id), and every song artist's name
        let mut credited: BTreeMap<String, (String, String)> = BTreeMap::new();
        for c in credit_of.values().filter(|c| c.len() >= 2) {
            for (n, m) in c {
                let k = key(n);
                if !k.is_empty() {
                    let e = credited.entry(k).or_insert_with(|| (n.clone(), m.clone()));
                    if e.1.is_empty() {
                        e.1 = m.clone();
                    }
                }
            }
        }
        let song_keys: HashSet<String> = rows.iter().map(|r| key(&r.s("artist"))).filter(|k| !k.is_empty()).collect();

        // featured artists a song's credit names beyond its artist text (ARTISTS = Coldplay; Selena Gomez under
        // ARTIST = Coldplay): they are artists of the library too, with their MusicBrainz ids
        let mut feat_of: HashMap<i64, (String, Credit)> = HashMap::new();
        for r in &rows {
            let text = r.s("artist");
            if text.trim().is_empty() || credit_of.contains_key(&text) {
                continue; // the credit fits the text: nothing beyond it
            }
            let mut extra_names: Credit = vec![];
            for json in [r.s("artist_credit"), mb_credits.get(&r.s("mb_trackid")).cloned().unwrap_or_default()] {
                extra_names = beyond_text(&text, &json);
                if !extra_names.is_empty() {
                    break;
                }
            }
            for (n, m) in &extra_names {
                let e = credited.entry(key(n)).or_insert_with(|| (n.clone(), m.clone()));
                if e.1.is_empty() {
                    e.1 = m.clone();
                }
            }
            if !extra_names.is_empty() {
                feat_of.insert(r.i("id"), (r.s("title"), extra_names));
            }
        }

        // albumartist values
        let mut raw: BTreeMap<String, RawAa> = BTreeMap::new();
        for r in &rows {
            let aa = r.s("albumartist");
            let e = raw.entry(aa).or_default();
            e.tracks += 1;
            e.size += r.i("size");
            e.added = e.added.max(r.f("added_at"));
            let m = r.s("mb_albumartistid");
            if is_mbid(&m) && m > e.mbid {
                e.mbid = m;
            }
            e.albums.insert(r.s("album_id"));
        }
        let own_keys: HashSet<String> = raw.keys().map(|n| key(n)).filter(|k| !k.is_empty()).collect();
        let mut alias_of: HashMap<String, Vec<(String, String)>> = HashMap::new(); // alias_key → (artist_key, alias)
        for a in &aliases {
            if own_keys.contains(&a.s("artist_key")) && !a.s("alias_key").is_empty() {
                alias_of.entry(a.s("alias_key")).or_default().push((a.s("artist_key"), a.s("alias")));
            }
        }
        let known = |k: &str| own_keys.contains(k) || alias_of.contains_key(k) || credited.contains_key(k) || song_keys.contains(k);

        // featured artists in titles ("Song (feat. Selena Gomez)"), added to what the credits said
        for r in &rows {
            let title = r.s("title");
            let names = title_features(&title, &known);
            if names.is_empty() {
                continue;
            }
            let text = fold(&r.s("artist"));
            let e = feat_of.entry(r.i("id")).or_insert_with(|| (title.clone(), vec![]));
            for n in names {
                // a name the artist text already has ("A feat. B" with "(feat. B)" in the title too) counts once
                let f = fold(&n);
                if f.is_empty() || format!(" {text} ").contains(&format!(" {f} ")) || e.1.iter().any(|(x, _)| key(x) == key(&n)) {
                    continue;
                }
                let m = credited.get(&key(&n)).map(|c| c.1.clone()).unwrap_or_default();
                e.1.push((n, m));
            }
            if e.1.is_empty() {
                feat_of.remove(&r.i("id"));
            }
        }
        // every artist a song's own artist text names beside others ("Coldplay x Selena Gomez", "3R2 · t+pazolite"
        // on a song of an album by someone else): each becomes an artist of the library, like the parts of an album artist
        let song_parts: BTreeSet<String> = rows.iter().map(|r| r.s("artist")).collect::<BTreeSet<_>>().into_iter()
            .filter(|t| credit_of.get(t).is_none_or(|c| c.len() >= 2))
            .flat_map(|t| match credit_of.get(&t) {
                Some(c) => c.iter().map(|x| x.0.clone()).collect(),
                None => {
                    let p = collab_parts(&t, &known);
                    if p.len() >= 2 { p } else { vec![] }
                }
            })
            .chain(feat_of.values().flat_map(|(_, c)| c.iter().map(|x| x.0.clone())))
            .collect();

        // collaborations among the albumartist values: (part names, whether a credit named them)
        let mut collab: HashMap<String, (Vec<String>, bool)> = HashMap::new();
        for name in raw.keys() {
            let parts = match credit_of.get(name) {
                Some(c) if c.len() == 1 => continue, // one artist, by its tags or MusicBrainz
                Some(c) => (c.iter().map(|x| x.0.clone()).collect(), true),
                // the albumartist value itself is known (it is one): `collab_parts` never counts the whole text
                None => (collab_parts(name, &known), false),
            };
            if parts.0.len() >= 2 {
                collab.insert(name.clone(), parts);
            }
        }
        // a part that is no album artist of its own nor an alias becomes an artist of its own (its key → name);
        // a part known only through an alias of a name that turned out to be a collaboration itself couldn't
        // be split into artists, so such a collaboration stays an artist of its own
        let mut extra: BTreeMap<String, String> = BTreeMap::new();
        loop {
            extra.clear();
            for (parts, _) in collab.values() {
                for p in parts {
                    let k = key(p);
                    if !k.is_empty() && !own_keys.contains(&k) && !alias_of.contains_key(&k) {
                        extra.entry(k).or_insert_with(|| credited.get(&key(p)).map(|c| c.0.clone()).unwrap_or_else(|| p.clone()));
                    }
                }
            }
            // the artists songs name beside others, and the ones they feature
            for p in &song_parts {
                let k = key(p);
                if !k.is_empty() && !own_keys.contains(&k) && !alias_of.contains_key(&k) {
                    extra.entry(k).or_insert_with(|| credited.get(&key(p)).map(|c| c.0.clone()).unwrap_or_else(|| p.clone()));
                }
            }
            let nodes: HashSet<String> = raw.keys().filter(|n| !collab.contains_key(*n)).map(|n| key(n)).filter(|k| !k.is_empty()).chain(extra.keys().cloned()).collect();
            let alias_keys: HashSet<&String> = alias_of.iter().filter(|(_, owners)| owners.iter().any(|(o, _)| nodes.contains(o))).map(|(k, _)| k).collect();
            let ok = |k: &str| nodes.contains(k) || alias_keys.contains(&k.to_string());
            let before = collab.len();
            collab.retain(|name, (parts, by_credit)| {
                if parts.iter().all(|p| ok(&key(p))) {
                    return true;
                }
                // a split of its own may still be had from the artists that are left
                match (*by_credit, split(name, &ok)) {
                    (false, Some(p)) => {
                        *parts = p;
                        true
                    }
                    _ => false,
                }
            });
            if collab.len() == before {
                break;
            }
        }
        // everything else is an artist of its own: one node per key, merged by MusicBrainz id and alias
        let mut uf = Uf(HashMap::new());
        let mut node_mbid: HashMap<String, String> = HashMap::new();
        for (name, r) in &raw {
            let k = key(name);
            if k.is_empty() || collab.contains_key(name) {
                continue;
            }
            uf.0.entry(k.clone()).or_insert_with(|| k.clone());
            if !r.mbid.is_empty() {
                node_mbid.entry(k).or_insert_with(|| r.mbid.clone());
            }
        }
        for k in extra.keys() {
            uf.0.entry(k.clone()).or_insert_with(|| k.clone());
            if let Some((_, m)) = credited.get(k).filter(|(_, m)| !m.is_empty()) {
                node_mbid.entry(k.clone()).or_insert_with(|| m.clone());
            }
        }
        let mut by_mbid: HashMap<String, String> = HashMap::new();
        for (k, m) in node_mbid.clone() {
            match by_mbid.get(&m) {
                Some(other) => uf.union(&k, other),
                None => {
                    by_mbid.insert(m, k);
                }
            }
        }
        for (alias_key, owners) in &alias_of {
            if !uf.0.contains_key(alias_key) {
                continue;
            }
            for (artist_key, _) in owners {
                if !uf.0.contains_key(artist_key) {
                    continue;
                }
                // a different MusicBrainz artist that happens to have that name stays apart
                match (node_mbid.get(alias_key), node_mbid.get(artist_key)) {
                    (Some(a), Some(b)) if a != b => {}
                    _ => uf.union(alias_key, artist_key),
                }
            }
        }

        // groups
        let mut root_ix: HashMap<String, usize> = HashMap::new();
        let mut artists: Vec<Artist> = vec![];
        let node_keys: Vec<String> = uf.0.keys().cloned().collect();
        let mut by_key: HashMap<String, usize> = HashMap::new();
        for k in &node_keys {
            let root = uf.find(k);
            let ix = *root_ix.entry(root).or_insert_with(|| {
                artists.push(Artist::default());
                artists.len() - 1
            });
            by_key.insert(k.clone(), ix);
        }
        for (name, r) in &raw {
            if collab.contains_key(name) {
                continue;
            }
            if let Some(&ix) = by_key.get(&key(name)) {
                let a = &mut artists[ix];
                a.spellings.push(name.clone());
                if a.mbid.is_empty() {
                    a.mbid = r.mbid.clone();
                }
            }
        }
        // a credited artist without albums of its own goes by its name in the credit
        for (k, name) in &extra {
            if let Some(&ix) = by_key.get(k) {
                let a = &mut artists[ix];
                if !a.spellings.iter().any(|x| key(x) == *k) {
                    a.spellings.push(name.clone());
                }
                if a.mbid.is_empty() {
                    a.mbid = node_mbid.get(k).cloned().unwrap_or_default();
                }
            }
        }
        for (alias_key, owners) in &alias_of {
            for (artist_key, alias) in owners {
                if let Some(&ix) = by_key.get(artist_key) {
                    by_key.entry(alias_key.clone()).or_insert(ix);
                    let a = &mut artists[ix];
                    if !a.aliases.iter().any(|x| key(x) == *alias_key) {
                        a.aliases.push(alias.clone());
                    }
                }
            }
        }
        // what a name stands for: an artist, an alias, or a collaboration of them (as its credit names them,
        // split into artists of the library, or around "feat.")
        let resolve = |name: &str, by_key: &HashMap<String, usize>| -> Vec<usize> {
            let k = key(name);
            if k.is_empty() {
                return vec![];
            }
            if let Some(&ix) = by_key.get(&k) {
                return vec![ix];
            }
            let mut out = vec![];
            let in_lib = |pk: &str| by_key.contains_key(pk);
            let parts = match collab.get(name) {
                Some((p, _)) => Some(p.clone()),
                None => match credit_of.get(name) {
                    Some(c) if c.len() >= 2 => Some(c.iter().map(|x| x.0.clone()).collect()),
                    Some(_) => None,
                    None => Some(collab_parts(name, &in_lib)),
                },
            };
            if let Some(parts) = parts {
                for p in parts {
                    if let Some(&ix) = by_key.get(&key(&p)) {
                        if !out.contains(&ix) {
                            out.push(ix);
                        }
                    }
                }
            }
            out
        };

        let mut names: Vec<Name> = vec![];
        let mut name_ix: HashMap<String, usize> = HashMap::new();
        let mut intern = |s: &str, names: &mut Vec<Name>| -> usize {
            if let Some(&i) = name_ix.get(s) {
                return i;
            }
            names.push(Name { raw: s.to_string(), lower: s.to_lowercase(), fold: fold(s), groups: resolve(s, &by_key) });
            name_ix.insert(s.to_string(), names.len() - 1);
            names.len() - 1
        };

        let mut album_map: HashMap<String, usize> = HashMap::new();
        let mut albums: Vec<Album> = vec![];
        let mut tracks: Vec<Track> = Vec::with_capacity(rows.len());
        for r in &rows {
            let aa_ix = intern(&r.s("albumartist"), &mut names);
            let artist_ix = intern(&r.s("artist"), &mut names);
            let aid = r.s("album_id");
            let ai = *album_map.entry(aid.clone()).or_insert_with(|| {
                albums.push(Album { id: aid.clone(), title: String::new(), title_f: String::new(), base_f: String::new(), aa_ix, artist_ixs: vec![] });
                albums.len() - 1
            });
            let al = &mut albums[ai];
            if r.s("album") > al.title {
                al.title = r.s("album");
                al.title_f = fold(&al.title);
            }
            if !al.artist_ixs.contains(&artist_ix) && al.artist_ixs.len() < 64 {
                al.artist_ixs.push(artist_ix);
            }
            let title = r.s("title");
            let album = r.s("album");
            tracks.push(Track {
                id: r.i("id"),
                title_f: fold(&title),
                title_l: title.to_lowercase(),
                album_f: fold(&album),
                album_l: album.to_lowercase(),
                album_raw: album,
                title,
                artist_ix,
                aa_ix,
                disc: r.i("disc"),
                track: r.i("track"),
                year: r.s("year"),
                duration: r.f("duration"),
                added: r.f("added_at"),
                lossless: LOSSLESS.contains(&r.s("format").to_lowercase().as_str()),
                bitrate: r.i("bitrate"),
            });
        }

        for al in albums.iter_mut() {
            al.base_f = crate::search::base_of(&al.title, &al.title_f);
        }

        // numbers, display names and the names each artist's songs go by
        let mut album_sets: Vec<BTreeSet<String>> = vec![BTreeSet::new(); artists.len()];
        let mut own_best: Vec<(i64, String)> = vec![(-1, String::new()); artists.len()];
        for (name, r) in &raw {
            let n = &names[name_ix[name.as_str()]];
            let own = !collab.contains_key(name);
            for &g in &n.groups {
                let a = &mut artists[g];
                a.tracks += r.tracks;
                a.size += r.size;
                a.added = a.added.max(r.added);
                album_sets[g].extend(r.albums.iter().cloned());
                if !a.aa_names.contains(name) {
                    a.aa_names.push(name.clone());
                }
                if own {
                    let b = &mut own_best[g];
                    if r.tracks > b.0 || (r.tracks == b.0 && *name < b.1) {
                        *b = (r.tracks, name.clone());
                    }
                    let first = r.albums.iter().next().cloned().unwrap_or_default();
                    if a.cover_album.is_empty() || first < a.cover_album {
                        a.cover_album = first;
                    }
                }
            }
        }
        for n in &names {
            for &g in &n.groups {
                if !artists[g].artist_names.contains(&n.raw) {
                    artists[g].artist_names.push(n.raw.clone());
                }
            }
        }
        // the songs each artist is featured on, and what an artist without albums of its own appears on: the
        // songs whose artist text names it, or that feature it (counted, with their albums, for the Artists grid)
        let mut appears: Vec<(i64, i64, f64, BTreeSet<String>)> = vec![(0, 0, 0.0, BTreeSet::new()); artists.len()];
        for (t, r) in tracks.iter().zip(&rows) {
            let mut gs: Vec<usize> = names[t.artist_ix].groups.clone();
            if let Some((_, c)) = feat_of.get(&t.id) {
                for (n, _) in c {
                    if let Some(&g) = by_key.get(&key(n)) {
                        if !artists[g].feat_tracks.contains(&t.id) {
                            artists[g].feat_tracks.push(t.id);
                        }
                        if !gs.contains(&g) {
                            gs.push(g);
                        }
                    }
                }
            }
            for g in gs {
                let e = &mut appears[g];
                e.0 += 1;
                e.1 += r.i("size");
                e.2 = e.2.max(t.added);
                e.3.insert(r.s("album_id"));
            }
        }
        for (g, a) in artists.iter_mut().enumerate() {
            if a.tracks == 0 {
                // no album of its own: a featured or collaborating artist, counted by the songs it is on
                let (n, size, added, albums) = std::mem::take(&mut appears[g]);
                a.tracks = n;
                a.size = size;
                a.added = added;
                album_sets[g] = albums;
            }
            a.albums = album_sets[g].len() as i64;
            a.name = own_best[g].1.clone();
            if a.name.is_empty() {
                // an artist only on collaborations: its name in the credit, the cover of its first album
                a.name = a.spellings.first().cloned().unwrap_or_default();
                a.cover_album = album_sets[g].iter().next().cloned().unwrap_or_default();
            }
            a.key = key(&a.name);
            a.aliases.retain(|x| key(x) != a.key && !a.spellings.iter().any(|s| key(s) == key(x)));
            a.aliases.sort();
            a.folds = a.spellings.iter().map(|s| (s.clone(), fold(s), false)).chain(a.aliases.iter().map(|s| (s.clone(), fold(s), true))).collect();
        }
        Ok(Index { artists, by_key, names, tracks, albums, credit_of, feat_of })
    }

    /// (title, artist, album, album artist) of every song, in the order of the songs table (the
    /// translator's backlog reads the library from here).
    pub(crate) fn texts(&self) -> impl Iterator<Item = (&str, &str, &str, &str)> {
        self.tracks.iter().map(|t| (t.title.as_str(), self.names[t.artist_ix].raw.as_str(), t.album_raw.as_str(), self.names[t.aa_ix].raw.as_str()))
    }

    /// The artist a name stands for (any spelling or alias); None for unknown names and collaborations.
    pub fn find(&self, name: &str) -> Option<&Artist> {
        self.by_key.get(&key(name)).map(|&i| &self.artists[i])
    }

    /// `artists` for a song: each name in its artist text with the artist's MusicBrainz id when known and
    /// whether it is a library artist (see `credit_names`).
    /// A credit from the song's tags or MusicBrainz names them as it says (and keeps "Simon & Garfunkel" one).
    pub fn credits(&self, artist: &str) -> Value {
        let names: Credit = match self.credit_of.get(artist) {
            Some(c) => c.clone(),
            None => credit_names(artist, &|k: &str| self.by_key.contains_key(k)).into_iter().map(|n| (n, String::new())).collect(),
        };
        Value::Array(names.into_iter().map(|(n, m)| match self.find(&n) {
            Some(a) if !a.mbid.is_empty() => json!({ "name": n, "mbid": a.mbid, "in_library": true }),
            Some(_) if !m.is_empty() => json!({ "name": n, "mbid": m, "in_library": true }),
            Some(_) => json!({ "name": n, "in_library": true }),
            None if !m.is_empty() => json!({ "name": n, "mbid": m, "in_library": false }),
            None => json!({ "name": n, "in_library": false }),
        }).collect())
    }

    /// `featured` for a song: the artists it features that its artist text doesn't name (from its title, its
    /// ARTISTS tag or its MusicBrainz credit), each `{name, mbid?, in_library}` like `credits`. Empty for an
    /// unknown id, or when `title` isn't that song's (a row of another table that happens to share the id).
    pub fn featured(&self, id: i64, title: &str) -> Value {
        let Some((_, names)) = self.feat_of.get(&id).filter(|(t, _)| t == title) else { return json!([]) };
        Value::Array(names.iter().map(|(n, m)| {
            let mbid = self.find(n).map(|a| a.mbid.clone()).filter(|x| !x.is_empty()).unwrap_or_else(|| m.clone());
            let mut v = json!({ "name": n, "in_library": self.find(n).is_some() });
            if !mbid.is_empty() {
                v["mbid"] = json!(mbid);
            }
            v
        }).collect())
    }

    /// An artist as the API lists it.
    pub fn to_json(a: &Artist) -> Value {
        let mut v = json!({
            "name": a.name, "albums": a.albums, "tracks": a.tracks, "mbid": if a.mbid.is_empty() { Value::Null } else { json!(a.mbid) },
            "cover_album": a.cover_album, "size": a.size, "added": a.added,
        });
        if !a.aliases.is_empty() {
            v["aliases"] = json!(a.aliases);
        }
        v
    }

    /// Every artist, sorted like `/api/library/artists` always did (name, albums, tracks, added).
    pub fn list(&self, sort: &str, dir: &str) -> Vec<Value> {
        let mut v: Vec<&Artist> = self.artists.iter().filter(|a| !a.name.is_empty() || a.tracks > 0).collect();
        let (desc, cmp): (bool, ArtistCmp) = match sort {
            "albums" => (dir != "asc", Box::new(|a: &Artist, b: &Artist| a.albums.cmp(&b.albums))),
            "tracks" => (dir != "asc", Box::new(|a: &Artist, b: &Artist| a.tracks.cmp(&b.tracks))),
            "added" => (dir != "asc", Box::new(|a: &Artist, b: &Artist| a.added.partial_cmp(&b.added).unwrap_or(Ordering::Equal))),
            _ => (dir == "desc", Box::new(|a: &Artist, b: &Artist| a.name.to_lowercase().cmp(&b.name.to_lowercase()))),
        };
        v.sort_by(|a, b| {
            let o = cmp(a, b);
            if desc { o.reverse() } else { o }
        });
        v.into_iter().map(Self::to_json).collect()
    }

    /// Each artist's best tier for `q` over its names: (tier, matched by an alias).
    fn artist_tiers(&self, q: &Q) -> Vec<Option<(u8, bool)>> {
        self.artists.iter().map(|a| {
            let mut out: Option<(u8, bool)> = None;
            for (raw, f, alias) in &a.folds {
                if let Some(t) = q.tier(raw, f) {
                    if out.is_none_or(|(o, oa)| t < o || (t == o && oa && !alias)) {
                        out = Some((t, *alias));
                    }
                }
            }
            out
        }).collect()
    }

    /// Tier of each name (albumartist / artist value): its own text or any of its artists' names.
    fn name_tiers(&self, q: &Q, at: &[Option<(u8, bool)>]) -> Vec<Option<u8>> {
        self.names.iter().map(|n| {
            let mut t = q.tier(&n.raw, &n.fold);
            for &g in &n.groups {
                t = best(t, at[g].map(|x| x.0));
            }
            t
        }).collect()
    }

    /// Artists for a search, best first: their names and aliases, then (lowest) their albums' and songs'
    /// titles. Each comes with "match": name | alias | album | song.
    pub fn search_artists(&self, q: &str, limit: usize) -> Vec<Value> {
        self.search_artist_ixs(q).into_iter().take(limit).map(|(g, m)| {
            let mut v = Self::to_json(&self.artists[g]);
            v["match"] = json!(m);
            v
        }).collect()
    }

    /// `search_artists` as artist indexes with their "match", every match (the library search counts them
    /// before it takes a page).
    pub(crate) fn search_artist_ixs(&self, q: &str) -> Vec<(usize, &'static str)> {
        let q = Q::new(q);
        let at = self.artist_tiers(&q);
        // (tier, how well the album or song title matched, "match"): an artist found through an album or a song
        // ranks by its best title, so a song called the query ("Go") puts its artist before one whose song only
        // starts with it ("Gone")
        let mut hit: Vec<Option<(u8, u8, &'static str)>> = at.iter().map(|t| t.map(|(t, alias)| (t, 0, if alias { "alias" } else { "name" }))).collect();
        let better = |hit: &mut Vec<Option<(u8, u8, &'static str)>>, g: usize, tier: u8, sub: u8, m: &'static str| match hit[g] {
            None => hit[g] = Some((tier, sub, m)),
            Some((t, s, _)) if t == tier && sub < s => hit[g] = Some((tier, sub, m)),
            _ => {}
        };
        for al in &self.albums {
            if let Some(sub) = q.tier(&al.title, &al.title_f) {
                for &g in &self.names[al.aa_ix].groups {
                    better(&mut hit, g, 4, sub, "album");
                }
            }
        }
        if hit.iter().any(|h| h.is_none()) {
            for t in &self.tracks {
                if let Some(sub) = q.tier_fast(&t.title, &t.title_f) {
                    for ix in [t.artist_ix, t.aa_ix] {
                        for &g in &self.names[ix].groups {
                            better(&mut hit, g, 5, sub, "song");
                        }
                    }
                }
            }
        }
        let mut found: Vec<(u8, u8, &'static str, usize)> = hit.iter().enumerate().filter_map(|(i, h)| h.map(|(t, s, m)| (t, s, m, i))).collect();
        let a = |i: usize| &self.artists[i];
        found.sort_by(|x, y| (x.0, x.1).cmp(&(y.0, y.1)).then(a(y.3).tracks.cmp(&a(x.3).tracks)).then_with(|| a(x.3).name.to_lowercase().cmp(&a(y.3).name.to_lowercase())));
        found.into_iter().map(|(_, _, m, i)| (i, m)).collect()
    }

    /// Album ids for a search, best first (title, album artist, the songs' artists), with their tier.
    pub fn search_albums(&self, q: &str) -> Vec<(String, u8)> {
        let q = Q::new(q);
        let at = self.artist_tiers(&q);
        let nt = self.name_tiers(&q, &at);
        let mut out: Vec<(String, u8)> = self.albums.iter().filter_map(|al| {
            let mut t = q.tier(&al.title, &al.title_f);
            t = best(t, nt[al.aa_ix]);
            for &ix in &al.artist_ixs {
                t = best(t, nt[ix].map(|x| x.max(1)));
            }
            t.map(|t| (al.id.clone(), t))
        }).collect();
        out.sort_by_key(|x| x.1);
        out
    }

    /// Songs for a search, best first, then in the requested order (the same song twice is still in: see
    /// `dedupe`).
    pub(crate) fn search_track_refs(&self, q: &str, sort: &str, dir: &str) -> Vec<&Track> {
        let q = Q::new(q);
        let at = self.artist_tiers(&q);
        let nt = self.name_tiers(&q, &at);
        let mut hits: Vec<(u8, &Track)> = self.tracks.iter().filter_map(|t| {
            let mut tier = q.tier_fast(&t.title, &t.title_f);
            tier = best(tier, nt[t.artist_ix]);
            tier = best(tier, q.tier_fast(&t.album_raw, &t.album_f).map(|x| x.max(1)));
            tier.map(|x| (x, t))
        }).collect();
        let ord = self.track_order(sort, dir);
        hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| ord(a.1, b.1)));
        hits.into_iter().map(|(_, t)| t).collect()
    }

    /// How `/api/library/tracks` orders songs for `sort` and `dir` (artist, title, album, added, released,
    /// duration; the id breaks ties).
    pub(crate) fn track_order<'s>(&'s self, sort: &'s str, dir: &str) -> impl Fn(&Track, &Track) -> Ordering + 's {
        let artist_l = |t: &Track| self.names[t.artist_ix].lower.as_str();
        let (by, desc_default): (&str, bool) = match sort {
            "title" | "album" => (sort, false),
            "added" | "released" | "duration" => (sort, true),
            _ => ("artist", false),
        };
        let desc = if dir == "asc" { false } else if dir == "desc" { true } else { desc_default };
        move |a: &Track, b: &Track| -> Ordering {
            let first = match by {
                "title" => a.title_l.cmp(&b.title_l),
                "album" => a.album_l.cmp(&b.album_l),
                "added" => a.added.partial_cmp(&b.added).unwrap_or(Ordering::Equal),
                "released" => a.year.cmp(&b.year),
                "duration" => a.duration.partial_cmp(&b.duration).unwrap_or(Ordering::Equal),
                _ => artist_l(a).cmp(artist_l(b)),
            };
            let first = if desc { first.reverse() } else { first };
            first.then_with(|| match by {
                "artist" => a.album_l.cmp(&b.album_l).then(a.disc.cmp(&b.disc)).then(a.track.cmp(&b.track)),
                "album" => a.disc.cmp(&b.disc).then(a.track.cmp(&b.track)),
                "released" => artist_l(a).cmp(artist_l(b)),
                _ => Ordering::Equal,
            }).then(a.id.cmp(&b.id))
        }
    }

    /// The same song twice (same title, artist and album, length within 2 s) only once, in its best
    /// quality, at the place of its first copy.
    pub(crate) fn dedupe<'a>(&'a self, hits: impl IntoIterator<Item = &'a Track>) -> Vec<&'a Track> {
        let mut out: Vec<&Track> = vec![];
        let mut seen: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
        for t in hits {
            let n = &self.names[t.artist_ix];
            let ak = match n.groups.as_slice() {
                [] => key(&n.raw),
                gs => gs.iter().map(|g| g.to_string()).collect::<Vec<_>>().join(","),
            };
            let k = (t.title_f.clone(), ak, strip_the(&t.album_f).to_string());
            let slots = seen.entry(k).or_default();
            if let Some(&slot) = slots.iter().find(|&&s| (out[s].duration - t.duration).abs() <= 2.0) {
                let cur = out[slot];
                let better = (t.lossless, t.bitrate, t.added.to_bits()).cmp(&(cur.lossless, cur.bitrate, cur.added.to_bits())) == Ordering::Greater;
                if better {
                    out[slot] = t;
                }
                continue;
            }
            slots.push(out.len());
            out.push(t);
        }
        out
    }
}

// ---- cache ---------------------------------------------------------------------------------------

/// A built index and when it was last checked.
struct Cached {
    /// when it was last checked against the database
    checked: Instant,
    /// the database's [`fingerprint`] then
    print: String,
    /// the index
    index: Arc<Index>,
    /// `Db::library_version` when it was checked: unchanged means nothing was written since
    version: u64,
}

impl Cached {
    /// Still good without reading the table again: checked within the last second, or nothing written
    /// to the songs or aliases since.
    fn current(&self, db: &Db) -> bool {
        self.checked.elapsed() < Duration::from_secs(1) || self.version == db.library_version()
    }
}

/// One database's index, and the lock its rebuild runs under (so only one request rebuilds it).
#[derive(Default)]
struct Slot {
    /// the index once built
    cached: Option<Cached>,
    /// changed or forgotten since `cached` was made: check again before using it
    dirty: bool,
    /// counts `invalidate` calls, so a rebuild that read the tables before one doesn't count as fresh
    generation: u64,
    /// held by the one rebuild that runs
    build: Arc<Mutex<()>>,
}

/// What the songs, aliases and credits look like now, cheaply (counts and sums): a change moves it.
fn fingerprint(db: &Db) -> String {
    // also the columns the library lists sort and filter on (year, length, size, added, format, tags):
    // their cached answers (routes/library.rs) live as long as this index does
    let t = db.one("SELECT COUNT(*) n, COALESCE(MAX(id),0) m, TOTAL(mtime) t, TOTAL(length(albumartist)) + TOTAL(length(artist)) * 3 + TOTAL(length(title)) * 7 + TOTAL(length(album)) * 11 + TOTAL(length(mb_albumartistid)) * 13 l,
                    TOTAL(length(year)) + TOTAL(length(format)) * 3 + TOTAL(length(mb_albumid)) * 7 + TOTAL(length(genre)) * 11 + TOTAL(length(verify_status)) * 13 + TOTAL(length(album_id)) * 17 l2,
                    TOTAL(length(artist_credit)) + TOTAL(length(albumartist_credit)) * 3 l3,
                    TOTAL(duration) d, TOTAL(size) s, TOTAL(added_at) a, TOTAL(disc * 1000 + track) o FROM tracks", p![]).ok().flatten();
    let a = db.one("SELECT COUNT(*) n, TOTAL(length(alias_key)) l FROM artist_alias", p![]).ok().flatten();
    let c = db.one("SELECT COUNT(*) n, TOTAL(length(credit)) l FROM mb_credits", p![]).ok().flatten();
    let f = |r: &Option<encore_db::Row>| r.as_ref().map(|r| serde_json::to_string(r).unwrap_or_default()).unwrap_or_default();
    format!("{}|{}|{}", f(&t), f(&a), f(&c))
}

/// The artist index of one library: built from its database, rebuilt when the songs or aliases change.
/// The server keeps one (in [`crate::Library`]); clones share it.
#[derive(Clone, Default)]
pub struct Artists(Arc<Mutex<Slot>>);

impl Artists {
    /// Run `f` on the slot, locked.
    fn with_slot<T>(&self, f: impl FnOnce(&mut Slot) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }


    /// The artist index for this database: rebuilt when the tracks or aliases changed (checked at most every
    /// second, and only after a write; `invalidate` forces it). The check and the rebuild run outside the shared lock, by one request
    /// at a time; other requests keep the previous index meanwhile (they only wait when there is none yet).
    pub fn index(&self, db: &Db) -> Arc<Index> {
        let (stale, build) = self.with_slot(|s| {
            if let Some(c) = &s.cached {
                if !s.dirty && c.current(db) {
                    return (Some(c.index.clone()), None);
                }
            }
            (s.cached.as_ref().map(|c| c.index.clone()), Some(s.build.clone()))
        });
        let Some(build) = build else { return stale.unwrap_or_default() };
        let _building = match (&stale, build.try_lock()) {
            (_, Ok(g)) => g,
            (_, Err(std::sync::TryLockError::Poisoned(e))) => e.into_inner(),
            (Some(old), Err(std::sync::TryLockError::WouldBlock)) => return old.clone(),
            (None, Err(std::sync::TryLockError::WouldBlock)) => build.lock().unwrap_or_else(|e| e.into_inner()),
        };
        // another request may have finished a rebuild while this one waited
        let (fresh, old_print, generation) = self.with_slot(|s| match &s.cached {
            Some(c) if !s.dirty && c.current(db) => (Some(c.index.clone()), None, s.generation),
            Some(c) => (None, if s.dirty { None } else { Some((c.print.clone(), c.index.clone())) }, s.generation),
            None => (None, None, s.generation),
        });
        if let Some(ix) = fresh {
            return ix;
        }
        // read before the table: a write while we look makes the next request look again
        let version = db.library_version();
        let print = fingerprint(db);
        if let Some((old, ix)) = old_print {
            if old == print {
                self.with_slot(|s| {
                    if let Some(c) = s.cached.as_mut() {
                        c.checked = Instant::now();
                        c.version = version;
                    }
                });
                return ix;
            }
        }
        let index = Arc::new(Index::build(db).unwrap_or_else(|e| {
            tracing::warn!("artist index: {e}");
            Index::default()
        }));
        self.with_slot(|s| {
            s.cached = Some(Cached { checked: Instant::now(), print, index: index.clone(), version });
            // forgotten again while building: the next request checks once more
            s.dirty = s.generation != generation;
        });
        index
    }

    /// Forget the cached index (after a scan, or anything else that changed the tracks). The old one is still
    /// served to other requests while one of them rebuilds it.
    pub fn invalidate(&self) {
        self.with_slot(|s| {
            s.dirty = true;
            s.generation += 1;
        });
    }

    /// Songs in a JSON answer (objects with a numeric `id`, a `title` and an `artist`) gain `artists`, so
    /// clients can link every artist of a song on its own. The index is only looked at when there is a song.
    pub fn stamp_credits(&self, db: &Db, v: &mut Value) {
        fn walk(v: &mut Value, me: &Artists, db: &Db, idx: &mut Option<Arc<Index>>, seen: &mut HashMap<String, Value>) {
            match v {
                Value::Array(a) => a.iter_mut().for_each(|x| walk(x, me, db, idx, seen)),
                Value::Object(o) => {
                    let song = o.get("id").is_some_and(|x| x.is_i64()) && o.get("title").is_some_and(|x| x.is_string()) && !o.contains_key("artists");
                    if let Some(artist) = o.get("artist").and_then(|x| x.as_str()).filter(|a| song && !a.trim().is_empty()).map(str::to_string) {
                        let c = seen.entry(artist.clone()).or_insert_with(|| idx.get_or_insert_with(|| me.index(db)).credits(&artist)).clone();
                        o.insert("artists".into(), c);
                        // the featured artists its artist text leaves out, by the song's row id (a playlist row
                        // names it `track_id`); only for the song the index has under that id and title
                        let id = o.get("track_id").and_then(|x| x.as_i64()).or_else(|| o.get("id").and_then(|x| x.as_i64())).unwrap_or(0);
                        let title = o.get("title").and_then(|x| x.as_str()).unwrap_or("");
                        let f = idx.get_or_insert_with(|| me.index(db)).featured(id, title);
                        if f.as_array().is_some_and(|a| !a.is_empty()) {
                            o.insert("featured".into(), f);
                        }
                    }
                    o.values_mut().filter(|x| x.is_array() || x.is_object()).for_each(|x| walk(x, me, db, idx, seen));
                }
                _ => {}
            }
        }
        walk(v, self, db, &mut None, &mut HashMap::new());
    }

    /// SQL condition (and its values) for "songs of this artist": every spelling, alias spelling and
    /// collaboration. Unknown names fall back to the name itself.
    pub fn track_filter(&self, db: &Db, name: &str) -> (String, Vec<Value>) {
        let idx = self.index(db);
        if let Some(a) = idx.find(name) {
            let (mut conds, mut params): (Vec<String>, Vec<Value>) = (vec![], vec![]);
            if !a.aa_names.is_empty() {
                conds.push(format!("albumartist IN ({})", marks(a.aa_names.len())));
                params.extend(a.aa_names.iter().map(|n| json!(n)));
            }
            if !a.artist_names.is_empty() {
                conds.push(format!("artist IN ({})", marks(a.artist_names.len())));
                params.extend(a.artist_names.iter().map(|n| json!(n)));
            }
            if !a.feat_tracks.is_empty() {
                // the songs it is featured on (one JSON list: no limit on how many)
                conds.push("id IN (SELECT value FROM json_each(?))".into());
                params.push(json!(json!(a.feat_tracks).to_string()));
            }
            if !conds.is_empty() {
                return (format!("({})", conds.join(" OR ")), params);
            }
        }
        ("(lower(albumartist)=lower(?) OR lower(artist)=lower(?))".into(), p![name, name])
    }

    /// SQL condition for "albums of this artist" (album artist is one of its spellings or collaborations). An
    /// artist with no album of its own (featured, or only part of songs' artist texts) has the albums it appears
    /// on. A name that is no artist of the library (an old link to "A · B", now split) has the albums of its
    /// album artist text, and else the albums of the artists it names, so the page is never empty for no reason.
    pub fn album_filter(&self, db: &Db, name: &str) -> (String, Vec<Value>) {
        let idx = self.index(db);
        match idx.find(name) {
            Some(a) if !a.aa_names.is_empty() => (format!("albumartist IN ({})", marks(a.aa_names.len())), a.aa_names.iter().map(|n| json!(n)).collect()),
            Some(a) => {
                let (cond, params) = self.track_filter(db, &a.name);
                (format!("album_id IN (SELECT album_id FROM tracks WHERE {cond})"), params)
            }
            None => {
                // a collaboration's text: the albums under it (as album artist or a song's artist), else its artists'
                let own = "(lower(albumartist)=lower(?) OR album_id IN (SELECT album_id FROM tracks WHERE lower(artist)=lower(?)))";
                if db.one(&format!("SELECT 1 FROM tracks WHERE {own} LIMIT 1"), p![name, name]).ok().flatten().is_some() {
                    return (own.into(), p![name, name]);
                }
                let parts: Vec<&Artist> = credit_names(name, &|k: &str| idx.by_key.contains_key(k)).iter().filter_map(|n| idx.find(n)).collect();
                let mut aa: Vec<String> = parts.iter().flat_map(|a| a.aa_names.iter().cloned()).collect();
                aa.sort();
                aa.dedup();
                if aa.is_empty() {
                    return ("lower(albumartist)=lower(?)".into(), p![name]);
                }
                (format!("albumartist IN ({})", marks(aa.len())), aa.iter().map(|n| json!(n)).collect())
            }
        }
    }
}

/// `?,?,?` for n values.
pub fn marks(n: usize) -> String {
    vec!["?"; n.max(1)].join(",")
}

// ---- aliases from MusicBrainz ---------------------------------------------------------------------

/// An artist's aliases are looked up again after this long.
const RELOOK_SECS: f64 = 30.0 * 86400.0;
/// A lookup that failed (MusicBrainz or the network down) is tried again after a day.
const RETRY_SECS: f64 = 86400.0;
/// Lookups that fail in a row before a run gives up (the rest wait for the next scan, unmarked).
const MAX_FAILS_IN_A_ROW: usize = 3;

/// The other names MusicBrainz lists for an artist: aliases (and their sort names) and the sort name.
pub fn alias_names(mb: &Value) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut add = |s: &str| {
        let s = s.trim();
        if !s.is_empty() && s.chars().count() <= 200 && !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    };
    if let Some(n) = mb["name"].as_str() {
        add(n);
    }
    if let Some(n) = mb["sort-name"].as_str() {
        add(n);
    }
    for a in mb["aliases"].as_array().into_iter().flatten() {
        if let Some(n) = a["name"].as_str() {
            add(n);
        }
        if let Some(n) = a["sort-name"].as_str() {
            add(n);
        }
    }
    out
}

/// Saves an artist's other names (replacing earlier ones) and refreshes the index (tests set aliases with it).
#[cfg(any(test, feature = "test-support"))]
pub fn store_aliases(artists: &Artists, db: &Db, artist_key: &str, names: &[String]) -> anyhow::Result<()> {
    db.write_now(|c| c.transaction(|c| save_aliases(c, artist_key, names)))?;
    artists.invalidate();
    Ok(())
}

/// Saves an artist's other names (replacing earlier ones) on the write connection `c`; true when they
/// changed. The index isn't touched: `refresh_aliases` refreshes it once at the end.
fn save_aliases(c: &Conn, artist_key: &str, names: &[String]) -> rusqlite::Result<bool> {
    let mut want: Vec<(String, String)> = vec![];
    for n in names {
        let k = key(n);
        if !k.is_empty() && k != artist_key && !want.iter().any(|(_, x)| *x == k) {
            want.push((n.clone(), k));
        }
    }
    let mut have: Vec<(String, String)> = c.query("SELECT alias, alias_key FROM artist_alias WHERE artist_key=?", p![artist_key])?.iter().map(|r| (r.s("alias"), r.s("alias_key"))).collect();
    let mut sorted = want.clone();
    sorted.sort();
    have.sort();
    if sorted == have {
        return Ok(false);
    }
    c.execute("DELETE FROM artist_alias WHERE artist_key=?", p![artist_key])?;
    for (n, k) in &want {
        c.execute("INSERT OR IGNORE INTO artist_alias (artist_key, alias, alias_key) VALUES (?,?,?)", p![artist_key, n, k])?;
    }
    Ok(true)
}

/// One artist's alias look-up: its key, its MusicBrainz id, the names MusicBrainz gave (None: the look-up
/// failed) and when it was asked.
type Looked = (String, String, Option<Vec<String>>, f64);

/// Artists whose look-ups are saved together, in one transaction: the aliases are part of the library
/// (`Db::library_version`), so each save makes the cached lists and the artist index be built again; once
/// per batch (about a minute of look-ups at MusicBrainz's pace) instead of once per artist.
const ALIAS_BATCH: usize = 50;

/// Saves a batch of look-ups (the aliases of those that answered, and when each was asked) in one
/// transaction on the blocking pool; true when any artist's aliases changed.
async fn save_looked(db: &Db, batch: Vec<Looked>) -> bool {
    if batch.is_empty() {
        return false;
    }
    let saved = db.write(move |c| c.transaction(|c| {
        let mut changed = false;
        for (k, mbid, names, at) in &batch {
            // a save that fails counts as a failed look-up (tried again after a day), as before
            let ok = match names {
                Some(names) => match save_aliases(c, k, names) {
                    Ok(ch) => {
                        changed |= ch;
                        true
                    }
                    Err(e) => {
                        tracing::debug!("aliases of {k}: {e}");
                        false
                    }
                },
                None => false,
            };
            let _ = c.execute("INSERT OR REPLACE INTO artist_alias_lookups (artist_key, mbid, looked_up_at, ok) VALUES (?,?,?,?)", p![k, mbid, at, ok as i64]);
        }
        Ok::<_, rusqlite::Error>(changed)
    })).await;
    saved.unwrap_or_else(|e| {
        tracing::warn!("artist aliases: could not save {e}");
        false
    })
}

/// The artists among `artists` (key, MusicBrainz id) whose aliases are due: never looked up, looked up more
/// than 30 days ago, or failed more than a day ago. One query per 500 artists.
fn due_for_aliases(db: &Db, artists: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut last: HashMap<(String, String), (f64, i64)> = HashMap::new();
    for chunk in artists.chunks(500) {
        let keys: Vec<Value> = chunk.iter().map(|(k, _)| json!(k)).collect();
        let rows = db.query(&format!("SELECT artist_key, mbid, looked_up_at, ok FROM artist_alias_lookups WHERE artist_key IN ({})", marks(keys.len())), keys).unwrap_or_default();
        for r in rows {
            last.insert((r.s("artist_key"), r.s("mbid")), (r.f("looked_up_at"), r.i("ok")));
        }
    }
    let now = now();
    artists.into_iter().filter(|a| last.get(a).is_none_or(|(at, ok)| now - at > if *ok == 1 { RELOOK_SECS } else { RETRY_SECS })).collect()
}

/// Looks up aliases for library artists with a MusicBrainz id that weren't looked up in 30 days (a
/// failed lookup is tried again after a day; a run stops after a few failures in a row, so an outage
/// doesn't mark every artist). The answers are saved [`ALIAS_BATCH`] artists at a time.
pub async fn refresh_aliases(st: &impl Ctx) {
    // the artist index and the last look-ups are read on the blocking pool
    let (artists, db) = (st.library().artists.clone(), st.db().clone());
    let due = tokio::task::spawn_blocking(move || {
        let idx = artists.index(&db);
        due_for_aliases(&db, idx.artists.iter().filter(|a| is_mbid(&a.mbid) && !a.key.is_empty()).map(|a| (a.key.clone(), a.mbid.clone())).collect())
    }).await.unwrap_or_default();
    let (mut changed, mut fails) = (false, 0);
    let mut batch: Vec<Looked> = vec![];
    for (k, mbid) in due {
        let res = st.mb().get(st, &format!("artist/{mbid}"), &[("inc", "aliases".into())]).await;
        let names = match res {
            Ok(v) => {
                fails = 0;
                Some(alias_names(&v))
            }
            Err(e) => {
                tracing::debug!("aliases for {mbid}: {e}");
                fails += 1;
                None
            }
        };
        batch.push((k, mbid, names, now()));
        if batch.len() >= ALIAS_BATCH {
            changed |= save_looked(st.db(), std::mem::take(&mut batch)).await;
        }
        if fails >= MAX_FAILS_IN_A_ROW {
            tracing::info!("artist aliases: MusicBrainz isn't answering, trying again after the next scan");
            break;
        }
    }
    changed |= save_looked(st.db(), batch).await;
    if changed {
        st.library().artists.invalidate();
    }
}

// ---- artist credits from MusicBrainz --------------------------------------------------------------

/// Releases whose credits are saved together, in one transaction (the credits are part of the library).
const CREDIT_BATCH: usize = 25;

/// Whether an artist text may name several artists ("A & B", "A feat. B", "A, B").
fn may_be_several(s: &str) -> bool {
    segments(s.trim()).len() >= 2
}

/// The MusicBrainz releases in the library whose credits are due: a song has no ARTISTS tag, or its album artist
/// reads like several artists while the file has no ALBUMARTISTS tag, and the release wasn't looked up yet (a failed
/// look-up is tried again after a day). Files tagged by Encore before 0.36 are the usual case.
fn due_for_credits(db: &Db) -> Vec<String> {
    let rows = db.query("SELECT DISTINCT mb_albumid id, artist, albumartist, artist_credit, albumartist_credit FROM tracks
                         WHERE length(mb_albumid)=36 AND (artist_credit='[]' OR albumartist_credit='[]')", p![]).unwrap_or_default();
    let last: HashMap<String, (f64, i64)> = db.query("SELECT release_id, looked_up_at, ok FROM mb_credit_lookups", p![]).unwrap_or_default()
        .into_iter().map(|r| (r.s("release_id"), (r.f("looked_up_at"), r.i("ok")))).collect();
    let now = now();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for r in rows {
        // a song without an ARTISTS tag is looked up even when its text names one artist: MusicBrainz may credit
        // a featured artist the text leaves out ("Coldplay" for "Coldplay x Selena Gomez"; see `beyond_text`)
        let several = r.s("artist_credit") == "[]" || (r.s("albumartist_credit") == "[]" && may_be_several(&r.s("albumartist")));
        let id = r.s("id");
        if several && is_mbid(&id) && last.get(&id).is_none_or(|(at, ok)| *ok == 0 && now - at > RETRY_SECS) {
            out.insert(id);
        }
    }
    out.into_iter().collect()
}

/// One release's look-up: its id and the credits MusicBrainz gave (the release's own, and each recording's),
/// as (mbid, credit JSON); None when the look-up failed.
type CreditsLooked = (String, Option<Vec<(String, String)>>);

/// The credits of one release as `mb::release` gives them: the release's under its id, each song's under its recording id.
pub(crate) fn release_credits(rel: &Value) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut add = |id: &str, c: &Value| {
        if is_mbid(id) && c.as_array().is_some_and(|a| !a.is_empty()) {
            out.push((id.to_string(), c.to_string()));
        }
    };
    add(rel["id"].as_str().unwrap_or(""), &rel["artists"]);
    for t in rel["tracks"].as_array().into_iter().flatten() {
        add(t["recording_id"].as_str().unwrap_or(""), &t["artists"]);
    }
    out
}

/// Save looked-up credits and mark their releases looked up (in one transaction). True when a credit changed.
async fn save_credits(db: &Db, batch: Vec<CreditsLooked>) -> bool {
    if batch.is_empty() {
        return false;
    }
    let at = now();
    let saved = db.write(move |c| c.transaction(|c| {
        let mut changed = false;
        for (rid, credits) in &batch {
            for (id, credit) in credits.iter().flatten() {
                if c.one("SELECT credit FROM mb_credits WHERE mbid=?", p![id])?.is_none_or(|r| r.s("credit") != *credit) {
                    c.execute("INSERT OR REPLACE INTO mb_credits (mbid, credit) VALUES (?,?)", p![id, credit])?;
                    changed = true;
                }
            }
            c.execute("INSERT OR REPLACE INTO mb_credit_lookups (release_id, looked_up_at, ok) VALUES (?,?,?)", p![rid, at, credits.is_some() as i64])?;
        }
        Ok::<_, rusqlite::Error>(changed)
    })).await;
    saved.unwrap_or_else(|e| {
        tracing::warn!("artist credits: could not save {e}");
        false
    })
}

/// Looks up the MusicBrainz artist credits of releases whose files have only the joined artist text ("A & B"),
/// so the library can tell their artists apart without touching the files. A run stops after a few failures in
/// a row (an outage doesn't mark every release).
pub async fn refresh_credits(st: &impl Ctx) {
    let db = st.db().clone();
    let due = tokio::task::spawn_blocking(move || due_for_credits(&db)).await.unwrap_or_default();
    let (mut changed, mut fails) = (false, 0);
    let mut batch: Vec<CreditsLooked> = vec![];
    for rid in due {
        let credits = match encore_http::mb::release(st, &rid).await {
            Ok(rel) => {
                fails = 0;
                Some(release_credits(&rel))
            }
            Err(e) => {
                tracing::debug!("artist credits for {rid}: {e}");
                fails += 1;
                None
            }
        };
        batch.push((rid, credits));
        if batch.len() >= CREDIT_BATCH {
            changed |= save_credits(st.db(), std::mem::take(&mut batch)).await;
        }
        if fails >= MAX_FAILS_IN_A_ROW {
            tracing::info!("artist credits: MusicBrainz isn't answering, trying again after the next scan");
            break;
        }
    }
    changed |= save_credits(st.db(), batch).await;
    if changed {
        st.library().artists.invalidate();
    }
}

/// Runs `refresh_aliases` and `refresh_credits` after every library scan (including the one at start).
pub async fn alias_worker(st: impl Ctx) {
    let mut done: Option<f64> = None;
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let (scanning, last) = {
            let s = st.library().scan.lock().unwrap();
            (s.scanning, s.last_scan)
        };
        if scanning || last.is_none() || last == done {
            continue;
        }
        done = last;
        st.library().artists.invalidate();
        refresh_aliases(&st).await;
        refresh_credits(&st).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_folds() {
        assert_eq!(key("The Black Eyed Peas"), "black eyed peas");
        assert_eq!(key("Black Eyed Peas"), "black eyed peas");
        assert_eq!(key("Simon and Garfunkel"), key("Simon & Garfunkel"));
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(key("  AC/DC  "), "ac dc");
        assert_eq!(key("The The"), "the");
        assert_eq!(key("!!!"), "#!!!");
        assert_eq!(key("  "), "");
    }

    #[test]
    fn splits_only_when_every_part_is_known() {
        let known: HashSet<String> = ["ye", "xxxtentacion", "earth wind and fire", "paul simon"].iter().map(|s| s.to_string()).collect();
        let k = |s: &str| known.contains(s);
        assert_eq!(split("Ye & XXXTENTACION", &k), Some(vec!["Ye".to_string(), "XXXTENTACION".to_string()]));
        assert_eq!(split("Simon & Garfunkel", &k), None);
        assert_eq!(split("Earth, Wind & Fire", &k), None);
        assert_eq!(split("Earth, Wind & Fire feat. Ye", &k), Some(vec!["Earth, Wind & Fire".to_string(), "Ye".to_string()]));
        assert_eq!(split("Ye", &k), None);
    }

    #[test]
    fn credit_names_split_feat_always_and_collaborations_when_known() {
        let known: HashSet<String> = ["ye", "xxxtentacion", "travis scott", "earth wind and fire"].iter().map(|s| s.to_string()).collect();
        let k = |s: &str| known.contains(s);
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(credit_names("Travis Scott feat. The Weeknd", &k), v(&["Travis Scott", "The Weeknd"]));
        assert_eq!(credit_names("Ye & XXXTENTACION", &k), v(&["Ye", "XXXTENTACION"]));
        assert_eq!(credit_names("Simon & Garfunkel", &k), v(&["Simon & Garfunkel"]));
        assert_eq!(credit_names("Earth, Wind & Fire", &k), v(&["Earth, Wind & Fire"]));
        assert_eq!(credit_names("A & B ft. Ye & XXXTENTACION", &k), v(&["A & B", "Ye", "XXXTENTACION"]));
        assert_eq!(credit_names("  ", &k), Vec::<String>::new());
        // a library artist with " ft " in its own name stays one link
        let known2: HashSet<String> = [key("Left ft Right")].into_iter().collect();
        assert_eq!(credit_names("Left ft Right", &|s: &str| known2.contains(s)), v(&["Left ft Right"]));
    }

    #[test]
    fn tiers() {
        let q = Q::new("ye");
        assert_eq!(q.tier("Ye", &fold("Ye")), Some(0));
        assert_eq!(q.tier("Years & Years", &fold("Years & Years")), Some(1));
        assert_eq!(q.tier("Elton John & Years & Years", &fold("Elton John & Years & Years")), Some(2));
        assert_eq!(q.tier("TAEYEON", &fold("TAEYEON")), Some(3));
        assert_eq!(Q::new("beyonce").tier("Beyoncé", &fold("Beyoncé")), Some(0));
        assert_eq!(Q::new("the black eyed peas").tier("Black Eyed Peas", &fold("Black Eyed Peas")), Some(0));
        assert_eq!(Q::new("acdc").tier("AC/DC", &fold("AC/DC")), Some(3));
    }

    /// Which artists' aliases are looked up: never looked up, looked up more than 30 days ago, or failed more
    /// than a day ago, and another MusicBrainz id under the same key counts as never (one query per 500).
    #[test]
    fn aliases_are_due_by_their_last_lookup() {
        let dir = encore_core::tmp::dir("alias-due");
        let db = Db::open(&dir).unwrap();
        let t = now();
        for (key, mbid, at, ok) in [("fresh", "m1", t - 10.0, 1), ("old", "m2", t - RELOOK_SECS - 10.0, 1), ("failed_now", "m3", t - 10.0, 0),
                                    ("failed_long_ago", "m4", t - RETRY_SECS - 10.0, 0), ("other_id", "m-old", t - 10.0, 1)] {
            db.execute("INSERT INTO artist_alias_lookups (artist_key, mbid, looked_up_at, ok) VALUES (?,?,?,?)", p![key, mbid, at, ok]).unwrap();
        }
        let mut asked: Vec<(String, String)> = [("fresh", "m1"), ("old", "m2"), ("failed_now", "m3"), ("failed_long_ago", "m4"), ("other_id", "m5"), ("never", "m6")]
            .iter().map(|(k, m)| (k.to_string(), m.to_string())).collect();
        // more than one query's worth
        asked.extend((0..1200).map(|i| (format!("k{i}"), format!("id{i}"))));
        let due = due_for_aliases(&db, asked);
        let named: Vec<&str> = due.iter().map(|(k, _)| k.as_str()).filter(|k| !k.starts_with('k')).collect();
        assert_eq!(named, vec!["old", "failed_long_ago", "other_id", "never"]);
        assert_eq!(due.len(), 4 + 1200);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A batch of look-ups is saved in one transaction: the library version moves once, the aliases of those
    /// that answered are saved, and every look-up is noted (failed ones as failed).
    #[tokio::test]
    async fn a_batch_of_lookups_moves_the_library_once() {
        let dir = encore_core::tmp::dir("alias-batch");
        let db = Db::open(&dir).unwrap();
        let v0 = db.library_version();
        let t = now();
        let batch: Vec<Looked> = vec![
            ("ye".into(), "m1".into(), Some(vec!["Ye".into(), "Kanye West".into()]), t),
            ("bjork".into(), "m2".into(), Some(vec!["Bjork Gudmundsdottir".into()]), t),
            ("down".into(), "m3".into(), None, t),
        ];
        assert!(save_looked(&db, batch).await);
        assert_eq!(db.library_version(), v0 + 1, "one move for the batch");
        let aliases: Vec<String> = db.query("SELECT alias_key FROM artist_alias ORDER BY alias_key", p![]).unwrap().iter().map(|r| r.s("alias_key")).collect();
        assert_eq!(aliases, vec!["bjork gudmundsdottir", "kanye west"]);
        let looked: Vec<(String, i64)> = db.query("SELECT artist_key, ok FROM artist_alias_lookups ORDER BY artist_key", p![]).unwrap().iter().map(|r| (r.s("artist_key"), r.i("ok"))).collect();
        assert_eq!(looked, vec![("bjork".to_string(), 1), ("down".to_string(), 0), ("ye".to_string(), 1)]);
        // the same names again: nothing changed
        let again: Vec<Looked> = vec![("ye".into(), "m1".into(), Some(vec!["Kanye West".into()]), t)];
        assert!(!save_looked(&db, again).await);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Songs of a test library: (artist, albumartist, album, artist_credit, albumartist_credit, mb_trackid, mb_albumid).
    fn library(name: &str, songs: &[(&str, &str, &str, &str, &str, &str, &str)]) -> (std::path::PathBuf, Db) {
        let dir = encore_core::tmp::dir(name);
        let db = Db::open(&dir).unwrap();
        for (i, (artist, aa, album, ac, aac, rec, rel)) in songs.iter().enumerate() {
            db.execute("INSERT INTO tracks (path, album_id, title, artist, albumartist, album, artist_credit, albumartist_credit, mb_trackid, mb_albumid, size, added_at)
                        VALUES (?,?,?,?,?,?,?,?,?,?,1,1)", p![format!("/m/{i}.flac"), crate::scan::album_key(aa, album, ""), format!("Song {i}"), artist, aa, album, ac, aac, rec, rel]).unwrap();
        }
        (dir, db)
    }

    fn names_of(ix: &Index) -> Vec<String> {
        let mut v: Vec<String> = ix.artists.iter().filter(|a| a.tracks > 0).map(|a| format!("{} ({})", a.name, a.albums)).collect();
        v.sort();
        v
    }

    #[test]
    fn credits_from_tags_split_collaborations_and_keep_bands_whole() {
        let two = json!([{ "name": "Kanye West" }, { "name": "Ty Dolla $ign" }]).to_string();
        let sg = json!([{ "name": "Simon & Garfunkel" }]).to_string();
        let (dir, db) = library("credits-tags", &[
            ("Kanye West & Ty Dolla $ign", "Kanye West & Ty Dolla $ign", "Vultures 1", &two, &two, "", ""),
            ("Kanye West", "Kanye West", "Graduation", "[]", "[]", "", ""),
            // one credited artist: never split, even with both halves in the library
            ("Simon & Garfunkel", "Simon & Garfunkel", "Bookends", &sg, &sg, "", ""),
            ("Simon", "Simon", "Solo", "[]", "[]", "", ""),
            ("Garfunkel", "Garfunkel", "Solo 2", "[]", "[]", "", ""),
        ]);
        let ix = Index::build(&db).unwrap();
        assert_eq!(names_of(&ix), vec!["Garfunkel (1)", "Kanye West (2)", "Simon & Garfunkel (1)", "Simon (1)", "Ty Dolla $ign (1)"]);
        assert!(ix.find("Kanye West & Ty Dolla $ign").is_none(), "not an artist of its own");
        let c = ix.credits("Kanye West & Ty Dolla $ign");
        assert_eq!(c, json!([{ "name": "Kanye West", "in_library": true }, { "name": "Ty Dolla $ign", "in_library": true }]));
        assert_eq!(ix.credits("Simon & Garfunkel"), json!([{ "name": "Simon & Garfunkel", "in_library": true }]));
        // songs of Ty Dolla $ign: the collaboration's album
        assert_eq!(ix.find("ty dolla $ign").unwrap().aa_names, vec!["Kanye West & Ty Dolla $ign".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untagged_collaborations_split_by_song_artists_and_feat() {
        let (dir, db) = library("credits-fallback", &[
            // the guest is a song artist elsewhere (a compilation): a part of its own
            ("Drake & Future", "Drake & Future", "What a Time to Be Alive", "[]", "[]", "", ""),
            ("Drake", "Drake", "Views", "[]", "[]", "", ""),
            ("Future", "Various Artists", "Hits", "[]", "[]", "", ""),
            // "feat." always splits, known or not
            ("Travis Scott feat. The Weeknd", "Travis Scott feat. The Weeknd", "Single", "[]", "[]", "", ""),
            // nobody knows either half: stays whole
            ("Hall & Oates", "Hall & Oates", "H2O", "[]", "[]", "", ""),
            // not read yet (NULL): the text alone
            ("Earth, Wind & Fire", "Earth, Wind & Fire", "Gratitude", "", "", "", ""),
        ]);
        let ix = Index::build(&db).unwrap();
        assert_eq!(names_of(&ix), vec!["Drake (2)", "Earth, Wind & Fire (1)", "Future (1)", "Hall & Oates (1)", "The Weeknd (1)", "Travis Scott (1)", "Various Artists (1)"]);
        assert_eq!(ix.credits("Travis Scott feat. The Weeknd"), json!([{ "name": "Travis Scott", "in_library": true }, { "name": "The Weeknd", "in_library": true }]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn musicbrainz_credits_apply_to_untagged_songs_and_bad_credits_are_left_out() {
        let (rec, rel) = ("11111111-1111-1111-1111-111111111111", "22222222-2222-2222-2222-222222222222");
        let m = "33333333-3333-3333-3333-333333333333";
        let (dir, db) = library("credits-mb", &[
            ("Silk Sonic & Friend", "Bruno Mars & Anderson .Paak", "An Evening", "[]", "[]", rec, rel),
            // a tag that doesn't fit its text is ignored
            ("Someone", "Someone", "Other", &json!([{ "name": "Somebody Else" }, { "name": "X" }]).to_string(), "[]", "", ""),
        ]);
        db.execute("INSERT INTO mb_credits (mbid, credit) VALUES (?,?)", p![rel, json!([{ "name": "Bruno Mars" }, { "name": "Anderson .Paak", "mbid": m }]).to_string()]).unwrap();
        db.execute("INSERT INTO mb_credits (mbid, credit) VALUES (?,?)", p![rec, json!([{ "name": "Silk Sonic" }, { "name": "Friend" }]).to_string()]).unwrap();
        let ix = Index::build(&db).unwrap();
        // the song's own credit names artists too: each is an artist of the library, with the song
        assert_eq!(names_of(&ix), vec!["Anderson .Paak (1)", "Bruno Mars (1)", "Friend (1)", "Silk Sonic (1)", "Someone (1)"]);
        assert_eq!(ix.find("Anderson .Paak").unwrap().mbid, m);
        assert_eq!(ix.credits("Silk Sonic & Friend"), json!([{ "name": "Silk Sonic", "in_library": true }, { "name": "Friend", "in_library": true }]));
        assert_eq!(ix.credits("Someone"), json!([{ "name": "Someone", "in_library": true }]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only releases whose files have no credit tags are looked up (a featured artist may hide behind one name),
    /// once (a failed look-up again after a day); the answers become the songs' credits.
    #[tokio::test]
    async fn musicbrainz_credits_are_looked_up_for_joined_names_only() {
        let (a, b, c, d) = ("aaaaaaaa-1111-1111-1111-111111111111", "bbbbbbbb-1111-1111-1111-111111111111",
                            "cccccccc-1111-1111-1111-111111111111", "dddddddd-1111-1111-1111-111111111111");
        let tagged = json!([{ "name": "X" }, { "name": "Y" }]).to_string();
        let (dir, db) = library("credits-due", &[
            ("X & Y", "X & Y", "Joined", "[]", "[]", "", a),
            ("Solo", "Solo", "One", "[]", "[]", "", b),
            ("X & Y", "X & Y", "Tagged", &tagged, &tagged, "", c),
            ("P feat. Q", "P", "Feature", "[]", "[]", "", d),
        ]);
        db.execute("INSERT INTO mb_credit_lookups (release_id, looked_up_at, ok) VALUES (?,?,0)", p![d, now() - 10.0]).unwrap();
        assert_eq!(due_for_credits(&db), vec![a.to_string(), b.to_string()]);
        let rec = "eeeeeeee-1111-1111-1111-111111111111";
        let rel = json!({ "id": a, "artists": [{ "name": "X", "mbid": "" }, { "name": "Y", "mbid": "" }],
                          "tracks": [{ "recording_id": rec, "artists": [{ "name": "X" }, { "name": "Y" }] }, { "recording_id": null, "artists": [] }] });
        let credits = release_credits(&rel);
        assert_eq!(credits.len(), 2);
        assert!(save_credits(&db, vec![(a.to_string(), Some(credits.clone())), (b.to_string(), Some(vec![])), (d.to_string(), None)]).await);
        assert!(!save_credits(&db, vec![(a.to_string(), Some(credits))]).await, "the same credits again: nothing changed");
        assert!(due_for_credits(&db).is_empty());
        let ix = Index::build(&db).unwrap();
        // "P feat. Q" on P's album: Q is an artist of the library too, with the one song
        assert_eq!(names_of(&ix), vec!["P (1)", "Q (1)", "Solo (1)", "X (2)", "Y (2)"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// " · ", "; " and " / " always part artists; "&", "x" and "," when one side is known, or a list of three
    /// or more names joined by commas; bands no side of which is known stay whole.
    #[test]
    fn collab_parts_split_hard_joins_and_known_halves() {
        let known: HashSet<String> = ["t pazolite", "coldplay", "tyler the creator", "ye", "earth wind and fire"].iter().map(|s| s.to_string()).collect();
        let k = |s: &str| known.contains(s);
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(collab_parts("20syl · Lord Apex", &k), v(&["20syl", "Lord Apex"]));
        assert_eq!(collab_parts("3R2 · t+pazolite", &k), v(&["3R2", "t+pazolite"]));
        assert_eq!(collab_parts("3R2 & t+pazolite", &k), v(&["3R2", "t+pazolite"]));
        assert_eq!(collab_parts("Coldplay x Selena Gomez", &k), v(&["Coldplay", "Selena Gomez"]));
        assert_eq!(collab_parts("Coldplay × Selena Gomez", &k), v(&["Coldplay", "Selena Gomez"]));
        assert_eq!(collab_parts("111nightshift, c4rp3tg4r, Moe Shop", &k), v(&["111nightshift", "c4rp3tg4r", "Moe Shop"]));
        assert_eq!(collab_parts("Tyler, The Creator & Ye", &k), v(&["Tyler, The Creator", "Ye"]));
        assert_eq!(collab_parts("A; B", &k), v(&["A", "B"]));
        assert_eq!(collab_parts("A / B", &k), v(&["A", "B"]));
        // nobody known, two names: one artist
        assert_eq!(collab_parts("Simon & Garfunkel", &k), v(&["Simon & Garfunkel"]));
        assert_eq!(collab_parts("25:00, Night Code for.", &k), v(&["25:00, Night Code for."]));
        assert_eq!(collab_parts("Earth, Wind & Fire", &k), v(&["Earth, Wind & Fire"]));
        // the whole text being known is no reason to split it, nor to keep a hard join together
        let known2: HashSet<String> = [key("20syl · Lord Apex"), key("Florence + the Machine")].into_iter().collect();
        let k2 = |s: &str| known2.contains(s);
        assert_eq!(collab_parts("20syl · Lord Apex", &k2), v(&["20syl", "Lord Apex"]));
        assert_eq!(collab_parts("Florence + the Machine", &k2), v(&["Florence + the Machine"]));
        assert_eq!(credit_names("Florence + the Machine", &k2), v(&["Florence + the Machine"]));
    }

    /// Featured artists in titles, and in credits that name more artists than the text.
    #[test]
    fn features_from_titles_and_credits() {
        let known: HashSet<String> = ["selena gomez", "justin bieber"].iter().map(|s| s.to_string()).collect();
        let k = |s: &str| known.contains(s);
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(title_features("Let Somebody Go (feat. Selena Gomez)", &k), v(&["Selena Gomez"]));
        assert_eq!(title_features("Song [ft. A & Selena Gomez]", &k), v(&["A", "Selena Gomez"]));
        assert_eq!(title_features("Song feat. Someone New", &k), v(&["Someone New"]));
        assert_eq!(title_features("Stay (with Justin Bieber)", &k), v(&["Justin Bieber"]));
        // "(With Love)" names nobody the library knows
        assert!(title_features("Yours (With Love)", &k).is_empty());
        assert!(title_features("Plain Song (Live)", &k).is_empty());
        let c = json!([{ "name": "Coldplay" }, { "name": "Selena Gomez", "mbid": "33333333-3333-3333-3333-333333333333" }]).to_string();
        assert_eq!(beyond_text("Coldplay", &c), vec![("Selena Gomez".to_string(), "33333333-3333-3333-3333-333333333333".to_string())]);
        // a credit about somebody else adds nobody
        assert!(beyond_text("Muse", &c).is_empty());
        assert!(beyond_text("Coldplay & Selena Gomez", &c).is_empty());
    }

    /// A featured artist is an artist of the library: listed with the songs it is on, its songs and albums
    /// found by its name, and the song says whom it features. Combined pseudo-artists are gone, and an old link
    /// to one still finds its album.
    #[test]
    fn featured_artists_get_their_own_page() {
        let c = json!([{ "name": "Coldplay" }, { "name": "Selena Gomez" }]).to_string();
        let (dir, db) = library("featured", &[
            ("Coldplay", "Coldplay", "Music of the Spheres", &c, "[]", "", ""),
            ("Coldplay", "Coldplay", "Parachutes", "[]", "[]", "", ""),
            ("3R2 · t+pazolite", "3R2 · t+pazolite", "I Wanna", "[]", "[]", "", ""),
            ("t+pazolite", "t+pazolite", "Solo", "[]", "[]", "", ""),
        ]);
        db.execute("UPDATE tracks SET title='Fix You (feat. Chris Martin Jr)' WHERE album='Parachutes'", p![]).unwrap();
        let ix = Index::build(&db).unwrap();
        assert_eq!(names_of(&ix), vec!["3R2 (1)", "Chris Martin Jr (1)", "Coldplay (2)", "Selena Gomez (1)", "t+pazolite (2)"]);
        let sg = ix.find("Selena Gomez").unwrap();
        assert_eq!((sg.tracks, sg.feat_tracks.len()), (1, 1));
        let id = db.one("SELECT id FROM tracks WHERE album='Music of the Spheres'", p![]).unwrap().unwrap().i("id");
        assert_eq!(ix.featured(id, "Song 0"), json!([{ "name": "Selena Gomez", "in_library": true }]));
        assert_eq!(ix.featured(id, "Another title"), json!([]));
        assert_eq!(ix.credits("3R2 · t+pazolite"), json!([{ "name": "3R2", "in_library": true }, { "name": "t+pazolite", "in_library": true }]));

        let arts = Artists::default();
        let count = |(cond, ps): (String, Vec<Value>)| db.one(&format!("SELECT COUNT(DISTINCT album_id) n FROM tracks WHERE {cond}"), ps).unwrap().unwrap().i("n");
        assert_eq!(count(arts.album_filter(&db, "Selena Gomez")), 1);
        assert_eq!(count(arts.track_filter(&db, "Selena Gomez")), 1);
        assert_eq!(count(arts.album_filter(&db, "3R2")), 1);
        assert_eq!(count(arts.album_filter(&db, "t+pazolite")), 2);
        // a link to the old combined artist
        assert_eq!(count(arts.album_filter(&db, "3R2 · t+pazolite")), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn alias_names_from_musicbrainz() {
        let v = json!({ "name": "Ye", "sort-name": "Ye", "aliases": [{ "name": "Kanye West", "sort-name": "West, Kanye" }, { "name": "Ye" }] });
        assert_eq!(alias_names(&v), vec!["Ye", "Kanye West", "West, Kanye"]);
    }
}
