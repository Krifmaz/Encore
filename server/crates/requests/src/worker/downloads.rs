//! The download folders (backlog item 17, the read-only disk of Oct 17: yt-dlp failed every request with
//! "Read-only file system (os error 30)"). Every folder a download is saved in – the staging folder (yt-dlp,
//! streamrip and links), Soulseek's downloads folder, the torrent and Usenet clients' folders as Encore sees
//! them and the blackhole folders – is probed in the background every minute while requests are queued,
//! downloading or waiting or a folder fails, right before a source starts when the last probe is older than
//! a minute, and when an admin presses Check again ([`tick`], [`hold`]): a file `.encore-probe-<pid>` is
//! created, 4 KiB are written and flushed to the disk, and it is removed. While nothing downloads the folders
//! are only looked at, every 10 minutes, without writing ([`look_dir`]: there, not read-only, not full, Encore
//! may write), so idle disks can spin down. What the probe found is told apart: read-only (EROFS), permission
//! denied (EACCES), missing (ENOENT), full (ENOSPC, EDQUOT), or another error (EIO, a mount that went away). A
//! folder a pass has waited on for longer than [`HUNG_SECS`] (a network mount that stopped answering) counts
//! as failing until the pass ends; Check again and a starting source don't wait for such a pass.
//!
//! A request whose source saves into a folder that fails is neither started nor failed: it waits (status
//! `waiting` with `detail.wait.reason` [`REASON`] – the waiting status and reasons of [`super::storage`] with
//! one reason more), "Waiting for the downloads folder – /downloads/encore is read-only", its source first in
//! its sources again and no attempt counted. It goes on by itself at the queue's next look once a probe finds
//! the folder writable again. Admins see one banner on every page (`downloads` in `GET
//! /api/requests/storage`) and each folder with its result and the time of its probe in Diagnostics
//! (`download_folders` in `GET /api/system/diagnostics`). One log line when a folder starts or stops holding
//! up the downloads, in the activity log and on stdout.
//!
//! Encore's own folders (the staging and Soulseek folders, the blackhole watch folders) must be writable; one
//! that isn't there yet is made. A download client's own folder (qBittorrent, Transmission, Deluge, rTorrent,
//! SABnzbd, NZBGet, a blackhole's finished folder) is written by the client, which may run as another user or
//! in another container: Encore only reads it (an import copies what it may not move). There a permission
//! that is denied while the folder can still be read, or a read-only mount over a file system that is
//! writable (a `:ro` volume), is noted in Diagnostics and holds nothing up. SABnzbd and NZBGet are never told
//! a folder: theirs is only where Encore looks for what they report when it can't find their path as it is,
//! so it may well not exist in Encore's container – that holds nothing up either.
//!
//! The indexers' releases go to the torrent or the Usenet client: an indexer request waits before it starts
//! only when neither client's folder can be written, and a release is not handed to a client whose folder
//! fails – the next release goes to the other client, or the request waits for that folder
//! ([`failing_client`], [`park_release`]).
use super::*;
use crate::sources::dl::Client;
use encore_core::fsx::{self, Step, StepError};
use std::sync::atomic::{AtomicBool, Ordering};

/// The reason a request waits for a downloads folder (`detail.wait.reason`, `wait_reason` in the API).
pub const REASON: &str = "waiting_disk";
/// How often the folders are probed while requests are active or a folder fails, and how old a probe may be
/// when a source starts (seconds).
pub const PROBE_SECS: f64 = 60.0;
/// How often they are looked at, without writing, while nothing downloads (seconds).
pub const IDLE_SECS: f64 = 600.0;
/// A pass that has waited this long on one folder (seconds): that folder doesn't answer, and counts as failing
/// until the pass ends.
pub const HUNG_SECS: f64 = 30.0;
/// How long Check again and a starting source wait for a pass (seconds); the last answer stands after that.
const WAIT_SECS: u64 = 10;
/// The start of a waiting request's message, and the words of its status in the clients.
pub const WAITING: &str = "Waiting for the downloads folder";
/// The start of Download now's answer when the request's folder still fails.
pub const STILL_FAILING: &str = "The downloads folder still can't be written";
/// What other users than admins read for such a request's message (no server folders).
pub const PLAIN_WAITING: &str = "Waiting for the downloads folder – a storage problem on the server; the download starts when it is fixed";
/// A download client's folder Encore may not write in but can read.
const NOTE_READS: &str = "Encore may not write here, which is fine: the download client writes here and Encore only reads its finished downloads";
/// A download client's folder mounted read-only in Encore's container over a writable file system.
const NOTE_RO_MOUNT: &str = "Mounted read-only in Encore's container, which is fine: the download client writes here and Encore only reads its finished downloads";
/// SABnzbd's or NZBGet's folder that isn't in Encore's container.
const NOTE_MAPPED: &str = "Not in Encore's container, which is fine when the Usenet client's own folder is mounted at the same path here: Encore reads the finished downloads where the client says they are";
/// What a folder that doesn't answer says (the error's words in the banner).
const NOT_ANSWERING: &str = "it doesn't answer";

/// What a probe of a folder found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    Ok,
    /// EROFS
    ReadOnly,
    /// EACCES, EPERM
    Denied,
    /// ENOENT, ENOTDIR
    Missing,
    /// ENOSPC, EDQUOT
    Full,
    /// any other error (EIO, ENOTCONN, ESTALE …)
    Failed,
}

impl Found {
    /// What a failed step says about its folder.
    pub fn of(e: &StepError) -> Found {
        match e.errno {
            Some(30) => Found::ReadOnly,
            Some(13 | 1) => Found::Denied,
            Some(2 | 20) => Found::Missing,
            Some(28 | 122) => Found::Full,
            _ => Found::Failed,
        }
    }

    /// The result's name in the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Found::Ok => "ok",
            Found::ReadOnly => "read_only",
            Found::Denied => "denied",
            Found::Missing => "missing",
            Found::Full => "full",
            Found::Failed => "error",
        }
    }

    /// The result in a word or two, for Diagnostics.
    pub fn words(self) -> &'static str {
        match self {
            Found::Ok => "Writable",
            Found::ReadOnly => "Read-only",
            Found::Denied => "Permission denied",
            Found::Missing => "Missing",
            Found::Full => "Full",
            Found::Failed => "Not writable",
        }
    }
}

/// A folder downloads are saved in.
#[derive(Clone, Debug)]
pub struct Root {
    /// the setting that names it
    pub key: &'static str,
    pub label: String,
    pub path: PathBuf,
    /// the request sources that save here ("ytdlp", "streamrip", "url" for links, [`PLUGINS`] for every plugin
    /// program's source, "slskd", "slskd_lossless", "torznab")
    pub sources: Vec<&'static str>,
    /// Encore writes here itself (the folder is made when it isn't there); false: a download client's folder
    pub own: bool,
    /// the indexers' releases saved here: "torrent", "usenet" (none for the other sources)
    pub protos: Vec<&'static str>,
    /// only where Encore looks for what the client reports (SABnzbd, NZBGet: never told a folder): missing is
    /// fine
    pub mapped: bool,
}

