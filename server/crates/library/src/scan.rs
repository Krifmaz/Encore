//! The music library: scan files into the database, read tags, find covers, make thumbnails.
//! Album ids are computed exactly like older servers did (sha1 of "albumartist|album"), so links keep working.
use crate::Ctx;
use encore_db::{now, p, Conn, Db, Row, RowExt};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use encore_core::audio::AUDIO_EXTS;
/// The columns a scan writes from the file's tags and audio properties (the loudness columns are written
/// beside them, see [`crate::loudness::tag_columns`]).
pub(crate) const COLS: &[&str] = &["title", "artist", "album", "albumartist", "track", "disc", "year", "genre", "duration", "bitrate", "format",
    "mb_trackid", "mb_albumid", "mb_releasegroupid", "mb_artistid", "mb_albumartistid", "isrc", "sample_rate", "bit_depth",
    "releasetype", "tracktotal", "compilation", "artist_credit", "albumartist_credit"];

/// Who brought a new song into the library (`tracks.added_by`, `added_via`), written when its row is made and
/// never by a rescan: a scan ([`Added::SCAN`]), a Files upload or import (the admin who did it). A request's
/// songs are stamped by the request worker right after the import (`record_origin`), as `request_id` is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Added {
    /// the admin who added it (None for a scan)
    pub by: Option<i64>,
    /// how: "scan", "upload" or "import"
    pub via: &'static str,
}

impl Added {
    /// Found in a library folder: nobody added it.
    pub const SCAN: Added = Added { by: None, via: "scan" };
}

impl Default for Added {
    fn default() -> Self {
        Added::SCAN
    }
}

/// The library scan's progress, for the API.
#[derive(Default, Clone, serde::Serialize)]
pub struct ScanState {
    /// a scan runs now
    pub scanning: bool,
    /// files read so far
    pub progress: usize,
    /// files to read
    pub total: usize,
    /// when the last scan finished, Unix seconds
    pub last_scan: Option<f64>,
    /// what it is doing, or how it ended
    pub message: String,
}

/// What the library keeps while the server runs. Clones share it.
#[derive(Clone, Default)]
pub struct Library {
    /// the scan's progress
    pub scan: Arc<Mutex<ScanState>>,
    /// album → cover file + version (see encore_media::covers)
    pub covers: encore_media::covers::CoverIndex,
    /// the artist index of this library (see crate::artists)
    pub artists: crate::artists::Artists,
    /// translated and romanized titles: who has them on, the titles waiting, the model server
    pub translator: crate::translate::Translator,
    /// what the AcoustID and MusicBrainz submissions said last, and the AcoustID pacing
    pub contribute: crate::contribute::Contrib,
    /// every song's identity for matching (requests, playlist imports, the sync), kept while the songs are
    /// unchanged (see crate::matcher::Songs)
    pub songs: crate::matcher::Songs,
    /// when the app was last used: background passes (the "Measure loudness" task) wait while it is
    pub busy: encore_jobs::Busy,
    /// the library folders a scan has warned about (not there, looks empty, a part unreadable): each warning
    /// is written to the activity log once, not on every scan, until the folder is fine again
    pub warned: Arc<Mutex<HashSet<String>>>,
    /// every album's type (Album, EP, Single, One-off; live, compilation …), once per state of the library
    pub types: crate::album_types::Types,
}

