//! Search that reads what was typed: "travis scott 90210", "90210 travis scott", "90210 by travis scott" and
//! "travis scott - 90210" are all the song 90210 by Travis Scott.
//!
//! - The query is split into (artist, title) at every word boundary, both orders, plus "title by artist" and
//!   "artist - title". Each split is scored by how well its artist part is a known artist (the library and its
//!   aliases: strong; MusicBrainz exact name or alias: medium) and by how many free-text hits on Deezer / Apple
//!   agree with it. The best split drives targeted searches; the whole query is searched as free text too.
//! - Discover (the catalogue): MusicBrainz, Deezer and Apple iTunes in parallel (4 s each, cached 10 min),
//!   merged by ISRC or artist + title + length, ranked (exact artist + exact title first; joke, karaoke,
//!   tribute and uploader-style results last), with a Top result.
//! - The library: the same splits over the library index, typos forgiven ("trvis scot 90210"), accents
//!   folded, with a Top result (GET /api/library/search).
use crate::artists::{fold, strip_the, Index, Track};
use crate::catalog;
use crate::Ctx;
use encore_core::error::ApiResult;
use encore_core::json::s;
use encore_db::RowExt;
use encore_http::{apple, deezer, mb};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::time::Instant;

// ---- reading the query ---------------------------------------------------------------------------

/// One way to read a query: this artist, this title.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub artist: String,
    pub title: String,
}

fn add_split(out: &mut Vec<Split>, a: &str, t: &str) {
    let (a, t) = (a.trim(), t.trim());
    if fold(a).is_empty() || fold(t).is_empty() || out.iter().any(|s| s.artist == a && s.title == t) {
        return;
    }
    out.push(Split { artist: a.to_string(), title: t.to_string() });
}

/// Every way to read the query as an artist and a title ("title by artist", "artist - title", then every word
/// boundary in both orders).
pub fn splits(q: &str) -> Vec<Split> {
    let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = vec![];
    let low = q.to_ascii_lowercase();
    if let Some(p) = low.rfind(" by ") {
        add_split(&mut out, &q[p + 4..], &q[..p]);
    }
    for sep in [" - ", " – ", " — "] {
        if let Some(p) = q.find(sep) {
            add_split(&mut out, &q[..p], &q[p + sep.len()..]);
            add_split(&mut out, &q[p + sep.len()..], &q[..p]);
        }
    }
    let words: Vec<&str> = q.split(' ').filter(|w| !matches!(*w, "-" | "–" | "—")).collect();
    if words.len() <= 12 {
        for i in 1..words.len() {
            let (a, b) = (words[..i].join(" "), words[i..].join(" "));
            add_split(&mut out, &a, &b);
            add_split(&mut out, &b, &a);
        }
    }
    out
}

// ---- comparing names ------------------------------------------------------------------------------

static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[][^\)\]]*[\)\]]").unwrap());
static DASH_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+-\s+.*$").unwrap());
/// Karaoke, tributes, parodies and other versions nobody searching for the song means.
static JUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"(?i)\b(karaoke|tribute|parody|in the style of|made famous|originally performed|as performed by|cover version|lullaby|8[- ]bit|backing track|ringtone|kidz bop|sped up|slowed)\b").unwrap());
/// Uploader-style artist names ("user-177634521").
static USERISH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(user|artist|unknown)[-_ ]?\d{3,}|^\d{6,}$").unwrap());

/// A title without "(feat. …)", "[Live]" or " - Remastered 2011", folded.
pub fn base_title(s: &str) -> String {
    let t = BRACKETS.replace_all(s, "");
    let t = DASH_SUFFIX.replace(&t, "");
    let f = fold(&t);
    if f.is_empty() { fold(s) } else { f }
}

/// `base_title` of a title whose `fold` is known, without the regexes when there is nothing to strip.
fn base_of(raw: &str, folded: &str) -> String {
    if raw.contains(['(', '[']) || raw.contains(" - ") { base_title(raw) } else { folded.to_string() }
}

/// `n` is in `h` as whole words (both folded).
fn has_words(h: &str, n: &str) -> bool {
    !n.is_empty() && (h == n || h.starts_with(&format!("{n} ")) || h.ends_with(&format!(" {n}")) || h.contains(&format!(" {n} ")))
}

/// 2: the credit is this artist; 1: the artist is one of the credit's names; 0: someone else.
pub fn artist_grade(credit: &str, artist: &str) -> usize {
    let (fc, fa) = (fold(credit), fold(artist));
    if fc.is_empty() || fa.is_empty() {
        return 0;
    }
    if fc == fa || strip_the(&fc) == strip_the(&fa) {
        2
    } else if has_words(&fc, &fa) {
        1
    } else {
        0
    }
}

/// 3: the title is this; 2: it starts with it; 1: it has it as words; 0: no.
pub fn title_grade(title: &str, want: &str) -> usize {
    let ft = fold(want);
    if ft.is_empty() {
        return 0;
    }
    let (bt, ff) = (base_title(title), fold(title));
    if bt == ft || ff == ft {
        3
    } else if bt.starts_with(&format!("{ft} ")) || ff.starts_with(&format!("{ft} ")) {
        2
    } else if has_words(&ff, &ft) {
        1
    } else {
        0
    }
}

/// Edit distance of a and b when it is at most `max`.
pub fn lev(a: &str, b: &str, max: usize) -> Option<usize> {
    if a.chars().count().abs_diff(b.chars().count()) > max {
        return None;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        let mut row_min = i;
        for j in 1..=b.len() {
            let c = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + c);
            row_min = row_min.min(cur[j]);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[b.len()] <= max).then_some(prev[b.len()])
}

/// Typos forgiven for a name of n characters.
fn tolerance(n: usize) -> usize {
    match n {
        0..=3 => 0,
        4..=7 => 1,
        8..=12 => 2,
        _ => 3,
    }
}

/// Folded a and b are the same name, give or take a typo or two.
pub fn fuzzy_eq(a: &str, b: &str) -> bool {
    if a == b {
        return !a.is_empty();
    }
    let t = tolerance(a.chars().count().min(b.chars().count()));
    t > 0 && lev(a, b, t).is_some()
}

/// A joke / karaoke / tribute word in these names that the query didn't ask for ("Tribute" by Tenacious D
/// searched as "tenacious d tribute" is the song, not junk).
fn junk(parts: &[&str], qf: &str) -> bool {
    let asked: HashSet<&str> = qf.split(' ').filter(|w| !w.is_empty()).collect();
    parts.iter().any(|p| JUNK.find_iter(p).any(|m| {
        let f = fold(m.as_str());
        f.is_empty() || !f.split(' ').all(|w| asked.contains(w))
    }))
}

// ---- choosing the split ------------------------------------------------------------------------------

/// What is known about artist names when a split is chosen.
pub struct Evidence<'a> {
    /// 2: a library artist (or alias); 1: one with a typo; 0: unknown
    pub library: &'a dyn Fn(&str) -> u8,
    /// MusicBrainz artists found for the whole query: (name, aliases, score)
    pub artists: &'a [(String, Vec<String>, f64)],
    /// free-text song hits (artist, title), best first
    pub hits: &'a [(String, String)],
}

