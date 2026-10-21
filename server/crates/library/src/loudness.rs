//! Each song's loudness, for volume normalization in the apps (0.31): what the scan reads from the file's
//! ReplayGain / R128 tags ([`tag_columns`]), the "Measure loudness" task that measures the rest with ffmpeg
//! ([`scheduled`]), and what the apps get: the `gain`, `sample_rate` and `bit_depth` of every track row
//! ([`stamp`], run by the server's enrichment pass) and the `gain` of the signal path (`info_gain`).
//!
//! Every gain is a ReplayGain gain in dB relative to the −18 LUFS reference, every peak linear (1.0 = full
//! scale). A song's columns (schema 27): `rg_track_gain`, `rg_track_peak` (from its tags, or worked out
//! from the measurement: −18 − LUFS, 10^(dBTP / 20)), `rg_album_gain`, `rg_album_peak` (its album tags),
//! `loudness_lufs` (measured, or −18 − the track gain tag), `loudness_source` ('tags' | 'measured' |
//! 'none' when it could not be measured | NULL while nobody has looked). The album's values are worked
//! out when the songs are read, from every song with the same `album_id` (docs/contracts/
//! client-shapes-0.31-0.33.md §2.1): the tags' album gain when every song has one, else
//! −18 − 10·log10(mean of 10^(LUFS / 10)) once every song has a loudness; the album peak goes with it.
use crate::Ctx;
use encore_db::{p, Db, Query, Row, RowExt};
use encore_jobs::Busy;
use encore_media::tags::{AudioFacts, GainTags};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The ReplayGain reference level, LUFS.
pub const REFERENCE_LUFS: f64 = -18.0;
/// Integrated loudness at or below this is silence (ebur128 answers −70 for digital silence): no gain is
/// worked out from it.
const SILENCE_LUFS: f64 = -70.0;

fn round(v: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (v * m).round() / m
}

/// A gain to two decimals, a peak to four: plenty for a volume stage, and short in JSON.
fn gain2(v: f64) -> f64 {
    round(v, 2)
}

fn peak4(v: f64) -> f64 {
    round(v, 4)
}

// ---- the scan ------------------------------------------------------------------------------------

/// The loudness columns, in the order [`tag_columns`] gives their values.
pub const COLUMNS: [&str; 6] = ["rg_track_gain", "rg_track_peak", "rg_album_gain", "rg_album_peak", "loudness_lufs", "loudness_source"];

/// What the scan writes into [`COLUMNS`] for a file whose tags say `g`. `keep_measured`: the row holds a
/// measurement of this audio (its source is 'measured' and the length is the same), which a file without a
/// track gain tag keeps; then only the album tags are written (`Some` of the two album columns' values).
pub enum TagColumns {
    /// every column (a song with a track gain tag, or one to be measured again: the others NULL)
    All([Value; 6]),
    /// only `rg_album_gain`, `rg_album_peak`: the measurement stays
    AlbumOnly([Value; 2]),
}

pub fn tag_columns(g: &GainTags, keep_measured: bool) -> TagColumns {
    match g.track_gain {
        None if keep_measured => TagColumns::AlbumOnly([json!(g.album_gain.map(gain2)), json!(g.album_peak.map(peak4))]),
        _ => TagColumns::All(tag_values(g)),
    }
}

/// The values of every one of [`COLUMNS`] for a file whose tags say `g` (a new song): with a track gain tag
/// it is 'tags' (its loudness −18 − the gain), else only the album tags are known.
pub fn tag_values(g: &GainTags) -> [Value; 6] {
    let (ag, ap) = (json!(g.album_gain.map(gain2)), json!(g.album_peak.map(peak4)));
    match g.track_gain {
        Some(tg) => [json!(gain2(tg)), json!(g.track_peak.map(peak4)), ag, ap, json!(gain2(REFERENCE_LUFS - tg)), json!("tags")],
        None => [Value::Null, Value::Null, ag, ap, Value::Null, Value::Null],
    }
}

