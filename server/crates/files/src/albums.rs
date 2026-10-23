//! Library → Albums' Delete… and an album's "Move album to Trash" (0.36, docs/contracts/library-types.md §4.3,
//! §5): whole albums go to the file manager's Trash, through the same [`crate::trash::trash_one`] and
//! [`crate::rows::Change`] as Files → Move to trash, so Files → Trash → Restore brings them back (old ids,
//! playlist entries, who added them, their type).
//!
//! An album becomes one Trash item for its folder when that folder is its own (strictly inside a root, not a
//! library folder, holding no extra library folder, and every song file in it is a song of the albums this
//! call moves: two selected albums sharing a folder move it once); otherwise one item per song file, its
//! lyrics files (.lrc / .txt) going with it, and the folders left empty are tidied up.
use crate::dirfd::free_bytes;
use crate::resolve::Target;
use crate::roots::{self, LibraryDirs, Root, RootKind};
use crate::rows::Change;
use crate::Who;
use encore_core::error::{err, ApiError};
use encore_core::text::size_text;
use encore_db::{p, RowExt};
use encore_library::Ctx;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Albums a dry run may count at once.
pub const MAX_DRY: usize = 5000;
/// Albums one real run moves (the apps send bigger selections in chunks of this many).
pub const MAX_REAL: usize = 100;
/// What a Trash on another disk must have free beyond the albums' size.
const SPARE: u64 = 1 << 30;

/// Why an album stays: its files aren't under any root.
pub const OUTSIDE_ROOTS: &str = "Its files are outside the folders Encore manages";
/// Why an album stays: none of its files are on the disk.
pub const FILE_MISSING: &str = "Its files aren't there any more";

/// One song of an album to move: its row's id, path (as the settings name the folder) and size.
#[derive(Clone, Debug)]
struct Song {
    /// the file, as the settings name its folder
    path: PathBuf,
    /// its size in bytes
    size: u64,
}

/// An album to move: its row and its songs' files.
#[derive(Clone, Debug)]
struct Album {
    /// the album row's id
    id: String,
    /// its title, for messages
    title: String,
    /// its songs with files
    songs: Vec<Song>,
}

impl Album {
    /// The size of all its songs' files.
    fn bytes(&self) -> u64 {
        self.songs.iter().map(|s| s.size).sum()
    }
}

/// "Delete 1 album (1.5 MB)?" / "Delete 12 albums (3.4 GB)?"
pub fn confirm_text(albums: usize, bytes: u64) -> String {
    if albums == 1 { format!("Delete 1 album ({})?", size_text(bytes)) } else { format!("Delete {albums} albums ({})?", size_text(bytes)) }
}

/// "Moved 1 album to the Trash" / "Moved 12 albums to the Trash"
pub fn moved_text(albums: usize) -> String {
    if albums == 1 { "Moved 1 album to the Trash".into() } else { format!("Moved {albums} albums to the Trash") }
}

/// Why some of them stay ("" when none do).
pub fn skipped_text(skipped: &[Value]) -> String {
    let Some(first) = skipped.first() else { return String::new() };
    let reason = first["reason"].as_str().unwrap_or("");
    if skipped.len() == 1 {
        format!("1 of them can't be moved to the Trash and stays: {reason}")
    } else {
        format!("{} of them can't be moved to the Trash and stay: {reason}", skipped.len())
    }
}

/// The text when the Trash's disk hasn't room for them.
pub fn room_text(bytes: u64, free: u64) -> String {
    format!("There isn't room in the Trash for {} ({} free on its disk). Empty the Trash first, or delete fewer albums.", size_text(bytes), size_text(free))
}

