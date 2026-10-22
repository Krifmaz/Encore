//! The Surround library (0.35): Dolby Atmos and other multichannel versions of songs, in their own folder
//! (`surround_path`, `<library_path>/Surround` when empty) and their own table (`surround_tracks`, schema 30).
//! Nothing that reads `tracks` sees them: the stereo lists, search, shuffle, playlists, likes, stats, duplicates,
//! the quality and loudness tasks and the sync stay stereo without a filter to forget. docs/contracts/surround.md.
//!
//! - Ids: surround songs are numbered from [`BASE_ID`], so `/api/library/tracks/{id}/…` tells them apart by number;
//!   surround albums are `s` + 15 hex characters ([`album_key`]), stereo ones are hex only.
//! - Links: a surround album belongs to the stereo album of the same artist and name (the "(Dolby Atmos)" kind of
//!   suffix left out), a surround song to the stereo song with its ISRC, else its disc and track, else its title.
//! - The scan of the stereo library leaves the surround folder out (crate::scan); this one indexes it.
use crate::scan::{album_key as stereo_album_key, mtime_size, Scanned, COLS};
use encore_db::{now, p, Conn, Db, RowExt};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The first surround song id (schema 30 seeds the table's sequence below it).
pub const BASE_ID: i64 = 1_000_000_000;

/// A surround song's id.
pub fn is_track(id: i64) -> bool {
    id >= BASE_ID
}

/// A surround album's id (`s` + hex).
pub fn is_album(id: &str) -> bool {
    id.starts_with('s')
}

/// The Surround folder: `surround_path`, or `Surround` in `library_path` when it is empty. None without a library.
pub fn root(cfg: &encore_config::Config) -> Option<PathBuf> {
    let own = cfg.str("surround_path");
    let own = own.trim().trim_end_matches('/');
    if !own.is_empty() {
        return Some(PathBuf::from(own));
    }
    let lib = cfg.str("library_path");
    let lib = lib.trim().trim_end_matches('/');
    (!lib.is_empty()).then(|| Path::new(lib).join("Surround"))
}

/// Whether a path is in the Surround folder.
pub fn in_root(cfg: &encore_config::Config, path: &Path) -> bool {
    root(cfg).is_some_and(|r| path.starts_with(&r))
}

/// A surround album's id: `s` + 15 hex characters of sha1("surround|albumartist|album") (of its folder without
/// an album name), never equal to a stereo album's id.
pub fn album_key(albumartist: &str, album: &str, path: &str) -> String {
    let key = if !album.is_empty() {
        format!("surround|{}|{}", albumartist.trim().to_lowercase(), album.trim().to_lowercase())
    } else {
        format!("surround|dir|{}", Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
    };
    format!("s{}", &hex::encode(Sha1::digest(key.as_bytes()))[..15])
}

/// An album's name without what the services add to an Atmos release: "Album (Dolby Atmos)" → "Album".
pub fn stereo_name(album: &str) -> String {
    let mut a = album.trim().to_string();
    loop {
        let lower = a.to_lowercase();
        let cut = ["(dolby atmos)", "[dolby atmos]", "(atmos)", "[atmos]", "(spatial audio)", "[spatial audio]", "- dolby atmos", "(dolby atmos mix)",
            "(5.1)", "[5.1]", "(5.1 surround)", "(surround)", "[surround]"]
            .iter()
            .find(|s| lower.ends_with(*s))
            .map(|s| a.len() - s.len());
        match cut {
            Some(at) => a = a[..at].trim_end().to_string(),
            None => return a,
        }
    }
}

/// The columns a surround row has beside [`COLS`].
const OWN: [&str; 5] = ["codec", "profile", "channels", "layout", "atmos"];

/// What one file's row will hold.
struct FileRow {
    path: String,
    album_id: String,
    mtime: f64,
    size: i64,
    vals: Vec<Value>,
    own: Vec<Value>,
    albumartist: String,
    album: String,
    title: String,
    track: i64,
    disc: i64,
    isrc: String,
}

fn read_file(path: &str) -> std::io::Result<FileRow> {
    let meta = std::fs::metadata(path)?;
    let (mtime, size) = mtime_size(&meta);
    let (mut t, facts) = encore_media::tags::read_tags_and_facts(path);
    let stream = encore_media::probe::stream_info(path).unwrap_or_default();
    t.insert("sample_rate".into(), json!(facts.sample_rate.or((stream.sample_rate > 0).then_some(stream.sample_rate))));
    t.insert("bit_depth".into(), json!(facts.bit_depth.map(u32::from).or((stream.bit_depth > 0).then_some(stream.bit_depth))));
    // lofty knows no E-AC-3: the length, bitrate and format come from ffprobe then
    if t.f("duration") <= 0.0 && stream.duration > 0.0 {
        t.insert("duration".into(), json!(stream.duration));
    }
    if t.i("bitrate") <= 0 && stream.bitrate > 0 {
        t.insert("bitrate".into(), json!(stream.bitrate / 1000));
    }
    if !stream.codec.is_empty() {
        t.insert("format".into(), json!(stream.codec));
    }
    let album_id = album_key(&t.s("albumartist"), &t.s("album"), path);
    let vals: Vec<Value> = COLS.iter().map(|c| t.get(*c).cloned().unwrap_or(Value::Null)).collect();
    let own = vec![json!(stream.codec), json!(stream.profile), json!(stream.channels), json!(stream.layout), json!(stream.atmos() as i64)];
    Ok(FileRow {
        path: path.to_string(), album_id, mtime, size, vals, own, albumartist: t.s("albumartist"), album: t.s("album"), title: t.s("title"),
        track: t.i("track"), disc: t.i("disc"), isrc: t.s("isrc"),
    })
}

/// The stereo album and song a surround file belongs to (None, None when the stereo library has neither).
fn link(c: &Conn, f: &FileRow) -> rusqlite::Result<(Option<String>, Option<i64>)> {
    let album = crate::merge::resolve(c, &stereo_album_key(&f.albumartist, &stereo_name(&f.album), &f.path))?;
    let album = c.one("SELECT 1 FROM tracks WHERE album_id=? LIMIT 1", p![album])?.map(|_| album);
    if !f.isrc.trim().is_empty() {
        if let Some(r) = c.one("SELECT id FROM tracks WHERE isrc=? COLLATE NOCASE ORDER BY album_id=? DESC LIMIT 1", p![f.isrc.trim(), album.clone().unwrap_or_default()])? {
            return Ok((album, Some(r.i("id"))));
        }
    }
    let Some(a) = album.clone() else { return Ok((None, None)) };
    if f.track > 0 {
        if let Some(r) = c.one("SELECT id FROM tracks WHERE album_id=? AND track=? AND COALESCE(disc,1)=? LIMIT 1", p![a, f.track, f.disc.max(1)])? {
            return Ok((album, Some(r.i("id"))));
        }
    }
    let want = crate::matcher::fold(&f.title);
    let rows = c.query("SELECT id, title FROM tracks WHERE album_id=?", p![a])?;
    Ok((album, rows.iter().find(|r| !want.is_empty() && crate::matcher::fold(&r.s("title")) == want).map(|r| r.i("id"))))
}

/// Writes one file's row (new or changed). → its album id
fn store(c: &Conn, f: &FileRow) -> rusqlite::Result<String> {
    let (stereo_album, stereo_track) = link(c, f)?;
    let cols: Vec<&str> = COLS.iter().chain(OWN.iter()).copied().collect();
    let mut vals: Vec<Value> = f.vals.clone();
    vals.extend(f.own.iter().cloned());
    vals.extend([json!(stereo_track), json!(stereo_album)]);
    if let Some(ex) = c.one("SELECT id FROM surround_tracks WHERE path=?", p![f.path])? {
        let sets = cols.iter().map(|c| format!("{c}=?")).collect::<Vec<_>>().join(", ");
        let mut params = vec![json!(f.album_id), json!(f.mtime), json!(f.size)];
        params.extend(vals);
        params.push(json!(ex.i("id")));
        c.execute(&format!("UPDATE surround_tracks SET album_id=?, mtime=?, size=?, {sets}, stereo_track_id=?, stereo_album_id=? WHERE id=?"), params)?;
    } else {
        let mut params = vec![json!(f.path), json!(f.album_id), json!(f.mtime), json!(f.size)];
        params.extend(vals);
        params.push(json!(now()));
        // found in the Surround folder; a request's import is stamped by its worker afterwards
        c.execute(
            &format!("INSERT INTO surround_tracks (path, album_id, mtime, size, {}, stereo_track_id, stereo_album_id, added_at, added_via) VALUES (?,?,?,?,{},?,?,?,'scan')",
                cols.join(", "), vec!["?"; cols.len()].join(",")),
            params,
        )?;
    }
    Ok(f.album_id.clone())
}

/// Reads and writes these files into the Surround library (an import). → the albums they are in, and the files
/// that could not be read or written with why
pub fn index_files(db: &Db, paths: &[String]) -> (Vec<String>, Vec<(String, String)>) {
    let (mut albums, mut failed) = (Vec::<String>::new(), vec![]);
    for path in paths {
        let row = match read_file(path) {
            Ok(r) => r,
            Err(e) => {
                failed.push((path.clone(), e.to_string()));
                continue;
            }
        };
        match db.write_now(|c| store(c, &row)) {
            Ok(a) => {
                if !albums.contains(&a) {
                    albums.push(a);
                }
            }
            Err(e) => failed.push((path.clone(), e.to_string())),
        }
    }
    (albums, failed)
}

/// Walks the Surround folder: new and changed files are read, vanished ones removed. A folder that isn't there
/// keeps its rows (a disk that is away), as the stereo scan does.
pub fn scan(db: &Db, root: &Path, full: bool) -> Scanned {
    let Ok(rd) = std::fs::metadata(root) else { return Scanned::default() };
    if !rd.is_dir() {
        return Scanned::default();
    }
    let known: HashMap<String, f64> = db.query("SELECT path, mtime FROM surround_tracks", p![]).unwrap_or_default()
        .into_iter().map(|r| (r.s("path"), r.f("mtime"))).collect();
    let files: Vec<PathBuf> = walkdir::WalkDir::new(root).follow_links(true).into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|x| is_audio(&x.to_string_lossy())))
        .collect();
    let mut seen = HashSet::new();
    let mut paths = vec![];
    for f in &files {
        let path = f.to_string_lossy().into_owned();
        seen.insert(path.clone());
        let Ok(meta) = std::fs::metadata(f) else { continue };
        let (mtime, _) = mtime_size(&meta);
        if full || !known.get(&path).is_some_and(|k| (k - mtime).abs() < 0.001) {
            paths.push(path);
        }
    }
    let (albums, failed) = index_files(db, &paths);
    for (path, e) in &failed {
        db.log("warning", &format!("Could not read {path} (Surround): {e}"));
    }
    // a folder with no songs while the table has some: a disk that isn't there, nothing removed
    let removed: Vec<&String> = if files.is_empty() { vec![] } else { known.keys().filter(|p| !seen.contains(*p)).collect() };
    let mut gone_albums = vec![];
    for chunk in removed.chunks(500) {
        let marks = crate::artists::marks(chunk.len());
        let params: Vec<Value> = chunk.iter().map(|p| json!(p)).collect();
        let r = db.write_now(|c| c.transaction(|c| {
            let albums = c.query(&format!("SELECT DISTINCT album_id FROM surround_tracks WHERE path IN ({marks})"), params.clone())?;
            c.execute(&format!("DELETE FROM surround_tracks WHERE path IN ({marks})"), params.clone())?;
            Ok::<_, rusqlite::Error>(albums)
        }));
        if let Ok(rows) = r {
            gone_albums.extend(rows.iter().map(|r| r.s("album_id")));
        }
    }
    let mut all: std::collections::BTreeSet<String> = albums.into_iter().collect();
    all.extend(gone_albums);
    Scanned { changed: paths.len() - failed.len(), removed: removed.len(), albums: all, looked_empty: files.is_empty() && !known.is_empty() }
}

