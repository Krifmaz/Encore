//! The music library: scan files into the database, read tags, find covers, make thumbnails.
//! Album ids are computed exactly like older servers did (sha1 of "albumartist|album"), so links keep working.
use crate::db::{now, Db, Row, RowExt};
use crate::p;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey, Tag};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const AUDIO_EXT: &[&str] = &["flac", "mp3", "m4a", "mp4", "aac", "ogg", "opus", "wav", "aiff", "aif", "wma", "ape", "wv", "alac"];
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "artwork"];
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
    /// album → cover file + version (see covers.rs)
    pub covers: crate::covers::CoverIndex,
}

pub fn album_key(albumartist: &str, album: &str, path: &str) -> String {
    let key = if !album.is_empty() {
        format!("{}|{}", albumartist.trim().to_lowercase(), album.trim().to_lowercase())
    } else {
        format!("dir|{}", Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
    };
    hex::encode(Sha1::digest(key.as_bytes()))[..16].to_string()
}

fn first(tag: Option<&Tag>, keys: &[ItemKey]) -> Option<String> {
    let tag = tag?;
    keys.iter().find_map(|k| tag.get_string(k.clone()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

fn num(s: Option<String>) -> Option<i64> {
    s.and_then(|v| v.split('/').next().and_then(|n| n.trim().parse().ok()))
}

/// Tags of one file, same shape as on older servers (missing artist/album fall back to the folder names).
pub fn read_tags(path: &str) -> Row {
    let tagged = lofty::read_from_path(path).ok();
    let tag = tagged.as_ref().and_then(|t| t.primary_tag().or_else(|| t.first_tag()));
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title = first(tag, &[ItemKey::TrackTitle]).unwrap_or(stem);
    let mut artist = first(tag, &[ItemKey::TrackArtist]);
    let mut album = first(tag, &[ItemKey::AlbumTitle]);
    let mut albumartist = first(tag, &[ItemKey::AlbumArtist]).or(artist.clone());
    if artist.is_none() {
        let parts: Vec<String> = Path::new(path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if parts.len() >= 3 {
            artist = Some(parts[parts.len() - 3].clone());
            albumartist = artist.clone();
            album = album.or(Some(parts[parts.len() - 2].clone()));
        }
    }
    let props = tagged.as_ref().map(|t| t.properties());
    let duration = props.map(|p| (p.duration().as_secs_f64() * 100.0).round() / 100.0).unwrap_or(0.0);
    let bitrate = props.and_then(|p| p.overall_bitrate().or(p.audio_bitrate())).unwrap_or(0);
    let year: String = first(tag, &[ItemKey::RecordingDate, ItemKey::OriginalReleaseDate, ItemKey::Year]).unwrap_or_default().chars().take(4).collect();
    let isrc = first(tag, &[ItemKey::Isrc]).map(|s| s.to_uppercase().replace('-', "")).filter(|s| !s.is_empty());
    let mut m = Row::new();
    m.insert("title".into(), json!(title));
    m.insert("artist".into(), json!(artist.clone().unwrap_or_else(|| "Unknown Artist".into())));
    m.insert("album".into(), json!(album));
    m.insert("albumartist".into(), json!(albumartist.or(artist).unwrap_or_else(|| "Unknown Artist".into())));
    m.insert("track".into(), json!(num(first(tag, &[ItemKey::TrackNumber])).or_else(|| tag.and_then(|t| t.track()).map(|n| n as i64))));
    m.insert("disc".into(), json!(num(first(tag, &[ItemKey::DiscNumber])).or_else(|| tag.and_then(|t| t.disk()).map(|n| n as i64)).unwrap_or(1)));
    m.insert("year".into(), json!(year));
    m.insert("genre".into(), json!(first(tag, &[ItemKey::Genre])));
    m.insert("duration".into(), json!(duration));
    m.insert("bitrate".into(), json!(bitrate));
    m.insert("format".into(), json!(Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()));
    m.insert("mb_trackid".into(), json!(first(tag, &[ItemKey::MusicBrainzRecordingId])));
    m.insert("mb_albumid".into(), json!(first(tag, &[ItemKey::MusicBrainzReleaseId])));
    m.insert("mb_releasegroupid".into(), json!(first(tag, &[ItemKey::MusicBrainzReleaseGroupId])));
    m.insert("mb_artistid".into(), json!(first(tag, &[ItemKey::MusicBrainzArtistId])));
    m.insert("mb_albumartistid".into(), json!(first(tag, &[ItemKey::MusicBrainzReleaseArtistId])));
    m.insert("isrc".into(), json!(isrc));
    m
}

fn mtime_size(meta: &std::fs::Metadata) -> (f64, i64) {
    let mt = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    (mt, meta.len() as i64)
}

pub fn index_file(db: &Db, path: &str) -> anyhow::Result<()> {
    let meta = std::fs::metadata(path)?;
    let (mtime, size) = mtime_size(&meta);
    let t = read_tags(path);
    let aid = album_key(&t.s("albumartist"), &t.s("album"), path);
    let vals: Vec<Value> = COLS.iter().map(|c| t.get(*c).cloned().unwrap_or(Value::Null)).collect();
    if let Some(ex) = db.one("SELECT id, mtime, size, verified_at FROM tracks WHERE path=?", p![path])? {
        let sets = COLS.iter().map(|c| format!("{c}=?")).collect::<Vec<_>>().join(", ");
        let mut params = vec![json!(aid), json!(mtime), json!(size)];
        params.extend(vals);
        params.push(json!(ex.i("id")));
        db.execute(&format!("UPDATE tracks SET album_id=?, mtime=?, size=?, {sets} WHERE id=?"), params)?;
        // the audio may have changed: check it again (tag-only writes after a verification keep it)
        let changed = (ex.f("mtime") - mtime).abs() > 0.001 || ex.i("size") != size;
        if changed && ex.f("verified_at") < mtime - 1.0 {
            db.execute("UPDATE tracks SET verify_status=NULL WHERE id=? AND verify_status IS NOT 'manual'", p![ex.i("id")])?;
        }
    } else {
        let mut params = vec![json!(path), json!(aid), json!(mtime), json!(size)];
        params.extend(vals);
        params.push(json!(now()));
        db.execute(
            &format!("INSERT INTO tracks (path, album_id, mtime, size, {}, added_at) VALUES (?,?,?,?,{},?)", COLS.join(", "), vec!["?"; COLS.len()].join(",")),
            params,
        )?;
    }
    Ok(())
}

impl Library {
    /// Walk the library folder: new/changed files are read, vanished ones removed.
    pub fn scan_sync(&self, db: &Db, root: &str, full: bool) -> (usize, usize) {
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
                !name.starts_with("._") && p.extension().map(|x| AUDIO_EXT.contains(&x.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)
            })
            .collect();
        self.scan.lock().unwrap().total = files.len();
        let mut seen = HashSet::new();
        let mut changed = 0;
        for (i, f) in files.iter().enumerate() {
            self.scan.lock().unwrap().progress = i + 1;
            let path = f.to_string_lossy().into_owned();
            seen.insert(path.clone());
            let Ok(meta) = std::fs::metadata(f) else { continue };
            let (mtime, _) = mtime_size(&meta);
            if !full && known.get(&path).is_some_and(|k| (k - mtime).abs() < 0.001) {
                continue;
            }
            match index_file(db, &path) {
                Ok(()) => changed += 1,
                Err(e) => db.log("warning", &format!("Could not read {path}: {e}")),
            }
        }
        let removed: Vec<&String> = known.keys().filter(|p| !seen.contains(*p)).collect();
        for p in &removed {
            let _ = db.execute("UPDATE playlist_items SET track_id=NULL WHERE track_id=(SELECT id FROM tracks WHERE path=?)", p![p]);
            let _ = db.execute("DELETE FROM tracks WHERE path=?", p![p]);
        }
        (changed, removed.len())
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
        let (changed, removed) = tokio::task::spawn_blocking(move || me.scan_sync(&db2, &root, full)).await.unwrap_or((0, 0));
        let msg = format!("Scan complete: {changed} updated, {removed} removed");
        if changed + removed > 0 {
            db.log("info", &msg);
            self.forget_cover(None);
        }
        self.warm_covers(db.clone());
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

    /// Make the grid/list cover thumbnails for every album in the background (after scans and imports).
    pub fn warm_covers(&self, db: Db) {
        self.covers.warm(db);
    }
}

pub(crate) fn find_cover_uncached(db: &Db, album_id: &str) -> Option<PathBuf> {
    let tracks = db.query("SELECT path FROM tracks WHERE album_id=? ORDER BY disc, track LIMIT 50", p![album_id]).ok()?;
    let mut dirs: Vec<PathBuf> = vec![];
    for t in &tracks {
        let d = Path::new(&t.s("path")).parent()?.to_path_buf();
        if dirs.contains(&d) {
            continue;
        }
        // a folder holding several albums (e.g. "Singles") can't use one cover.jpg for all of them
        let ds = d.to_string_lossy().trim_end_matches('/').to_string();
        let shared = db.one("SELECT COUNT(DISTINCT album_id) n FROM tracks WHERE path LIKE ? AND path NOT LIKE ?", p![format!("{ds}/%"), format!("{ds}/%/%")])
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
    let data = embedded_art(&first)?;
    std::fs::create_dir_all(cached.parent()?).ok()?;
    std::fs::write(&cached, data).ok()?;
    Some(cached)
}

pub fn embedded_art(path: &str) -> Option<Vec<u8>> {
    let tagged = lofty::read_from_path(path).ok()?;
    tagged.tags().iter().find_map(|t| t.pictures().first().map(|p| p.data().to_vec()))
}

pub fn stats(db: &Db) -> Value {
    let mut s = db.one("SELECT COUNT(*) tracks, COUNT(DISTINCT album_id) albums, COUNT(DISTINCT albumartist) artists, COALESCE(SUM(size),0) size, COALESCE(SUM(duration),0) duration FROM tracks", p![])
        .ok().flatten().unwrap_or_default();
    s.insert("untagged_albums".into(), json!(db.one("SELECT COUNT(DISTINCT album_id) n FROM tracks WHERE mb_albumid IS NULL OR mb_albumid=''", p![]).ok().flatten().map(|r| r.i("n")).unwrap_or(0)));
    let counts: serde_json::Map<String, Value> = db.query("SELECT verify_status, COUNT(*) n FROM tracks GROUP BY verify_status", p![]).unwrap_or_default()
        .into_iter().map(|r| { let k = r.s("verify_status"); (if k.is_empty() { "unchecked".into() } else { k }, json!(r.i("n"))) }).collect();
    s.insert("verify_counts".into(), Value::Object(counts));
    s.insert("formats".into(), json!(db.query("SELECT format, COUNT(*) n FROM tracks GROUP BY format ORDER BY n DESC", p![]).unwrap_or_default()));
    Value::Object(s)
}

/// Delete files (and their lyrics) and their rows; playlist entries move to `replacement` if given.
pub fn delete_tracks(db: &Db, ids: &[i64], replacement: Option<i64>) -> usize {
    let mut n = 0;
    for id in ids {
        let Ok(Some(t)) = db.one("SELECT path FROM tracks WHERE id=?", p![id]) else { continue };
        let path = t.s("path");
        let _ = std::fs::remove_file(&path);
        for side in ["lrc", "txt"] {
            let _ = std::fs::remove_file(Path::new(&path).with_extension(side));
        }
        let _ = db.execute("UPDATE playlist_items SET track_id=? WHERE track_id=?", p![replacement, id]);
        let _ = db.execute("DELETE FROM tracks WHERE id=?", p![id]);
        crate::dsp::render::forget_track(&db.data_dir, *id);
        n += 1;
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
