//! Trash (§4.7–4.11 of the client shapes): Move to trash puts each item in its own folder
//! `<config>/trash/<unix>-<n>/` with a `.origin.json` beside it – `{root, path, at, by}` and, for songs of the
//! library, their rows' ids and the playlist entries (likes too) that pointed at them, so a restore gives
//! all of it back. The songs leave the library as a delete takes them out. Restore puts an item back where
//! it was (missing folders made again, " (2)" when the name is taken). Delete forever and Empty trash are
//! the only real deletes the file manager has.
//!
//! The report flow parks reported songs in the same `<config>/trash` as plain files (`<unix>-<id>-<name>`);
//! those are not file manager items: they are never listed, emptied or restored here.
use crate::dirfd::{size_of, Dir, Kind};
use crate::names::{self, MISSING};
use crate::resolve::{io_error, Target};
use crate::roots::{self, LibraryDirs, Root};
use crate::rows::{self, Change};
use crate::Who;
use encore_core::error::{err, ApiError};
use encore_db::now;
use encore_library::Ctx;
use serde_json::{json, Value};
use std::path::PathBuf;

/// The file beside each trashed item that says where it came from.
pub const ORIGIN: &str = ".origin.json";

/// Is this a file manager trash id (`<unix>-<n>`)? Anything else (a path, a report's parked file) is refused.
pub fn valid_id(id: &str) -> bool {
    let mut parts = id.split('-');
    let ok = |s: Option<&str>| s.is_some_and(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()));
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

fn trash_root(st: &impl Ctx) -> Result<Dir, ApiError> {
    let dir = roots::trash_dir(st.data_dir());
    encore_core::fsx::mkdir_all(&dir, encore_core::fsx::Step::Files).map_err(io_error)?;
    Dir::open(&dir).map_err(io_error)
}

/// A new, empty item folder `<unix>-<n>` in the trash.
fn new_item(trash: &Dir) -> Result<String, ApiError> {
    let t = now() as i64;
    for n in 1..10_000 {
        let id = format!("{t}-{n}");
        match trash.mkdir(&id) {
            Ok(()) => return Ok(id),
            Err(e) if e.errno == Some(libc::EEXIST) => continue,
            Err(e) => return Err(io_error(e)),
        }
    }
    Err(err(500, "Couldn't make a folder in the trash"))
}

