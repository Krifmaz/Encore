//! Each album's type (0.36, docs/contracts/library-types.md §3): its **format** – Album, EP, Single or One-off
//! (a song or two of a bigger release: a compilation, a remix album, a soundtrack) – and its **kind** – studio,
//! live, compilation, remix, soundtrack … – worked out from what is known about it, and the one **filter key**
//! of Library → Albums' Type dropdown that follows from the two.
//!
//! What is known comes, in this order, from the facts stored in `album_types` (taken at import from the
//! release the files were tagged from, or looked up by the "Sort albums by type" task: [`scheduled`]), the files'
//! own RELEASETYPE tags, and else the album's title and its songs' titles. The owner's choice (Change type…) is
//! stored beside the facts and always wins. Nothing here asks MusicBrainz on a request's way: the album lists,
//! the album page and the counts read the classification [`Types`] keeps per state of the library.
use encore_config::release_types::{self as rt, canon};
use encore_core::json::str_ref as s;
use encore_db::{now, p, Conn, Db, RowExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// The formats an album can have (what Change type sets): (key, word).
pub const FORMATS: [(&str, &str); 4] = [("album", "Album"), ("ep", "EP"), ("single", "Single"), ("one_off", "One-off")];

/// What is on an album (always worked out): (key, word on the chip; studio has none).
pub const KINDS: [(&str, &str); 10] = [
    ("studio", ""), ("live", "Live"), ("compilation", "Compilation"), ("remix", "Remix"), ("soundtrack", "Soundtrack"), ("demo", "Demo"),
    ("dj_mix", "DJ mix"), ("mixtape", "Mixtape"), ("karaoke", "Karaoke / Instrumental"), ("other", "Other"),
];

/// The Type filter's keys and labels, in the dropdown's order (every album is under exactly one).
pub const TYPES: [(&str, &str); 12] = [
    ("album", "Studio albums"), ("live", "Live"), ("compilation", "Compilations"), ("remix", "Remixes"), ("soundtrack", "Soundtracks"),
    ("demo", "Demos"), ("dj_mix", "DJ mixes"), ("mixtape", "Mixtapes"), ("karaoke", "Karaoke / Instrumental"), ("singles", "Singles & EPs"),
    ("one_off", "One-offs"), ("other", "Other"),
];

/// MusicBrainz's Various Artists.
pub const VA_MBID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

/// Album artist names that mean "various artists" (folded by [`encore_core::text::words`]).
const VA_NAMES: [&str; 6] = ["various artists", "various", "va", "v a", "verschiedene interpreten", "artistes divers"];

pub fn format_word(f: &str) -> &'static str {
    FORMATS.iter().find(|x| x.0 == f).map(|x| x.1).unwrap_or("Album")
}

pub fn kind_word(k: &str) -> &'static str {
    KINDS.iter().find(|x| x.0 == k).map(|x| x.1).unwrap_or("")
}

/// The label of a filter key ("Studio albums", "Singles & EPs" …).
pub fn type_label(key: &str) -> &'static str {
    TYPES.iter().find(|x| x.0 == key).map(|x| x.1).unwrap_or("Other")
}

/// The album page's chip: the kind's word, with the format after it when that isn't Album ("Live · EP",
/// "Compilation · One-off"); for a studio album the format alone ("Album", "EP").
pub fn chip_label(format: &str, kind: &str) -> String {
    if kind == "studio" || kind_word(kind).is_empty() {
        format_word(format).to_string()
    } else if format == "album" {
        kind_word(kind).to_string()
    } else {
        format!("{} · {}", kind_word(kind), format_word(format))
    }
}

/// The Type filter key of a format and kind: One-offs, then what is on it, then Singles & EPs, then Studio albums.
pub fn filter_key(format: &str, kind: &str) -> &'static str {
    if format == "one_off" {
        "one_off"
    } else if kind != "studio" {
        KINDS.iter().find(|x| x.0 == kind).map(|x| x.0).unwrap_or("other")
    } else if format == "single" || format == "ep" {
        "singles"
    } else {
        "album"
    }
}

/// A release-types key (MusicBrainz secondary type or Encore's own extra) → the album kind.
pub fn kind_of_extra(extra: &str) -> &'static str {
    match extra {
        "Live" => "live",
        "Compilation" => "compilation",
        "Remix" => "remix",
        "Soundtrack" => "soundtrack",
        "Demo" => "demo",
        "DJ-mix" => "dj_mix",
        "Mixtape/Street" => "mixtape",
        "Karaoke" | "Instrumental" => "karaoke",
        // an acoustic version is still the artist's own album ("Unplugged" live sets are Live by the title rules)
        "Acoustic" => "studio",
        _ => "other",
    }
}

fn is_primary(k: &str) -> bool {
    rt::KINDS.iter().any(|x| x.0 == k)
}