/// An extension the Surround library takes: the library's audio files and the surround-only ones (.mka, .ac3 …).
pub fn is_audio(ext: &str) -> bool {
    let e = ext.to_lowercase();
    encore_core::audio::AUDIO_EXTS.contains(&e.as_str()) || encore_media::probe::SURROUND_EXTS.contains(&e.as_str())
}

/// The stereo library dropped songs (deleted or replaced): their surround versions point at `replacement` or at
/// nothing.
pub fn stereo_gone(c: &Conn, ids: &[i64], replacement: Option<i64>) -> rusqlite::Result<()> {
    for chunk in ids.chunks(500) {
        let marks = crate::artists::marks(chunk.len());
        let mut params = vec![json!(replacement)];
        params.extend(chunk.iter().map(|i| json!(i)));
        c.execute(&format!("UPDATE surround_tracks SET stereo_track_id=? WHERE stereo_track_id IN ({marks})"), params)?;
    }
    Ok(())
}

// ---- imports ---------------------------------------------------------------------------------------------

/// What an import into the Surround library knows besides the files: the request's names (for files without
/// tags) and the stereo album and songs it was asked for.
#[derive(Default, Clone)]
pub struct Import {
    pub artist: String,
    pub album: String,
    pub stereo_album_id: Option<String>,
    pub stereo_track_ids: Vec<i64>,
}

