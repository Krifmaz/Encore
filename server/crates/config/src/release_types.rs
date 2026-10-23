//! Release types (Settings → Downloads → Release types), like Lidarr's metadata profiles: which
//! kinds of release a discography request, the artist page and New releases count (Album, EP, Single …)
//! and which extras are skipped (live, remix, compilation, interview … and Encore's own instrumental,
//! acoustic, sped up, karaoke and covers, read from titles). MusicBrainz release groups carry their
//! secondary types; Deezer / Apple Music albums only have a record type and a title, so the extras are
//! read from the title words.
use encore_core::json::str_ref as s;
use encore_core::text::norm;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// MusicBrainz primary types: (key, label).
pub const KINDS: [(&str, &str); 5] = [("Album", "Album"), ("EP", "EP"), ("Single", "Single"), ("Broadcast", "Broadcast"), ("Other", "Other")];

/// Extras: MusicBrainz secondary types, then Encore's own (from titles and disambiguation). (key, label)
pub const EXTRAS: [(&str, &str); 17] = [
    ("Compilation", "Compilation"),
    ("Soundtrack", "Soundtrack"),
    ("Spokenword", "Spoken word"),
    ("Interview", "Interview"),
    ("Audiobook", "Audiobook"),
    ("Audio drama", "Audio drama"),
    ("Live", "Live"),
    ("Remix", "Remix"),
    ("DJ-mix", "DJ-mix"),
    ("Mixtape/Street", "Mixtape / Street"),
    ("Demo", "Demo"),
    ("Field recording", "Field recording"),
    ("Instrumental", "Instrumental"),
    ("Acoustic", "Acoustic"),
    ("Sped up", "Sped up / Slowed"),
    ("Karaoke", "Karaoke"),
    ("Covers", "Covers / Tributes"),
];

/// The release kinds requested when nothing is set.
pub const DEFAULT_KINDS: &str = "Album,EP";

/// Every extra is skipped by default: only plain studio releases are requested.
pub fn default_skip() -> String {
    EXTRAS.iter().map(|e| e.0).collect::<Vec<_>>().join(",")
}

/// A kind or extra as typed (CSV in an environment variable, a client's list) → its key.
pub fn canon(s: &str) -> Option<&'static str> {
    let n = norm(s);
    if n.is_empty() {
        return None;
    }
    if let Some(k) = KINDS.iter().find(|k| norm(k.0) == n) {
        return Some(k.0);
    }
    if let Some(e) = EXTRAS.iter().find(|e| norm(e.0) == n || norm(e.1) == n) {
        return Some(e.0);
    }
    Some(match n.as_str() {
        "compilations" => "Compilation",
        "soundtracks" | "ost" => "Soundtrack",
        "spoken" => "Spokenword",
        "interviews" | "commentary" => "Interview",
        "audiobooks" => "Audiobook",
        "remixes" | "remixed" => "Remix",
        "djmixes" => "DJ-mix",
        "mixtape" | "mixtapes" | "street" => "Mixtape/Street",
        "demos" => "Demo",
        "instrumentals" => "Instrumental",
        "spedup" | "slowed" | "slowedreverb" | "nightcore" => "Sped up",
        "cover" | "covers" | "tribute" | "tributes" => "Covers",
        "singles" => "Single",
        "albums" => "Album",
        "eps" => "EP",
        _ => return None,
    })
}

/// Whether `k` is one of [`KINDS`].
fn is_kind(k: &str) -> bool {
    KINDS.iter().any(|x| x.0 == k)
}

/// A list as CSV text or a JSON array → its keys of one family (kinds or extras), in table order.
fn keys(v: &Value, kinds: bool) -> Vec<&'static str> {
    let raw: Vec<String> = match v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        Value::String(s) => s.split(',').map(String::from).collect(),
        _ => vec![],
    };
    let found: Vec<&'static str> = raw.iter().filter_map(|r| canon(r)).filter(|k| is_kind(k) == kinds).collect();
    let order: Vec<&'static str> = if kinds { KINDS.iter().map(|k| k.0).collect() } else { EXTRAS.iter().map(|e| e.0).collect() };
    order.into_iter().filter(|k| found.contains(k)).collect()
}

