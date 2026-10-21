//! The spawn builder every plugin program goes through, and the sandbox level the host allows.
//!
//! Two levels (docs/plugins/CONTRACT.md, "The sandbox"):
//! - **Full isolation**: the program runs under bubblewrap in namespaces of its own (user, mount, pid, ipc, uts,
//!   cgroup, and the network unless the plugin may use it), sees the system folders read-only, its own folders,
//!   the folders the call adds, a private `/tmp`, and nothing else of the host: not the config folder, not the
//!   database, not the other plugins. It dies with Encore.
//! - **Limits only**: when the host does not allow user namespaces (a Docker container without the compose
//!   option): resource limits, no new privileges, a session of its own, nice 10 and idle disk priority, a
//!   cleared environment with a private HOME and TMPDIR. It does not hide files.
//!
//! Both levels set the limits, the session, no new privileges, close-on-exec on every inherited descriptor above
//! stderr, and a cleared environment (the caller gives the whole environment). The level is found once at start
//! ([`Sandbox::detect`], the probe) and is an input of [`Spawn`], so tests run both.
use encore_core::fsx::{self, Step};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// What the host allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    /// bubblewrap works: `proc` says whether it may mount a /proc of the sandbox's own (Docker refuses that at
    /// times; the sandbox then has no /proc at all, never the host's)
    Full { bwrap: PathBuf, proc: bool },
    Limits,
}

/// The sandbox level found on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub level: Level,
    /// why it is not full isolation (bwrap's own words), for the log and Diagnostics
    pub reason: Option<String>,
}

/// System folders a fully isolated program sees read-only (when the host has them).
const SYSTEM_RO: [&str; 15] = [
    "/usr", "/etc/ssl", "/etc/ca-certificates", "/etc/pki", "/etc/resolv.conf", "/etc/hosts", "/etc/nsswitch.conf",
    "/etc/host.conf", "/etc/alternatives", "/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/ld.so.conf.d", "/etc/localtime",
    "/etc/mime.types", "/opt/python",
];
/// Top-level folders that are links into /usr on merged-/usr systems (made as the same links), or folders.
const SYSTEM_TOP: [&str; 6] = ["/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32"];

impl Sandbox {
    /// The level of this host, probed once (`bwrap` on PATH, then the probe).
    pub fn detect() -> Sandbox {
        static FOUND: OnceLock<Sandbox> = OnceLock::new();
        FOUND
            .get_or_init(|| {
                let s = Sandbox::probe(encore_core::which::which("bwrap").as_deref());
                match &s.level {
                    Level::Full { proc, .. } => tracing::info!("plugins: full isolation (bubblewrap{})", if *proc { "" } else { ", no /proc" }),
                    Level::Limits => tracing::info!("plugins: limits only ({})", s.reason.as_deref().unwrap_or("no bubblewrap")),
                }
                s
            })
            .clone()
    }

    /// Limits only (no probe).
    pub fn limits_only() -> Sandbox {
        Sandbox { level: Level::Limits, reason: None }
    }

    /// Run bubblewrap as a plugin would be run, with the same pre-exec rules: with a /proc of its own, else
    /// without one; neither works → limits only.
    pub fn probe(bwrap: Option<&Path>) -> Sandbox {
        let Some(bwrap) = bwrap else {
            return Sandbox { level: Level::Limits, reason: Some("bubblewrap is not installed".into()) };
        };
        let mut reason = None;
        for proc in [true, false] {
            let full = Sandbox { level: Level::Full { bwrap: bwrap.to_path_buf(), proc }, reason: None };
            let t = Path::new("/bin/true");
            let program = if t.exists() { t.to_path_buf() } else { PathBuf::from("/usr/bin/true") };
            let spawn = Spawn::new(&full, &program).env(vec![("PATH".into(), "/usr/bin:/bin".into())]);
            let mut cmd = spawn.command();
            cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
            match cmd.output() {
                Ok(o) if o.status.success() => return full,
                Ok(o) => reason = Some(String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").trim().to_string()),
                Err(e) => reason = Some(e.to_string()),
            }
        }
        Sandbox { level: Level::Limits, reason }
    }

    pub fn is_full(&self) -> bool {
        matches!(self.level, Level::Full { .. })
    }

    /// The line on every plugin's card.
    pub fn text(&self) -> &'static str {
        if self.is_full() { "Full isolation" } else { "Limits only, see the compose option" }
    }