/// Which of these files are surround (more than two channels, or Atmos): (surround, the others).
pub async fn split(files: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let files = files.to_vec();
    tokio::task::spawn_blocking(move || files.into_iter().partition(|f| encore_media::probe::surround_of(&f.to_string_lossy()).is_some()))
        .await
        .unwrap_or_default()
}

/// Where a surround file goes: `<root>/<album artist>/<album>/<disc-><track> - <title>.<ext>`.
fn destination(root: &Path, src: &Path, want: &Import) -> PathBuf {
    let (t, _) = encore_media::tags::read_tags_and_facts(&src.to_string_lossy());
    let pick = |a: String, b: &str| if a.trim().is_empty() { b.to_string() } else { a };
    let albumartist = pick(t.s("albumartist"), &pick(t.s("artist"), &want.artist));
    let album = pick(t.s("album"), &want.album);
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title = pick(t.s("title"), &stem);
    let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "m4a".into());
    let (track, disc) = (t.i("track"), t.i("disc"));
    let name = match (track > 0, disc > 1) {
        (true, true) => format!("{disc}-{track:02} - {title}"),
        (true, false) => format!("{track:02} - {title}"),
        _ => title,
    };
    let part = |s: &str| {
        let s = encore_core::fs::safe_file_name(s);
        let s = s.trim_start_matches('.').chars().take(120).collect::<String>();
        if s.is_empty() { "Unknown".to_string() } else { s }
    };
    root.join(part(&albumartist)).join(part(&album)).join(format!("{}.{ext}", part(&name)))
}

