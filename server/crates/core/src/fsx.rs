//! File and program steps that say what failed. Every file operation and program start of the downloads and
//! the import goes through these helpers, so an error names its step, the operation, the path and the errno
//! ("import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5) – storage problem, not
//! the source") instead of a bare "Input/output error (os error 5)".
//!
//! A storage error ([`is_storage`]: EIO, EROFS, ENOSPC, EDQUOT, ENOTCONN, ESTALE, EHOSTDOWN) on a folder Encore
//! downloads into or imports to ([`roots`]) is a problem of the disk, not of the source: the request worker
//! waits for the storage instead of trying other sources ([`StepError::is_storage`]).
//!
//! Tests, and a server built with the `fault-io` feature (never a release build), can make an operation on a
//! path fail with a chosen errno ([`fault`]), to see what Encore does when the disk behind /music fails.
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};

// The seam must never be in a release build. Note: a release-profile `cargo test` or `cargo bench` across the
// whole workspace turns `fault-io` on through encore-api's and encore-requests' dev-dependencies and stops
// here; run the benchmark for its own package (`cargo bench -p encore --bench perf`), as the workflows do.
#[cfg(all(feature = "fault-io", not(debug_assertions)))]
compile_error!("the fault-io feature is for test builds only, never a release build");

/// The step of a request a file operation belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// a source's download: its folder, its program
    Download,
    /// Encore's staging folder (`staging_path`)
    Stage,
    /// tagging the files and moving them into the library
    Import,
    /// a program's configuration (streamrip's config.toml)
    Config,
    /// starting a program
    Start,
    /// the admin file manager (encore-files): browse, upload, move, rename, trash
    Files,
}

impl Step {
    /// The step's name in the API ("download", "stage", "import", "config", "start").
    pub fn as_str(self) -> &'static str {
        match self {
            Step::Download => "download",
            Step::Stage => "stage",
            Step::Import => "import",
            Step::Config => "config",
            Step::Start => "start",
            Step::Files => "files",
        }
    }

    pub fn parse(s: &str) -> Option<Step> {
        [Step::Download, Step::Stage, Step::Import, Step::Config, Step::Start, Step::Files].into_iter().find(|x| x.as_str() == s)
    }

    /// The words that start the error ("import failed").
    fn failed(self) -> &'static str {
        match self {
            Step::Download => "download failed",
            Step::Stage => "staging failed",
            Step::Import => "import failed",
            Step::Config => "config failed",
            Step::Start => "start failed",
            Step::Files => "file operation failed",
        }
    }
}

/// The errnos that mean the storage failed, not the program or the file: EIO 5, EROFS 30, ENOSPC 28, EDQUOT
/// 122, ENOTCONN 107, ESTALE 116, EHOSTDOWN 112 (a disk error, a file system remounted read-only, a full disk
/// or quota, a network or FUSE mount that went away).
pub const STORAGE_ERRNOS: [i32; 7] = [5, 30, 28, 122, 107, 116, 112];

/// Is `errno` a storage error ([`STORAGE_ERRNOS`])?
pub fn is_storage(errno: i32) -> bool {
    STORAGE_ERRNOS.contains(&errno)
}

/// A file operation or program start that failed, with its step, operation, path (and the other path of a
/// rename or copy) and errno. Its text is the whole story; it has no `source()` of its own, so `{:#}` of an
/// error chain holding it doesn't say the io error twice.
#[derive(Debug)]
pub struct StepError {
    pub step: Step,
    /// "mkdir", "write", "rename", "copy", "link", "remove", "open", "read", "stat", "start", "fsync", "set times"
    pub op: &'static str,
    pub path: PathBuf,
    /// where a rename, copy or link went
    pub to: Option<PathBuf>,
    pub errno: Option<i32>,
    pub source: io::Error,
    /// a copy or link that failed because its source could not be read (see [`copy`]): the source is the
    /// failing side, not the destination
    pub source_failed: bool,
}

impl StepError {
    pub fn new(step: Step, op: &'static str, path: &Path, to: Option<&Path>, source: io::Error) -> StepError {
        StepError { step, op, path: path.to_path_buf(), to: to.map(Path::to_path_buf), errno: source.raw_os_error(), source, source_failed: false }
    }

    /// The operation, its path and the error: "mkdir /music/Duumu: Input/output error (os error 5)".
    pub fn what(&self) -> String {
        match &self.to {
            Some(to) => format!("{} {} to {}: {}", self.op, self.path.display(), to.display(), self.source),
            None => format!("{} {}: {}", self.op, self.path.display(), self.source),
        }
    }

