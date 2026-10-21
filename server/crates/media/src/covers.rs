//! Album covers made fast: an in-memory index (album → cover file + version), versioned cacheable
//! URLs (`cover_v` in the JSON, `&v=` in the URL), small WebP/JPEG thumbnails in a few fixed sizes,
//! and a background pass that makes the grid/list sizes ahead of time.
//!
//! The pass also finds each cover's main colour (`cover_color` in the JSON, "#rrggbb"), which clients
//! paint as the placeholder while the image loads. It looks at recently added and recently played
//! albums first, on up to half the cores at low priority, and its progress shows in Diagnostics.
use crate::Ctx;
use encore_db::{p, Db, RowExt};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Thumbnail sizes: a requested size snaps up to the next one so caches hit.
pub const SIZES: &[u32] = &[96, 200, 400, 600, 1000];
/// Made ahead of time for every album (the grid and list sizes).
const WARM_SIZES: &[u32] = &[200, 400];
/// How long a known cover is trusted before its file is looked at again.
const RECHECK: Duration = Duration::from_secs(600);
/// Covers a JSON response may look up on the spot (the rest come from the background pass).
const INLINE_LOOKUPS: usize = 6;
const WEBP_QUALITY: f32 = 82.0;
const JPEG_QUALITY: u8 = 85;

#[derive(Clone)]
struct Entry {
    path: Option<PathBuf>,
    v: u64,
    checked: Instant,
    stale: bool,
    /// its artwork changed: lists must not use the old version any more
    forgotten: bool,
}

/// album → (cover version, "#rrggbb")
type ColorMap = HashMap<String, (u64, String)>;

/// What the thumbnail pass has still to do: every album, or only these (their songs changed).
#[derive(Default)]
struct WarmQueue {
    all: bool,
    albums: Vec<String>,
}

#[derive(Clone, Default)]
pub struct CoverIndex {
    memo: Arc<Mutex<HashMap<String, Entry>>>,
    warming: Arc<AtomicBool>,
    queue: Arc<Mutex<WarmQueue>>,
    /// goes up with every `forget`: a lookup that saw it change may have read the old file
    generation: Arc<AtomicU64>,
    /// album → (cover version, "#rrggbb"), read from the cover_color table on first use
    colors: Arc<Mutex<Option<ColorMap>>>,
    progress: Arc<WarmProgress>,
    /// the pass waits while somebody uses the app
    busy: encore_jobs::Busy,
}

/// How far the thumbnail pass is (Diagnostics: "Cover thumbnails: 12 340 / 18 000").
#[derive(Default)]
struct WarmProgress {
    done: AtomicUsize,
    total: AtomicUsize,
}

/// Recently played albums count for this long when ordering the thumbnail pass.
const PLAYED_WINDOW: f64 = 180.0 * 86400.0;

/// A size bucket for `s` (0 = the original file).
pub fn snap(s: u32) -> u32 {
    if s == 0 {
        return 0;
    }
    SIZES.iter().copied().find(|b| s <= *b).unwrap_or(*SIZES.last().unwrap())
}

/// The cover file's version: changes whenever the file (or which file) changes. Fits in a JS number.
pub fn version_of(path: &Path) -> Option<u64> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut h = Sha1::new();
    h.update(path.to_string_lossy().as_bytes());
    h.update(mtime.to_le_bytes());
    h.update(m.len().to_le_bytes());
    let d = h.finalize();
    let mut b = [0u8; 8];
    b[2..].copy_from_slice(&d[..6]);
    Some(u64::from_be_bytes(b).max(1))
}

fn thumbs_dir(db: &Db) -> PathBuf {
    db.data_dir.join("covers").join("thumbs")
}

pub fn thumb_path(db: &Db, album_id: &str, size: u32, v: u64, webp: bool) -> PathBuf {
    thumbs_dir(db).join(format!("{album_id}_{size}_{v:x}.{}", if webp { "webp" } else { "jpg" }))
}

/// Thumbnails of an older version of a cover.
fn remove_thumbs(db: &Db, album_id: &str, v: u64) {
    for s in SIZES {
        for webp in [true, false] {
            let _ = std::fs::remove_file(thumb_path(db, album_id, *s, v, webp));
        }
    }
}

impl CoverIndex {
    /// An empty index whose thumbnail pass waits while `busy` says the app is in use.
    pub fn new(busy: encore_jobs::Busy) -> Self {
        CoverIndex { busy, ..Default::default() }
    }