/// A folder and what its last probe found.
#[derive(Clone, Debug)]
pub struct Probed {
    pub root: Root,
    pub found: Found,
    /// the failed step (`{step, op, path, to, errno, error, storage, text}`), null when the folder is writable
    pub error: Value,
    /// the folder holds up the requests whose source saves in it
    pub blocks: bool,
    /// a remark on a result that holds nothing up
    pub note: Option<&'static str>,
    pub checked_at: f64,
    /// since when it holds them up (its first such probe in a row)
    pub since: Option<f64>,
    /// the folder's owner (uid) and mode, read by the pass for a denied permission (the banner must not stat:
    /// a network mount may not answer)
    pub owner: Option<(u32, u32)>,
}

impl Probed {
    fn io_error(&self) -> String {
        self.error["error"].as_str().unwrap_or("").to_string()
    }

    /// The banner's sentence for this folder (None when it holds nothing up).
    pub fn sentence(&self) -> Option<String> {
        self.blocks.then(|| sentence(self.found, &self.root.path, &self.io_error(), self.owner))
    }

    /// What a waiting request's message says about the folder: "/downloads/encore is read-only".
    pub fn short(&self) -> String {
        let p = self.root.path.display();
        match self.found {
            Found::Ok => format!("{p} can be written"),
            Found::ReadOnly => format!("{p} is read-only"),
            Found::Denied => format!("Encore may not write in {p}"),
            Found::Missing => format!("{p} is missing"),
            Found::Full => format!("{p} is full"),
            Found::Failed => format!("{p} can't be written ({})", self.io_error()),
        }
    }

    /// The folder for the API and Diagnostics.
    pub fn json(&self) -> Value {
        json!({ "key": self.root.key, "label": self.root.label, "path": self.root.path.to_string_lossy(), "sources": self.root.sources, "own": self.root.own,
                "state": self.found.as_str(), "result": self.found.words(), "ok": !self.blocks, "note": self.note, "error": self.error,
                "checked_at": self.checked_at, "since": self.since, "text": self.sentence() })
    }
}

/// A folder no probe has looked at yet, for the API.
fn unprobed(root: &Root) -> Value {
    json!({ "key": root.key, "label": root.label, "path": root.path.to_string_lossy(), "sources": root.sources, "own": root.own,
            "state": "unknown", "result": "Not checked yet", "ok": true, "note": null, "error": null, "checked_at": null, "since": null, "text": null })
}

/// Encore's own uid and gid (what the chown advice names).
fn ids() -> (u32, u32) {
    // SAFETY: both calls only read the process's credentials and cannot fail.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

/// The banner's sentence for a folder (or several, `path` their common folder) that holds up the downloads:
/// "The downloads folder /downloads is read-only – check the disk (dmesg, fsck) or the mount". A denied
/// permission on a folder Encore owns without write permission asks for that permission (a chown to Encore
/// would change nothing); `owner` is the folder's (uid, mode).
fn sentence(found: Found, path: &Path, io_error: &str, owner: Option<(u32, u32)>) -> String {
    let p = path.display();
    match found {
        Found::ReadOnly => format!("The downloads folder {p} is read-only – check the disk (dmesg, fsck) or the mount"),
        Found::Missing => format!("The downloads folder {p} is missing – check the mount"),
        Found::Full => format!("The downloads folder {p} is full – free some space on its disk"),
        Found::Denied => {
            let (uid, gid) = ids();
            if let Some((o, mode)) = owner {
                if o == uid && mode & 0o200 == 0 {
                    return format!("The downloads folder {p} isn't writable: Encore (uid {uid}) owns it, but it has no write permission – chmod u+w {p} on the host");
                }
            }
            format!("The downloads folder {p} isn't writable by Encore (uid {uid}, gid {gid}) – chown {uid}:{gid} {p} on the host, or set PUID/PGID to its owner")
        }
        Found::Failed | Found::Ok => format!("The downloads folder {p} can't be written ({io_error}) – check the disk (dmesg, fsck) or the mount"),
    }
}

/// The deepest folder every path lies in.
fn common_folder(paths: &[&Path]) -> Option<PathBuf> {
    let first = paths.first()?;
    let mut out = PathBuf::new();
    for (i, c) in first.components().enumerate() {
        if paths.iter().all(|p| p.components().nth(i) == Some(c)) {
            out.push(c);
        } else {
            break;
        }
    }
    Some(out)
}

/// The banner: one sentence per folder that holds up the downloads, or one for several of them that are
/// read-only or full when they are all the folders probed under one folder of their own ("The downloads
/// folder /downloads is read-only …" when /downloads/encore and /downloads/slskd both are: a property of the
/// file system they share). A denied permission, a missing folder or another error names each folder (a
/// chown of the folder above changes nothing in them). None while every folder can be written.
pub fn banner(probed: &[Probed]) -> Option<String> {
    let blocking: Vec<&Probed> = probed.iter().filter(|p| p.blocks).collect();
    if blocking.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = vec![];
    let mut said: Vec<&Path> = vec![];
    for p in &blocking {
        if said.contains(&p.root.path.as_path()) {
            continue;
        }
        let same: Vec<&Path> = blocking.iter().filter(|q| q.found == p.found && !said.contains(&q.root.path.as_path())).map(|q| q.root.path.as_path()).collect();
        let merges = matches!(p.found, Found::ReadOnly | Found::Full);
        let shared = (merges && same.len() > 1).then(|| common_folder(&same)).flatten().filter(|c| {
            c.components().count() > 1 && probed.iter().filter(|q| q.root.path.starts_with(c)).all(|q| q.blocks && q.found == p.found)
        });
        match shared {
            Some(c) => {
                parts.push(sentence(p.found, &c, &p.io_error(), None));
                said.extend(same);
            }
            None => {
                parts.push(sentence(p.found, &p.root.path, &p.io_error(), p.owner));
                said.push(&p.root.path);
            }
        }
    }
    Some(parts.join(". "))
}

// ---- the folders and their probe ------------------------------------------------------------------------

/// What the probe passes keep while the server runs (in [`super::storage::Health`]).
#[derive(Default)]
pub struct Roots {
    state: Mutex<State>,
    /// held while a pass runs: one at a time (two probes of a folder would write the same file)
    probing: Mutex<()>,
    /// the background pass is running (the worker's loop starts it and goes on)
    ticking: AtomicBool,
    #[cfg(any(test, feature = "test-support"))]
    fake: Mutex<Option<FakeProbe>>,
}

#[derive(Default)]
struct State {
    /// the last pass's folders, in the order of [`roots`]
    probed: Vec<Probed>,
    /// when the last pass ended (None: no pass yet)
    checked_at: Option<f64>,
    /// when the last pass that wrote in the folders ended (the idle look writes nothing)
    wrote_at: Option<f64>,
    /// the folder a pass is on now, and since when
    on: Option<(Root, f64)>,
}

impl State {
    /// The last pass's folders, with the folder a pass has waited on for [`HUNG_SECS`] or longer as failing:
    /// it doesn't answer (a network mount that went away – the probe waits in the kernel).
    fn current(&self) -> Vec<Probed> {
        let mut out = self.probed.clone();
        let Some((root, started)) = &self.on else { return out };
        if now() - started < HUNG_SECS {
            return out;
        }
        let since = out.iter().find(|p| p.root.path == root.path && p.blocks).and_then(|p| p.since).unwrap_or(*started);
        let step = if root.key == "staging_path" { Step::Stage } else { Step::Download };
        let path = root.path.to_string_lossy();
        let error = json!({ "step": step.as_str(), "op": "write", "path": path, "to": null, "errno": null, "error": NOT_ANSWERING, "storage": true,
                            "text": format!("{path} doesn't answer – check the mount") });
        let hung = Probed { root: root.clone(), found: Found::Failed, error, blocks: true, note: None, checked_at: *started, since: Some(since), owner: None };
        match out.iter_mut().find(|p| p.root.path == root.path) {
            Some(p) => *p = hung,
            None => out.push(hung),
        }
        out
    }
}

/// A stand-in for the probe of a folder in tests: what the probe would find there.
#[cfg(any(test, feature = "test-support"))]
pub type FakeProbe = Arc<dyn Fn(&Path) -> Result<(), StepError> + Send + Sync>;

/// Let `f` answer for the probe from now on (None: the real probe again). Tests only.
#[cfg(any(test, feature = "test-support"))]
pub fn fake_probe(st: &impl Ctx, f: Option<FakeProbe>) {
    *roots_of(st).fake.lock().unwrap_or_else(|p| p.into_inner()) = f;
}

fn roots_of(st: &impl Ctx) -> &Roots {
    &st.requests().storage.downloads
}

fn state(st: &impl Ctx) -> std::sync::MutexGuard<'_, State> {
    roots_of(st).state.lock().unwrap_or_else(|p| p.into_inner())
}