/// The split the evidence agrees with; None when nothing supports one (or the query is just an artist).
/// It also weighs `extra` readings (a sentence's "title by artist", or its words without the filler:
/// "radiohead album ok computer" → radiohead / ok computer).
pub fn choose_split_with(q: &str, extra: &[Split], ev: &Evidence) -> Option<Split> {
    let qf = fold(q);
    let known = |name: &str| {
        let f = fold(name);
        (ev.library)(name) == 2 || ev.artists.iter().any(|(n, al, _)| fold(n) == f || al.iter().any(|a| fold(a) == f))
    };
    if known(q) && !ev.hits.iter().take(3).any(|(a, _)| fold(a) != qf && has_words(&qf, &fold(a))) {
        return None; // "travis scott": the artist, not a song called "scott" by Travis
    }
    let mut cands = splits(q);
    for sp in extra {
        add_split(&mut cands, &sp.artist, &sp.title);
    }
    let mut best: Option<(f64, Split)> = None;
    for sp in cands {
        let fa = fold(&sp.artist);
        let mut x = match (ev.library)(&sp.artist) {
            2 => 60.0,
            1 => 35.0,
            _ => 0.0,
        };
        if let Some((_, _, sc)) = ev.artists.iter().find(|(n, al, _)| fold(n) == fa || al.iter().any(|a| fold(a) == fa)) {
            x += 20.0 + sc / 100.0 * 10.0;
        }
        let mut agree = 0.0;
        for (i, (ha, ht)) in ev.hits.iter().enumerate().take(12) {
            if artist_grade(ha, &sp.artist) > 0 && title_grade(ht, &sp.title) >= 2 {
                agree += (40.0 - 3.0 * i as f64).max(10.0);
            }
        }
        x += f64::min(agree, 90.0);
        if x < 30.0 {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| x > *b) {
            best = Some((x, sp));
        }
    }
    best.map(|b| b.1)
}

// ---- what the services found ---------------------------------------------------------------------------

/// Raw answers of the services (MusicBrainz, Deezer, Apple iTunes) for one search; tests fill it from fixtures.
#[derive(Default, Clone)]
pub struct Found {
    /// MusicBrainz /artist?query= (whole query)
    pub mb_artists: Value,
    /// MusicBrainz /recording?query= answers (split, free text)
    pub mb_recordings: Vec<Value>,
    /// MusicBrainz /release-group?query= answers (split, free text)
    pub mb_groups: Vec<Value>,
    /// Deezer /search answers (split, free text), best first
    pub deezer_tracks: Vec<Value>,
    /// Deezer /search/album
    pub deezer_albums: Value,
    /// iTunes entity=song / album / musicArtist
    pub itunes_songs: Value,
    pub itunes_albums: Value,
    pub itunes_artists: Value,
    /// Deezer track id → ISRC (looked up for the top few)
    pub isrcs: HashMap<String, String>,
}

/// MusicBrainz artists (the shape of `mb::search_artists`) with their aliases and score.
pub fn mb_artists(raw: &Value) -> Vec<(Value, Vec<String>, f64)> {
    raw["artists"].as_array().cloned().unwrap_or_default().iter().map(|a| {
        let mut tags: Vec<(i64, String)> = a["tags"].as_array().map(|t| t.iter().map(|x| (x["count"].as_i64().unwrap_or(0), s(&x["name"]))).collect()).unwrap_or_default();
        tags.sort_by_key(|x| std::cmp::Reverse(x.0));
        let v = json!({
            "id": s(&a["id"]), "name": s(&a["name"]), "disambiguation": s(&a["disambiguation"]), "type": s(&a["type"]), "country": s(&a["country"]),
            "score": a.get("score").cloned().unwrap_or(json!(0)), "begin": s(&a["life-span"]["begin"]),
            "tags": tags.into_iter().take(4).map(|t| t.1).collect::<Vec<_>>(),
        });
        let aliases = a["aliases"].as_array().map(|x| x.iter().map(|y| s(&y["name"])).filter(|n| !n.is_empty()).collect()).unwrap_or_default();
        let sc = a["score"].as_f64().unwrap_or(0.0);
        (v, aliases, sc)
    }).collect()
}

/// Free-text song hits (artist, title) of Deezer and Apple, interleaved by position.
pub fn evidence_hits(f: &Found) -> Vec<(String, String)> {
    let dz: Vec<(String, String)> = f.deezer_tracks.last().and_then(|d| d["data"].as_array()).cloned().unwrap_or_default().iter()
        .map(|t| (s(&t["artist"]["name"]), s(&t["title"]))).collect();
    let it: Vec<(String, String)> = f.itunes_songs["results"].as_array().cloned().unwrap_or_default().iter()
        .map(|t| (s(&t["artistName"]), s(&t["trackName"]))).collect();
    let mut out = vec![];
    for i in 0..dz.len().max(it.len()) {
        out.extend(dz.get(i).cloned());
        out.extend(it.get(i).cloned());
    }
    out
}

#[derive(Clone, Debug)]
struct Song {
    v: Value,
    artist: String,
    title: String,
    album: String,
    length: f64,
    isrcs: Vec<String>,
    mb: bool,
    mb_score: f64,
    rank: f64,
    bonus: f64,
    official: bool,
    rg_type: String,
    secondary: Vec<String>,
}

fn release_rank(r: &Value) -> (i32, std::cmp::Reverse<String>) {
    let rg = &r["release-group"];
    let sec = rg["secondary-types"].as_array().map(|a| a.len()).unwrap_or(0) as i32;
    let typ = match rg["primary-type"].as_str().unwrap_or("") { "Album" => 3, "EP" => 1, _ => 0 };
    let date = s(&r["date"]);
    (i32::from(r["status"] == "Official") * 4 + typ - sec * 4, std::cmp::Reverse(if date.is_empty() { "9999".into() } else { date }))
}

fn song_from_mb(r: &Value) -> Song {
    let rels = r["releases"].as_array().cloned().unwrap_or_default();
    let rel = rels.iter().max_by_key(|x| release_rank(x)).cloned().unwrap_or(json!({}));
    let rgid = s(&rel["release-group"]["id"]);
    let year: String = { let d = s(&rel["date"]); if d.is_empty() { s(&r["first-release-date"]) } else { d } }.chars().take(4).collect();
    let isrcs: Vec<String> = r["isrcs"].as_array().map(|a| a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| s(&x["id"]))).collect()).unwrap_or_default();
    let (artist, title, album) = (mb::credit(&r["artist-credit"]), s(&r["title"]), s(&rel["title"]));
    let length = r["length"].as_f64().unwrap_or(0.0) / 1000.0;
    let v = json!({
        "id": s(&r["id"]), "title": title, "artist": artist, "artist_id": mb::credit_id(&r["artist-credit"]),
        "length": length, "album": album, "release_id": rel.get("id").cloned().unwrap_or(Value::Null),
        "release_group_id": if rgid.is_empty() { Value::Null } else { json!(rgid) }, "year": year,
        "score": r.get("score").cloned().unwrap_or(json!(0)), "cover": if rgid.is_empty() { Value::Null } else { json!(mb::cover_rg(&rgid, 250)) },
        "isrc": isrcs.first().cloned(), "source": "MusicBrainz",
    });
    Song {
        v, artist, title, album, length, isrcs, mb: true, mb_score: r["score"].as_f64().unwrap_or(0.0), rank: 0.0, bonus: 0.0,
        official: rel["status"] == "Official", rg_type: s(&rel["release-group"]["primary-type"]),
        secondary: rel["release-group"]["secondary-types"].as_array().map(|a| a.iter().map(s).collect()).unwrap_or_default(),
    }
}