    /// For the API: `{level: "full"|"limits", text, reason}`.
    pub fn json(&self) -> Value {
        json!({ "level": if self.is_full() { "full" } else { "limits" }, "text": self.text(), "reason": self.reason })
    }
}

/// Resource limits of a run (0: none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    /// address space, MB
    pub memory_mb: u64,
    /// processor seconds
    pub cpu_secs: u64,
    /// processes and threads it may add (over what Encore's user runs already)
    pub processes: u64,
    pub open_files: u64,
}

impl Limits {
    pub const NONE: Limits = Limits { memory_mb: 0, cpu_secs: 0, processes: 0, open_files: 0 };
}

/// One program start.
#[derive(Debug, Clone)]
pub struct Spawn {
    sandbox: Sandbox,
    program: PathBuf,
    args: Vec<OsString>,
    /// the whole environment (nothing else of Encore's reaches the program)
    env: Vec<(String, String)>,
    cwd: Option<PathBuf>,
    /// folders it sees read-only / writable (full isolation; at the same paths as outside)
    ro: Vec<PathBuf>,
    rw: Vec<PathBuf>,
    network: bool,
    limits: Limits,
    /// nice 10 and idle disk priority
    low_priority: bool,
}

impl Spawn {
    pub fn new(sandbox: &Sandbox, program: &Path) -> Spawn {
        Spawn {
            sandbox: sandbox.clone(),
            program: program.to_path_buf(),
            args: vec![],
            env: vec![],
            cwd: None,
            ro: vec![],
            rw: vec![],
            network: false,
            limits: Limits::NONE,
            low_priority: false,
        }
    }

    pub fn arg(mut self, a: impl Into<OsString>) -> Spawn {
        self.args.push(a.into());
        self
    }

    pub fn args<I: Into<OsString>>(mut self, a: impl IntoIterator<Item = I>) -> Spawn {
        self.args.extend(a.into_iter().map(Into::into));
        self
    }

    /// The whole environment.
    pub fn env(mut self, env: Vec<(String, String)>) -> Spawn {
        self.env = env;
        self
    }

    pub fn cwd(mut self, dir: &Path) -> Spawn {
        self.cwd = Some(dir.to_path_buf());
        self
    }

    pub fn ro(mut self, p: &Path) -> Spawn {
        self.ro.push(p.to_path_buf());
        self
    }

    pub fn rw(mut self, p: &Path) -> Spawn {
        self.rw.push(p.to_path_buf());
        self
    }

    pub fn network(mut self, on: bool) -> Spawn {
        self.network = on;
        self
    }

    pub fn limits(mut self, l: Limits) -> Spawn {
        self.limits = l;
        self
    }