/// The folders downloads are saved in now, as the settings name them: the staging folder always (links use
/// it whatever is switched on), Soulseek's while Soulseek is on, and the chosen torrent and Usenet clients'
/// while the indexers are on. A folder two settings name is one folder with both sources.
pub fn roots(st: &impl Ctx) -> Vec<Root> {
    let mut out: Vec<Root> = vec![];
    let mut add = |key: &'static str, label: &str, path: PathBuf, sources: &[&'static str], own: bool, proto: Option<&'static str>, mapped: bool| {
        if path.as_os_str().is_empty() || !path.is_absolute() {
            return;
        }
        if let Some(r) = out.iter_mut().find(|r| r.path == path) {
            for s in sources {
                if !r.sources.contains(s) {
                    r.sources.push(s);
                }
            }
            if let Some(p) = proto.filter(|p| !r.protos.contains(p)) {
                r.protos.push(p);
            }
            r.own |= own;
            // a folder another setting saves in is more than a place to look
            r.mapped &= mapped;
            r.label = format!("{} · {label}", r.label);
            return;
        }
        out.push(Root { key, label: label.to_string(), path, sources: sources.to_vec(), own, protos: proto.into_iter().collect(), mapped });
    };
    let cfg = |k: &str| PathBuf::from(st.cfg().str(k).trim());
    // plugin programs download into the request's staging folder too (fetch.rs `start_plugin`:
    // request-N/plugin-<source>); their own folders under <config>/plugins are their install and work folders,
    // never a download's, so they are not probed here
    add("staging_path", "Staging folder (yt-dlp, streamrip, links, plugins)", cfg("staging_path"), &["ytdlp", "streamrip", "url", PLUGINS], true, None, false);
    if slskd::enabled(st) {
        add("slskd_download_path", "Soulseek downloads", crate::slskd_managed::download_dir(st), &["slskd", "slskd_lossless"], true, None, false);
    }
    if torznab::enabled(st) {
        for (c, what, proto) in [(Client::torrent(st), "Torrent", "torrent"), (Client::usenet(st), "Usenet", "usenet")] {
            if !c.ready(st) {
                continue;
            }
            let p = Some(proto);
            match c {
                Client::TorrentHole => {
                    add("torrent_blackhole_watch", "Torrent watch folder", cfg("torrent_blackhole_watch"), &["torznab"], true, p, false);
                    add(c.download_path_key(), "Finished torrents folder", cfg(c.download_path_key()), &["torznab"], false, p, false);
                }
                Client::NzbHole => {
                    add("nzb_blackhole_watch", "Usenet watch folder", cfg("nzb_blackhole_watch"), &["torznab"], true, p, false);
                    add(c.download_path_key(), "Finished Usenet downloads folder", cfg(c.download_path_key()), &["torznab"], false, p, false);
                }
                // SABnzbd and NZBGet are never told a folder: theirs is where Encore looks when their path isn't there as it is
                Client::Sab | Client::NzbGet => add(c.download_path_key(), &format!("{what} downloads ({})", c.name()), cfg(c.download_path_key()), &["torznab"], false, p, true),
                _ => add(c.download_path_key(), &format!("{what} downloads ({})", c.name()), cfg(c.download_path_key()), &["torznab"], false, p, false),
            }
        }
    }
    out
}

/// Probe a folder: it is there (Encore's own folder is made when it isn't), and a file `.encore-probe-<pid>`
/// in it is created, written (4 KiB), flushed to the disk and removed – also when a step failed. Every step
/// goes through [`fsx`], so its error names the operation and path (and tests can make one fail).
pub fn probe_dir(dir: &Path, step: Step, own: bool) -> Result<(), StepError> {
    match fsx::metadata(dir, step) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(StepError::new(step, "stat", dir, None, std::io::Error::from_raw_os_error(libc::ENOTDIR))),
        Err(e) if own && e.errno == Some(libc::ENOENT) => fsx::mkdir_all(dir, step)?,
        Err(e) => return Err(e),
    }
    let f = dir.join(format!(".encore-probe-{}", std::process::id()));
    match fsx::write(&f, [0u8; 4096], step).and_then(|_| fsx::fsync(&f, step)) {
        Ok(()) => fsx::remove_file(&f, step),
        Err(e) => {
            let _ = std::fs::remove_file(&f);
            Err(e)
        }
    }
}