/// What is wanted: these kinds, without releases that carry any of `skip`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseFilter {
    /// the kinds wanted ("Album", "EP", …)
    pub kinds: Vec<&'static str>,
    /// the extras that rule a release out ("Live", "Remix", …)
    pub skip: Vec<&'static str>,
}

impl Default for ReleaseFilter {
    fn default() -> Self {
        ReleaseFilter { kinds: keys(&json!(DEFAULT_KINDS), true), skip: keys(&json!(default_skip()), false) }
    }
}

impl ReleaseFilter {
    /// From the two settings (CSV). An empty kinds list falls back to the default (a filter that
    /// wants nothing would make "Request discography" do nothing without saying why).
    pub fn from_csv(kinds: &str, skip: &str) -> Self {
        let k = keys(&json!(kinds), true);
        ReleaseFilter { kinds: if k.is_empty() { Self::default().kinds } else { k }, skip: keys(&json!(skip), false) }
    }

    /// The filter the `release_types` and `release_skip` settings describe.
    pub fn from_cfg(cfg: &crate::Config) -> Self {
        Self::from_csv(&cfg.str("release_types"), &cfg.str("release_skip"))
    }

    /// An artist's own release types (`artist_release_types`, set from Request discography's "Use these for this
    /// artist from now on"), when they have them.
    pub fn artist_own(cfg: &crate::Config, mbid: &str) -> Option<Self> {
        use encore_db::RowExt;
        let r = cfg.db.one("SELECT kinds, skip FROM artist_release_types WHERE mbid=?", encore_db::p![mbid]).ok().flatten()?;
        Some(Self::from_value(&json!({ "kinds": r.s("kinds"), "skip": r.s("skip") }), &Self::from_cfg(cfg)))
    }

    /// What counts for this artist: their own release types, else the setting's.
    pub fn for_artist(cfg: &crate::Config, mbid: &str) -> Self {
        Self::artist_own(cfg, mbid).unwrap_or_else(|| Self::from_cfg(cfg))
    }

    /// Keeps `f` as the artist's own release types, or (None) goes back to the setting's for them.
    pub fn save_for_artist(cfg: &crate::Config, mbid: &str, f: Option<&Self>) -> anyhow::Result<()> {
        match f {
            Some(f) => cfg.db.execute("INSERT OR REPLACE INTO artist_release_types (mbid, kinds, skip, updated_at) VALUES (?,?,?,?)",
                encore_db::p![mbid, f.kinds.join(","), f.skip.join(","), encore_db::now()])?,
            None => cfg.db.execute("DELETE FROM artist_release_types WHERE mbid=?", encore_db::p![mbid])?,
        };
        Ok(())
    }

    /// A request's own filter ({"kinds": [...], "skip": [...]}, arrays or CSV); what's missing comes from `base`.
    pub fn from_value(v: &Value, base: &ReleaseFilter) -> Self {
        let mut f = base.clone();
        if !v["kinds"].is_null() {
            let k = keys(&v["kinds"], true);
            if !k.is_empty() {
                f.kinds = k;
            }
        }
        if !v["skip"].is_null() {
            f.skip = keys(&v["skip"], false);
        }
        f
    }

    /// The filter as the API sends it.
    pub fn to_value(&self) -> Value {
        json!({ "kinds": self.kinds, "skip": self.skip })
    }

    /// For the clients' "Request discography" panel: every box with its label and whether it is ticked.
    pub fn options(&self) -> Value {
        json!({
            "kinds": KINDS.iter().map(|(k, l)| json!({ "key": k, "label": l, "on": self.kinds.contains(k) })).collect::<Vec<_>>(),
            "skip": EXTRAS.iter().map(|(k, l)| json!({ "key": k, "label": l, "on": self.skip.contains(k) })).collect::<Vec<_>>(),
        })
    }

    /// Why a release (MusicBrainz group or Deezer / Apple album) is left out, or None when it is wanted:
    /// "Single", "Live", "Remix" … (the first reason).
    pub fn reason(&self, rg: &Value) -> Option<&'static str> {
        let p = primary(rg);
        if !self.kinds.contains(&p) {
            return Some(p);
        }
        marks(rg).into_iter().find(|m| self.skip.contains(m))
    }

    /// Whether a release (MusicBrainz group, or Deezer / Apple album) is wanted.
    pub fn wants(&self, rg: &Value) -> bool {
        self.reason(rg).is_none()
    }
}

