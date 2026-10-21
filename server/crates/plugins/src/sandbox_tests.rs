//! The sandbox: the probe, bubblewrap's arguments, and what a program sees at each level this host allows
//! (limits only always; full isolation when user namespaces work here).
use crate::install::output;
use crate::sandbox::{tasks_of_this_user, Level, Limits, Sandbox, Spawn};
use crate::test_util::levels;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn tmp(name: &str) -> PathBuf {
    encore_core::tmp::dir(&format!("sandbox-{name}"))
}

fn sh(s: &Sandbox, cmd: &str) -> Spawn {
    Spawn::new(s, Path::new("/bin/sh")).args(["-c", cmd]).env(vec![("PATH".into(), "/usr/bin:/bin".into()), ("FOO".into(), "bar".into())])
}

fn run(spawn: &Spawn) -> (bool, String, String) {
    let o = output(spawn, Duration::from_secs(20)).unwrap();
    (o.ok, o.stdout, o.stderr)
}

#[test]
fn without_bubblewrap_it_is_limits_only() {
    let s = Sandbox::probe(None);
    assert_eq!(s.level, Level::Limits);
    assert!(s.reason.as_deref().unwrap_or("").contains("not installed"));
    let s = Sandbox::probe(Some(Path::new("/nonexistent/bwrap")));
    assert_eq!(s.level, Level::Limits);
    assert_eq!(Sandbox::limits_only().text(), "Limits only, see the compose option");
    assert_eq!(Sandbox::limits_only().json()["level"], "limits");
    assert!(tasks_of_this_user() >= 1);
}

#[test]
fn bubblewrap_gets_namespaces_binds_and_no_network_unless_asked() {
    let full = Sandbox { level: Level::Full { bwrap: PathBuf::from("/usr/bin/bwrap"), proc: true }, reason: None };
    assert_eq!(full.text(), "Full isolation");
    let spawn = Spawn::new(&full, Path::new("/usr/bin/python3"))
        .args(["/p/1.0.0/plugin.py", "search"])
        .ro(Path::new("/p/1.0.0"))
        .rw(Path::new("/p/data"))
        .cwd(Path::new("/p/data"));
    let (program, args) = spawn.argv();
    assert_eq!(program, PathBuf::from("/usr/bin/bwrap"));
    let a: Vec<String> = args.iter().map(|s| s.to_string_lossy().to_string()).collect();
    let has = |w: &[&str]| a.windows(w.len()).any(|x| x == w);
    assert!(has(&["--unshare-all"]) && has(&["--die-with-parent"]), "{a:?}");
    assert!(!has(&["--share-net"]), "no network unless the manifest asks: {a:?}");
    assert!(has(&["--ro-bind", "/p/1.0.0", "/p/1.0.0"]) && has(&["--bind", "/p/data", "/p/data"]), "{a:?}");
    assert!(has(&["--proc", "/proc"]) && has(&["--tmpfs", "/tmp"]) && has(&["--dev", "/dev"]), "{a:?}");
    assert!(has(&["--chdir", "/p/data"]), "{a:?}");
    assert!(has(&["--", "/usr/bin/python3", "/p/1.0.0/plugin.py", "search"]), "the program and its argv come last: {a:?}");
    assert!(a.iter().any(|x| x == "/usr") && !a.iter().any(|x| x == "/home" || x == "/root"), "{a:?}");
    let netted = spawn.clone().network(true).argv().1;
    assert!(netted.contains(&OsString::from("--share-net")));
    let noproc = Sandbox { level: Level::Full { bwrap: PathBuf::from("/usr/bin/bwrap"), proc: false }, reason: None };
    let (_, a2) = Spawn::new(&noproc, Path::new("/bin/true")).argv();
    assert!(!a2.contains(&OsString::from("--proc")), "never the host's /proc instead");
    // limits only: the program itself
    let (program, args) = Spawn::new(&Sandbox::limits_only(), Path::new("/bin/true")).arg("x").argv();
    assert_eq!((program, args), (PathBuf::from("/bin/true"), vec![OsString::from("x")]));
}