/// A changed file keeps its measurement when it was measured and is as long as before (a tag edit, the
/// same recording written again): `old_source` and `old_duration` are the row's, `duration` the file's.
pub fn keeps_measurement(old_source: &str, old_duration: f64, duration: f64) -> bool {
    old_source == "measured" && (old_duration - duration).abs() < 0.5
}

// ---- reading: the album aggregate and the stamp ----------------------------------------------------

/// One song's loudness columns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SongLoudness {
    pub album_id: String,
    pub track_gain: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_gain: Option<f64>,
    pub album_peak: Option<f64>,
    pub lufs: Option<f64>,
    pub source: Option<String>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
}

fn opt_f(r: &Row, k: &str) -> Option<f64> {
    r.get(k).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn opt_i(r: &Row, k: &str) -> Option<i64> {
    r.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).filter(|v| *v > 0)
}

impl SongLoudness {
    fn from_row(r: &Row) -> Self {
        SongLoudness {
            album_id: r.s("album_id"),
            track_gain: opt_f(r, "rg_track_gain"),
            track_peak: opt_f(r, "rg_track_peak"),
            album_gain: opt_f(r, "rg_album_gain"),
            album_peak: opt_f(r, "rg_album_peak"),
            lufs: opt_f(r, "loudness_lufs"),
            source: r.get("loudness_source").and_then(Value::as_str).map(str::to_string),
            sample_rate: opt_i(r, "sample_rate"),
            bit_depth: opt_i(r, "bit_depth"),
        }
    }
}

const SONG_COLS: &str = "id, album_id, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, loudness_lufs, loudness_source, sample_rate, bit_depth";

/// What an album's songs say together (every song with the same `album_id` counts once).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlbumLoudness {
    pub songs: usize,
    /// songs with an album gain tag / an album peak tag / a loudness
    pub with_album_gain: usize,
    pub with_album_peak: usize,
    pub with_lufs: usize,
    /// sum of 10^(LUFS / 10) over the songs with a loudness
    pub energy: f64,
    /// the loudest track peak known
    pub max_track_peak: Option<f64>,
}

impl AlbumLoudness {
    pub fn add(&mut self, s: &SongLoudness) {
        self.songs += 1;
        self.with_album_gain += s.album_gain.is_some() as usize;
        self.with_album_peak += s.album_peak.is_some() as usize;
        if let Some(l) = s.lufs {
            self.with_lufs += 1;
            self.energy += 10f64.powf(l / 10.0);
        }
        if let Some(p) = s.track_peak {
            self.max_track_peak = Some(self.max_track_peak.map_or(p, |m| m.max(p)));
        }
    }

    /// The album's (gain, peak) as the song `s` of it gets them: the tags' album gain (the song's own) when
    /// every song has one, else −18 − the energy mean of the songs' loudness once every song has one. The
    /// peak: the tags' album peak (the song's own) when the tags' gain is used and every song has a peak
    /// tag, else the loudest track peak known.
    pub fn for_song(&self, s: &SongLoudness) -> (Option<f64>, Option<f64>) {
        let songs = self.songs.max(1);
        let tags = self.with_album_gain >= songs && s.album_gain.is_some();
        let gain = if tags {
            s.album_gain
        } else if self.with_lufs >= songs && self.energy > 0.0 {
            Some(REFERENCE_LUFS - 10.0 * (self.energy / self.with_lufs as f64).log10())
        } else {
            None
        };
        let peak = if tags && self.with_album_peak >= songs && s.album_peak.is_some() { s.album_peak } else { self.max_track_peak };
        (gain, peak)
    }
}

