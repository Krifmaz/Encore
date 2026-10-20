//! A client's path in a root, resolved and confined.
//!
//! [`confine`] is the rule every path goes through: absolute paths and `..` parts are refused before the disk
//! is touched (see [`crate::names`]), the parts are joined to the root, the deepest part that exists is
//! resolved (links followed) and must still lie in the root.
//!
//! Writes go further ([`Target::open_parent`], [`Target::open_dir`]): every folder on the way is opened with
//! `O_NOFOLLOW` from the root down, so a path that passes through a link – any part before the last, or the
//! folder written into – is refused with 403 "That folder is reached through a link …", and a folder swapped
//! for a link after the check can't send the write elsewhere. The last part may itself be a link: renaming,
//! moving or trashing it acts on the link, never on what it points at.
//!
//! Browsing ([`browse_dir`]) follows a link to a folder only when its target lies in one of the roots.
use crate::dirfd::{Dir, Kind};
use crate::names::{self, CONFIG_FOLDER, LINK_OUTSIDE, MISSING, OUTSIDE, THROUGH_LINK};
use crate::roots::{Root, RootKind};
use encore_core::error::{err, ApiError};
use encore_core::fsx::StepError;
use std::path::{Path, PathBuf};

/// The contract's `confine(root, rel)`: the resolved path of `rel` in `root`, which must lie in the root.
/// Parts that don't exist yet are joined as they are (a new name is checked on its own with
/// [`names::check_name`]).
pub fn confine(root: &Path, rel: &str) -> Result<PathBuf, ApiError> {
    let parts = names::parse_rel(rel)?;
    let base = std::fs::canonicalize(root).map_err(|_| err(404, format!("The folder {} isn't there", root.display())))?;
    let mut joined = base.clone();
    for p in &parts {
        joined.push(p);
    }
    // the deepest part that exists, resolved; the rest joined to it
    let mut existing = joined.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = vec![];
    let real = loop {
        if let Ok(r) = std::fs::canonicalize(existing) {
            break r;
        }
        match (existing.file_name(), existing.parent()) {
            (Some(n), Some(up)) => {
                rest.push(n);
                existing = up;
            }
            _ => return Err(err(403, OUTSIDE)),
        }
    };
    if !real.starts_with(&base) {
        return Err(err(403, OUTSIDE));
    }
    let mut out = real;
    for n in rest.into_iter().rev() {
        out.push(n);
    }
    Ok(out)
}

/// A path in a root, checked by [`names::parse_rel`].
#[derive(Clone, Debug)]
pub struct Target {
    pub root: Root,
    pub parts: Vec<String>,
}

/// An io failure of a file operation as the API answer: the operation, the path and the errno in the
/// detail ("rename /music/A to /music/B: Permission denied (os error 13) – Encore may not write there …").
pub fn io_error(e: StepError) -> ApiError {
    let status = match e.errno {
        Some(libc::ENOENT) => return err(404, MISSING),
        Some(libc::EEXIST) | Some(libc::ENOTEMPTY) => return err(409, names::TAKEN),
        Some(libc::ELOOP) => return err(403, THROUGH_LINK),
        Some(libc::EACCES) | Some(libc::EPERM) | Some(libc::EROFS) => 403,
        Some(libc::ENOSPC) | Some(libc::EDQUOT) => 507,
        _ => 500,
    };
    let mut text = e.what();
    if let Some(h) = e.hint() {
        text.push_str(" – ");
        text.push_str(h);
    }
    err(status, text)
}

impl Target {
    pub fn new(root: &Root, rel: &str) -> Result<Target, ApiError> {
        Ok(Target { root: root.clone(), parts: names::parse_rel(rel)? })
    }

    /// The path relative to the root, as the API gives it ("" for the root).
    pub fn rel(&self) -> String {
        names::join(&self.parts)
    }

    pub fn is_root(&self) -> bool {
        self.parts.is_empty()
    }