/// A Deezer / Apple album (no MusicBrainz secondary types to go by).
fn is_ext(rg: &Value) -> bool {
    rg["ext"].is_object() || rg.get("secondary").is_none()
}

/// The primary type: MusicBrainz's, or the record type of an ext album ("Compilation" there is an
/// album with the Compilation extra). No type at all is "Other".
pub fn primary(rg: &Value) -> &'static str {
    match canon(s(&rg["type"])) {
        Some(k) if is_kind(k) => k,
        Some("Compilation") => "Album",
        _ if s(&rg["type"]).is_empty() => "Other",
        _ => "Album",
    }
}

/// The order the clients show release groups in (the artist page's sections and the Request discography
/// dialog); kinds not listed come after, in `EXTRAS` order.
pub const GROUP_ORDER: [&str; 15] = ["Album", "EP", "Single", "Compilation", "Live", "Soundtrack", "Remix", "DJ-mix", "Mixtape/Street", "Demo", "Other", "Broadcast", "Interview", "Spokenword", "Audiobook"];

/// Where a group goes in the clients' order (`GROUP_ORDER`, then the other extras).
pub fn group_rank(kind: &str) -> usize {
    GROUP_ORDER.iter().position(|k| *k == kind)
        .or_else(|| EXTRAS.iter().position(|e| e.0 == kind).map(|i| GROUP_ORDER.len() + i))
        .unwrap_or(GROUP_ORDER.len() + EXTRAS.len())
}

/// The label of a kind or extra ("Mixtape / Street" for "Mixtape/Street"); an unknown key is its own label.
pub fn label(kind: &str) -> &str {
    KINDS.iter().chain(EXTRAS.iter()).find(|(k, _)| *k == kind).map(|(_, l)| *l).unwrap_or(kind)
}

/// The group a release is shown under – its canonical kind: the first MusicBrainz secondary type (Live,
/// Compilation, Soundtrack …), else, for a Deezer / Apple album, the first extra its record type and title
/// say (Compilation, Live, Remix …), else the primary type (Album, EP, Single, Broadcast, Other). The same
/// key the Release types settings use, so a client can tell which group a setting is about.
pub fn group_kind(rg: &Value) -> &'static str {
    if let Some(k) = rg["secondary"].as_array().and_then(|a| a.iter().find_map(|x| canon(s(x)).filter(|k| !is_kind(k)))) {
        return k;
    }
    if is_ext(rg) {
        if let Some(m) = marks(rg).into_iter().next() {
            return m;
        }
    }
    primary(rg)
}

/// A case-insensitive regex (the patterns are fixed, so a bad one is a bug).
fn re(p: &str) -> Regex {
    Regex::new(&format!("(?i){p}")).unwrap()
}


/// Encore's own extras, from the title and disambiguation of any release.
static OWN: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| vec![
    (re(r"\binstrumentals?\b|\boff[\s-]?vocal\b"), "Instrumental"),
    (re(r"\bacoustic\b|\bunplugged\b|\bstripped\b"), "Acoustic"),
    (re(r"\bsped[\s-]*up\b|\bslowed\b|\bnightcore\b|\bdaycore\b|\b8d audio\b"), "Sped up"),
    (re(r"\bkaraoke\b|\bbacking tracks?\b|\bin the style of\b"), "Karaoke"),
    (re(r"\bcovers\b|[\(\[][^\)\]]*\bcover\b[^\)\]]*[\)\]]|\btributes?\b|\bcover versions?\b"), "Covers"),
]);

