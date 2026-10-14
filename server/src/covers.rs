//! Album covers made fast: an in-memory index (album → cover file + version), versioned cacheable
//! URLs (`cover_v` in the JSON, `&v=` in the URL), small WebP/JPEG thumbnails in a few fixed sizes,
//! and a background pass that makes the grid/list sizes ahead of time.
use crate::db::{Db, RowExt};
use crate::p;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

#[derive(Clone, Default)]
pub struct CoverIndex {
    memo: Arc<Mutex<HashMap<String, Entry>>>,
    warming: Arc<AtomicBool>,
    warm_again: Arc<AtomicBool>,
    /// goes up with every `forget`: a lookup that saw it change may have read the old file
    generation: Arc<AtomicU64>,
}

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
        let found = crate::library::find_cover_uncached(db, album_id);
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
        self.warm_again.store(true, Ordering::SeqCst);
        if self.warming.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("cover-thumbs".into()).spawn(move || {
            while me.warm_again.swap(false, Ordering::SeqCst) {
                me.warm_once(&db);
            }
            me.warming.store(false, Ordering::SeqCst);
            // a call that came in just as we finished
            if me.warm_again.load(Ordering::SeqCst) {
                me.warm(db);
            }
        });
        if spawned.is_err() {
            self.warming.store(false, Ordering::SeqCst);
        }
    }

    fn warm_once(&self, db: &Db) {
        let ids: Vec<String> = db.query("SELECT DISTINCT album_id FROM tracks", p![]).unwrap_or_default().iter().map(|r| r.s("album_id")).collect();
        let started = Instant::now();
        let (mut made, mut seen) = (0usize, 0usize);
        // thumbnails on disk by album: those of older versions (and of the old `{album}_{size}.jpg`
        // names, and of albums gone from the library) are removed as we go
        let mut on_disk: HashMap<String, Vec<String>> = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(thumbs_dir(db)) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some((album, _)) = name.split_once('_') {
                    on_disk.entry(album.to_string()).or_default().push(name);
                }
            }
        }
        let mut removed = 0usize;
        for (i, a) in ids.iter().enumerate() {
            let found = self.get(db, a);
            if let Some(names) = on_disk.remove(a) {
                removed += remove_other_versions(db, &names, found.as_ref().map(|f| f.1));
            }
            let Some((src, v)) = found else { continue };
            seen += 1;
            let want: Vec<(u32, bool)> = WARM_SIZES.iter().flat_map(|s| [(*s, true), (*s, false)])
                .filter(|(s, w)| !thumb_path(db, a, *s, v, *w).is_file()).collect();
            if !want.is_empty() {
                made += make_thumbs(db, &src, a, v, &want);
                // stay out of the way of requests being served
                std::thread::sleep(Duration::from_millis(15));
            }
            if (i + 1) % 500 == 0 {
                tracing::info!("cover thumbnails: {}/{} albums looked at, {made} made", i + 1, ids.len());
            }
        }
        for names in on_disk.values() {
            removed += remove_other_versions(db, names, None);
        }
        if made > 0 || removed > 0 {
            tracing::info!("cover thumbnails: {made} made for {seen} albums with covers, {removed} old ones removed in {:.1}s", started.elapsed().as_secs_f64());
        }
    }
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
            let tmp = out.with_extension(format!("{}.tmp", crate::auth::random_hex(6)));
            if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &out).is_ok() {
                n += 1;
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    n
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

fn apply(v: &mut Value, vs: &HashMap<String, u64>) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| apply(x, vs)),
        Value::Object(o) => {
            if let Some(cv) = album_of(o).and_then(|a| vs.get(&a).copied()) {
                o.insert("cover_v".into(), json!(cv));
            }
            o.values_mut().filter(|x| x.is_array() || x.is_object()).for_each(|x| apply(x, vs));
        }
        _ => {}
    }
}

/// Add `cover_v` to every album and track in a JSON value. Unknown covers are looked up on the spot
/// when there are only a few (an album page); otherwise the background pass is started for them.
pub fn stamp(st: &AppState, v: &mut Value) {
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
        match st.library.covers.known(&a) {
            Some(Some(cv)) => {
                vs.insert(a, cv);
            }
            Some(None) => {}
            None => missing.push(a),
        }
    }
    if missing.len() <= INLINE_LOOKUPS {
        for a in missing {
            if let Some((_, cv)) = st.library.covers.get(&st.db, &a) {
                vs.insert(a, cv);
            }
        }
    } else {
        // the pass looks at the library's albums; the others are "no cover" from now on
        let library: std::collections::HashSet<String> = album_ids_in_library(&st.db, &missing);
        let outside: Vec<String> = missing.into_iter().filter(|a| !library.contains(a)).collect();
        st.library.covers.assume_none(&outside);
        st.library.covers.warm(st.db.clone());
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
        match db.query(&format!("SELECT DISTINCT album_id FROM tracks WHERE album_id IN ({marks})"), params) {
            Ok(rows) => out.extend(rows.iter().map(|r| r.s("album_id"))),
            // can't tell: treat them all as library albums (the pass decides)
            Err(_) => out.extend(chunk.iter().cloned()),
        }
    }
    out
}

/// Middleware: JSON answers to GETs gain `cover_v` wherever an album or song is listed.
pub async fn stamp_layer(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let wanted = req.method() == Method::GET && req.uri().path().starts_with("/api/") && !req.uri().path().starts_with("/api/remote/events");
    let resp = next.run(req).await;
    let json = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("application/json"));
    if !wanted || !json || resp.status() != StatusCode::OK {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 32 << 20).await else {
        return Response::from_parts(parts, Body::empty());
    };
    // cheap check first: no album fields, nothing to do
    if !bytes.windows(7).any(|w| w == b"\"album\"" || w == b"\"album_" || w == b"\"cover_") {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let Ok(mut v) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let st2 = st.clone();
    let out = tokio::task::spawn_blocking(move || {
        stamp(&st2, &mut v);
        serde_json::to_vec(&v).ok()
    }).await.ok().flatten();
    match out {
        Some(b) => {
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from(b))
        }
        None => Response::from_parts(parts, Body::from(bytes)),
    }
}

#[cfg(test)]
#[path = "covers_tests.rs"]
mod covers_tests;