    /// Forget one album's cover (artwork changed), or mark every cover for a fresh look (after a scan
    /// or import) while lists keep their versions until the look is done.
    pub fn forget(&self, album_id: Option<&str>) {
        let mut m = self.memo.lock().unwrap();
        self.generation.fetch_add(1, Ordering::SeqCst);
        match album_id {
            None => m.values_mut().for_each(|e| e.stale = true),
            Some(a) => {
                // also when it wasn't known yet: a lookup running now must not leave its (old) answer behind
                let e = m.entry(a.to_string()).or_insert_with(|| Entry { path: None, v: 0, checked: Instant::now(), stale: true, forgotten: true });
                e.stale = true;
                e.forgotten = true;
            }
        }
    }

    /// Albums a list names that no cover pass will look at (not in the library, e.g. a YouTube Music
    /// album or a deleted one): remembered as "no cover" so they don't start the pass on every list.
    /// Stale, so a real album among them is looked at properly by the pass or the next request.
    fn assume_none(&self, album_ids: &[String]) {
        let mut m = self.memo.lock().unwrap();
        for a in album_ids {
            m.entry(a.clone()).or_insert_with(|| Entry { path: None, v: 0, checked: Instant::now(), stale: true, forgotten: false });
        }
    }

    /// The cover's version as last seen (no file access): for stamping lists.
    pub fn known(&self, album_id: &str) -> Option<Option<u64>> {
        self.memo.lock().unwrap().get(album_id).filter(|e| !e.forgotten).map(|e| e.path.as_ref().map(|_| e.v))
    }

    /// The album's cover file and its version, from memory when it was looked at recently.
    pub fn get(&self, db: &Db, album_id: &str) -> Option<(PathBuf, u64)> {
        let generation = self.generation.load(Ordering::SeqCst);
        let cur = self.memo.lock().unwrap().get(album_id).cloned();
        if let Some(e) = &cur {
            if !e.stale {
                if e.checked.elapsed() < RECHECK {
                    return e.path.clone().map(|p| (p, e.v));
                }
                // a while since we looked: one stat says whether the file is still the same
                if let Some(p) = &e.path {
                    if version_of(p) == Some(e.v) {
                        self.memo.lock().unwrap().insert(album_id.to_string(), Entry { checked: Instant::now(), ..e.clone() });
                        return Some((p.clone(), e.v));
                    }
                }
            }
        }
        let found = find_cover_uncached(db, album_id);
        let v = found.as_deref().and_then(version_of).unwrap_or(0);
        let path = found.filter(|_| v != 0);
        if let Some(old) = cur.filter(|e| e.path.is_some() && (e.v != v || path.is_none())) {
            remove_thumbs(db, album_id, old.v);
        }
        let mut m = self.memo.lock().unwrap();
        // the artwork was forgotten while we looked: what we read may be the old file, so the next look
        // goes to the disk again (and lists keep not stamping it if this album was the one forgotten)
        let raced = self.generation.load(Ordering::SeqCst) != generation;
        let forgotten = raced && m.get(album_id).is_some_and(|e| e.forgotten);
        m.insert(album_id.to_string(), Entry { path: path.clone(), v, checked: Instant::now(), stale: raced, forgotten });
        path.map(|p| (p, v))
    }

    /// Look at every album's cover and make the grid/list thumbnails, one album at a time on one
    /// low-priority thread. A call while it runs makes it go round once more.
    pub fn warm(&self, db: Db) {
        self.queue.lock().unwrap().all = true;
        self.start(db);
    }

    /// Look again at these albums only (a scan or an import changed their songs): lists keep the version
    /// they know until the look is done; then their thumbnails and colours are made, and an album left
    /// without songs loses its thumbnails and colour. Runs on the pass's thread, after what it is doing.
    pub fn refresh(&self, db: Db, album_ids: Vec<String>) {
        if album_ids.is_empty() {
            return;
        }
        {
            let mut m = self.memo.lock().unwrap();
            // a lookup running now may read the old file: it leaves its answer stale (see `get`)
            self.generation.fetch_add(1, Ordering::SeqCst);
            for a in &album_ids {
                if let Some(e) = m.get_mut(a) {
                    e.stale = true;
                }
            }
        }
        {
            let mut q = self.queue.lock().unwrap();
            for a in album_ids {
                if !q.albums.contains(&a) {
                    q.albums.push(a);
                }
            }
        }
        self.start(db);
    }

    /// Runs the pass's thread unless it runs already; it works through the queue until it is empty.
    fn start(&self, db: Db) {
        if self.warming.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("cover-thumbs".into()).spawn(move || {
            loop {
                let job = std::mem::take(&mut *me.queue.lock().unwrap());
                if job.all {
                    // the whole pass looks at the changed albums too
                    me.warm_once(&db, None);
                } else if !job.albums.is_empty() {
                    me.warm_once(&db, Some(job.albums));
                } else {
                    break;
                }
            }
            me.warming.store(false, Ordering::SeqCst);
            // a call that came in just as we finished
            let q = me.queue.lock().unwrap();
            let more = q.all || !q.albums.is_empty();
            drop(q);
            if more {
                me.start(db);
            }
        });
        if spawned.is_err() {
            self.warming.store(false, Ordering::SeqCst);
        }
    }

