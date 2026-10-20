//! The library's rows follow what the file manager does to the files under the library's folders
//! ([`crate::roots::LibraryDirs`]): a move or rename inside the library changes the rows' paths (and the
//! album of a song whose album comes from its folder) – the same rows, so likes, playlists and plays stay
//! with them, as `tagger::relink` keeps them for imports – a move out of the library or to Trash takes the
//! rows out as a delete does (playlist entries and likes are kept as missing), and files that arrive in the
//! library (a move from a download folder, an upload, a restore) are read in as a scan reads them. The path
//! changes and removals of one operation are written in one transaction, so the library version moves once.
use crate::roots::LibraryDirs;
use encore_db::{p, Conn, Row, RowExt};
use encore_library::Ctx;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The rows at `path` or under it (a folder): one look up the path index.
fn rows_at(c: &Conn, path: &Path) -> rusqlite::Result<Vec<Row>> {
    let p = path.to_string_lossy();
    let below = format!("{}/", p.trim_end_matches('/'));
    // every path under "<p>/" sorts between "<p>/" and "<p>0" ('0' follows '/')
    let end = format!("{}0", p.trim_end_matches('/'));
    c.query("SELECT id, path, album, albumartist, album_id FROM tracks WHERE path=? OR (path>? AND path<?)", p![p, below, end])
}

/// What one operation does to the library's rows, gathered while the files move and written at the end.
#[derive(Default)]
pub struct Change {
    /// (old row path, new row path): rows at or under the old path take the new one
    moves: Vec<(PathBuf, PathBuf)>,
    /// rows at or under these paths leave the library
    removals: Vec<PathBuf>,
    /// files that arrived in the library: read in as a scan reads them
    arrivals: Vec<PathBuf>,
}

/// What [`Change::apply`] did.
#[derive(Default, Debug)]
pub struct Applied {
    pub moved: usize,
    pub removed: Vec<i64>,
    pub indexed: usize,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty() && self.removals.is_empty() && self.arrivals.is_empty()
    }

    /// A file or folder went from `old` to `new` (resolved paths): its rows follow, leave or arrive. `before`
    /// are the library's folders before the move, `after` after it (a moved extra library folder is named by
    /// its new path then).
    pub fn moved(&mut self, before: &LibraryDirs, after: &LibraryDirs, old: &Path, new: &Path) {
        match (before.db_path(old), after.db_path(new)) {
            (Some(o), Some(n)) => self.moves.push((o, n)),
            (Some(o), None) => self.removals.push(o),
            (None, Some(_)) => self.arrived(after, new),
            (None, None) => {}
        }
    }

    /// The rows at or under `old` (a row path) take `new`: an extra library folder whose name changed because
    /// a folder above it was renamed or moved (that folder itself has no rows).
    pub fn rows_moved(&mut self, old: PathBuf, new: PathBuf) {
        if old != new {
            self.moves.push((old, new));
        }
    }

    /// A file or folder left the library (to Trash, say).
    pub fn removed(&mut self, dirs: &LibraryDirs, old: &Path) {
        if let Some(o) = dirs.db_path(old) {
            self.removals.push(o);
        }
    }

    /// A file or folder arrived at `new` (resolved): its songs are read in when it is in the library.
    pub fn arrived(&mut self, dirs: &LibraryDirs, new: &Path) {
        if dirs.db_path(new).is_none() {
            return;
        }
        let files: Vec<PathBuf> = if new.is_dir() {
            walkdir::WalkDir::new(new).follow_links(false).into_iter().flatten()
                .filter(|e| e.file_type().is_file() && crate::names::is_audio(&e.file_name().to_string_lossy()))
                .map(|e| e.into_path()).collect()
        } else if crate::names::is_audio(&new.file_name().unwrap_or_default().to_string_lossy()) {
            vec![new.to_path_buf()]
        } else {
            vec![]
        };
        self.arrivals.extend(files.iter().filter_map(|f| dirs.db_path(f)));
    }

    /// Writes the change: path changes and removals in one transaction, then the arrivals read in; the
    /// covers of the albums touched are looked at again.
    pub fn apply(self, st: &impl Ctx) -> anyhow::Result<Applied> {
        let mut out = Applied::default();
        if self.is_empty() {
            return Ok(out);
        }
        let (moves, removals) = (self.moves, self.removals);
        let (moved, removed, albums) = st.db().write_now(|c| c.transaction(|c| {
            let mut albums: BTreeSet<String> = BTreeSet::new();
            let mut removed: Vec<i64> = vec![];
            let mut moved = 0;
            for (old, new) in &moves {
                let (o, n) = (old.to_string_lossy().into_owned(), new.to_string_lossy().into_owned());
                for r in rows_at(c, old)? {
                    let path = r.s("path");
                    let np = format!("{n}{}", &path[o.len()..]);
                    // a stale row where the file now is (its file went away without a scan): the moved row takes
                    // its playlist entries and its place, as tagger::relink does
                    if let Some(clash) = c.one("SELECT id FROM tracks WHERE path=?", p![np])? {
                        if clash.i("id") != r.i("id") {
                            c.execute("UPDATE playlist_items SET track_id=? WHERE track_id=?", p![r.i("id"), clash.i("id")])?;
                            c.execute("DELETE FROM tracks WHERE id=?", p![clash.i("id")])?;
                            removed.push(clash.i("id"));
                        }
                    }
                    // a song without an album tag is its folder's album: its album follows the folder
                    let album_id = if r.s("album").trim().is_empty() { encore_library::album_key(&r.s("albumartist"), "", &np) } else { r.s("album_id") };
                    c.execute("UPDATE tracks SET path=?, album_id=? WHERE id=?", p![np, album_id, r.i("id")])?;
                    albums.insert(r.s("album_id"));
                    albums.insert(album_id);
                    moved += 1;
                }
            }
            for old in &removals {
                let rows = rows_at(c, old)?;
                if rows.is_empty() {
                    continue;
                }
                let ids: Vec<Value> = rows.iter().map(|r| json!(r.i("id"))).collect();
                let marks = encore_library::artists::marks(ids.len());
                // likes and playlist entries stay, as missing songs (a delete does the same)
                c.execute(&format!("UPDATE playlist_items SET track_id=NULL WHERE track_id IN ({marks})"), ids.clone())?;
                c.execute(&format!("DELETE FROM tracks WHERE id IN ({marks})"), ids)?;
                albums.extend(rows.iter().map(|r| r.s("album_id")));
                removed.extend(rows.iter().map(|r| r.i("id")));
            }
            Ok::<_, rusqlite::Error>((moved, removed, albums))
        }))?;
        for id in &removed {
            st.track_removed(*id);
        }
        out.moved = moved;
        out.removed = removed;
        let albums: Vec<String> = albums.into_iter().filter(|a| !a.is_empty()).collect();
        if !albums.is_empty() {
            st.library().refresh_covers(st.db().clone(), albums);
        }
        if !self.arrivals.is_empty() {
            encore_library::tagger::index_paths(st, &self.arrivals);
            out.indexed = self.arrivals.len();
        }
        Ok(out)
    }
}