/// The `gain` object of a track row: `{track, album, track_peak, album_peak}`, each only when known; None
/// when neither gain is known (the row then has no `gain`).
pub fn gain_object(s: &SongLoudness, album: Option<&AlbumLoudness>) -> Option<Map<String, Value>> {
    let (album_gain, album_peak) = album.map(|a| a.for_song(s)).unwrap_or((None, None));
    if s.track_gain.is_none() && album_gain.is_none() {
        return None;
    }
    let mut m = Map::new();
    if let Some(v) = s.track_gain {
        m.insert("track".into(), json!(gain2(v)));
    }
    if let Some(v) = album_gain {
        m.insert("album".into(), json!(gain2(v)));
    }
    if let Some(v) = s.track_peak {
        m.insert("track_peak".into(), json!(peak4(v)));
    }
    if let Some(v) = album_peak {
        m.insert("album_peak".into(), json!(peak4(v)));
    }
    Some(m)
}

/// `cols` of the songs with these ids (500 per statement).
fn songs_by_id(db: &impl Query, ids: &[i64]) -> HashMap<i64, SongLoudness> {
    let mut out = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        let sql = format!("SELECT {SONG_COLS} FROM tracks WHERE id IN ({})", crate::artists::marks(chunk.len()));
        for r in db.query(&sql, chunk.iter().map(|i| json!(i)).collect()).unwrap_or_default() {
            out.insert(r.i("id"), SongLoudness::from_row(&r));
        }
    }
    out
}

/// What these albums' songs say together (500 albums per statement).
fn albums_of(db: &impl Query, album_ids: &[String]) -> HashMap<String, AlbumLoudness> {
    let mut out: HashMap<String, AlbumLoudness> = HashMap::with_capacity(album_ids.len());
    for chunk in album_ids.chunks(500) {
        let sql = format!("SELECT {SONG_COLS} FROM tracks WHERE album_id IN ({})", crate::artists::marks(chunk.len()));
        for r in db.query(&sql, chunk.iter().map(|a| json!(a)).collect()).unwrap_or_default() {
            let s = SongLoudness::from_row(&r);
            out.entry(s.album_id.clone()).or_default().add(&s);
        }
    }
    out
}

/// The library song a JSON object is a row of: an object with `title`, `album_id` and `format` (every
/// track row the server writes has them; catalogue items and requests don't), whose song is `track_id`
/// when it has one (a playlist entry: its own `id` is the entry's) and else `id`.
fn song_of(o: &Map<String, Value>) -> Option<i64> {
    if !(o.get("title").is_some_and(Value::is_string) && o.contains_key("album_id") && o.contains_key("format")) {
        return None;
    }
    match o.get("track_id") {
        Some(t) => t.as_i64(),
        None => o.get("id").and_then(Value::as_i64),
    }
    .filter(|id| *id > 0)
}

fn collect(v: &Value, out: &mut Vec<i64>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Value::Object(o) => {
            out.extend(song_of(o));
            o.values().filter(|x| x.is_array() || x.is_object()).for_each(|x| collect(x, out));
        }
        _ => {}
    }
}

/// The raw columns a row may carry (`SELECT t.*`): the stamp puts `gain` in their place.
const RAW: [&str; 6] = COLUMNS;

fn apply(v: &mut Value, songs: &HashMap<i64, SongLoudness>, albums: &HashMap<String, AlbumLoudness>) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| apply(x, songs, albums)),
        Value::Object(o) => {
            if let Some(id) = song_of(o) {
                for k in RAW {
                    o.remove(k);
                }
                let s = songs.get(&id);
                match s.and_then(|s| gain_object(s, albums.get(&s.album_id))) {
                    Some(g) => o.insert("gain".into(), Value::Object(g)),
                    None => o.remove("gain"),
                };
                for (k, val) in [("sample_rate", s.and_then(|s| s.sample_rate)), ("bit_depth", s.and_then(|s| s.bit_depth))] {
                    match val {
                        Some(n) => o.insert(k.into(), json!(n)),
                        None => o.remove(k),
                    };
                }
            }
            o.values_mut().filter(|x| x.is_array() || x.is_object()).for_each(|x| apply(x, songs, albums));
        }
        _ => {}
    }
}