/// A song only Deezer / Apple know, before it becomes a `Song`.
struct ExtSong<'a> {
    title: String,
    artist: String,
    album: String,
    length: f64,
    year: String,
    cover: Value,
    isrc: Option<String>,
    ext: Value,
    source: &'a str,
    rank: f64,
}

fn ext_song(e: ExtSong) -> Song {
    let ExtSong { title, artist, album, length, year, cover, isrc, ext, source, rank } = e;
    let v = json!({
        "id": null, "title": title, "artist": artist, "artist_id": null, "length": length, "album": album, "release_id": null,
        "release_group_id": null, "year": year, "score": 0, "cover": cover, "isrc": isrc, "ext": ext, "source": source,
    });
    Song { v, artist, title, album, length, isrcs: isrc.into_iter().collect(), mb: false, mb_score: 0.0, rank, bonus: 0.0, official: false, rg_type: String::new(), secondary: vec![] }
}

fn song_from_deezer(t: &Value, isrcs: &HashMap<String, String>) -> Song {
    let id = t["id"].to_string();
    let isrc = t["isrc"].as_str().map(String::from).or_else(|| isrcs.get(&id).cloned());
    let cover = t["album"].get("cover_xl").filter(|c| !c.is_null()).or(t["album"].get("cover_medium")).cloned().unwrap_or(Value::Null);
    ext_song(ExtSong {
        title: s(&t["title"]), artist: s(&t["artist"]["name"]), album: s(&t["album"]["title"]), length: t["duration"].as_f64().unwrap_or(0.0),
        year: String::new(), cover, isrc, ext: json!({ "deezer": t["id"] }), source: "Deezer", rank: t["rank"].as_f64().unwrap_or(0.0),
    })
}

fn song_from_itunes(t: &Value) -> Song {
    let cover = s(&t["artworkUrl100"]).replace("100x100bb", "600x600bb");
    ext_song(ExtSong {
        title: s(&t["trackName"]), artist: s(&t["artistName"]), album: s(&t["collectionName"]), length: t["trackTimeMillis"].as_f64().unwrap_or(0.0) / 1000.0,
        year: s(&t["releaseDate"]).chars().take(4).collect(), cover: if cover.is_empty() { Value::Null } else { json!(cover) }, isrc: None,
        ext: json!({ "apple": t["trackId"] }), source: "Apple Music", rank: 0.0,
    })
}

fn same_song(a: &Song, b: &Song) -> bool {
    if !a.isrcs.is_empty() && b.isrcs.iter().any(|i| a.isrcs.contains(i)) {
        return true;
    }
    let ag = artist_grade(&a.artist, &b.artist).max(artist_grade(&b.artist, &a.artist));
    ag > 0 && base_title(&a.title) == base_title(&b.title) && (a.length <= 0.0 || b.length <= 0.0 || (a.length - b.length).abs() <= 3.0)
}

/// Free text: the share of the query's words found in these names.
fn coverage(qf: &str, parts: &[&str]) -> f64 {
    let hay = parts.iter().map(|p| fold(p)).collect::<Vec<_>>().join(" ");
    let words: Vec<&str> = qf.split(' ').filter(|w| !w.is_empty()).collect();
    words.iter().filter(|w| has_words(&hay, w)).count() as f64 / words.len().max(1) as f64
}

const ARTIST_W: [f64; 3] = [0.0, 50.0, 100.0];
const TITLE_W: [f64; 4] = [0.0, 25.0, 60.0, 100.0];

/// How well a name pair answers the query: the split's artist and title, or the words of the query.
fn match_score(artist: &str, title: &str, extra: &str, sp: Option<&Split>, qf: &str) -> (f64, usize, usize) {
    match sp {
        Some(sp) => {
            let (ag, tg) = (artist_grade(artist, &sp.artist), title_grade(title, &sp.title));
            let mut x = ARTIST_W[ag] + TITLE_W[tg];
            if ag == 0 {
                let fa = fold(&sp.artist);
                // a song titled "travis scott" by somebody else
                if has_words(&fold(title), &fa) || has_words(&fold(extra), &fa) {
                    x -= 80.0;
                }
                if tg == 0 {
                    x -= 40.0;
                }
            }
            (x, ag, tg)
        }
        None => {
            let mut x = 100.0 * coverage(qf, &[artist, title]);
            if base_title(title) == qf {
                x += 40.0;
            }
            (x, 0, 0)
        }
    }
}

fn score_song(x: &Song, sp: Option<&Split>, qf: &str) -> f64 {
    let (mut sc, _, _) = match_score(&x.artist, &x.title, &x.album, sp, qf);
    if USERISH.is_match(x.artist.trim()) {
        sc -= 150.0;
    }
    if junk(&[&x.title, &x.artist, &x.album], qf) {
        sc -= 90.0;
    }
    sc += x.mb_score / 100.0 * 10.0 + (x.rank / 1_000_000.0).min(1.0) * 30.0 + x.bonus.min(40.0);
    if x.official {
        sc += 10.0;
    }
    sc += match x.rg_type.as_str() { "Album" => 10.0, "EP" => 3.0, _ => 0.0 };
    for sec in &x.secondary {
        sc -= match sec.as_str() { "Compilation" => 15.0, "Live" => 10.0, _ => 5.0 };
    }
    if !x.mb {
        sc -= 10.0; // a MusicBrainz copy can be requested and tagged exactly
    }
    sc
}

#[derive(Clone, Debug)]
struct Alb {
    v: Value,
    artist: String,
    title: String,
    typ: String,
    secondary: Vec<String>,
    mb: bool,
    mb_score: f64,
    bonus: f64,
    fixed: Option<f64>,
}

fn alb_from_mb(g: &Value) -> Alb {
    let v = mb::rg(g);
    Alb {
        artist: s(&v["artist"]), title: s(&v["title"]), typ: s(&v["type"]), secondary: v["secondary"].as_array().map(|a| a.iter().map(s).collect()).unwrap_or_default(),
        mb: true, mb_score: g["score"].as_f64().unwrap_or(0.0), bonus: 0.0, fixed: None, v,
    }
}