/// A song's row before it went to Trash: its id, where it was relative to the trashed item ("" for the item
/// itself) and the playlist entries (likes too) that pointed at it, so a restore can put all of it back.
pub fn saved_rows(st: &impl Ctx, dirs: &LibraryDirs, real: &Path) -> Vec<Value> {
    let Some(base) = dirs.db_path(real) else { return vec![] };
    let rows = st.db().read_now(|c| rows_at(c, &base)).unwrap_or_default();
    let b = base.to_string_lossy().into_owned();
    rows.iter().map(|r| {
        let items: Vec<i64> = st.db().query("SELECT id FROM playlist_items WHERE track_id=?", p![r.i("id")]).unwrap_or_default().iter().map(|x| x.i("id")).collect();
        let rel = r.s("path")[b.len()..].trim_start_matches('/').to_string();
        json!({ "id": r.i("id"), "rel": rel, "items": items })
    }).collect()
}

/// After a restore read the songs in again: each takes back its old id when that is free (plays, the quality
/// check and the source it came from stay with it), and the playlist entries and likes that pointed at it
/// and are still missing point at it again. `base` is the restored item's row path.
pub fn give_back(st: &impl Ctx, base: &Path, saved: &[Value]) -> usize {
    let b = base.to_string_lossy().into_owned();
    let r = st.db().write_now(|c| c.transaction(|c| {
        let mut n = 0;
        for s in saved {
            let rel = s["rel"].as_str().unwrap_or("");
            let path = if rel.is_empty() { b.clone() } else { format!("{}/{rel}", b.trim_end_matches('/')) };
            let Some(row) = c.one("SELECT id FROM tracks WHERE path=?", p![path])? else { continue };
            let mut id = row.i("id");
            let old = s["id"].as_i64().unwrap_or(0);
            if old > 0 && old != id && c.one("SELECT id FROM tracks WHERE id=?", p![old])?.is_none() {
                c.execute("UPDATE tracks SET id=? WHERE id=?", p![old, id])?;
                id = old;
            }
            for item in s["items"].as_array().cloned().unwrap_or_default() {
                c.execute("UPDATE playlist_items SET track_id=? WHERE id=? AND track_id IS NULL", p![id, item])?;
            }
            n += 1;
        }
        Ok::<_, rusqlite::Error>(n)
    }));
    r.unwrap_or_else(|e| {
        tracing::warn!("files: giving restored songs their rows back: {e}");
        0
    })
}