/// Adds `gain` (when anything is known), `sample_rate` and `bit_depth` (when known) to every library track
/// row in `v` (see [`song_of`]), wherever it sits in the answer, and takes out the raw loudness columns
/// a `SELECT *` brought. Two lookups: the songs by id, and the songs of the albums whose loudness is
/// partly known (for the album gain). Running it again on its own answer changes nothing.
pub fn stamp(db: &impl Query, v: &mut Value) {
    let mut ids = vec![];
    collect(v, &mut ids);
    if ids.is_empty() {
        return;
    }
    ids.sort_unstable();
    ids.dedup();
    let songs = songs_by_id(db, &ids);
    // an album gain needs the album's other songs: only for songs that have an album tag or a loudness
    let mut wanted: Vec<String> = songs.values().filter(|s| s.album_gain.is_some() || s.lufs.is_some()).map(|s| s.album_id.clone()).collect::<HashSet<_>>().into_iter().collect();
    wanted.sort();
    let albums = if wanted.is_empty() { HashMap::new() } else { albums_of(db, &wanted) };
    apply(v, &songs, &albums);
}

/// The `gain` of `GET /api/library/tracks/{id}/info`: the row's four numbers, `source` ('tags' | 'measured')
/// and `lufs` (the song's integrated loudness); None when nothing is known.
pub fn info_gain(db: &impl Query, id: i64) -> Option<Value> {
    let s = songs_by_id(db, &[id]).remove(&id)?;
    let album = (s.album_gain.is_some() || s.lufs.is_some()).then(|| albums_of(db, std::slice::from_ref(&s.album_id)).remove(&s.album_id)).flatten();
    let mut g = gain_object(&s, album.as_ref())?;
    if let Some(src) = s.source.as_deref().filter(|x| *x == "tags" || *x == "measured") {
        g.insert("source".into(), json!(src));
    }
    if let Some(l) = s.lufs {
        g.insert("lufs".into(), json!(gain2(l)));
    }
    Some(Value::Object(g))
}

// ---- the "Measure loudness" task ---------------------------------------------------------------------

/// How a run of the task goes.
#[derive(Debug, Clone)]
pub struct Limits {
    /// songs read, measured and written together (one write of the songs per slice)
    pub slice: usize,
    /// a run stops after this long; the next one goes on where it stopped
    pub budget: Duration,
    /// the app in use this long without a break ends the run (the busy gate)
    pub busy_max: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { slice: 200, budget: Duration::from_secs(4 * 3600), busy_max: Duration::from_secs(600) }
    }
}

/// Why a run stopped before the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Time,
    Busy,
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    /// songs whose tags had a track gain (songs scanned before 0.31, whose tags were never read for it)
    pub from_tags: usize,
    /// songs measured by ffmpeg
    pub measured: usize,
    /// songs whose quality scan had measured them already
    pub reused: usize,
    /// songs that could not be measured (silence, unreadable): marked, not tried again until they change
    pub failed: usize,
    /// songs still to measure after the run
    pub left: i64,
    pub stopped: Option<Stop>,
    /// ffmpeg is missing: only tags were read
    pub no_ffmpeg: bool,
}

impl Run {
    /// The result line of Settings → Scheduled tasks.
    pub fn line(&self) -> String {
        if self.measured + self.reused + self.from_tags + self.failed == 0 && self.left == 0 && self.stopped.is_none() {
            return "Nothing left to measure".into();
        }
        let mut parts = vec![format!("{} measured", self.measured + self.reused)];
        if self.from_tags > 0 {
            parts.push(format!("{} from their tags", self.from_tags));
        }
        if self.failed > 0 {
            parts.push(format!("{} could not be measured", self.failed));
        }
        if self.left > 0 {
            parts.push(format!("{} still to measure", self.left));
        }
        let mut s = parts.join(", ");
        match self.stopped {
            Some(Stop::Time) => s.push_str(" (the run's time was up; the next run goes on)"),
            Some(Stop::Busy) => s.push_str(" (paused while the app is in use; the next run goes on)"),
            None => {}
        }
        if self.no_ffmpeg && self.left > 0 {
            s.push_str(". Measuring needs ffmpeg");
        }
        s
    }
}