/// The albums' songs (in the order asked, each album once) and the ids the library doesn't have.
fn albums_of(st: &impl Ctx, ids: &[String]) -> Result<(Vec<Album>, Vec<String>), ApiError> {
    let mut seen: HashSet<&str> = HashSet::new();
    let wanted: Vec<&String> = ids.iter().filter(|i| !i.is_empty() && seen.insert(i.as_str())).collect();
    let mut found: HashMap<String, Album> = HashMap::new();
    for chunk in wanted.chunks(500) {
        let marks = encore_library::artists::marks(chunk.len());
        let rows = st.db().query(&format!("SELECT album_id, album, path, size FROM tracks WHERE album_id IN ({marks}) ORDER BY path"),
            chunk.iter().map(|i| json!(i)).collect()).map_err(|e| err(500, e.to_string()))?;
        for r in rows {
            let a = found.entry(r.s("album_id")).or_insert_with(|| Album { id: r.s("album_id"), title: r.s("album"), songs: vec![] });
            a.songs.push(Song { path: PathBuf::from(r.s("path")), size: r.i("size").max(0) as u64 });
        }
    }
    let mut albums = vec![];
    let mut missing = vec![];
    for id in wanted {
        match found.remove(id.as_str()) {
            Some(a) => albums.push(a),
            None => missing.push(id.clone()),
        }
    }
    Ok((albums, missing))
}

/// The file manager root that holds a row path (as the settings name it, or resolved), never Trash.
fn root_of<'a>(roots: &'a [Root], path: &Path) -> Option<&'a Root> {
    roots.iter().filter(|r| r.kind != RootKind::Trash)
        .filter(|r| path.starts_with(&r.path) || r.canon.as_deref().is_some_and(|c| path.starts_with(c)))
        .max_by_key(|r| r.path.components().count())
}

/// Is the Trash on another disk than one of these roots (a move there is then a copy)?
fn other_disk(st: &impl Ctx, roots: &[&Root]) -> bool {
    let trash = roots::trash_dir(st.data_dir());
    let _ = std::fs::create_dir_all(&trash);
    let Ok(t) = std::fs::metadata(&trash) else { return false };
    roots.iter().any(|r| r.canon.as_ref().and_then(|c| std::fs::metadata(c).ok()).is_some_and(|m| m.dev() != t.dev()))
}

/// What a run would move and what it would leave: (albums that can go, the ones that stay with why).
fn split<'a>(roots: &[Root], albums: &'a [Album]) -> (Vec<&'a Album>, Vec<Value>) {
    let (mut go, mut stay) = (vec![], vec![]);
    for a in albums {
        if a.songs.iter().all(|s| root_of(roots, &s.path).is_some()) {
            go.push(a);
        } else {
            stay.push(json!({ "id": a.id, "album": a.title, "reason": OUTSIDE_ROOTS }));
        }
    }
    (go, stay)
}

/// The deepest folder that holds every one of these paths' folders.
fn common_folder(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut it = paths.iter().filter_map(|p| p.parent());
    let mut acc = it.next()?.to_path_buf();
    for p in it {
        while !p.starts_with(&acc) {
            acc = acc.parent()?.to_path_buf();
        }
    }
    Some(acc)
}