/// An album only Deezer / Apple know, shaped like the catalog merge's "ext" releases.
fn ext_album(title: &str, artist: &str, typ: &str, date: &str, cover: Value, tracks: Value, src: (&str, &str, Value, String)) -> Alb {
    let (name, key, id, url) = src;
    let id_s = id.as_str().map(String::from).unwrap_or(id.to_string());
    let v = json!({
        "id": format!("ext-{}{}", name.chars().next().unwrap_or('x').to_lowercase(), id_s), "title": title, "artist": artist, "artist_id": null,
        "type": typ, "secondary": [], "date": date, "year": date.chars().take(4).collect::<String>(), "score": 0, "cover": cover, "tracks": tracks,
        "sources": [{ "name": name, "id": id, "url": url }], "ext": { key: id }, "harmony": catalog::harmony_url(std::slice::from_ref(&url), None),
    });
    Alb { v, artist: artist.into(), title: title.into(), typ: typ.into(), secondary: vec![], mb: false, mb_score: 0.0, bonus: 0.0, fixed: None }
}

fn alb_from_deezer(a: &Value) -> Alb {
    let title = s(&a["title"]);
    let cover = a.get("cover_xl").filter(|c| !c.is_null()).or(a.get("cover_medium")).cloned().unwrap_or(Value::Null);
    ext_album(&title, &s(&a["artist"]["name"]), catalog::kind_of(&s(&a["record_type"]), &title), &s(&a["release_date"]), cover, a["nb_tracks"].clone(),
        ("Deezer", "deezer", a["id"].clone(), format!("https://www.deezer.com/album/{}", a["id"])))
}

fn alb_from_itunes(a: &Value) -> Alb {
    let raw = s(&a["collectionName"]);
    let title = raw.trim_end_matches(" - Single").trim_end_matches(" - EP").to_string();
    let cover = s(&a["artworkUrl100"]).replace("100x100bb", "600x600bb");
    let url = a["collectionViewUrl"].as_str().map(String::from).unwrap_or(format!("https://music.apple.com/album/{}", a["collectionId"]));
    ext_album(&title, &s(&a["artistName"]), catalog::kind_of("", &raw), &s(&a["releaseDate"]).chars().take(10).collect::<String>(),
        if cover.is_empty() { Value::Null } else { json!(cover) }, a["trackCount"].clone(), ("Apple Music", "apple", a["collectionId"].clone(), url))
}

fn same_album(a: &Alb, b: &Alb) -> bool {
    let ag = artist_grade(&a.artist, &b.artist).max(artist_grade(&b.artist, &a.artist));
    ag > 0 && catalog::title_key(&a.title) == catalog::title_key(&b.title) && (a.typ == b.typ || (a.typ != "Single" && b.typ != "Single"))
}

fn score_album(x: &Alb, sp: Option<&Split>, qf: &str) -> f64 {
    let (mut sc, _, _) = match_score(&x.artist, &x.title, "", sp, qf);
    if USERISH.is_match(x.artist.trim()) {
        sc -= 150.0;
    }
    if junk(&[&x.title, &x.artist], qf) {
        sc -= 90.0;
    }
    sc += x.mb_score / 100.0 * 10.0 + x.bonus.min(40.0);
    sc += match x.typ.as_str() { "Album" => 15.0, "EP" => 5.0, "Single" => -5.0, _ => 0.0 };
    for sec in &x.secondary {
        sc -= match sec.as_str() { "Compilation" => 20.0, "Live" => 10.0, _ => 5.0 };
    }
    if !x.mb {
        sc -= 5.0;
    }
    // the album of one of the best songs ranks with the song
    x.fixed.map_or(sc, |f| f.max(sc))
}

fn score_artist(name: &str, aliases: &[String], mb_score: f64, sp: Option<&Split>, qf: &str, in_library: bool) -> f64 {
    let f = fold(name);
    let is = |want: &str| f == want || strip_the(&f) == strip_the(want) || aliases.iter().any(|a| fold(a) == want);
    let mut x = if is(qf) {
        300.0
    } else if sp.is_some_and(|sp| is(&fold(&sp.artist))) {
        150.0
    } else if has_words(qf, &f) {
        40.0
    } else {
        0.0
    };
    x += mb_score / 100.0 * 20.0;
    if in_library {
        x += 30.0;
    }
    if USERISH.is_match(name.trim()) {
        x -= 150.0;
    }
    x
}