/// What MusicBrainz would call it, read from a Deezer / Apple title. "Remastered" is not a remix and
/// "Deluxe" is the album itself.
static EXT: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| vec![
    (re(r"[\(\[][^\)\]]*\blive\b[^\)\]]*[\)\]]|\s-\s+live\b|^live (at|in|from|on)\b|\blive (at|from)\b|\bin concert\b|\bunplugged\b"), "Live"),
    (re(r"\bremix(es|ed)?\b|\brmx\b|\bremixe?s\b"), "Remix"),
    (re(r"\binterviews?\b|\bcommentary\b|\btrack[\s-]by[\s-]track\b"), "Interview"),
    (re(r"\bdemos?\b"), "Demo"),
    (re(r"\bsoundtrack\b|\boriginal (motion picture|score|cast|series)\b|\bost\b"), "Soundtrack"),
    (re(r"\bdj[\s-]?mix\b|\bmixed by\b|\bcontinuous mix\b"), "DJ-mix"),
    (re(r"\bmixtape\b"), "Mixtape/Street"),
    (re(r"\baudiobook\b|\bunabridged\b"), "Audiobook"),
    (re(r"\bspoken word\b"), "Spokenword"),
    (re(r"\bgreatest hits\b|\bthe best of\b|\bthe essential\b"), "Compilation"),
]);

/// Every extra a release carries: MusicBrainz secondary types (or, for ext albums, what the record type
/// and title say) plus Encore's own from the title and disambiguation.
pub fn marks(rg: &Value) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = vec![];
    let mut add = |m: &'static str| if !out.contains(&m) { out.push(m) };
    for x in rg["secondary"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        if let Some(k) = canon(s(x)).filter(|k| !is_kind(k)) {
            add(k);
        }
    }
    let text = format!("{} {}", s(&rg["title"]), s(&rg["disambiguation"]));
    let text = text.trim();
    if is_ext(rg) {
        if canon(s(&rg["type"])) == Some("Compilation") {
            add("Compilation");
        }
        for (r, m) in EXT.iter() {
            if r.is_match(text) {
                add(m);
            }
        }
    }
    // a MusicBrainz title is the release's real name ("Stripped", "Tribute", "Acoustic"): only the
    // version part of it counts – brackets, a trailing " - …" – and the disambiguation
    let own_text = if is_ext(rg) { text.to_string() } else { version_part(rg) };
    for (r, m) in OWN.iter() {
        if r.is_match(&own_text) {
            add(m);
        }
    }
    out
}

/// A bracketed part of a title: `(…)` or `[…]`.
static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[][^\)\]]*[\)\]]").unwrap());

/// The bracketed and trailing " - …" parts of a title, plus the disambiguation.
fn version_part(rg: &Value) -> String {
    let title = s(&rg["title"]);
    let mut parts: Vec<&str> = BRACKETS.find_iter(title).map(|m| m.as_str()).collect();
    if let Some((_, tail)) = title.split_once(" - ") {
        parts.push(tail);
    }
    parts.push(s(&rg["disambiguation"]));
    parts.join(" ")
}