/// Look at a folder without writing in it (the pass while nothing downloads, so a disk that sleeps stays
/// asleep): it is there – for Encore's own folder that isn't, the folder it would be made in –, Encore may
/// write in it (access(2), which also says EROFS for a read-only mount), and its file system is neither
/// mounted read-only nor full (statvfs). What the kernel keeps in memory answers these.
pub fn look_dir(dir: &Path, step: Step, own: bool) -> Result<(), StepError> {
    use std::os::unix::ffi::OsStrExt;
    let mut at = dir.to_path_buf();
    loop {
        match fsx::metadata(&at, step) {
            Ok(m) if m.is_dir() => break,
            Ok(_) => return Err(StepError::new(step, "stat", &at, None, std::io::Error::from_raw_os_error(libc::ENOTDIR))),
            Err(e) if own && e.errno == Some(libc::ENOENT) => match at.parent() {
                Some(p) if p != at && !p.as_os_str().is_empty() => at = p.to_path_buf(),
                _ => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
    fsx::fault_point(step, "write", &at.join(format!(".encore-probe-{}", std::process::id())))?;
    let fail = |errno: i32| Err(StepError::new(step, "write", &at, None, std::io::Error::from_raw_os_error(errno)));
    let Ok(c) = std::ffi::CString::new(at.as_os_str().as_bytes()) else { return Ok(()) };
    // SAFETY: `c` is a NUL-terminated path that outlives the call; access only reads it.
    if unsafe { libc::access(c.as_ptr(), libc::W_OK) } != 0 {
        return Err(StepError::new(step, "write", &at, None, std::io::Error::last_os_error()));
    }
    // SAFETY: an all-zero statvfs is a valid value for the call to fill in.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` as above, `s` a valid statvfs to write to.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0 {
        if s.f_flag & libc::ST_RDONLY != 0 {
            return fail(libc::EROFS);
        }
        // root may use the blocks kept back for it
        let free = if ids().0 == 0 { s.f_bfree } else { s.f_bavail };
        if s.f_blocks > 0 && free == 0 {
            return fail(libc::ENOSPC);
        }
    }
    Ok(())
}

/// Probe a folder (`write`), or look at it without writing ([`look_dir`]).
fn run_probe(st: &impl Ctx, root: &Root, write: bool) -> Result<(), StepError> {
    let step = if root.key == "staging_path" { Step::Stage } else { Step::Download };
    #[cfg(any(test, feature = "test-support"))]
    {
        let fake = roots_of(st).fake.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(f) = fake {
            return f(&root.path);
        }
    }
    let _ = st;
    if write { probe_dir(&root.path, step, root.own) } else { look_dir(&root.path, step, root.own) }
}

/// What a probe's answer means for a folder: (found, the error as JSON, whether it holds up its downloads, a
/// note). A download client's folder Encore can read holds nothing up for a denied permission or a read-only
/// mount of its own (see the module's words).
fn judge(root: &Root, res: Result<(), StepError>) -> (Found, Value, bool, Option<&'static str>) {
    let e = match res {
        Ok(()) => return (Found::Ok, Value::Null, false, None),
        Err(e) => e,
    };
    let found = Found::of(&e);
    let mut error = e.json();
    // the downloads folder is the server's storage, whatever the errno: other users read it as such
    error["storage"] = json!(true);
    if root.mapped && found == Found::Missing {
        return (found, error, false, Some(NOTE_MAPPED));
    }
    if !root.own {
        let readable = || fsx::read_dir(&root.path, Step::Download).is_ok();
        if found == Found::Denied && readable() {
            return (found, error, false, Some(NOTE_READS));
        }
        if found == Found::ReadOnly && read_only_mount(&root.path) && readable() {
            return (found, error, false, Some(NOTE_RO_MOUNT));
        }
    }
    (found, error, true, None)
}

/// Is `path` on a mount that is read-only while its file system is not (a `:ro` volume, a read-only bind
/// mount), rather than on a file system that is read-only itself (a disk the kernel remounted read-only
/// after errors)? Read from /proc/self/mountinfo; false when it can't be told.
fn read_only_mount(path: &Path) -> bool {
    std::fs::read_to_string("/proc/self/mountinfo").map(|t| ro_mount_in(&t, path)).unwrap_or(false)
}

/// [`read_only_mount`] on the text of a mountinfo file: the deepest mount `path` lies in (the last one
/// mounted at that point) has `ro` in its own options and not in its file system's (the last field).
fn ro_mount_in(mountinfo: &str, path: &Path) -> bool {
    let mut best: Option<(usize, bool, bool)> = None;
    for line in mountinfo.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(sep) = f.iter().position(|x| *x == "-") else { continue };
        if sep < 6 || f.len() < sep + 4 {
            continue;
        }
        let point = unescape(f[4]);
        let point = Path::new(&point);
        if !path.starts_with(point) {
            continue;
        }
        let depth = point.components().count();
        let ro = |opts: &str| opts.split(',').any(|o| o == "ro");
        if best.is_none_or(|(d, _, _)| depth >= d) {
            best = Some((depth, ro(f[5]), ro(f[sep + 3])));
        }
    }
    best.is_some_and(|(_, mount_ro, fs_ro)| mount_ro && !fs_ro)
}

/// A mount point as mountinfo writes it, with its octal escapes (`\040` for a space) undone.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let code = (b[i] == b'\\' && i + 4 <= b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)))
            .then(|| b[i + 1..i + 4].iter().fold(0u32, |n, c| n * 8 + u32::from(c - b'0')))
            .and_then(|n| u8::try_from(n).ok());
        if let Some(c) = code {
            out.push(c);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Requests waiting for `dir`.
fn waiting_in(st: &impl Ctx, dir: &Path) -> i64 {
    st.db().one("SELECT COUNT(*) n FROM requests WHERE status='waiting' AND json_extract(detail, '$.wait.reason')=? AND json_extract(detail, '$.wait.dir')=?",
        p![REASON, dir.to_string_lossy()]).ok().flatten().map(|r| r.i("n")).unwrap_or(0)
}

/// Requests waiting for a downloads folder.
fn waiting_count(st: &impl Ctx) -> i64 {
    st.db().one("SELECT COUNT(*) n FROM requests WHERE status='waiting' AND json_extract(detail, '$.wait.reason')=?", p![REASON])
        .ok().flatten().map(|r| r.i("n")).unwrap_or(0)
}

fn log(st: &impl Ctx, level: &str, line: &str) {
    if level == "warning" {
        tracing::warn!("{line}");
    } else {
        tracing::info!("{line}");
    }
    st.db().log(level, line);
}

/// Probe every folder now, waiting for a pass that runs (called on the blocking pool), keep what was found and
/// log what changed. → the folders
pub fn probe_all(st: &impl Ctx) -> Vec<Probed> {
    let _one = roots_of(st).probing.lock().unwrap_or_else(|p| p.into_inner());
    pass(st, true)
}

/// Check again: every folder probed now, unless a pass still runs after a few seconds (one that hangs on a
/// network mount that went away is not waited for). → whether this pass ran
pub fn probe_now(st: &impl Ctx) -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(WAIT_SECS / 2);
    loop {
        match roots_of(st).probing.try_lock() {
            Ok(_one) => {
                pass(st, true);
                return true;
            }
            Err(std::sync::TryLockError::Poisoned(p)) => {
                let _one = p.into_inner();
                pass(st, true);
                return true;
            }
            Err(std::sync::TryLockError::WouldBlock) if std::time::Instant::now() < until => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        }
    }
}

/// A pass when the last one that wrote is older than a minute (or there was none) and none runs now, else
/// nothing: a pass that runs (or hangs on a network mount that went away) is not waited for – the last pass's
/// answer stands until it ends.
fn probe_if_stale(st: &impl Ctx) {
    let _one = match roots_of(st).probing.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if stale(st) {
        pass(st, true);
    }
}

/// Download now on a request waiting for a downloads folder: a pass that writes now unless the last one is
/// younger than `max_age` seconds and found every folder writable (a folder that failed is probed again at
/// every press: it may just have been fixed), on the blocking pool – waiting for a pass that runs (the
/// background one) –, not waited for longer than [`WAIT_SECS`] (a network mount that doesn't answer).
/// → whether the folders' state is that fresh now
pub(super) async fn probe_soon(st: &impl Ctx, max_age: f64) -> bool {
    let fresh = move |s: &State| s.wrote_at.is_some_and(|t| now() - t < max_age) && !s.probed.iter().any(|p| p.blocks);
    if fresh(&state(st)) {
        return true;
    }
    let s = st.clone();
    let run = tokio::task::spawn_blocking(move || {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(WAIT_SECS);
        loop {
            let one = match roots_of(&s).probing.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
                Err(std::sync::TryLockError::WouldBlock) => return false,
            };
            // a pass that ran meanwhile counts
            if !fresh(&state(&s)) {
                pass(&s, true);
            }
            drop(one);
            return true;
        }
    });
    matches!(tokio::time::timeout(std::time::Duration::from_secs(WAIT_SECS), run).await, Ok(Ok(true)))
}