/// Files the Surround library: each surround file moves into the Surround folder (a song fetched again
/// replaces its earlier copy, with its .lrc), is indexed and linked to the stereo album and songs. Files that
/// are not surround stay where they are. → the new paths, and how many files were not surround
pub async fn import(st: &impl crate::Ctx, files: &[PathBuf], want: &Import) -> anyhow::Result<(Vec<PathBuf>, usize)> {
    let root = root(st.cfg()).ok_or_else(|| anyhow::anyhow!("No Surround folder: set the music library path or the Surround folder"))?;
    let (surround, others) = split(files).await;
    let mut new = vec![];
    for src in &surround {
        let src2 = src.clone();
        let (root2, want2) = (root.clone(), want.clone());
        let dest = tokio::task::spawn_blocking(move || destination(&root2, &src2, &want2)).await?;
        if dest.exists() && dest != *src {
            let _ = std::fs::remove_file(&dest);
            let _ = std::fs::remove_file(dest.with_extension("lrc"));
        }
        let placed = crate::tagger::move_file(st, src, &dest, false).await?;
        let lrc = src.with_extension("lrc");
        if lrc.is_file() {
            let _ = crate::tagger::move_file(st, &lrc, &placed.with_extension("lrc"), false).await;
        }
        new.push(placed);
    }
    let paths: Vec<String> = new.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let db = st.db().clone();
    let paths2 = paths.clone();
    let (_, failed) = tokio::task::spawn_blocking(move || index_files(&db, &paths2)).await?;
    if let Some((p, e)) = failed.first() {
        anyhow::bail!("Could not index {p} in the Surround library: {e}");
    }
    // what the request knows wins over the names' guess
    if let Some(a) = want.stereo_album_id.as_deref().filter(|a| !a.is_empty()) {
        for p in &paths {
            st.db().execute("UPDATE surround_tracks SET stereo_album_id=? WHERE path=? AND (stereo_album_id IS NULL OR stereo_album_id='')", p![a, p])?;
        }
    }
    if let ([only], [one]) = (want.stereo_track_ids.as_slice(), paths.as_slice()) {
        st.db().execute("UPDATE surround_tracks SET stereo_track_id=? WHERE path=? AND stereo_track_id IS NULL", p![only, one])?;
    }
    Ok((new, others.len()))
}

