//! Make folder, rename and move (§4.4–4.6 of the client shapes). Each runs under the file manager's lock,
//! works through folder handles opened without following links ([`crate::dirfd`]), never replaces anything
//! that is there, and has the library's rows follow ([`crate::rows`]). A move to another disk copies (each
//! file flushed to the disk) under a temporary name, renames the copy into place, and only then removes the
//! original.
use crate::dirfd::{copy_tree, Dir, Kind};
use crate::names::{self, MISSING, TAKEN};
use crate::resolve::{io_error, Target};
use crate::roots::LibraryDirs;
use crate::rows::Change;
use crate::Who;
use encore_core::error::{err, ApiError};
use encore_library::Ctx;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// `POST /api/files/mkdir {root, path, name}` → `{path}`.
pub fn mkdir(st: &impl Ctx, who: &Who, at: &Target, name: &str) -> Result<Value, ApiError> {
    names::check_name(name)?;
    let _g = crate::lock();
    let dir = at.open_dir()?;
    dir.mkdir(name).map_err(io_error)?;
    let made = at.child(name);
    crate::audit(st, who, "made the folder", &made, None);
    Ok(json!({ "path": made.rel() }))
}

/// `POST /api/files/rename {root, path, name}` → `{path}`. A name that is taken is a 409 and nothing is
/// replaced; a change of case only, on a disk that doesn't tell case apart, is a rename.
pub fn rename(st: &impl Ctx, who: &Who, t: &Target, name: &str) -> Result<Value, ApiError> {
    names::check_name(name)?;
    let _g = crate::lock();
    let (dir, old, st_old) = t.open_existing()?;
    let before = LibraryDirs::of(st.cfg());
    let mut to = t.clone();
    *to.parts.last_mut().unwrap() = name.to_string();
    if old == name {
        return Ok(json!({ "path": to.rel() }));
    }
    match dir.lstat(name).map_err(io_error)? {
        // the same file under both names: only the case differs and the disk ignores case
        Some(s) if s.dev == st_old.dev && s.ino == st_old.ino => dir.rename_over(&old, name).map_err(io_error)?,
        Some(_) => return Err(err(409, TAKEN)),
        None => dir.rename_noreplace(&old, &dir, name).map_err(io_error)?,
    }
    let followed = follow_extra_roots(st, &dir.path.join(&old), &dir.path.join(name));
    let mut change = Change::default();
    change.moved(&before, &LibraryDirs::of(st.cfg()), &dir.path.join(&old), &dir.path.join(name));
    rows_follow(&mut change, &before, &dir.path.join(&old), followed);
    if let Err(e) = change.apply(st) {
        tracing::warn!("files: the library's rows after renaming {}: {e:#}", t.rel());
    }
    crate::audit(st, who, "renamed", t, Some(&to));
    Ok(json!({ "path": to.rel() }))
}

/// An extra library folder that was moved or renamed (itself or a folder above it) is named by its new path
/// in `library_extra_roots`. → the (old, new) folders that changed
fn follow_extra_roots(st: &impl Ctx, old: &Path, new: &Path) -> Vec<(PathBuf, PathBuf)> {
    let extra = crate::roots::extra_roots(st.cfg());
    if !extra.iter().any(|e| e.starts_with(old)) {
        return vec![];
    }
    let mut changed = vec![];
    let moved: Vec<PathBuf> = extra.iter().map(|e| match e.strip_prefix(old) {
        Ok(rest) => {
            let n = if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) };
            changed.push((e.clone(), n.clone()));
            n
        }
        Err(_) => e.clone(),
    }).collect();
    st.cfg().set_many(&json!({ "library_extra_roots": crate::roots::format_extra(&moved) }));
    changed
}

/// The rows of extra library folders inside a renamed or moved folder that isn't in the library itself (a
/// download folder holding a kept album): they take the folder's new path, in the same transaction as the
/// move. (When the folder is in the library, [`Change::moved`] already moves every row under it.)
fn rows_follow(change: &mut Change, before: &LibraryDirs, old: &Path, followed: Vec<(PathBuf, PathBuf)>) {
    if before.db_path(old).is_some() {
        return;
    }
    for (o, n) in followed {
        change.rows_moved(o, n);
    }
}

