//! Library artists. There is no artists table: an artist is a name in `tracks.albumartist`. This module
//! turns those names into real artists and keeps the result in memory (rebuilt when the tracks change):
//! - spellings with the same key ("The Black Eyed Peas" / "black eyed peas") or the same MusicBrainz id
//!   are one artist, shown with the spelling that has the most songs;
//! - collaborations ("Kanye West & XXXTENTACION") are split into their artists when every part is an
//!   artist of its own in the library (or a known alias), and then count under each part;
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
use std::sync::{Arc, Mutex};
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

/// Separators between the artists of a collaboration (matched case-insensitively).
const SEPARATORS: &[&str] = &[" featuring ", " feat. ", " feat ", " ft. ", " ft ", " vs. ", " with ", " and ", " & ", ", ", " x ", " / "];

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

/// Separators that always start another artist ("feat."), even one that isn't in the library.
const FEAT: &[&str] = &[" featuring ", " feat. ", " feat ", " ft. ", " ft "];

/// The artist names in a song's artist text, in order: the whole text when it is one known artist, the
/// index's split of a collaboration ("A & B" only when both are known), and otherwise the pieces around
/// "feat." ("Travis Scott feat. The Weeknd" → both, known or not), each split again when it can be.
pub fn credit_names(s: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return vec![];
    }
    let one = |part: &str| -> Vec<String> {
        let k = key(part);
        if !k.is_empty() && known(&k) {
            return vec![part.to_string()];
        }
        split(part, known).unwrap_or_else(|| vec![part.to_string()])
    };
    // a library artist whose own name has " ft " / " feat " in it stays whole
    let whole = key(s);
    if !whole.is_empty() && known(&whole) {
        return vec![s.to_string()];
    }
    let low = s.to_ascii_lowercase();
    let b = low.as_bytes();
    let (mut start, mut i, mut out) = (0, 0, vec![]);
    while i < b.len() {
        if let Some(sep) = FEAT.iter().find(|sep| b[i..].starts_with(sep.as_bytes())) {
            let part = s[start..i].trim();
            if !part.is_empty() {
                out.extend(one(part));
            }
            i += sep.len();
            start = i;
        } else {
            i += 1;
        }
    }
    if start == 0 {
        return one(s);
    }
    let part = s[start..].trim();
    if !part.is_empty() {
        out.extend(one(part));
    }
    out
}

// ---- matching ------------------------------------------------------------------------------------

/// A search query, folded once.
pub struct Q {
    f: String,
    k: String,
    word: String,
    compact: String,
    raw: String,
}

impl Q {
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

#[derive(Clone, Debug, Default)]
pub struct Artist {
    pub name: String,
    pub key: String,
    pub mbid: String,
    /// albumartist spellings of this artist itself
    pub spellings: Vec<String>,
    /// other names (MusicBrainz aliases)
    pub aliases: Vec<String>,
    pub tracks: i64,
    pub albums: i64,
    pub size: i64,
    pub added: f64,
    pub cover_album: String,
    /// every albumartist value whose albums belong here (own spellings and collaborations)
    pub aa_names: Vec<String>,
    /// every track artist value whose songs belong here
    pub artist_names: Vec<String>,
    pub(crate) folds: Vec<(String, String, bool)>, // (raw, fold, is alias)
}

/// A name as it appears in the tracks table, with the artists it stands for.
pub(crate) struct Name {
    pub(crate) raw: String,
    lower: String,
    pub(crate) fold: String,
    pub(crate) groups: Vec<usize>,
}

pub struct Track {
    pub id: i64,
    pub(crate) title: String,
    pub(crate) title_f: String,
    title_l: String,
    pub(crate) album_f: String,
    album_l: String,
    album_raw: String,
    pub(crate) artist_ix: usize,
    pub(crate) aa_ix: usize,
    disc: i64,
    track: i64,
    year: String,
    duration: f64,
    added: f64,
    lossless: bool,
    bitrate: i64,
}

pub struct Album {
    pub id: String,
    pub(crate) title: String,
    pub(crate) title_f: String,
    pub(crate) aa_ix: usize,
    pub(crate) artist_ixs: Vec<usize>,
}

#[derive(Default)]
pub struct Index {
    pub artists: Vec<Artist>,
    by_key: HashMap<String, usize>,
    pub(crate) names: Vec<Name>, // albumartist and artist values (one entry per distinct value)
    pub(crate) tracks: Vec<Track>,
    pub(crate) albums: Vec<Album>,
}

struct Uf(HashMap<String, String>);
impl Uf {
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
    fn union(&mut self, a: &str, b: &str) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0.insert(hi, lo);
        }
    }
}