    pub fn low_priority(mut self, on: bool) -> Spawn {
        self.low_priority = on;
        self
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn sandbox(&self) -> &Sandbox {
        &self.sandbox
    }

    /// What is executed: (program, arguments) – bubblewrap and its arguments around the program at full
    /// isolation, the program itself otherwise.
    pub fn argv(&self) -> (PathBuf, Vec<OsString>) {
        let Level::Full { bwrap, proc } = &self.sandbox.level else {
            return (self.program.clone(), self.args.clone());
        };
        let mut a: Vec<OsString> = vec!["--unshare-all".into()];
        if self.network {
            a.push("--share-net".into());
        }
        a.push("--die-with-parent".into());
        let ro_bind = |a: &mut Vec<OsString>, p: &Path, must: bool| {
            a.push(if must { "--ro-bind" } else { "--ro-bind-try" }.into());
            a.push(p.into());
            a.push(p.into());
        };
        for top in SYSTEM_TOP {
            let p = Path::new(top);
            match std::fs::symlink_metadata(p) {
                Ok(m) if m.file_type().is_symlink() => {
                    if let Ok(target) = std::fs::read_link(p) {
                        a.extend(["--symlink".into(), target.into_os_string(), p.into()]);
                    }
                }
                Ok(m) if m.is_dir() => ro_bind(&mut a, p, false),
                _ => {}
            }
        }
        for sys in SYSTEM_RO {
            ro_bind(&mut a, Path::new(sys), false);
        }
        if *proc {
            a.extend(["--proc".into(), "/proc".into()]);
        }
        a.extend(["--dev".into(), "/dev".into(), "--tmpfs".into(), "/tmp".into()]);
        for p in &self.ro {
            ro_bind(&mut a, p, true);
        }
        for p in &self.rw {
            a.extend(["--bind".into(), p.into(), p.into()]);
        }
        if let Some(c) = &self.cwd {
            a.extend(["--chdir".into(), c.into()]);
        }
        a.push("--".into());
        a.push(self.program.clone().into_os_string());
        a.extend(self.args.iter().cloned());
        (bwrap.clone(), a)
    }

    /// The command, ready to spawn (stdin, stdout and stderr are the caller's to set): the cleared environment,
    /// the working folder, and the limits applied between fork and exec.
    pub fn command(&self) -> std::process::Command {
        use std::os::unix::process::CommandExt;
        let (program, args) = self.argv();
        let mut cmd = std::process::Command::new(program);
        cmd.args(args).env_clear().envs(self.env.iter().map(|(k, v)| (k, v)));
        if let Some(c) = &self.cwd {
            cmd.current_dir(c);
        }
        let limits = self.limits;
        // RLIMIT_NPROC counts every process and thread of the user, Encore's own included: the limit is what runs
        // now plus what the plugin may add
        let nproc = if limits.processes > 0 { tasks_of_this_user() + limits.processes } else { 0 };
        let low = self.low_priority;
        // SAFETY: the closure runs between fork and exec and only makes async-signal-safe system calls
        // (setsid, prctl, close_range or fcntl, setrlimit, setpriority, ioprio_set); it allocates nothing.
        unsafe {
            cmd.pre_exec(move || pre_exec(limits, nproc, low));
        }
        cmd
    }

    /// [`Spawn::command`] for tokio.
    pub fn tokio_command(&self) -> tokio::process::Command {
        let mut c = tokio::process::Command::from(self.command());
        c.kill_on_drop(true);
        c
    }

    /// Start it: the error names the program and says what to check ("start failed: start /config/plugins/x/…:
    /// No such file or directory – the program is missing").
    pub fn spawn_tokio(&self, setup: impl FnOnce(&mut tokio::process::Command)) -> Result<tokio::process::Child, fsx::StepError> {
        let mut c = self.tokio_command();
        setup(&mut c);
        fsx::spawn(Step::Plugin, &self.program, || c.spawn())
    }
}

fn pre_exec(l: Limits, nproc: u64, low: bool) -> std::io::Result<()> {
    // SAFETY (whole function): plain system calls on this process, no allocation, no locks
    unsafe {
        if libc::setsid() == -1 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // every descriptor above stderr closes at exec (std's own error pipe is close-on-exec already)
        if libc::syscall(libc::SYS_close_range, 3 as libc::c_uint, libc::c_uint::MAX, libc::CLOSE_RANGE_CLOEXEC) != 0 {
            let mut top = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            let max = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut top) == 0 { top.rlim_cur.min(65536) as libc::c_int } else { 4096 };
            for fd in 3..max {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        let set = |res, soft: u64, hard: u64| -> std::io::Result<()> {
            let r = libc::rlimit { rlim_cur: soft as libc::rlim_t, rlim_max: hard as libc::rlim_t };
            if libc::setrlimit(res, &r) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        };
        set(libc::RLIMIT_CORE, 0, 0)?;
        if l.memory_mb > 0 {
            let b = l.memory_mb.saturating_mul(1 << 20);
            set(libc::RLIMIT_AS, b, b)?;
        }
        if l.cpu_secs > 0 {
            set(libc::RLIMIT_CPU, l.cpu_secs, l.cpu_secs + 5)?;
        }
        if l.open_files > 0 {
            set(libc::RLIMIT_NOFILE, l.open_files, l.open_files)?;
        }
        if nproc > 0 {
            set(libc::RLIMIT_NPROC, nproc, nproc)?;
        }
        if low {
            // lowering the priority only; an Encore that runs lower already stays where it is
            let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
            // IOPRIO_WHO_PROCESS = 1, IOPRIO_CLASS_IDLE = 3 (in the top bits of the priority value)
            let _ = libc::syscall(libc::SYS_ioprio_set, 1 as libc::c_int, 0 as libc::c_int, (3 << 13) as libc::c_int);
        }
    }
    Ok(())
}

/// The environment of a program Encore starts itself (yt-dlp, streamrip, slskd, Cadenza, the translator's
/// llama-server): Encore's own, without Encore's settings – no `ENCORE_*` name and no variable a setting reads
/// (settings.toml `env`, such as passwords and API keys) – and without forced colours. Such a program is
/// dumpable (a fresh exec), so at limits only a plugin of the same user could read its /proc/<pid>/environ;
/// what Encore keeps for itself is not in it.
pub fn program_env() -> Vec<(String, String)> {
    let settings: std::collections::HashSet<&str> = encore_config::settings::TABLE.iter().filter_map(|d| d.env).collect();
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(k, _)| !k.starts_with("ENCORE_") && !settings.contains(k.as_str()) && k != "FORCE_COLOR" && k != "CLICOLOR_FORCE")
        .collect()
}

/// Make this process non-dumpable: other processes of the same user (a plugin at limits only) can no longer
/// read its /proc entries (environ, mem, fd, maps). Encore's own reading of /proc/self is unaffected.
pub fn hide_own_proc() {
    // SAFETY: prctl on this process only
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong);
    }
}