    /// One pass: every album in priority order (`only` None), or only these albums.
    fn warm_once(&self, db: &Db, only: Option<Vec<String>>) {
        let whole = only.is_none();
        // a pass over some albums: those still in the library are looked at; the thumbnails and colours of the
        // others go (below)
        let mut asked: Vec<String> = vec![];
        let ids = match only {
            Some(ids) => {
                let present = album_ids_in_library(db, &ids);
                asked = ids;
                asked.iter().filter(|a| present.contains(*a)).cloned().collect()
            }
            None => {
                let added: Vec<(String, f64)> = db.query("SELECT album_id, MAX(COALESCE(added_at, 0)) t FROM tracks WHERE album_id IS NOT NULL AND album_id != '' GROUP BY album_id", p![])
                    .unwrap_or_default().iter().map(|r| (r.s("album_id"), r.f("t"))).collect();
                let played: HashMap<String, f64> = db.query("SELECT t.album_id, MAX(p.played_at) t FROM plays p JOIN tracks t ON t.id = p.track_id WHERE p.played_at > ? GROUP BY t.album_id",
                    p![encore_db::now() - PLAYED_WINDOW]).unwrap_or_default().iter().map(|r| (r.s("album_id"), r.f("t"))).collect();
                priority_order(added, &played)
            }
        };
        // Diagnostics counts the whole pass ("12 340 / 18 000 albums looked at"); a pass over a few changed
        // albums leaves those numbers as they are
        if whole {
            self.progress.total.store(ids.len(), Ordering::SeqCst);
            self.progress.done.store(0, Ordering::SeqCst);
        }
        self.load_colors(db);
        let started = Instant::now();
        let (made, seen, removed, painted) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
        // thumbnails on disk by album: those of older versions (and of the old `{album}_{size}.jpg`
        // names, and of albums gone from the library) are removed as we go
        let mut on_disk: HashMap<String, Vec<String>> = HashMap::new();
        let mine: Option<std::collections::HashSet<&String>> = (!whole).then(|| asked.iter().collect());
        if let Ok(rd) = std::fs::read_dir(thumbs_dir(db)) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some((album, _)) = name.split_once('_') {
                    // a pass over some albums only touches their files
                    if mine.as_ref().is_none_or(|m| m.contains(&album.to_string())) {
                        on_disk.entry(album.to_string()).or_default().push(name);
                    }
                }
            }
        }
        let on_disk = Mutex::new(on_disk);
        let next = AtomicUsize::new(0);
        let one = |a: &String| {
            let found = self.get(db, a);
            let names = on_disk.lock().unwrap().remove(a);
            if let Some(names) = names {
                removed.fetch_add(remove_other_versions(db, &names, found.as_ref().map(|f| f.1)), Ordering::Relaxed);
            }
            let Some((src, v)) = found else { return };
            seen.fetch_add(1, Ordering::Relaxed);
            let want: Vec<(u32, bool)> = WARM_SIZES.iter().flat_map(|s| [(*s, true), (*s, false)])
                .filter(|(s, w)| !thumb_path(db, a, *s, v, *w).is_file()).collect();
            let mut worked = false;
            if !want.is_empty() {
                made.fetch_add(make_thumbs(db, &src, a, v, &want), Ordering::Relaxed);
                worked = true;
            }
            if self.color_of(a).is_none_or(|(cv, _)| cv != v) {
                // the 200 px thumbnail is small and already made: cheap to read again
                let small = thumb_path(db, a, 200, v, false);
                if let Some(c) = color_of_file(&small).or_else(|| color_of_file(&src)) {
                    self.set_color(db, a, v, &c);
                    painted.fetch_add(1, Ordering::Relaxed);
                }
                worked = true;
            }
            if worked {
                // stay out of the way of requests being served
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        // each worker takes the next album in priority order until none are left
        let work = |n: usize| {
            encore_jobs::busy::lower_this_thread();
            loop {
                // while somebody uses the app, one thread keeps going and the others wait
                if n > 0 {
                    self.busy.wait_while_in_use(Duration::from_secs(600));
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(a) = ids.get(i) else { break };
                one(a);
                if whole {
                    let done = self.progress.done.fetch_add(1, Ordering::SeqCst) + 1;
                    if done.is_multiple_of(1000) {
                        tracing::info!("cover thumbnails: {done}/{} albums looked at", ids.len());
                    }
                }
            }
        };
        let threads = warm_threads();
        std::thread::scope(|s| {
            let mut spawned = 0;
            for n in 0..threads {
                let work = &work;
                if std::thread::Builder::new().name(format!("cover-thumbs-{n}")).spawn_scoped(s, move || work(n)).is_ok() {
                    spawned += 1;
                }
            }
            if spawned == 0 {
                // no thread to be had: do it on this one (already a background thread)
                work(0);
            }
        });
        for names in on_disk.into_inner().unwrap().values() {
            removed.fetch_add(remove_other_versions(db, names, None), Ordering::Relaxed);
        }
        // colours of albums gone from the library
        if whole {
            let _ = db.execute("DELETE FROM cover_color WHERE album_id NOT IN (SELECT DISTINCT album_id FROM tracks WHERE album_id IS NOT NULL)
                                AND album_id NOT IN (SELECT DISTINCT album_id FROM surround_tracks WHERE album_id IS NOT NULL)", p![]);
        } else {
            let gone: Vec<&String> = asked.iter().filter(|a| !ids.contains(*a)).collect();
            for chunk in gone.chunks(500) {
                let marks = vec!["?"; chunk.len()].join(",");
                let _ = db.execute(&format!("DELETE FROM cover_color WHERE album_id IN ({marks})"), chunk.iter().map(|a| json!(a)).collect());
            }
        }
        let (made, seen, removed, painted) = (made.into_inner(), seen.into_inner(), removed.into_inner(), painted.into_inner());
        if made > 0 || removed > 0 || painted > 0 {
            tracing::info!("cover thumbnails: {made} made and {painted} colours found for {seen} albums with covers, {removed} old ones removed in {:.1}s ({threads} threads)",
                started.elapsed().as_secs_f64());
        }
    }

    /// The thumbnail pass: (albums looked at, albums in the library, running?).
    pub fn progress(&self) -> (usize, usize, bool) {
        (self.progress.done.load(Ordering::SeqCst), self.progress.total.load(Ordering::SeqCst), self.warming.load(Ordering::SeqCst))
    }

    // ---- placeholder colours --------------------------------------------------------------------

    fn load_colors(&self, db: &Db) {
        let mut c = self.colors.lock().unwrap();
        if c.is_some() {
            return;
        }
        let rows = db.query("SELECT album_id, v, color FROM cover_color", p![]).unwrap_or_default();
        *c = Some(rows.iter().map(|r| (r.s("album_id"), (r.i("v") as u64, r.s("color")))).collect());
    }

    /// The album's cover colour as last found, with the cover version it was found for.
    fn color_of(&self, album_id: &str) -> Option<(u64, String)> {
        self.colors.lock().unwrap().as_ref().and_then(|m| m.get(album_id).cloned())
    }

    /// The colour of this version of the album's cover, if the pass found it.
    pub fn color(&self, db: &Db, album_id: &str, v: u64) -> Option<String> {
        self.load_colors(db);
        self.color_of(album_id).filter(|c| c.0 == v).map(|c| c.1)
    }

    fn set_color(&self, db: &Db, album_id: &str, v: u64, color: &str) {
        let _ = db.execute("INSERT OR REPLACE INTO cover_color (album_id, v, color) VALUES (?, ?, ?)", p![album_id, v as i64, color]);
        if let Some(m) = self.colors.lock().unwrap().as_mut() {
            m.insert(album_id.to_string(), (v, color.to_string()));
        }
    }
}

/// The thumbnail pass's threads: half the cores, at least one.
fn warm_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(1).max(1)
}

/// The order the pass looks at albums in: the most recent of when an album was added and when it was
/// last played first (what people open now gets its thumbnails and colour first), then the rest.
fn priority_order(added: Vec<(String, f64)>, played: &HashMap<String, f64>) -> Vec<String> {
    let mut v: Vec<(String, f64)> = added.into_iter()
        .map(|(a, t)| {
            let p = played.get(&a).copied().unwrap_or(0.0);
            (a, t.max(p))
        })
        .collect();
    v.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| x.0.cmp(&y.0)));
    v.into_iter().map(|x| x.0).collect()
}

