//! The registry: the plugins table, installing an uploaded zip (the test plugin), its settings, enable and
//! disable, remove; a catalog program from a stand-in GitHub; an update from a release with its sums and a
//! broken update rolled back; install from a URL (a zip, a raw plugin.toml, owner/repo@ref); refusals.
use crate::manifest::Origin;
use crate::registry::{github_ref, Registry};
use crate::releases::Hosts;
use crate::sandbox::Sandbox;
use crate::test_util::{python, tar_gz, file, tone_zip, zip_of, StandIn};
use encore_db::{p, RowExt};
use serde_json::json;
use std::path::PathBuf;

struct Env {
    dir: PathBuf,
    reg: Registry,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env(name: &str, sandbox: Sandbox) -> Env {
    let dir = encore_core::tmp::dir(&format!("registry-{name}"));
    let db = encore_db::Db::open(&dir).unwrap();
    let cfg = encore_config::Config::load_with(db.clone(), |_| None).unwrap();
    let reg = Registry::new(&dir, db, cfg);
    reg.set_sandbox(sandbox);
    reg.allow_plain_http();
    Env { dir, reg }
}

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn github_refs_are_read() {
    assert_eq!(github_ref("Krifmaz/Encore@main"), Some(("Krifmaz/Encore".into(), None, "main".into())));
    assert_eq!(github_ref("Krifmaz/Encore/plugins/internet-archive@internet-archive-v0.1.0"), Some(("Krifmaz/Encore".into(), Some("plugins/internet-archive".into()), "internet-archive-v0.1.0".into())));
    assert_eq!(github_ref("o/r@refs/tags/v1"), Some(("o/r".into(), None, "refs/tags/v1".into())));
    for bad in ["https://github.com/o/r@main", "o@main", "o/r", "o/../r@main", "o/r@../x", "o/r@", "o r/x@y"] {
        assert_eq!(github_ref(bad), None, "{bad}");
    }
}

#[tokio::test]
async fn a_fresh_install_lists_the_catalog_with_nothing_installed() {
    let e = env("fresh", Sandbox::limits_only());
    let l = e.reg.list();
    let plugins = l["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), crate::catalog::FILES.len());
    assert!(plugins.iter().all(|p| p["installed"] == false && p["enabled"] == false && p["origin"] == "catalog" && p["trust"] == "first-party"));
    assert_eq!(l["sandbox"]["level"], "limits");
    assert_eq!(plugins[0]["id"], "ytdlp");
    assert_eq!(plugins[0]["sources"][0]["picker"], "ytmusic");
    // a catalog manifest's version is Encore's, not the program's: nothing is shown until one is known
    assert!(plugins.iter().all(|p| p["version"].is_null()), "{plugins:?}");
    assert!(e.reg.rows().is_empty());
    // a catalog plugin without a program is installed as a row, switched on
    let c = e.reg.install("torznab").await.unwrap();
    assert_eq!((c["installed"].as_bool(), c["enabled"].as_bool(), c["installed_version"].is_null(), c["version"].is_null()), (Some(true), Some(true), true, true));
    let st = e.reg.status("torznab").await.unwrap();
    assert_eq!(st["ok"], true);
    // its settings are Encore's own keys, drawn from the settings table
    let s = e.reg.settings("torznab").unwrap();
    assert!(s["builtin"].as_bool().unwrap());
    assert!(s["fields"].as_array().unwrap().iter().any(|f| f["key"] == "prowlarr_url" && f["label"].is_string()));
    e.reg.save_settings("torznab", &json!({ "prowlarr_url": "http://prowlarr:9696" }).as_object().unwrap().clone()).unwrap();
    assert_eq!(e.reg.values(&e.reg.manifest("torznab").unwrap())["prowlarr_url"], "http://prowlarr:9696");
    assert!(e.reg.save_settings("torznab", &json!({ "slskd_enabled": true }).as_object().unwrap().clone()).is_err(), "a key of another plugin");
    e.reg.remove("torznab").await.unwrap();
    assert!(!e.reg.installed("torznab"));
}

/// The test plugin through the upload path, at every level: unpacked, checked, described, installed; its
/// settings (a secret masked), status, test, search and download; disable stops it; remove takes everything.
#[tokio::test]
async fn an_uploaded_plugin_installs_runs_and_goes_away() {
    if python().is_none() {
        return;
    }
    for level in crate::test_util::levels() {
        let e = env("upload", level.clone());
        let zip = write(&e.dir, "tone.zip", &tone_zip(Some("test-tone-0.1.0"), |t| t));
        let card = e.reg.upload(&zip).await.unwrap_or_else(|r| panic!("{:?}: {}", level.level, r.message));
        assert!(!zip.exists(), "the upload is removed");
        assert_eq!((card["id"].as_str(), card["origin"].as_str(), card["trust"].as_str()), (Some("test-tone"), Some("upload"), Some("third-party")));
        assert_eq!((card["installed"].as_bool(), card["enabled"].as_bool(), card["installed_version"].as_str()), (Some(true), Some(true), Some("0.1.0")));
        assert!(card["warning"].as_str().unwrap().contains("Third-party plugin"));
        let row = e.reg.row("test-tone").unwrap();
        assert_eq!(row.s("sha256").len(), 64);
        assert!(row.s("manifest").contains("id = \"test-tone\""));
        let root = e.dir.join("plugins/test-tone");
        assert!(root.join("0.1.0/plugin.py").is_file() && root.join("plugin.toml").is_file());
        assert_eq!(std::fs::read_to_string(root.join("current")).unwrap().trim(), "0.1.0");
        // settings: kept as plugin.<id>.<key>, checked against the field, a secret never shown
        let s = e.reg.settings("test-tone").unwrap();
        assert_eq!(s["fields"].as_array().unwrap().len(), 4);
        assert_eq!(s["values"]["seconds"], 1);
        e.reg.save_settings("test-tone", &json!({ "seconds": 2, "rate": "48000", "token": "tok-123" }).as_object().unwrap().clone()).unwrap();
        let s = e.reg.settings("test-tone").unwrap();
        assert_eq!((s["values"]["seconds"].clone(), s["values"]["rate"].clone()), (json!(2), json!("48000")));
        assert_eq!(s["values"]["token"], format!("{} (saved)", crate::registry::MASK));
        assert!(e.reg.save_settings("test-tone", &json!({ "seconds": 99 }).as_object().unwrap().clone()).is_err(), "out of range");
        assert!(e.reg.save_settings("test-tone", &json!({ "rate": "1" }).as_object().unwrap().clone()).is_err(), "not an option");
        e.reg.save_settings("test-tone", &json!({ "token": format!("{} (saved)", crate::registry::MASK) }).as_object().unwrap().clone()).unwrap();
        let stored = e.reg.values(&e.reg.manifest("test-tone").unwrap());
        assert_eq!(stored["token"], "tok-123", "a masked value leaves the secret as it is");
        // status, test, search, download
        let st = e.reg.status("test-tone").await.unwrap();
        assert_eq!(st["message"], "Ready · three test tones");
        let t = e.reg.test("test-tone").await.unwrap();
        assert!(t["ok"].as_bool().unwrap() && t["message"].as_str().unwrap().contains("in env: False"), "{t}");
        let results = e.reg.search("test-tone", &json!({ "kind": "album", "artist": "Test Tones", "album": "Tuning" }), None).await.unwrap();
        assert_eq!(results.len(), 3);
        let folder = e.dir.join("downloads/request-1/plugin");
        let got = e.reg.download("test-tone", &json!({ "key": "tone:e330", "title": "E 330", "payload": results[2]["payload"] }), &folder, None).await.unwrap();
        assert!(got.files[0].path.starts_with(std::fs::canonicalize(&folder).unwrap()));
        let logs = e.reg.logs("test-tone", 100).unwrap();
        assert!(logs["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("[install] installed 0.1.0")), "{logs}");
        assert!(!logs.to_string().contains("tok-123"), "no secret in the log");
        // switched off: no search
        e.reg.enable("test-tone", false).await.unwrap();
        assert!(e.reg.search("test-tone", &json!({ "kind": "track", "title": "A" }), None).await.is_err());
        e.reg.enable("test-tone", true).await.unwrap();
        // the same zip again: an update in place to the same version, settings kept
        let zip = write(&e.dir, "tone.zip", &tone_zip(None, |t| t));
        e.reg.upload(&zip).await.unwrap();
        assert_eq!(e.reg.values(&e.reg.manifest("test-tone").unwrap())["seconds"], 2);
        // remove: its folder, row and settings go
        e.reg.remove("test-tone").await.unwrap();
        assert!(!root.exists());
        assert!(e.reg.row("test-tone").is_none());
        assert_eq!(e.reg.manifest("test-tone"), None);
        let n = e.reg.list()["plugins"].as_array().unwrap().len();
        assert_eq!(n, crate::catalog::FILES.len());
    }
}

#[tokio::test]
async fn bad_uploads_are_refused_and_leave_nothing() {
    let e = env("bad", Sandbox::limits_only());
    let cases: Vec<(&str, Vec<u8>, u16, &str)> = vec![
        ("no manifest", zip_of(&[("README", b"hi", 0o644)]), 400, "no plugin.toml"),
        ("not a zip", b"just text".to_vec(), 400, "Not a zip"),
        ("leaves its folder", zip_of(&[("plugin.toml", b"x", 0o644), ("../evil", b"x", 0o644)]), 400, "leaves its folder"),
        ("unknown key", tone_zip(None, |t| t.replace("[run]\n", "[run]\nshel = true\n")), 422, "unknown field"),
        ("shell", tone_zip(None, |t| t.replace("[run]\n", "[run]\nshell = true\n")), 422, "shell"),
        ("builtin", tone_zip(None, |t| t.replace("needs_sign_in = false", "needs_sign_in = false\nbuiltin = true")), 422, "builtin"),
        ("catalog id", tone_zip(None, |t| t.replace("id = \"test-tone\"", "id = \"ytdlp\"")), 409, "Encore ships"),
        ("source id taken", tone_zip(None, |t| t.replace("[run]\n", "[[sources]]\nid = \"deezer\"\nname = \"Deezer\"\n\n[run]\n")), 409, "deezer"),
        ("reserved id", tone_zip(None, |t| t.replace("id = \"test-tone\"", "id = \"upload\"")), 400, "another id"),
        ("too new", tone_zip(None, |t| t.replace("needs_sign_in = false", "needs_sign_in = false\nmin_encore = \"999.0.0\"")), 400, "needs Encore 999.0.0"),
        ("no program", zip_of(&[("plugin.toml", &std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test-tone/plugin.toml")).unwrap(), 0o644)]), 400, "not in the zip"),
    ];
    for (name, bytes, status, words) in cases {
        let zip = write(&e.dir, "bad.zip", &bytes);
        let r = e.reg.upload(&zip).await.unwrap_err();
        assert_eq!(r.status, status, "{name}: {}", r.message);
        assert!(r.message.contains(words) || r.problems.iter().any(|p| p.contains(words)), "{name}: {} {:?}", r.message, r.problems);
    }
    assert!(e.reg.rows().is_empty());
    assert!(!e.dir.join("plugins/test-tone/0.1.0").exists());
    assert!(!e.dir.join("evil").exists() && !e.dir.join("plugins/evil").exists());
    let incoming = e.dir.join("plugins/.incoming");
    assert!(std::fs::read_dir(&incoming).map(|r| r.count()).unwrap_or(0) == 0, "no scratch left");
}

/// The plugin's folder is moved out of the archive on its own: a link that stayed inside the archive but leaves
/// that folder (or a folder that is itself a link) is refused, so nothing is written or bound outside it.
#[tokio::test]
async fn links_that_leave_the_plugins_folder_are_refused() {
    let e = env("links", Sandbox::limits_only());
    let files = crate::test_util::tone_files(|t| t);
    let build = |extra: &[(&str, &[u8], u32)], top: &str| {
        let names: Vec<String> = files.iter().map(|(n, _)| format!("{top}/{n}")).collect();
        let mut entries: Vec<(&str, &[u8], u32)> = files.iter().zip(&names).map(|((_, d), n)| (n.as_str(), d.as_slice(), 0o644)).collect();
        entries.extend_from_slice(extra);
        zip_of(&entries)
    };
    // top/a/b is the plugin; its `program` points at the archive's root, three folders up
    let zip = write(&e.dir, "l.zip", &build(&[("top/a/b/program", b"../../..", 0o120777)], "top/a/b"));
    let r = e.reg.install_archive(&zip, Origin::Url, None, Some("a/b"), None).await.unwrap_err();
    assert_eq!(r.status, 400, "{}", r.message);
    assert!(r.message.contains("leaves the plugin's folder"), "{}", r.message);
    // the plugin's folder is itself a link to a folder of the archive
    let zip = write(&e.dir, "l.zip", &build(&[("top/l", b"real", 0o120777)], "top/real"));
    let r = e.reg.install_archive(&zip, Origin::Url, None, Some("l"), None).await.unwrap_err();
    assert!(r.message.contains("is a link"), "{}", r.message);
    // a link that stays inside the plugin's folder is kept as it was
    let zip = write(&e.dir, "l.zip", &build(&[("top/a/b/docs/readme", b"../plugin.py", 0o120777)], "top/a/b"));
    match e.reg.install_archive(&zip, Origin::Url, None, Some("a/b"), None).await {
        Ok(_) => {}
        Err(r) => assert!(!r.message.contains("link"), "{}", r.message),
    }
    assert!(!e.dir.join("plugins/test-tone/0.1.0/program").exists());
}

/// A program whose `describe` does not fit its manifest is not installed; an update that fails it is not
/// switched to (the version in use stays).
#[tokio::test]
async fn describe_decides_and_a_broken_update_rolls_back() {
    if python().is_none() {
        return;
    }
    let e = env("describe", Sandbox::limits_only());
    // the manifest says 0.2.0, the program says 0.1.0
    let zip = write(&e.dir, "a.zip", &tone_zip(None, |t| t.replace("version = \"0.1.0\"", "version = \"0.2.0\"")));
    let r = e.reg.upload(&zip).await.unwrap_err();
    assert!(r.message.contains("version"), "{}", r.message);
    assert!(e.reg.row("test-tone").is_none());
    // a good one, then a broken 0.1.0 (its program does not start): the good files stay
    let zip = write(&e.dir, "a.zip", &tone_zip(None, |t| t));
    e.reg.upload(&zip).await.unwrap();
    let broken = zip_of(&[
        ("plugin.toml", &std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test-tone/plugin.toml")).unwrap(), 0o644),
        ("plugin.py", b"import sys\nsys.exit(3)\n", 0o644),
    ]);
    let zip = write(&e.dir, "b.zip", &broken);
    let r = e.reg.upload(&zip).await.unwrap_err();
    assert!(r.message.contains("does not start here"), "{}", r.message);
    assert!(std::fs::read_to_string(e.dir.join("plugins/test-tone/0.1.0/plugin.py")).unwrap().contains("TONES"), "the working version stays");
    assert_eq!(e.reg.status("test-tone").await.unwrap()["ok"], true);
}

/// A catalog program (Cadenza's manifest) installed from a stand-in GitHub: the newest release fitting the tag,
/// its asset for this machine, the sha256 from SHA256SUMS, the version command's JSON and protocol check.
#[tokio::test]
async fn a_catalog_program_installs_from_its_releases() {
    let arch = std::env::consts::ARCH;
    if !matches!(arch, "x86_64" | "aarch64") {
        return;
    }
    let s = StandIn::start().await;
    let e = env("catalog", Sandbox::limits_only());
    e.reg.set_hosts(Hosts { github_api: s.base.clone(), github: s.base.clone(), pypi: s.base.clone(), huggingface: s.base.clone() });
    let program = |v: &str, proto: u32| format!("#!/bin/sh\necho '{{\"name\":\"cadenza\",\"version\":\"{v}\",\"protocol\":{proto}}}'\n").into_bytes();
    let release = |v: &str, proto: u32| {
        let asset = format!("cadenza-{v}-linux-{arch}.tar.gz");
        let archive = tar_gz(&[file("cadenza", &program(v, proto), 0o755)]);
        s.put(&format!("/dl/{asset}"), archive.clone());
        s.put(&format!("/dl/{v}/SHA256SUMS"), format!("{}  {asset}\n", encore_core::ids::sha256_hex(&archive)));
        json!({ "tag_name": format!("v{v}"), "draft": false, "prerelease": false, "assets": [
            { "name": asset, "browser_download_url": s.url(&format!("/dl/{asset}")) },
            { "name": "SHA256SUMS", "browser_download_url": s.url(&format!("/dl/{v}/SHA256SUMS")) },
        ] })
    };
    let r1 = release("2.1.0", 1);
    let other = json!({ "tag_name": "something-else", "assets": [] });
    s.put("/repos/Krifmaz/Cadenza/releases?per_page=50", serde_json::to_vec(&json!([other, r1])).unwrap());
    s.put("/repos/Krifmaz/Cadenza/releases/tags/v2.1.0", serde_json::to_vec(&r1).unwrap());
    let card = e.reg.install("cadenza").await.unwrap_or_else(|r| panic!("{}", r.message));
    assert_eq!(card["installed_version"], "2.1.0");
    assert!(e.dir.join("plugins/cadenza/2.1.0/cadenza").is_file());
    assert_eq!(e.reg.program("cadenza"), Some(("2.1.0".to_string(), e.dir.join("plugins/cadenza/2.1.0/cadenza"))));
    assert_eq!(e.reg.program("slskd"), None);
    let t = e.reg.test("cadenza").await.unwrap();
    assert!(t["message"].as_str().unwrap().ends_with("2.1.0 runs"), "{t}");
    // a newer release that speaks another protocol: the update fails, 2.1.0 stays
    let r2 = release("2.2.0", 2);
    s.put("/repos/Krifmaz/Cadenza/releases?per_page=50", serde_json::to_vec(&json!([r2, r1])).unwrap());
    s.put("/repos/Krifmaz/Cadenza/releases/tags/v2.2.0", serde_json::to_vec(&r2).unwrap());
    let c = e.reg.check("cadenza").await.unwrap();
    assert_eq!((c["latest"].as_str(), c["update_available"].as_bool()), (Some("2.2.0"), Some(true)));
    let r = e.reg.update("cadenza").await.unwrap_err();
    assert!(r.message.contains("another render protocol"), "{}", r.message);
    assert_eq!(e.reg.row("cadenza").unwrap().s("version"), "2.1.0");
    assert!(e.reg.row("cadenza").unwrap().s("last_error").contains("Update failed"));
    assert!(e.dir.join("plugins/cadenza/2.1.0/cadenza").is_file() && !e.dir.join("plugins/cadenza/2.2.0").exists());
    // a sum that does not match: refused
    let r3 = release("2.3.0", 1);
    s.put("/dl/2.3.0/SHA256SUMS", format!("{}  cadenza-2.3.0-linux-{arch}.tar.gz\n", "0".repeat(64)));
    s.put("/repos/Krifmaz/Cadenza/releases?per_page=50", serde_json::to_vec(&json!([r3])).unwrap());
    s.put("/repos/Krifmaz/Cadenza/releases/tags/v2.3.0", serde_json::to_vec(&r3).unwrap());
    let r = e.reg.update("cadenza").await.unwrap_err();
    assert!(r.message.contains("does not match"), "{}", r.message);
    assert_eq!(e.reg.row("cadenza").unwrap().s("version"), "2.1.0");
    // remove: the plugin's folder goes, Encore's settings stay
    e.reg.remove("cadenza").await.unwrap();
    assert!(!e.dir.join("plugins/cadenza").exists());
}

/// Install from a URL: a zip, a raw plugin.toml whose [update] names the release zip and its sums, and
/// owner/repo@ref (GitHub's archive zip, the plugin in a folder of the repository). Only https in a real build.
#[tokio::test]
async fn plugins_install_from_urls() {
    if python().is_none() {
        return;
    }
    let s = StandIn::start().await;
    let e = env("url", Sandbox::limits_only());
    e.reg.set_hosts(Hosts { github_api: s.base.clone(), github: s.base.clone(), pypi: s.base.clone(), huggingface: s.base.clone() });
    // a zip
    s.put("/files/tone.zip", tone_zip(Some("tone"), |t| t));
    let c = e.reg.install_url(&s.url("/files/tone.zip")).await.unwrap_or_else(|r| panic!("{}", r.message));
    assert_eq!((c["origin"].as_str(), c["trust"].as_str()), (Some("url"), Some("third-party")));
    e.reg.remove("test-tone").await.unwrap();
    // owner/repo/folder@ref: GitHub's archive of the ref, one top folder, the plugin in plugins/test-tone
    let files = crate::test_util::tone_files(|t| t);
    let names: Vec<String> = files.iter().map(|(n, _)| format!("Encore-main/plugins/test-tone/{n}")).collect();
    let mut entries: Vec<(&str, &[u8], u32)> = files.iter().zip(&names).map(|((_, d), n)| (n.as_str(), d.as_slice(), 0o644)).collect();
    entries.push(("Encore-main/README.md", b"repo", 0o644));
    s.put("/Krifmaz/Encore/archive/main.zip", zip_of(&entries));
    let c = e.reg.install_url("Krifmaz/Encore/plugins/test-tone@main").await.unwrap_or_else(|r| panic!("{}", r.message));
    assert_eq!(c["id"], "test-tone");
    e.reg.remove("test-tone").await.unwrap();
    let r = e.reg.install_url("Krifmaz/Encore@main").await.unwrap_err();
    assert!(r.message.contains("no plugin.toml"), "{}", r.message);
    // a raw plugin.toml: the newest release fitting its tag, the zip checked against SHA256SUMS
    let with_update = |t: String| format!("{t}\n[update]\ngithub = \"Krifmaz/Encore\"\ntag = \"test-tone-v{{version}}\"\nasset = \"test-tone-{{version}}.zip\"\nsums = \"SHA256SUMS\"\n");
    let zip = tone_zip(None, with_update);
    s.put("/dl/test-tone-0.1.0.zip", zip.clone());
    s.put("/dl/SHA256SUMS", format!("{}  test-tone-0.1.0.zip\n", encore_core::ids::sha256_hex(&zip)));
    let rel = json!({ "tag_name": "test-tone-v0.1.0", "assets": [
        { "name": "test-tone-0.1.0.zip", "browser_download_url": s.url("/dl/test-tone-0.1.0.zip") },
        { "name": "SHA256SUMS", "browser_download_url": s.url("/dl/SHA256SUMS") } ] });
    s.put("/repos/Krifmaz/Encore/releases?per_page=50", serde_json::to_vec(&json!([{ "tag_name": "v0.31.0", "assets": [] }, rel.clone()])).unwrap());
    let manifest = String::from_utf8(crate::test_util::tone_files(with_update)[0].1.clone()).unwrap();
    s.put("/raw/plugin.toml", manifest);
    let c = e.reg.install_url(&s.url("/raw/plugin.toml")).await.unwrap_or_else(|r| panic!("{}", r.message));
    assert_eq!(c["installed_version"], "0.1.0");
    assert_eq!(e.reg.row("test-tone").unwrap().s("sha256"), encore_core::ids::sha256_hex(&zip));
    // its update: the same version is not installed again
    let zips = s.hits_of("/dl/test-tone-0.1.0.zip");
    let c = e.reg.update("test-tone").await.unwrap();
    assert_eq!(c["installed_version"], "0.1.0");
    assert_eq!(s.hits_of("/dl/test-tone-0.1.0.zip"), zips, "nothing downloaded for the version it has");
    // 0.2.0 asks for the network: the scheduled task's update refuses it and leaves it for the admin
    let wider = |t: String| with_update(t.replace("version = \"0.1.0\"", "version = \"0.2.0\"").replace("network = false", "network = [\"example.com\"]"));
    let zip2 = tone_zip(None, wider);
    s.put("/dl/test-tone-0.2.0.zip", zip2.clone());
    s.put("/dl/SHA256SUMS2", format!("{}  test-tone-0.2.0.zip\n", encore_core::ids::sha256_hex(&zip2)));
    let rel2 = json!({ "tag_name": "test-tone-v0.2.0", "assets": [
        { "name": "test-tone-0.2.0.zip", "browser_download_url": s.url("/dl/test-tone-0.2.0.zip") },
        { "name": "SHA256SUMS", "browser_download_url": s.url("/dl/SHA256SUMS2") } ] });
    s.put("/repos/Krifmaz/Encore/releases?per_page=50", serde_json::to_vec(&json!([rel2.clone()])).unwrap());
    s.put("/repos/Krifmaz/Encore/releases/tags/test-tone-v0.2.0", serde_json::to_vec(&rel2).unwrap());
    let r = e.reg.update_unattended("test-tone").await.unwrap_err();
    assert_eq!(r.status, 409, "{}", r.message);
    assert!(r.message.contains("asks for more than the installed version: the network (example.com)"), "{}", r.message);
    let row = e.reg.row("test-tone").unwrap();
    assert_eq!(row.s("version"), "0.1.0", "the accepted version stays");
    assert!(row.s("last_error").contains("update it by hand"), "the card says so: {}", row.s("last_error"));
    s.put("/repos/Krifmaz/Encore/releases?per_page=50", serde_json::to_vec(&json!([rel])).unwrap());
    // a sums file that does not match: refused, the installed one stays
    e.reg.remove("test-tone").await.unwrap();
    s.put("/dl/SHA256SUMS", format!("{}  test-tone-0.1.0.zip\n", "0".repeat(64)));
    let r = e.reg.install_url(&s.url("/raw/plugin.toml")).await.unwrap_err();
    assert!(r.message.contains("does not match"), "{}", r.message);
    // neither a zip nor a manifest
    s.put("/page.html", "<html>hello</html>");
    let r = e.reg.install_url(&s.url("/page.html")).await.unwrap_err();
    assert!(r.message.contains("neither a zip nor a plugin.toml"), "{}", r.message);
    assert_eq!(e.reg.rows().len(), 0);
}

#[test]
fn origins_are_kept_in_the_table() {
    let e = env("origins", Sandbox::limits_only());
    e.reg.set_sandbox(Sandbox::limits_only());
    assert_eq!(Origin::Upload.trust(), "third-party");
    // a plugin row the registry did not write (a manifest that no longer reads) is left out of the list
    let db = encore_db::Db::open(&e.dir).unwrap();
    db.execute("INSERT INTO plugins (id, version, origin, trust, manifest) VALUES ('broken', '1.0.0', 'upload', 'third-party', 'not toml')", p![]).unwrap();
    let ids: Vec<String> = e.reg.list()["plugins"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    assert!(!ids.contains(&"broken".to_string()));
}