/// Merges and ranks what the services found. `t` is all | artist | album | track. The result has
/// artists / albums / tracks (best first) and top: {kind, i} (an index into its list), before library
/// annotations.
pub fn rank(q: &str, sp: Option<&Split>, f: &Found, lib_artist: &dyn Fn(&str) -> bool, t: &str) -> Value {
    let qf = fold(q);
    let want = |k: &str| t == "all" || t == k;

    // songs: MusicBrainz first, then Deezer / Apple matched onto them (ISRC, or artist + title + length)
    let mut songs: Vec<Song> = vec![];
    if want("track") {
        let mut seen = HashSet::new();
        for raw in &f.mb_recordings {
            for r in raw["recordings"].as_array().cloned().unwrap_or_default() {
                if seen.insert(s(&r["id"])) {
                    songs.push(song_from_mb(&r));
                }
            }
        }
        let mut ext: Vec<(usize, Song)> = vec![];
        for raw in &f.deezer_tracks {
            for (i, x) in raw["data"].as_array().cloned().unwrap_or_default().iter().enumerate() {
                ext.push((i, song_from_deezer(x, &f.isrcs)));
            }
        }
        for (i, x) in f.itunes_songs["results"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            if x["wrapperType"].as_str().unwrap_or("track") == "track" {
                ext.push((i, song_from_itunes(x)));
            }
        }
        for (pos, e) in ext {
            let bonus = (20.0 - 2.0 * pos as f64).max(4.0);
            let mut matched = false;
            for m in songs.iter_mut() {
                if same_song(m, &e) {
                    matched = true;
                    m.bonus += bonus;
                    m.rank = m.rank.max(e.rank);
                    if m.v["cover"].is_null() {
                        m.v["cover"] = e.v["cover"].clone();
                    }
                    if m.v["isrc"].is_null() && !e.isrcs.is_empty() {
                        m.v["isrc"] = json!(e.isrcs[0]);
                    }
                }
            }
            if !matched {
                songs.push(Song { bonus, ..e });
            }
        }
    }
    let mut scored: Vec<(f64, Song)> = songs.into_iter().map(|x| (score_song(&x, sp, &qf), x)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    // one entry per song (the same recording on many releases, or a Deezer / Apple twin)
    let mut keep: Vec<(f64, Song)> = vec![];
    for (sc, x) in scored {
        if keep.iter().any(|(_, k)| fold(&k.artist) == fold(&x.artist) && base_title(&k.title) == base_title(&x.title)
            && (k.length <= 0.0 || x.length <= 0.0 || (k.length - x.length).abs() <= 3.0)) {
            continue;
        }
        keep.push((sc, x));
    }
    keep.truncate(if t == "all" { 15 } else { 40 });

    // albums: MusicBrainz release groups, Deezer / Apple albums MusicBrainz lacks, and the albums of the best songs
    let mut albums: Vec<Alb> = vec![];
    if want("album") {
        let mut seen = HashSet::new();
        for raw in &f.mb_groups {
            for g in raw["release-groups"].as_array().cloned().unwrap_or_default() {
                if seen.insert(s(&g["id"])) {
                    albums.push(alb_from_mb(&g));
                }
            }
        }
        let mut ext: Vec<(usize, Alb)> = vec![];
        for (i, a) in f.deezer_albums["data"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            ext.push((i, alb_from_deezer(a)));
        }
        for (i, a) in f.itunes_albums["results"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            if a["wrapperType"].as_str().unwrap_or("collection") == "collection" {
                ext.push((i, alb_from_itunes(a)));
            }
        }
        for (pos, e) in ext {
            let bonus = (20.0 - 2.0 * pos as f64).max(4.0);
            if let Some(m) = albums.iter_mut().find(|m| same_album(m, &e)) {
                m.bonus += bonus;
                if !m.mb {
                    // the same album on Deezer and Apple: one entry with both sources
                    if let (Some(src), Some(add)) = (m.v["sources"].as_array_mut(), e.v["sources"].as_array()) {
                        src.extend(add.iter().cloned());
                    }
                    if let (Some(ex), Some(add)) = (m.v["ext"].as_object_mut(), e.v["ext"].as_object()) {
                        for (k, v) in add {
                            ex.entry(k.clone()).or_insert(v.clone());
                        }
                    }
                }
                continue;
            }
            albums.push(Alb { bonus, ..e });
        }
    }
    if t == "all" || t == "album" {
        for (sc, x) in keep.iter().take(3) {
            let rgid = s(&x.v["release_group_id"]);
            if rgid.is_empty() || *sc < 100.0 || !x.secondary.is_empty() {
                continue;
            }
            if let Some(a) = albums.iter_mut().find(|a| s(&a.v["id"]) == rgid) {
                a.fixed = Some(a.fixed.unwrap_or(f64::MIN).max(sc - 40.0));
                continue;
            }
            let v = json!({
                "id": rgid, "title": x.album, "artist": x.artist, "artist_id": x.v["artist_id"], "type": x.rg_type, "secondary": x.secondary,
                "year": x.v["year"], "date": x.v["year"], "score": 0, "cover": mb::cover_rg(&rgid, 250),
            });
            albums.push(Alb { v, artist: x.artist.clone(), title: x.album.clone(), typ: x.rg_type.clone(), secondary: x.secondary.clone(), mb: true,
                mb_score: 0.0, bonus: 0.0, fixed: Some(sc - 40.0) });
        }
    }
    let mut ascored: Vec<(f64, Alb)> = albums.into_iter().map(|a| (score_album(&a, sp, &qf), a)).collect();
    ascored.sort_by(|a, b| b.0.total_cmp(&a.0));
    ascored.truncate(if t == "all" { 18 } else { 40 });

    // artists: MusicBrainz (they open artist pages and can be requested)
    let mut artists: Vec<(f64, Value)> = vec![];
    if want("artist") {
        for (v, aliases, sc) in mb_artists(&f.mb_artists) {
            let name = s(&v["name"]);
            artists.push((score_artist(&name, &aliases, sc, sp, &qf, lib_artist(&name)), v));
        }
        artists.sort_by(|a, b| b.0.total_cmp(&a.0));
        artists.truncate(if t == "all" { 12 } else { 30 });
    }

    // the Top result: the query is an artist's name → the artist; else the best song or album
    let song_top = keep.first().map(|(sc, x)| {
        let tg = sp.map(|sp| title_grade(&x.title, &sp.title)).unwrap_or(0);
        (sc + if tg == 3 { 10.0 } else { 0.0 }, "song")
    });
    let album_top = ascored.first().map(|(sc, a)| {
        let tg = sp.map(|sp| title_grade(&a.title, &sp.title)).unwrap_or(0);
        (sc + if tg == 3 && a.typ == "Album" { 12.0 } else { 0.0 } - if a.typ == "Single" { 20.0 } else { 0.0 }, "album")
    });
    let artist_top = artists.first().map(|(sc, _)| (*sc, "artist"));
    let top = match artist_top {
        Some((sc, _)) if sc >= 300.0 => Some("artist"),
        _ => {
            let best = [song_top, album_top].into_iter().flatten().max_by(|a, b| a.0.total_cmp(&b.0));
            match (best, artist_top) {
                (Some((x, k)), Some((y, _))) if x < 100.0 && y >= 150.0 => Some(if y > x { "artist" } else { k }),
                (Some((_, k)), _) => Some(k),
                (None, Some(_)) => Some("artist"),
                (None, None) => None,
            }
        }
    };
    json!({
        "artists": artists.into_iter().map(|a| a.1).collect::<Vec<_>>(),
        "albums": ascored.into_iter().map(|a| a.1.v).collect::<Vec<_>>(),
        "tracks": keep.into_iter().map(|x| x.1.v).collect::<Vec<_>>(),
        "top": top.map(|k| json!({ "kind": k, "i": 0 })).unwrap_or(Value::Null),
        "split": sp.map(|sp| json!({ "artist": sp.artist, "title": sp.title })).unwrap_or(Value::Null),
    })
}

// ---- fetching (one deadline for the whole search) ---------------------------------------------------

/// The whole Discover search answers within this, whatever is slow or blocked.
const DEADLINE: Duration = Duration::from_secs(5);
/// One Deezer / Apple call; MusicBrainz gets the rest of the deadline (its calls wait for a 1/s gate).
const EXT_TIMEOUT: Duration = Duration::from_millis(2500);

/// A Deezer / Apple call; Null when it failed or took too long (EXT_TIMEOUT, and never past `until`).
async fn get_json(req: reqwest::RequestBuilder, until: Instant) -> Value {
    let fut = async {
        let r = req.send().await.ok()?;
        r.error_for_status().ok()?.json::<Value>().await.ok()
    };
    let end = until.min(Instant::now() + EXT_TIMEOUT);
    tokio::time::timeout_at(end, fut).await.ok().flatten().unwrap_or(Value::Null)
}

/// A MusicBrainz search; Err when MusicBrainz failed or did not answer by `until`.
async fn mb_search(st: &impl Ctx, path: &str, query: String, limit: usize, until: Instant) -> Result<Value, String> {
    match tokio::time::timeout_at(until, st.mb().get(st, path, &[("query", query), ("limit", limit.to_string())])).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("MusicBrainz took too long".into()),
    }
}

async fn deezer_isrc(st: &impl Ctx, id: String, until: Instant) -> Option<(String, String)> {
    let key = format!("dzisrc:{id}");
    if let Some(v) = st.cache_get(&key) {
        return v.as_str().map(|i| (id, i.to_string()));
    }
    let v = get_json(deezer::track(st, &id), until).await;
    let isrc = v["isrc"].as_str()?.to_string();
    st.cache_put(&key, json!(isrc), 86400);
    Some((id, isrc))
}