/// Moves one item to the trash: `dir`/`name` (the item `t` names) into a new item folder, with its origin.
/// `extras`: files beside it in `dir` that go with it into the same item (a song's .lrc / .txt lyrics) and come
/// back beside it on a restore. The library's rows of it are taken out through `change`. → the trash id
pub fn trash_one(st: &impl Ctx, who: &Who, t: &Target, dir: &Dir, name: &str, change: &mut Change, extras: &[String]) -> Result<String, ApiError> {
    let kind = dir.lstat(name).map_err(io_error)?.ok_or_else(|| err(404, MISSING))?.kind;
    let real = dir.path.join(name);
    // a folder that holds an extra library folder: its songs' rows can't go to Trash with it (they are named
    // by that folder, which the setting would still name), so it is refused
    if let Some(inner) = roots::extra_roots(st.cfg()).into_iter().find(|e| e.starts_with(&real) && *e != real) {
        return Err(err(409, format!("“{name}” holds an extra library folder ({}): take that out of Settings → Library → Extra library folders first, or move it to Trash by itself", inner.display())));
    }
    let dirs = LibraryDirs::of(st.cfg());
    let trash = trash_root(st)?;
    // the extras that are there, plain files beside it (never the item itself, never twice)
    let mut extras: Vec<&str> = extras.iter().map(String::as_str).filter(|e| *e != name && *e != ORIGIN && names::check_name(e).is_ok()).collect();
    extras.dedup();
    extras.retain(|e| dir.lstat(e).ok().flatten().is_some_and(|x| x.kind == Kind::File));
    // on another disk the trash holds a whole copy: there must be room for it
    let size = size_of(dir, name).unwrap_or(0) + extras.iter().map(|e| size_of(dir, e).unwrap_or(0)).sum::<u64>();
    let other_disk = dir.stat().map(|s| s.dev).ok() != trash.stat().map(|s| s.dev).ok();
    if other_disk && size > 0 && size > crate::dirfd::free_bytes(&trash.path) {
        return Err(err(507, format!("There isn't room for it in the trash ({})", trash.path.display())));
    }
    let id = new_item(&trash)?;
    let item = trash.child(&id).map_err(io_error)?;
    let undo = |trash: &Dir| {
        let _ = trash.remove_all(&id);
    };
    // the item keeps its name, unless it is called like the origin file
    let stored = if name == ORIGIN { format!("{ORIGIN} (item)") } else { name.to_string() };
    // each extra under a name of its own in the item folder
    let mut taken: Vec<String> = vec![stored.clone(), ORIGIN.to_string()];
    let extra_names: Vec<(String, String)> = extras.iter().map(|e| {
        let n = names::free_name(e, false, |x| taken.iter().any(|t| t == x));
        taken.push(n.clone());
        (e.to_string(), n)
    }).collect();
    let origin = json!({
        "root": t.root.id, "path": t.rel(), "at": now() as i64, "by": who.name, "name": name, "stored": stored,
        "kind": if kind == Kind::Dir { "dir" } else { "file" }, "link": kind == Kind::Link, "size": size,
        "tracks": rows::saved_rows(st, &dirs, &real),
        "extras": extra_names.iter().map(|(n, stored)| json!({ "name": n, "stored": stored })).collect::<Vec<_>>(),
    });
    let written = item.create_new(ORIGIN).map_err(io_error).and_then(|mut f| {
        use std::io::Write;
        f.write_all(serde_json::to_string_pretty(&origin).unwrap_or_default().as_bytes()).and_then(|_| f.sync_all())
            .map_err(|e| err(500, format!("Couldn't write the trash's note of where it came from: {e}")))
    });
    if let Err(e) = written {
        undo(&trash);
        return Err(e);
    }
    if let Err(e) = crate::ops::move_entry(dir, name, &item, &stored) {
        undo(&trash);
        return Err(e);
    }
    // the extras follow it; one that can't be moved stays where it is (the item went all the same)
    for (n, stored) in &extra_names {
        if let Err(e) = crate::ops::move_entry(dir, n, &item, stored) {
            tracing::warn!("files: {} stays beside the place {name} left: {}", n, e.detail);
        }
    }
    if other_disk && size > 0 {
        tracing::info!("files: {} went to the trash on another disk ({size} bytes copied)", real.display());
    }
    change.removed(&dirs, &real);
    crate::audit(st, who, "moved to trash", t, None);
    Ok(id)
}

/// `POST /api/files/trash {root, paths}` → `{trashed, items, failed}`.
pub fn trash(st: &impl Ctx, who: &Who, items: &[Target]) -> Result<Value, ApiError> {
    let _g = crate::lock();
    let mut change = Change::default();
    let (mut ids, mut failed) = (vec![], vec![]);
    for t in items {
        let res = t.open_existing().and_then(|(dir, name, _)| trash_one(st, who, t, &dir, &name, &mut change, &[]));
        match res {
            Ok(id) => ids.push(id),
            Err(e) => failed.push(json!({ "path": t.rel(), "reason": e.detail })),
        }
    }
    if let Err(e) = change.apply(st) {
        tracing::warn!("files: the library's rows after a move to trash: {e:#}");
    }
    Ok(json!({ "trashed": ids.len(), "items": ids, "failed": failed }))
}

/// The name the item is stored under in its folder, from its note: one plain name (never a path), and its
/// own `name` must be one too. None for a note that says otherwise: such an item is neither listed nor
/// restored.
fn stored_name(o: &Value) -> Option<String> {
    let name = o["name"].as_str();
    let stored = o["stored"].as_str().or(name)?;
    let plain = |n: &str| names::check_name(n).is_ok() && n != ORIGIN;
    (plain(stored) && name.is_none_or(plain)).then(|| stored.to_string())
}

/// Removes an item from the trash for good: what it holds first, its note last, then its folder. An item that
/// can't be removed whole keeps its note, so it stays listed (and Delete forever or Empty trash can try
/// again) instead of lingering where nobody sees it.
fn remove_item(trash: &Dir, id: &str) -> Result<(), encore_core::fsx::StepError> {
    let item = trash.child(id)?;
    for n in item.names()? {
        if n != ORIGIN {
            item.remove_all(&n)?;
        }
    }
    // a name that isn't UTF-8 is left by names(): the note stays with it
    if item.count()? > 1 {
        return Err(encore_core::fsx::StepError::new(encore_core::fsx::Step::Files, "remove", &item.path, None, std::io::Error::from_raw_os_error(libc::ENOTEMPTY)));
    }
    match item.unlink(ORIGIN) {
        Err(e) if e.errno != Some(libc::ENOENT) => return Err(e),
        _ => {}
    }
    trash.rmdir(id)
}