/// `POST /api/library/albums/trash {ids, dry_run}` (and `DELETE /api/library/albums/{id}`, one album): what
/// moving these albums to the Trash would do (a dry run touches nothing), or doing it. Runs on the caller's
/// thread (the API's blocking pool); a real run holds the file manager's lock.
pub fn trash_albums(st: &impl Ctx, who: &Who, roots: &[Root], ids: &[String], dry_run: bool) -> Result<Value, ApiError> {
    if ids.iter().all(|i| i.trim().is_empty()) {
        return Err(err(400, "Pick at least one album"));
    }
    let max = if dry_run { MAX_DRY } else { MAX_REAL };
    if ids.len() > max {
        return Err(err(400, format!("At most {max} albums at a time")));
    }
    let _g = (!dry_run).then(crate::lock);
    let (albums, missing) = albums_of(st, ids)?;
    let (go, skipped) = split(roots, &albums);
    let bytes: u64 = go.iter().map(|a| a.bytes()).sum();
    let songs: usize = go.iter().map(|a| a.songs.len()).sum();
    let used: Vec<&Root> = {
        let mut v: Vec<&Root> = vec![];
        for a in &go {
            for s in &a.songs {
                if let Some(r) = root_of(roots, &s.path) {
                    if !v.iter().any(|x| x.id == r.id) {
                        v.push(r);
                    }
                }
            }
        }
        v
    };
    let far = other_disk(st, &used);
    let free = free_bytes(&roots::trash_dir(st.data_dir()));
    let room = !far || bytes + SPARE <= free;
    if dry_run {
        return Ok(json!({
            "albums": go.len(), "songs": songs, "bytes": bytes, "size": size_text(bytes), "text": confirm_text(go.len(), bytes),
            "skipped": skipped, "skipped_text": skipped_text(&skipped), "other_disk": far, "room": room,
            "room_text": if room { String::new() } else { room_text(bytes, free) }, "missing": missing,
        }));
    }
    if !room {
        return Err(err(507, room_text(bytes, free)));
    }
    let done = move_albums(st, who, roots, &go);
    let mut failed: Vec<Value> = skipped;
    failed.extend(done.failed);
    let moved: Vec<&&Album> = go.iter().filter(|a| done.gone.contains(&a.id)).collect();
    let moved_songs: usize = moved.iter().map(|a| a.songs.len()).sum();
    let moved_bytes: u64 = moved.iter().map(|a| a.bytes()).sum();
    for a in &moved {
        st.library().forget_cover(Some(&a.id));
    }
    if !moved.is_empty() {
        let what = if moved.len() == 1 { "1 album".to_string() } else { format!("{} albums", moved.len()) };
        st.db().log("info", &format!("{} moved {what} to the Trash ({moved_songs} songs, {})", who.name, size_text(moved_bytes)));
    }
    Ok(json!({ "albums": moved.len(), "songs": moved_songs, "bytes": moved_bytes, "items": done.items, "failed": failed,
               "missing": missing, "text": moved_text(moved.len()) }))
}

/// What the moves did.
#[derive(Default)]
struct Moved {
    /// trash ids, in the order made
    items: Vec<String>,
    /// albums all of whose songs went
    gone: BTreeSet<String>,
    /// albums that couldn't be moved: `{id, album, reason}` each
    failed: Vec<Value>,
}

/// One planned Trash item: a folder (of one or more albums) or one song with its lyrics files.
struct Item {
    /// the file or folder to move, resolved
    real: PathBuf,
    /// the albums it holds
    albums: Vec<String>,
    /// files next to a song that go with it (its .lrc and .txt lyrics)
    extras: Vec<String>,
}

