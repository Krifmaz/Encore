//! A folder's entries (§4.2 of the client shapes): `{name, kind, size, mtime, audio, image}` (a link also has
//! `target` and `target_kind`), folders first in every sort, 500 at a time. What the library knows about its
//! audio files comes from one query for the whole page.
use crate::names;
use crate::resolve::{browse_dir, Target};
use crate::roots::{LibraryDirs, Root};
use encore_core::error::{err, ApiError};
use encore_db::{Db, RowExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// The most entries one answer has.
pub const PAGE: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Name,
    Size,
    Mtime,
}

pub struct Query {
    pub sort: Sort,
    pub desc: bool,
    pub offset: usize,
    pub limit: usize,
}

impl Query {
    pub fn parse(sort: Option<&str>, dir: Option<&str>, offset: Option<usize>, limit: Option<usize>) -> Result<Query, ApiError> {
        let sort = match sort.unwrap_or("name") {
            "" | "name" => Sort::Name,
            "size" => Sort::Size,
            "mtime" | "date" => Sort::Mtime,
            _ => return Err(err(400, "sort is name, size or mtime")),
        };
        let desc = match dir.unwrap_or("asc") {
            "" | "asc" => false,
            "desc" => true,
            _ => return Err(err(400, "dir is asc or desc")),
        };
        Ok(Query { sort, desc, offset: offset.unwrap_or(0), limit: limit.unwrap_or(PAGE).clamp(1, PAGE) })
    }
}

struct Entry {
    name: String,
    kind: &'static str,
    size: u64,
    mtime: i64,
    target: Option<String>,
    target_kind: Option<&'static str>,
}

