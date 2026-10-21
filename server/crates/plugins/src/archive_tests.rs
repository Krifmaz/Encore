//! Unpacking: names, links, modes, caps, both formats; the program found at any depth.
use crate::archive::{entry_path, find_bin, link_ok, unpack, Kind, Limits};
use crate::test_util::{file, link, tar_gz, zip_of, TarEntry};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    encore_core::tmp::dir(&format!("archive-{name}"))
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o7777
}

#[test]
fn names_stay_inside() {
    assert_eq!(entry_path("./build/bin/x").unwrap(), Some(PathBuf::from("build/bin/x")));
    assert_eq!(entry_path("a/./b").unwrap(), Some(PathBuf::from("a/b")));
    assert_eq!(entry_path("./").unwrap(), None);
    for bad in ["../etc/passwd", "a/../../b", "a/../b", "/etc/passwd", "a\0b"] {
        assert!(entry_path(bad).is_err(), "{bad}");
    }
    // a link: relative, `..` only first and no deeper than the link
    assert!(link_ok(Path::new("lib/libx.so"), "libx.so.1"));
    assert!(link_ok(Path::new("bin/x"), "../lib/libx.so"));
    assert!(link_ok(Path::new("a/b/c"), "../../d"));
    assert!(!link_ok(Path::new("bin/x"), "../../etc/passwd"));
    assert!(!link_ok(Path::new("x"), ".."));
    assert!(!link_ok(Path::new("x"), "/etc/passwd"));
    assert!(!link_ok(Path::new("a/b/x"), "c/../../../d"), "`..` after a name is refused, whatever it resolves to");
    assert!(!link_ok(Path::new("x"), ""));
}