/// An item that couldn't be removed whole, as the API answer: it stays in the trash, listed with its note.
fn not_all(e: encore_core::fsx::StepError) -> ApiError {
    let status = if matches!(e.errno, Some(libc::EACCES) | Some(libc::EPERM) | Some(libc::EROFS)) { 403 } else { 409 };
    err(status, format!("Not all of it could be deleted, so it stays in the trash ({})", e.what()))
}

/// The origin note of an item (None for anything that isn't a file manager item).
fn origin_of(trash: &Dir, id: &str) -> Option<Value> {
    if !valid_id(id) {
        return None;
    }
    let item = trash.child(id).ok()?;
    let mut f = item.open_file(ORIGIN).ok()?;
    let mut s = String::new();
    std::io::Read::read_to_string(&mut f, &mut s).ok()?;
    serde_json::from_str(&s).ok()
}

/// `GET /api/files/trash`: the items, newest first.
pub fn list(st: &impl Ctx) -> Result<Value, ApiError> {
    let trash = trash_root(st)?;
    let mut out: Vec<Value> = vec![];
    for id in trash.names().map_err(io_error)? {
        let Some(o) = origin_of(&trash, &id) else { continue };
        let Some(stored) = stored_name(&o) else {
            tracing::warn!("files: trash item {id} has a note that names no single file: left out");
            continue;
        };
        let item = trash.child(&id).map_err(io_error)?;
        let size = o["size"].as_u64().or_else(|| size_of(&item, &stored)).unwrap_or(0);
        let name = o["name"].as_str().map(String::from).unwrap_or_else(|| o["path"].as_str().unwrap_or("").rsplit('/').next().unwrap_or("").to_string());
        out.push(json!({ "id": id, "name": name, "root": o["root"], "path": o["path"], "at": o["at"], "by": o["by"], "size": size,
                         "kind": if o["kind"] == "dir" { "dir" } else { "file" } }));
    }
    let key = |v: &Value| -> (i64, i64) {
        let id = v["id"].as_str().unwrap_or("0-0");
        let (a, b) = id.split_once('-').unwrap_or((id, "0"));
        (a.parse().unwrap_or(0), b.parse().unwrap_or(0))
    };
    out.sort_by_key(|v| std::cmp::Reverse(key(v)));
    Ok(json!(out))
}

/// `POST /api/files/restore {items}` → `{restored, failed: [{id, reason}]}`.
pub fn restore(st: &impl Ctx, who: &Who, roots: &[Root], ids: &[String]) -> Result<Value, ApiError> {
    let _g = crate::lock();
    let trash = trash_root(st)?;
    let dirs = LibraryDirs::of(st.cfg());
    let (mut restored, mut failed) = (0, vec![]);
    for id in ids {
        match restore_one(st, who, roots, &trash, &dirs, id) {
            Ok(()) => restored += 1,
            Err(e) => failed.push(json!({ "id": id, "reason": e.detail })),
        }
    }
    Ok(json!({ "restored": restored, "failed": failed }))
}

