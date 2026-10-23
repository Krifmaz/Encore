//! Folders held open while an operation works in them: every step below a root is opened with
//! `openat(…, O_NOFOLLOW | O_DIRECTORY)`, and files are made, renamed and removed relative to those handles
//! (`mkdirat`, `renameat2`, `unlinkat`, `openat(O_CREAT | O_EXCL | O_NOFOLLOW)`). A folder that is swapped for
//! a link after it was checked can't redirect the operation: the handle still points at the folder that was
//! checked, and a link in the way fails the open (ELOOP / ENOTDIR) instead of being followed.
//!
//! Every operation goes through the fault seam of [`encore_core::fsx`] (tests make one fail with a chosen
//! errno, EXDEV for a move across disks) and fails with a [`StepError`] that names the operation, the path and
//! the errno.
use encore_core::fsx::{self, Step, StepError};
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// An open folder and the path it was reached by (for messages, and for the library's rows).
pub struct Dir {
    /// the folder's descriptor (O_RDONLY | O_DIRECTORY)
    fd: OwnedFd,
    /// the path it was reached by
    pub path: PathBuf,
}

impl std::fmt::Debug for Dir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Dir({})", self.path.display())
    }
}

/// What `lstat` says about a name in a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// a folder
    Dir,
    /// a regular file
    File,
    /// a symbolic link
    Link,
    /// a socket, a device, a pipe
    Other,
}

/// What lstat says about an entry.
#[derive(Clone, Copy, Debug)]
pub struct Stat {
    /// what it is
    pub kind: Kind,
    /// its size in bytes
    pub size: u64,
    /// its modification time, Unix seconds
    pub mtime: i64,
    /// the device it is on
    pub dev: u64,
    /// its inode
    pub ino: u64,
}

/// A C string of `name`; EINVAL when it holds a NUL.
fn cstr(name: &str) -> io::Result<CString> {
    CString::new(name).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

/// A libc return value as a Result: negative means errno.
fn check(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// The parts of a libc stat we use.
fn stat_of(st: &libc::stat) -> Stat {
    let kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFDIR => Kind::Dir,
        libc::S_IFREG => Kind::File,
        libc::S_IFLNK => Kind::Link,
        _ => Kind::Other,
    };
    Stat { kind, size: st.st_size as u64, mtime: st.st_mtime, dev: st.st_dev, ino: st.st_ino }
}

/// The injected-fault check (test builds) for a file operation on `path`.
fn fault(op: &'static str, path: &Path) -> Result<(), StepError> {
    fsx::fault_point(Step::Files, op, path)
}

/// A closure that wraps an io error into a [`StepError`] for `op` on `path` (and `to`).
fn step<'a>(op: &'static str, path: &'a Path, to: Option<&'a Path>) -> impl FnOnce(io::Error) -> StepError + 'a {
    move |e| StepError::new(Step::Files, op, path, to, e)
}