/// Settings → Scheduled tasks → Measure loudness: the songs whose loudness nobody has looked at yet
/// (`loudness_source IS NULL`, so a run goes on where the last one stopped), at the lowest priority and
/// only while nobody uses the app; while a group listen plays it waits altogether, and goes on after the last one
/// stopped (the wait does not count against the run's time). First their tags are read again, once (songs scanned before 0.31, which
/// have no `sample_rate` yet: a track gain tag is used as it is, and the sample rate — 0 when the file does
/// not say — and the bit depth are filled in), then the rest are measured
/// with ffmpeg's EBU R128 filter (`ebur128=peak=true`) one at a time at nice 19 and idle disk priority, a
/// slice of [`Limits::slice`] songs per write. A song the quality scan measured already (same file) takes
/// that measurement.
pub async fn scheduled(st: &impl Ctx) -> anyhow::Result<String> {
    let ffmpeg = st.media().ffmpeg_ok();
    let measure = |path: PathBuf| async move { encore_media::loudness::measure(&path, true).await };
    let r = run(st.db(), &st.library().busy, ffmpeg, measure, Limits::default()).await?;
    if r.measured + r.reused + r.from_tags > 0 {
        st.db().log("info", &format!("Measure loudness: {}", r.line()));
    }
    Ok(r.line())
}