    /// The text without the advice: "import failed: mkdir /music/Duumu: Input/output error (os error 5)".
    pub fn short(&self) -> String {
        format!("{}: {}", self.step.failed(), self.what())
    }

    /// A storage error ([`is_storage`]) on a folder Encore downloads into or imports to ([`roots`]): the
    /// disk's problem, not the source's.
    pub fn is_storage(&self) -> bool {
        self.errno.is_some_and(is_storage) && (roots::covers(&self.path) || self.to.as_deref().is_some_and(roots::covers))
    }

    /// The path that failed, as far as it can be told: the destination of a rename, copy or link when it lies
    /// in a folder Encore uses – the side that is written (the library side of a move from staging or a
    /// download client's folder, both of them Encore's folders) – unless the source was found unreadable;
    /// else the path.
    pub fn failing_path(&self) -> &Path {
        match &self.to {
            Some(to) if roots::covers(to) && !(self.source_failed && roots::covers(&self.path)) => to,
            _ => &self.path,
        }
    }

    /// What to do about it, when that can be told from the errno.
    pub fn hint(&self) -> Option<&'static str> {
        let errno = self.errno?;
        if self.is_storage() {
            return Some("storage problem, not the source");
        }
        match errno {
            13 | 1 if self.op == "start" => Some("Encore may not run it (check the file's mode and owner, or a noexec mount)"),
            13 | 1 if matches!(self.op, "read" | "open") => Some("Encore may not read there (check the folder's owner, PUID/PGID)"),
            13 | 1 => Some("Encore may not write there (check the folder's owner, PUID/PGID)"),
            2 if self.op == "start" => Some("the program is missing"),
            2 if roots::is_root(&self.path) || roots::root_of(&self.path).is_some_and(|r| !r.exists()) => Some("the folder is missing (check the volume mapping)"),
            _ => None,
        }
    }

    /// The error for the API: `{step, op, path, to, errno, error, storage, text}`.
    pub fn json(&self) -> Value {
        json!({ "step": self.step.as_str(), "op": self.op, "path": self.path.to_string_lossy(), "to": self.to.as_ref().map(|t| t.to_string_lossy()),
                "errno": self.errno, "error": self.source.to_string(), "storage": self.is_storage(), "text": self.to_string() })
    }
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.short())?;
        if let Some(h) = self.hint() {
            write!(f, " – {h}")?;
        }
        Ok(())
    }
}

impl std::error::Error for StepError {}

/// The step error in an error's chain (directly, or under a context).
pub fn find(e: &anyhow::Error) -> Option<&StepError> {
    e.chain().find_map(|c| c.downcast_ref::<StepError>())
}

/// The storage error in an error's chain ([`StepError::is_storage`]).
pub fn storage(e: &anyhow::Error) -> Option<&StepError> {
    find(e).filter(|s| s.is_storage())
}