/// Library artist evidence for a name: 2 a library artist (any spelling or alias), 1 with a typo, 0 unknown.
fn library_grade(idx: &Index, name: &str) -> u8 {
    if idx.find(name).is_some() {
        return 2;
    }
    let f = fold(name);
    let k = strip_the(&f);
    if tolerance(k.chars().count()) == 0 {
        return 0;
    }
    if idx.artists.iter().any(|a| a.folds.iter().any(|(_, af, _)| fuzzy_eq(strip_the(af), k))) { 1 } else { 0 }
}

/// Discover search, ranked (cached 10 min; 30 s when a service failed or was slow); the server then
/// annotates it with the library and requests. `alt` is another reading of a sentence (nlsearch without the
/// filler) whose splits are weighed too. Err when nothing answered and MusicBrainz failed.
pub async fn discover_ranked(st: &impl Ctx, q: &str, alt: Option<&str>, t: &str) -> Result<Value, String> {
    let key = format!("dsearch:{t}:{}|{}", fold(q), alt.map(fold).unwrap_or_default());
    match st.cache_get(&key) {
        Some(v) => Ok(v),
        None => {
            let (v, degraded) = discover_uncached(st, q, alt, t).await?;
            st.cache_put(&key, v.clone(), if degraded { 30 } else { 600 });
            Ok(v)
        }
    }
}

/// The ranked result and whether any service failed or was cut off (a degraded answer is cached briefly).
async fn discover_uncached(st: &impl Ctx, q: &str, alt: Option<&str>, t: &str) -> Result<(Value, bool), String> {
    let want = |k: &str| t == "all" || t == k;
    let until = Instant::now() + DEADLINE;
    let idx = st.library().artists.index(st.db());
    // 1. the whole query: artists (MusicBrainz) and free-text songs / albums (Deezer, Apple)
    let n = if t == "all" { 12 } else { 30 };
    let (mb_art, dz_plain, it_song, it_album, it_artist, dz_album) = tokio::join!(
        mb_search(st, "artist", q.to_string(), n, until),
        get_json(deezer::search(st, "", q, 15), until),
        get_json(apple::search(st, q, "song", 15), until),
        async { if want("album") { get_json(apple::search(st, q, "album", 10), until).await } else { Value::Null } },
        get_json(apple::search(st, q, "musicArtist", 5), until),
        async { if want("album") { get_json(deezer::search(st, "album", q, 10), until).await } else { Value::Null } },
    );
    let mut mb_failed = mb_art.is_err();
    // Deezer / Apple unreachable (blocked, down): don't wait on them again below
    let deezer_up = !dz_plain.is_null();
    let mut degraded = !deezer_up || it_song.is_null();
    let mut f = Found { mb_artists: mb_art.unwrap_or(Value::Null), deezer_tracks: vec![], deezer_albums: dz_album, itunes_songs: it_song,
        itunes_albums: it_album, itunes_artists: it_artist, ..Default::default() };
    f.deezer_tracks.push(dz_plain);
    let arts: Vec<(String, Vec<String>, f64)> = mb_artists(&f.mb_artists).into_iter().map(|(v, al, sc)| (s(&v["name"]), al, sc)).collect();
    let mut arts_ev = arts.clone();
    for a in f.itunes_artists["results"].as_array().cloned().unwrap_or_default() {
        arts_ev.push((s(&a["artistName"]), vec![], 50.0));
    }
    let hits = evidence_hits(&f);
    let lib = |name: &str| library_grade(&idx, name);
    let extra = alt.map(splits).unwrap_or_default();
    let sp = choose_split_with(q, &extra, &Evidence { library: &lib, artists: &arts_ev, hits: &hits });

    // 2. targeted searches with the split, free text where there is none
    let n = if t == "all" { 15 } else { 40 };
    let rec_split = sp.as_ref().filter(|_| want("track")).map(|sp| format!("recording:\"{}\" AND artist:\"{}\"", mb::esc(&sp.title), mb::esc(&sp.artist)));
    let rg_split = sp.as_ref().filter(|_| want("album")).map(|sp| format!("releasegroup:\"{}\" AND artist:\"{}\"", mb::esc(&sp.title), mb::esc(&sp.artist)));
    let rec_free = (want("track") && (sp.is_none() || t == "track")).then(|| q.to_string());
    let rg_free = (want("album") && (sp.is_none() || t == "album")).then(|| q.to_string());
    let dz_q = sp.as_ref().filter(|_| deezer_up).map(|sp| format!("artist:\"{}\" track:\"{}\"", sp.artist.replace('"', ""), sp.title.replace('"', "")));
    let dz_split_wanted = dz_q.is_some();
    let opt_mb = |path: &'static str, query: Option<String>, lim: usize| async move {
        match query { Some(qq) => Some(mb_search(st, path, qq, lim, until).await), None => None }
    };
    let (r1, r2, g1, g2, dz_split) = tokio::join!(
        opt_mb("recording", rec_split, n),
        opt_mb("recording", rec_free, n),
        opt_mb("release-group", rg_split, if t == "all" { 18 } else { 40 }),
        opt_mb("release-group", rg_free, if t == "all" { 18 } else { 40 }),
        async { match dz_q { Some(dq) => get_json(deezer::search(st, "", &dq, 10), until).await, None => Value::Null } },
    );
    degraded |= dz_split_wanted && dz_split.is_null();
    for r in [r1, r2].into_iter().flatten() {
        match r { Ok(v) => f.mb_recordings.push(v), Err(_) => mb_failed = true }
    }
    for r in [g1, g2].into_iter().flatten() {
        match r { Ok(v) => f.mb_groups.push(v), Err(_) => mb_failed = true }
    }
    if !dz_split.is_null() {
        f.deezer_tracks.insert(0, dz_split);
    }

    // 3. ISRCs of the top Deezer songs (to meet their MusicBrainz recordings)
    if want("track") && deezer_up && until.saturating_duration_since(Instant::now()) > Duration::from_millis(500) {
        let mut ids: Vec<String> = vec![];
        for d in &f.deezer_tracks {
            for x in d["data"].as_array().cloned().unwrap_or_default().iter().take(3) {
                let id = x["id"].to_string();
                if x["isrc"].is_null() && !ids.contains(&id) && ids.len() < 3 {
                    ids.push(id);
                }
            }
        }
        let mut set = tokio::task::JoinSet::new();
        for id in ids {
            let st = st.clone();
            set.spawn(async move { deezer_isrc(&st, id, until).await });
        }
        while let Some(r) = set.join_next().await {
            if let Ok(Some((id, isrc))) = r {
                f.isrcs.insert(id, isrc);
            }
        }
    }

    let lib_artist = |name: &str| idx.find(name).is_some();
    let mut out = rank(q, sp.as_ref(), &f, &lib_artist, t);
    let empty = ["artists", "albums", "tracks"].iter().all(|k| out[*k].as_array().is_none_or(|a| a.is_empty()));
    if empty && mb_failed {
        return Err("MusicBrainz error: no answer".into());
    }
    degraded |= mb_failed;
    // ext songs open their artist by MusicBrainz id when the artist was found
    if let Some(tracks) = out["tracks"].as_array_mut() {
        for x in tracks.iter_mut().filter(|x| x["artist_id"].is_null()) {
            let fa = fold(&s(&x["artist"]));
            if let Some((v, _, _)) = mb_artists(&f.mb_artists).into_iter().find(|(v, _, _)| fold(&s(&v["name"])) == fa) {
                x["artist_id"] = v["id"].clone();
            }
        }
    }
    Ok((out, degraded))
}