/// Is the last pass that wrote in the folders older than a minute (or was there none)?
fn stale(st: &impl Ctx) -> bool {
    state(st).wrote_at.is_none_or(|t| now() - t >= PROBE_SECS)
}

/// [`probe_all`] with the pass's lock held; `write`: the probe that writes a file, else the look of
/// [`look_dir`].
fn pass(st: &impl Ctx, write: bool) -> Vec<Probed> {
    super::storage::register_roots(st);
    let before = state(st).probed.clone();
    let mut out: Vec<Probed> = vec![];
    for root in roots(st) {
        state(st).on = Some((root.clone(), now()));
        let res = run_probe(st, &root, write);
        let t = now();
        let (found, error, blocks, note) = judge(&root, res);
        let since = blocks.then(|| before.iter().find(|b| b.root.path == root.path && b.blocks).and_then(|b| b.since).unwrap_or(t));
        // who owns a folder Encore may not write in, for the banner's advice (chmod or chown)
        let owner = (found == Found::Denied).then(|| {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&root.path).ok().map(|m| (m.uid(), m.mode()))
        }).flatten();
        out.push(Probed { root, found, error, blocks, note, checked_at: t, since, owner });
    }
    for p in &out {
        let was = before.iter().any(|b| b.root.path == p.root.path && b.blocks);
        match (was, p.blocks) {
            (false, true) => log(st, "warning", &format!("downloads folder: {} – requests that save there wait", p.short())),
            (true, false) => {
                let n = waiting_in(st, &p.root.path);
                log(st, "info", &format!("downloads folder: {} can be written again; {n} waiting request{} go{} on", p.root.path.display(),
                    if n == 1 { "" } else { "s" }, if n == 1 { "es" } else { "" }));
            }
            _ => {}
        }
    }
    for b in before.iter().filter(|b| b.blocks && !out.iter().any(|p| p.root.path == b.root.path)) {
        log(st, "info", &format!("downloads folder: {} is no longer used for downloads; requests waiting for it go on", b.root.path.display()));
    }
    let mut s = state(st);
    s.probed = out.clone();
    s.on = None;
    s.checked_at = Some(now());
    if write {
        s.wrote_at = s.checked_at;
    }
    out
}

/// Clears the running flag when the background pass ends (also when it panicked).
struct Ticking<C: Ctx>(C);

impl<C: Ctx> Drop for Ticking<C> {
    fn drop(&mut self) {
        roots_of(&self.0).ticking.store(false, Ordering::SeqCst);
    }
}

/// The worker's look every few seconds (`busy`: requests are active): while requests are active or a folder
/// fails, a pass that writes once a minute has passed since the last one; while nothing downloads, a look
/// without writing once 10 minutes have passed since the last pass (at start too). It runs on the blocking
/// pool; the worker's loop goes on without waiting for it (a hung network mount must not hold up the queue),
/// and no second one starts while it runs. → the pass started now, if any (tests wait for it).
pub fn tick<C: Ctx>(st: &C, busy: bool) -> Option<tokio::task::JoinHandle<()>> {
    let (due, write) = {
        let s = state(st);
        if busy || s.current().iter().any(|p| p.blocks) {
            (s.wrote_at.is_none_or(|t| now() - t >= PROBE_SECS), true)
        } else {
            (s.checked_at.is_none_or(|t| now() - t >= IDLE_SECS), false)
        }
    };
    if !due || roots_of(st).ticking.swap(true, Ordering::SeqCst) {
        return None;
    }
    let st = st.clone();
    Some(tokio::task::spawn_blocking(move || {
        let done = Ticking(st);
        let _one = roots_of(&done.0).probing.lock().unwrap_or_else(|p| p.into_inner());
        pass(&done.0, write);
    }))
}

/// The folders that hold up `source` now (from the last pass, a folder that doesn't answer included); None
/// before the first pass. The indexers' releases go to the torrent or the Usenet client: "torznab" waits
/// only while neither client's folder can be written (a release for a client whose folder fails waits in
/// [`failing_client`]).
pub fn failing_for(st: &impl Ctx, source: &str) -> Option<Vec<Probed>> {
    let source = saves_as(source);
    let s = state(st);
    s.checked_at?;
    let mine: Vec<Probed> = s.current().into_iter().filter(|p| p.root.sources.contains(&source)).collect();
    let failing: Vec<Probed> = mine.iter().filter(|p| p.blocks).cloned().collect();
    if source == "torznab" {
        let mut protos: Vec<&str> = mine.iter().flat_map(|p| p.root.protos.iter().copied()).collect();
        protos.sort_unstable();
        protos.dedup();
        if !protos.iter().all(|proto| failing.iter().any(|p| p.root.protos.contains(proto))) {
            return Some(vec![]);
        }
    }
    Some(failing)
}

/// A folder of the client the indexers' releases of `proto` ("torrent", "usenet") go to that can't be
/// written now (from the last pass), if any.
pub(super) fn failing_client(st: &impl Ctx, proto: &str) -> Option<Probed> {
    let s = state(st);
    s.checked_at?;
    s.current().into_iter().find(|p| p.blocks && p.root.protos.contains(&proto))
}

/// The sources of plugin programs (any source id that isn't one of Encore's own: `[[sources]]` of an
/// uploaded or URL plugin) save in the staging folder; this is their name among a folder's sources.
pub const PLUGINS: &str = "plugins";

/// The source a folder knows `source` by: Encore's own as they are, a plugin program's source as [`PLUGINS`].
fn saves_as(source: &str) -> &str {
    match source {
        "slskd" | "slskd_lossless" | "torznab" | "ytdlp" | "streamrip" | "url" => source,
        _ => PLUGINS,
    }
}

/// The source a picked download saves with: Soulseek, the indexers, streamrip for a Qobuz, Tidal or Deezer
/// pick or link, else yt-dlp (all but Soulseek and the indexers save in the staging folder).
pub(super) fn pick_source(src: &str, url: &str) -> &'static str {
    match src {
        "slskd" => "slskd",
        "torznab" => "torznab",
        "deezer" | "tidal" | "qobuz" => "streamrip",
        "link" if RIP_LINK.is_match(url) => "streamrip",
        _ => "ytdlp",
    }
}

/// Before a source starts (a request's next source, a hand pick, a link): when a folder it saves in can't
/// be written the request waits for it instead ([`park`]); `requeue`: the source goes back first in the
/// request's sources, to be tried when the folder is back. A probe older than a minute (after an idle
/// stretch) is made again first, on the blocking pool, unless a pass runs already – and not waited for
/// longer than a few seconds (a network mount that doesn't answer). → whether it waits
pub(super) async fn hold(st: &impl Ctx, r: &mut Req, src: &str, requeue: bool) -> anyhow::Result<bool> {
    if stale(st) {
        let s = st.clone();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(WAIT_SECS), tokio::task::spawn_blocking(move || probe_if_stale(&s))).await;
    }
    let failing = failing_for(st, src).unwrap_or_default();
    let Some(first) = failing.first() else { return Ok(false) };
    check(st, r.id)?;
    if requeue {
        requeue_source(r, src);
    }
    park(st, r, src, first, None);
    Ok(true)
}