/// A RELEASETYPE tag as Picard and the stores write it ("album; live", "Album/Live", "album, compilation",
/// "single\0remix") → (primary type, secondary types), in MusicBrainz's words. Words it doesn't know are left out.
pub fn parse_releasetype(tag: &str) -> (Option<&'static str>, Vec<&'static str>) {
    let mut primary = None;
    let mut secondary: Vec<&'static str> = vec![];
    for word in tag.split([';', '/', ',', '\0']).map(str::trim).filter(|w| !w.is_empty()) {
        let Some(k) = canon(word) else { continue };
        if is_primary(k) {
            primary = primary.or(Some(k));
        } else if !secondary.contains(&k) {
            secondary.push(k);
        }
    }
    (primary, secondary)
}

/// Picard's RELEASETYPE text for a release: the primary and secondary types lower-cased, "; " between.
pub fn releasetype_tag(primary: &str, secondary: &[String]) -> String {
    std::iter::once(primary.to_string()).chain(secondary.iter().cloned()).map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty())
        .collect::<Vec<_>>().join("; ")
}

/// Is this album artist Various Artists?
pub fn is_various(albumartist: &str, albumartist_id: &str) -> bool {
    albumartist_id.trim() == VA_MBID || VA_NAMES.contains(&encore_core::text::words(albumartist).as_str())
}

// ---- the classification ------------------------------------------------------------------------------

/// One song of an album, as the classification reads it.
#[derive(Clone, Debug, Default)]
pub struct Song {
    pub title: String,
    pub track: i64,
    /// the file's RELEASETYPE ('' none)
    pub releasetype: String,
    /// its TRACKTOTAL (0 none)
    pub tracktotal: i64,
    /// its COMPILATION flag
    pub compilation: bool,
}

/// What `album_types` holds for an album.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stored {
    /// the owner's format (None: automatic)
    pub chosen: Option<String>,
    /// musicbrainz | deezer | apple | discogs ('' no facts)
    pub source: String,
    /// the release's primary type (None: not known – never looked up, or MusicBrainz didn't know it)
    pub release_type: Option<String>,
    pub secondary: Vec<String>,
    /// songs on the release (0 unknown)
    pub tracks: i64,
}

/// An album as the classification reads it.
#[derive(Clone, Debug, Default)]
pub struct AlbumIn {
    pub album: String,
    pub albumartist: String,
    pub albumartist_id: String,
    pub songs: Vec<Song>,
    pub stored: Option<Stored>,
}

/// An album's type: what it is, why, and what the automatic answer would be.
#[derive(Clone, Debug, PartialEq)]
pub struct Class {
    pub format: &'static str,
    pub kind: &'static str,
    /// the owner's choice, when there is one
    pub chosen: Option<&'static str>,
    /// what the automatic answer is (the same as `format` without a choice)
    pub auto_format: &'static str,
    /// the rule that decided ([`why_text`])
    pub why: &'static str,
    /// the rule that decided the automatic answer
    pub auto_why: &'static str,
    /// the provider the facts are from (deezer | apple | discogs), for the "From …" text
    pub provider: &'static str,
    /// songs here and on the release (0 unknown), for the "Only N of its M songs" text
    pub present: usize,
    pub total: i64,
    /// a One-off's sub-line on the artist page: its kind's word, "Various artists" or its primary type
    pub release_label: String,
}

impl Default for Class {
    fn default() -> Self {
        Class { format: "album", kind: "studio", chosen: None, auto_format: "album", why: "default", auto_why: "default", provider: "",
                present: 0, total: 0, release_label: "Album".into() }
    }
}

impl Class {
    /// The Type filter key.
    pub fn key(&self) -> &'static str {
        filter_key(self.format, self.kind)
    }

    pub fn label(&self) -> String {
        chip_label(self.format, self.kind)
    }

    /// The album page's `type` object.
    pub fn json(&self) -> Value {
        json!({
            "key": self.key(), "label": self.label(), "format": self.format, "kind": self.kind, "chosen": self.chosen,
            "auto": { "format": self.auto_format, "kind": self.kind, "label": chip_label(self.auto_format, self.kind),
                      "key": filter_key(self.auto_format, self.kind), "why": self.auto_why,
                      "why_text": why_text(self.auto_why, self.provider, self.present, self.total) },
            "why": self.why, "why_text": why_text(self.why, self.provider, self.present, self.total),
        })
    }
}

/// The words for why an album is what it is (the Change type dialog shows the automatic one's).
pub fn why_text(why: &str, provider: &str, present: usize, total: i64) -> String {
    match why {
        "chosen" => "You chose this".into(),
        "musicbrainz" => "From MusicBrainz".into(),
        "provider" => format!("From {}", match provider { "apple" => "Apple Music", "discogs" => "Discogs", _ => "Deezer" }),
        "tags" => "From the file tags".into(),
        "title" => "From the album title".into(),
        "songs" => "Most of its songs say so in their titles".into(),
        "various" => "Various artists".into(),
        "partial" if total > 0 => format!("Only {present} of its {total} songs are here"),
        "partial" => format!("Only {present} of its songs are here"),
        "size" => "Guessed from the number of songs".into(),
        _ => "No type known".into(),
    }
}