fn secs(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn read(dir: &Path) -> Result<Vec<Entry>, ApiError> {
    let rd = std::fs::read_dir(dir).map_err(|e| err(500, format!("Couldn't read the folder: {e}")))?;
    let mut out = vec![];
    for e in rd.flatten() {
        // a name that isn't UTF-8 can't be named back by a client: it is left out
        let Some(name) = e.file_name().to_str().map(String::from) else { continue };
        let Ok(meta) = e.path().symlink_metadata() else { continue };
        let ft = meta.file_type();
        let entry = if ft.is_symlink() {
            let target = std::fs::read_link(e.path()).ok().map(|t| t.to_string_lossy().into_owned());
            let (tk, size, mtime) = match std::fs::metadata(e.path()) {
                Ok(m) if m.is_dir() => ("dir", 0, secs(&m)),
                Ok(m) => ("file", m.len(), secs(&m)),
                Err(_) => ("missing", 0, secs(&meta)),
            };
            Entry { name, kind: "link", size, mtime, target, target_kind: Some(tk) }
        } else if ft.is_dir() {
            Entry { name, kind: "dir", size: 0, mtime: secs(&meta), target: None, target_kind: None }
        } else {
            Entry { name, kind: "file", size: meta.len(), mtime: secs(&meta), target: None, target_kind: None }
        };
        out.push(entry);
    }
    Ok(out)
}

/// Lowercase for the name sort, then the name itself (so "a" and "A" keep one order).
fn name_key(n: &str) -> (String, String) {
    (n.to_lowercase(), n.to_string())
}

fn sort(entries: &mut [Entry], q: &Query) {
    let is_dir = |e: &Entry| e.kind == "dir" || (e.kind == "link" && e.target_kind == Some("dir"));
    entries.sort_by(|a, b| {
        // folders first, whichever way
        let group = is_dir(b).cmp(&is_dir(a));
        let key = match q.sort {
            Sort::Name => name_key(&a.name).cmp(&name_key(&b.name)),
            Sort::Size => a.size.cmp(&b.size).then_with(|| name_key(&a.name).cmp(&name_key(&b.name))),
            Sort::Mtime => a.mtime.cmp(&b.mtime).then_with(|| name_key(&a.name).cmp(&name_key(&b.name))),
        };
        group.then(if q.desc { key.reverse() } else { key })
    });
}

/// `GET /api/files/list`: `{root, path, total, offset, entries, via_link}`.
pub fn list(db: &Db, dirs: &LibraryDirs, roots: &[Root], t: &Target, q: &Query) -> Result<Value, ApiError> {
    let (dir, via_link) = browse_dir(t, roots)?;
    let mut entries = read(&dir)?;
    sort(&mut entries, q);
    let total = entries.len();
    let page: Vec<Entry> = entries.into_iter().skip(q.offset).take(q.limit).collect();
    // the library's rows of the page's audio files: one query
    let mut known: HashMap<String, encore_db::Row> = HashMap::new();
    if dirs.contains(&dir) {
        let paths: Vec<(String, String)> = page.iter()
            .filter(|e| e.kind == "file" && names::is_audio(&e.name))
            .filter_map(|e| dirs.db_path(&dir.join(&e.name)).map(|p| (e.name.clone(), p.to_string_lossy().into_owned())))
            .collect();
        if !paths.is_empty() {
            let marks = encore_library::artists::marks(paths.len());
            let rows = db.query(&format!("SELECT id, path, format, duration, title, artist, album FROM tracks WHERE path IN ({marks})"),
                paths.iter().map(|(_, p)| json!(p)).collect()).unwrap_or_default();
            let by_path: HashMap<String, encore_db::Row> = rows.into_iter().map(|r| (r.s("path"), r)).collect();
            for (name, p) in paths {
                if let Some(r) = by_path.get(&p) {
                    known.insert(name, r.clone());
                }
            }
        }
    }
    let out: Vec<Value> = page.iter().map(|e| {
        let mut v = json!({ "name": e.name, "kind": e.kind, "size": e.size, "mtime": e.mtime, "image": e.kind != "dir" && names::is_image(&e.name) });
        if let Some(tk) = e.target_kind {
            v["target"] = json!(e.target.clone().unwrap_or_default());
            v["target_kind"] = json!(tk);
        }
        if e.kind == "file" && names::is_audio(&e.name) {
            v["audio"] = match known.get(&e.name) {
                Some(r) => json!({ "format": r.s("format"), "duration": r.f("duration"), "title": r.s("title"), "artist": r.s("artist"),
                                   "album": r.s("album"), "track_id": r.i("id") }),
                None => json!({ "format": names::ext_of(&e.name) }),
            };
        }
        v
    }).collect();
    Ok(json!({ "root": t.root.id, "path": t.rel(), "total": total, "offset": q.offset, "entries": out, "via_link": via_link }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, kind: &'static str, size: u64, mtime: i64) -> Entry {
        Entry { name: name.into(), kind, size, mtime, target: None, target_kind: None }
    }

    #[test]
    fn folders_come_first_in_every_sort() {
        let mut v = vec![e("b.flac", "file", 10, 3), e("Zed", "dir", 0, 1), e("a.flac", "file", 30, 2), e("alpha", "dir", 0, 5),
                         Entry { name: "link".into(), kind: "link", size: 0, mtime: 0, target: None, target_kind: Some("dir") }];
        let names = |v: &[Entry]| v.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        sort(&mut v, &Query::parse(None, None, None, None).unwrap());
        assert_eq!(names(&v), ["alpha", "link", "Zed", "a.flac", "b.flac"]);
        sort(&mut v, &Query::parse(Some("name"), Some("desc"), None, None).unwrap());
        assert_eq!(names(&v), ["Zed", "link", "alpha", "b.flac", "a.flac"]);
        sort(&mut v, &Query::parse(Some("size"), Some("desc"), None, None).unwrap());
        assert_eq!(&names(&v)[3..], ["a.flac", "b.flac"]);
        sort(&mut v, &Query::parse(Some("mtime"), Some("asc"), None, None).unwrap());
        assert_eq!(names(&v), ["link", "Zed", "alpha", "a.flac", "b.flac"]);
        assert!(Query::parse(Some("colour"), None, None, None).is_err());
        assert_eq!(Query::parse(None, None, None, Some(10_000)).unwrap().limit, PAGE);
    }
}
