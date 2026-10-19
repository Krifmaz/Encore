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
const COLS: &[&str] = &["title", "artist", "album", "albumartist", "track", "disc", "year", "genre", "duration", "bitrate", "format",
    "mb_trackid", "mb_albumid", "mb_releasegroupid", "mb_artistid", "mb_albumartistid", "isrc"];

#[derive(Default, Clone, serde::Serialize)]
pub struct ScanState {
    pub scanning: bool,
    pub progress: usize,
    pub total: usize,
    pub last_scan: Option<f64>,
    pub message: String,
}

#[derive(Clone, Default)]
pub struct Library {
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
}

pub fn album_key(albumartist: &str, album: &str, path: &str) -> String {
    let key = if !album.is_empty() {
        format!("{}|{}", albumartist.trim().to_lowercase(), album.trim().to_lowercase())
    } else {
        format!("dir|{}", Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
    };
    hex::encode(Sha1::digest(key.as_bytes()))[..16].to_string()
}

use encore_media::tags::read_tags;

fn mtime_size(meta: &std::fs::Metadata) -> (f64, i64) {
    let mt = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    (mt, meta.len() as i64)
}

/// Files the scan writes per transaction: the library version moves once per batch, so the lists and the
/// artist index are built again once per 200 songs during a full rescan instead of after every song.
pub const SCAN_BATCH: usize = 200;

/// What one file's row will hold: read from the file (its tags), before the database is touched.
pub struct FileRow {
    path: String,
    album_id: String,
    mtime: f64,
    size: i64,
    vals: Vec<Value>,
}

/// Reads a file's tags (no database work): `meta` is its metadata when the caller has it already.
fn read_file(path: &str, meta: Option<std::fs::Metadata>) -> std::io::Result<FileRow> {
    let meta = match meta {
        Some(m) => m,
        None => std::fs::metadata(path)?,
    };
    let (mtime, size) = mtime_size(&meta);
    let t = read_tags(path);
    let album_id = album_key(&t.s("albumartist"), &t.s("album"), path);
    let vals: Vec<Value> = COLS.iter().map(|c| t.get(*c).cloned().unwrap_or(Value::Null)).collect();
    Ok(FileRow { path: path.to_string(), album_id, mtime, size, vals })
}

/// Writes one file's row (a new song, or the new tags of a known one). The albums it touched come back: its
/// album, and the one it was in before when its tags moved it.
fn store_file(c: &Conn, f: &FileRow) -> rusqlite::Result<Vec<String>> {
    let mut albums = vec![f.album_id.clone()];
    if let Some(ex) = c.one("SELECT id, mtime, size, verified_at, album_id FROM tracks WHERE path=?", p![f.path])? {
        let sets = COLS.iter().map(|c| format!("{c}=?")).collect::<Vec<_>>().join(", ");
        let mut params = vec![json!(f.album_id), json!(f.mtime), json!(f.size)];
        params.extend(f.vals.iter().cloned());
        params.push(json!(ex.i("id")));
        c.execute(&format!("UPDATE tracks SET album_id=?, mtime=?, size=?, {sets} WHERE id=?"), params)?;
        // the audio may have changed: check it again (tag-only writes after a verification keep it)
        let changed = (ex.f("mtime") - f.mtime).abs() > 0.001 || ex.i("size") != f.size;
        if changed && ex.f("verified_at") < f.mtime - 1.0 {
            c.execute("UPDATE tracks SET verify_status=NULL WHERE id=? AND verify_status IS NOT 'manual'", p![ex.i("id")])?;
        }
        let before = ex.s("album_id");
        if !before.is_empty() && before != f.album_id {
            albums.push(before);
        }
    } else {
        let mut params = vec![json!(f.path), json!(f.album_id), json!(f.mtime), json!(f.size)];
        params.extend(f.vals.iter().cloned());
        // stamped as it is written, in the order the files come: `added_at` keeps the order the files were found in
        params.push(json!(now()));
        c.execute(
            &format!("INSERT INTO tracks (path, album_id, mtime, size, {}, added_at) VALUES (?,?,?,?,{},?)", COLS.join(", "), vec!["?"; COLS.len()].join(",")),
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
fn write_batch(db: &Db, rows: &[FileRow], out: &mut Indexed) {
    if rows.is_empty() {
        return;
    }
    let done = db.write_now(|c| c.transaction(|c| {
        let mut res = Vec::with_capacity(rows.len());
        for f in rows {
            res.push(store_file(c, f));
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

/// Reads and writes these files, [`SCAN_BATCH`] per transaction (imports, a report's fix).
pub fn index_files(db: &Db, paths: &[String]) -> Indexed {
    let mut out = Indexed::default();
    for chunk in paths.chunks(SCAN_BATCH) {
        let mut rows = Vec::with_capacity(chunk.len());
        for path in chunk {
            match read_file(path, None) {
                Ok(r) => rows.push(r),
                Err(e) => out.failed.push((path.clone(), e.to_string())),
            }
        }
        write_batch(db, &rows, &mut out);
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
    pub changed: usize,
    pub removed: usize,
    /// albums whose songs were added, changed or removed
    pub albums: BTreeSet<String>,
}

impl Library {
    /// An empty library whose background passes wait while `busy` says the app is in use.
    pub fn new(busy: encore_jobs::Busy) -> Self {
        Library { covers: encore_media::covers::CoverIndex::new(busy), ..Default::default() }
    }

    /// Walk the library folder: new/changed files are read, vanished ones removed. Writes go in
    /// transactions of [`SCAN_BATCH`] files (tags are read before each one, outside it).
    pub fn scan_sync(&self, db: &Db, root: &str, full: bool) -> Scanned {
        let prefix = format!("{}/%", root.trim_end_matches('/'));
        let known: HashMap<String, f64> = db.query("SELECT path, mtime FROM tracks WHERE path LIKE ?", p![prefix]).unwrap_or_default()
            .into_iter().map(|r| (r.s("path"), r.f("mtime"))).collect();
        let files: Vec<PathBuf> = walkdir::WalkDir::new(root).follow_links(true).into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                !name.starts_with("._") && p.extension().map(|x| AUDIO_EXTS.contains(&x.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)
            })
            .collect();
        self.scan.lock().unwrap().total = files.len();
        let mut seen = HashSet::new();
        let mut out = Indexed::default();
        let mut batch: Vec<FileRow> = Vec::with_capacity(SCAN_BATCH);
        for (i, f) in files.iter().enumerate() {
            self.scan.lock().unwrap().progress = i + 1;
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
                write_batch(db, &batch, &mut out);
                batch.clear();
            }
        }
        write_batch(db, &batch, &mut out);
        for (path, e) in &out.failed {
            db.log("warning", &format!("Could not read {path}: {e}"));
        }
        let removed: Vec<&String> = known.keys().filter(|p| !seen.contains(*p)).collect();
        let mut albums = out.albums;
        for chunk in removed.chunks(500) {
            let marks = crate::artists::marks(chunk.len());
            let paths: Vec<Value> = chunk.iter().map(|p| json!(p)).collect();
            let gone = db.write_now(|c| c.transaction(|c| {
                let albums = c.query(&format!("SELECT DISTINCT album_id FROM tracks WHERE path IN ({marks})"), paths.clone())?;
                c.execute(&format!("UPDATE playlist_items SET track_id=NULL WHERE track_id IN (SELECT id FROM tracks WHERE path IN ({marks}))"), paths.clone())?;
                c.execute(&format!("DELETE FROM tracks WHERE path IN ({marks})"), paths.clone())?;
                Ok::<_, rusqlite::Error>(albums)
            }));
            match gone {
                Ok(rows) => albums.extend(rows.iter().map(|r| r.s("album_id")).filter(|a| !a.is_empty())),
                Err(e) => tracing::warn!("scan: removing {} vanished files failed: {e}", chunk.len()),
            }
        }
        Scanned { changed: out.changed, removed: removed.len(), albums }
    }

    pub async fn scan(&self, db: Db, root: String, full: bool) {
        if !Path::new(&root).is_dir() {
            self.scan.lock().unwrap().message = format!("Library path {root} does not exist");
            return;
        }
        {
            let mut s = self.scan.lock().unwrap();
            if s.scanning {
                return;
            }
            *s = ScanState { scanning: true, message: "Scanning…".into(), ..s.clone() };
        }
        let me = self.clone();
        let db2 = db.clone();
        let done = tokio::task::spawn_blocking(move || {
            let n = me.scan_sync(&db2, &root, full);
            // the planner's statistics follow what the scan's queries wanted (cheap when there is nothing to do)
            db2.optimize();
            n
        }).await.unwrap_or_default();
        let (changed, removed) = (done.changed, done.removed);
        let msg = format!("Scan complete: {changed} updated, {removed} removed");
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

/// Delete files (and their lyrics) and their rows; playlist entries move to `replacement` if given. One
/// lookup, one playlist update and one delete per 500 songs (in a transaction).
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
            c.execute(&format!("UPDATE playlist_items SET track_id=? WHERE track_id IN ({marks})"), params)?;
            c.execute(&format!("DELETE FROM tracks WHERE id IN ({marks})"), id_params)
        }));
        for id in &found {
            st.track_removed(*id);
            n += 1;
        }
    }
    n
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

pub fn has_release_group(db: &Db, rgid: &str) -> bool {
    !rgid.is_empty() && db.one("SELECT 1 FROM tracks WHERE mb_releasegroupid=? LIMIT 1", p![rgid]).ok().flatten().is_some()
}

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