fn restore_one(st: &impl Ctx, who: &Who, roots: &[Root], trash: &Dir, dirs: &LibraryDirs, id: &str) -> Result<(), ApiError> {
    let o = origin_of(trash, id).ok_or_else(|| err(404, "That isn't in the trash"))?;
    // the note names the stored item by one plain name, never a path out of its folder
    let stored = stored_name(&o).ok_or_else(|| err(409, "Its note in the trash isn't one Encore wrote: it is left as it is"))?;
    let root = roots.iter().find(|r| Some(r.id.as_str()) == o["root"].as_str() && r.kind != roots::RootKind::Trash)
        .ok_or_else(|| err(409, "Its folder is no longer one Encore may use"))?;
    let t = Target::new(root, o["path"].as_str().unwrap_or(""))?;
    let Some((name, up)) = t.parts.split_last() else { return Err(err(409, "Its folder is no longer one Encore may use")) };
    // the folders it was in, made again where they are missing (each one checked, none through a link)
    let mut parent = Dir::open(root.dir()?).map_err(io_error)?;
    for part in up {
        match parent.lstat(part).map_err(io_error)? {
            None => parent.mkdir(part).map_err(io_error)?,
            Some(s) if s.kind == Kind::Link => return Err(err(403, names::THROUGH_LINK)),
            Some(s) if s.kind != Kind::Dir => return Err(err(409, format!("“{part}” is a file now, not a folder"))),
            Some(_) => {}
        }
        parent = parent.child(part).map_err(io_error)?;
    }
    let item = trash.child(id).map_err(io_error)?;
    let is_dir = item.lstat(&stored).map_err(io_error)?.ok_or_else(|| err(404, "Its file is missing from the trash"))?.kind == Kind::Dir;
    let back = names::free_name(name, is_dir, |n| parent.lstat(n).ok().flatten().is_some());
    crate::ops::move_entry(&item, &stored, &parent, &back)?;
    // the files that went with it (its lyrics) come back beside it, each under a free name
    for x in o["extras"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let (Some(n), Some(kept)) = (x["name"].as_str(), x["stored"].as_str()) else { continue };
        if names::check_name(n).is_err() || names::check_name(kept).is_err() || kept == ORIGIN || kept == stored {
            continue;
        }
        if item.lstat(kept).ok().flatten().is_none() {
            tracing::info!("files: {kept} is missing from trash item {id}: not restored");
            continue;
        }
        let to = names::free_name(n, false, |x| parent.lstat(x).ok().flatten().is_some());
        if let Err(e) = crate::ops::move_entry(&item, kept, &parent, &to) {
            tracing::warn!("files: {n} stays in trash item {id}: {}", e.detail);
        }
    }
    if let Err(e) = remove_item(trash, id) {
        tracing::warn!("files: the restored item's folder stays in the trash: {e}");
    }
    let real = parent.path.join(&back);
    let mut change = Change::default();
    change.arrived(dirs, &real);
    if let Err(e) = change.apply(st) {
        tracing::warn!("files: reading in restored songs: {e:#}");
    }
    if let (Some(base), Some(saved)) = (dirs.db_path(&real), o["tracks"].as_array()) {
        if !saved.is_empty() {
            rows::give_back(st, &base, saved);
        }
    }
    let mut at = t.clone();
    *at.parts.last_mut().unwrap() = back;
    crate::audit(st, who, "restored from trash", &at, None);
    Ok(())
}

/// `DELETE /api/files/trash/{id}`: the item deleted for good.
pub fn delete(st: &impl Ctx, who: &Who, id: &str) -> Result<(), ApiError> {
    let _g = crate::lock();
    let trash = trash_root(st)?;
    let o = origin_of(&trash, id).ok_or_else(|| err(404, "That isn't in the trash"))?;
    remove_item(&trash, id).map_err(not_all)?;
    crate::audit_text(st, who, &format!("deleted forever {}:{} (from the trash)", o["root"].as_str().unwrap_or(""), o["path"].as_str().unwrap_or("")));
    Ok(())
}

/// `POST /api/files/empty-trash`: every item deleted for good (the report flow's parked files stay).
pub fn empty(st: &impl Ctx, who: &Who) -> Result<usize, ApiError> {
    let _g = crate::lock();
    let trash = trash_root(st)?;
    let (mut n, mut left) = (0, vec![]);
    for id in trash.names().map_err(io_error)? {
        if origin_of(&trash, &id).is_none() {
            continue;
        }
        // one item that can't be removed doesn't stop the others; it stays listed with its note
        match remove_item(&trash, &id) {
            Ok(()) => n += 1,
            Err(e) => {
                tracing::warn!("files: emptying the trash: {e}");
                left.push(not_all(e));
            }
        }
    }
    crate::audit_text(st, who, &format!("emptied the trash ({n} items deleted forever)"));
    match left.first() {
        None => Ok(n),
        Some(e) => Err(err(e.status.as_u16(), format!("{} of the items couldn't be deleted whole and stay in the trash. {}", left.len(), e.detail))),
    }
}

/// The trash folder (for tests and the roots list).
pub fn dir_of(st: &impl Ctx) -> PathBuf {
    roots::trash_dir(st.data_dir())
}