// ---- the library ----------------------------------------------------------------------------------

/// Library artists a name stands for: (artist index, 1.0 exact / 0.7 with a typo).
fn artist_matches(idx: &Index, name: &str) -> Vec<(usize, f64)> {
    let f = fold(name);
    let k = strip_the(&f).to_string();
    if k.is_empty() {
        return vec![];
    }
    let fuzzy_ok = tolerance(k.chars().count()) > 0;
    idx.artists.iter().enumerate().filter_map(|(g, a)| {
        let mut w: f64 = 0.0;
        for (_, af, _) in &a.folds {
            if *af == f || strip_the(af) == k {
                return Some((g, 1.0));
            }
            if fuzzy_ok && fuzzy_eq(strip_the(af), &k) {
                w = 0.7;
            }
        }
        (w > 0.0).then_some((g, w))
    }).collect()
}

fn lib_title_grade(title: &str, want: &str) -> usize {
    let g = title_grade(title, want);
    if g > 0 {
        return g;
    }
    if fuzzy_eq(&base_title(title), &fold(want)) { 2 } else { 0 }
}

/// What the splits of a query find in the library.
#[derive(Default)]
pub struct LibHits<'a> {
    /// (score, song), best first
    pub tracks: Vec<(f64, &'a Track)>,
    /// (score, album index, title grade), best first
    pub albums: Vec<(f64, usize, usize)>,
    /// (score, artist index), best first
    pub artists: Vec<(f64, usize)>,
}

/// Every split whose artist part is a library artist (typos forgiven) and whose title part names one of its
/// songs or albums.
pub fn library_splits<'a>(idx: &'a Index, q: &str) -> LibHits<'a> {
    let mut tracks: HashMap<i64, (f64, &Track)> = HashMap::new();
    let mut albums: HashMap<usize, (f64, usize)> = HashMap::new();
    let mut artists: HashMap<usize, f64> = HashMap::new();
    for sp in splits(q) {
        let ws: HashMap<usize, f64> = artist_matches(idx, &sp.artist).into_iter().collect();
        if ws.is_empty() {
            continue;
        }
        let weight = |ixs: &[usize]| -> f64 {
            ixs.iter().flat_map(|&ix| idx.names[ix].groups.iter()).filter_map(|g| ws.get(g)).fold(0.0, |a: f64, b| a.max(*b))
        };
        let mut found = false;
        for t in &idx.tracks {
            let w = weight(&[t.artist_ix, t.aa_ix]);
            if w == 0.0 {
                continue;
            }
            let tg = lib_title_grade(&t.title, &sp.title);
            if tg == 0 {
                continue;
            }
            found = true;
            let sc = w * 100.0 + TITLE_W[tg];
            let e = tracks.entry(t.id).or_insert((sc, t));
            e.0 = e.0.max(sc);
        }
        for (i, al) in idx.albums.iter().enumerate() {
            let mut ixs = vec![al.aa_ix];
            ixs.extend(al.artist_ixs.iter().copied());
            let w = weight(&ixs);
            if w == 0.0 {
                continue;
            }
            let tg = lib_title_grade(&al.title, &sp.title);
            if tg == 0 {
                continue;
            }
            found = true;
            let sc = w * 100.0 + TITLE_W[tg];
            let e = albums.entry(i).or_insert((sc, tg));
            if sc > e.0 {
                *e = (sc, tg);
            }
        }
        if found {
            for (g, w) in ws {
                let e = artists.entry(g).or_insert(0.0);
                *e = e.max(w * 100.0);
            }
        }
    }
    let mut out = LibHits {
        tracks: tracks.into_values().collect(),
        albums: albums.into_iter().map(|(i, (sc, tg))| (sc, i, tg)).collect(),
        artists: artists.into_iter().map(|(g, sc)| (sc, g)).collect(),
    };
    out.tracks.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    out.albums.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    out.artists.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    out
}

/// The whole query with typos forgiven: artists, albums and songs whose name is it, give or take a letter.
pub fn library_fuzzy<'a>(idx: &'a Index, q: &str) -> LibHits<'a> {
    let qf = fold(q);
    let qk = strip_the(&qf).to_string();
    let mut out = LibHits::default();
    if tolerance(qk.chars().count()) == 0 {
        return out;
    }
    let arts: HashSet<usize> = artist_matches(idx, q).into_iter().map(|(g, _)| g).collect();
    out.artists = arts.iter().map(|&g| (70.0, g)).collect();
    out.artists.sort_by(|a, b| idx.artists[b.1].tracks.cmp(&idx.artists[a.1].tracks));
    for (i, al) in idx.albums.iter().enumerate() {
        if fuzzy_eq(&base_of(&al.title, &al.title_f), &qf) {
            out.albums.push((70.0, i, 2));
        }
    }
    for t in &idx.tracks {
        if fuzzy_eq(&base_of(&t.title, &t.title_f), &qf) {
            out.tracks.push((70.0, t));
        } else if idx.names[t.artist_ix].groups.iter().chain(idx.names[t.aa_ix].groups.iter()).any(|g| arts.contains(g)) {
            out.tracks.push((40.0, t));
        }
    }
    out.tracks.sort_by(|a, b| b.0.total_cmp(&a.0));
    out
}

/// `/api/library/tracks?q=`: songs by a split first ("travis scott 90210"), then the plain ranked search, then
/// typo matches when that found little; the same song twice only once.
pub fn library_track_ids(idx: &Index, q: &str, sort: &str, dir: &str) -> Vec<i64> {
    track_ids_with(idx, q, sort, dir, &library_splits(idx, q), &Fuzzy::new(idx, q))
}

/// `library_fuzzy` worked out once, when first needed (library_search shares it between songs, albums and
/// artists).
struct Fuzzy<'a> {
    idx: &'a Index,
    q: &'a str,
    cell: std::cell::OnceCell<LibHits<'a>>,
}

impl<'a> Fuzzy<'a> {
    fn new(idx: &'a Index, q: &'a str) -> Self {
        Fuzzy { idx, q, cell: std::cell::OnceCell::new() }
    }
    fn get(&self) -> &LibHits<'a> {
        self.cell.get_or_init(|| library_fuzzy(self.idx, self.q))
    }
}