/// Move the albums' files to the Trash: whole folders when an album (or several) fills one, single songs otherwise.
fn move_albums(st: &impl Ctx, who: &Who, roots: &[Root], go: &[&Album]) -> Moved {
    let mut out = Moved::default();
    let library = roots.iter().find(|r| r.kind == RootKind::Library).and_then(|r| r.canon.clone());
    let extras: Vec<PathBuf> = roots::extra_roots(st.cfg()).iter().map(|e| std::fs::canonicalize(e).unwrap_or_else(|_| e.clone())).collect();
    // every song's file as it is on the disk. A song whose file is gone while others of its album are there
    // is gone already: its row leaves with the album. An album none of whose files is there (a disk that
    // isn't mounted looks the same), or one whose files can't be looked at, stays whole: nothing of it moves.
    let mut real: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut lost: BTreeSet<String> = BTreeSet::new();
    let mut gone_rows: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for a in go {
        let (mut v, mut gone, mut broken) = (vec![], vec![], false);
        for s in &a.songs {
            match std::fs::canonicalize(&s.path) {
                Ok(r) => v.push(r),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => gone.push(s.path.clone()),
                Err(_) => broken = true,
            }
        }
        if broken || v.is_empty() {
            lost.insert(a.id.clone());
            v.clear();
        } else if !gone.is_empty() {
            gone_rows.insert(a.id.clone(), gone);
        }
        real.insert(a.id.clone(), v);
    }
    let all_songs: HashSet<&PathBuf> = real.values().flatten().collect();
    // each album's folder, and whether it can go whole
    let mut folders: Vec<(PathBuf, Vec<String>)> = vec![];
    let mut per_song: Vec<&Album> = vec![];
    for a in go {
        let files = &real[&a.id];
        let whole = common_folder(files).filter(|f| {
            let Some(root) = roots::holding(roots, f).filter(|r| r.kind != RootKind::Trash) else { return false };
            let inside = root.canon.as_deref().is_some_and(|c| f.starts_with(c) && f != c);
            inside && Some(f) != library.as_ref() && !extras.iter().any(|e| e.starts_with(f))
                && walkdir::WalkDir::new(f).follow_links(false).into_iter().flatten()
                    .filter(|e| e.file_type().is_file() && crate::names::is_audio(&e.file_name().to_string_lossy()))
                    .all(|e| all_songs.contains(&e.path().to_path_buf()))
        });
        match whole {
            _ if lost.contains(&a.id) => {}
            Some(f) => match folders.iter_mut().find(|(x, _)| *x == f) {
                Some((_, ids)) => ids.push(a.id.clone()),
                None => folders.push((f, vec![a.id.clone()])),
            },
            None => per_song.push(a),
        }
    }
    // a folder inside another that goes whole goes with it
    folders.sort_by_key(|(f, _)| f.components().count());
    let mut items: Vec<Item> = vec![];
    for (f, ids) in folders {
        match items.iter_mut().find(|i| f.starts_with(&i.real)) {
            Some(outer) => outer.albums.extend(ids),
            None => items.push(Item { real: f, albums: ids, extras: vec![] }),
        }
    }
    let mut prune: Vec<PathBuf> = vec![];
    for a in &per_song {
        for f in &real[&a.id] {
            let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let extras: Vec<String> = ["lrc", "txt"].iter().map(|x| format!("{stem}.{x}")).filter(|n| *n != name).collect();
            items.push(Item { real: f.clone(), albums: vec![a.id.clone()], extras });
            if let Some(d) = f.parent() {
                if !prune.contains(&d.to_path_buf()) {
                    prune.push(d.to_path_buf());
                }
            }
        }
    }
    let mut change = Change::default();
    let mut bad: HashMap<String, String> = HashMap::new();
    for id in &lost {
        bad.insert(id.clone(), FILE_MISSING.into());
    }
    for it in &items {
        let res = (|| -> Result<String, ApiError> {
            let root = roots::holding(roots, &it.real).filter(|r| r.kind != RootKind::Trash).ok_or_else(|| err(403, OUTSIDE_ROOTS))?;
            let rel = it.real.strip_prefix(root.dir()?).map_err(|_| err(403, OUTSIDE_ROOTS))?;
            let t = Target::new(root, &rel.to_string_lossy())?;
            let (dir, name, _) = t.open_existing()?;
            crate::trash::trash_one(st, who, &t, &dir, &name, &mut change, &it.extras)
        })();
        match res {
            Ok(id) => out.items.push(id),
            Err(e) => {
                for a in &it.albums {
                    bad.entry(a.clone()).or_insert_with(|| e.detail.clone());
                }
            }
        }
    }
    // the rows of the songs that were gone already leave with their album, when the rest of it went
    for (id, rows) in gone_rows {
        if !bad.contains_key(&id) {
            for row in rows {
                change.row_removed(row);
            }
        }
    }
    if let Err(e) = change.apply(st) {
        tracing::warn!("files: the library's rows after moving albums to the Trash: {e:#}");
    }
    // the folders the songs left, tidied up to the root or library folder that holds them
    for d in prune {
        let stop = extras.iter().chain(roots.iter().filter_map(|r| r.canon.as_ref())).filter(|s| d.starts_with(s)).max_by_key(|s| s.components().count()).cloned();
        if let Some(stop) = stop {
            encore_library::prune_empty(&d, &stop);
        }
    }
    for a in go {
        match bad.remove(&a.id) {
            Some(reason) => out.failed.push(json!({ "id": a.id, "album": a.title, "reason": reason })),
            None => {
                out.gone.insert(a.id.clone());
            }
        }
    }
    out
}

