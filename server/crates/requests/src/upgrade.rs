//! Quality upgrades: how good the copy of an album in the library is (its [`Tier`]), and asking for a better
//! one – lossless for an album held only as MP3/YouTube rips, hi-res (24-bit or above 48 kHz) for one held as
//! CD-quality FLAC. An upgrade is an ordinary album request with `lossless_only` (and `hires_only` for hi-res)
//! in its detail and no user: the worker tries only the sources that can give that quality, and the import
//! replaces the old files with the better ones (playlists, likes and plays follow). The album page's
//! ⋯ → Upgrade quality asks for one album ([`queue_album`]); the Hi-res upgrades task and Settings →
//! Auto-download → Upgrade now go through the whole library ([`queue_hires_upgrades`], next to
//! [`crate::upkeep::queue_lossy_upgrades`]).
use crate::Ctx;
use encore_core::audio::LOSSLESS;
use encore_db::{now, p, Row, RowExt};
use serde_json::{json, Value};

/// The highest sample rate (Hz) that still counts as CD/standard resolution: above it a file is hi-res.
pub const STANDARD_MAX_RATE: i64 = 48_000;
/// The highest bit depth that still counts as CD resolution: above it a file is hi-res.
pub const STANDARD_MAX_BITS: i64 = 16;
/// The streamrip quality a hi-res upgrade downloads at ("Hi-res max"), whatever Settings → Sources says.
pub const HIRES_STREAMRIP_QUALITY: i64 = 4;
/// The request statuses of an upgrade still under way (a second one for the same album is not queued).
const ACTIVE: &str = "'queued','searching','downloading','monitoring','importing','waiting','pending'";

/// How good a copy is, worst to best (the order compares: `Lossy < Lossless < HiRes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// MP3, AAC, Opus, a YouTube rip… – what was cut away is gone
    Lossy,
    /// lossless at CD resolution or below (16-bit, ≤ 48 kHz)
    Lossless,
    /// lossless above CD resolution: 24-bit or more, or above 48 kHz
    HiRes,
}

impl Tier {
    /// The tier of one file from its format (extension, lower case), bit depth and sample rate (Hz). A lossless
    /// format whose depth and rate are unknown counts as plain lossless: only a file known to be hi-res is one.
    pub fn of(format: &str, bit_depth: Option<i64>, sample_rate: Option<i64>) -> Tier {
        if !LOSSLESS.contains(&format.to_lowercase().as_str()) {
            return Tier::Lossy;
        }
        if bit_depth.is_some_and(|b| b > STANDARD_MAX_BITS) || sample_rate.is_some_and(|r| r > STANDARD_MAX_RATE) {
            Tier::HiRes
        } else {
            Tier::Lossless
        }
    }

    /// The tier of a `tracks` row (its `format`, `bit_depth` and `sample_rate` columns).
    pub fn of_row(r: &Row) -> Tier {
        Tier::of(&r.s("format"), r.get("bit_depth").and_then(Value::as_i64), r.get("sample_rate").and_then(Value::as_i64))
    }

    /// Its key in the API: "lossy", "lossless" or "hires".
    pub fn key(self) -> &'static str {
        match self {
            Tier::Lossy => "lossy",
            Tier::Lossless => "lossless",
            Tier::HiRes => "hires",
        }
    }

    /// Its words on a badge: "Lossy", "Lossless", "Hi-Res".
    pub fn label(self) -> &'static str {
        match self {
            Tier::Lossy => "Lossy",
            Tier::Lossless => "Lossless",
            Tier::HiRes => "Hi-Res",
        }
    }

    /// The tier an upgrade from here asks for: lossless for a lossy copy, hi-res for a lossless one, None
    /// when it is hi-res already (nothing better to ask for).
    pub fn next(self) -> Option<Tier> {
        match self {
            Tier::Lossy => Some(Tier::Lossless),
            Tier::Lossless => Some(Tier::HiRes),
            Tier::HiRes => None,
        }
    }
}