#[derive(Default)]
struct RawAa {
    tracks: i64,
    size: i64,
    added: f64,
    mbid: String,
    albums: BTreeSet<String>,
}

impl Index {
    pub fn build(db: &Db) -> anyhow::Result<Index> {
        let rows = db.query(
            "SELECT id, title, artist, album, albumartist, album_id, disc, track, year, duration, format, bitrate, size, added_at, mb_albumartistid FROM tracks",
            p![],
        )?;
        let aliases = db.query("SELECT artist_key, alias, alias_key FROM artist_alias", p![])?;

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
        let known = |k: &str| own_keys.contains(k) || alias_of.contains_key(k);

        // collaborations among the albumartist values
        let mut collab: HashMap<String, Vec<String>> = HashMap::new(); // albumartist → part names
        for name in raw.keys() {
            if let Some(parts) = split(name, &known) {
                collab.insert(name.clone(), parts);
            }
        }
        // a part may be known only through an alias of a name that turned out to be a collaboration
        // itself: such a name couldn't be split into artists below, so it stays an artist of its own
        loop {
            let nodes: HashSet<String> = raw.keys().filter(|n| !collab.contains_key(*n)).map(|n| key(n)).filter(|k| !k.is_empty()).collect();
            let alias_keys: HashSet<&String> = alias_of.iter().filter(|(_, owners)| owners.iter().any(|(o, _)| nodes.contains(o))).map(|(k, _)| k).collect();
            let before = collab.len();
            collab.retain(|name, _| split(name, &|k: &str| nodes.contains(k) || alias_keys.contains(&k.to_string())).is_some());
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
        // what a name stands for: an artist, an alias, or a collaboration of them
        let resolve = |name: &str, by_key: &HashMap<String, usize>| -> Vec<usize> {
            let k = key(name);
            if k.is_empty() {
                return vec![];
            }
            if let Some(&ix) = by_key.get(&k) {
                return vec![ix];
            }
            let mut out = vec![];
            if let Some(parts) = split(name, &|pk: &str| by_key.contains_key(pk)) {
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
                albums.push(Album { id: aid.clone(), title: String::new(), title_f: String::new(), aa_ix, artist_ixs: vec![] });
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
        for (g, a) in artists.iter_mut().enumerate() {
            a.albums = album_sets[g].len() as i64;
            a.name = own_best[g].1.clone();
            a.key = key(&a.name);
            a.aliases.retain(|x| key(x) != a.key && !a.spellings.iter().any(|s| key(s) == key(x)));
            a.aliases.sort();
            a.folds = a.spellings.iter().map(|s| (s.clone(), fold(s), false)).chain(a.aliases.iter().map(|s| (s.clone(), fold(s), true))).collect();
        }
        Ok(Index { artists, by_key, names, tracks, albums })
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
    pub fn credits(&self, artist: &str) -> Value {
        let names = credit_names(artist, &|k: &str| self.by_key.contains_key(k));
        Value::Array(names.into_iter().map(|n| match self.find(&n) {
            Some(a) if !a.mbid.is_empty() => json!({ "name": n, "mbid": a.mbid, "in_library": true }),
            Some(_) => json!({ "name": n, "in_library": true }),
            None => json!({ "name": n, "in_library": false }),
        }).collect())
    }

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
        let q = Q::new(q);
        let at = self.artist_tiers(&q);
        let mut hit: Vec<Option<(u8, &str)>> = at.iter().map(|t| t.map(|(t, alias)| (t, if alias { "alias" } else { "name" }))).collect();
        for al in &self.albums {
            if q.tier(&al.title, &al.title_f).is_some() {
                for &g in &self.names[al.aa_ix].groups {
                    if hit[g].is_none() {
                        hit[g] = Some((4, "album"));
                    }
                }
            }
        }
        if hit.iter().any(|h| h.is_none()) {
            for t in &self.tracks {
                if q.tier_fast(&t.title, &t.title_f).is_some() {
                    for ix in [t.artist_ix, t.aa_ix] {
                        for &g in &self.names[ix].groups {
                            if hit[g].is_none() {
                                hit[g] = Some((5, "song"));
                            }
                        }
                    }
                }
            }
        }
        let mut found: Vec<(u8, &str, &Artist)> = hit.iter().enumerate().filter_map(|(i, h)| h.map(|(t, m)| (t, m, &self.artists[i]))).collect();
        found.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.tracks.cmp(&a.2.tracks)).then_with(|| a.2.name.to_lowercase().cmp(&b.2.name.to_lowercase())));
        found.into_iter().take(limit).map(|(_, m, a)| {
            let mut v = Self::to_json(a);
            v["match"] = json!(m);
            v
        }).collect()
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
        let artist_l = |t: &Track| self.names[t.artist_ix].lower.as_str();
        let (by, desc_default): (&str, bool) = match sort {
            "title" | "album" => (sort, false),
            "added" | "released" | "duration" => (sort, true),
            _ => ("artist", false),
        };
        let desc = if dir == "asc" { false } else if dir == "desc" { true } else { desc_default };
        let ord = |a: &Track, b: &Track| -> Ordering {
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
        };
        hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| ord(a.1, b.1)));
        hits.into_iter().map(|(_, t)| t).collect()
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

struct Cached {
    checked: Instant,
    print: String,
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
    cached: Option<Cached>,
    /// changed or forgotten since `cached` was made: check again before using it
    dirty: bool,
    /// counts `invalidate` calls, so a rebuild that read the tables before one doesn't count as fresh
    generation: u64,
    build: Arc<Mutex<()>>,
}

fn fingerprint(db: &Db) -> String {
    // also the columns the library lists sort and filter on (year, length, size, added, format, tags):
    // their cached answers (routes/library.rs) live as long as this index does
    let t = db.one("SELECT COUNT(*) n, COALESCE(MAX(id),0) m, TOTAL(mtime) t, TOTAL(length(albumartist)) + TOTAL(length(artist)) * 3 + TOTAL(length(title)) * 7 + TOTAL(length(album)) * 11 + TOTAL(length(mb_albumartistid)) * 13 l,
                    TOTAL(length(year)) + TOTAL(length(format)) * 3 + TOTAL(length(mb_albumid)) * 7 + TOTAL(length(genre)) * 11 + TOTAL(length(verify_status)) * 13 + TOTAL(length(album_id)) * 17 l2,
                    TOTAL(duration) d, TOTAL(size) s, TOTAL(added_at) a, TOTAL(disc * 1000 + track) o FROM tracks", p![]).ok().flatten();
    let a = db.one("SELECT COUNT(*) n, TOTAL(length(alias_key)) l FROM artist_alias", p![]).ok().flatten();
    let f = |r: &Option<encore_db::Row>| r.as_ref().map(|r| serde_json::to_string(r).unwrap_or_default()).unwrap_or_default();
    format!("{}|{}", f(&t), f(&a))
}

/// The artist index of one library: built from its database, rebuilt when the songs or aliases change.
/// The server keeps one (in [`crate::Library`]); clones share it.
#[derive(Clone, Default)]
pub struct Artists(Arc<Mutex<Slot>>);

impl Artists {
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
            if !conds.is_empty() {
                return (format!("({})", conds.join(" OR ")), params);
            }
        }
        ("(lower(albumartist)=lower(?) OR lower(artist)=lower(?))".into(), p![name, name])
    }

    /// SQL condition for "albums of this artist" (album artist is one of its spellings or collaborations).
    pub fn album_filter(&self, db: &Db, name: &str) -> (String, Vec<Value>) {
        let idx = self.index(db);
        match idx.find(name) {
            Some(a) if !a.aa_names.is_empty() => (format!("albumartist IN ({})", marks(a.aa_names.len())), a.aa_names.iter().map(|n| json!(n)).collect()),
            _ => ("lower(albumartist)=lower(?)".into(), p![name]),
        }
    }
}

/// `?,?,?` for n values.
pub fn marks(n: usize) -> String {
    vec!["?"; n.max(1)].join(",")
}

// ---- aliases from MusicBrainz ---------------------------------------------------------------------

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

/// Runs `refresh_aliases` after every library scan (including the one at start).
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

    #[test]
    fn alias_names_from_musicbrainz() {
        let v = json!({ "name": "Ye", "sort-name": "Ye", "aliases": [{ "name": "Kanye West", "sort-name": "West, Kanye" }, { "name": "Ye" }] });
        assert_eq!(alias_names(&v), vec!["Ye", "Kanye West", "West, Kanye"]);
    }
}