fn track_ids_with(idx: &Index, q: &str, sort: &str, dir: &str, split: &LibHits, fuzzy: &Fuzzy) -> Vec<i64> {
    let plain = idx.search_track_refs(q, sort, dir);
    let mut seq: Vec<&Track> = split.tracks.iter().map(|x| x.1).collect();
    let few = plain.len() < 5;
    seq.extend(plain);
    if few {
        seq.extend(fuzzy.get().tracks.iter().map(|x| x.1));
    }
    let mut seen = HashSet::new();
    idx.dedupe(seq.into_iter().filter(|t| seen.insert(t.id))).into_iter().map(|t| t.id).collect()
}

/// `/api/library/albums?q=`: album ids and their tier (0 for a split's album, the plain search's tiers after).
pub fn library_album_ranks(idx: &Index, q: &str) -> Vec<(String, u8)> {
    album_ranks_with(idx, q, &library_splits(idx, q), &Fuzzy::new(idx, q))
}

fn album_ranks_with(idx: &Index, q: &str, split: &LibHits, fuzzy: &Fuzzy) -> Vec<(String, u8)> {
    let mut out: Vec<(String, u8)> = split.albums.iter().map(|(_, i, _)| (idx.albums[*i].id.clone(), 0)).collect();
    let plain = idx.search_albums(q);
    let empty = plain.is_empty();
    out.extend(plain.into_iter().map(|(id, t)| (id, t + 1)));
    if empty {
        out.extend(fuzzy.get().albums.iter().map(|(_, i, _)| (idx.albums[*i].id.clone(), 6)));
    }
    let mut seen = HashSet::new();
    out.retain(|(id, _)| seen.insert(id.clone()));
    out
}

/// `/api/library/artists?q=`: the plain ranked search, with a split's artist first and typo matches when it
/// found nothing.
pub fn library_artists(idx: &Index, q: &str, limit: usize) -> Vec<Value> {
    artists_with(idx, q, limit, &library_splits(idx, q), &Fuzzy::new(idx, q))
}

fn artists_with(idx: &Index, q: &str, limit: usize, split: &LibHits, fuzzy: &Fuzzy) -> Vec<Value> {
    let mut out: Vec<Value> = split.artists.iter().map(|(_, g)| {
        let mut v = Index::to_json(&idx.artists[*g]);
        v["match"] = json!("name");
        v
    }).collect();
    let plain = idx.search_artists(q, limit);
    let empty = plain.is_empty();
    out.extend(plain);
    if empty {
        out.extend(fuzzy.get().artists.iter().map(|&(_, g)| {
            let mut v = Index::to_json(&idx.artists[g]);
            v["match"] = json!("name");
            v
        }));
    }
    let mut seen = HashSet::new();
    out.retain(|v| seen.insert(v["name"].as_str().unwrap_or("").to_lowercase()));
    out.truncate(limit);
    out
}

const ALBUM_COLS: &str = "album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(genre) genre,
    COUNT(*) tracks, SUM(duration) duration, SUM(size) size, MAX(added_at) added,
    GROUP_CONCAT(DISTINCT format) formats, MAX(mb_albumid) mb_albumid, MAX(mb_releasegroupid) mb_releasegroupid,
    SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) untagged";

/// {top: {kind, item}, songs, albums, artists} over the library (GET /api/library/search?q=: the phone's
/// Search tab and Discover's "In your library" row).
pub fn library_search(st: &impl Ctx, q: &str, n: usize) -> ApiResult<Value> {
    let empty = json!({ "top": null, "songs": [], "albums": [], "artists": [] });
    if q.chars().count() < 2 {
        return Ok(empty);
    }
    let idx = st.library().artists.index(st.db());
    let split = library_splits(&idx, q);
    let fuzzy = Fuzzy::new(&idx, q);
    let song_ids: Vec<i64> = track_ids_with(&idx, q, "", "", &split, &fuzzy).into_iter().take(n).collect();
    let mut album_ids: Vec<String> = album_ranks_with(&idx, q, &split, &fuzzy).into_iter().map(|x| x.0).collect();
    // the albums of the songs a split found ("I didn't know what album the song was on")
    for (_, t) in split.tracks.iter().take(3) {
        if let Some(al) = idx.albums.iter().find(|al| al.aa_ix == t.aa_ix && al.title_f == t.album_f) {
            if !album_ids.contains(&al.id) {
                album_ids.insert(album_ids.len().min(split.albums.len()), al.id.clone());
            }
        }
    }
    album_ids.truncate(n.min(40));
    let artists = artists_with(&idx, q, n.min(20), &split, &fuzzy);

    let songs = if song_ids.is_empty() { vec![] } else {
        let rows = st.db().query(&format!("SELECT {} FROM tracks WHERE id IN ({})", crate::lists::TRACK_COLS, crate::artists::marks(song_ids.len())),
            song_ids.iter().map(|i| json!(i)).collect())?;
        let mut by_id: HashMap<i64, encore_db::Row> = rows.into_iter().map(|r| (r.i("id"), r)).collect();
        song_ids.iter().filter_map(|i| by_id.remove(i)).collect::<Vec<_>>()
    };
    let albums = if album_ids.is_empty() { vec![] } else {
        let rows = st.db().query(&format!("SELECT {ALBUM_COLS} FROM tracks WHERE album_id IN ({}) GROUP BY album_id", crate::artists::marks(album_ids.len())),
            album_ids.iter().map(|i| json!(i)).collect())?;
        let mut by_id: HashMap<String, encore_db::Row> = rows.into_iter().map(|r| (r.s("id"), r)).collect();
        album_ids.iter().filter_map(|i| by_id.remove(i)).collect::<Vec<_>>()
    };

    // the Top result
    let qf = fold(q);
    let is_artist = idx.find(q).is_some() || artists.first().is_some_and(|a| fold(a["name"].as_str().unwrap_or("")) == qf);
    let best_song = split.tracks.first().map(|x| x.0).unwrap_or(0.0);
    let best_album = split.albums.first().map(|x| x.0 + if x.2 == 3 { 5.0 } else { 0.0 }).unwrap_or(0.0);
    let top = if is_artist && !artists.is_empty() && best_song < 160.0 {
        Some(("artist", artists[0].clone()))
    } else if best_album > best_song && !albums.is_empty() {
        Some(("album", json!(albums[0])))
    } else if best_song > 0.0 && !songs.is_empty() {
        Some(("song", json!(songs[0])))
    } else if let Some(sg) = songs.iter().find(|r| base_title(&r.s("title")) == qf) {
        Some(("song", json!(sg)))
    } else if let Some(al) = albums.iter().find(|r| base_title(&r.s("album")) == qf) {
        Some(("album", json!(al)))
    } else if let Some(a) = artists.first() {
        Some(("artist", a.clone()))
    } else if let Some(sg) = songs.first() {
        Some(("song", json!(sg)))
    } else {
        albums.first().map(|al| ("album", json!(al)))
    };
    Ok(json!({
        "top": top.map(|(k, item)| json!({ "kind": k, "item": item })).unwrap_or(Value::Null),
        "songs": songs, "albums": albums, "artists": artists,
    }))
}

#[cfg(test)]
#[path = "search_rank_tests.rs"]
mod tests;