fn re(p: &str) -> regex::Regex {
    regex::Regex::new(p).unwrap()
}

/// Markers in song titles (in brackets or after " - "): live, remix, karaoke / instrumental.
static SONG_MARKS: LazyLock<[(regex::Regex, &'static str); 3]> = LazyLock::new(|| [
    (re(r"(?i)[\(\[][^\)\]]*\blive\b[^\)\]]*[\)\]]|\s-\s+live\b"), "live"),
    (re(r"(?i)([\(\[][^\)\]]*|\s-\s+[^-]*)\b(remix(ed)?|rmx|club mix|extended mix|dub mix|vip mix|bootleg)\b"), "remix"),
    (re(r"(?i)([\(\[][^\)\]]*|\s-\s+[^-]*)\b(karaoke|instrumental|off[\s-]?vocal|backing track)\b"), "karaoke"),
]);

/// Where the facts came from.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Src {
    Mb,
    Provider(&'static str),
    Tags,
    None,
}

impl Src {
    fn why(self) -> &'static str {
        match self {
            Src::Mb => "musicbrainz",
            Src::Provider(_) => "provider",
            Src::Tags => "tags",
            Src::None => "title",
        }
    }
}

fn provider_key(source: &str) -> &'static str {
    match source {
        "apple" => "apple",
        "discogs" => "discogs",
        _ => "deezer",
    }
}

/// The most common value among the songs (ties: the first seen); None when every one is empty / 0.
fn most_common<T: PartialEq + Clone>(vals: impl Iterator<Item = T>, empty: impl Fn(&T) -> bool) -> Option<T> {
    let mut counts: Vec<(T, usize)> = vec![];
    for v in vals.filter(|v| !empty(v)) {
        match counts.iter_mut().find(|(x, _)| *x == v) {
            Some(c) => c.1 += 1,
            None => counts.push((v, 1)),
        }
    }
    let best = counts.iter().map(|c| c.1).max()?;
    counts.into_iter().find(|c| c.1 == best).map(|c| c.0)
}

/// The facts as a release-types `rg` (title, type, secondary; `ext` when they aren't MusicBrainz's, so the
/// title rules apply), and where they come from.
fn facts(a: &AlbumIn) -> (Value, Src) {
    if let Some(st) = a.stored.as_ref().filter(|st| st.release_type.as_deref().is_some_and(|t| !t.is_empty()) && !st.source.is_empty()) {
        let rg = json!({ "title": a.album, "type": st.release_type, "secondary": st.secondary, "disambiguation": "" });
        if st.source == "musicbrainz" {
            return (rg, Src::Mb);
        }
        let mut rg = rg;
        rg["ext"] = json!({});
        return (rg, Src::Provider(provider_key(&st.source)));
    }
    if let Some(tag) = most_common(a.songs.iter().map(|s| s.releasetype.trim().to_string()), |t| t.is_empty()) {
        let (primary, secondary) = parse_releasetype(&tag);
        if primary.is_some() || !secondary.is_empty() {
            return (json!({ "title": a.album, "type": primary.unwrap_or(""), "secondary": secondary, "disambiguation": "" }), Src::Tags);
        }
    }
    (json!({ "title": a.album, "type": "", "ext": {} }), Src::None)
}