/// The quality of an album's songs for its page (`quality_summary`): `{tier, label, line, upgrade}`. The
/// album is as good as its worst song (one MP3 among FLACs makes it lossy, one 16-bit song among 24-bit
/// ones makes it lossless); `line` describes that song ("FLAC · 16-bit / 44.1 kHz", "MP3 · 320 kbps"), and
/// `upgrade` is the tier ⋯ → Upgrade quality would ask for (null when hi-res already). Null for an album
/// without songs.
pub fn album_quality(rows: &[Row]) -> Value {
    let Some(worst) = rows.iter().min_by_key(|r| (Tier::of_row(r), r.i("bit_depth"), r.i("sample_rate"), r.i("bitrate"))) else {
        return Value::Null;
    };
    let tier = Tier::of_row(worst);
    json!({
        "tier": tier.key(),
        "label": tier.label(),
        "line": line(worst),
        "upgrade": tier.next().map(Tier::key),
    })
}

/// "FLAC · 24-bit / 96 kHz", "FLAC · 44.1 kHz", "MP3 · 320 kbps" or "MP3": a song's format and resolution.
pub fn line(r: &Row) -> String {
    let mut parts = vec![r.s("format").to_uppercase()];
    let (bits, rate, kbps) = (r.i("bit_depth"), r.i("sample_rate"), r.i("bitrate"));
    if Tier::of_row(r) != Tier::Lossy {
        match (bits > 0, rate > 0) {
            (true, true) => parts.push(format!("{bits}-bit / {} kHz", khz(rate))),
            (false, true) => parts.push(format!("{} kHz", khz(rate))),
            _ => {}
        }
    } else if kbps > 0 {
        parts.push(format!("{kbps} kbps"));
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

/// Hz as kHz without trailing zeros: 44100 → "44.1", 96000 → "96".
fn khz(hz: i64) -> String {
    let s = format!("{:.1}", hz as f64 / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Whether downloaded files are a hi-res copy: at least one is lossless above CD resolution. A hi-res upgrade
/// turns down a download that isn't (the service only had the CD version), instead of importing a copy no
/// better than the one in the library.
pub fn files_are_hires(files: &[std::path::PathBuf]) -> bool {
    files.iter().any(|f| {
        let path = f.to_string_lossy();
        let (_, facts) = encore_media::tags::read_tags_and_facts(&path);
        let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        Tier::of(&ext, facts.bit_depth.map(i64::from), facts.sample_rate.map(i64::from)) == Tier::HiRes
    })
}

/// Whether a hi-res upgrade can be looked for: streamrip is set up with Qobuz or Tidal among its services
/// (Deezer stops at CD quality, so it alone can't give one).
pub fn hires_source(st: &impl Ctx) -> bool {
    use crate::sources::streamrip;
    streamrip::enabled(st) && streamrip::services(st).iter().any(|s| s == "qobuz" || s == "tidal")
}

/// Whether a request is a hi-res upgrade (its detail has `hires_only`).
pub fn is_hires(detail: &Value) -> bool {
    detail["hires_only"].as_bool().unwrap_or(false)
}

/// What [`queue_album`] did.
#[derive(Debug, PartialEq)]
pub enum Queued {
    /// a request was queued for this tier (its id)
    Requested(i64, Tier),
    /// an upgrade of this album is already under way (its request id)
    Running(i64),
    /// the album is hi-res already
    Best,
    /// the album isn't matched to a MusicBrainz release group, so there is nothing exact to look for
    Unmatched,
    /// a hi-res upgrade needs streamrip with Qobuz or Tidal ([`hires_source`]), which isn't set up
    NoHiresSource,
    /// no such album
    NotFound,
}

/// The release group, artist, title and year of an album, from its MusicBrainz-tagged songs.
fn album_facts(rows: &[Row]) -> Option<(String, String, String, String)> {
    let f = rows.iter().find(|r| !r.s("mb_releasegroupid").is_empty())?;
    let artist = if f.s("albumartist").is_empty() { f.s("artist") } else { f.s("albumartist") };
    Some((f.s("mb_releasegroupid"), artist, f.s("album"), f.s("year")))
}

/// Queue an upgrade request for an album: lossless for a lossy one, hi-res for a lossless one.
fn insert_upgrade(st: &impl Ctx, rg: &str, artist: &str, album: &str, year: &str, want: Tier) -> anyhow::Result<i64> {
    let t = now();
    let (message, detail) = match want {
        Tier::HiRes => ("Hi-res upgrade", json!({ "lossless_only": true, "hires_only": true })),
        _ => ("Lossless upgrade", json!({ "lossless_only": true })),
    };
    // execute answers the new row's id
    Ok(st.db().execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, message, detail, created_at, updated_at)
                   VALUES (NULL,'album',?,?,?,?,?,?,'queued',?,?,?,?)",
        p![rg, artist, album, album, year, encore_http::mb::cover_rg(rg, 250), message, detail.to_string(), t, t])?)
}

/// The request already working on this release group, if any.
fn running(st: &impl Ctx, rg: &str) -> anyhow::Result<Option<i64>> {
    Ok(st.db().one(&format!("SELECT id FROM requests WHERE mbid=? AND status IN ({ACTIVE})"), p![rg])?.map(|r| r.i("id")))
}

/// ⋯ → Upgrade quality on an album page: ask for the next better copy of library album `album_id` right away
/// (lossless when it is lossy, hi-res when it is lossless). Its next turn in the Hi-res upgrades task is put
/// off by the retry interval, so the task doesn't ask again for what was just asked.
pub fn queue_album(st: &impl Ctx, album_id: &str) -> anyhow::Result<Queued> {
    let rows = st.db().query("SELECT format, bit_depth, sample_rate, bitrate, mb_releasegroupid, albumartist, artist, album, year FROM tracks WHERE album_id=?", p![album_id])?;
    if rows.is_empty() {
        return Ok(Queued::NotFound);
    }
    let Some(tier) = rows.iter().map(Tier::of_row).min() else { return Ok(Queued::NotFound) };
    let Some(want) = tier.next() else { return Ok(Queued::Best) };
    let Some((rg, artist, album, year)) = album_facts(&rows) else { return Ok(Queued::Unmatched) };
    if want == Tier::HiRes && !hires_source(st) {
        return Ok(Queued::NoHiresSource);
    }
    if let Some(id) = running(st, &rg)? {
        return Ok(Queued::Running(id));
    }
    // the scheduled tasks count from now for this album
    let scope = if want == Tier::HiRes { format!("hires:{rg}") } else { format!("upg:{rg}") };
    st.db().execute("INSERT OR REPLACE INTO sync_state VALUES (0,?,?)", p![scope, now().to_string()])?;
    let id = insert_upgrade(st, &rg, &artist, &album, &year, want)?;
    st.requests().wake.notify_one(); // asked for by hand: the queue starts on it now, not on its next 5 s tick
    st.db().log("info", &format!("Queued a {} upgrade of {artist} – {album}", if want == Tier::HiRes { "hi-res" } else { "lossless" }));
    Ok(Queued::Requested(id, want))
}

/// The SQL that picks the MusicBrainz-tagged albums whose every song is lossless at CD resolution or below
/// (none lossy, none hi-res): the albums a hi-res upgrade is for.
const CD_ALBUMS: &str = "SELECT mb_releasegroupid rg, MAX(albumartist) artist, MAX(album) album, MAX(year) year FROM tracks
    WHERE mb_releasegroupid IS NOT NULL AND mb_releasegroupid != '' GROUP BY album_id
    HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','aif','ape','wv') THEN 0 ELSE 1 END) = 0
       AND MAX(COALESCE(bit_depth, 0)) <= 16 AND MAX(COALESCE(sample_rate, 0)) <= 48000";

/// The number of albums a hi-res upgrade could improve (Settings → Auto-download's count).
pub fn cd_quality_albums(st: &impl Ctx) -> anyhow::Result<i64> {
    Ok(st.db().one(&format!("SELECT COUNT(*) n FROM ({CD_ALBUMS})"), p![])?.map(|r| r.i("n")).unwrap_or(0))
}

/// The Hi-res upgrades task (and Upgrade now): queue up to `limit` hi-res upgrades of CD-quality albums. An
/// album is asked about again only after `upgrade_lossy_days` (the same retry interval as lossless upgrades),
/// and never while a request for it runs. Nothing is queued without streamrip on Qobuz or Tidal
/// ([`hires_source`]): that is where hi-res releases come from. → how many were queued
pub fn queue_hires_upgrades(st: &impl Ctx, limit: usize) -> anyhow::Result<usize> {
    if !hires_source(st) {
        return Ok(0);
    }
    let days = st.cfg().int("upgrade_lossy_days").max(1) as f64;
    let rows = st.db().query(CD_ALBUMS, p![])?;
    let t = now();
    let mut n = 0;
    for a in rows {
        if n >= limit {
            break;
        }
        let rg = a.s("rg");
        let scope = format!("hires:{rg}");
        if let Some(last) = st.db().one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", p![scope])? {
            if t - last.s("value").parse::<f64>().unwrap_or(0.0) < days * 86400.0 {
                continue;
            }
        }
        if running(st, &rg)?.is_some() {
            continue;
        }
        st.db().execute("INSERT OR REPLACE INTO sync_state VALUES (0,?,?)", p![scope, t.to_string()])?;
        insert_upgrade(st, &rg, &a.s("artist"), &a.s("album"), &a.s("year"), Tier::HiRes)?;
        n += 1;
    }
    if n > 0 {
        st.db().log("info", &format!("Queued {n} album(s) for a hi-res upgrade"));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `tracks` row with this format, bit depth, sample rate and bitrate (0 = unknown).
    fn row(format: &str, bits: i64, rate: i64, kbps: i64) -> Row {
        let mut r = Row::new();
        r.insert("format".into(), json!(format));
        r.insert("bit_depth".into(), if bits > 0 { json!(bits) } else { Value::Null });
        r.insert("sample_rate".into(), if rate > 0 { json!(rate) } else { Value::Null });
        r.insert("bitrate".into(), json!(kbps));
        r
    }

    #[test]
    fn tiers_of_files() {
        assert_eq!(Tier::of("mp3", None, Some(44_100)), Tier::Lossy);
        assert_eq!(Tier::of("m4a", Some(24), Some(96_000)), Tier::Lossy); // an .m4a is AAC unless scanned as ALAC
        assert_eq!(Tier::of("flac", Some(16), Some(44_100)), Tier::Lossless);
        assert_eq!(Tier::of("FLAC", None, None), Tier::Lossless); // unknown resolution: not claimed as hi-res
        assert_eq!(Tier::of("flac", Some(24), Some(44_100)), Tier::HiRes);
        assert_eq!(Tier::of("flac", Some(16), Some(96_000)), Tier::HiRes);
        assert_eq!(Tier::of("wav", Some(16), Some(48_000)), Tier::Lossless);
        assert!(Tier::Lossy < Tier::Lossless && Tier::Lossless < Tier::HiRes);
    }

    #[test]
    fn what_an_upgrade_asks_for() {
        assert_eq!(Tier::Lossy.next(), Some(Tier::Lossless));
        assert_eq!(Tier::Lossless.next(), Some(Tier::HiRes));
        assert_eq!(Tier::HiRes.next(), None);
    }

    #[test]
    fn an_album_is_as_good_as_its_worst_song() {
        let hires = row("flac", 24, 96_000, 2800);
        let cd = row("flac", 16, 44_100, 900);
        let q = album_quality(&[hires.clone(), cd]);
        assert_eq!((q["tier"].as_str(), q["line"].as_str(), q["upgrade"].as_str()), (Some("lossless"), Some("FLAC · 16-bit / 44.1 kHz"), Some("hires")));
        let q = album_quality(&[hires.clone(), row("mp3", 0, 44_100, 320)]);
        assert_eq!((q["tier"].as_str(), q["line"].as_str(), q["upgrade"].as_str()), (Some("lossy"), Some("MP3 · 320 kbps"), Some("lossless")));
        let q = album_quality(&[hires]);
        assert_eq!((q["label"].as_str(), q["line"].as_str()), (Some("Hi-Res"), Some("FLAC · 24-bit / 96 kHz")));
        assert!(q["upgrade"].is_null());
        assert!(album_quality(&[]).is_null());
    }

    #[test]
    fn lines() {
        assert_eq!(line(&row("flac", 0, 48_000, 0)), "FLAC · 48 kHz");
        assert_eq!(line(&row("mp3", 0, 0, 0)), "MP3");
        assert_eq!(khz(88_200), "88.2");
        assert_eq!(khz(192_000), "192");
    }

    #[test]
    fn is_hires_reads_the_detail() {
        assert!(is_hires(&json!({ "lossless_only": true, "hires_only": true })));
        assert!(!is_hires(&json!({ "lossless_only": true })));
    }
}