/// An album's id: the first 16 hex digits of a SHA-1 of "album artist|album" (lower case), or of the folder for songs without an album tag.
pub fn album_key(albumartist: &str, album: &str, path: &str) -> String {
    let key = if !album.is_empty() {
        format!("{}|{}", albumartist.trim().to_lowercase(), album.trim().to_lowercase())
    } else {
        format!("dir|{}", Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
    };
    hex::encode(Sha1::digest(key.as_bytes()))[..16].to_string()
}

use encore_media::tags::{read_tags_and_facts, GainTags};

/// A file's modification time (Unix seconds) and size.
pub(crate) fn mtime_size(meta: &std::fs::Metadata) -> (f64, i64) {
    let mt = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    (mt, meta.len() as i64)
}

/// Files the scan writes per transaction: the library version moves once per batch, so the lists and the
/// artist index are built again once per 200 songs during a full rescan instead of after every song.
pub const SCAN_BATCH: usize = 200;

/// What one file's row will hold: read from the file (its tags), before the database is touched.
pub struct FileRow {
    /// the file
    path: String,
    /// its album id
    album_id: String,
    /// its modification time
    mtime: f64,
    /// its size in bytes
    size: i64,
    /// the column values ([`COLS`])
    vals: Vec<Value>,
    /// its ReplayGain / R128 tags
    gain: GainTags,
    /// its length, seconds
    duration: f64,
}

/// Reads a file's tags (no database work): `meta` is its metadata when the caller has it already.
fn read_file(path: &str, meta: Option<std::fs::Metadata>) -> std::io::Result<FileRow> {
    let meta = match meta {
        Some(m) => m,
        None => std::fs::metadata(path)?,
    };
    let (mtime, size) = mtime_size(&meta);
    let (mut t, facts) = read_tags_and_facts(path);
    t.insert("sample_rate".into(), json!(facts.sample_rate));
    t.insert("bit_depth".into(), json!(facts.bit_depth));
    let album_id = album_key(&t.s("albumartist"), &t.s("album"), path);
    let vals: Vec<Value> = COLS.iter().map(|c| t.get(*c).cloned().unwrap_or(Value::Null)).collect();
    Ok(FileRow { path: path.to_string(), album_id, mtime, size, vals, gain: facts.gain, duration: t.f("duration") })
}

/// Writes one file's row (a new song, or the new tags of a known one). The albums it touched come back: its
/// album, and the one it was in before when its tags moved it.
fn store_file(c: &Conn, f: &FileRow, added: Added) -> rusqlite::Result<Vec<String>> {
    use crate::loudness::{keeps_measurement, tag_columns, tag_values, TagColumns, COLUMNS};
    // an album merged into another (encore_library::merge) is written as that one, and an album of one name
    // split by release (encore_library::split) as its part
    let album_id = crate::merge::resolve(c, &f.album_id)?;
    let col = |k: &str| COLS.iter().position(|c| *c == k).and_then(|i| f.vals.get(i)).map(|v| match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }).unwrap_or_default();
    let album_id = crate::split::resolve(c, &album_id, &col("mb_releasegroupid"), &col("year"))?;
    let mut albums = vec![album_id.clone()];
    if let Some(ex) = c.one("SELECT id, mtime, size, verified_at, album_id, duration, loudness_source FROM tracks WHERE path=?", p![f.path])? {
        let mut sets = COLS.iter().map(|c| format!("{c}=?")).collect::<Vec<_>>().join(", ");
        let mut params = vec![json!(album_id), json!(f.mtime), json!(f.size)];
        params.extend(f.vals.iter().cloned());
        // the loudness: a track gain tag wins; a measurement of the same audio stays; else measured again
        let (cols, vals): (&[&str], Vec<Value>) = match tag_columns(&f.gain, keeps_measurement(&ex.s("loudness_source"), ex.f("duration"), f.duration)) {
            TagColumns::All(v) => (&COLUMNS, v.to_vec()),
            TagColumns::AlbumOnly(v) => (&COLUMNS[2..4], v.to_vec()),
        };
        for col in cols {
            sets.push_str(&format!(", {col}=?"));
        }
        params.extend(vals);
        params.push(json!(ex.i("id")));
        c.execute(&format!("UPDATE tracks SET album_id=?, mtime=?, size=?, {sets} WHERE id=?"), params)?;
        // the audio may have changed: check it again (tag-only writes after a verification keep it)
        let changed = (ex.f("mtime") - f.mtime).abs() > 0.001 || ex.i("size") != f.size;
        if changed && ex.f("verified_at") < f.mtime - 1.0 {
            c.execute("UPDATE tracks SET verify_status=NULL WHERE id=? AND verify_status IS NOT 'manual'", p![ex.i("id")])?;
        }
        let before = ex.s("album_id");
        if !before.is_empty() && before != album_id {
            albums.push(before);
        }
    } else {
        let mut params = vec![json!(f.path), json!(album_id), json!(f.mtime), json!(f.size)];
        params.extend(f.vals.iter().cloned());
        params.extend(tag_values(&f.gain));
        // stamped as it is written, in the order the files come: `added_at` keeps the order the files were found in
        params.push(json!(now()));
        // who brought it in: only when the row is made (a rescan or a retag never changes it)
        params.extend([json!(added.by), json!(added.via)]);
        c.execute(
            &format!("INSERT INTO tracks (path, album_id, mtime, size, {}, {}, added_at, added_by, added_via) VALUES (?,?,?,?,{},?,?,?)", COLS.join(", "), COLUMNS.join(", "),
                vec!["?"; COLS.len() + COLUMNS.len()].join(",")),
            params,
        )?;
    }
    Ok(albums)
}

/// What writing a batch of files did.
#[derive(Default)]
pub struct Indexed {
    /// files written
    pub changed: usize,
    /// the albums whose songs changed (for their covers)
    pub albums: BTreeSet<String>,
    /// files that could not be read or written, with why
    pub failed: Vec<(String, String)>,
}

/// Writes `rows` in one transaction (one library version move). A file whose statements fail is left out and
/// named in `failed`; the others are saved. When the transaction itself can't be had, every file is named.
fn write_batch(db: &Db, rows: &[FileRow], added: Added, out: &mut Indexed) {
    if rows.is_empty() {
        return;
    }
    let done = db.write_now(|c| c.transaction(|c| {
        let mut res = Vec::with_capacity(rows.len());
        for f in rows {
            res.push(store_file(c, f, added));
        }
        Ok::<_, rusqlite::Error>(res)
    }));
    match done {
        Ok(res) => {
            for (f, r) in rows.iter().zip(res) {
                match r {
                    Ok(albums) => {
                        out.changed += 1;
                        out.albums.extend(albums);
                    }
                    Err(e) => out.failed.push((f.path.clone(), e.to_string())),
                }
            }
        }
        Err(e) => out.failed.extend(rows.iter().map(|f| (f.path.clone(), e.to_string()))),
    }
}

/// Reads and writes these files, [`SCAN_BATCH`] per transaction (imports, a report's fix); new songs are
/// found in the music folder ([`Added::SCAN`]).
pub fn index_files(db: &Db, paths: &[String]) -> Indexed {
    index_files_as(db, paths, Added::SCAN)
}

/// [`index_files`] with who brought the new songs in (a Files upload or import).
pub fn index_files_as(db: &Db, paths: &[String], added: Added) -> Indexed {
    let mut out = Indexed::default();
    for chunk in paths.chunks(SCAN_BATCH) {
        let mut rows = Vec::with_capacity(chunk.len());
        for path in chunk {
            match read_file(path, None) {
                Ok(r) => rows.push(r),
                Err(e) => out.failed.push((path.clone(), e.to_string())),
            }
        }
        write_batch(db, &rows, added, &mut out);
    }
    // an import of a release the library has from another source joins that album (encore_library::merge)
    if out.changed > 0 {
        // albums of one name split by release first (encore_library::split): their parts are never merged back
        if let Ok(m) = db.write_now(crate::split::split_albums) {
            out.albums.extend(m.into_iter().map(|(_, to)| to));
        }
        if let Ok(m) = db.write_now(crate::merge::merge_albums) {
            for (from, to) in m {
                if out.albums.remove(&from) {
                    out.albums.insert(to);
                }
            }
        }
        // a song the library has from MusicBrainz too: shown once (encore_library::twins)
        if let Ok(f) = db.write_now(crate::twins::find_copies) {
            for (from, to) in f.merged {
                if out.albums.remove(&from) {
                    out.albums.insert(to);
                }
            }
        }
    }
    out
}