/// Works out an album's type (§3.3 of the contract).
pub fn classify(a: &AlbumIn) -> Class {
    let present = a.songs.len();
    let (rg, src) = facts(a);
    let primary: Option<&'static str> = canon(s(&rg["type"])).filter(|k| is_primary(k));
    let secondary: Vec<&'static str> = rg["secondary"].as_array().map(|v| v.iter().filter_map(|x| canon(s(x)).filter(|k| !is_primary(k))).collect()).unwrap_or_default();

    // ---- kind
    let mut kind = "studio";
    let mut kind_why: Option<&'static str> = None;
    match src {
        // MusicBrainz's (or the tags') secondary types; their titles are the releases' real names, not read
        Src::Mb | Src::Tags => {
            if let Some(k) = secondary.iter().map(|x| kind_of_extra(x)).find(|k| *k != "studio") {
                kind = k;
                kind_why = Some(src.why());
            }
        }
        // a store's record type and title, or the title alone
        Src::Provider(_) | Src::None => {
            for m in rt::marks(&rg) {
                let k = kind_of_extra(m);
                if k != "studio" {
                    kind = k;
                    kind_why = Some(if secondary.contains(&m) { src.why() } else { "title" });
                    break;
                }
            }
        }
    }
    let va_name = is_various(&a.albumartist, &a.albumartist_id);
    let flagged = present > 0 && a.songs.iter().filter(|s| s.compilation).count() * 2 > present;
    if kind == "studio" && (va_name || flagged) {
        kind = "compilation";
        kind_why = Some(if va_name { "various" } else { "tags" });
    }
    if kind == "studio" && present > 0 {
        for (r, k) in SONG_MARKS.iter() {
            let n = a.songs.iter().filter(|s| r.is_match(&s.title)).count();
            let enough = if present <= 2 { n == present } else { n * 10 >= present * 6 };
            if enough {
                kind = k;
                kind_why = Some("songs");
                break;
            }
        }
    }
    if kind == "studio" && matches!(primary, Some("Broadcast" | "Other")) && secondary.is_empty() {
        kind = "other";
        kind_why = Some(src.why());
    }

    // ---- format
    let stored_tracks = a.stored.as_ref().map(|s| s.tracks).unwrap_or(0);
    let tag_total = most_common(a.songs.iter().map(|s| s.tracktotal), |n| *n <= 0).unwrap_or(0);
    let max_track = a.songs.iter().map(|s| s.track).max().unwrap_or(0);
    let total = stored_tracks.max(tag_total).max(max_track);
    let bigger = total == 0 || (present as i64) * 2 <= total;
    let title_kind = if a.album.ends_with(" - Single") { Some("single") } else if a.album.ends_with(" - EP") { Some("ep") } else { None };
    let known_small = matches!(primary, Some("Single" | "EP")) || title_kind.is_some();
    let album_key = crate::catalog::title_key(&a.album);
    let lone = present <= 2 && !a.songs.iter().any(|s| crate::catalog::title_key(&s.title) == album_key) && !known_small;
    let (auto_format, format_why): (&'static str, &'static str) =
        if present > 0 && present <= 3 && bigger && (matches!(kind, "compilation" | "soundtrack" | "remix" | "dj_mix") || va_name || flagged || lone) {
            ("one_off", "partial")
        } else if let Some(p) = primary {
            (match p { "EP" => "ep", "Single" => "single", _ => "album" }, src.why())
        } else if let Some(t) = title_kind {
            (t, "title")
        } else if present <= 2 {
            ("single", "size")
        } else {
            ("album", "size")
        };
    let auto_why = if format_why == "partial" { "partial" } else { kind_why.unwrap_or(format_why) };
    let chosen: Option<&'static str> = a.stored.as_ref().and_then(|s| s.chosen.as_deref()).and_then(|c| FORMATS.iter().find(|f| f.0 == c)).map(|f| f.0);
    let release_label = if kind_why == Some("various") && kind == "compilation" {
        "Various artists".to_string()
    } else if kind != "studio" {
        kind_word(kind).to_string()
    } else {
        match primary.or(title_kind.map(|t| if t == "ep" { "EP" } else { "Single" })) {
            Some("EP") => "EP".into(),
            Some("Single") => "Single".into(),
            _ => "Album".into(),
        }
    };
    Class {
        format: chosen.unwrap_or(auto_format),
        kind,
        chosen,
        auto_format,
        why: if chosen.is_some() { "chosen" } else { auto_why },
        auto_why,
        provider: match src { Src::Provider(p) => p, _ => "" },
        present,
        total,
        release_label,
    }
}

// ---- the filters -----------------------------------------------------------------------------------------