    /// The last part (the root's own folder name for the root).
    pub fn name(&self) -> String {
        self.parts.last().cloned().unwrap_or_else(|| self.root.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    }

    /// The resolved path (the root resolved, the parts joined: what a write reaches, links not followed).
    pub fn real(&self) -> Result<PathBuf, ApiError> {
        let mut p = self.root.dir()?.to_path_buf();
        for part in &self.parts {
            p.push(part);
        }
        Ok(p)
    }

    /// The same path in another place of the tree ("" + more parts).
    pub fn child(&self, name: &str) -> Target {
        let mut t = self.clone();
        t.parts.push(name.to_string());
        t
    }

    fn refuse_trash(&self) -> Result<(), ApiError> {
        if self.root.kind == RootKind::Trash {
            return Err(err(403, "Files go to the trash with Move to trash"));
        }
        Ok(())
    }

    /// Opens the folders of `parts` from the root down, none of them through a link, and none in Encore's own
    /// config folder (see [`Root::fence`]).
    fn walk(&self, parts: &[String]) -> Result<Dir, ApiError> {
        let mut d = Dir::open(self.root.dir()?).map_err(io_error)?;
        let fenced = |d: &Dir| if self.root.fenced_off(&d.path) { Err(err(403, CONFIG_FOLDER)) } else { Ok(()) };
        fenced(&d)?;
        for p in parts {
            d = match d.child(p) {
                Ok(c) => c,
                Err(e) => {
                    // why it failed: a link (the write is refused), a file, or nothing there
                    return Err(match d.lstat(p) {
                        Ok(Some(s)) if s.kind == Kind::Link => err(403, THROUGH_LINK),
                        Ok(Some(s)) if s.kind != Kind::Dir => err(409, format!("“{p}” is a file, not a folder")),
                        Ok(None) => err(404, MISSING),
                        _ => io_error(e),
                    });
                }
            };
            fenced(&d)?;
        }
        Ok(d)
    }

    /// The folder this path names, opened for writing into it (no part of it a link, the folder itself
    /// neither).
    pub fn open_dir(&self) -> Result<Dir, ApiError> {
        self.refuse_trash()?;
        self.walk(&self.parts)
    }

    /// The folder that holds the last part, opened for writing (no part before the last a link), and the
    /// last part's name. The root itself has no parent here: it can't be renamed, moved or trashed.
    pub fn open_parent(&self) -> Result<(Dir, String), ApiError> {
        self.refuse_trash()?;
        let Some((last, up)) = self.parts.split_last() else {
            return Err(err(400, "That is one of the folders Encore may use; it can't be changed itself"));
        };
        Ok((self.walk(up)?, last.clone()))
    }

    /// [`Target::open_parent`] for a path that must be there.
    pub fn open_existing(&self) -> Result<(Dir, String, crate::dirfd::Stat), ApiError> {
        let (d, name) = self.open_parent()?;
        let st = d.lstat(&name).map_err(io_error)?.ok_or_else(|| err(404, MISSING))?;
        Ok((d, name, st))
    }
}

/// The folder to list for a browse: links followed only to folders inside one of the roots. → (the resolved
/// folder, whether a link was followed on the way).
pub fn browse_dir(t: &Target, roots: &[Root]) -> Result<(PathBuf, bool), ApiError> {
    let base = t.root.dir()?.to_path_buf();
    let mut cur = base;
    let mut via_link = false;
    for part in &t.parts {
        let next = cur.join(part);
        let meta = std::fs::symlink_metadata(&next).map_err(|_| err(404, MISSING))?;
        cur = if meta.file_type().is_symlink() {
            let real = std::fs::canonicalize(&next).map_err(|_| err(404, "That link points at nothing"))?;
            if crate::roots::holding(roots, &real).is_none() {
                return Err(err(403, LINK_OUTSIDE));
            }
            via_link = true;
            real
        } else {
            next
        };
    }
    let meta = std::fs::metadata(&cur).map_err(|_| err(404, MISSING))?;
    if !meta.is_dir() {
        return Err(err(400, "That is a file, not a folder"));
    }
    // whatever happened on the way, the folder listed is in a root
    let real = std::fs::canonicalize(&cur).map_err(|_| err(404, MISSING))?;
    match crate::roots::holding(roots, &real) {
        None => return Err(err(403, OUTSIDE)),
        Some(r) if r.fenced_off(&real) || t.root.fenced_off(&real) => return Err(err(403, CONFIG_FOLDER)),
        Some(_) => {}
    }
    Ok((real, via_link))
}

/// The file a path names, a regular file reached without links, opened through its folder's handle: for
/// reading (any root), or for reading and writing in place (`write`; not in Trash). The tags are read and
/// written through this handle, so the file can't be swapped between the check and the write.
pub fn open_file(t: &Target, write: bool) -> Result<std::fs::File, ApiError> {
    let (d, name) = if write { t.open_parent()? } else { t.open_parent_read()? };
    match d.lstat(&name).map_err(io_error)? {
        None => Err(err(404, MISSING)),
        Some(s) if s.kind == Kind::Link => Err(err(403, THROUGH_LINK)),
        Some(s) if s.kind != Kind::File => Err(err(400, "That is a folder, not a file")),
        Some(_) if write => d.open_rw(&name).map_err(io_error),
        Some(_) => d.open_file(&name).map_err(io_error),
    }
}

impl Target {
    /// [`Target::open_parent`] without the trash check (reading a file in any root).
    fn open_parent_read(&self) -> Result<(Dir, String), ApiError> {
        let Some((last, up)) = self.parts.split_last() else { return Err(err(400, "That is a folder, not a file")) };
        Ok((self.walk(up)?, last.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roots::RootKind;

    fn root(dir: &Path) -> Root {
        Root::new("library", "Library", dir.to_path_buf(), RootKind::Library)
    }

    #[test]
    fn confine_keeps_every_path_in_its_root() {
        let d = encore_core::tmp::dir("files-confine");
        let (r, out) = (d.join("root"), d.join("outside"));
        std::fs::create_dir_all(r.join("A/B")).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(&out, r.join("escape")).unwrap();
        std::os::unix::fs::symlink(r.join("A"), r.join("inner")).unwrap();
        let rc = std::fs::canonicalize(&r).unwrap();
        assert_eq!(confine(&r, "A/B").unwrap(), rc.join("A/B"));
        assert_eq!(confine(&r, "A/B/new folder/x.flac").unwrap(), rc.join("A/B/new folder/x.flac"), "parts that aren't there yet are joined");
        assert_eq!(confine(&r, "inner/B").unwrap(), rc.join("A/B"), "a link that stays in the root is resolved");
        for bad in ["escape", "escape/x", "/etc", "../outside", "A/../../outside", "%2e%2e/outside".replace("%2e", ".").as_str(), "．．/outside"] {
            assert_eq!(confine(&r, bad).unwrap_err().status.as_u16(), 403, "{bad}");
        }
        assert_eq!(confine(&r, "A\0B").unwrap_err().status.as_u16(), 400);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn writes_never_pass_through_a_link() {
        let d = encore_core::tmp::dir("files-writes");
        let (r, out) = (d.join("root"), d.join("outside"));
        std::fs::create_dir_all(r.join("A")).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(&out, r.join("escape")).unwrap();
        std::os::unix::fs::symlink(r.join("A"), r.join("inner")).unwrap();
        let root = root(&r);
        for (path, status) in [("escape", 403), ("escape/x", 403), ("inner", 403), ("inner/x", 403), ("missing/x", 404)] {
            let t = Target::new(&root, path).unwrap();
            let got = t.open_dir().map(|_| ()).unwrap_err();
            assert_eq!(got.status.as_u16(), if path == "missing/x" { 404 } else { status }, "{path}: {}", got.detail);
        }
        assert_eq!(Target::new(&root, "inner/x").unwrap().open_parent().map(|_| ()).unwrap_err().detail, THROUGH_LINK);
        // the link itself can be acted on (renamed, moved, trashed): its parent is a real folder
        let (dir, name) = Target::new(&root, "escape").unwrap().open_parent().unwrap();
        assert_eq!((dir.path.clone(), name.as_str()), (std::fs::canonicalize(&r).unwrap(), "escape"));
        assert!(Target::new(&root, "").unwrap().open_parent().is_err(), "the root itself can't be moved");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn browsing_follows_links_only_into_the_roots() {
        let d = encore_core::tmp::dir("files-browse");
        let (r, other, out) = (d.join("root"), d.join("other"), d.join("outside"));
        for p in [r.join("A"), other.join("X"), out.clone()] {
            std::fs::create_dir_all(p).unwrap();
        }
        std::os::unix::fs::symlink(&out, r.join("escape")).unwrap();
        std::os::unix::fs::symlink(other.join("X"), r.join("to-other")).unwrap();
        let roots = vec![root(&r), Root::new("staging", "Staging", other.clone(), RootKind::Staging)];
        let (dir, via) = browse_dir(&Target::new(&roots[0], "to-other").unwrap(), &roots).unwrap();
        assert_eq!((dir, via), (std::fs::canonicalize(other.join("X")).unwrap(), true));
        let e = browse_dir(&Target::new(&roots[0], "escape").unwrap(), &roots).unwrap_err();
        assert_eq!((e.status.as_u16(), e.detail.as_str()), (403, LINK_OUTSIDE));
        assert!(!browse_dir(&Target::new(&roots[0], "A").unwrap(), &roots).unwrap().1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