/// An error in full, every context of its chain: what a request's message and the log show.
pub fn explain(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

/// `map_err` for an operation these helpers don't wrap: `.map_err(fsx::at(Step::Import, "set times", &p))`.
pub fn at<'a>(step: Step, op: &'static str, path: &'a Path) -> impl FnOnce(io::Error) -> StepError + 'a {
    move |e| StepError::new(step, op, path, None, e)
}

/// The fault seam's check for an operation these helpers don't wrap (an async program start, say): Err when
/// a test made `op` on `path` fail. Nothing in a normal build.
pub fn fault_point(step: Step, op: &'static str, path: &Path) -> Result<(), StepError> {
    match fault::check(op, path, None) {
        Some(e) => Err(StepError::new(step, op, path, None, e)),
        None => Ok(()),
    }
}

fn run<T>(step: Step, op: &'static str, path: &Path, to: Option<&Path>, f: impl FnOnce() -> io::Result<T>) -> Result<T, StepError> {
    if let Some(e) = fault::check(op, path, to) {
        return Err(StepError::new(step, op, path, to, e));
    }
    f().map_err(|e| StepError::new(step, op, path, to, e))
}

/// `create_dir_all` ("mkdir"). The error names the folder asked for. A folder that is there already is not
/// made again (no operation, so no fault either).
pub fn mkdir_all(path: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    let p = path.as_ref();
    if p.is_dir() {
        return Ok(());
    }
    run(step, "mkdir", p, None, || std::fs::create_dir_all(p))
}

/// `create_dir` ("mkdir").
pub fn mkdir(path: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    let p = path.as_ref();
    run(step, "mkdir", p, None, || std::fs::create_dir(p))
}

/// `fs::write` ("write").
pub fn write(path: impl AsRef<Path>, data: impl AsRef<[u8]>, step: Step) -> Result<(), StepError> {
    let p = path.as_ref();
    run(step, "write", p, None, || std::fs::write(p, data))
}

/// `fs::read` ("read").
pub fn read(path: impl AsRef<Path>, step: Step) -> Result<Vec<u8>, StepError> {
    let p = path.as_ref();
    run(step, "read", p, None, || std::fs::read(p))
}

/// `fs::read_to_string` ("read").
pub fn read_to_string(path: impl AsRef<Path>, step: Step) -> Result<String, StepError> {
    let p = path.as_ref();
    run(step, "read", p, None, || std::fs::read_to_string(p))
}

/// `fs::read_dir` ("read").
pub fn read_dir(path: impl AsRef<Path>, step: Step) -> Result<std::fs::ReadDir, StepError> {
    let p = path.as_ref();
    run(step, "read", p, None, || std::fs::read_dir(p))
}

/// `fs::metadata` ("stat"). A file that is gone is `ErrorKind::NotFound`; any other error (a network mount
/// that went away, a disk error) says nothing about whether it is there.
pub fn metadata(path: impl AsRef<Path>, step: Step) -> Result<std::fs::Metadata, StepError> {
    let p = path.as_ref();
    run(step, "stat", p, None, || std::fs::metadata(p))
}

/// Read the first block of a file ("read"): a disk or mount that can't be read shows here, before the file
/// is judged by what it holds.
pub fn read_head(path: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    use std::io::Read;
    let p = path.as_ref();
    run(step, "read", p, None, || std::fs::File::open(p).and_then(|mut f| f.read(&mut [0u8; 4096])).map(|_| ()))
}

/// `fs::rename` ("rename from to").
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    let (f, t) = (from.as_ref(), to.as_ref());
    run(step, "rename", f, Some(t), || std::fs::rename(f, t))
}

/// `fs::copy` ("copy from to"). When it fails with an error either side could have caused (EIO, a mount
/// that went away), the source is read once: only when that fails too is the source the failing side.
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>, step: Step) -> Result<u64, StepError> {
    let (f, t) = (from.as_ref(), to.as_ref());
    run(step, "copy", f, Some(t), || std::fs::copy(f, t)).map_err(blame_source)
}

/// `fs::hard_link` ("link from to"), the failing side told as for [`copy`].
pub fn hard_link(from: impl AsRef<Path>, to: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    let (f, t) = (from.as_ref(), to.as_ref());
    run(step, "link", f, Some(t), || std::fs::hard_link(f, t)).map_err(blame_source)
}

/// A failed copy or link: a full, read-only or over-quota disk is the destination's; for any other error
/// the source is read once, and is the failing side only when that fails too.
fn blame_source(mut e: StepError) -> StepError {
    if !matches!(e.errno, Some(28 | 30 | 122)) {
        use std::io::Read;
        e.source_failed = fault::check("read", &e.path, None).is_some()
            || std::fs::File::open(&e.path).and_then(|mut f| f.read(&mut [0u8; 1])).is_err();
    }
    e
}

/// `fs::remove_file` ("remove").
pub fn remove_file(path: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    let p = path.as_ref();
    run(step, "remove", p, None, || std::fs::remove_file(p))
}

/// `fs::remove_dir` ("remove").
pub fn remove_dir(path: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    let p = path.as_ref();
    run(step, "remove", p, None, || std::fs::remove_dir(p))
}

/// `fs::remove_dir_all` ("remove").
pub fn remove_dir_all(path: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    let p = path.as_ref();
    run(step, "remove", p, None, || std::fs::remove_dir_all(p))
}

/// `File::open` ("open").
pub fn open(path: impl AsRef<Path>, step: Step) -> Result<std::fs::File, StepError> {
    let p = path.as_ref();
    run(step, "open", p, None, || std::fs::File::open(p))
}

/// `File::create` ("open").
pub fn create(path: impl AsRef<Path>, step: Step) -> Result<std::fs::File, StepError> {
    let p = path.as_ref();
    run(step, "open", p, None, || std::fs::File::create(p))
}