/// Which albums a list shows (`type=`, `kind=` of GET /api/library/albums and /index).
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    All,
    /// every album but the One-offs (the phone's list, the side index without a tab)
    NoOneOffs,
    /// these formats (the web's Albums and Singles & EPs tabs)
    Formats(Vec<&'static str>),
    /// these filter keys (and the format keys `ep` / `single`, each its own set)
    Keys(Vec<&'static str>),
}

impl Pick {
    /// `type` (one key or a comma list) wins over `kind`; a search without a type finds One-offs too.
    pub fn of(types: &str, kind: &str, searching: bool) -> Result<Pick, String> {
        let wanted: Vec<&str> = types.split(',').map(str::trim).filter(|t| !t.is_empty()).collect();
        if !wanted.is_empty() {
            let mut keys = vec![];
            for w in wanted {
                let k = TYPES.iter().map(|t| t.0).chain(["ep", "single"]).find(|k| *k == w).ok_or_else(|| "Unknown album type".to_string())?;
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
            return Ok(Pick::Keys(keys));
        }
        Ok(match kind {
            "albums" => Pick::Formats(vec!["album"]),
            "singles" => Pick::Formats(vec!["single", "ep"]),
            _ if searching => Pick::All,
            _ => Pick::NoOneOffs,
        })
    }

    pub fn wants(&self, c: Option<&Class>) -> bool {
        let d = Class::default();
        let c = c.unwrap_or(&d);
        match self {
            Pick::All => true,
            Pick::NoOneOffs => c.format != "one_off",
            Pick::Formats(f) => f.contains(&c.format),
            Pick::Keys(k) => k.contains(&c.key()) || (matches!(c.format, "ep" | "single") && k.contains(&c.format)),
        }
    }
}

// ---- the cache -------------------------------------------------------------------------------------------

/// Every album's [`Class`], built once per state of the library (`Db::library_version`: writes to `tracks` and
/// `album_types` move it) on the caller's thread – the API calls it from the blocking pool. Callers that come
/// while it is being built wait for that build.
/// Every album's class, by album id.
pub type Classes = Arc<HashMap<String, Class>>;

#[derive(Clone, Default)]
pub struct Types {
    kept: Arc<Mutex<Option<(u64, Classes)>>>,
    building: Arc<Mutex<()>>,
}

impl Types {
    pub fn get(&self, db: &Db) -> Classes {
        let fresh = |v: u64| self.kept.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(at, _)| *at == v).map(|(_, m)| m.clone());
        if let Some(m) = fresh(db.library_version()) {
            return m;
        }
        let _one = self.building.lock().unwrap_or_else(|e| e.into_inner());
        // read before the rows: a write meanwhile makes the next call build it again
        let version = db.library_version();
        if let Some(m) = fresh(version) {
            return m;
        }
        let m = Arc::new(build(db));
        *self.kept.lock().unwrap_or_else(|e| e.into_inner()) = Some((version, m.clone()));
        m
    }

    /// One album's class (the default, a studio album, for one the library doesn't have).
    pub fn of(&self, db: &Db, album_id: &str) -> Class {
        self.get(db).get(album_id).cloned().unwrap_or_default()
    }
}

/// What `album_types` holds, by album.
pub fn stored_rows(db: &impl encore_db::Query) -> HashMap<String, Stored> {
    db.query("SELECT album_id, chosen, release_source, release_type, release_secondary, release_tracks FROM album_types", p![]).unwrap_or_default().iter().map(|r| {
        let secondary: Vec<String> = serde_json::from_str::<Vec<String>>(&r.s("release_secondary")).unwrap_or_default();
        (r.s("album_id"), Stored {
            chosen: Some(r.s("chosen")).filter(|c| !c.is_empty()),
            source: r.s("release_source"),
            release_type: r.get("release_type").and_then(|v| v.as_str()).map(String::from),
            secondary,
            tracks: r.i("release_tracks"),
        })
    }).collect()
}

/// Every album's class, from one query over the songs and one over `album_types`.
pub fn build(db: &Db) -> HashMap<String, Class> {
    let rows = db.query("SELECT album_id, album, albumartist, mb_albumartistid, title, track, releasetype, tracktotal, compilation FROM tracks", p![]).unwrap_or_default();
    let mut stored = stored_rows(db);
    let mut albums: HashMap<String, AlbumIn> = HashMap::new();
    for r in rows {
        let id = r.s("album_id");
        let a = albums.entry(id).or_insert_with(|| AlbumIn { album: r.s("album"), albumartist: r.s("albumartist"), albumartist_id: r.s("mb_albumartistid"), ..Default::default() });
        if a.albumartist_id.is_empty() {
            a.albumartist_id = r.s("mb_albumartistid");
        }
        a.songs.push(Song { title: r.s("title"), track: r.i("track"), releasetype: r.s("releasetype"), tracktotal: r.i("tracktotal"), compilation: r.i("compilation") == 1 });
    }
    albums.into_iter().map(|(id, mut a)| {
        a.stored = stored.remove(&id);
        let c = classify(&a);
        (id, c)
    }).collect()
}

// ---- writing ---------------------------------------------------------------------------------------------

/// The facts about a release, as `album_types` keeps them.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    /// musicbrainz | deezer | apple | discogs
    pub source: &'static str,
    pub id: String,
    /// Album | EP | Single | Broadcast | Other ('' none given)
    pub primary: String,
    pub secondary: Vec<String>,
    pub tracks: i64,
}