/// `src` first in the request's sources again.
fn requeue_source(r: &mut Req, src: &str) {
    let mut srcs: Vec<Value> = r.d["sources"].as_array().cloned().unwrap_or_default();
    if srcs.first().and_then(|x| x.as_str()) != Some(src) {
        srcs.insert(0, json!(src));
    }
    r.d["sources"] = json!(srcs);
}

/// The indexers' releases left for a request all go to a client (`proto`) whose folder can't be written: the
/// request waits for that folder, and asks the indexers again once it can be written (a hand-picked release
/// is handed over again).
pub(super) fn park_release(st: &impl Ctx, r: &mut Req, proto: &str, p: &Probed) -> anyhow::Result<()> {
    check(st, r.id)?;
    if !r.d["pick"].is_object() {
        r.d["torznab"] = Value::Null;
        requeue_source(r, "torznab");
    }
    park(st, r, "torznab", p, Some(proto));
    Ok(())
}

/// The request waits for the folder: `waiting`, reason [`REASON`] with the source, the folder and the
/// probe's error (its details show them) – and for an indexer release the client's protocol (`proto`) –,
/// "Waiting for the downloads folder – /downloads/encore is read-only"; nothing else changes (no attempt is
/// counted, no other source tried). Writes only what changed.
fn park(st: &impl Ctx, r: &mut Req, src: &str, p: &Probed, proto: Option<&str>) {
    let t = now();
    let same = r.d["wait"]["reason"] == REASON;
    let since = if same { r.d["wait"]["since"].as_f64().unwrap_or(t) } else { t };
    let mut wait = json!({ "reason": REASON, "since": since, "next_try_at": null, "source": src, "dir": p.root.path.to_string_lossy(), "error": p.error });
    if let Some(proto) = proto {
        wait["proto"] = json!(proto);
    }
    let msg = format!("{WAITING} – {}", p.short());
    let news = !same || r.d["wait"]["dir"] != wait["dir"] || r.d["wait"]["error"]["errno"] != wait["error"]["errno"];
    if news {
        log_push(r, format!("{src}: {msg}"));
    }
    let changed = r.d["wait"] != wait;
    r.d["wait"] = wait;
    let mut fields = vec![("status", json!("waiting")), ("progress", json!(0)), ("message", json!(msg))];
    if src != "url" {
        fields.push(("source", json!(src)));
    }
    if changed || r.s("status") != "waiting" || r.s("message") != msg || (src != "url" && r.s("source") != src) {
        upd(st, r, &fields);
    }
}

/// A request waiting for a downloads folder, at the queue's look: it stays while the folder still fails
/// (its words follow the folder) – for an indexer release, while its client's folder does –, and goes on
/// once the last pass found it writable: the source it waited with first; a hand pick or a link starts
/// again. Nothing happens before the first pass after a start.
pub(super) async fn retry(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let src = r.d["wait"]["source"].as_str().unwrap_or("").to_string();
    let Some(failing) = failing_for(st, &src) else { return Ok(()) };
    let proto = r.d["wait"]["proto"].as_str().map(String::from);
    let still = match &proto {
        Some(proto) => failing_client(st, proto),
        None => failing.first().cloned(),
    };
    if let Some(p) = still {
        park(st, r, &src, &p, proto.as_deref());
        return Ok(());
    }
    if let Some(d) = r.d.as_object_mut() {
        d.remove("wait");
    }
    upd(st, r, &[("status", json!("queued")), ("message", json!("The downloads folder can be written again"))]);
    if r.d["pick"].is_object() || r.s("kind") == "url" {
        return start(st, r).await;
    }
    next_source(st, r, "").await
}