/// Waits while the app is in use, at most `max`; false when it still is.
async fn quiet(busy: &Busy, max: Duration) -> bool {
    let until = Instant::now() + max;
    while busy.in_use() {
        if Instant::now() >= until {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    true
}

/// One run of the task: the tags pass (on a thread of its own at low priority), then the measuring.
/// `measure(path)` gives (integrated LUFS, true peak dBTP), as [`encore_media::loudness::measure`] does.
pub async fn run<F, Fut>(db: &Db, busy: &Busy, ffmpeg: bool, measure: F, lim: Limits) -> anyhow::Result<Run>
where
    F: Fn(PathBuf) -> Fut,
    Fut: Future<Output = (Option<f64>, Option<f64>)>,
{
    // moved on by the time spent waiting for group listens: only the work counts against the run's time
    let deadline = Instant::now() + lim.budget;
    let mut out = Run { no_ffmpeg: !ffmpeg, ..Default::default() };
    // 1. tags: a light read of each file, no ffmpeg
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let (db, busy, lim) = (db.clone(), busy.clone(), lim.clone());
        std::thread::Builder::new().name("loudness-tags".into()).spawn(move || {
            encore_jobs::busy::lower_this_thread();
            let _ = tx.send(tags_pass(&db, &busy, &lim, deadline));
        })?;
    }
    let (from_tags, stopped, mut deadline) = rx.await.map_err(|_| anyhow::anyhow!("the tags pass stopped unexpectedly"))??;
    out.from_tags = from_tags;
    out.stopped = stopped;
    // 2. measuring, slice by slice
    let mut after = 0i64;
    while out.stopped.is_none() {
        let rows = db.query("SELECT t.id, t.path, t.mtime, q.lufs q_lufs, q.true_peak q_peak, q.mtime q_mtime FROM tracks t LEFT JOIN track_quality q ON q.track_id=t.id
            WHERE t.loudness_source IS NULL AND t.id > ? ORDER BY t.id LIMIT ?", p![after, lim.slice as i64])?;
        if rows.is_empty() {
            break;
        }
        let mut done: Vec<Measured> = Vec::with_capacity(rows.len());
        for r in &rows {
            if Instant::now() >= deadline {
                out.stopped = Some(Stop::Time);
                break;
            }
            let (id, mtime) = (r.i("id"), r.f("mtime"));
            let quality = opt_f(r, "q_lufs").filter(|_| r.get("q_mtime").and_then(Value::as_f64).is_some_and(|m| (m - mtime).abs() < 0.001));
            if let Some(lufs) = quality {
                done.push(Measured { id, mtime, lufs: Some(lufs), peak_db: opt_f(r, "q_peak"), from_quality: true });
            } else if ffmpeg {
                // a group listen playing: the measuring waits for it to stop, however long it plays (the run
                // goes on after it with the time it had left)
                deadline += busy.wait_out_groups().await;
                if !quiet(busy, lim.busy_max).await {
                    out.stopped = Some(Stop::Busy);
                    break;
                }
                let (lufs, peak_db) = measure(PathBuf::from(r.s("path"))).await;
                done.push(Measured { id, mtime, lufs, peak_db, from_quality: false });
            }
            after = id;
        }
        let (measured, reused, failed) = write_measured(db, &done)?;
        out.measured += measured;
        out.reused += reused;
        out.failed += failed;
    }
    out.left = db.one("SELECT COUNT(*) n FROM tracks WHERE loudness_source IS NULL", p![])?.map(|r| r.i("n")).unwrap_or(0);
    Ok(out)
}

/// The tags pass: every song not looked at yet whose tags were not read since 0.31 (`sample_rate IS NULL`: the
/// scan writes it for every file it reads, and this pass writes it, 0 when unknown, for every file it read),
/// its file's tags read again ([`encore_media::tags::read_facts`]); a track gain tag makes it 'tags', and the
/// album tags, sample rate and bit depth are written. What a run read stays read: a run the busy gate stops
/// here leaves the next run only the songs after it, and once every older song was read the pass finds
/// nothing (songs scanned since have theirs). A group listen playing: it waits, and the run's `deadline` moves
/// on by the time waited. Runs on a thread of its own; (songs that got their loudness from tags, why it stopped
/// early, the deadline).
fn tags_pass(db: &Db, busy: &Busy, lim: &Limits, mut deadline: Instant) -> anyhow::Result<(usize, Option<Stop>, Instant)> {
    let mut after = 0i64;
    let mut found = 0;
    loop {
        if Instant::now() >= deadline {
            return Ok((found, Some(Stop::Time), deadline));
        }
        let rows = db.query("SELECT id, path, mtime, rg_album_gain, rg_album_peak, sample_rate, bit_depth FROM tracks
            WHERE loudness_source IS NULL AND sample_rate IS NULL AND id > ? ORDER BY id LIMIT ?", p![after, lim.slice as i64])?;
        if rows.is_empty() {
            return Ok((found, None, deadline));
        }
        let mut read: Vec<(&Row, AudioFacts)> = Vec::with_capacity(rows.len());
        let mut stopped = None;
        for r in &rows {
            deadline += busy.wait_out_groups_blocking();
            busy.wait_while_in_use(lim.busy_max);
            if busy.in_use() {
                stopped = Some(Stop::Busy);
                break;
            }
            read.push((r, encore_media::tags::read_facts(&r.s("path"))));
            after = r.i("id");
        }
        found += write_tags(db, &read)?;
        if stopped.is_some() {
            return Ok((found, stopped, deadline));
        }
    }
}

/// Writes what the tags pass read (one transaction): the songs with a track gain tag become 'tags'; the
/// others get their album tags (when the file has them), and every song its sample rate (0 when unknown, so
/// it is not read again) and bit depth. A song whose file changed meanwhile (another mtime: the scan wrote it
/// again) is left alone. Returns how many became 'tags'.
fn write_tags(db: &Db, read: &[(&Row, AudioFacts)]) -> anyhow::Result<usize> {
    // (statement, values, whether it makes the song 'tags')
    let mut stmts: Vec<(String, Vec<Value>, bool)> = vec![];
    for (r, f) in read {
        let (id, mtime) = (r.i("id"), r.f("mtime"));
        let (rate, bits) = (f.sample_rate.map(|v| v as i64), f.bit_depth.map(|v| v as i64));
        match f.gain.track_gain {
            Some(_) => {
                let mut params: Vec<Value> = tag_values(&f.gain).to_vec();
                params.extend([json!(rate), json!(bits), json!(id), json!(mtime)]);
                stmts.push((format!("UPDATE tracks SET {}, sample_rate=COALESCE(?, sample_rate, 0), bit_depth=COALESCE(?, bit_depth) WHERE id=? AND loudness_source IS NULL AND abs(mtime - ?) < 0.001",
                    COLUMNS.iter().map(|c| format!("{c}=?")).collect::<Vec<_>>().join(", ")), params, true));
            }
            None => {
                // always written: the sample rate (0 when unknown) marks the song as read
                stmts.push(("UPDATE tracks SET rg_album_gain=COALESCE(?, rg_album_gain), rg_album_peak=COALESCE(?, rg_album_peak), sample_rate=COALESCE(?, sample_rate, 0), bit_depth=COALESCE(?, bit_depth) WHERE id=? AND loudness_source IS NULL AND abs(mtime - ?) < 0.001".into(),
                    p![f.gain.album_gain.map(gain2), f.gain.album_peak.map(peak4), rate, bits, id, mtime], false));
            }
        }
    }
    if stmts.is_empty() {
        return Ok(0);
    }
    Ok(db.write_now(|c| c.transaction(|c| {
        let mut tagged = 0;
        for (sql, params, tags) in &stmts {
            c.execute(sql, params.clone())?;
            if *tags && c.one("SELECT changes() n", p![])?.map(|r| r.i("n")).unwrap_or(0) > 0 {
                tagged += 1;
            }
        }
        Ok::<_, rusqlite::Error>(tagged)
    }))?)
}

/// A measurement a gain can be worked out from: a loudness above silence.
fn measurable(lufs: Option<f64>) -> bool {
    lufs.is_some_and(|l| l.is_finite() && l > SILENCE_LUFS)
}

/// One song's measurement: (integrated LUFS, true peak dBTP) and the mtime of the file it was made of.
struct Measured {
    id: i64,
    mtime: f64,
    lufs: Option<f64>,
    peak_db: Option<f64>,
    /// taken from the quality scan's report of the same file
    from_quality: bool,
}

/// Writes a slice of measurements (one transaction). A loudness makes the song 'measured' (track gain
/// −18 − LUFS, peak 10^(dBTP / 20)); none, or silence, makes it 'none' so the next run moves on. A song
/// whose file changed meanwhile (another mtime: the scan wrote it again) is left alone. Returns what was
/// written: (measured by ffmpeg, taken from the quality scan, could not be measured).
fn write_measured(db: &Db, done: &[Measured]) -> anyhow::Result<(usize, usize, usize)> {
    if done.is_empty() {
        return Ok((0, 0, 0));
    }
    Ok(db.write_now(|c| c.transaction(|c| {
        let (mut measured, mut reused, mut failed) = (0, 0, 0);
        for m in done {
            let ok = m.lufs.filter(|_| measurable(m.lufs));
            match ok {
                Some(l) => {
                    let peak = m.peak_db.filter(|p| p.is_finite()).map(|db| peak4(10f64.powf(db / 20.0)));
                    c.execute("UPDATE tracks SET rg_track_gain=?, rg_track_peak=?, loudness_lufs=?, loudness_source='measured' WHERE id=? AND loudness_source IS NULL AND abs(mtime - ?) < 0.001",
                        p![gain2(REFERENCE_LUFS - l), peak, gain2(l), m.id, m.mtime])?;
                }
                None => {
                    c.execute("UPDATE tracks SET loudness_source='none' WHERE id=? AND loudness_source IS NULL AND abs(mtime - ?) < 0.001", p![m.id, m.mtime])?;
                }
            }
            if c.one("SELECT changes() n", p![])?.map(|r| r.i("n")).unwrap_or(0) > 0 {
                match (ok.is_some(), m.from_quality) {
                    (false, _) => failed += 1,
                    (true, true) => reused += 1,
                    (true, false) => measured += 1,
                }
            }
        }
        Ok::<_, rusqlite::Error>((measured, reused, failed))
    }))?)
}

#[cfg(test)]
#[path = "loudness_tests.rs"]
mod tests;