/// At every level: the environment is only what was given, a session of its own, no new privileges, the
/// limits, nice 10.
#[test]
fn every_level_clears_the_environment_and_sets_the_limits() {
    for s in levels() {
        let proc = !matches!(s.level, Level::Full { proc: false, .. });
        let d = tmp("limits");
        let spawn = sh(&s, "env | sort; echo nofile=$(ulimit -n); echo as=$(ulimit -v); echo cwd=$(pwd)")
            .limits(Limits { memory_mb: 512, cpu_secs: 30, processes: 0, open_files: 256 })
            .low_priority(true)
            .cwd(&d)
            .rw(&d);
        let (ok, out, err) = run(&spawn);
        assert!(ok, "{:?}: {err}", s.level);
        let env: Vec<&str> = out.lines().filter(|l| l.contains('=') && !l.starts_with("nofile=") && !l.starts_with("as=") && !l.starts_with("cwd=")).collect();
        for l in &env {
            let k = l.split('=').next().unwrap();
            assert!(["PATH", "FOO", "PWD", "SHLVL", "_"].contains(&k), "{:?}: Encore's environment leaked: {l}", s.level);
        }
        assert!(out.contains("FOO=bar"), "{out}");
        assert!(out.contains("nofile=256"), "{:?}: {out}", s.level);
        assert!(out.contains(&format!("as={}", 512 * 1024)), "{:?}: {out}", s.level);
        assert!(out.contains(&format!("cwd={}", d.display())), "{:?}: {out}", s.level);
        if proc {
            let (_, st, _) = run(&sh(&s, "grep NoNewPrivs /proc/self/status; cut -d' ' -f6,19 /proc/self/stat; echo $$").low_priority(true));
            let lines: Vec<&str> = st.lines().collect();
            assert!(lines[0].ends_with('1'), "{:?}: no new privileges: {st}", s.level);
            let f: Vec<&str> = lines[1].split(' ').collect();
            assert_eq!(f[1], "10", "{:?}: nice 10: {st}", s.level);
            if s.level == Level::Limits {
                assert_eq!(f[0], lines[2], "a session of its own: {st}");
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Full isolation (when this host allows it): only the folders given are there, read-only ones cannot be
/// written, the network is off unless asked.
#[test]
fn full_isolation_hides_the_host() {
    let Some(full) = levels().into_iter().find(|s| s.is_full()) else { return };
    let d = tmp("hide");
    let secret = d.join("config/encore.db");
    std::fs::create_dir_all(secret.parent().unwrap()).unwrap();
    std::fs::write(&secret, "secret").unwrap();
    let ro = d.join("plugin/1.0.0");
    let rw = d.join("plugin/data");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::create_dir_all(&rw).unwrap();
    std::fs::write(ro.join("plugin.py"), "code").unwrap();
    let base = |cmd: &str| sh(&full, cmd).ro(&ro).rw(&rw);
    let (ok, _, _) = run(&base(&format!("cat {}", secret.display())));
    assert!(!ok, "the config folder is not there");
    let (ok, out, _) = run(&base(&format!("cat {}/plugin.py", ro.display())));
    assert!(ok && out == "code", "its own program is");
    let (ok, _, _) = run(&base(&format!("echo x > {}/plugin.py", ro.display())));
    assert!(!ok, "read-only");
    let (ok, _, err) = run(&base(&format!("echo x > {}/state && cat {}/state", rw.display(), rw.display())));
    assert!(ok, "its data folder is writable: {err}");
    assert_eq!(std::fs::read_to_string(rw.join("state")).unwrap(), "x\n");
    if matches!(full.level, Level::Full { proc: true, .. }) {
        let (_, net, _) = run(&base("cat /proc/net/dev"));
        let ifaces: Vec<&str> = net.lines().skip(2).filter_map(|l| l.split(':').next()).map(str::trim).collect();
        assert_eq!(ifaces, ["lo"], "no network: {net}");
        let (_, procs, _) = run(&base("ls /proc | grep -c '^[0-9]'"));
        assert!(procs.trim().parse::<u32>().unwrap_or(99) <= 4, "a pid namespace of its own: {procs}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The programs Encore starts itself get its environment without Encore's own settings (they are dumpable, so a
/// plugin at limits only could read their /proc/<pid>/environ).
#[test]
fn programs_encore_starts_get_no_settings_in_their_environment() {
    let env = crate::sandbox::program_env();
    let settings: Vec<&str> = encore_config::settings::TABLE.iter().filter_map(|d| d.env).collect();
    assert!(settings.contains(&"SOULSEEK_PASSWORD") && settings.contains(&"GITHUB_TOKEN"), "the table names the variables");
    for (k, _) in &env {
        assert!(!k.starts_with("ENCORE_") && !settings.contains(&k.as_str()), "{k} is Encore's own");
        assert!(k != "FORCE_COLOR" && k != "CLICOLOR_FORCE");
    }
    if let Ok(path) = std::env::var("PATH") {
        assert!(env.iter().any(|(k, v)| k == "PATH" && *v == path), "the rest stays (PATH, the proxy, the locale)");
    }
}