/// Flush a written file to the disk ("fsync"): before its source is removed, the copy is really there.
pub fn fsync(path: impl AsRef<Path>, step: Step) -> Result<(), StepError> {
    let p = path.as_ref();
    run(step, "fsync", p, None, || std::fs::OpenOptions::new().write(true).open(p)?.sync_all())
}

/// Start a program ("start <program>"): `fsx::spawn(Step::Download, &bin, || cmd.spawn())`. For an async
/// start (`output().await`) use [`fault_point`] before it and [`at`] on its error.
pub fn spawn<T>(step: Step, program: impl AsRef<Path>, f: impl FnOnce() -> io::Result<T>) -> Result<T, StepError> {
    run(step, "start", program.as_ref(), None, f)
}

static PROBES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Can Encore write in `dir`? Creates the folder `.encore-probe-<pid>-<n>` in it, writes 4 KiB into a file
/// there and flushes it to the disk (a full disk or one that fails on writes shows, not only one that can't
/// make folders), and removes both again – also when a step failed. The error names the operation that
/// failed ("mkdir /music/.encore-probe-12-3: Input/output error (os error 5)").
pub fn probe(dir: &Path, step: Step) -> Result<(), StepError> {
    let n = PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = dir.join(format!(".encore-probe-{}-{n}", std::process::id()));
    mkdir(&p, step)?;
    let f = p.join("probe");
    let wrote = write(&f, [0u8; 4096], step).and_then(|_| fsync(&f, step));
    let _ = std::fs::remove_file(&f);
    let removed = remove_dir(&p, step);
    if wrote.is_err() {
        let _ = std::fs::remove_dir(&p);
    }
    wrote.and(removed)
}

/// The deepest folder of `path` that exists (`path` itself when it does; "/" at worst). With mergerfs the
/// library's root can be fine while the disk holding an artist's folder is not: the probe goes there.
pub fn existing_ancestor(path: &Path) -> PathBuf {
    let mut p = path;
    loop {
        if p.is_dir() {
            return p.to_path_buf();
        }
        match p.parent() {
            Some(up) if !up.as_os_str().is_empty() => p = up,
            _ => return PathBuf::from("/"),
        }
    }
}

/// The folders Encore downloads into or imports to: the library, the staging folder and the download
/// clients' folders. A storage error on a path under one of them is a storage problem, not the source's.
/// The server registers them from its settings; a folder once registered stays (a folder a setting pointed
/// at a minute ago is still Encore's).
pub mod roots {
    use std::path::{Path, PathBuf};
    use std::sync::RwLock;

    static ROOTS: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());

    /// Register folders (empty and relative paths are left out).
    pub fn add<P: AsRef<Path>>(paths: impl IntoIterator<Item = P>) {
        let new: Vec<PathBuf> = paths.into_iter().map(|p| p.as_ref().to_path_buf()).filter(|p| p.is_absolute() && p.parent().is_some()).collect();
        if new.iter().all(|p| ROOTS.read().unwrap().contains(p)) {
            return;
        }
        let mut r = ROOTS.write().unwrap();
        for p in new {
            if !r.contains(&p) {
                r.push(p);
            }
        }
    }

    /// The registered folder `path` lies in (the deepest one).
    pub fn root_of(path: &Path) -> Option<PathBuf> {
        ROOTS.read().unwrap().iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.components().count()).cloned()
    }

    /// Does `path` lie in a registered folder?
    pub fn covers(path: &Path) -> bool {
        root_of(path).is_some()
    }

    /// Is `path` a registered folder itself?
    pub fn is_root(path: &Path) -> bool {
        ROOTS.read().unwrap().iter().any(|r| r == path)
    }

    /// Every registered folder.
    pub fn all() -> Vec<PathBuf> {
        ROOTS.read().unwrap().clone()
    }
}