/// One file: [`index_files`] for a single path.
pub fn index_file(db: &Db, path: &str) -> anyhow::Result<()> {
    match index_files(db, &[path.to_string()]).failed.pop() {
        Some((_, e)) => Err(anyhow::anyhow!(e)),
        None => Ok(()),
    }
}

/// What a scan did.
#[derive(Default)]
pub struct Scanned {
    /// files read in again (new or changed)
    pub changed: usize,
    /// rows removed (their files gone)
    pub removed: usize,
    /// albums whose songs were added, changed or removed
    pub albums: BTreeSet<String>,
    /// the folder had no songs although the library has songs in it (a disk that isn't mounted leaves its
    /// empty mount point behind): nothing was removed
    pub looked_empty: bool,
}

impl Library {
    /// An empty library whose background passes wait while `busy` says the app is in use.
    pub fn new(busy: encore_jobs::Busy) -> Self {
        Library { covers: encore_media::covers::CoverIndex::new(busy.clone()), busy, ..Default::default() }
    }

    /// Walk the library folder: new/changed files are read, vanished ones removed. Writes go in
    /// transactions of [`SCAN_BATCH`] files (tags are read before each one, outside it).
    pub fn scan_sync(&self, db: &Db, root: &str, full: bool) -> Scanned {
        self.scan_folder(db, root, full, 0, None)
    }

