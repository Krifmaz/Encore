//! Import from a folder (§4.14–4.16 of the client shapes), on the staging import's own pieces: every album
//! folder under the chosen folder (a folder with audio files) is matched as an import matches a download
//! (`tagger::auto_match`, the match review's search for other releases), and applied as the match review
//! applies a release (`tagger::preview`, the release's tags written into each file), moved into the library
//! where the naming template puts it (`tagger::place_new`: a taken name gets " (2)", nothing is replaced),
//! read in, and marked as confirmed when a person picked the release. Nothing is deleted for good: what the
//! import leaves behind in an emptied folder, and the copies it makes redundant in the library
//! (`tagger::superseded`), go to Trash. A song that was in the library already keeps its row (its id, likes,
//! plays and shares) when the import moves it.
//!
//! The album folder is listed through its handle (no links, no hidden files) and looked at again right
//! before its files are tagged and moved, under the file manager's lock: a folder or file swapped meanwhile
//! stops the import of that album.
//!
//! "Keep the files where they are" (`in_place`) moves, renames and retags nothing: the folder is added to
//! the extra library folders (`library_extra_roots`, see [`crate::roots::with_extra`]) and its songs are
//! read in where they are.
use crate::dirfd::{Dir, Kind};
use crate::names;
use crate::resolve::{io_error, Target};
use crate::roots::{self, LibraryDirs};
use crate::rows::Change;
use crate::Who;
use encore_core::error::{err, ApiError};
use encore_db::{now, p, Row, RowExt};
use encore_library::{tagger, Ctx};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Album folders matched in one preview; the rest are listed with no match (each match asks MusicBrainz a
/// few times, at one call a second).
pub const MATCH_MAX: usize = 25;
/// Album folders one preview lists.
pub const ALBUMS_MAX: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Move,
    InPlace,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode, ApiError> {
        match s {
            "" | "move" => Ok(Mode::Move),
            "in_place" => Ok(Mode::InPlace),
            _ => Err(err(400, "mode is move or in_place")),
        }
    }
}

/// One album folder: where it is, its audio files (resolved paths, in name order), and what the folder and
/// its files were when they were listed.
pub struct Folder {
    pub at: Target,
    pub files: Vec<PathBuf>,
    seen: Seen,
}

/// A folder as it was listed: its device and inode, and each audio file's name, device, inode, size and time.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    dir: (u64, u64),
    files: Vec<(String, u64, u64, u64, i64)>,
}

/// The audio files right in the folder `at` names, read through its handle (opened without following a
/// link; links and hidden files are left out), in name order.
fn list(at: &Target) -> Result<(Dir, Seen), ApiError> {
    let dir = at.open_dir()?;
    let me = dir.stat().map_err(io_error)?;
    let mut files = vec![];
    for n in dir.names().map_err(io_error)? {
        if n.starts_with('.') || !names::is_audio(&n) {
            continue;
        }
        if let Ok(Some(s)) = dir.lstat(&n) {
            if s.kind == Kind::File {
                files.push((n, s.dev, s.ino, s.size, s.mtime));
            }
        }
    }
    files.sort();
    Ok((dir, Seen { dir: (me.dev, me.ino), files }))
}

fn listed(at: Target) -> Result<Folder, ApiError> {
    let (dir, seen) = list(&at)?;
    let files = seen.files.iter().map(|(n, ..)| dir.path.join(n)).collect();
    Ok(Folder { at, files, seen })
}

/// The album folder again, right before its files are changed: the same folder holding the same files,
/// none of them swapped (for a link or another file) since it was listed. Else the import of it stops.
fn again(f: &Folder) -> Result<Dir, ApiError> {
    let (dir, seen) = list(&f.at)?;
    if seen != f.seen {
        return Err(err(409, "The folder changed while it was being imported: look at it again and import it again"));
    }
    Ok(dir)
}