/// Moves `name` from `from` to `to` under the same name: a rename, or across disks a copy under a temporary
/// name, renamed into place, then the original removed. Nothing that is there is replaced.
pub fn move_entry(from: &Dir, name: &str, to: &Dir, to_name: &str) -> Result<(), ApiError> {
    match from.rename_noreplace(name, to, to_name) {
        Ok(()) => Ok(()),
        Err(e) if e.errno == Some(libc::EXDEV) => {
            if to.lstat(to_name).map_err(io_error)?.is_some() {
                return Err(err(409, TAKEN));
            }
            let part = names::free_name(&format!(".{to_name}.encore-part"), true, |n| to.lstat(n).ok().flatten().is_some());
            let copied = copy_tree(from, name, to, &part).map_err(io_error)?;
            if let Err(e) = to.rename_noreplace(&part, to, to_name) {
                let _ = to.remove_all(&part);
                return Err(io_error(e));
            }
            // the new name on the disk before the original goes; when that can't be told, the copy (ours, under
            // a name that was free) goes again and the original stays
            if let Err(e) = to.sync() {
                let _ = to.remove_all(to_name);
                return Err(io_error(e));
            }
            // the copy is on the disk under its name: now the original goes, only as far as it was copied (a
            // file that appeared or changed during the copy stays, and so does its folder); what can't be
            // removed only stays behind
            let left = copied.remove_source(from);
            if !left.is_empty() {
                tracing::warn!("files: after a move across disks {} item(s) stay in the old place (new or changed during the copy, or not removable): {}",
                    left.len(), left.iter().take(5).map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
            }
            Ok(())
        }
        Err(e) => Err(io_error(e)),
    }
}

/// `POST /api/files/move {root, from: [paths], to_root, to_path}` → `{moved, failed: [{path, reason}]}`.
pub fn move_items(st: &impl Ctx, who: &Who, from: &[Target], to: &Target) -> Result<Value, ApiError> {
    let _g = crate::lock();
    let dest = to.open_dir()?;
    let dest_real = to.real()?;
    let mut change = Change::default();
    let (mut moved, mut failed) = (0, vec![]);
    for t in from {
        let res = (|| -> Result<bool, ApiError> {
            let (dir, name, _) = t.open_existing()?;
            let real = dir.path.join(&name);
            // a folder can't go into itself or a folder below it
            if dest_real.starts_with(&real) {
                return Err(err(400, "A folder can't be moved into itself"));
            }
            if dir.path == dest.path {
                return Ok(false); // already there
            }
            if dest.lstat(&name).map_err(io_error)?.is_some() {
                return Err(err(409, TAKEN));
            }
            let before = LibraryDirs::of(st.cfg());
            move_entry(&dir, &name, &dest, &name)?;
            let followed = follow_extra_roots(st, &real, &dest.path.join(&name));
            change.moved(&before, &LibraryDirs::of(st.cfg()), &real, &dest.path.join(&name));
            rows_follow(&mut change, &before, &real, followed);
            crate::audit(st, who, "moved", t, Some(&to.child(&name)));
            Ok(true)
        })();
        match res {
            Ok(_) => moved += 1,
            Err(e) => failed.push(json!({ "path": t.rel(), "reason": e.detail })),
        }
    }
    if let Err(e) = change.apply(st) {
        tracing::warn!("files: the library's rows after a move: {e:#}");
    }
    Ok(json!({ "moved": moved, "failed": failed }))
}

/// What a folder holds, for a check before a write (a link to a folder is a link).
pub fn kind_of(dir: &Dir, name: &str) -> Result<Option<Kind>, ApiError> {
    Ok(dir.lstat(name).map_err(io_error)?.map(|s| s.kind))
}

/// 404 unless something has that name.
pub fn must_exist(dir: &Dir, name: &str) -> Result<Kind, ApiError> {
    kind_of(dir, name)?.ok_or_else(|| err(404, MISSING))
}