/// The main colour of an image file ("#rrggbb").
fn color_of_file(path: &Path) -> Option<String> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let small = if img.width().max(img.height()) > 200 { img.thumbnail(200, 200) } else { img };
    dominant_color(&small.to_rgb8())
}

/// The cover's main colour: a small k-means in Lab over a sample of the pixels; the biggest cluster
/// wins (a more colourful one wins a near tie, so a thin grey border doesn't beat the artwork).
pub fn dominant_color(img: &image::RgbImage) -> Option<String> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    // about 2 500 pixels whatever the size
    let step = (((w as f64 * h as f64) / 2500.0).sqrt().floor() as u32).max(1);
    let mut px: Vec<[f64; 3]> = vec![];
    for y in (0..h).step_by(step as usize) {
        for x in (0..w).step_by(step as usize) {
            px.push(srgb_to_lab(img.get_pixel(x, y).0));
        }
    }
    const K: usize = 5;
    // start from pixels spread over the lightness range: the same answer every time
    let mut by_l: Vec<&[f64; 3]> = px.iter().collect();
    by_l.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    let k = K.min(px.len());
    let mut centres: Vec<[f64; 3]> = (0..k).map(|i| *by_l[(i * 2 + 1) * by_l.len() / (2 * k)]).collect();
    let mut counts = vec![0usize; k];
    for _ in 0..10 {
        let mut sums = vec![[0.0f64; 3]; k];
        counts.iter_mut().for_each(|c| *c = 0);
        for p in &px {
            let j = (0..k).min_by(|a, b| dist2(p, &centres[*a]).partial_cmp(&dist2(p, &centres[*b])).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0);
            counts[j] += 1;
            for d in 0..3 {
                sums[j][d] += p[d];
            }
        }
        let mut moved = 0.0;
        for j in 0..k {
            if counts[j] > 0 {
                let c = [sums[j][0] / counts[j] as f64, sums[j][1] / counts[j] as f64, sums[j][2] / counts[j] as f64];
                moved += dist2(&c, &centres[j]);
                centres[j] = c;
            }
        }
        if moved < 0.01 {
            break;
        }
    }
    let score = |j: usize| {
        let chroma = (centres[j][1].powi(2) + centres[j][2].powi(2)).sqrt();
        counts[j] as f64 * (1.0 + chroma.min(60.0) / 200.0)
    };
    let best = (0..k).max_by(|a, b| score(*a).partial_cmp(&score(*b)).unwrap_or(std::cmp::Ordering::Equal))?;
    let [r, g, b] = lab_to_srgb(centres[best]);
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn dist2(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

// sRGB (D65) ↔ CIE Lab
const WHITE: [f64; 3] = [0.95047, 1.0, 1.08883];

fn srgb_to_lab(c: [u8; 3]) -> [f64; 3] {
    let lin = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let xyz = [
        0.4124564 * r + 0.3575761 * g + 0.1804375 * b,
        0.2126729 * r + 0.7151522 * g + 0.0721750 * b,
        0.0193339 * r + 0.1191920 * g + 0.9503041 * b,
    ];
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(xyz[0] / WHITE[0]), f(xyz[1] / WHITE[1]), f(xyz[2] / WHITE[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn lab_to_srgb(lab: [f64; 3]) -> [u8; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let (fx, fz) = (fy + lab[1] / 500.0, fy - lab[2] / 200.0);
    let inv = |t: f64| if t.powi(3) > 216.0 / 24389.0 { t.powi(3) } else { (116.0 * t - 16.0) / (24389.0 / 27.0) };
    let (x, y, z) = (inv(fx) * WHITE[0], inv(fy) * WHITE[1], inv(fz) * WHITE[2]);
    let lin = [
        3.2404542 * x - 1.5371385 * y - 0.4985314 * z,
        -0.9692660 * x + 1.8760108 * y + 0.0415560 * z,
        0.0556434 * x - 0.2040259 * y + 1.0572252 * z,
    ];
    let gam = |v: f64| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [gam(lin[0]), gam(lin[1]), gam(lin[2])]
}

/// Removes the thumbnails among `names` (one album's files in the thumbs folder) that are not of
/// version `v` (all of them when the album has no cover). How many were removed.
fn remove_other_versions(db: &Db, names: &[String], v: Option<u64>) -> usize {
    let keep = v.map(|v| format!("_{v:x}."));
    names.iter()
        .filter(|n| !n.ends_with(".tmp"))
        .filter(|n| keep.as_ref().is_none_or(|k| !n.contains(k.as_str())))
        .filter(|n| std::fs::remove_file(thumbs_dir(db).join(n)).is_ok())
        .count()
}

/// Decode a cover once and write the given (size, webp?) thumbnails. How many were written.
fn make_thumbs(db: &Db, src: &Path, album_id: &str, v: u64, want: &[(u32, bool)]) -> usize {
    let Ok(img) = image::ImageReader::open(src).and_then(|r| r.with_guessed_format()).map_err(anyhow::Error::from).and_then(|r| r.decode().map_err(anyhow::Error::from)) else { return 0 };
    let dir = thumbs_dir(db);
    if std::fs::create_dir_all(&dir).is_err() {
        return 0;
    }
    let mut n = 0;
    let mut sizes: Vec<u32> = want.iter().map(|w| w.0).collect();
    sizes.sort_unstable();
    sizes.dedup();
    for size in sizes {
        let small = if img.width().max(img.height()) > size {
            // fast area filter, then a light sharpen to keep it crisp
            img.thumbnail(size, size).unsharpen(0.6, 2)
        } else {
            img.clone()
        };
        let rgb = small.to_rgb8();
        for webp in want.iter().filter(|w| w.0 == size).map(|w| w.1) {
            let out = thumb_path(db, album_id, size, v, webp);
            let bytes = if webp {
                webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height()).encode(WEBP_QUALITY).to_vec()
            } else {
                let mut buf = Vec::new();
                if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY).encode_image(&rgb).is_err() {
                    continue;
                }
                buf
            };
            let tmp = out.with_extension(format!("{}.tmp", encore_core::ids::random_hex(6)));
            if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &out).is_ok() {
                n += 1;
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    n
}

/// Side of the picture [`mosaic`] makes, in pixels.
pub const MOSAIC_SIZE: u32 = 600;

/// The picture of a shared playlist: the first four `srcs` cropped to squares in a 2x2 grid (top left, top
/// right, bottom left, bottom right), a 600x600 JPEG at `out`. It is written next to `out` and then renamed
/// into place, so a reader never sees half a file. False, and no file, when there are fewer than four
/// pictures or one of them can't be read.
pub fn mosaic(srcs: &[PathBuf], out: &Path) -> bool {
    let tile = MOSAIC_SIZE / 2;
    if srcs.len() < 4 {
        return false;
    }
    let mut canvas = image::RgbImage::new(MOSAIC_SIZE, MOSAIC_SIZE);
    for (i, src) in srcs.iter().take(4).enumerate() {
        let Ok(img) = image::ImageReader::open(src).and_then(|r| r.with_guessed_format()).map_err(anyhow::Error::from).and_then(|r| r.decode().map_err(anyhow::Error::from)) else { return false };
        let square = img.resize_to_fill(tile, tile, image::imageops::FilterType::Triangle).to_rgb8();
        let (x, y) = ((i as u32 % 2) * tile, (i as u32 / 2) * tile);
        image::imageops::replace(&mut canvas, &square, x as i64, y as i64);
    }
    let mut buf = Vec::new();
    if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY).encode_image(&canvas).is_err() {
        return false;
    }
    // a hidden name in the same folder: the rename is atomic, and folder tidying knows these
    let tmp = out.parent().unwrap_or(Path::new(".")).join(format!(".{}.tmp.jpg", encore_core::ids::random_hex(6)));
    if std::fs::write(&tmp, &buf).is_ok() && std::fs::rename(&tmp, out).is_ok() {
        return true;
    }
    let _ = std::fs::remove_file(&tmp);
    false
}

/// The thumbnail for a request (made now if needed), or the original when it can't be made.
pub fn thumbnail(db: &Db, src: &Path, album_id: &str, v: u64, size: u32, webp: bool) -> PathBuf {
    let out = thumb_path(db, album_id, size, v, webp);
    if out.is_file() || make_thumbs(db, src, album_id, v, &[(size, webp)]) > 0 {
        out
    } else {
        src.to_path_buf()
    }
}

// ---- cover_v in JSON ----------------------------------------------------------------------------

fn is_album_id(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The album an object shows the cover of: its `album_id`, an artist's `cover_album`, or its own
/// `id` when it is an album.
fn album_of(o: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(a) = o.get("album_id").and_then(|v| v.as_str()) {
        return Some(a.to_string()).filter(|a| !a.is_empty());
    }
    // artists show one of their albums' covers
    if let Some(a) = o.get("cover_album").and_then(|v| v.as_str()) {
        return Some(a.to_string()).filter(|a| !a.is_empty());
    }
    let id = o.get("id")?.as_str()?;
    (is_album_id(id) && (o.contains_key("album") || o.contains_key("albumartist"))).then(|| id.to_string())
}

fn collect(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Value::Object(o) => {
            if let Some(a) = album_of(o) {
                out.push(a);
            }
            o.values().filter(|x| x.is_array() || x.is_object()).for_each(|x| collect(x, out));
        }
        _ => {}
    }
}

fn apply(v: &mut Value, vs: &HashMap<String, (u64, Option<String>)>) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| apply(x, vs)),
        Value::Object(o) => {
            if let Some((cv, color)) = album_of(o).and_then(|a| vs.get(&a)) {
                o.insert("cover_v".into(), json!(cv));
                if let Some(c) = color {
                    o.insert("cover_color".into(), json!(c));
                }
            }
            o.values_mut().filter(|x| x.is_array() || x.is_object()).for_each(|x| apply(x, vs));
        }
        _ => {}
    }
}