/// The album folders under `t` (`t` itself when it holds audio files), checked for links on the way.
pub fn folders(t: &Target) -> Result<Vec<Folder>, ApiError> {
    t.open_dir()?;
    let base = t.real()?;
    let mut out = vec![];
    for e in walkdir::WalkDir::new(&base).follow_links(false).max_depth(12).sort_by_file_name().into_iter().flatten() {
        if !e.file_type().is_dir() || (e.depth() > 0 && e.file_name().to_string_lossy().starts_with('.')) {
            continue;
        }
        let rel: Vec<String> = e.path().strip_prefix(&base).unwrap_or(Path::new("")).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let mut at = t.clone();
        at.parts.extend(rel);
        // a folder that can't be opened as one (swapped for a link meanwhile, say) is passed by
        match listed(at) {
            Ok(f) if !f.files.is_empty() => out.push(f),
            _ => continue,
        }
        if out.len() >= ALBUMS_MAX {
            break;
        }
    }
    Ok(out)
}

/// One album folder named by the client, which must lie in the folder being imported.
pub fn folder(t: &Target, rel: &str) -> Result<Folder, ApiError> {
    let at = Target::new(&t.root, rel)?;
    if !at.parts.starts_with(&t.parts) {
        return Err(err(400, "That folder isn't in the folder being imported"));
    }
    let f = listed(at)?;
    if f.files.is_empty() {
        return Err(err(404, "No songs in this folder"));
    }
    Ok(f)
}

/// The album's artist and title as its files' tags say most often (the folder's name for the title when
/// they don't say).
fn guess(files: &[PathBuf], folder_name: &str) -> (String, String, Vec<Row>) {
    let tags: Vec<Row> = files.iter().map(|f| encore_media::tags::read_tags(&f.to_string_lossy())).collect();
    let most = |key: &str| {
        let mut n: HashMap<String, usize> = HashMap::new();
        for t in &tags {
            let v = t.s(key);
            if !v.trim().is_empty() && v != "Unknown Artist" {
                *n.entry(v).or_default() += 1;
            }
        }
        n.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|x| x.0)
    };
    let artist = most("albumartist").or_else(|| most("artist")).unwrap_or_default();
    let album = most("album").unwrap_or_else(|| folder_name.to_string());
    (artist, album, tags)
}

fn rel_to(path: &str, base: &Path) -> String {
    let b = format!("{}/", base.to_string_lossy().trim_end_matches('/'));
    path.strip_prefix(&b).unwrap_or(path).to_string()
}

/// The release of a preview as the clients read it: `{id, title, artist, date, provider, track_count, score,
/// match}` (score 0…100, match 0…1).
fn release_of(pv: &Value) -> Value {
    let r = &pv["release"];
    let conf = pv["confidence"].as_f64().unwrap_or(0.0);
    json!({ "id": r["id"], "title": r["title"], "artist": r["artist"], "date": r["date"], "provider": r["provider"].as_str().unwrap_or("MusicBrainz"),
            "track_count": pv["tracks"].as_array().map(|t| t.len()).unwrap_or(0), "score": (conf * 100.0).round() as i64, "match": conf })
}

/// Where a file goes by its own tags: where the naming template puts it, or where it is for a song of the
/// library when the rename setting is off.
fn own_place(st: &impl Ctx, dirs: &LibraryDirs, path: &Path, tags: &Row) -> PathBuf {
    if dirs.contains(path) && !st.cfg().bool("rename_on_import") {
        return path.to_path_buf();
    }
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    tagger::render_path(st, tags, &ext, false)
}

/// A move-mode preview's rows go into the library: a file outside it whose new path is still its own (the
/// rename setting is off) is put where the naming template says, and a file the release has no song for
/// goes where its own tags put it.
fn into_library(st: &impl Ctx, dirs: &LibraryDirs, pv: &mut Value) {
    let Some(rows) = pv["rows"].as_array_mut() else { return };
    for row in rows {
        let path = PathBuf::from(row["path"].as_str().unwrap_or(""));
        match row["values"].as_object().cloned() {
            Some(vals) => {
                if row["new_path"] == row["path"] && !dirs.contains(&path) {
                    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                    row["new_path"] = json!(tagger::render_path(st, &vals, &ext, false));
                }
            }
            None => {
                let tags = encore_media::tags::read_tags(&path.to_string_lossy());
                row["new_path"] = json!(own_place(st, dirs, &path, &tags));
            }
        }
    }
}