impl Dir {
    /// Opens a folder by its path (a root: links in the root's own path are followed, it is the folder the
    /// settings name).
    pub fn open(path: &Path) -> Result<Dir, StepError> {
        fault("open", path)?;
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| StepError::new(Step::Files, "open", path, None, io::Error::from_raw_os_error(libc::EINVAL)))?;
        let fd = check(unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) }).map_err(step("open", path, None))?;
        Ok(Dir { fd: unsafe { OwnedFd::from_raw_fd(fd) }, path: path.to_path_buf() })
    }

    /// The folder `name` inside this one, never through a link: a link fails with ELOOP, a file with ENOTDIR.
    pub fn child(&self, name: &str) -> Result<Dir, StepError> {
        let path = self.path.join(name);
        fault("open", &path)?;
        let c = cstr(name).map_err(step("open", &path, None))?;
        let fd = check(unsafe { libc::openat(self.fd.as_raw_fd(), c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) })
            .map_err(step("open", &path, None))?;
        Ok(Dir { fd: unsafe { OwnedFd::from_raw_fd(fd) }, path })
    }

    /// `lstat` of a name in this folder (a link is a link). None when nothing has that name.
    pub fn lstat(&self, name: &str) -> Result<Option<Stat>, StepError> {
        let path = self.path.join(name);
        fault("stat", &path)?;
        let c = cstr(name).map_err(step("stat", &path, None))?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        match check(unsafe { libc::fstatat(self.fd.as_raw_fd(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) }) {
            Ok(_) => Ok(Some(stat_of(&st))),
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => Ok(None),
            Err(e) => Err(StepError::new(Step::Files, "stat", &path, None, e)),
        }
    }

    /// `fstat` of the folder itself.
    pub fn stat(&self) -> Result<Stat, StepError> {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        check(unsafe { libc::fstat(self.fd.as_raw_fd(), &mut st) }).map_err(step("stat", &self.path, None))?;
        Ok(stat_of(&st))
    }

    /// Makes the folder `name` here (EEXIST when the name is taken).
    pub fn mkdir(&self, name: &str) -> Result<(), StepError> {
        let path = self.path.join(name);
        fault("mkdir", &path)?;
        let c = cstr(name).map_err(step("mkdir", &path, None))?;
        check(unsafe { libc::mkdirat(self.fd.as_raw_fd(), c.as_ptr(), 0o777) }).map_err(step("mkdir", &path, None))?;
        Ok(())
    }

    /// A new file `name` here, open for writing; never an existing file and never through a link.
    pub fn create_new(&self, name: &str) -> Result<File, StepError> {
        let path = self.path.join(name);
        fault("open", &path)?;
        let c = cstr(name).map_err(step("open", &path, None))?;
        let fd = check(unsafe {
            libc::openat(self.fd.as_raw_fd(), c.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o666 as libc::c_uint)
        }).map_err(step("open", &path, None))?;
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// The file `name` here, open for reading (never through a link).
    pub fn open_file(&self, name: &str) -> Result<File, StepError> {
        let path = self.path.join(name);
        fault("open", &path)?;
        let c = cstr(name).map_err(step("open", &path, None))?;
        let fd = check(unsafe { libc::openat(self.fd.as_raw_fd(), c.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC) })
            .map_err(step("open", &path, None))?;
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// Renames `name` here to `to_name` in `to`, never over something that is there (RENAME_NOREPLACE; on a
    /// file system without it, a look first). EEXIST when the new name is taken, EXDEV across disks.
    pub fn rename_noreplace(&self, name: &str, to: &Dir, to_name: &str) -> Result<(), StepError> {
        let (from_p, to_p) = (self.path.join(name), to.path.join(to_name));
        fault("rename", &from_p)?;
        fault("rename", &to_p)?;
        let e = |e: io::Error| StepError::new(Step::Files, "rename", &from_p, Some(&to_p), e);
        let (a, b) = (cstr(name).map_err(e)?, cstr(to_name).map_err(e)?);
        let r = unsafe { libc::renameat2(self.fd.as_raw_fd(), a.as_ptr(), to.fd.as_raw_fd(), b.as_ptr(), libc::RENAME_NOREPLACE) };
        match check(r) {
            Ok(_) => Ok(()),
            // the file system can't do RENAME_NOREPLACE (some network and FUSE mounts): look, then rename
            Err(err) if matches!(err.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP)) => {
                if to.lstat(to_name)?.is_some() {
                    return Err(e(io::Error::from_raw_os_error(libc::EEXIST)));
                }
                check(unsafe { libc::renameat(self.fd.as_raw_fd(), a.as_ptr(), to.fd.as_raw_fd(), b.as_ptr()) }).map(|_| ()).map_err(e)
            }
            Err(err) => Err(e(err)),
        }
    }

    /// Renames `name` to `to_name` in the same folder over what is there (a change of case on a disk that
    /// doesn't tell case apart: both names are the same file).
    pub fn rename_over(&self, name: &str, to_name: &str) -> Result<(), StepError> {
        let (from_p, to_p) = (self.path.join(name), self.path.join(to_name));
        fault("rename", &from_p)?;
        let e = |e: io::Error| StepError::new(Step::Files, "rename", &from_p, Some(&to_p), e);
        let (a, b) = (cstr(name).map_err(e)?, cstr(to_name).map_err(e)?);
        check(unsafe { libc::renameat(self.fd.as_raw_fd(), a.as_ptr(), self.fd.as_raw_fd(), b.as_ptr()) }).map(|_| ()).map_err(e)
    }

    /// Removes the file or link `name` here.
    pub fn unlink(&self, name: &str) -> Result<(), StepError> {
        let path = self.path.join(name);
        fault("remove", &path)?;
        let c = cstr(name).map_err(step("remove", &path, None))?;
        check(unsafe { libc::unlinkat(self.fd.as_raw_fd(), c.as_ptr(), 0) }).map(|_| ()).map_err(step("remove", &path, None))
    }

    /// Removes the empty folder `name` here.
    pub fn rmdir(&self, name: &str) -> Result<(), StepError> {
        let path = self.path.join(name);
        fault("remove", &path)?;
        let c = cstr(name).map_err(step("remove", &path, None))?;
        check(unsafe { libc::unlinkat(self.fd.as_raw_fd(), c.as_ptr(), libc::AT_REMOVEDIR) }).map(|_| ()).map_err(step("remove", &path, None))
    }

    /// Removes `name` here and, when it is a folder, everything in it; links are removed, never followed.
    pub fn remove_all(&self, name: &str) -> Result<(), StepError> {
        match self.lstat(name)? {
            None => Ok(()),
            Some(s) if s.kind == Kind::Dir => {
                let sub = self.child(name)?;
                for entry in sub.names()? {
                    sub.remove_all(&entry)?;
                }
                self.rmdir(name)
            }
            Some(_) => self.unlink(name),
        }
    }

    /// The names in this folder (without `.` and `..`), read through the handle. A name that isn't UTF-8
    /// can't be named by the API and is left out (see [`Dir::count`]).
    pub fn names(&self) -> Result<Vec<String>, StepError> {
        Ok(self.raw_names()?.into_iter().filter_map(|n| match n.into_string() {
            Ok(n) => Some(n),
            Err(_) => {
                tracing::debug!("files: a name in {} is not UTF-8", self.path.display());
                None
            }
        }).collect())
    }

    /// How many entries the folder holds (without `.` and `..`), names that aren't UTF-8 too.
    pub fn count(&self) -> Result<usize, StepError> {
        Ok(self.raw_names()?.len())
    }

    /// The entry names, without "." and "..", as the disk has them.
    fn raw_names(&self) -> Result<Vec<std::ffi::OsString>, StepError> {
        fault("read", &self.path)?;
        // a second handle for the listing: fdopendir takes over the descriptor it is given
        let fd = check(unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) }).map_err(step("read", &self.path, None))?;
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            let e = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(StepError::new(Step::Files, "read", &self.path, None, e));
        }
        unsafe { libc::rewinddir(dir) };
        let mut out = vec![];
        loop {
            let ent = unsafe { libc::readdir(dir) };
            if ent.is_null() {
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*ent).d_name.as_ptr()) };
            let name = std::ffi::OsStr::from_bytes(name.to_bytes());
            if name == "." || name == ".." {
                continue;
            }
            out.push(name.to_os_string());
        }
        unsafe { libc::closedir(dir) };
        Ok(out)
    }

    /// The text of the link `name` here.
    pub fn readlink(&self, name: &str) -> Result<PathBuf, StepError> {
        let path = self.path.join(name);
        let c = cstr(name).map_err(step("read", &path, None))?;
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        let n = unsafe { libc::readlinkat(self.fd.as_raw_fd(), c.as_ptr(), buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if n < 0 {
            return Err(StepError::new(Step::Files, "read", &path, None, io::Error::last_os_error()));
        }
        buf.truncate(n as usize);
        Ok(PathBuf::from(std::ffi::OsString::from(std::ffi::OsStr::from_bytes(&buf))))
    }

    /// A link `name` here pointing at `target` (copies of links across disks).
    pub fn symlink(&self, target: &Path, name: &str) -> Result<(), StepError> {
        let path = self.path.join(name);
        fault("link", &path)?;
        let c = cstr(name).map_err(step("link", &path, None))?;
        let t = CString::new(target.as_os_str().as_bytes()).map_err(|_| StepError::new(Step::Files, "link", &path, None, io::Error::from_raw_os_error(libc::EINVAL)))?;
        check(unsafe { libc::symlinkat(t.as_ptr(), self.fd.as_raw_fd(), c.as_ptr()) }).map(|_| ()).map_err(step("link", &path, None))
    }

    /// The file `name` here, open for reading and writing in place (never through a link, never made): what
    /// the tag writer works through, so the file it writes is the one that was checked.
    pub fn open_rw(&self, name: &str) -> Result<File, StepError> {
        let path = self.path.join(name);
        fault("open", &path)?;
        let c = cstr(name).map_err(step("open", &path, None))?;
        let fd = check(unsafe { libc::openat(self.fd.as_raw_fd(), c.as_ptr(), libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC) })
            .map_err(step("open", &path, None))?;
        let f = unsafe { File::from_raw_fd(fd) };
        let st = f.metadata().map_err(step("stat", &path, None))?;
        if !st.is_file() {
            return Err(StepError::new(Step::Files, "open", &path, None, io::Error::from_raw_os_error(libc::EISDIR)));
        }
        Ok(f)
    }

    /// Flushes the folder's own entries (names made, renamed or removed in it) to the disk.
    pub fn sync(&self) -> Result<(), StepError> {
        fault("fsync", &self.path)?;
        check(unsafe { libc::fsync(self.fd.as_raw_fd()) }).map(|_| ()).map_err(step("fsync", &self.path, None))
    }
}

/// What [`copy_tree`] copied: each entry with what it was when it was copied (its device, inode, size and
/// time), so the original is removed only as far as it was copied and hasn't changed since.
#[derive(Debug)]
pub struct Copied {
    /// the bytes copied
    pub bytes: u64,
    /// what was copied, entry by entry
    tree: Node,
}

/// One entry of a [`Copied`] tree.
#[derive(Debug)]
enum Node {
    /// a file or link and its (dev, ino, size, mtime) when it was copied
    Leaf(String, (u64, u64, u64, i64)),
    /// a folder and what was copied inside it
    Dir(String, Vec<Node>),
    /// not copied (a socket, a device, a pipe)
    Skipped(String),
}

impl Copied {
    /// Removes from `from` what was copied of its entry: a file or link only when it is still the one that
    /// was copied (the same file, size and time), a folder only once it is empty. Anything that appeared or
    /// changed meanwhile stays where it is. → the paths that stayed
    pub fn remove_source(&self, from: &Dir) -> Vec<PathBuf> {
        let mut left = vec![];
        remove_node(from, &self.tree, &mut left);
        left
    }
}

/// Remove what `node` copied from `from`, depth first; anything that stayed goes into `left`.
fn remove_node(from: &Dir, node: &Node, left: &mut Vec<PathBuf>) {
    match node {
        Node::Leaf(name, was) => match from.lstat(name) {
            Ok(Some(s)) if (s.dev, s.ino, s.size, s.mtime) == *was && s.kind != Kind::Dir => {
                if from.unlink(name).is_err() {
                    left.push(from.path.join(name));
                }
            }
            Ok(None) => {}
            _ => left.push(from.path.join(name)),
        },
        Node::Skipped(name) => left.push(from.path.join(name)),
        Node::Dir(name, kids) => {
            let Ok(sub) = from.child(name) else {
                left.push(from.path.join(name));
                return;
            };
            for k in kids {
                remove_node(&sub, k, left);
            }
            // something that wasn't copied is in it: it stays, and so does the folder
            if from.rmdir(name).is_err() {
                left.push(from.path.join(name));
            }
        }
    }
}

/// Copies `name` from `from` to `to_name` in `to` (a folder with everything in it; links as links, never
/// followed), each file flushed to the disk, and each folder made flushed once its entries are in. On an
/// error the copy made so far is removed again. → what was copied ([`Copied::remove_source`] removes just that)
pub fn copy_tree(from: &Dir, name: &str, to: &Dir, to_name: &str) -> Result<Copied, StepError> {
    let mut bytes = 0;
    let r = copy_inner(from, name, to, to_name, &mut bytes);
    match r {
        Ok(tree) => Ok(Copied { bytes, tree }),
        Err(e) => {
            let _ = to.remove_all(to_name);
            Err(e)
        }
    }
}

/// Copy one entry, recursively for a folder, adding the bytes copied to `bytes`.
fn copy_inner(from: &Dir, name: &str, to: &Dir, to_name: &str, bytes: &mut u64) -> Result<Node, StepError> {
    let st = from.lstat(name)?.ok_or_else(|| StepError::new(Step::Files, "copy", &from.path.join(name), Some(&to.path.join(to_name)), io::Error::from_raw_os_error(libc::ENOENT)))?;
    let was = (st.dev, st.ino, st.size, st.mtime);
    match st.kind {
        Kind::Dir => {
            to.mkdir(to_name)?;
            let (src, dst) = (from.child(name)?, to.child(to_name)?);
            let mut kids = vec![];
            for entry in src.names()? {
                kids.push(copy_inner(&src, &entry, &dst, &entry, bytes)?);
            }
            dst.sync()?;
            Ok(Node::Dir(name.to_string(), kids))
        }
        Kind::Link => {
            let target = from.readlink(name)?;
            to.symlink(&target, to_name)?;
            Ok(Node::Leaf(name.to_string(), was))
        }
        Kind::File => {
            let (src_p, dst_p) = (from.path.join(name), to.path.join(to_name));
            fault("copy", &src_p)?;
            fault("copy", &dst_p)?;
            let mut src = from.open_file(name)?;
            let mut dst = to.create_new(to_name)?;
            let n = io::copy(&mut src, &mut dst).map_err(|e| StepError::new(Step::Files, "copy", &src_p, Some(&dst_p), e))?;
            dst.sync_all().map_err(step("fsync", &dst_p, None))?;
            if let Ok(m) = src.metadata() {
                if let Ok(t) = m.modified() {
                    let _ = dst.set_modified(t);
                }
            }
            *bytes += n;
            // the size and time of the open file after the copy: a file still being written reads as changed
            let now = src.metadata().map(|m| (m.len(), std::os::unix::fs::MetadataExt::mtime(&m))).unwrap_or((u64::MAX, 0));
            Ok(Node::Leaf(name.to_string(), (st.dev, st.ino, now.0, now.1)))
        }
        // sockets, devices and pipes are not copied (and so never removed)
        Kind::Other => Ok(Node::Skipped(name.to_string())),
    }
}

/// Bytes of everything under `name` in `dir` (links count nothing); None when it can't be read.
pub fn size_of(dir: &Dir, name: &str) -> Option<u64> {
    let st = dir.lstat(name).ok()??;
    match st.kind {
        Kind::File => Some(st.size),
        Kind::Dir => {
            let sub = dir.child(name).ok()?;
            let mut n = 0;
            for e in sub.names().ok()? {
                n += size_of(&sub, &e).unwrap_or(0);
            }
            Some(n)
        }
        _ => Some(0),
    }
}

/// Free bytes for Encore on the disk of `path` (0 when it can't be told).
pub fn free_bytes(path: &Path) -> u64 {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return 0 };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return 0;
    }
    (s.f_bavail as u64).saturating_mul(s.f_frsize as u64)
}