/// The fault seam: make an operation on a path fail with a chosen errno, so tests can see what Encore does
/// when the disk behind /music returns EIO ("simulate EIO on the move into /music"). It exists only in test
/// builds and with the `fault-io` feature (the integrator's live run), never in a release build. A fault
/// matches an operation whose path, or the other path of a rename/copy/link, lies under its prefix; its op is
/// one of the helpers' ops ("mkdir", "copy", …) or "*" for every op. With `fault-io` the server also reads
/// `ENCORE_FAULT_IO` at start ("/music:mkdir:5,/music:copy:5").
#[cfg(any(test, feature = "fault-io"))]
pub mod fault {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, Once};

    static FAULTS: Mutex<Vec<(PathBuf, String, i32)>> = Mutex::new(Vec::new());
    static ENV: Once = Once::new();

    fn from_env() {
        ENV.call_once(|| {
            let Ok(v) = std::env::var("ENCORE_FAULT_IO") else { return };
            for item in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let mut parts = item.rsplitn(3, ':');
                let (Some(errno), Some(op), Some(prefix)) = (parts.next(), parts.next(), parts.next()) else { continue };
                if let Ok(n) = errno.parse() {
                    FAULTS.lock().unwrap().push((PathBuf::from(prefix), op.to_string(), n));
                }
            }
        });
    }

    /// From now on `op` ("*" for any) on a path under `prefix` fails with `errno`.
    pub fn fail(prefix: impl AsRef<Path>, op: &str, errno: i32) {
        from_env();
        FAULTS.lock().unwrap().push((prefix.as_ref().to_path_buf(), op.to_string(), errno));
    }

    /// The faults under exactly this prefix are gone.
    pub fn clear(prefix: impl AsRef<Path>) {
        from_env();
        FAULTS.lock().unwrap().retain(|(p, _, _)| p != prefix.as_ref());
    }

    /// Every fault is gone.
    pub fn clear_all() {
        from_env();
        FAULTS.lock().unwrap().clear();
    }

    /// The faults set now: (prefix, op, errno).
    pub fn list() -> Vec<(PathBuf, String, i32)> {
        from_env();
        FAULTS.lock().unwrap().clone()
    }

    pub(crate) fn check(op: &str, path: &Path, to: Option<&Path>) -> Option<io::Error> {
        from_env();
        let faults = FAULTS.lock().unwrap();
        faults.iter().find(|(prefix, o, _)| (o == "*" || o == op) && (path.starts_with(prefix) || to.is_some_and(|t| t.starts_with(prefix))))
            .map(|(_, _, errno)| io::Error::from_raw_os_error(*errno))
    }
}