/// Installs from before release types existed: "discography includes singles" (on by default, from the
/// DB or ENCORE_DISCOGRAPHY_SINGLES) becomes the Single box.
/// → the release_types value to store, when one has to be written.
pub fn migrated_kinds(stored_types: Option<&Value>, singles: Option<&Value>) -> Option<String> {
    if stored_types.is_some() {
        return None;
    }
    let on = match singles {
        None | Some(Value::Null) => true, // never saved: the old default
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        _ => false,
    };
    on.then(|| format!("{DEFAULT_KINDS},Single"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mb(title: &str, typ: &str, sec: &[&str]) -> Value {
        json!({ "id": "rg", "title": title, "type": typ, "secondary": sec })
    }
    fn ext(title: &str, typ: &str) -> Value {
        json!({ "id": "ext-d1", "title": title, "type": typ, "ext": { "deezer": 1 } })
    }

    #[test]
    fn defaults_keep_only_studio_albums_and_eps() {
        let f = ReleaseFilter::default();
        assert_eq!(f.kinds, vec!["Album", "EP"]);
        assert_eq!(f.skip.len(), EXTRAS.len());
        assert!(f.wants(&mb("Rodeo", "Album", &[])));
        assert!(f.wants(&mb("Some EP", "EP", &[])));
        assert_eq!(f.reason(&mb("90210", "Single", &[])), Some("Single"));
        assert_eq!(f.reason(&mb("Live at Rolling Loud", "Album", &["Live"])), Some("Live"));
        assert_eq!(f.reason(&mb("Hits", "Album", &["Compilation"])), Some("Compilation"));
        assert_eq!(f.reason(&mb("Rodeo (Instrumentals)", "Album", &[])), Some("Instrumental"));
        assert_eq!(f.reason(&json!({ "title": "Rodeo", "type": "Album", "secondary": [], "disambiguation": "instrumental version" })), Some("Instrumental"));
        assert_eq!(f.reason(&mb("Thing", "", &[])), Some("Other"));
    }

    #[test]
    fn mb_types_are_filtered_by_the_ticked_boxes() {
        let f = ReleaseFilter::from_csv("Album,EP,Single", "Compilation,Live");
        assert!(f.wants(&mb("90210", "Single", &[])));
        assert!(f.wants(&mb("Remixes", "Album", &["Remix"])), "remix is not ticked, so it is kept");
        assert!(!f.wants(&mb("Live", "Album", &["Live"])));
        assert!(!f.wants(&mb("Box", "Album", &["Compilation", "Live"])));
        // MusicBrainz's own words, any case, aliases
        let f = ReleaseFilter::from_csv("album, singles", "dj mix, Mixtape, spoken word, sped up / slowed");
        assert_eq!(f.kinds, vec!["Album", "Single"]);
        assert_eq!(f.skip, vec!["Spokenword", "DJ-mix", "Mixtape/Street", "Sped up"]);
        assert!(!f.wants(&mb("Tape", "Album", &["Mixtape/Street"])));
        assert!(!f.wants(&mb("Song (Sped Up)", "Single", &[])));
        // nothing ticked to keep: the default, never "nothing at all"
        assert_eq!(ReleaseFilter::from_csv("", "").kinds, vec!["Album", "EP"]);
        assert!(ReleaseFilter::from_csv("Album", "").wants(&mb("Live", "Album", &["Live"])));
    }

    #[test]
    fn ext_albums_are_read_from_record_type_and_title() {
        let f = ReleaseFilter::default();
        for (title, why) in [
            ("Rodeo (Live at Coachella)", "Live"),
            ("Live at Wembley", "Live"),
            ("Some Song - Live", "Live"),
            ("Astroworld (The Remixes)", "Remix"),
            ("Remixed", "Remix"),
            ("Track by Track Commentary", "Interview"),
            ("An Interview with Travis", "Interview"),
            ("Song (Karaoke Version)", "Karaoke"),
            ("Rodeo (Instrumentals)", "Instrumental"),
            ("Blinding Lights (Sped Up)", "Sped up"),
            ("Blinding Lights (Slowed + Reverb)", "Sped up"),
            ("Acoustic Sessions", "Acoustic"),
            ("Early Demos", "Demo"),
            ("Dune (Original Motion Picture Soundtrack)", "Soundtrack"),
            ("A Tribute to Travis Scott", "Covers"),
            ("Greatest Hits", "Compilation"),
        ] {
            assert_eq!(f.reason(&ext(title, "Album")), Some(why), "{title}");
        }
        // Deezer's "compile" record type
        assert_eq!(f.reason(&ext("Hits", "Compilation")), Some("Compilation"));
        assert_eq!(primary(&ext("Hits", "Compilation")), "Album");
        // kept: remastered is not a remix, deluxe is the album, "live" inside a title is not a live album
        for title in ["Rodeo (Remastered)", "Rodeo (Deluxe)", "Live Through This", "Alive", "Days Before Rodeo", "Coverage"] {
            assert!(f.wants(&ext(title, "Album")), "{title}");
        }
        assert_eq!(f.reason(&ext("Utopia", "Single")), Some("Single"));
        // live words in an MusicBrainz title are left to MusicBrainz's own types
        assert!(f.wants(&mb("Live at Wembley", "Album", &[])));
        // a MusicBrainz title that is just a word is the album's name, not a version
        for title in ["Stripped", "Tribute", "Unplugged", "Acoustic", "Covers"] {
            assert!(f.wants(&mb(title, "Album", &[])), "{title}");
        }
        assert_eq!(f.reason(&mb("Stripped (Acoustic Version)", "Album", &[])), Some("Acoustic"));
        assert_eq!(f.reason(&mb("Songs - Unplugged", "Album", &[])), Some("Acoustic"));
    }

    #[test]
    fn a_requests_own_filter_overrides_the_setting() {
        let base = ReleaseFilter::default();
        let f = ReleaseFilter::from_value(&json!({ "kinds": ["Album", "Single"], "skip": ["Live"] }), &base);
        assert_eq!(f.kinds, vec!["Album", "Single"]);
        assert_eq!(f.skip, vec!["Live"]);
        assert!(f.wants(&mb("Hits", "Album", &["Compilation"])));
        // missing parts come from the setting; an empty skip list skips nothing
        let f = ReleaseFilter::from_value(&json!({ "skip": [] }), &base);
        assert_eq!(f.kinds, base.kinds);
        assert!(f.skip.is_empty());
        assert_eq!(ReleaseFilter::from_value(&Value::Null, &base), base);
        assert_eq!(ReleaseFilter::from_value(&f.to_value(), &base), f);
        let o = base.options();
        assert_eq!(o["kinds"].as_array().unwrap().len(), 5);
        assert_eq!(o["kinds"][0], json!({ "key": "Album", "label": "Album", "on": true }));
        assert_eq!(o["kinds"][2]["on"], false);
        assert_eq!(o["skip"].as_array().unwrap().len(), EXTRAS.len());
    }

    #[test]
    fn releases_are_grouped_by_their_canonical_kind() {
        // MusicBrainz: the first secondary type, else the primary type (the artist page's sections)
        assert_eq!(group_kind(&mb("Rodeo", "Album", &[])), "Album");
        assert_eq!(group_kind(&mb("90210", "Single", &[])), "Single");
        assert_eq!(group_kind(&mb("Live at Rolling Loud", "Album", &["Live"])), "Live");
        assert_eq!(group_kind(&mb("Box", "Album", &["Compilation", "Live"])), "Compilation");
        assert_eq!(group_kind(&mb("Tape", "Album", &["Mixtape/Street"])), "Mixtape/Street");
        assert_eq!(group_kind(&mb("Thing", "", &[])), "Other");
        // a MusicBrainz title's words don't move it: "Rodeo (Instrumentals)" stays an album, only unticked
        assert_eq!(group_kind(&mb("Rodeo (Instrumentals)", "Album", &[])), "Album");
        // Deezer / Apple: the record type and the title words
        assert_eq!(group_kind(&ext("Hits", "Compilation")), "Compilation");
        assert_eq!(group_kind(&ext("Rodeo (Live at Coachella)", "Album")), "Live");
        assert_eq!(group_kind(&ext("Astroworld (The Remixes)", "Album")), "Remix");
        assert_eq!(group_kind(&ext("Days Before Rodeo", "Album")), "Album");
        assert_eq!(group_kind(&ext("Utopia", "Single")), "Single");
        assert_eq!(group_kind(&ext("Hello - EP", "EP")), "EP");
        // the clients' order, then the other extras, then anything unknown
        assert!(group_rank("Album") < group_rank("EP") && group_rank("EP") < group_rank("Single"));
        assert!(group_rank("Single") < group_rank("Compilation") && group_rank("Compilation") < group_rank("Live"));
        assert!(group_rank("Audiobook") < group_rank("Instrumental") && group_rank("Instrumental") < group_rank("Covers"));
        assert!(group_rank("Covers") < group_rank("Whatever"));
        assert_eq!(label("Mixtape/Street"), "Mixtape / Street");
        assert_eq!(label("Spokenword"), "Spoken word");
        assert_eq!(label("EP"), "EP");
        assert_eq!(label("Whatever"), "Whatever");
    }

    #[test]
    fn discography_singles_migrates_to_the_single_box() {

        assert_eq!(migrated_kinds(None, Some(&json!(true))).as_deref(), Some("Album,EP,Single"));
        assert_eq!(migrated_kinds(None, Some(&json!("true"))).as_deref(), Some("Album,EP,Single"));
        assert_eq!(migrated_kinds(None, Some(&json!(false))), None);
        // never saved: the old switch defaulted to on
        assert_eq!(migrated_kinds(None, None).as_deref(), Some("Album,EP,Single"));
        // once release types were saved, the old switch no longer matters
        assert_eq!(migrated_kinds(Some(&json!("Album")), Some(&json!(true))), None);
    }
}