/// Add `cover_v` (and `cover_color` once the pass found it) to every album and track in a JSON value. Unknown covers are looked up on the spot
/// when there are only a few (an album page); otherwise the background pass is started for them.
pub fn stamp(st: &impl Ctx, v: &mut Value) {
    let mut ids = vec![];
    collect(v, &mut ids);
    if ids.is_empty() {
        return;
    }
    ids.sort();
    ids.dedup();
    let mut vs = HashMap::new();
    let mut missing = vec![];
    for a in ids {
        match st.covers().known(&a) {
            Some(Some(cv)) => {
                let color = st.covers().color(st.db(), &a, cv);
                vs.insert(a, (cv, color));
            }
            Some(None) => {}
            None => missing.push(a),
        }
    }
    if missing.len() <= INLINE_LOOKUPS {
        for a in missing {
            if let Some((_, cv)) = st.covers().get(st.db(), &a) {
                let color = st.covers().color(st.db(), &a, cv);
                vs.insert(a, (cv, color));
            }
        }
    } else {
        // the pass looks at the library's albums; the others are "no cover" from now on
        let library: std::collections::HashSet<String> = album_ids_in_library(st.db(), &missing);
        let outside: Vec<String> = missing.into_iter().filter(|a| !library.contains(a)).collect();
        st.covers().assume_none(&outside);
        st.covers().warm(st.db().clone());
    }
    if !vs.is_empty() {
        apply(v, &vs);
    }
}