    /// [`Library::scan_sync`] as one folder of several: the progress counts on from `before` files. `skip`: the
    /// Surround folder, walked by [`crate::surround::scan`] into its own table; a stereo row under it (a file put
    /// there before 0.35) leaves the stereo library, as the Surround scan takes it.
    fn scan_folder(&self, db: &Db, root: &str, full: bool, before: usize, skip: Option<&Path>) -> Scanned {
        let prefix = format!("{}/%", root.trim_end_matches('/'));
        // LIKE ignores the case of ASCII letters and reads `_` as any letter: only the rows really in this folder
        // (another library folder may be named like it)
        let known: HashMap<String, f64> = db.query("SELECT path, mtime FROM tracks WHERE path LIKE ?", p![prefix]).unwrap_or_default()
            .into_iter().map(|r| (r.s("path"), r.f("mtime"))).filter(|(path, _)| Path::new(path).starts_with(root)).collect();
        // what the walk couldn't read (a folder without permission, a link to a disk that is away): the rows
        // under it are not taken for vanished files; an error without a path keeps every row
        let (mut unreadable, mut unread_all): (Vec<PathBuf>, bool) = (vec![], false);
        let files: Vec<PathBuf> = walkdir::WalkDir::new(root).follow_links(true).into_iter()
            .filter_entry(|e| e.depth() == 0 || (!e.file_name().to_string_lossy().starts_with('.') && skip.is_none_or(|s| e.path() != s)))
            .filter_map(|e| match e {
                Ok(e) => Some(e),
                Err(err) => {
                    match err.path() {
                        Some(p) => unreadable.push(p.to_path_buf()),
                        None => unread_all = true,
                    }
                    None
                }
            })
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                !name.starts_with("._") && p.extension().map(|x| AUDIO_EXTS.contains(&x.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)
            })
            .collect();
        self.scan.lock().unwrap().total = before + files.len();
        let mut seen = HashSet::new();
        let mut out = Indexed::default();
        let mut batch: Vec<FileRow> = Vec::with_capacity(SCAN_BATCH);
        for (i, f) in files.iter().enumerate() {
            self.scan.lock().unwrap().progress = before + i + 1;
            let path = f.to_string_lossy().into_owned();
            seen.insert(path.clone());
            let Ok(meta) = std::fs::metadata(f) else { continue };
            let (mtime, _) = mtime_size(&meta);
            if !full && known.get(&path).is_some_and(|k| (k - mtime).abs() < 0.001) {
                continue;
            }
            match read_file(&path, Some(meta)) {
                Ok(r) => batch.push(r),
                Err(e) => out.failed.push((path, e.to_string())),
            }
            if batch.len() >= SCAN_BATCH {
                write_batch(db, &batch, Added::SCAN, &mut out);
                batch.clear();
            }
        }
        write_batch(db, &batch, Added::SCAN, &mut out);
        for (path, e) in &out.failed {
            db.log("warning", &format!("Could not read {path}: {e}"));
        }
        // a folder with no songs at all while the library has songs in it is taken for a disk that isn't there
        // (an unmounted disk leaves its empty mount point): its songs stay, as for a folder that is missing
        let looked_empty = files.is_empty() && !known.is_empty();
        let removed: Vec<&String> = if looked_empty || unread_all {
            vec![]
        } else {
            known.keys().filter(|p| !seen.contains(*p) && !unreadable.iter().any(|u| Path::new(p.as_str()).starts_with(u))).collect()
        };
        self.warn_once(db, &format!("empty:{root}"), looked_empty,
            &format!("Library folder {root} looks empty: not scanned as empty, its songs stay in the library (is its disk mounted?)"));
        for u in &unreadable {
            self.warn_once(db, &format!("unreadable:{}", u.display()), true, &format!("Couldn't read {} while scanning: the songs under it stay in the library", u.display()));
        }
        self.warn_once(db, &format!("unreadable:{root}"), unread_all, &format!("Couldn't read all of {root} while scanning: no songs were removed"));
        let mut albums = out.albums;
        for chunk in removed.chunks(500) {
            let marks = crate::artists::marks(chunk.len());
            let paths: Vec<Value> = chunk.iter().map(|p| json!(p)).collect();
            let gone = db.write_now(|c| c.transaction(|c| {
                let albums = c.query(&format!("SELECT DISTINCT album_id FROM tracks WHERE path IN ({marks})"), paths.clone())?;
                c.execute(&format!("UPDATE playlist_items SET track_id=NULL WHERE track_id IN (SELECT id FROM tracks WHERE path IN ({marks}))"), paths.clone())?;
                c.execute(&format!("UPDATE surround_tracks SET stereo_track_id=NULL WHERE stereo_track_id IN (SELECT id FROM tracks WHERE path IN ({marks}))"), paths.clone())?;
                c.execute(&format!("DELETE FROM tracks WHERE path IN ({marks})"), paths.clone())?;
                Ok::<_, rusqlite::Error>(albums)
            }));
            match gone {
                Ok(rows) => albums.extend(rows.iter().map(|r| r.s("album_id")).filter(|a| !a.is_empty())),
                Err(e) => tracing::warn!("scan: removing {} vanished files failed: {e}", chunk.len()),
            }
        }
        Scanned { changed: out.changed, removed: removed.len(), albums, looked_empty }
    }

    /// A warning about a library folder in the activity log, once until the folder is fine again (`on` false
    /// clears it).
    fn warn_once(&self, db: &Db, key: &str, on: bool, text: &str) {
        let mut w = self.warned.lock().unwrap_or_else(|e| e.into_inner());
        if !on {
            w.remove(key);
        } else if w.insert(key.to_string()) {
            db.log("warning", text);
        }
    }

    /// Scan everything the settings name: `library_path`, the extra library folders and the Surround folder (whose
    /// ignore files for the media servers are written first, see [`crate::surround::write_ignore_files`]).
    pub async fn scan_library(&self, db: Db, cfg: &encore_config::Config, full: bool) {
        crate::surround::write_ignore_files(cfg);
        self.scan_all(db, crate::folders::scan_roots(cfg), crate::surround::root(cfg), full).await
    }

    /// Scan one folder (see [`Library::scan_all`]).
    pub async fn scan(&self, db: Db, root: String, full: bool) {
        self.scan_all(db, vec![root], None, full).await
    }

    /// Scan every library folder in one scanning session: `library_path` and the extra library folders
    /// (`library_extra_roots`, see [`crate::folders::scan_roots`]). A folder that isn't there is skipped, never
    /// taken as empty: its songs stay in the library until it is back. Each folder's scan only looks at and
    /// removes the rows under that folder, and extra folders are never inside each other or the library.
    /// `surround`: the Surround folder ([`crate::surround::root`]), left out of the stereo scan and scanned after it
    /// into the Surround library.
    pub async fn scan_all(&self, db: Db, roots: Vec<String>, surround: Option<PathBuf>, full: bool) {
        let mut roots: Vec<String> = roots.iter().map(|r| r.trim()).filter(|r| !r.is_empty())
            .map(|r| if r.len() > 1 { r.trim_end_matches('/') } else { r }.to_string()).collect();
        // a folder inside one scanned before it (typed into the setting by hand) is walked with that one
        let mut i = 0;
        while i < roots.len() {
            if roots[..i].iter().any(|r| Path::new(&roots[i]).starts_with(r)) {
                roots.remove(i);
            } else {
                i += 1;
            }
        }
        let (here, gone): (Vec<String>, Vec<String>) = roots.into_iter().partition(|r| Path::new(r).is_dir());
        if here.is_empty() {
            let first = gone.first().cloned().unwrap_or_default();
            self.scan.lock().unwrap().message = format!("Library path {first} does not exist");
            return;
        }
        {
            let mut s = self.scan.lock().unwrap();
            if s.scanning {
                return;
            }
            *s = ScanState { scanning: true, message: "Scanning…".into(), ..s.clone() };
        }
        for r in &gone {
            self.warn_once(&db, &format!("gone:{r}"), true, &format!("Library folder {r} isn't there: not scanned, its songs stay in the library"));
        }
        for r in &here {
            self.warn_once(&db, &format!("gone:{r}"), false, "");
        }
        let me = self.clone();
        let db2 = db.clone();
        let done = tokio::task::spawn_blocking(move || {
            let mut all = (Scanned::default(), vec![]);
            for (i, root) in here.iter().enumerate() {
                // the progress goes on over the folders: the files of the folders before this one are done
                let before = if i == 0 { 0 } else { me.scan.lock().unwrap().total };
                let n = me.scan_folder(&db2, root, full, before, surround.as_deref());
                all.0.changed += n.changed;
                all.0.removed += n.removed;
                all.0.albums.extend(n.albums);
                if n.looked_empty {
                    all.1.push(root.clone());
                }
            }
            if let Some(s) = &surround {
                let n = crate::surround::scan(&db2, s, full);
                all.0.changed += n.changed;
                all.0.removed += n.removed;
                all.0.albums.extend(n.albums);
            }
            // albums of one name (Weezer's self-titled albums): one album per release (no file is touched)
            match db2.write_now(crate::split::split_albums) {
                Ok(m) if !m.is_empty() => {
                    db2.log("info", &format!("Split {} album{} that shared a name with another release", m.len(), if m.len() == 1 { "" } else { "s" }));
                    all.0.changed += m.len();
                    all.0.albums.extend(m.into_iter().flat_map(|(from, to)| [from, to]));
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("scan: splitting albums of one name failed: {e}"),
            }
            // the same release from MusicBrainz and from another source: one album (no file is touched)
            match db2.write_now(crate::merge::merge_albums) {
                Ok(m) if !m.is_empty() => {
                    db2.log("info", &format!("Merged {} duplicate album{} into their MusicBrainz or fuller copy", m.len(), if m.len() == 1 { "" } else { "s" }));
                    all.0.changed += m.len();
                    all.0.albums.extend(m.into_iter().map(|(_, to)| to));
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("scan: merging duplicate albums failed: {e}"),
            }
            // songs without MusicBrainz's tags that the library has from MusicBrainz: shown once (no file is touched)
            match db2.write_now(crate::twins::find_copies) {
                Ok(f) if f.changed > 0 || !f.merged.is_empty() => {
                    if f.changed > 0 {
                        db2.log("info", &format!("{} song{} without MusicBrainz tags matched a MusicBrainz song (shown once)", f.changed, if f.changed == 1 { "" } else { "s" }));
                    }
                    all.0.changed += f.changed + f.merged.len();
                    all.0.albums.extend(f.merged.into_iter().map(|(_, to)| to));
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("scan: finding copies of songs failed: {e}"),
            }
            // the planner's statistics follow what the scan's queries wanted (cheap when there is nothing to do)
            db2.optimize();
            all
        }).await.unwrap_or_default();
        let (done, empty) = done;
        let (changed, removed) = (done.changed, done.removed);
        let mut msg = format!("Scan complete: {changed} updated, {removed} removed");
        if !gone.is_empty() {
            msg.push_str(&format!(" (not there, so not scanned: {})", gone.join(", ")));
        }
        if !empty.is_empty() {
            msg.push_str(&format!(" (looks empty, so its songs were kept: {})", empty.join(", ")));
        }
        if changed + removed > 0 {
            db.log("info", &msg);
            // thumbnails and colours of the albums that changed; the others' covers are left alone (a scan
            // that found nothing new leaves the disk alone, one that found one album looks at that album)
            self.refresh_covers(db.clone(), done.albums.into_iter().collect());
        }
        let mut s = self.scan.lock().unwrap();
        s.scanning = false;
        s.last_scan = Some(now());
        s.message = msg;
    }

    // ---- covers --------------------------------------------------------------------------------

    /// Forget an album's cover (all of them with None), so it is looked up again.
    pub fn forget_cover(&self, album_id: Option<&str>) {
        self.covers.forget(album_id);
    }

    /// Folder picture (cover.jpg, folder.png, …) or the art embedded in the first file (cached).
    pub fn find_cover(&self, db: &Db, album_id: &str) -> Option<PathBuf> {
        self.covers.get(db, album_id).map(|c| c.0)
    }

    /// Make the grid/list cover thumbnails for every album in the background (at start).
    pub fn warm_covers(&self, db: Db) {
        self.covers.warm(db);
    }

    /// Look again at these albums' covers and make their thumbnails and colours in the background (after
    /// a scan or an import changed their songs); albums left without songs lose theirs.
    pub fn refresh_covers(&self, db: Db, album_ids: Vec<String>) {
        self.covers.refresh(db, album_ids);
    }
}

/// The library's totals for Settings → Library: four GROUP BYs over every song (on a `Db` or a lent `Conn`).
pub fn stats(db: &impl encore_db::Query) -> Value {
    let mut s = db.one("SELECT COUNT(*) tracks, COUNT(DISTINCT album_id) albums, COUNT(DISTINCT albumartist) artists, COALESCE(SUM(size),0) size, COALESCE(SUM(duration),0) duration FROM tracks", p![])
        .ok().flatten().unwrap_or_default();
    s.insert("untagged_albums".into(), json!(db.one("SELECT COUNT(DISTINCT album_id) n FROM tracks WHERE mb_albumid IS NULL OR mb_albumid=''", p![]).ok().flatten().map(|r| r.i("n")).unwrap_or(0)));
    let counts: serde_json::Map<String, Value> = db.query("SELECT verify_status, COUNT(*) n FROM tracks GROUP BY verify_status", p![]).unwrap_or_default()
        .into_iter().map(|r| { let k = r.s("verify_status"); (if k.is_empty() { "unchecked".into() } else { k }, json!(r.i("n"))) }).collect();
    s.insert("verify_counts".into(), Value::Object(counts));
    s.insert("formats".into(), json!(db.query("SELECT format, COUNT(*) n FROM tracks GROUP BY format ORDER BY n DESC", p![]).unwrap_or_default()));
    Value::Object(s)
}

/// Delete files (and their lyrics) and their rows; playlist entries move to `replacement` if given, and so does
/// who added the song ([`inherit_added`]). One lookup, one playlist update and one delete per 500 songs (in a
/// transaction).
pub fn delete_tracks(st: &impl Ctx, ids: &[i64], replacement: Option<i64>) -> usize {
    let db = st.db();
    let mut n = 0;
    let mut done: HashSet<i64> = HashSet::new();
    for chunk in ids.chunks(500) {
        let marks = crate::artists::marks(chunk.len());
        let Ok(rows) = db.query(&format!("SELECT id, path FROM tracks WHERE id IN ({marks})"), chunk.iter().map(|i| json!(i)).collect()) else { continue };
        let paths: HashMap<i64, String> = rows.iter().map(|r| (r.i("id"), r.s("path"))).collect();
        // in the order asked, each song once
        let found: Vec<i64> = chunk.iter().copied().filter(|id| paths.contains_key(id) && done.insert(*id)).collect();
        if found.is_empty() {
            continue;
        }
        for id in &found {
            let path = &paths[id];
            let _ = std::fs::remove_file(path);
            for side in ["lrc", "txt"] {
                let _ = std::fs::remove_file(Path::new(path).with_extension(side));
            }
        }
        let marks = crate::artists::marks(found.len());
        let id_params: Vec<Value> = found.iter().map(|i| json!(i)).collect();
        let mut params = vec![json!(replacement)];
        params.extend(id_params.iter().cloned());
        let _ = db.write_now(|c| c.transaction(|c| {
            if let (Some(new), Some(old)) = (replacement, found.first()) {
                inherit_added(c, *old, new)?;
            }
            c.execute(&format!("UPDATE playlist_items SET track_id=? WHERE track_id IN ({marks})"), params)?;
            // the surround versions of these songs follow the replacement, or keep no stereo song
            crate::surround::stereo_gone(c, &found, replacement)?;
            c.execute(&format!("DELETE FROM tracks WHERE id IN ({marks})"), id_params)
        }));
        for id in &found {
            st.track_removed(*id);
            n += 1;
        }
    }
    n
}

/// A new copy (`new`) takes the place of a song (`old`): it takes who added the old one, when nobody is named
/// for it yet (its `added_via` is still `scan`, a fresh import a request hasn't stamped). A user's request
/// stamps it afterwards all the same; a request without a user (a lossless upgrade) leaves it as inherited.
pub fn inherit_added(c: &Conn, old: i64, new: i64) -> rusqlite::Result<()> {
    if old == new {
        return Ok(());
    }
    c.execute("UPDATE tracks SET added_by=o.added_by, added_via=o.added_via, requested_at=o.requested_at
               FROM (SELECT added_by, added_via, requested_at FROM tracks WHERE id=?) o
               WHERE tracks.id=? AND tracks.added_via='scan' AND o.added_via IS NOT NULL", p![old, new])?;
    Ok(())
}

/// Remove empty folders (and leftover junk files) upwards from `path`, never above `stop`.
pub fn prune_empty(path: &Path, stop: &Path) {
    let mut cur = path.to_path_buf();
    while cur.starts_with(stop) && cur != stop {
        let Ok(rd) = std::fs::read_dir(&cur) else { break };
        let names: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        let junk = |p: &PathBuf| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            p.is_file() && (n.ends_with(".jpg") || n.ends_with(".png") || n.ends_with(".nfo") || n.ends_with(".txt") || n.ends_with(".lrc") || n.ends_with(".log") || n.ends_with(".cue") || n.starts_with('.'))
        };
        if names.iter().any(|p| !junk(p)) {
            break;
        }
        for p in &names {
            let _ = std::fs::remove_file(p);
        }
        if std::fs::remove_dir(&cur).is_err() {
            break;
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => break,
        }
    }
}

/// The library has songs of this MusicBrainz release group.
pub fn has_release_group(db: &Db, rgid: &str) -> bool {
    !rgid.is_empty() && db.one("SELECT 1 FROM tracks WHERE mb_releasegroupid=? LIMIT 1", p![rgid]).ok().flatten().is_some()
}

/// The library has an album of this title by this artist (album artist or artist, any case).
pub fn has_album(db: &Db, artist: &str, album: &str) -> bool {
    !album.is_empty() && db.one("SELECT 1 FROM tracks WHERE lower(album)=lower(?) AND (lower(albumartist)=lower(?) OR lower(artist)=lower(?)) LIMIT 1", p![album, artist, artist]).ok().flatten().is_some()
}


/// The library album for a MusicBrainz release group (or artist + title), so it can be played.
pub fn album_id_for(db: &Db, rgid: &str, artist: &str, album: &str) -> Option<String> {
    let mut row = if rgid.is_empty() { None } else { db.one("SELECT album_id FROM tracks WHERE mb_releasegroupid=? LIMIT 1", p![rgid]).ok().flatten() };
    if row.is_none() && !album.is_empty() {
        row = db.one("SELECT album_id FROM tracks WHERE lower(album)=lower(?) AND (lower(albumartist)=lower(?) OR lower(artist)=lower(?)) LIMIT 1", p![album, artist, artist]).ok().flatten();
    }
    row.map(|r| r.s("album_id"))
}

/// The library copy of a song (best quality first), so it can be played.
pub fn find_track(db: &Db, mbid: &str, artist: &str, title: &str) -> Option<Row> {
    const COLS: &str = "SELECT id, title, artist, album, album_id, format, duration FROM tracks";
    const ORDER: &str = " ORDER BY CASE WHEN format IN ('flac','alac','wav','aiff') THEN 0 ELSE 1 END LIMIT 1";
    if !mbid.is_empty() {
        if let Ok(Some(r)) = db.one(&format!("{COLS} WHERE mb_trackid=?{ORDER}"), p![mbid]) {
            return Some(r);
        }
    }
    db.one(&format!("{COLS} WHERE lower(title)=lower(?) AND (lower(artist)=lower(?) OR lower(albumartist)=lower(?)){ORDER}"), p![title, artist, artist]).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scan writes its files in transactions of SCAN_BATCH: the library version moves once per batch, the
    /// songs keep the order they were found in (`added_at`), every album that changed is named, and files
    /// that vanished go with their playlist entries cleared.
    #[test]
    fn the_scan_writes_in_batches_and_names_the_changed_albums() {
        let dir = encore_core::tmp::dir("scan-batches");
        let root = dir.join("music");
        // 450 songs on 9 albums; the files are empty, so the tags come from the folder names
        for i in 0..450 {
            let album = root.join(format!("Artist {}", i % 3)).join(format!("Album {}", i % 9));
            std::fs::create_dir_all(&album).unwrap();
            std::fs::write(album.join(format!("{i:03}.mp3")), b"").unwrap();
        }
        let db = Db::open(&dir.join("cfg")).unwrap();
        let lib = Library::default();
        let root_s = root.to_string_lossy().to_string();
        let v0 = db.library_version();
        let s = lib.scan_sync(&db, &root_s, false);
        assert_eq!((s.changed, s.removed), (450, 0));
        assert_eq!(db.library_version(), v0 + 3, "450 files in batches of 200: three moves of the version");
        let albums: BTreeSet<String> = db.query("SELECT DISTINCT album_id FROM tracks", p![]).unwrap().iter().map(|r| r.s("album_id")).collect();
        assert_eq!(albums.len(), 9);
        assert_eq!(s.albums, albums);
        // added_at follows the order the scan wrote them in (the row ids)
        let added: Vec<f64> = db.query("SELECT added_at FROM tracks ORDER BY id", p![]).unwrap().iter().map(|r| r.f("added_at")).collect();
        assert!(added.windows(2).all(|w| w[0] <= w[1]), "added_at keeps the scan's order");
        assert_eq!(lib.scan.lock().unwrap().progress, 450);
        // nothing new: nothing written, the version stays
        let v1 = db.library_version();
        let s = lib.scan_sync(&db, &root_s, false);
        assert_eq!((s.changed, s.removed, s.albums.len()), (0, 0, 0));
        assert_eq!(db.library_version(), v1);
        // two songs of one album vanish (one in a playlist), one song of another album changes
        let gone: Vec<i64> = db.query("SELECT id FROM tracks WHERE path LIKE '%/000.mp3' OR path LIKE '%/009.mp3' ORDER BY id", p![]).unwrap().iter().map(|r| r.i("id")).collect();
        db.execute("INSERT INTO playlists (id, user_id, name, kind, created_at, updated_at) VALUES (1, 1, 'p', 'playlist', 0, 0)", p![]).unwrap();
        db.execute("INSERT INTO playlist_items (playlist_id, track_id, position, added_at) VALUES (1, ?, 0, 0)", p![gone[0]]).unwrap();
        std::fs::remove_file(root.join("Artist 0/Album 0/000.mp3")).unwrap();
        std::fs::remove_file(root.join("Artist 0/Album 0/009.mp3")).unwrap();
        let touched = root.join("Artist 1/Album 1/001.mp3");
        let f = std::fs::File::options().write(true).open(&touched).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        drop(f);
        let s = lib.scan_sync(&db, &root_s, false);
        assert_eq!((s.changed, s.removed), (1, 2));
        let want: BTreeSet<String> = [album_key("Artist 0", "Album 0", ""), album_key("Artist 1", "Album 1", "")].into_iter().collect();
        assert_eq!(s.albums, want);
        assert_eq!(db.one("SELECT COUNT(*) n FROM tracks", p![]).unwrap().unwrap().i("n"), 448);
        let item = db.one("SELECT track_id FROM playlist_items WHERE playlist_id=1", p![]).unwrap().unwrap();
        assert!(item.get("track_id").is_some_and(|v| v.is_null()), "the playlist entry of a vanished song is cleared");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// scan_all walks the library folder and the extra library folders in one session: a folder that isn't
    /// there is skipped and keeps its songs, a folder named like another (other case, `_` for a letter) keeps
    /// its own rows (SQLite's LIKE would mix them up), a folder inside an earlier one is walked once.
    #[tokio::test]
    async fn scan_all_walks_every_library_folder_and_skips_a_missing_one() {
        let dir = encore_core::tmp::dir("scan-all");
        let add = |root: &Path, rel: &str| {
            let f = root.join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, b"").unwrap();
        };
        let (music, lower, under, other, usb) = (dir.join("Music"), dir.join("music"), dir.join("x_y"), dir.join("xzy"), dir.join("usb"));
        add(&music, "A/One/1.mp3");
        add(&lower, "B/Two/1.mp3");
        add(&lower, "B/Two/2.mp3");
        add(&under, "C/Three/1.mp3");
        add(&other, "D/Four/1.mp3");
        add(&usb, "E/Five/1.mp3");
        let db = Db::open(&dir.join("cfg")).unwrap();
        let lib = Library::default();
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let roots = vec![text(&music), format!("{}/", text(&lower)), text(&under), text(&other), text(&usb)];
        let ids = |db: &Db| -> Vec<(i64, String)> {
            db.query("SELECT id, path FROM tracks ORDER BY id", p![]).unwrap().iter().map(|r| (r.i("id"), r.s("path"))).collect()
        };
        lib.scan_all(db.clone(), roots.clone(), None, false).await;
        let first = ids(&db);
        assert_eq!(first.len(), 6, "{first:?}");
        {
            let s = lib.scan.lock().unwrap();
            assert_eq!((s.scanning, s.progress, s.total), (false, 6, 6), "the progress goes on over the folders");
            assert_eq!(s.message, "Scan complete: 6 updated, 0 removed");
        }
        // the USB disk is gone: its song stays, and no folder takes another's rows for vanished files
        std::fs::remove_dir_all(&usb).unwrap();
        lib.scan_all(db.clone(), roots.clone(), None, false).await;
        assert_eq!(ids(&db), first, "same rows, same ids");
        assert_eq!(lib.scan.lock().unwrap().message, format!("Scan complete: 0 updated, 0 removed (not there, so not scanned: {})", text(&usb)));
        // a song leaves an extra folder: only that one goes
        std::fs::remove_file(lower.join("B/Two/2.mp3")).unwrap();
        lib.scan_all(db.clone(), roots.clone(), None, false).await;
        let left = ids(&db);
        assert_eq!(left.len(), 5);
        assert!(left.iter().all(|(_, p)| !p.ends_with("Two/2.mp3")));
        assert!(left.iter().any(|(_, p)| p.ends_with("E/Five/1.mp3")), "the missing folder's song is still there");
        // only missing folders: nothing is scanned
        lib.scan_all(db.clone(), vec![text(&usb)], None, false).await;
        assert_eq!(lib.scan.lock().unwrap().message, format!("Library path {} does not exist", text(&usb)));
        assert_eq!(ids(&db), left);
        // a folder inside an earlier one is walked with it, once
        lib.scan_all(db.clone(), vec![text(&music), text(&music.join("A"))], None, true).await;
        assert_eq!(lib.scan.lock().unwrap().total, 1);
        assert_eq!(ids(&db), left, "a full scan rewrites the rows in place");
        // a disk that isn't mounted leaves its empty mount point: the folder is not taken for one whose songs
        // all went, and the warning is written once
        std::fs::remove_dir_all(other.join("D")).unwrap();
        for _ in 0..2 {
            lib.scan_all(db.clone(), roots.clone(), None, false).await;
            assert_eq!(ids(&db), left, "the empty folder's song stays");
        }
        let msg = lib.scan.lock().unwrap().message.clone();
        assert!(msg.ends_with(&format!("(looks empty, so its songs were kept: {})", text(&other))), "{msg}");
        let warned = |what: &str| db.query("SELECT message FROM activity WHERE message LIKE ?", p![format!("%{what}%")]).unwrap().len();
        assert_eq!(warned("looks empty"), 1, "warned once, not on every scan");
        assert_eq!(warned(&format!("Library folder {} isn't there", text(&usb))), 1);
        // a link to a folder that is away (a walk error): the songs under it stay, the others are scanned
        let away = dir.join("away");
        add(&away, "G/1.mp3");
        std::os::unix::fs::symlink(&away, music.join("Linked")).unwrap();
        lib.scan_all(db.clone(), roots.clone(), None, false).await;
        let linked = ids(&db);
        assert!(linked.iter().any(|(_, p)| p.ends_with("Linked/G/1.mp3")), "{linked:?}");
        std::fs::remove_dir_all(&away).unwrap();
        std::fs::remove_file(music.join("A/One/1.mp3")).unwrap();
        add(&music, "A/One/2.mp3");
        lib.scan_all(db.clone(), roots.clone(), None, false).await;
        let now = ids(&db);
        assert!(now.iter().any(|(_, p)| p.ends_with("Linked/G/1.mp3")), "the song behind the broken link stays: {now:?}");
        assert!(!now.iter().any(|(_, p)| p.ends_with("A/One/1.mp3")) && now.iter().any(|(_, p)| p.ends_with("A/One/2.mp3")), "the rest is scanned: {now:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Who added a song is written when its row is made, never by a rescan; a new file's type tags are read
    /// ('' / 0 without them); a new copy takes who added the copy it replaces only while nobody is named for it.
    #[test]
    fn who_added_is_kept_on_the_song() {
        let dir = encore_core::tmp::dir("scan-added");
        let album = dir.join("music/A/B");
        std::fs::create_dir_all(&album).unwrap();
        for n in ["1.flac", "2.flac", "3.flac"] {
            std::fs::write(album.join(n), b"").unwrap();
        }
        let db = Db::open(&dir.join("cfg")).unwrap();
        let path = |n: &str| album.join(n).to_string_lossy().to_string();
        index_files(&db, &[path("1.flac")]);
        index_files_as(&db, &[path("2.flac")], Added { by: Some(7), via: "upload" });
        let row = |n: &str| db.one("SELECT id, added_by, added_via, requested_at, releasetype, tracktotal, compilation FROM tracks WHERE path=?", p![path(n)]).unwrap().unwrap();
        let r1 = row("1.flac");
        assert_eq!((r1.s("added_via"), r1.get("added_by").unwrap().is_null()), ("scan".to_string(), true));
        assert_eq!((r1.s("releasetype"), r1.i("tracktotal"), r1.i("compilation")), (String::new(), 0, 0), "read: '' / 0 without the tags");
        let r2 = row("2.flac");
        assert_eq!((r2.s("added_via"), r2.i("added_by")), ("upload".to_string(), 7));
        // a request stamped song 1; a full rescan (and an index of the same file) never changes it
        db.execute("UPDATE tracks SET added_by=3, added_via='request', requested_at=1000 WHERE id=?", p![r1.i("id")]).unwrap();
        Library::default().scan_sync(&db, &dir.join("music").to_string_lossy(), true);
        index_files_as(&db, &[path("1.flac")], Added { by: Some(9), via: "import" });
        let r1 = row("1.flac");
        assert_eq!((r1.s("added_via"), r1.i("added_by"), r1.f("requested_at")), ("request".to_string(), 3, 1000.0));
        // song 3 is found by the scan: a fresh copy that takes song 1's place inherits who asked for it …
        let r3 = row("3.flac");
        assert_eq!(r3.s("added_via"), "scan");
        db.write_now(|c| inherit_added(c, r1.i("id"), r3.i("id"))).unwrap();
        let r3 = row("3.flac");
        assert_eq!((r3.s("added_via"), r3.i("added_by"), r3.f("requested_at")), ("request".to_string(), 3, 1000.0));
        // … but a copy someone is named for keeps its own
        db.write_now(|c| inherit_added(c, r1.i("id"), r2.i("id"))).unwrap();
        assert_eq!(row("2.flac").s("added_via"), "upload");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// index_files reads and writes a list of files; one that can't be read is named, the others are saved.
    #[test]
    fn index_files_names_what_it_could_not_read() {
        let dir = encore_core::tmp::dir("index-files");
        let album = dir.join("music/A/B");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("1.flac"), b"").unwrap();
        let db = Db::open(&dir.join("cfg")).unwrap();
        let paths = vec![album.join("1.flac").to_string_lossy().to_string(), album.join("missing.flac").to_string_lossy().to_string()];
        let out = index_files(&db, &paths);
        assert_eq!(out.changed, 1);
        assert_eq!(out.failed.len(), 1);
        assert!(out.failed[0].0.ends_with("missing.flac"));
        assert_eq!(out.albums.len(), 1);
        assert!(index_file(&db, &paths[1]).is_err());
        assert!(index_file(&db, &paths[0]).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