// ---- who added the songs the 0.33 file manager brought in -------------------------------------------------

/// The setting that says the one-time backfill ran.
pub const BACKFILL_DONE: &str = "added_from_log_v31";

/// 0.36, once: the songs the 0.33 file manager uploaded or imported before songs said who added them get that
/// admin's name back from the activity log ("File manager: alex uploaded library:A/B/1.flac", "… imported in
/// place torrent-1:Album", "… imported into the library staging:Album"). Only rows still called found in the
/// music folder (`scan`) and added within a few minutes of the line change; a name the users table doesn't
/// have leaves the row alone. → the songs given their adder
pub fn backfill_added_from_log(st: &impl Ctx, roots: &[Root]) -> usize {
    let db = st.db();
    if db.one("SELECT 1 x FROM settings WHERE key=?", p![BACKFILL_DONE]).ok().flatten().is_some() {
        return 0;
    }
    let users: HashMap<String, i64> = db.query("SELECT id, username FROM users", p![]).unwrap_or_default().iter().map(|r| (r.s("username").to_lowercase(), r.i("id"))).collect();
    let lines = db.query("SELECT ts, message FROM activity WHERE message LIKE 'File manager: %' ORDER BY id", p![]).unwrap_or_default();
    let dirs = LibraryDirs::of(st.cfg());
    let library = st.cfg().str("library_path").trim().trim_end_matches('/').to_string();
    let mut n = 0;
    for l in lines {
        let msg = l.s("message");
        let Some(rest) = msg.strip_prefix("File manager: ") else { continue };
        let found = [(" uploaded ", "upload"), (" imported in place ", "import"), (" imported into the library ", "import")].into_iter()
            .find_map(|(marker, via)| rest.split_once(marker).map(|(user, place)| (user, place, marker, via)));
        let Some((user, place, marker, via)) = found else { continue };
        let Some(uid) = users.get(&user.trim().to_lowercase()).copied() else { continue };
        let ts = l.f("ts");
        let (cond, params): (&str, Vec<serde_json::Value>) = if marker == " imported into the library " {
            // the files moved into the library under new names: the songs added just before the line
            if library.is_empty() {
                continue;
            }
            ("added_via='scan' AND request_id IS NULL AND added_at BETWEEN ? AND ? AND path > ? AND path < ?",
             p![ts - 600.0, ts + 5.0, format!("{library}/"), format!("{library}0")])
        } else {
            let Some((root_id, rel)) = place.split_once(':') else { continue };
            let Some(root) = roots.iter().find(|r| r.id == root_id) else { continue };
            let Some(base) = root.canon.as_ref().map(|c| if rel.is_empty() { c.clone() } else { c.join(rel) }) else { continue };
            let Some(row_path) = dirs.db_path(&base) else { continue };
            let p0 = row_path.to_string_lossy().into_owned();
            ("added_via='scan' AND added_at BETWEEN ? AND ? AND (path=? OR (path > ? AND path < ?))",
             p![ts - 600.0, ts + 600.0, p0, format!("{p0}/"), format!("{p0}0")])
        };
        let found = db.one(&format!("SELECT COUNT(*) n FROM tracks WHERE {cond}"), params.clone()).ok().flatten().map(|r| r.i("n")).unwrap_or(0);
        if found == 0 {
            continue;
        }
        let mut ps = p![uid, via];
        ps.extend(params);
        if db.execute(&format!("UPDATE tracks SET added_by=?, added_via=? WHERE {cond}"), ps).is_ok() {
            n += found as usize;
        }
    }
    let _ = db.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?, 'true')", p![BACKFILL_DONE]);
    n
}