/// How many processes and threads Encore's user runs now (what RLIMIT_NPROC counts), read from /proc.
pub fn tasks_of_this_user() -> u64 {
    // SAFETY: getuid cannot fail
    let uid = unsafe { libc::getuid() };
    let Ok(rd) = std::fs::read_dir("/proc") else { return 256 };
    let mut n = 0u64;
    for e in rd.flatten() {
        let name = e.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(status) = std::fs::read_to_string(e.path().join("status")) else { continue };
        let mut mine = false;
        let mut threads = 1u64;
        for line in status.lines() {
            if let Some(v) = line.strip_prefix("Uid:") {
                mine = v.split_whitespace().next().and_then(|r| r.parse::<u32>().ok()) == Some(uid);
            } else if let Some(v) = line.strip_prefix("Threads:") {
                threads = v.trim().parse().unwrap_or(1);
            }
        }
        if mine {
            n += threads;
        }
    }
    n.max(1)
}

/// Stop a process group started by [`Spawn`] (its leader `pid` is the group's id): SIGTERM, then after
/// `grace` SIGKILL to the whole group. At full isolation the leader is bubblewrap, which would end the sandbox
/// at once on SIGTERM: the plugin's own processes get the SIGTERM (so the SDK can clean its `.part` files), and
/// bubblewrap follows when they end.
pub async fn stop_group(pid: u32, sandboxed: bool, grace: std::time::Duration, mut exited: impl FnMut() -> bool) {
    let pgid = pid as libc::pid_t;
    let inner: Vec<libc::pid_t> = if sandboxed { group_members(pgid).into_iter().filter(|p| *p != pgid).collect() } else { vec![] };
    // SAFETY: kill only sends a signal
    unsafe {
        if inner.is_empty() {
            libc::kill(-pgid, libc::SIGTERM);
        } else {
            for p in inner {
                libc::kill(p, libc::SIGTERM);
            }
        }
    }
    let until = std::time::Instant::now() + grace;
    while std::time::Instant::now() < until {
        if exited() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // SAFETY: as above; the group may be gone already (ESRCH is fine)
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

/// The processes whose process group is `pgid` (from /proc).
pub fn group_members(pgid: libc::pid_t) -> Vec<libc::pid_t> {
    let Ok(rd) = std::fs::read_dir("/proc") else { return vec![] };
    let mut out = vec![];
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<libc::pid_t>() else { continue };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        // pid (comm) state ppid pgrp …: comm may hold spaces and parentheses, so read after the last ')'
        let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else { continue };
        let f: Vec<&str> = rest.split_whitespace().collect();
        if f.get(2).and_then(|g| g.parse::<libc::pid_t>().ok()) == Some(pgid) {
            out.push(pid);
        }
    }
    out
}
