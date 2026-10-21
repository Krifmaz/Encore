//! The first start after an upgrade from 0.30 (no plugins yet): what was in use is adopted where it is and
//! switched on, its settings untouched, its old auto-update switch carried over, and a program that was in use
//! but is not there (the image no longer ships it) installed; a fresh install adopts nothing.
use crate::migrate::adopted_program;
use crate::releases::Hosts;
use crate::registry::Registry;
use crate::sandbox::Sandbox;
use crate::test_util::{script, StandIn};
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Env {
    dir: PathBuf,
    reg: Registry,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn open(dir: &Path) -> Registry {
    let db = encore_db::Db::open(dir).unwrap();
    let cfg = encore_config::Config::load_with(db.clone(), |_| None).unwrap();
    let reg = Registry::new(dir, db, cfg);
    reg.set_sandbox(Sandbox::limits_only());
    reg.allow_plain_http();
    reg.without_system_copies();
    reg
}

/// A config folder as 0.30 left it: a database at schema 27 that was in use (an account and saved settings),
/// opened by this version (the migration to 28 writes the marker), with `settings` saved.
fn upgraded(name: &str, settings: Value) -> Env {
    let dir = encore_core::tmp::dir(&format!("migrate-{name}"));
    {
        let db = encore_db::Db::open(&dir).unwrap();
        // as 0.30 had it: no plugins table and no marker, an owner account, the schema stamp before plugins
        db.execute_batch("DROP TABLE plugins; DROP TABLE plugin_state").unwrap();
        db.execute("INSERT INTO users (username, password_hash, role, created_at) VALUES ('owner', 'x', 'owner', 0)", p![]).unwrap();
        let cfg = encore_config::Config::load_with(db.clone(), |_| None).unwrap();
        cfg.set_many(&settings);
        db.execute_batch("PRAGMA user_version = 27").unwrap();
    }
    let reg = open(&dir);
    Env { dir, reg }
}

fn marker(reg: &Registry) -> String {
    reg.migration_marker()
}

/// The 0.30 layout: the venv Encore kept yt-dlp and streamrip in, and the slskd it installed.
fn old_tools(dir: &Path, ytdlp_runs: bool) {
    let bin = dir.join("tools/venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin.join("yt-dlp"), if ytdlp_runs { "echo 2026.10.14" } else { "exit 3" });
    script(&bin.join("rip"), "echo 'rip, version 2.4.9'");
    let code = dir.join("tools/venv/lib/python3.14/site-packages/streamrip");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(code.join("config.py"), "no_update_check = False\n").unwrap();
    let slskd = dir.join("tools/slskd/0.26.1");
    std::fs::create_dir_all(&slskd).unwrap();
    script(&slskd.join("slskd"), "echo 0.26.1");
    std::fs::write(dir.join("tools/slskd/current"), "0.26.1\n").unwrap();
    std::fs::create_dir_all(dir.join("streamrip")).unwrap();
    std::fs::write(dir.join("streamrip/config.toml"), "[deezer]\narl = \"abc\"\n").unwrap();
}

fn in_use_settings() -> Value {
    json!({ "streamrip_enabled": true, "streamrip_deezer_arl": "abc", "slskd_enabled": true, "soulseek_username": "alice", "soulseek_password": "secret",
            "source_order": "deezer,slskd,ytdlp", "tools_auto_streamrip": false })
}

fn row(reg: &Registry, id: &str) -> Option<encore_db::Row> {
    reg.row(id)
}

#[tokio::test]
async fn an_upgraded_config_adopts_and_switches_on_what_was_in_use_with_its_settings() {
    let e = upgraded("adopt", in_use_settings());
    old_tools(&e.dir, true);
    assert_eq!(marker(&e.reg), "adopt", "a database that was in use before plugins");
    let r = e.reg.migrate().await;
    let adopted: Vec<&str> = r["adopted"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(adopted, ["ytdlp", "streamrip", "slskd"], "{r}");
    assert_eq!(r["installed"], json!([]), "every adopted program runs here: nothing downloaded");
    assert_eq!(r["failed"], json!([]));
    assert_eq!(marker(&e.reg), "done");
    for id in ["ytdlp", "streamrip", "slskd"] {
        let row = row(&e.reg, id).unwrap();
        assert!(row.b("enabled"), "{id} switched on");
        assert_eq!(row.s("origin"), "catalog");
        assert_eq!(row.s("trust"), "first-party");
    }
    // the old auto-update switches: yt-dlp's default (on), streamrip's turned off, slskd's default (off)
    assert!(row(&e.reg, "ytdlp").unwrap().b("auto_update"));
    assert!(!row(&e.reg, "streamrip").unwrap().b("auto_update"));
    assert!(!row(&e.reg, "slskd").unwrap().b("auto_update"));
    assert_eq!(row(&e.reg, "slskd").unwrap().s("version"), "0.26.1", "the version folder it adopted");
    // a venv has no version folder: the cards show the version its program named when it was checked
    assert_eq!(row(&e.reg, "streamrip").unwrap().s("version"), "2.4.9", "rip --version of the adopted venv");
    assert_eq!(row(&e.reg, "ytdlp").unwrap().s("version"), "2026.10.14", "yt-dlp --version of the adopted venv");
    // nothing else: no Prowlarr, no Cadenza switch, no model, no media server address
    for id in ["torznab", "qbittorrent", "cadenza", "translator", "jellyfin", "plex", "navidrome"] {
        assert!(!e.reg.installed(id), "{id} was not in use");
    }
    // the settings are where they were, read by the plugins as they are
    assert_eq!(e.reg.cfg().str("streamrip_deezer_arl"), "abc");
    assert_eq!(e.reg.cfg().str("soulseek_username"), "alice");
    assert_eq!(e.reg.cfg().str("source_order"), "deezer,slskd,ytdlp");
    assert_eq!(e.reg.values(crate::catalog::find("streamrip").unwrap())["streamrip_deezer_arl"], "abc");
    // the programs are found where they were: nothing moved (the previous image still runs on them)
    let y = e.reg.program_or_adopted("ytdlp").unwrap();
    assert_eq!((y.path.clone(), y.adopted), (e.dir.join("tools/venv/bin/yt-dlp"), true));
    assert_eq!(e.reg.program_or_adopted("streamrip").unwrap().path, e.dir.join("tools/venv/bin/rip"));
    let s = e.reg.program_or_adopted("slskd").unwrap();
    assert_eq!((s.version.as_str(), s.path.clone()), ("0.26.1", e.dir.join("tools/slskd/0.26.1/slskd")));
    assert!(e.dir.join("streamrip/config.toml").is_file());
    // the cards say installed, the old tool each took over
    let card = e.reg.card("streamrip").unwrap();
    assert_eq!((card["installed"].clone(), card["enabled"].clone(), card["migration"].clone()), (json!(true), json!(true), json!("streamrip")));
    // once is enough: a second start adopts nothing again, and a plugin removed since stays removed
    e.reg.db().execute("DELETE FROM plugins WHERE id='slskd'", p![]).unwrap();
    let again = e.reg.migrate().await;
    assert_eq!(again["adopted"], json!([]));
    assert!(!e.reg.installed("slskd"));
}

#[tokio::test]
async fn a_fresh_config_installs_nothing() {
    let dir = encore_core::tmp::dir("migrate-fresh");
    let reg = open(&dir);
    assert_eq!(marker(&reg), "fresh");
    // even with the defaults that would count as "in use" on an upgrade (yt-dlp is on by default)
    assert!(reg.cfg().bool("ytdlp_enabled"));
    let r = reg.migrate().await;
    assert_eq!(r, json!({ "state": "fresh", "adopted": [], "installed": [], "failed": [] }));
    assert!(reg.rows().is_empty());
    assert!(reg.list()["plugins"].as_array().unwrap().iter().all(|p| p["installed"] == false));
    // a second start of the same (now used) config is still not an upgrade
    reg.cfg().set_many(&json!({ "streamrip_enabled": true }));
    drop(reg);
    let reg = open(&dir);
    assert_eq!(marker(&reg), "fresh");
    assert_eq!(reg.migrate().await["adopted"], json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// streamrip was in use from the image's own copy (0.30 shipped it in /opt/tools, gone from this image), and the
/// adopted yt-dlp does not run here: both are installed – streamrip's pinned release (v2.4.10, which calls itself
/// 2.4.9 and must carry the update switch), yt-dlp's newest from PyPI – so downloads keep working.
#[tokio::test]
async fn a_program_that_was_in_use_but_is_missing_or_broken_is_installed() {
    let s = StandIn::start().await;
    s.put("/pypi/yt-dlp/json", json!({ "info": { "version": "2026.10.20" } }).to_string());
    let e = upgraded("install", in_use_settings());
    old_tools(&e.dir, false);
    // no streamrip in the venv (0.30 ran the image's)
    std::fs::remove_file(e.dir.join("tools/venv/bin/rip")).unwrap();
    e.reg.set_hosts(Hosts { pypi: s.url(""), ..Hosts::default() });
    let uv = e.dir.join("fake-uv");
    script(&uv, r#"set -e
case "$1" in
  venv) mkdir -p "$4/bin" "$4/lib/python3.14/site-packages/streamrip" ;;
  pip) venv=$(dirname "$(dirname "$4")"); spec="$5"
       case "$spec" in
         streamrip*) printf '#!/bin/sh\necho "rip, version 2.4.9"\n' > "$venv/bin/rip"; chmod +x "$venv/bin/rip"
                     echo 'no_update_check = 1' > "$venv/lib/python3.14/site-packages/streamrip/config.py" ;;
         yt-dlp*) printf '#!/bin/sh\necho "%s"\n' "${spec#*==}" > "$venv/bin/yt-dlp"; chmod +x "$venv/bin/yt-dlp" ;;
       esac ;;
esac"#);
    e.reg.set_uv(&uv);
    let r = e.reg.migrate().await;
    let installed: Vec<&str> = r["installed"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(installed, ["streamrip", "ytdlp"], "{r}");
    assert_eq!(marker(&e.reg), "done");
    // the plugin's own installs, the version switch moved
    assert_eq!(e.reg.program("streamrip").unwrap(), ("2.4.10".to_string(), e.dir.join("plugins/streamrip/2.4.10/venv/bin/rip")));
    assert_eq!(e.reg.program("ytdlp").unwrap().0, "2026.10.20");
    let y = e.reg.program_or_adopted("ytdlp").unwrap();
    assert!(!y.adopted, "its own install first");
    assert_eq!(e.reg.row("streamrip").unwrap().s("version"), "2.4.10");
    // slskd ran: nothing installed for it
    assert!(!e.dir.join("plugins/slskd/current").exists());
}

/// YouTube Music was switched off, but 0.30 still ran every pasted link (and the last-resort step) through the
/// image's yt-dlp: the upgrade adopts yt-dlp anyway and installs it, so links keep working.
#[tokio::test]
async fn yt_dlp_is_adopted_with_youtube_music_off_because_links_used_it() {
    let s = StandIn::start().await;
    s.put("/pypi/yt-dlp/json", json!({ "info": { "version": "2026.10.20" } }).to_string());
    let e = upgraded("ytdlp-off", json!({ "ytdlp_enabled": false, "ytdlp_last_resort": false }));
    e.reg.set_hosts(Hosts { pypi: s.url(""), ..Hosts::default() });
    let uv = e.dir.join("fake-uv");
    script(&uv, r#"set -e
case "$1" in
  venv) mkdir -p "$4/bin" ;;
  pip) venv=$(dirname "$(dirname "$4")"); printf '#!/bin/sh\necho "%s"\n' "${5#*==}" > "$venv/bin/yt-dlp"; chmod +x "$venv/bin/yt-dlp" ;;
esac"#);
    e.reg.set_uv(&uv);
    let r = e.reg.migrate().await;
    assert_eq!((r["adopted"].clone(), r["installed"].clone()), (json!(["ytdlp"]), json!(["ytdlp"])), "{r}");
    assert!(e.reg.enabled("ytdlp"));
    assert!(!e.reg.cfg().bool("ytdlp_enabled"), "YouTube Music stays off: only links and the program come back");
    assert_eq!(e.reg.program("ytdlp").unwrap().0, "2026.10.20");
}

/// The pinned streamrip must call itself what Encore was tested with, and carry the update switch.
#[tokio::test]
async fn the_pinned_streamrip_is_checked_like_the_image_checked_it() {
    let e = upgraded("pin", json!({ "streamrip_enabled": true, "streamrip_deezer_arl": "abc", "ytdlp_enabled": false }));
    // the yt-dlp 0.30 kept updated (every upgrade adopts yt-dlp: links always used it)
    script(&e.dir.join("tools/venv/bin/yt-dlp"), "echo 2026.10.14");
    let uv = e.dir.join("fake-uv");
    // a tag that calls itself another version
    script(&uv, r#"set -e
case "$1" in
  venv) mkdir -p "$4/bin" "$4/lib/python3.14/site-packages/streamrip"; echo 'no_update_check = 1' > "$4/lib/python3.14/site-packages/streamrip/config.py" ;;
  pip) venv=$(dirname "$(dirname "$4")"); printf '#!/bin/sh\necho "rip, version 2.5.0"\n' > "$venv/bin/rip"; chmod +x "$venv/bin/rip" ;;
esac"#);
    e.reg.set_uv(&uv);
    let r = e.reg.migrate().await;
    assert_eq!(r["adopted"], json!(["ytdlp", "streamrip"]), "{r}");
    assert_eq!(r["failed"][0]["id"], "streamrip");
    assert!(r["failed"][0]["error"].as_str().unwrap().contains("calls itself 2.5.0 instead of 2.4.9"), "{r}");
    assert!(e.reg.program("streamrip").is_none(), "nothing half-installed");
    assert_eq!(marker(&e.reg), "install", "tried again at the next start");
}

#[test]
fn adopted_programs_are_found_in_their_old_folders() {
    let dir = encore_core::tmp::dir("migrate-found");
    old_tools(&dir, true);
    let llama = dir.join("tools/translator/llama/b6800/build/bin");
    std::fs::create_dir_all(&llama).unwrap();
    script(&llama.join("llama-server"), "echo version: 6800");
    std::fs::write(dir.join("tools/translator/llama/current"), "b6800\n").unwrap();
    let cad = dir.join("tools/cadenza/2.1.0");
    std::fs::create_dir_all(&cad).unwrap();
    script(&cad.join("cadenza"), "true");
    std::fs::write(dir.join("tools/cadenza/current"), "2.1.0\n").unwrap();
    let find = |id: &str| adopted_program(crate::catalog::find(id).unwrap(), &dir).map(|p| (p.version, p.path));
    assert_eq!(find("ytdlp"), Some((String::new(), dir.join("tools/venv/bin/yt-dlp"))));
    assert_eq!(find("streamrip"), Some((String::new(), dir.join("tools/venv/bin/rip"))));
    assert_eq!(find("slskd"), Some(("0.26.1".into(), dir.join("tools/slskd/0.26.1/slskd"))));
    assert_eq!(find("translator"), Some(("b6800".into(), llama.join("llama-server"))));
    assert_eq!(find("cadenza"), Some(("2.1.0".into(), cad.join("cadenza"))));
    std::fs::write(dir.join("tools/cadenza/current"), "../../etc\n").unwrap();
    assert_eq!(find("cadenza"), None, "a current that leaves its folder");
    assert_eq!(find("torznab"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// No network at the first start after the upgrade: the install pass fails, says so in the activity log, and
/// is tried again by itself (the server waits 1, 2, 5, 10, 30 minutes, then hourly; here a few milliseconds),
/// so yt-dlp comes back without a restart and whatever the "Update plugins" task's settings are.
#[tokio::test]
async fn an_install_that_failed_at_the_start_is_tried_again_until_it_works() {
    let s = StandIn::start().await;
    s.put_status("/pypi/yt-dlp/json", 503, "down", &[]);
    let e = upgraded("retry", json!({ "ytdlp_enabled": true, "task_tools_enabled": false }));
    e.reg.set_hosts(Hosts { pypi: s.url(""), ..Hosts::default() });
    let uv = e.dir.join("fake-uv");
    script(&uv, r#"set -e
case "$1" in
  venv) mkdir -p "$4/bin" ;;
  pip) venv=$(dirname "$(dirname "$4")"); printf '#!/bin/sh\necho "%s"\n' "${5#*==}" > "$venv/bin/yt-dlp"; chmod +x "$venv/bin/yt-dlp" ;;
esac"#);
    e.reg.set_uv(&uv);
    let waits = std::sync::Mutex::new(vec![]);
    let r = e
        .reg
        .migrate_until_done(|n| {
            waits.lock().unwrap().push(n);
            // PyPI answers again before the second try
            s.put("/pypi/yt-dlp/json", json!({ "info": { "version": "2026.10.20" } }).to_string());
            std::time::Duration::from_millis(10)
        })
        .await;
    assert_eq!(*waits.lock().unwrap(), [1], "one failed try, then it worked: {r}");
    assert_eq!(r["installed"], json!(["ytdlp"]), "{r}");
    assert_eq!(marker(&e.reg), "done");
    assert_eq!(e.reg.program("ytdlp").unwrap().0, "2026.10.20");
    let log: Vec<String> = e.reg.db().query("SELECT message FROM activity ORDER BY id", p![]).unwrap().iter().map(|r| r.s("message")).collect();
    assert!(log.iter().any(|l| l.contains("installing what was in use before the upgrade failed (ytdlp:") && l.contains("trying again in")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("are installed (yt-dlp)")), "{log:?}");
    // the waits the server uses
    let mins: Vec<u64> = (1..=7).map(|n| crate::migrate::retry_after(n).as_secs() / 60).collect();
    assert_eq!(mins, [1, 2, 5, 10, 30, 60, 60]);
}

/// While the install pass is not done, a switched-on source whose program is not there yet is "being
/// installed" (the request worker waits for it); once it is there, or the pass is done, it is not.
#[tokio::test]
async fn a_source_is_being_installed_until_its_program_is_there() {
    let e = upgraded("installing", json!({ "ytdlp_enabled": true }));
    e.reg.adopt_in_use();
    assert_eq!(marker(&e.reg), "install");
    assert!(e.reg.installing_after_upgrade("ytdlp"));
    assert!(!e.reg.installing_after_upgrade("streamrip"), "not in use: not adopted");
    std::fs::create_dir_all(e.dir.join("tools/venv/bin")).unwrap();
    script(&e.dir.join("tools/venv/bin/yt-dlp"), "echo 2026.10.14");
    assert!(!e.reg.installing_after_upgrade("ytdlp"), "its adopted program is there");
    std::fs::remove_file(e.dir.join("tools/venv/bin/yt-dlp")).unwrap();
    e.reg.db().execute("UPDATE plugin_state SET value='done' WHERE key='migration'", p![]).unwrap();
    assert!(!e.reg.installing_after_upgrade("ytdlp"), "the pass is done: a missing program is missing");
}