/// The facts of a release a preview / import used (`meta::release`'s shape): MusicBrainz's group types, or a
/// store's record type ("Compile" is an album with the Compilation extra; " - Single" / " - EP" titles).
pub fn facts_of_release(rel: &Value, tracks: usize) -> Option<Facts> {
    let provider = s(&rel["provider"]);
    let count = rel["track_count"].as_i64().filter(|n| *n > 0).unwrap_or(tracks as i64);
    if provider.is_empty() || provider == "MusicBrainz" {
        let id = s(&rel["release_group_id"]).to_string();
        if id.is_empty() {
            return None;
        }
        let secondary = rel["secondary"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        return Some(Facts { source: "musicbrainz", id, primary: s(&rel["type"]).to_string(), secondary, tracks: count });
    }
    let source = match provider { "Deezer" => "deezer", "Apple Music" => "apple", "Discogs" => "discogs", _ => return None };
    let (primary, secondary) = match crate::catalog::kind_of(s(&rel["type"]), s(&rel["title"])) {
        "EP" => ("EP", vec![]),
        "Single" => ("Single", vec![]),
        "Compilation" => ("Album", vec!["Compilation".to_string()]),
        _ => ("Album", vec![]),
    };
    Some(Facts { source, id: s(&rel["id"]).to_string(), primary: primary.into(), secondary, tracks: count })
}

/// The facts of the release `mb::recording` picked for a lone song (`release_type`, `release_secondary`,
/// `release_track_count`, `release_group_id`).
pub fn facts_of_recording(rec: &Value) -> Option<Facts> {
    let id = s(&rec["release_group_id"]).to_string();
    if id.is_empty() {
        return None;
    }
    let secondary = rec["release_secondary"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    Some(Facts { source: "musicbrainz", id, primary: s(&rec["release_type"]).to_string(), secondary, tracks: rec["release_track_count"].as_i64().unwrap_or(0) })
}

/// Keeps the facts of these albums (the owner's choice stays): one statement per album, on a lent connection.
pub fn store_facts_in(c: &Conn, album_ids: &[String], f: &Facts) -> rusqlite::Result<()> {
    for a in album_ids.iter().filter(|a| !a.is_empty()) {
        c.execute(
            "INSERT INTO album_types (album_id, release_source, release_id, release_type, release_secondary, release_tracks, looked_up_at) VALUES (?,?,?,?,?,?,?)
             ON CONFLICT(album_id) DO UPDATE SET release_source=excluded.release_source, release_id=excluded.release_id, release_type=excluded.release_type,
                 release_secondary=excluded.release_secondary, release_tracks=excluded.release_tracks, looked_up_at=excluded.looked_up_at",
            p![a, f.source, f.id, f.primary, json!(f.secondary).to_string(), f.tracks, now()],
        )?;
    }
    Ok(())
}

/// [`store_facts_in`] in one transaction (one move of the library version).
pub fn store_facts(db: &Db, album_ids: &[String], f: &Facts) {
    if album_ids.is_empty() {
        return;
    }
    if let Err(e) = db.write_now(|c| c.transaction(|c| store_facts_in(c, album_ids, f))) {
        tracing::warn!("album types: keeping the release's facts: {e}");
    }
}

/// The albums these files are in.
pub fn albums_of_paths(db: &Db, paths: &[std::path::PathBuf]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for chunk in paths.chunks(500) {
        let marks = crate::artists::marks(chunk.len());
        for r in db.query(&format!("SELECT DISTINCT album_id FROM tracks WHERE path IN ({marks})"), chunk.iter().map(|p| json!(p.to_string_lossy())).collect()).unwrap_or_default() {
            let a = r.s("album_id");
            if !a.is_empty() && !out.contains(&a) {
                out.push(a);
            }
        }
    }
    out
}

/// The album these songs were in got a new id by Encore's own hand (Edit details, Tag / match, a folder of an
/// untagged album moved): when every song of `old` now has the one id `new` and `new` has no row yet, the
/// album's row (the owner's choice) follows it. `drop_facts`: a retag writes the new release's facts itself.
pub fn carry_in(c: &Conn, old: &str, new: &str, drop_facts: bool) -> rusqlite::Result<()> {
    if old.is_empty() || new.is_empty() || old == new {
        return Ok(());
    }
    let reset = if drop_facts { ", release_source=NULL, release_id=NULL, release_type=NULL, release_secondary=NULL, release_tracks=NULL, looked_up_at=NULL" } else { "" };
    c.execute(&format!("UPDATE album_types SET album_id=?{reset} WHERE album_id=? AND NOT EXISTS (SELECT 1 FROM album_types WHERE album_id=?)
                        AND NOT EXISTS (SELECT 1 FROM tracks WHERE album_id=?)"), p![new, old, new, old])?;
    Ok(())
}

/// [`carry_in`] for songs that were album `old` (by their ids, which a retag keeps): their one new album, if they
/// all have the same one.
pub fn carry(db: &Db, old: &str, track_ids: &[i64], drop_facts: bool) {
    if track_ids.is_empty() {
        return;
    }
    let marks = crate::artists::marks(track_ids.len().min(500));
    let ids: Vec<Value> = track_ids.iter().take(500).map(|i| json!(i)).collect();
    let now_in: Vec<String> = db.query(&format!("SELECT DISTINCT album_id FROM tracks WHERE id IN ({marks})"), ids).unwrap_or_default().iter().map(|r| r.s("album_id")).collect();
    if let [new] = now_in.as_slice() {
        if let Err(e) = db.write_now(|c| carry_in(c, old, new, drop_facts)) {
            tracing::warn!("album types: the owner's choice following album {old}: {e}");
        }
    }
}

/// Change type…: the owner's format for an album (None: automatic again).
pub fn choose(db: &Db, album_id: &str, format: Option<&str>, by: i64) -> rusqlite::Result<()> {
    db.execute("INSERT INTO album_types (album_id, chosen, chosen_by, chosen_at) VALUES (?,?,?,?)
                ON CONFLICT(album_id) DO UPDATE SET chosen=excluded.chosen, chosen_by=excluded.chosen_by, chosen_at=excluded.chosen_at",
        p![album_id, format, by, now()])?;
    Ok(())
}

// ---- the scheduled task "Sort albums by type" ------------------------------------------------------------

/// Songs whose type tags step A reads per transaction.
pub const TAG_BATCH: usize = 200;
/// Most MusicBrainz lookups of one run.
pub const LOOKUPS_PER_RUN: usize = 300;
/// Albums whose facts are written per transaction.
pub const WRITE_BATCH: usize = 25;
/// An album MusicBrainz didn't know is asked about again after this long.
pub const LOOK_AGAIN_AFTER: f64 = 30.0 * 86400.0;

/// What one run did.
#[derive(Default, Debug, PartialEq)]
pub struct Run {
    pub tags_read: usize,
    pub looked_up: usize,
    pub not_found: usize,
}

impl Run {
    /// The result line of Settings → Tasks.
    pub fn line(&self) -> String {
        format!("{} song tags read, {} albums looked up ({} not on MusicBrainz)", self.tags_read, self.looked_up, self.not_found)
    }
}

/// Step A: the type tags and artist credits of songs scanned before 0.36 (`releasetype` or `artist_credit` NULL),
/// [`TAG_BATCH`] songs per transaction, waiting while the app is in use. A file that can't be read gets
/// '' / 0 / 0 / '[]': it isn't read again.
pub fn read_missing_tags(db: &Db, busy: &encore_jobs::Busy) -> usize {
    let mut n = 0;
    loop {
        busy.wait_out_groups_blocking();
        busy.wait_while_in_use(std::time::Duration::from_secs(600));
        let rows = match db.query(&format!("SELECT id, path FROM tracks WHERE releasetype IS NULL OR artist_credit IS NULL ORDER BY id LIMIT {TAG_BATCH}"), p![]) {
            Ok(r) if !r.is_empty() => r,
            _ => break,
        };
        let read: Vec<(i64, [Value; 5])> = rows.iter().map(|r| {
            let t = encore_media::tags::read_tags(&r.s("path"));
            let get = |k: &str, d: Value| t.get(k).cloned().filter(|v| !v.is_null()).unwrap_or(d);
            (r.i("id"), [get("releasetype", json!("")), get("tracktotal", json!(0)), get("compilation", json!(0)),
                         get("artist_credit", json!("[]")), get("albumartist_credit", json!("[]"))])
        }).collect();
        let done = db.write_now(|c| c.transaction(|c| {
            for (id, [rt, tt, comp, ac, aac]) in &read {
                c.execute("UPDATE tracks SET releasetype=?, tracktotal=?, compilation=?, artist_credit=?, albumartist_credit=? WHERE id=?", p![rt, tt, comp, ac, aac, id])?;
            }
            Ok::<_, rusqlite::Error>(())
        }));
        if let Err(e) = done {
            tracing::warn!("album types: writing the songs' type tags: {e}");
            break;
        }
        n += read.len();
        if rows.len() < TAG_BATCH {
            break;
        }
    }
    n
}

/// An album step B may look up.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub album_id: String,
    pub album: String,
    pub songs: i64,
    pub rgid: String,
    pub recording: String,
    /// max(tracktotal, highest track number): the release group itself has no one count of songs
    pub total: i64,
}

/// The albums step B looks up, in order: the likely One-offs first (≤ 3 songs with a release group or a
/// recording id), then the other albums with a release group. Albums whose files have a RELEASETYPE tag, or
/// whose facts were taken (or MusicBrainz didn't know them less than 30 days ago), are left out.
pub fn candidates(db: &Db, limit: usize) -> Vec<Candidate> {
    let rows = db.query(
        "SELECT t.album_id, MAX(t.album) album, COUNT(*) n, MAX(COALESCE(t.mb_releasegroupid,'')) rgid, MAX(COALESCE(t.mb_trackid,'')) rec,
                MAX(COALESCE(t.track,0)) maxtrack, MAX(COALESCE(t.tracktotal,0)) total
         FROM tracks t LEFT JOIN album_types a ON a.album_id=t.album_id
         WHERE t.album_id IS NOT NULL AND (a.album_id IS NULL OR a.looked_up_at IS NULL OR (a.release_type IS NULL AND a.looked_up_at < ?))
         GROUP BY t.album_id
         HAVING SUM(CASE WHEN COALESCE(t.releasetype,'')<>'' THEN 1 ELSE 0 END)=0 AND (rgid<>'' OR (rec<>'' AND n<=3))
         ORDER BY CASE WHEN n<=3 THEN 0 ELSE 1 END, t.album_id",
        p![now() - LOOK_AGAIN_AFTER],
    ).unwrap_or_default();
    rows.into_iter().take(limit).map(|r| Candidate {
        album_id: r.s("album_id"), album: r.s("album"), songs: r.i("n"), rgid: r.s("rgid"), recording: r.s("rec"),
        total: r.i("maxtrack").max(r.i("total")),
    }).collect()
}

/// What a lookup found: the facts, MusicBrainz doesn't know it (Ok(None)), or it couldn't be asked (Err: busy,
/// no network – nothing is written, it is asked again next run).
pub async fn look_up(st: &impl crate::Ctx, c: &Candidate) -> anyhow::Result<Option<Facts>> {
    use encore_http::mb;
    if !c.rgid.is_empty() {
        return match mb::release_group(st, &c.rgid).await {
            Ok(g) => Ok(Some(Facts {
                source: "musicbrainz", id: c.rgid.clone(), primary: s(&g["type"]).to_string(),
                secondary: g["secondary"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                tracks: c.total,
            })),
            Err(e) if mb::is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        };
    }
    let releases = match mb::recording_releases(st, &c.recording).await {
        Ok(r) => r,
        Err(e) if mb::is_not_found(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    let key = crate::catalog::title_key(&c.album);
    let Some(rel) = releases.iter().find(|r| crate::catalog::title_key(s(&r["title"])) == key).or(releases.first()) else { return Ok(None) };
    let id = s(&rel["release_group_id"]).to_string();
    if id.is_empty() {
        return Ok(None);
    }
    Ok(Some(Facts {
        source: "musicbrainz", id, primary: s(&rel["type"]).to_string(),
        secondary: rel["secondary"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        tracks: rel["track_count"].as_i64().unwrap_or(0).max(c.total),
    }))
}

/// One looked-up album's answer, waiting to be written.
enum Found {
    Facts(String, Facts),
    Unknown(String),
}

fn write_found(db: &Db, found: &mut Vec<Found>) {
    if found.is_empty() {
        return;
    }
    let done = db.write_now(|c| c.transaction(|c| {
        for f in found.iter() {
            match f {
                Found::Facts(a, facts) => store_facts_in(c, std::slice::from_ref(a), facts)?,
                Found::Unknown(a) => {
                    c.execute("INSERT INTO album_types (album_id, release_type, looked_up_at) VALUES (?, NULL, ?)
                               ON CONFLICT(album_id) DO UPDATE SET release_type=NULL, release_source=NULL, looked_up_at=excluded.looked_up_at", p![a, now()])?;
                }
            }
        }
        Ok::<_, rusqlite::Error>(())
    }));
    if let Err(e) = done {
        tracing::warn!("album types: writing what MusicBrainz said: {e}");
    }
    found.clear();
}

/// Step B: at most `limit` albums looked up at MusicBrainz's background priority, written [`WRITE_BATCH`]
/// albums per transaction. → (looked up, not on MusicBrainz)
pub async fn look_up_missing(st: &impl crate::Ctx, limit: usize) -> (usize, usize) {
    let db = st.db().clone();
    let todo = tokio::task::spawn_blocking(move || candidates(&db, limit)).await.unwrap_or_default();
    let (mut looked, mut unknown) = (0, 0);
    let mut found: Vec<Found> = vec![];
    for c in todo {
        let res = encore_http::mb::with_priority(encore_http::mb::Priority::Background, look_up(st, &c)).await;
        match res {
            Ok(Some(f)) => {
                looked += 1;
                found.push(Found::Facts(c.album_id.clone(), f));
            }
            Ok(None) => {
                looked += 1;
                unknown += 1;
                found.push(Found::Unknown(c.album_id.clone()));
            }
            Err(e) => {
                tracing::info!("album types: {} wasn't looked up ({e}): next run", c.album);
                // MusicBrainz is busy or away: the rest waits for the next run
                if encore_http::mb::is_busy(&e) {
                    break;
                }
            }
        }
        if found.len() >= WRITE_BATCH {
            write_found(st.db(), &mut found);
        }
    }
    write_found(st.db(), &mut found);
    (looked, unknown)
}

/// The rows of albums the library no longer has, without an owner's choice (a choice stays: an album restored
/// from Trash gets it back).
pub fn forget_gone(db: &Db) {
    // only when there is such a row: a DELETE that removes nothing would still move the library version
    let gone = "chosen IS NULL AND album_id NOT IN (SELECT DISTINCT album_id FROM tracks WHERE album_id IS NOT NULL)";
    if db.one(&format!("SELECT 1 x FROM album_types WHERE {gone} LIMIT 1"), p![]).ok().flatten().is_some() {
        if let Err(e) = db.execute(&format!("DELETE FROM album_types WHERE {gone}"), p![]) {
            tracing::warn!("album types: forgetting albums that are gone: {e}");
        }
    }
}

/// The "Sort albums by type" task: step A (the files' type tags), step B (MusicBrainz for the albums without
/// any), then the rows of albums that are gone.
pub async fn scheduled(st: &impl crate::Ctx) -> anyhow::Result<String> {
    let (db, busy) = (st.db().clone(), st.library().busy.clone());
    let tags_read = tokio::task::spawn_blocking(move || {
        encore_jobs::busy::lower_this_thread();
        read_missing_tags(&db, &busy)
    }).await?;
    let (looked_up, not_found) = look_up_missing(st, LOOKUPS_PER_RUN).await;
    // and MusicBrainz's names of albums that share their name with another ("Blue Album")
    crate::split::look_up_names(st, LOOKUPS_PER_RUN).await;
    let db = st.db().clone();
    tokio::task::spawn_blocking(move || forget_gone(&db)).await?;
    let r = Run { tags_read, looked_up, not_found };
    if r.looked_up > 0 {
        st.db().log("info", &format!("Sort albums by type: {}", r.line()));
    }
    Ok(r.line())
}

#[cfg(test)]
#[path = "album_types_tests.rs"]
mod tests;