// ---- answers ---------------------------------------------------------------------------------------------

/// A surround song as the album page and the player read it: the stereo track's fields plus what the file is.
pub fn track_json(r: &encore_db::Row) -> Value {
    let stream = encore_media::probe::Stream {
        codec: r.s("codec"), profile: r.s("profile"), channels: r.i("channels") as u32, layout: r.s("layout"), ..Default::default()
    };
    json!({
        "id": r.i("id"), "title": r.s("title"), "artist": r.s("artist"), "album": r.s("album"), "albumartist": r.s("albumartist"),
        "album_id": r.s("album_id"), "track": r.i("track"), "disc": r.i("disc"), "year": r.s("year"), "duration": r.f("duration"),
        "bitrate": r.i("bitrate"), "format": r.s("codec"), "codec": r.s("codec"), "channels": r.i("channels"), "layout": r.s("layout"),
        "atmos": r.i("atmos") == 1, "quality": stream.label(), "sample_rate": r.i("sample_rate"), "bit_depth": r.i("bit_depth"),
        "isrc": r.s("isrc"), "stereo_track_id": if r.i("stereo_track_id") > 0 { json!(r.i("stereo_track_id")) } else { Value::Null },
        "stereo_album_id": r.s("stereo_album_id"), "class": "surround",
    })
}

/// The Surround albums, newest first or by name: `{id, album, albumartist, year, tracks, duration, atmos, quality, stereo_album_id}`.
pub fn albums(db: &Db, sort: &str, q: &str, limit: i64, offset: i64) -> Vec<Value> {
    let order = match sort {
        "name" => "lower(album) ASC",
        "artist" => "lower(albumartist) ASC, lower(album) ASC",
        "year" => "year DESC, lower(album) ASC",
        _ => "added DESC",
    };
    let like = format!("%{}%", q.trim());
    db.query(
        &format!(
            "SELECT album_id id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, COUNT(*) tracks, SUM(duration) duration,
                    MAX(atmos) atmos, MAX(channels) channels, MAX(codec) codec, MAX(layout) layout, MAX(stereo_album_id) stereo_album_id, MAX(added_at) added
             FROM surround_tracks WHERE (? = '' OR album LIKE ? OR albumartist LIKE ?) GROUP BY album_id ORDER BY {order} LIMIT ? OFFSET ?"
        ),
        p![q.trim(), like, like, limit.clamp(1, 500), offset.max(0)],
    )
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        let stream = encore_media::probe::Stream { codec: r.s("codec"), profile: if r.i("atmos") == 1 { "Atmos".into() } else { String::new() }, channels: r.i("channels") as u32, layout: r.s("layout"), ..Default::default() };
        json!({ "id": r.s("id"), "album": r.s("album"), "albumartist": r.s("albumartist"), "year": r.s("year"), "tracks": r.i("tracks"),
            "duration": r.f("duration"), "atmos": r.i("atmos") == 1, "quality": stream.label(), "stereo_album_id": r.s("stereo_album_id"), "class": "surround" })
    })
    .collect()
}

/// A surround album's songs, in disc and track order.
pub fn album_tracks(db: &Db, album_id: &str) -> Vec<Value> {
    db.query("SELECT * FROM surround_tracks WHERE album_id=? ORDER BY COALESCE(disc,1), track, lower(title)", p![album_id]).unwrap_or_default().iter().map(track_json).collect()
}

/// The surround albums of a stereo album: `[{album_id, tracks, quality}]`.
pub fn for_stereo_album(db: &Db, stereo_album_id: &str) -> Vec<Value> {
    db.query(
        "SELECT album_id, COUNT(*) tracks, MAX(atmos) atmos, MAX(channels) channels, MAX(codec) codec, MAX(layout) layout FROM surround_tracks WHERE stereo_album_id=? GROUP BY album_id",
        p![stereo_album_id],
    )
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        let s = encore_media::probe::Stream { codec: r.s("codec"), profile: if r.i("atmos") == 1 { "Atmos".into() } else { String::new() }, channels: r.i("channels") as u32, layout: r.s("layout"), ..Default::default() };
        json!({ "album_id": r.s("album_id"), "tracks": r.i("tracks"), "quality": s.label() })
    })
    .collect()
}

