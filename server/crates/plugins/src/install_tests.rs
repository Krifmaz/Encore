//! The installer: fetch (cap, sha256, https only), sums, the version folders, a program install end to end
//! against a stand-in (sha256 mismatch, a program that does not run, a check that does not hold), a venv made
//! by a stand-in uv, a data file.
use crate::fetch;
use crate::install::{self, Net, Program, Versions};
use crate::manifest::{parse_for, Origin};
use crate::test_util::{python_aware_uv, script, tar_gz, file, StandIn};
use std::path::PathBuf;
use std::time::Duration;

fn tmp(name: &str) -> PathBuf {
    encore_core::tmp::dir(&format!("install-{name}"))
}

fn sha(b: &[u8]) -> String {
    encore_core::ids::sha256_hex(b)
}

#[tokio::test]
async fn a_fetch_is_capped_summed_and_https_only() {
    let s = StandIn::start().await;
    s.put("/ok.bin", vec![7u8; 5000]);
    s.put("/big.bin", vec![1u8; 70_000]);
    let d = tmp("fetch");
    let http = fetch::client("test", true);
    let got = fetch::to_file(&http, &s.url("/ok.bin"), &d.join("ok.bin"), 10_000, Duration::from_secs(10), true).await.unwrap();
    assert_eq!((got.size, got.sha256.as_str()), (5000, sha(&[7u8; 5000]).as_str()));
    assert_eq!(fetch::sha256_file(&d.join("ok.bin")).unwrap(), got.sha256);
    let e = fetch::to_file(&http, &s.url("/big.bin"), &d.join("big.bin"), 10_000, Duration::from_secs(10), true).await.unwrap_err().to_string();
    assert!(e.contains("larger than"), "{e}");
    assert!(!d.join("big.bin").exists() && !d.join("big.bin.part").exists(), "nothing is left of a download that grew too large");
    let e = fetch::to_file(&http, &s.url("/missing"), &d.join("m"), 10_000, Duration::from_secs(10), true).await.unwrap_err().to_string();
    assert!(e.contains("HTTP 404"), "{e}");
    // plain http is refused unless allowed (tests only), and so is any other scheme or a password in the address
    let strict = fetch::client("test", false);
    let e = fetch::to_file(&strict, &s.url("/ok.bin"), &d.join("x"), 10_000, Duration::from_secs(10), false).await.unwrap_err().to_string();
    assert!(e.contains("only https"), "{e}");
    assert!(fetch::check_url("file:///etc/passwd", true).is_err());
    assert!(fetch::check_url("ftp://example.org/x", true).is_err());
    assert!(fetch::check_url("https://user:pw@example.org/x", false).is_err());
    assert!(fetch::check_url("https://example.org/x.zip", false).is_ok());
    assert_eq!(fetch::shown("https://example.org/a/b.zip?token=secret"), "https://example.org/a/b.zip");
    // a strict client does not follow a redirect to plain http (the first request made by hand here)
    s.put("/target", b"plain".to_vec());
    s.put_status("/moved", 302, "", &[("location", &s.url("/target"))]);
    let e = strict.get(s.url("/moved")).send().await.unwrap_err();
    assert!(e.is_redirect(), "{e}");
    assert_eq!(fetch::text(&http, &s.url("/moved"), 100, true).await.unwrap(), "plain", "a test client follows it");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn sums_files_are_read() {
    let a = "a".repeat(64);
    let b = "B".repeat(64);
    let text = format!("{a}  cadenza-2.1.0-linux-x86_64.tar.gz\n{b} *cadenza-2.1.0-linux-aarch64.tar.gz\n\nnot a line\n{}  short\n{a}  ./dot.tar.gz\n", "c".repeat(10));
    let m = install::parse_sums(&text);
    assert_eq!(m.get("cadenza-2.1.0-linux-x86_64.tar.gz"), Some(&a));
    assert_eq!(m.get("cadenza-2.1.0-linux-aarch64.tar.gz"), Some(&"b".repeat(64)), "binary mode, lower-cased");
    assert_eq!(m.get("dot.tar.gz"), Some(&a));
    assert_eq!(m.len(), 3);
    assert!(install::sum_for(&text, "missing").is_err());
}

#[test]
fn version_folders_switch_and_prune() {
    let d = tmp("versions");
    let v = Versions::new(&d);
    assert_eq!(v.current(), None);
    for keep in ["data", "logs", "home", "tmp"] {
        std::fs::create_dir_all(d.join(keep)).unwrap();
    }
    std::fs::create_dir_all(d.join("0.9.0")).unwrap();
    std::fs::create_dir_all(d.join(".0.8.0.new")).unwrap();
    assert!(v.stage("../x").is_err() && v.stage("data").is_err(), "a version is a folder name with a digit");
    let s = v.stage("1.0.0").unwrap();
    std::fs::write(s.join("prog"), b"x").unwrap();
    v.commit("1.0.0").unwrap();
    assert_eq!(v.current().as_deref(), Some("1.0.0"));
    assert!(d.join("1.0.0/prog").is_file());
    assert!(!d.join("0.9.0").exists() && !d.join(".0.8.0.new").exists(), "old versions and leftovers go");
    for keep in ["data", "logs", "home", "tmp"] {
        assert!(d.join(keep).is_dir(), "{keep} stays");
    }
    std::fs::write(d.join("current"), "../../etc\n").unwrap();
    assert_eq!(v.current(), None);
    let _ = std::fs::remove_dir_all(&d);
}

/// A fake program: answers `--version`, or `render --describe` with JSON.
fn program(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\nif [ \"$1\" = render ]; then echo '{{\"name\":\"cadenza\",\"version\":\"{version}\",\"protocol\":1}}'; else echo 'prog {version}'; fi\n").into_bytes()
}

const MANIFEST: &str = r#"
[plugin]
id = "prog-test"
name = "Prog"
version = "0.1.0"
kind = "tool"
protocol = 1
commands = []
builtin = true

[update]
github = "o/prog"
sums = "SHA256SUMS"

[[binary]]
arch = "x86_64"
asset = "prog-{version}-x86_64.tar.gz"
bin = "prog"

[[binary]]
arch = "aarch64"
asset = "prog-{version}-aarch64.tar.gz"
bin = "prog"

[install]
remove = ["junk"]
"#;

#[tokio::test]
async fn a_program_is_fetched_checked_unpacked_verified_and_switched() {
    let s = StandIn::start().await;
    let d = tmp("program");
    let m = parse_for(MANIFEST, Origin::Catalog).unwrap();
    let v = Versions::new(&d.join("plugins/prog-test"));
    let net = Net { http: fetch::client("test", true), allow_http: true };
    let good = tar_gz(&[file("prog-1.2.0/prog", &program("1.2.0"), 0o755), file("prog-1.2.0/junk/x", b"x", 0o644)]);
    s.put("/good.tar.gz", good.clone());
    let p = |url: &str, sum: &str| Program { url: s.url(url), name: "prog-1.2.0-x86_64.tar.gz".into(), sha256: Some(sum.into()), unpack: "tar.gz".into(), bin: "prog".into(), remove: vec!["prog-1.2.0/junk".into()], max_bytes: 1 << 20 };
    // a sha256 that does not match: nothing changes
    let e = install::install_program(&net, &m, &v, "1.2.0", &p("/good.tar.gz", &"0".repeat(64)), |_| Ok(())).await.unwrap_err().to_string();
    assert!(e.contains("does not match"), "{e}");
    assert_eq!(v.current(), None);
    assert!(!v.dir("1.2.0").exists() && !v.root.join(".1.2.0.new").exists());
    // the real thing
    let inst = install::install_program(&net, &m, &v, "1.2.0", &p("/good.tar.gz", &sha(&good)), |_| Ok(())).await.unwrap();
    assert_eq!((inst.version.as_str(), inst.reports.as_deref()), ("1.2.0", Some("1.2.0")));
    assert_eq!(inst.bin, v.dir("1.2.0").join("prog-1.2.0/prog"));
    assert!(inst.bin.is_file());
    assert!(!v.dir("1.2.0").join("prog-1.2.0/junk").exists(), "[install] remove");
    assert_eq!(v.current().as_deref(), Some("1.2.0"));
    // a version that does not run is refused and the one in use stays
    let broken = tar_gz(&[file("prog", b"#!/bin/sh\nexit 4\n", 0o755)]);
    s.put("/broken.tar.gz", broken.clone());
    let e = install::install_program(&net, &m, &v, "1.3.0", &p("/broken.tar.gz", &sha(&broken)), |_| Ok(())).await.unwrap_err().to_string();
    assert!(e.contains("failed"), "{e}");
    assert_eq!(v.current().as_deref(), Some("1.2.0"));
    assert!(!v.dir("1.3.0").exists());
    // an archive without the program
    let empty = tar_gz(&[file("README", b"x", 0o644)]);
    s.put("/empty.tar.gz", empty.clone());
    let e = install::install_program(&net, &m, &v, "1.4.0", &p("/empty.tar.gz", &sha(&empty)), |_| Ok(())).await.unwrap_err().to_string();
    assert!(e.contains("no prog program"), "{e}");
    // no sum known: not installed
    let mut nosum = p("/good.tar.gz", "");
    nosum.sha256 = None;
    assert!(install::install_program(&net, &m, &v, "1.5.0", &nosum, |_| Ok(())).await.is_err());
    assert_eq!(v.current().as_deref(), Some("1.2.0"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn verify_reads_the_version_and_checks_the_json() {
    let d = tmp("verify");
    let bin = d.join("cadenza");
    script(&bin, "echo 'warning: something'\necho '{\"name\":\"cadenza\",\"version\":\"v2.1.0\",\"protocol\":1}'");
    let cadenza = crate::catalog::find("cadenza").unwrap();
    let v = install::verify(cadenza, &bin, &[], &d.join("home")).unwrap();
    assert_eq!(v.version.as_deref(), Some("2.1.0"), "json:version");
    script(&bin, "echo '{\"name\":\"cadenza\",\"version\":\"3.0.0\",\"protocol\":2}'");
    let e = install::verify(cadenza, &bin, &[], &d.join("home")).unwrap_err().to_string();
    assert!(e.contains("another render protocol"), "the check's message: {e}");
    // the environment is the one given, not Encore's
    let m = parse_for(MANIFEST, Origin::Catalog).unwrap();
    script(&bin, "echo \"prog 1.0.0 $LD_LIBRARY_PATH ${CARGO:-none}\"");
    let v = install::verify(&m, &bin, &[("LD_LIBRARY_PATH".into(), "/x/lib".into())], &d.join("home")).unwrap();
    assert!(v.output.stdout.contains("/x/lib none"), "{}", v.output.stdout);
    // a program that hangs is stopped
    let mut slow = m;
    slow.install.get_or_insert_with(Default::default).timeout = Some(1);
    script(&bin, "sleep 30");
    let t = std::time::Instant::now();
    assert!(install::verify(&slow, &bin, &[], &d.join("home")).is_err());
    assert!(t.elapsed() < Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn file_checks_match_within_one_name() {
    let d = tmp("checks");
    let f = d.join("lib/python3.14/site-packages/streamrip");
    std::fs::create_dir_all(&f).unwrap();
    std::fs::write(f.join("config.py"), "no_update_check = True").unwrap();
    let streamrip = crate::catalog::find("streamrip").unwrap();
    let checks = streamrip.install.as_ref().unwrap().checks.clone();
    install::check_files(&checks, &d).unwrap();
    std::fs::write(f.join("config.py"), "check_for_updates = True").unwrap();
    let e = install::check_files(&checks, &d).unwrap_err().to_string();
    assert!(e.contains("no_update_check"), "{e}");
    assert_eq!(install::glob(&d, "lib/python3*/site-packages/streamrip/config.py"), vec![f.join("config.py")]);
    assert!(install::glob(&d, "lib/python2*/x").is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

/// A venv is made by uv where it stays, its program checked, then `current` switches; a failed one is removed.
#[tokio::test]
async fn a_python_program_is_installed_into_a_venv() {
    let d = tmp("python");
    let uv = d.join("fake-uv");
    // a stand-in for uv: `venv … <dir>` makes the folders; `pip install --python <py> <spec>` writes the program,
    // naming the spec it got (so the test sees {version} filled in)
    script(&uv, r#"set -e
case "$1" in
  venv) mkdir -p "$4/bin" "$4/lib/python3.14/site-packages/streamrip"; echo '#!/bin/sh' > "$4/bin/python"; chmod +x "$4/bin/python" ;;
  pip) venv=$(dirname "$(dirname "$4")"); spec="$5"
       printf '#!/bin/sh\necho "rip, version 2.4.9 (%s)"\n' "$spec" > "$venv/bin/rip"; chmod +x "$venv/bin/rip"
       case "$spec" in *v9.9.9*) ;; *) echo 'no_update_check = 1' > "$venv/lib/python3.14/site-packages/streamrip/config.py" ;; esac ;;
esac"#);
    let m = crate::catalog::find("streamrip").unwrap();
    let v = Versions::new(&d.join("plugins/streamrip"));
    let inst = install::install_python(m, &v, "2.4.10", "v2.4.10", &uv).await.unwrap();
    assert_eq!(inst.bin, v.dir("2.4.10").join("venv/bin/rip"));
    assert_eq!(inst.reports.as_deref(), Some("2.4.9"), "the pinned tag still says 2.4.9");
    assert_eq!(v.current().as_deref(), Some("2.4.10"));
    let out = std::process::Command::new(&inst.bin).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("archive/refs/tags/v2.4.10.zip"), "{}", String::from_utf8_lossy(&out.stdout));
    // a version without the switch fails its check and is removed; the one in use stays
    let e = install::install_python(m, &v, "9.9.9", "v9.9.9", &uv).await.unwrap_err().to_string();
    assert!(e.contains("no_update_check"), "{e}");
    assert!(!v.dir("9.9.9").exists());
    assert_eq!(v.current().as_deref(), Some("2.4.10"));
    let _ = std::fs::remove_dir_all(&d);
}

/// uv's answer when a release's Requires-Python rules out the venv's Python (streamrip 2.6.1 on the 3.14 image),
/// and the Python to fetch for a range.
#[test]
fn a_python_requirement_is_read_from_uvs_answer() {
    let uv = "Using Python 3.14.7 environment at: /config/plugins/streamrip/2.6.1/venv\nerror: No solution found when resolving dependencies:\n  ╰─▶ Because the current Python version (3.14.7) does not satisfy Python>=3.15,<3.16 and streamrip==2.6.1 depends on Python>=3.15,<3.16, we can conclude that streamrip==2.6.1 cannot be used.\n      And because only streamrip==2.6.1 is available and you require streamrip, we can conclude that your requirements are unsatisfiable.";
    let need = install::python_mismatch(uv).unwrap();
    assert_eq!(need, install::PythonNeed { found: "3.14.7".into(), wants: ">=3.15,<3.16".into() });
    assert_eq!(install::python_mismatch("error: Failed to fetch: https://github.com/…/v9.zip (404)"), None, "another failure is not a Python one");
    assert_eq!(install::python_for(">=3.15,<3.16").as_deref(), Some("3.15"));
    assert_eq!(install::python_for(">=3.15.2").as_deref(), Some("3.15"));
    assert_eq!(install::python_for("~=3.16").as_deref(), Some("3.16"));
    assert_eq!(install::python_for("==3.15.*").as_deref(), Some("3.15"));
    assert_eq!(install::python_for(">3.14").as_deref(), Some("3.15"), "above 3.14 is the next minor");
    assert_eq!(install::python_for(">=3.9,!=3.10.*,>=3.15").as_deref(), Some("3.15"), "the highest floor counts");
    assert_eq!(install::python_for("<3.14"), None, "an older Python is not fetched");
}

/// A release that needs a newer Python gets it: uv fetches it into the plugins' own Python folder and the venv
/// is made again with it, so the update goes through instead of failing on the image's Python.
#[tokio::test]
async fn a_release_that_needs_a_newer_python_gets_it() {
    let d = tmp("python-newer");
    let uv = python_aware_uv(&d);
    let m = crate::catalog::find("streamrip").unwrap();
    let v = Versions::new(&d.join("plugins/streamrip"));
    install::install_python(m, &v, "2.5.1", "v2.5.1", &uv).await.unwrap();
    let inst = install::install_python(m, &v, "2.6.1", "v2.6.1", &uv).await.unwrap();
    assert_eq!(inst.reports.as_deref(), Some("2.6.1"));
    assert_eq!(v.current().as_deref(), Some("2.6.1"));
    let venv = v.dir("2.6.1").join("venv");
    assert_eq!(std::fs::read_to_string(venv.join("python-asked")).unwrap().trim(), "3.15");
    let own = install::pythons_dir(&v);
    assert_eq!(own, d.join("plugins/.python"));
    assert_eq!(std::fs::read_to_string(venv.join("python-from")).unwrap().trim(), own.display().to_string(), "made with the fetched Python");
    assert!(own.join("3.15").is_dir());
    let _ = std::fs::remove_dir_all(&d);
}

/// No way to get that Python (offline): a typed error the registry turns into "staying on …", and the version in
/// use is untouched.
#[tokio::test]
async fn a_python_that_cannot_be_had_leaves_the_version_in_use() {
    let d = tmp("python-offline");
    let uv = python_aware_uv(&d);
    std::fs::write(d.join("offline"), "").unwrap();
    let m = crate::catalog::find("streamrip").unwrap();
    let v = Versions::new(&d.join("plugins/streamrip"));
    install::install_python(m, &v, "2.5.1", "v2.5.1", &uv).await.unwrap();
    let e = install::install_python(m, &v, "2.6.1", "v2.6.1", &uv).await.unwrap_err();
    let need = e.downcast_ref::<install::PythonUnavailable>().expect("a typed error");
    assert_eq!((need.version.as_str(), need.python.as_str(), need.wants.as_str()), ("2.6.1", "3.15", ">=3.15,<3.16"));
    assert!(need.why.contains("network unreachable"), "{}", need.why);
    assert_eq!(v.current().as_deref(), Some("2.5.1"));
    assert!(!v.dir("2.6.1").exists());
    assert!(v.dir("2.5.1").join("venv/bin/rip").is_file());
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn a_data_file_is_checked_by_sum_and_magic() {
    let s = StandIn::start().await;
    let d = tmp("data");
    let net = Net { http: fetch::client("test", true), allow_http: true };
    s.put("/model.gguf", b"GGUF\x03rest".to_vec());
    s.put("/html", b"<html>".to_vec());
    let dest = d.join("models/m.gguf");
    assert!(install::data_file(&net, &s.url("/model.gguf"), &dest, Some(&sha(b"GGUF\x03rest")), Some("GGUF"), 1 << 20).await.unwrap());
    assert!(!install::data_file(&net, &s.url("/model.gguf"), &dest, None, Some("GGUF"), 1 << 20).await.unwrap(), "a whole file is kept");
    std::fs::remove_file(&dest).unwrap();
    assert!(install::data_file(&net, &s.url("/model.gguf"), &dest, Some(&"0".repeat(64)), Some("GGUF"), 1 << 20).await.is_err());
    assert!(!dest.exists(), "a file that does not match is deleted");
    assert!(install::data_file(&net, &s.url("/html"), &dest, None, Some("GGUF"), 1 << 20).await.is_err());
    assert!(!dest.exists());
    let _ = std::fs::remove_dir_all(&d);
}