/// Which of these albums have songs in the library.
fn album_ids_in_library(db: &Db, ids: &[String]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let params: Vec<Value> = chunk.iter().map(|a| json!(a)).collect();
        let both = format!("SELECT DISTINCT album_id FROM tracks WHERE album_id IN ({marks}) UNION SELECT DISTINCT album_id FROM surround_tracks WHERE album_id IN ({marks})");
        let mut params = params;
        params.extend(params.clone());
        match db.query(&both, params) {
            Ok(rows) => out.extend(rows.iter().map(|r| r.s("album_id"))),
            // can't tell: treat them all as library albums (the pass decides)
            Err(_) => out.extend(chunk.iter().cloned()),
        }
    }
    out
}

// ---- finding the cover file ----------------------------------------------------------------------

/// Picture files a cover is looked for under, in this order.
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "artwork"];

/// Folder picture (cover.jpg, folder.png, …) or the art embedded in the first file, read from the disk
/// every time ([`CoverIndex::get`] keeps the answer).
pub fn find_cover_uncached(db: &Db, album_id: &str) -> Option<PathBuf> {
    // a Surround album ("s…", encore_library::surround) has its songs in their own table
    let table = if album_id.starts_with('s') { "surround_tracks" } else { "tracks" };
    let tracks = db.query(&format!("SELECT path FROM {table} WHERE album_id=? ORDER BY disc, track LIMIT 50"), p![album_id]).ok()?;
    let mut dirs: Vec<PathBuf> = vec![];
    for t in &tracks {
        let d = Path::new(&t.s("path")).parent()?.to_path_buf();
        if dirs.contains(&d) {
            continue;
        }
        // a folder holding several albums (e.g. "Singles") can't use one cover.jpg for all of them
        let ds = d.to_string_lossy().trim_end_matches('/').to_string();
        // a range on the path index (`dir/` ≤ path < `dir0`, '0' being the character after '/'), not a
        // LIKE that reads every song: this runs for every album in the thumbnail pass. Bytes, not
        // characters, after the folder: no further '/'.
        let shared = db.one(&format!("SELECT COUNT(*) n FROM (SELECT DISTINCT album_id FROM {table} WHERE path >= ? AND path < ? AND instr(substr(CAST(path AS BLOB), ?), CAST('/' AS BLOB)) = 0 LIMIT 2)"),
            p![format!("{ds}/"), format!("{ds}0"), ds.len() as i64 + 2])
            .ok().flatten().map(|r| r.i("n") > 1).unwrap_or(false);
        if !shared {
            dirs.push(d.clone());
        }
        let base = d.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if (base.starts_with("cd") || base.starts_with("disc") || base.starts_with("disk")) && base.chars().any(|c| c.is_ascii_digit()) {
            if let Some(parent) = d.parent() {
                if !dirs.contains(&parent.to_path_buf()) {
                    dirs.push(parent.to_path_buf());
                }
            }
        }
    }
    let first_dir = tracks.first().and_then(|t| Path::new(&t.s("path")).parent().map(|p| p.to_path_buf()));
    for d in &dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut images: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| { let l = n.to_lowercase(); l.ends_with(".jpg") || l.ends_with(".jpeg") || l.ends_with(".png") || l.ends_with(".webp") }).collect();
        images.sort();
        for pref in COVER_NAMES {
            if let Some(n) = images.iter().find(|n| n.to_lowercase().starts_with(pref)) {
                return Some(d.join(n));
            }
        }
        if Some(d) == first_dir.as_ref() {
            if let Some(n) = images.first().filter(|n| !n.to_lowercase().starts_with("artist")) {
                return Some(d.join(n));
            }
        }
    }
    let cached = db.data_dir.join("covers").join(format!("{album_id}.jpg"));
    if cached.is_file() {
        return Some(cached);
    }
    let first = tracks.first()?.s("path");
    let data = crate::tags::embedded_art(&first)?;
    std::fs::create_dir_all(cached.parent()?).ok()?;
    std::fs::write(&cached, data).ok()?;
    Some(cached)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_snap_to_buckets() {
        assert_eq!(snap(0), 0);
        assert_eq!(snap(1), 96);
        assert_eq!(snap(96), 96);
        assert_eq!(snap(97), 200);
        assert_eq!(snap(300), 400);
        assert_eq!(snap(400), 400);
        assert_eq!(snap(512), 600);
        assert_eq!(snap(640), 1000);
        assert_eq!(snap(5000), 1000);
    }

    #[test]
    fn dominant_color_picks_the_main_colour() {
        // mostly red with a blue band and a thin grey frame: red
        let img = image::RgbImage::from_fn(200, 200, |x, y| {
            if x < 4 || y < 4 || x > 195 || y > 195 {
                image::Rgb([128, 128, 128])
            } else if y > 150 {
                image::Rgb([20, 40, 220])
            } else {
                image::Rgb([220, 30, 30])
            }
        });
        let c = dominant_color(&img).unwrap();
        let rgb = u32::from_str_radix(&c[1..], 16).unwrap();
        let (r, g, b) = ((rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255);
        assert!(c.len() == 7 && c.starts_with('#'), "{c}");
        assert!(r > 190 && g < 60 && b < 60, "red wins: {c}");
        // one flat colour comes back as itself (Lab round trip)
        let flat = image::RgbImage::from_pixel(50, 50, image::Rgb([18, 120, 77]));
        assert_eq!(dominant_color(&flat).as_deref(), Some("#12784d"));
        // the same answer every time
        assert_eq!(dominant_color(&img), dominant_color(&img));
        assert_eq!(dominant_color(&image::RgbImage::new(0, 0)), None);
    }

    #[test]
    fn mosaic_puts_four_covers_in_the_corners() {
        let dir = std::env::temp_dir().join(format!("encore-mosaic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // different sizes, shapes and formats: each is cropped to a square of its own corner
        let colours = [[220u8, 20, 20], [20, 200, 40], [30, 40, 220], [250, 250, 250]];
        let shapes = [(500u32, 500u32, "a.jpg"), (120, 80, "b.png"), (64, 300, "c.png"), (1000, 900, "d.jpg")];
        let srcs: Vec<PathBuf> = colours.iter().zip(shapes).map(|(c, (w, h, name))| {
            let p = dir.join(name);
            image::RgbImage::from_pixel(w, h, image::Rgb(*c)).save(&p).unwrap();
            p
        }).collect();
        let out = dir.join("tok.mosaic.jpg");
        assert!(mosaic(&srcs, &out));
        let img = image::open(&out).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (600, 600));
        for (i, (x, y)) in [(150u32, 150u32), (450, 150), (150, 450), (450, 450)].into_iter().enumerate() {
            let px = img.get_pixel(x, y).0;
            for (got, want) in px.iter().zip(colours[i]) {
                assert!((*got as i32 - want as i32).abs() < 24, "quadrant {i}: {px:?} vs {:?}", colours[i]);
            }
        }
        // nothing else left in the folder (the temp file was renamed)
        let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["a.jpg", "b.png", "c.png", "d.jpg", "tok.mosaic.jpg"]);

        // a picture that can't be read, or fewer than four: false and no file
        std::fs::write(dir.join("bad.jpg"), b"\xff\xd8\xff not a real jpeg").unwrap();
        let out2 = dir.join("tok.other.jpg");
        let bad = vec![srcs[0].clone(), srcs[1].clone(), dir.join("bad.jpg"), srcs[3].clone()];
        assert!(!mosaic(&bad, &out2));
        assert!(!mosaic(&srcs[..3], &out2));
        assert!(!mosaic(&[srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), dir.join("missing.jpg")], &out2));
        assert!(!out2.exists());
        assert!(!std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".tmp")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn warm_order_puts_recent_albums_first() {
        let added = vec![
            ("old".to_string(), 100.0),
            ("new".to_string(), 900.0),
            ("played".to_string(), 50.0),
            ("middle".to_string(), 500.0),
        ];
        let played: std::collections::HashMap<String, f64> = [("played".to_string(), 1000.0), ("old".to_string(), 10.0)].into_iter().collect();
        assert_eq!(priority_order(added, &played), vec!["played", "new", "middle", "old"]);
    }
}