#[cfg(not(any(test, feature = "fault-io")))]
mod fault {
    #[inline(always)]
    pub(crate) fn check(_op: &str, _path: &std::path::Path, _to: Option<&std::path::Path>) -> Option<std::io::Error> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("encore-fsx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn errors_name_the_step_the_operation_the_path_and_the_errno() {
        let d = tmp("words");
        let music = d.join("music");
        std::fs::create_dir_all(&music).unwrap();
        roots::add([&music]);
        let album = music.join("Duumu/Illuminate EP");
        fault::fail(&music, "mkdir", 5);
        let e = mkdir_all(&album, Step::Import).unwrap_err();
        assert_eq!(e.errno, Some(5));
        assert!(e.is_storage());
        assert_eq!(e.to_string(), format!("import failed: mkdir {}: Input/output error (os error 5) – storage problem, not the source", album.display()));
        assert_eq!(e.json()["op"], "mkdir");
        assert_eq!(e.json()["step"], "import");
        assert_eq!(e.json()["storage"], true);
        // through anyhow, with a context: the whole chain, the io error said once
        let a = anyhow::Error::from(e).context("streamrip");
        assert_eq!(explain(&a), format!("streamrip: import failed: mkdir {}: Input/output error (os error 5) – storage problem, not the source", album.display()));
        assert!(storage(&a).is_some());
        fault::clear(&music);
        mkdir_all(&album, Step::Import).unwrap();
        // the same errno outside Encore's folders is not a storage problem of a download
        let elsewhere = d.join("config");
        fault::fail(&elsewhere, "write", 5);
        let e = write(elsewhere.join("config.toml"), "x", Step::Config).unwrap_err();
        assert!(!e.is_storage());
        assert_eq!(e.to_string(), format!("config failed: write {}: Input/output error (os error 5)", elsewhere.join("config.toml").display()));
        fault::clear(&elsewhere);
        // a rename into the library names both paths; the failing side is the library's
        fault::fail(&music, "rename", 30);
        let src = d.join("a.flac");
        std::fs::write(&src, b"x").unwrap();
        let e = rename(&src, music.join("a.flac"), Step::Import).unwrap_err();
        assert!(e.is_storage(), "{e}");
        assert!(e.to_string().starts_with(&format!("import failed: rename {} to {}: Read-only file system (os error 30)", src.display(), music.join("a.flac").display())), "{e}");
        assert_eq!(e.failing_path(), music.join("a.flac"));
        fault::clear(&music);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_failing_side_of_a_copy_between_two_of_encores_folders() {
        let d = tmp("sides");
        let (staging, music) = (d.join("staging"), d.join("music"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&music).unwrap();
        roots::add([&staging, &music]);
        let src = staging.join("a.flac");
        std::fs::write(&src, b"x").unwrap();
        let dest = music.join("a.flac.encore-part");
        // the library's disk fails: the library side is named, though both are Encore's folders
        fault::fail(&dest, "copy", 5);
        let e = copy(&src, &dest, Step::Import).unwrap_err();
        assert!(e.is_storage() && !e.source_failed);
        assert_eq!(e.failing_path(), dest);
        // the staging disk can't be read either: the source is the failing side
        fault::fail(&src, "read", 5);
        let e = copy(&src, &dest, Step::Import).unwrap_err();
        assert!(e.source_failed);
        assert_eq!(e.failing_path(), src);
        // a full disk is always the destination's
        fault::clear(&dest);
        fault::fail(&dest, "link", 28);
        assert_eq!(hard_link(&src, &dest, Step::Import).unwrap_err().failing_path(), dest);
        fault::clear(&dest);
        fault::clear(&src);
        // a file that can't be looked at is not a file that is gone
        fault::fail(&src, "stat", 107);
        let e = metadata(&src, Step::Import).unwrap_err();
        assert_eq!((e.op, e.errno, e.source.kind() == io::ErrorKind::NotFound), ("stat", Some(107), false));
        fault::clear(&src);
        assert_eq!(metadata(staging.join("gone.flac"), Step::Import).unwrap_err().source.kind(), io::ErrorKind::NotFound);
        read_head(&src, Step::Import).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hints_for_permissions_and_missing_folders() {
        let d = tmp("hints");
        let lib = d.join("lib");
        roots::add([&lib]);
        fault::fail(&lib, "mkdir", 13);
        let e = mkdir_all(lib.join("A"), Step::Import).unwrap_err();
        assert!(e.to_string().ends_with("Permission denied (os error 13) – Encore may not write there (check the folder's owner, PUID/PGID)"), "{e}");
        assert!(!e.is_storage());
        fault::clear(&lib);
        // the library folder itself is missing (not mounted)
        let e = read_dir(&lib, Step::Import).unwrap_err();
        assert!(e.to_string().ends_with("– the folder is missing (check the volume mapping)"), "{e}");
        let e = spawn(Step::Download, d.join("no-such-program"), || std::process::Command::new(d.join("no-such-program")).output()).unwrap_err();
        assert!(e.to_string().starts_with(&format!("download failed: start {}: No such file or directory (os error 2)", d.join("no-such-program").display())), "{e}");
        // a program Encore may not run (no x bit, a noexec mount) says so, not "may not write there"
        let e = spawn(Step::Download, "/opt/tools/bin/rip", || Err::<(), _>(io::Error::from_raw_os_error(13))).unwrap_err();
        assert_eq!(e.to_string(), "download failed: start /opt/tools/bin/rip: Permission denied (os error 13) – Encore may not run it (check the file's mode and owner, or a noexec mount)");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn probes_and_ancestors() {
        let d = tmp("probe");
        probe(&d, Step::Import).unwrap();
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "the probe folder is removed");
        // a disk that makes folders but fails on writes (full, bad blocks) fails the probe too, and the
        // probe leaves nothing behind
        fault::fail(&d, "write", 28);
        let e = probe(&d, Step::Import).unwrap_err();
        assert_eq!((e.op, e.errno), ("write", Some(28)));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        fault::clear(&d);
        assert_eq!(existing_ancestor(&d.join("Artist/Album")), d);
        fault::fail(&d, "mkdir", 107);
        let e = probe(&d, Step::Import).unwrap_err();
        assert!(e.what().starts_with(&format!("mkdir {}/.encore-probe-", d.display())), "{e}");
        assert_eq!(e.errno, Some(107));
        fault::clear(&d);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn storage_errnos() {
        for n in [5, 30, 28, 122, 107, 116, 112] {
            assert!(is_storage(n));
        }
        for n in [1, 2, 13, 17, 20, 21] {
            assert!(!is_storage(n));
        }
        assert_eq!(Step::parse("import"), Some(Step::Import));
    }
}