/// The album as the clients read it (§2.4): its rows' paths relative to the root (old) and the library (new).
fn album_json(st: &impl Ctx, f: &Folder, artist: &str, album: &str, pv: Option<&Value>, own: &[Row], mode: Mode) -> Value {
    let root = f.at.root.canon.clone().unwrap_or_default();
    let lib = PathBuf::from(st.cfg().str("library_path"));
    let rows: Vec<Value> = match (mode, pv) {
        (Mode::InPlace, _) => vec![],
        (Mode::Move, Some(pv)) => pv["rows"].as_array().cloned().unwrap_or_default().iter().map(|r| json!({
            "path": rel_to(r["path"].as_str().unwrap_or(""), &root), "old_title": r["old_title"], "new_title": r["new_title"],
            "new_track": r["new_track"], "new_disc": r["new_disc"], "new_path": rel_to(r["new_path"].as_str().unwrap_or(""), &lib),
        })).collect(),
        // no match: where its own tags put each file
        (Mode::Move, None) => f.files.iter().zip(own).map(|(p, t)| {
            let np = own_place(st, &LibraryDirs::of(st.cfg()), p, t);
            json!({ "path": rel_to(&p.to_string_lossy(), &root), "old_title": t.s("title"), "new_title": t.s("title"), "new_track": t.get("track"),
                    "new_disc": t.get("disc"), "new_path": rel_to(&np.to_string_lossy(), &lib) })
        }).collect(),
    };
    json!({ "folder": f.at.rel(), "artist": artist, "album": album, "files": f.files.len(), "release": pv.map(release_of),
            "confidence": pv.and_then(|p| p["confidence"].as_f64()).unwrap_or(0.0), "rows": rows })
}

/// `POST /api/files/import/preview {root, path, mode, release_id?}` → `{albums}`.
pub async fn preview(st: &impl Ctx, t: &Target, mode: Mode, release_id: Option<&str>) -> Result<Value, ApiError> {
    let found = {
        let t = t.clone();
        tokio::task::spawn_blocking(move || folders(&t)).await.map_err(|e| err(500, e.to_string()))??
    };
    if found.is_empty() {
        return Err(err(404, "No songs in this folder"));
    }
    if release_id.is_some() && found.len() != 1 {
        return Err(err(400, "Pick one album folder to preview as a release"));
    }
    let dirs = LibraryDirs::of(st.cfg());
    let mut out = vec![];
    for (i, f) in found.iter().enumerate() {
        let (artist, album, own) = {
            let (files, name) = (f.files.clone(), f.at.name());
            tokio::task::spawn_blocking(move || guess(&files, &name)).await.map_err(|e| err(500, e.to_string()))?
        };
        let pv = match (mode, release_id) {
            (Mode::InPlace, _) => None,
            (Mode::Move, Some(id)) => Some(tagger::preview(st, &f.files, id, false).await.map_err(|e| err(502, format!("Couldn't use that release: {e}")))?),
            (Mode::Move, None) if i < MATCH_MAX => {
                let a = if artist == "Unknown Artist" { "" } else { artist.as_str() };
                match tagger::auto_match(st, &f.files, a, &album, None).await {
                    Ok(pv) => pv,
                    Err(e) => {
                        tracing::info!("files: no match for {}: {e}", f.at.rel());
                        None
                    }
                }
            }
            _ => None,
        };
        let pv = pv.map(|mut p| {
            into_library(st, &dirs, &mut p);
            p
        });
        out.push(album_json(st, f, &artist, &album, pv.as_ref(), &own, mode));
    }
    Ok(json!({ "albums": out }))
}