/// May Encore write in the folder `path` (its permissions and a read-only mount, as `access(W_OK)` says)?
pub fn writable(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_never_follow_links() {
        let d = encore_core::tmp::dir("files-dirfd");
        let (inside, outside) = (d.join("root"), d.join("elsewhere"));
        std::fs::create_dir_all(inside.join("real")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, inside.join("out")).unwrap();
        std::fs::write(inside.join("file"), b"x").unwrap();
        let root = Dir::open(&inside).unwrap();
        assert!(root.child("real").is_ok());
        let e = root.child("out").unwrap_err();
        assert!(matches!(e.errno, Some(libc::ELOOP) | Some(libc::ENOTDIR)), "a link is not opened as a folder: {e}");
        assert_eq!(root.child("file").unwrap_err().errno, Some(libc::ENOTDIR));
        assert_eq!(root.lstat("out").unwrap().unwrap().kind, Kind::Link);
        assert!(root.lstat("nothing").unwrap().is_none());
        // a new file is never made through a link, and never over a file that is there
        std::os::unix::fs::symlink(outside.join("planted"), inside.join("trap")).unwrap();
        assert_eq!(root.create_new("trap").unwrap_err().errno, Some(libc::EEXIST));
        assert!(!outside.join("planted").exists());
        assert_eq!(root.create_new("file").unwrap_err().errno, Some(libc::EEXIST));
        // a rename never replaces
        assert_eq!(root.rename_noreplace("file", &root, "real").unwrap_err().errno, Some(libc::EEXIST));
        root.rename_noreplace("file", &root, "file2").unwrap();
        assert!(inside.join("file2").is_file());
        // removing a link removes the link, never what it points at
        std::fs::write(outside.join("keep"), b"k").unwrap();
        root.remove_all("out").unwrap();
        assert!(outside.join("keep").is_file() && !inside.join("out").exists());
        let mut names = root.names().unwrap();
        names.sort();
        assert_eq!(names, vec!["file2", "real", "trap"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn trees_are_copied_with_their_links_and_sizes() {
        let d = encore_core::tmp::dir("files-copy");
        std::fs::create_dir_all(d.join("a/sub")).unwrap();
        std::fs::write(d.join("a/one"), b"12345").unwrap();
        std::fs::write(d.join("a/sub/two"), b"123").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", d.join("a/link")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        let (a, b) = (Dir::open(&d).unwrap(), Dir::open(&d.join("b")).unwrap());
        assert_eq!(copy_tree(&a, "a", &b, "copy").unwrap().bytes, 8);
        assert_eq!(std::fs::read(d.join("b/copy/sub/two")).unwrap(), b"123");
        assert_eq!(std::fs::read_link(d.join("b/copy/link")).unwrap(), PathBuf::from("/etc/passwd"));
        assert_eq!(size_of(&a, "a"), Some(8));
        // a failure part way leaves no half copy
        encore_core::fsx::fault::fail(d.join("b/again/sub"), "copy", 5);
        let e = copy_tree(&a, "a", &b, "again").unwrap_err();
        assert_eq!(e.errno, Some(5));
        assert!(!d.join("b/again").exists());
        encore_core::fsx::fault::clear(d.join("b/again/sub"));
        assert!(free_bytes(&d) > 0 && writable(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A move across disks removes only what it copied: a file that appeared in the folder during the copy,
    /// or one that changed, stays (and so does its folder).
    #[test]
    fn only_what_was_copied_is_removed_from_the_source() {
        let d = encore_core::tmp::dir("files-copied");
        std::fs::create_dir_all(d.join("src/a/sub")).unwrap();
        std::fs::create_dir_all(d.join("dst")).unwrap();
        std::fs::write(d.join("src/a/one"), b"1").unwrap();
        std::fs::write(d.join("src/a/sub/two"), b"22").unwrap();
        std::fs::write(d.join("src/a/grows"), b"3").unwrap();
        let (src, dst) = (Dir::open(&d.join("src")).unwrap(), Dir::open(&d.join("dst")).unwrap());
        let copied = copy_tree(&src, "a", &dst, "a").unwrap();
        assert_eq!(copied.bytes, 4);
        // meanwhile a download client writes into the folder
        std::fs::write(d.join("src/a/sub/new"), b"arrived during the copy").unwrap();
        std::fs::write(d.join("src/a/grows"), b"333").unwrap();
        let left = copied.remove_source(&src);
        assert!(!d.join("src/a/one").exists(), "copied and unchanged: removed");
        assert_eq!(std::fs::read(d.join("src/a/sub/new")).unwrap(), b"arrived during the copy");
        assert_eq!(std::fs::read(d.join("src/a/grows")).unwrap(), b"333");
        assert!(!d.join("src/a/sub/two").exists());
        assert!(left.contains(&d.join("src/a/sub/new").parent().unwrap().to_path_buf()) && left.contains(&d.join("src/a/grows")), "{left:?}");
        // with nothing new, the whole tree goes
        let copied = copy_tree(&src, "a", &dst, "b").unwrap();
        std::fs::remove_file(d.join("src/a/sub/new")).unwrap();
        std::fs::remove_file(d.join("dst/b/sub/new")).unwrap();
        assert!(copied.remove_source(&src).iter().all(|p| !p.starts_with(d.join("src/a/sub"))));
        let _ = std::fs::remove_dir_all(&d);
    }
}