/// The surround version of each stereo song that has one: stereo id → surround id.
pub fn for_stereo_tracks(db: &Db, ids: &[i64]) -> HashMap<i64, i64> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let marks = crate::artists::marks(chunk.len());
        for r in db.query(&format!("SELECT stereo_track_id, MIN(id) id FROM surround_tracks WHERE stereo_track_id IN ({marks}) GROUP BY stereo_track_id"), chunk.iter().map(|i| json!(i)).collect()).unwrap_or_default() {
            out.insert(r.i("stereo_track_id"), r.i("id"));
        }
    }
    out
}

/// One surround song's row.
pub fn track(db: &Db, id: i64) -> Option<encore_db::Row> {
    db.one("SELECT * FROM surround_tracks WHERE id=?", p![id]).ok().flatten()
}

/// The totals of the Surround library.
pub fn stats(db: &Db) -> Value {
    db.one("SELECT COUNT(*) tracks, COUNT(DISTINCT album_id) albums, SUM(atmos) atmos, COALESCE(SUM(size),0) size, COALESCE(SUM(duration),0) duration FROM surround_tracks", p![])
        .ok().flatten().map(Value::Object).unwrap_or_else(|| json!({}))
}

/// Ignore files in the library folder, so a stereo Plex or Navidrome library over it leaves the Surround folder
/// out (`.plexignore` and `.ndignore` lines naming it; a line Encore wrote is not written twice). Only when the
/// Surround folder is inside `library_path` and `surround_hide_from_stereo` is on.
pub fn write_ignore_files(cfg: &encore_config::Config) {
    if !cfg.bool("surround_hide_from_stereo") {
        return;
    }
    let (Some(root), lib) = (root(cfg), cfg.str("library_path")) else { return };
    let lib = Path::new(lib.trim());
    let Ok(rel) = root.strip_prefix(lib) else { return };
    let rel = rel.to_string_lossy().trim_matches('/').to_string();
    if rel.is_empty() {
        return;
    }
    for (file, line) in [(".plexignore", format!("{rel}/*")), (".ndignore", rel.clone())] {
        let path = lib.join(file);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.lines().any(|l| l.trim() == line) {
            continue;
        }
        let mut new = text.clone();
        if !new.is_empty() && !new.ends_with('\n') {
            new.push('\n');
        }
        new.push_str(&format!("# Encore's Surround library (Settings → Library → Surround)\n{line}\n"));
        if let Err(e) = std::fs::write(&path, new) {
            tracing::warn!("surround: could not write {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_names() {
        assert!(is_track(BASE_ID) && !is_track(BASE_ID - 1));
        let a = album_key("Radiohead", "In Rainbows", "/x/a.m4a");
        assert!(is_album(&a) && a.len() == 16 && !is_album(&stereo_album_key("Radiohead", "In Rainbows", "/x/a.m4a")));
        assert_ne!(&a[1..], &stereo_album_key("Radiohead", "In Rainbows", "/x/a.m4a")[..15]);
        for (n, want) in [("In Rainbows (Dolby Atmos)", "In Rainbows"), ("Abbey Road [Atmos]", "Abbey Road"), ("X (Deluxe) (Spatial Audio)", "X (Deluxe)"),
            ("Thriller - Dolby Atmos", "Thriller"), ("Plain", "Plain"), ("Live (5.1)", "Live")] {
            assert_eq!(stereo_name(n), want);
        }
    }

    #[test]
    fn ignore_files_are_written_once() {
        let dir = encore_core::tmp::dir("surround-ignore");
        let cfg = encore_config::Config::load_with(encore_db::Db::open(&dir).unwrap(), |_| None).unwrap();
        let lib = dir.join("music");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(".plexignore"), "Old/*").unwrap();
        cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "surround_path": "", "surround_hide_from_stereo": true }));
        assert_eq!(root(&cfg), Some(lib.join("Surround")));
        write_ignore_files(&cfg);
        write_ignore_files(&cfg);
        let plex = std::fs::read_to_string(lib.join(".plexignore")).unwrap();
        assert!(plex.starts_with("Old/*\n") && plex.matches("Surround/*").count() == 1, "{plex}");
        assert!(std::fs::read_to_string(lib.join(".ndignore")).unwrap().lines().any(|l| l == "Surround"));
        // a Surround folder outside the library needs no ignore file
        cfg.set_many(&json!({ "surround_path": dir.join("surround").to_string_lossy() }));
        std::fs::remove_file(lib.join(".ndignore")).unwrap();
        write_ignore_files(&cfg);
        assert!(!lib.join(".ndignore").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A short tone with tags, made with ffmpeg: `channels` 2 as FLAC, more as E-AC-3 in .m4a (Apple's Atmos files).
    fn tone(path: &Path, channels: u32, title: &str, album: &str, track: u32) -> bool {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut c = std::process::Command::new("ffmpeg");
        c.args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-ac", &channels.to_string()]);
        if channels > 2 {
            c.args(["-c:a", "eac3", "-b:a", "384k", "-f", "mp4"]);
        }
        for (k, v) in [("title", title), ("artist", "Tone"), ("album_artist", "Tone"), ("album", album), ("track", &track.to_string())] {
            c.args(["-metadata", &format!("{k}={v}")]);
        }
        c.arg(path).status().is_ok_and(|s| s.success())
    }

    #[tokio::test]
    async fn the_surround_folder_is_its_own_library() {
        if encore_core::which::which("ffmpeg").is_none() {
            return;
        }
        let dir = encore_core::tmp::dir("surround-scan");
        let db = Db::open(&dir).unwrap();
        let cfg = encore_config::Config::load_with(db.clone(), |_| None).unwrap();
        let music = dir.join("music");
        cfg.set_many(&json!({ "library_path": music.to_string_lossy(), "surround_path": "" }));
        assert!(tone(&music.join("Tone/Waves/01 Sine.flac"), 2, "Sine", "Waves", 1));
        assert!(tone(&music.join("Surround/Tone/Waves (Dolby Atmos)/01 Sine.m4a"), 6, "Sine", "Waves (Dolby Atmos)", 1));
        let lib = crate::Library::default();
        lib.scan_library(db.clone(), &cfg, false).await;
        // stereo: only the FLAC
        let stereo = db.query("SELECT id, path, album_id FROM tracks", p![]).unwrap();
        assert_eq!(stereo.len(), 1, "{stereo:?}");
        assert!(stereo[0].s("path").ends_with("Sine.flac"));
        // surround: the E-AC-3, linked to the stereo song and album
        let sur = db.query("SELECT * FROM surround_tracks", p![]).unwrap();
        assert_eq!(sur.len(), 1);
        let r = &sur[0];
        assert!(is_track(r.i("id")) && is_album(&r.s("album_id")));
        assert_eq!((r.s("codec").as_str(), r.i("channels"), r.s("format").as_str()), ("eac3", 6, "eac3"));
        assert_eq!((r.i("stereo_track_id"), r.s("stereo_album_id")), (stereo[0].i("id"), stereo[0].s("album_id")));
        assert!(r.f("duration") > 1.5, "the length from ffprobe: {}", r.f("duration"));
        assert_eq!(for_stereo_album(&db, &stereo[0].s("album_id"))[0]["quality"], "E-AC-3 5.1");
        assert_eq!(for_stereo_tracks(&db, &[stereo[0].i("id")]).get(&stereo[0].i("id")), Some(&r.i("id")));
        assert_eq!(albums(&db, "", "", 50, 0).len(), 1);
        // a stereo song deleted: the surround one stays, its link goes
        db.write_now(|c| stereo_gone(c, &[stereo[0].i("id")], None)).unwrap();
        assert_eq!(track(&db, r.i("id")).unwrap().i("stereo_track_id"), 0);
        // a file removed from the folder leaves the Surround library at the next scan; the stereo song stays
        std::fs::remove_file(music.join("Surround/Tone/Waves (Dolby Atmos)/01 Sine.m4a")).unwrap();
        assert!(tone(&music.join("Surround/Tone/Other/01 Other.m4a"), 6, "Other", "Other", 1));
        lib.scan_library(db.clone(), &cfg, false).await;
        assert_eq!(db.query("SELECT title FROM surround_tracks", p![]).unwrap().iter().map(|r| r.s("title")).collect::<Vec<_>>(), ["Other"]);
        assert_eq!(db.query("SELECT id FROM tracks", p![]).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