/// `POST /api/files/import/match {root, path, artist, album, provider?}` → `{candidates}`: the match
/// review's search, best first.
pub async fn candidates(st: &impl Ctx, f: &Folder, artist: &str, album: &str, provider: &str) -> Result<Value, ApiError> {
    let name = encore_library::meta::PROVIDERS.iter().find(|(k, _)| *k == provider).map(|x| x.1).ok_or_else(|| err(400, "Unknown metadata source"))?;
    let n = f.files.len();
    let a = if artist == "Unknown Artist" { "" } else { artist };
    let mut res = encore_library::meta::search(st, provider, a, album, n, &f.files).await.map_err(|e| err(502, format!("{name} error: {e}")))?;
    for r in res.iter_mut() {
        let tc = r["track_count"].as_i64().filter(|x| *x > 0).unwrap_or(n as i64);
        let score = r["score"].as_f64().unwrap_or(0.0);
        let fit = if tc == n as i64 { 0.5 } else { (0.5 - (tc - n as i64).abs() as f64 * 0.1).max(0.0) };
        r["match"] = json!(((score / 100.0 * 0.5 + fit) * 1000.0).round() / 1000.0);
    }
    res.sort_by(|a, b| b["match"].as_f64().partial_cmp(&a["match"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    Ok(json!({ "candidates": res, "provider": provider }))
}

/// You picked the release yourself: the fingerprint check stops calling it wrong (as the match review does).
fn confirmed(st: &impl Ctx, paths: &[PathBuf], who: &str) {
    for pth in paths {
        let _ = st.db().execute("UPDATE tracks SET verify_status='manual', verify_detail=?, verified_at=? WHERE path=?", p![format!("Confirmed by {who}"), now(), pth.to_string_lossy()]);
    }
}

/// Imports one album folder; → the row paths of its songs in the library.
async fn import_one(st: &impl Ctx, who: &Who, t: &Target, f: &Folder, mode: Mode, release_id: Option<&str>, dirs: &LibraryDirs) -> Result<Vec<PathBuf>, ApiError> {
    // songs new to the library are this admin's import (a song that was the library's keeps who added it)
    let added = encore_library::Added { by: Some(who.id), via: "import" };
    if mode == Mode::InPlace {
        let paths: Vec<PathBuf> = f.files.iter().filter_map(|p| dirs.db_path(p)).collect();
        tagger::index_paths_as(st, &paths, added);
        return Ok(paths);
    }
    // the release's tags (and cover) for each file, or none: each file's tags and where it goes
    let (pv, cover) = match release_id {
        Some(id) => {
            let mut pv = tagger::preview(st, &f.files, id, false).await.map_err(|e| err(502, format!("Couldn't use that release: {e}")))?;
            into_library(st, dirs, &mut pv);
            let cover = tagger::preview_cover(st, &pv, None).await;
            (Some(pv), cover)
        }
        None => (None, None),
    };
    let rows: HashMap<String, Value> = pv.as_ref().and_then(|p| p["rows"].as_array()).into_iter().flatten()
        .map(|r| (r["path"].as_str().unwrap_or("").to_string(), r.clone())).collect();
    let plan: Vec<(String, PathBuf, Option<Row>, PathBuf)> = f.seen.files.iter().zip(&f.files).map(|((name, ..), p)| {
        let row = rows.get(&*p.to_string_lossy());
        match (row.and_then(|r| r["values"].as_object()), row.and_then(|r| r["new_path"].as_str()).filter(|n| !n.is_empty())) {
            (Some(vals), Some(np)) => (name.clone(), p.clone(), Some(vals.clone()), PathBuf::from(np)),
            (_, Some(np)) => (name.clone(), p.clone(), None, PathBuf::from(np)),
            _ => {
                let tags = encore_media::tags::read_tags(&p.to_string_lossy());
                (name.clone(), p.clone(), None, own_place(st, dirs, p, &tags))
            }
        }
    }).collect();
    // what an emptied folder holds goes to Trash, up to the folder that holds the one imported: an album
    // folder imported by itself does not stay behind, and nothing above it is touched (nor the root)
    let keep = t.parts.len().saturating_sub(1);
    let embed = if st.cfg().bool("embed_cover") { cover.as_deref() } else { None };
    let mut change = Change::default();
    // under the file manager's lock and with no wait in between: the folder looked at again, the files
    // tagged through its handle, moved, the rows of songs that were the library's following them
    let row_paths = {
        let _g = crate::lock();
        let dir = again(f)?;
        for (name, src, vals, _) in &plan {
            let Some(vals) = vals else { continue };
            let mut file = dir.open_rw(name).map_err(io_error)?;
            if let Err(e) = encore_media::tags::write_tags_to(&mut file, vals, embed) {
                tagger::tag_failed(st, src, e).map_err(io_error)?;
            }
        }
        let moves: Vec<(PathBuf, PathBuf)> = plan.iter().map(|(_, s, _, d)| (s.clone(), d.clone())).collect();
        let placed = tagger::place_new(&moves).map_err(|e| err(500, format!("The import didn't work: {e:#}")))?;
        tagger::save_cover_file(st, &placed, cover.as_deref());
        let row_paths: Vec<PathBuf> = placed.iter().map(|n| dirs.db_path(n).unwrap_or_else(|| n.clone())).collect();
        // a song that was in the library keeps its row: its id, likes, plays and shares
        let (old, new): (Vec<PathBuf>, Vec<PathBuf>) = f.files.iter().zip(&row_paths).filter_map(|(p, n)| dirs.db_path(p).map(|o| (o, n.clone()))).unzip();
        tagger::relink(st, &old, &new);
        clear_emptied(st, who, &f.at, keep, &mut change);
        row_paths
    };
    if let Err(e) = change.apply(st) {
        tracing::warn!("files: the rows after clearing an imported folder: {e:#}");
    }
    tagger::index_paths_as(st, &row_paths, added);
    if release_id.is_some() {
        confirmed(st, &row_paths, &who.name);
    }
    trash_superseded(st, who, &row_paths).await;
    Ok(row_paths)
}

/// What an import leaves behind in a folder it emptied: covers, info and rip files, playlists, lyrics and
/// hidden files (what `encore_library::prune_empty` clears after a staging import).
fn leftover(d: &Dir, n: &str) -> bool {
    let l = n.to_lowercase();
    d.lstat(n).ok().flatten().is_some_and(|s| s.kind == Kind::File)
        && (l.starts_with('.') || [".jpg", ".jpeg", ".png", ".gif", ".nfo", ".txt", ".lrc", ".log", ".cue", ".m3u", ".m3u8", ".sfv", ".md5"].iter().any(|x| l.ends_with(x)))
}

/// The folders an import emptied, from the album folder up to the folder that holds `keep` parts of the path
/// (that one and everything above it stay): an empty folder is removed; one left with only what an import
/// leaves behind ([`leftover`]) goes to Trash as one item, so nothing is deleted for good; a folder with
/// anything else in it, or that is or holds an extra library folder, stays, and so does every folder above it.
fn clear_emptied(st: &impl Ctx, who: &Who, at: &Target, keep: usize, change: &mut Change) {
    let extra = roots::extra_roots(st.cfg());
    let mut cur = at.clone();
    while cur.parts.len() > keep {
        let Ok((parent, name, s)) = cur.open_existing() else { break };
        if s.kind != Kind::Dir {
            break;
        }
        let real = parent.path.join(&name);
        if extra.iter().any(|e| e.starts_with(&real)) {
            break;
        }
        let Ok(d) = parent.child(&name) else { break };
        let (Ok(names), Ok(count)) = (d.names(), d.count()) else { break };
        // a name that isn't UTF-8 can't be looked at: the folder stays
        if count != names.len() {
            break;
        }
        drop(d);
        if names.is_empty() {
            if parent.rmdir(&name).is_err() {
                break;
            }
        } else if names.iter().all(|n| parent.child(&name).is_ok_and(|d| leftover(&d, n))) {
            if crate::trash::trash_one(st, who, &cur, &parent, &name, change, &[]).is_err() {
                break;
            }
        } else {
            break;
        }
        cur.parts.pop();
    }
}

/// The copies the import makes redundant in the library folder (`tagger::superseded`: loose singles and
/// lower-quality copies of the same songs) go to Trash with their lyrics files, not away for good; their
/// playlist entries and likes go to the new song.
async fn trash_superseded(st: &impl Ctx, who: &Who, new: &[PathBuf]) {
    let gone = tagger::superseded(st, new).await;
    if gone.is_empty() {
        return;
    }
    let Some(lib) = roots::library(st.cfg(), st.data_dir()) else { return };
    let library_path = st.cfg().str("library_path");
    let _g = crate::lock();
    let mut change = Change::default();
    for s in gone {
        let res = (|| -> Result<(), ApiError> {
            let rel = s.old_path.strip_prefix(library_path.trim()).map_err(|_| err(400, "not in the library folder"))?;
            let t = Target::new(&lib, &rel.to_string_lossy())?;
            let (dir, name, _) = t.open_existing()?;
            crate::trash::trash_one(st, who, &t, &dir, &name, &mut change, &[])?;
            let _ = st.db().execute("UPDATE playlist_items SET track_id=? WHERE track_id=?", p![s.new, s.old]);
            for side in ["lrc", "txt"] {
                let side_name = Path::new(&name).with_extension(side).to_string_lossy().into_owned();
                if side_name != name && dir.lstat(&side_name).ok().flatten().is_some_and(|x| x.kind == Kind::File) {
                    let mut st_t = t.clone();
                    *st_t.parts.last_mut().unwrap() = side_name.clone();
                    let _ = crate::trash::trash_one(st, who, &st_t, &dir, &side_name, &mut change, &[]);
                }
            }
            Ok(())
        })();
        let what = if s.loose { "loose single" } else { "lower-quality copy" };
        match res {
            Ok(()) => st.db().log("info", &format!("Moved the {what} of “{} – {}” to Trash – replaced by the new import", s.artist, s.title)),
            Err(e) => tracing::warn!("files: the {what} of {} – {} stays: {}", s.artist, s.title, e.detail),
        }
    }
    if let Err(e) = change.apply(st) {
        tracing::warn!("files: the rows of copies moved to Trash: {e:#}");
    }
}

/// `POST /api/files/import/apply {root, path, mode, albums: [{folder, release_id}]}` →
/// `{imported, albums, message, failed}`.
pub async fn apply(st: &impl Ctx, who: &Who, t: &Target, mode: Mode, albums: &[(String, Option<String>)]) -> Result<Value, ApiError> {
    if albums.is_empty() {
        return Err(err(400, "No albums to import"));
    }
    // keep in place: the folder joins the library's folders first, so its songs are the library's
    if mode == Mode::InPlace {
        t.open_dir()?;
        let real = t.real()?;
        let lib = PathBuf::from(st.cfg().str("library_path"));
        if let Some(extra) = roots::with_extra(&lib, &roots::extra_roots(st.cfg()), &real) {
            // never Encore's own working folders (which it clears), nor a folder holding the library or the config
            if let Some(why) = roots::keep_refusal(st.cfg(), st.data_dir(), &real) {
                return Err(err(409, why));
            }
            if st.cfg().env_locked("library_extra_roots") {
                return Err(err(409, format!("The extra library folders are set by ENCORE_LIBRARY_EXTRA_ROOTS: add {} there first", real.display())));
            }
            st.cfg().set_many(&json!({ "library_extra_roots": roots::format_extra(&extra) }));
            crate::audit_text(st, who, &format!("added {} to the library's folders (kept where it is)", real.display()));
        }
    }
    let dirs = LibraryDirs::of(st.cfg());
    let (mut imported, mut album_ids, mut failed) = (0usize, vec![], vec![]);
    for (rel, release) in albums {
        let res = async {
            let f = {
                let (t, rel) = (t.clone(), rel.clone());
                tokio::task::spawn_blocking(move || folder(&t, &rel)).await.map_err(|e| err(500, e.to_string()))??
            };
            let release = if mode == Mode::InPlace { None } else { release.as_deref().filter(|r| !r.is_empty()) };
            import_one(st, who, t, &f, mode, release, &dirs).await.map(|paths| (f, paths))
        }.await;
        match res {
            Ok((f, paths)) => {
                imported += paths.len();
                let marks = encore_library::artists::marks(paths.len());
                let ids = st.db().query(&format!("SELECT DISTINCT album_id FROM tracks WHERE path IN ({marks})"), paths.iter().map(|p| json!(p.to_string_lossy())).collect()).unwrap_or_default();
                for a in ids.iter().map(|r| r.s("album_id")) {
                    if !a.is_empty() && !album_ids.contains(&a) {
                        album_ids.push(a);
                    }
                }
                crate::audit(st, who, if mode == Mode::InPlace { "imported in place" } else { "imported into the library" }, &f.at, None);
            }
            Err(e) => failed.push(json!({ "folder": rel, "reason": e.detail })),
        }
    }
    let mut message = if imported == 1 { "Imported 1 song".to_string() } else { format!("Imported {imported} songs") };
    if let Some(first) = failed.first() {
        message.push_str(&format!(". {} weren't imported. {}: {}", failed.len(), first["folder"].as_str().unwrap_or(""), first["reason"].as_str().unwrap_or("")));
    }
    Ok(json!({ "imported": imported, "albums": album_ids, "message": message, "failed": failed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roots::{Root, RootKind};

    /// The album folder is looked at again before its files change: another file in it, a file swapped for
    /// another, or the folder swapped for a link stops the import of it.
    #[test]
    fn a_folder_that_changed_since_it_was_listed_is_not_imported() {
        let d = encore_core::tmp::dir("files-again");
        let root = d.join("root");
        std::fs::create_dir_all(root.join("Album")).unwrap();
        std::fs::create_dir_all(d.join("elsewhere")).unwrap();
        std::fs::write(root.join("Album/01.flac"), b"x").unwrap();
        std::fs::write(root.join("Album/.hidden.flac"), b"x").unwrap();
        std::os::unix::fs::symlink(d.join("elsewhere"), root.join("Album/linked.flac")).unwrap();
        let r = Root::new("staging", "Staging", root.clone(), RootKind::Staging);
        let t = Target::new(&r, "").unwrap();
        let f = folder(&t, "Album").unwrap();
        assert_eq!(f.files, vec![std::fs::canonicalize(&root).unwrap().join("Album/01.flac")], "no hidden files, no links");
        assert!(again(&f).is_ok());
        std::fs::write(root.join("Album/02.flac"), b"y").unwrap();
        assert_eq!(again(&f).unwrap_err().status.as_u16(), 409, "a file was added");
        std::fs::remove_file(root.join("Album/02.flac")).unwrap();
        std::fs::remove_file(root.join("Album/01.flac")).unwrap();
        std::fs::write(root.join("Album/01.flac"), b"other").unwrap();
        assert_eq!(again(&f).unwrap_err().status.as_u16(), 409, "a file was swapped");
        let f = folder(&t, "Album").unwrap();
        std::fs::rename(root.join("Album"), d.join("moved")).unwrap();
        std::os::unix::fs::symlink(d.join("moved"), root.join("Album")).unwrap();
        assert_eq!(again(&f).unwrap_err().status.as_u16(), 403, "the folder is a link now");
        let _ = std::fs::remove_dir_all(&d);
    }
}