#[test]
fn a_zip_unpacks_with_its_modes_and_links_inside() {
    let d = tmp("zip");
    let z = write(&d, "a.zip", &zip_of(&[
        ("bin/", b"", 0o755),
        ("bin/prog", b"#!/bin/sh\necho hi\n", 0o4755),
        ("lib/libx.so.1", b"lib", 0o666),
        ("lib/libx.so", b"libx.so.1", 0o120777),
        ("bin/lib", b"../lib", 0o120777),
        ("README", b"read me", 0o600),
    ]));
    let out = d.join("out");
    let u = unpack(&z, Kind::Zip, &out, Limits::default()).unwrap();
    assert_eq!(u.files, 3);
    assert_eq!(mode(&out.join("bin/prog")), 0o755, "the execute bit only, never set-user-id");
    assert_eq!(mode(&out.join("lib/libx.so.1")), 0o644, "nothing writable by others");
    assert_eq!(mode(&out.join("README")), 0o644);
    assert_eq!(std::fs::read(out.join("lib/libx.so")).unwrap(), b"lib");
    assert_eq!(std::fs::read(out.join("bin/lib/libx.so.1")).unwrap(), b"lib");
    assert_eq!(find_bin(&out, "prog"), Some(out.join("bin/prog")));
    assert_eq!(find_bin(&out, "bin/prog"), Some(out.join("bin/prog")));
    assert_eq!(find_bin(&out, "missing"), None);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_zip_that_leaves_its_folder_is_refused() {
    let d = tmp("zip-evil");
    for (name, entries) in [
        ("dotdot", vec![("ok", &b"1"[..], 0o644), ("../evil", &b"x"[..], 0o644)]),
        ("absolute", vec![("/tmp/evil-encore-test", &b"x"[..], 0o644)]),
        ("link-out", vec![("out", &b"../../.."[..], 0o120777)]),
        ("link-abs", vec![("out", &b"/etc"[..], 0o120777)]),
        // a link inside, then a file written through it
        ("through", vec![("sub/", &b""[..], 0o755), ("l", &b"sub"[..], 0o120777), ("l/x", &b"x"[..], 0o644)]),
    ] {
        let z = write(&d, &format!("{name}.zip"), &zip_of(&entries));
        let out = d.join(name);
        let e = unpack(&z, Kind::Zip, &out, Limits::default()).unwrap_err().to_string();
        assert!(e.contains("leaves its folder") || e.contains("absolute") || e.contains("points outside") || e.contains("through a link"), "{name}: {e}");
    }
    assert!(!d.join("evil").exists());
    assert!(!Path::new("/tmp/evil-encore-test").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn caps_stop_an_archive_that_grows_too_large() {
    let d = tmp("caps");
    let big = vec![0u8; 300_000];
    let z = write(&d, "big.zip", &zip_of(&[("a", &big, 0o644), ("b", &big, 0o644)]));
    let e = unpack(&z, Kind::Zip, &d.join("o1"), Limits { bytes: 400_000, entries: 100 }).unwrap_err().to_string();
    assert!(e.contains("more than"), "{e}");
    let e = unpack(&z, Kind::Zip, &d.join("o2"), Limits { bytes: 1 << 30, entries: 1 }).unwrap_err().to_string();
    assert!(e.contains("entries"), "{e}");
    let t = write(&d, "big.tar.gz", &tar_gz(&[file("a", &big, 0o644), file("b", &big, 0o644)]));
    let e = unpack(&t, Kind::TarGz, &d.join("o3"), Limits { bytes: 400_000, entries: 100 }).unwrap_err().to_string();
    assert!(e.contains("more than"), "{e}");
    assert!(unpack(&t, Kind::TarGz, &d.join("o4"), Limits::default()).is_ok());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_tar_gz_unpacks_files_links_and_long_names() {
    let d = tmp("tar");
    let long = format!("deep/{}/prog", "x".repeat(120));
    let t = write(&d, "a.tar.gz", &tar_gz(&[
        file("build/bin/llama-server", b"#!/bin/sh\necho version: 1\n", 0o755),
        file("build/bin/libllama.so.0", b"lib", 0o644),
        link("build/bin/libllama.so", "libllama.so.0"),
        file(&long, b"long", 0o644),
        TarEntry { name: "build/copy", data: b"", mode: 0o644, kind: b'1', link: "build/bin/libllama.so.0" },
    ]));
    let out = d.join("out");
    unpack(&t, Kind::TarGz, &out, Limits::default()).unwrap();
    assert_eq!(mode(&out.join("build/bin/llama-server")), 0o755);
    assert_eq!(std::fs::read(out.join("build/bin/libllama.so")).unwrap(), b"lib");
    assert_eq!(std::fs::read(out.join(&long)).unwrap(), b"long", "a GNU long name");
    assert_eq!(std::fs::read(out.join("build/copy")).unwrap(), b"lib", "a hard link is a copy");
    assert_eq!(find_bin(&out, "llama-server"), Some(out.join("build/bin/llama-server")));
    // the system's tar (GNU or bsdtar) makes one we read too
    let src = d.join("src/cadenza-2.1.0");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("cadenza"), b"real tar").unwrap();
    let made = std::process::Command::new("tar").arg("-czf").arg(d.join("sys.tar.gz")).arg("-C").arg(d.join("src")).arg("cadenza-2.1.0").status();
    if made.is_ok_and(|s| s.success()) {
        let o = d.join("sys");
        unpack(&d.join("sys.tar.gz"), Kind::TarGz, &o, Limits::default()).unwrap();
        assert_eq!(std::fs::read(find_bin(&o, "cadenza").unwrap()).unwrap(), b"real tar");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_tar_gz_that_leaves_its_folder_or_holds_devices_is_refused() {
    let d = tmp("tar-evil");
    let cases: Vec<(&str, Vec<TarEntry>)> = vec![
        ("dotdot", vec![file("../evil", b"x", 0o644)]),
        ("absolute", vec![file("/tmp/evil-encore-tar", b"x", 0o644)]),
        ("link-out", vec![link("l", "../../etc")]),
        ("through", vec![TarEntry { name: "sub", data: b"", mode: 0o755, kind: b'5', link: "" }, link("l", "sub"), file("l/x", b"x", 0o644)]),
        ("device", vec![TarEntry { name: "dev", data: b"", mode: 0o644, kind: b'3', link: "" }]),
        ("hardlink-missing", vec![TarEntry { name: "h", data: b"", mode: 0o644, kind: b'1', link: "nothing" }]),
    ];
    for (name, entries) in cases {
        let t = write(&d, &format!("{name}.tar.gz"), &tar_gz(&entries));
        assert!(unpack(&t, Kind::TarGz, &d.join(name), Limits::default()).is_err(), "{name}");
    }
    assert!(!d.join("evil").exists() && !Path::new("/tmp/evil-encore-tar").exists());
    // not an archive at all
    let junk = write(&d, "junk.tar.gz", b"not gzip");
    assert!(unpack(&junk, Kind::TarGz, &d.join("junk"), Limits::default()).is_err());
    let junk = write(&d, "junk.zip", b"PK\x03\x04 broken");
    assert!(unpack(&junk, Kind::Zip, &d.join("junkz"), Limits::default()).is_err());
    assert_eq!(Kind::sniff(&junk), Some(Kind::Zip));
    assert_eq!(Kind::of_name("llama-b1-bin-ubuntu-x64.tar.gz?x=1"), Some(Kind::TarGz));
    assert_eq!(Kind::of_name("slskd-0.24.1-linux-x64.zip"), Some(Kind::Zip));
    assert_eq!(Kind::of_name("program"), None);
    let _ = std::fs::remove_dir_all(&d);
}