/// `downloads` of `GET /api/requests/storage` (and `download_folders` in Diagnostics). Admins: `{ok, text,
/// checked_at, next_check_at (at the latest), checking, waiting, roots}` – `text` the banner ("The downloads folder /downloads is
/// read-only – check the disk (dmesg, fsck) or the mount", null while every folder can be written), `checking`
/// whether a pass runs now (Check again answers while one still does), `roots`
/// each folder downloads are saved in with its result and the time of its probe (`{key, label, path,
/// sources, own, state, result, ok, note, error, checked_at, since, text}`; `state` ok, read_only, denied,
/// missing, full, error, or unknown before its first probe). Everyone else: `{ok}`. Times are unix seconds.
pub fn status(st: &impl Ctx, admin: bool) -> Value {
    let (probed, checked_at, checking) = {
        let s = state(st);
        (s.current(), s.checked_at, s.on.is_some())
    };
    let text = banner(&probed);
    if !admin {
        return json!({ "ok": text.is_none() });
    }
    let mut rows: Vec<Value> = roots(st).iter().map(|root| probed.iter().find(|p| p.root.path == root.path).map(Probed::json).unwrap_or_else(|| unprobed(root))).collect();
    // a folder that still holds things up until the next pass, though no setting names it any more
    let stale: Vec<Value> = probed.iter().filter(|p| p.blocks && !rows.iter().any(|r| r["path"] == p.root.path.to_string_lossy().as_ref())).map(Probed::json).collect();
    rows.extend(stale);
    let every = if text.is_some() { PROBE_SECS } else { IDLE_SECS };
    json!({ "ok": text.is_none(), "text": text, "checked_at": checked_at, "next_check_at": checked_at.map(|t| t + every), "checking": checking,
            "waiting": waiting_count(st), "roots": rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probed(path: &str, found: Found, blocks: bool) -> Probed {
        let root = Root { key: "staging_path", label: "x".into(), path: PathBuf::from(path), sources: vec!["ytdlp"], own: true, protos: vec![], mapped: false };
        let error = if found == Found::Ok { Value::Null } else { json!({ "error": "Input/output error (os error 5)" }) };
        Probed { root, found, error, blocks, note: None, checked_at: 1.0, since: blocks.then_some(1.0), owner: None }
    }

    #[test]
    fn errnos_are_told_apart() {
        let e = |n: i32| StepError::new(Step::Download, "write", Path::new("/downloads/x"), None, std::io::Error::from_raw_os_error(n));
        assert_eq!(Found::of(&e(30)), Found::ReadOnly);
        assert_eq!(Found::of(&e(13)), Found::Denied);
        assert_eq!(Found::of(&e(1)), Found::Denied);
        assert_eq!(Found::of(&e(2)), Found::Missing);
        assert_eq!(Found::of(&e(28)), Found::Full);
        assert_eq!(Found::of(&e(122)), Found::Full);
        assert_eq!(Found::of(&e(5)), Found::Failed);
        assert_eq!(Found::of(&e(107)), Found::Failed);
    }

    #[test]
    fn the_banner_names_the_folder_and_what_to_do() {
        assert_eq!(banner(&[probed("/downloads/encore", Found::Ok, false)]), None);
        assert_eq!(banner(&[probed("/downloads/encore", Found::ReadOnly, true)]).unwrap(),
            "The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount");
        assert_eq!(banner(&[probed("/downloads/torrents", Found::Missing, true)]).unwrap(), "The downloads folder /downloads/torrents is missing – check the mount");
        assert!(banner(&[probed("/downloads/usenet", Found::Full, true)]).unwrap().starts_with("The downloads folder /downloads/usenet is full"));
        let (uid, gid) = ids();
        assert_eq!(banner(&[probed("/downloads/slskd", Found::Denied, true)]).unwrap(),
            format!("The downloads folder /downloads/slskd isn't writable by Encore (uid {uid}, gid {gid}) – chown {uid}:{gid} /downloads/slskd on the host, or set PUID/PGID to its owner"));
        // a folder Encore owns but may not write in: the write permission, not a chown to the owner it has
        let mut own = probed("/downloads/slskd", Found::Denied, true);
        own.owner = Some((uid, 0o40555));
        assert_eq!(banner(&[own.clone()]).unwrap(),
            format!("The downloads folder /downloads/slskd isn't writable: Encore (uid {uid}) owns it, but it has no write permission – chmod u+w /downloads/slskd on the host"));
        own.owner = Some((uid.wrapping_add(1), 0o40755));
        assert!(banner(&[own.clone()]).unwrap().contains(&format!("chown {uid}:{gid} /downloads/slskd")), "another owner: the chown");
        own.owner = Some((uid, 0o40755));
        assert!(banner(&[own]).unwrap().contains("chown"), "Encore's own folder with write permission (an ACL, a policy): the chown advice as before");
        assert_eq!(banner(&[probed("/downloads/encore", Found::Failed, true)]).unwrap(),
            "The downloads folder /downloads/encore can't be written (Input/output error (os error 5)) – check the disk (dmesg, fsck) or the mount");
        // the whole mount read-only: one sentence for the folder they share
        let all = [probed("/downloads/encore", Found::ReadOnly, true), probed("/downloads/slskd", Found::ReadOnly, true)];
        assert_eq!(banner(&all).unwrap(), "The downloads folder /downloads is read-only – check the disk (dmesg, fsck) or the mount");
        // ... but not when another folder there is fine, nor across "/"
        let mixed = [probed("/downloads/encore", Found::ReadOnly, true), probed("/downloads/slskd", Found::ReadOnly, true), probed("/downloads/torrents", Found::Ok, false)];
        assert_eq!(banner(&mixed).unwrap(), "The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount. \
            The downloads folder /downloads/slskd is read-only – check the disk (dmesg, fsck) or the mount");
        let apart = [probed("/a/encore", Found::ReadOnly, true), probed("/b/slskd", Found::Missing, true)];
        assert_eq!(banner(&apart).unwrap(), "The downloads folder /a/encore is read-only – check the disk (dmesg, fsck) or the mount. The downloads folder /b/slskd is missing – check the mount");
        // a folder that holds nothing up (a client's folder Encore only reads) is not in the banner
        assert_eq!(banner(&[probed("/downloads/torrents", Found::Denied, false)]), None);
        // a full disk is one sentence for the folder they share too; a denied permission or a missing folder
        // names each (a chown of /downloads changes nothing in the folders below it)
        let full = [probed("/downloads/encore", Found::Full, true), probed("/downloads/slskd", Found::Full, true)];
        assert_eq!(banner(&full).unwrap(), "The downloads folder /downloads is full – free some space on its disk");
        let denied = [probed("/downloads/encore", Found::Denied, true), probed("/downloads/slskd", Found::Denied, true)];
        assert_eq!(banner(&denied).unwrap(), format!("The downloads folder /downloads/encore isn't writable by Encore (uid {uid}, gid {gid}) – chown {uid}:{gid} /downloads/encore on the host, or set PUID/PGID to its owner. \
            The downloads folder /downloads/slskd isn't writable by Encore (uid {uid}, gid {gid}) – chown {uid}:{gid} /downloads/slskd on the host, or set PUID/PGID to its owner"));
        let missing = [probed("/downloads/torrents", Found::Missing, true), probed("/downloads/usenet", Found::Missing, true)];
        assert_eq!(banner(&missing).unwrap(), "The downloads folder /downloads/torrents is missing – check the mount. The downloads folder /downloads/usenet is missing – check the mount");
    }

    /// A pass that has waited on a folder for half a minute (a network mount that stopped answering): that
    /// folder counts as failing, with its own words, until the pass ends.
    #[test]
    fn a_folder_that_does_not_answer_counts_as_failing() {
        let ok = probed("/mnt/nfs", Found::Ok, false);
        let mut s = State { probed: vec![ok.clone(), probed("/downloads/encore", Found::Ok, false)], checked_at: Some(1.0), wrote_at: Some(1.0), on: None };
        assert!(s.current().iter().all(|p| !p.blocks));
        s.on = Some((ok.root.clone(), now() - 5.0));
        assert!(s.current().iter().all(|p| !p.blocks), "a pass that runs is not hung yet");
        s.on = Some((ok.root.clone(), now() - HUNG_SECS - 1.0));
        let cur = s.current();
        let hung = cur.iter().find(|p| p.root.path == Path::new("/mnt/nfs")).unwrap();
        assert!(hung.blocks && hung.found == Found::Failed && hung.since.is_some(), "{hung:?}");
        assert_eq!(hung.short(), "/mnt/nfs can't be written (it doesn't answer)");
        assert_eq!(banner(&cur).unwrap(), "The downloads folder /mnt/nfs can't be written (it doesn't answer) – check the disk (dmesg, fsck) or the mount");
        assert!(!cur.iter().find(|p| p.root.path == Path::new("/downloads/encore")).unwrap().blocks);
        assert_eq!(s.probed.iter().filter(|p| p.blocks).count(), 0, "what the passes found stays as it was");
    }

    /// The look while nothing downloads writes nothing: a writable folder passes and stays empty, Encore's own
    /// folder that isn't there yet is looked at through the folder it would be made in (and not made), a
    /// client's missing folder is missing, a folder chmod 0555 is denied for any user but root, and the fault
    /// seam's write faults reach it.
    #[test]
    fn the_idle_look_writes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("encore-dl-look-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        assert!(look_dir(&base, Step::Download, false).is_ok());
        assert!(std::fs::read_dir(&base).unwrap().next().is_none(), "nothing written");
        let own = base.join("encore/staging");
        assert!(look_dir(&own, Step::Stage, true).is_ok() && !own.exists(), "not made by a look");
        let e = look_dir(&base.join("torrents"), Step::Download, false).unwrap_err();
        assert_eq!(Found::of(&e), Found::Missing);
        let ro = base.join("ro");
        std::fs::create_dir_all(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        if ids().0 != 0 {
            assert_eq!(Found::of(&look_dir(&ro, Step::Download, true).unwrap_err()), Found::Denied);
            assert_eq!(Found::of(&look_dir(&ro.join("below"), Step::Download, true).unwrap_err()), Found::Denied, "the folder it would be made in");
        }
        let seam = base.join("seam");
        std::fs::create_dir_all(&seam).unwrap();
        fsx::fault::fail(&seam, "write", 30);
        assert_eq!(Found::of(&look_dir(&seam, Step::Download, true).unwrap_err()), Found::ReadOnly);
        fsx::fault::clear(&seam);
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_waiting_requests_words() {
        assert_eq!(probed("/downloads/encore", Found::ReadOnly, true).short(), "/downloads/encore is read-only");
        assert_eq!(probed("/downloads/encore", Found::Missing, true).short(), "/downloads/encore is missing");
        assert_eq!(probed("/downloads/encore", Found::Full, true).short(), "/downloads/encore is full");
        assert_eq!(probed("/downloads/encore", Found::Denied, true).short(), "Encore may not write in /downloads/encore");
        assert_eq!(pick_source("deezer", ""), "streamrip");
        assert_eq!(pick_source("link", "https://www.qobuz.com/album/x"), "streamrip");
        assert_eq!(pick_source("link", "https://bandcamp.com/x"), "ytdlp");
        assert_eq!(pick_source("ytmusic", ""), "ytdlp");
        assert_eq!(pick_source("slskd", ""), "slskd");
        assert_eq!(pick_source("torznab", ""), "torznab");
        // a plugin program's source saves in the staging folder with the others
        assert_eq!((saves_as("ytdlp"), saves_as("slskd_lossless"), saves_as("url")), ("ytdlp", "slskd_lossless", "url"));
        assert_eq!((saves_as("internet-archive"), saves_as("ia_lossless")), (PLUGINS, PLUGINS));
    }

    /// The probe on a real folder: writable, made when it is Encore's own and missing, missing when it is a
    /// client's, and – as a user other than root, for whom a mode means something – a folder chmod 0555
    /// denies the write. A read-only file system can't be made without privileges: the fault seam stands in.
    #[test]
    fn the_probe_tells_a_folder_apart() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("encore-dl-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        assert!(probe_dir(&base, Step::Download, false).is_ok());
        assert!(std::fs::read_dir(&base).unwrap().next().is_none(), "the probe file is gone again");
        // Encore's own folder is made; a client's is missing
        let own = base.join("encore");
        assert!(probe_dir(&own, Step::Stage, true).is_ok() && own.is_dir());
        let theirs = base.join("torrents");
        let e = probe_dir(&theirs, Step::Download, false).unwrap_err();
        assert_eq!((Found::of(&e), e.op), (Found::Missing, "stat"));
        // a file where the folder should be
        std::fs::write(&theirs, b"x").unwrap();
        assert_eq!(Found::of(&probe_dir(&theirs, Step::Download, false).unwrap_err()), Found::Missing);
        // chmod 0555: denied for any user but root
        let ro = base.join("ro");
        std::fs::create_dir_all(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        if ids().0 != 0 {
            let e = probe_dir(&ro, Step::Download, true).unwrap_err();
            assert_eq!((Found::of(&e), e.op), (Found::Denied, "write"), "{e}");
            assert!(e.to_string().contains(".encore-probe-"), "{e}");
        }
        // the fault seam for what needs privileges: EROFS on the write, ENOSPC on the flush
        let seam = base.join("seam");
        std::fs::create_dir_all(&seam).unwrap();
        fsx::fault::fail(&seam, "write", 30);
        assert_eq!(Found::of(&probe_dir(&seam, Step::Download, true).unwrap_err()), Found::ReadOnly);
        fsx::fault::clear(&seam);
        fsx::fault::fail(&seam, "fsync", 28);
        let e = probe_dir(&seam, Step::Download, true).unwrap_err();
        assert_eq!((Found::of(&e), e.op), (Found::Full, "fsync"));
        fsx::fault::clear(&seam);
        assert!(std::fs::read_dir(&seam).unwrap().next().is_none(), "a failed probe removes its file too");
        assert!(probe_dir(&seam, Step::Download, true).is_ok());
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_client_folder_that_encore_only_reads_holds_nothing_up() {
        let base = std::env::temp_dir().join(format!("encore-dl-judge-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let client = Root { key: "qbit_download_path", label: "Torrent downloads (qBittorrent)".into(), path: base.clone(), sources: vec!["torznab"], own: false,
                            protos: vec!["torrent"], mapped: false };
        let denied = || Err(StepError::new(Step::Download, "write", &base.join(".encore-probe-1"), None, std::io::Error::from_raw_os_error(13)));
        let (found, error, blocks, note) = judge(&client, denied());
        assert_eq!((found, blocks, note), (Found::Denied, false, Some(NOTE_READS)));
        assert_eq!(error["errno"], 13);
        // Encore's own folder must be writable
        let own = Root { own: true, ..client.clone() };
        assert!(judge(&own, denied()).2);
        // a client's folder that is missing holds its downloads up
        let gone = Root { path: base.join("nope"), ..client.clone() };
        let missing = Err(StepError::new(Step::Download, "stat", &gone.path, None, std::io::Error::from_raw_os_error(2)));
        assert!(judge(&gone, missing).2);
        assert_eq!(judge(&client, Ok(())), (Found::Ok, Value::Null, false, None));
        // SABnzbd's or NZBGet's folder is only where Encore looks: missing in Encore's container is fine, read-only
        // over a read-only disk is not
        let sab = Root { key: "sab_download_path", label: "Usenet downloads (SABnzbd)".into(), protos: vec!["usenet"], mapped: true, ..gone.clone() };
        let missing = Err(StepError::new(Step::Download, "stat", &sab.path, None, std::io::Error::from_raw_os_error(2)));
        let (found, _, blocks, note) = judge(&sab, missing);
        assert_eq!((found, blocks, note), (Found::Missing, false, Some(NOTE_MAPPED)));
        let full = Err(StepError::new(Step::Download, "fsync", &sab.path, None, std::io::Error::from_raw_os_error(28)));
        assert!(judge(&sab, full).2, "a full disk holds its downloads up");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_read_only_volume_is_told_from_a_read_only_disk() {
        let info = "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw,errors=remount-ro\n\
                    40 22 8:17 / /downloads rw,relatime - ext4 /dev/sdb1 rw\n\
                    41 40 8:17 /torrents /downloads/torrents ro,relatime - ext4 /dev/sdb1 rw\n\
                    42 40 8:33 / /downloads/usenet ro,relatime - ext4 /dev/sdc1 ro,errors=remount-ro\n\
                    43 40 0:50 / /downloads/my\\040disk ro - fuse.mergerfs pool rw,user_id=0\n";
        assert!(ro_mount_in(info, Path::new("/downloads/torrents/x")), "a :ro volume over a writable disk");
        assert!(!ro_mount_in(info, Path::new("/downloads/usenet")), "a disk remounted read-only");
        assert!(!ro_mount_in(info, Path::new("/downloads/encore")), "a writable mount");
        assert!(ro_mount_in(info, Path::new("/downloads/my disk")), "escaped mount points");
        assert!(!ro_mount_in("", Path::new("/x")));
        // the last mount at a point is the one in use
        let stacked = "40 22 8:17 / /downloads rw - ext4 /dev/sdb1 rw\n50 22 8:17 / /downloads ro - ext4 /dev/sdb1 rw\n";
        assert!(ro_mount_in(stacked, Path::new("/downloads/x")));
        assert_eq!(unescape("/a\\040b\\134c"), "/a b\\c");
        assert_eq!(unescape("/a\\04"), "/a\\04");
    }
}
