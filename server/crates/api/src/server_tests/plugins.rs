//! The plugins on a whole server (0.32's builtin adapters and the migration from 0.30): an upgraded config keeps
//! downloading with no new setup (streamrip adopted from the venv 0.30 kept, its update switch read there); a
//! fresh server has nothing installed and its picker says where to install; a plugin program's source answers
//! the picker and the request worker through the registry, and stops when switched off; the builtin adapters
//! answer status, test and download; Cadenza's engine, the Translator and the old Download tools routes follow
//! their plugins.
use super::streamrip::{album_fixture, cover_server, fake_rip, log_text, request_row};
use crate::routes::picker::{self, SearchBody};
use crate::test_support::{call, user, TestServer};
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

const DEEZER_ALBUM: &str = "https://www.deezer.com/album/302127";

fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tmp = path.with_extension("tmp-script");
    std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("encore-p2-{name}-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn queue(st: &crate::state::AppState, kind: &str, artist: &str, title: &str, detail: Value) -> i64 {
    let t = encore_db::now();
    st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, message, detail, created_at, updated_at) VALUES (1,?,?,?,?,'queued','',?,?,?)",
        p![kind, artist, title, if kind == "album" { title } else { "" }, detail.to_string(), t, t]).unwrap()
}

fn plain_settings(dir: &Path) -> Value {
    json!({ "staging_path": dir.join("staging").to_string_lossy(), "library_path": dir.join("lib").to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9",
            "autotag_on_import": false, "quality_check_on_import": false, "lyrics_on_import": false, "auto_approve": true, "album_upgrade": "off",
            "ytdlp_enabled": false, "ytdlp_last_resort": false, "slskd_enabled": false })
}

/// 0.30 → this version: the config folder of a server that downloaded from Deezer with the streamrip Encore kept
/// updated in config/tools/venv (the image's own copy is gone now). The first start adopts streamrip there, and a
/// picked Deezer album downloads and imports as before – nothing set up again, nothing downloaded for it.
#[tokio::test]
async fn an_upgraded_server_keeps_downloading_with_no_new_setup() {
    let dir = tmp_dir("upgrade");
    let fake = dir.join("fake");
    let rip = fake_rip(&fake);
    if !album_fixture(&fake.join("fixture")) {
        let _ = std::fs::remove_dir_all(&dir);
        return; // no ffmpeg here
    }
    std::fs::write(fake.join("mode"), "album").unwrap();
    let Some(cover) = cover_server(&dir).await else { return };
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    // the 0.30 server: an owner, its settings; no plugins table yet
    {
        let ts = TestServer::in_dir(&dir);
        user(&ts.st, "boss", "admin");
        let mut s = plain_settings(&dir);
        s["streamrip_enabled"] = json!(true);
        s["streamrip_deezer_arl"] = json!("arl-cookie");
        s["source_order"] = json!("deezer");
        ts.st.cfg.set_many(&s);
        ts.st.db.execute_batch("DROP TABLE plugins; DROP TABLE plugin_state; PRAGMA user_version = 27").unwrap();
    }
    // its streamrip, where 0.30 kept it (the stand-in answers --version as the pinned fork does) with the fork's
    // update switch in its code
    let venv = dir.join("tools/venv");
    script(&venv.join("bin/rip"), &format!("case \"$1\" in --version) echo 'rip, version 2.4.9';; *) exec '{}' \"$@\";; esac", rip.display()));
    let code = venv.join("lib/python3.14/site-packages/streamrip");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(code.join("config.py"), "    no_update_check: bool = False\n").unwrap();
    // and the yt-dlp it kept updated beside it (links always used yt-dlp: every upgrade adopts it)
    script(&venv.join("bin/yt-dlp"), "echo 2026.10.14");

    // this version starts on the same folder
    let ts = TestServer::in_dir(&dir);
    let _plugins = crate::start_plugins(&ts.st);
    let st = ts.st.clone();
    assert!(st.plugins.enabled("streamrip"), "{:?}", st.plugins.rows());
    assert!(st.plugins.enabled("ytdlp"), "yt-dlp too: links always used it");
    assert!(!st.plugins.installed("slskd") && !st.plugins.installed("torznab"), "only what was in use");
    assert_eq!(st.cfg.str("streamrip_deezer_arl"), "arl-cookie", "its settings where they were");
    let found = encore_system::tools::resolve(&st, "streamrip").unwrap();
    assert_eq!((found.0, found.1.as_deref()), (venv.join("bin/rip"), Some("2.4.9")), "the adopted copy, in place");
    assert_eq!(picker::source_state(&st, "deezer").state, "ready");
    // a picked Deezer album, through the worker
    let detail = json!({ "pick": { "source": "deezer", "payload": { "url": DEEZER_ALBUM }, "title": "Illuminate EP" } });
    let t = encore_db::now();
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, cover_url, status, message, detail, created_at, updated_at) VALUES (1,'album','Duumu','Illuminate EP','Illuminate EP',?,'queued','',?,?,?)",
        p![cover, detail.to_string(), t, t]).unwrap();
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert_eq!(st.db.one("SELECT COUNT(*) n FROM tracks WHERE request_id=?", p![id]).unwrap().unwrap().i("n"), 4);
    // the update switch was read from the adopted venv's code and turned off in streamrip's config
    let config = std::fs::read_to_string(dir.join("streamrip/config.toml")).unwrap();
    assert!(config.contains("no_update_check = true"), "{config}");
    assert!(!dir.join("plugins/streamrip/current").exists(), "nothing installed beside it");
    drop(ts);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_fresh_server_has_nothing_installed_and_its_picker_says_where_to_install() {
    let ts = TestServer::new("p2-fresh");
    let _plugins = crate::start_plugins(&ts.st);
    let st = ts.st.clone();
    user(&st, "boss", "admin");
    st.cfg.set_many(&plain_settings(&ts.dir));
    assert!(st.plugins.rows().is_empty(), "every plugin available, none installed");
    let sources = picker::available(&st);
    let ids: Vec<&str> = sources.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["slskd", "torznab", "qobuz", "tidal", "deezer", "ytmusic", "zotify", "gamdl"], "the picker's tabs as before, then Spotify and Apple Music (0.34)");
    for s in &sources {
        assert_eq!(s["state"], "not_installed", "{s}");
        assert_eq!(s["settings"], "#/settings?s=plugins", "{s}");
        assert!(s["hint"].as_str().unwrap().starts_with("Install the "), "{s}");
    }
    // a request has nowhere to go
    let id = queue(&st, "track", "Duumu", "Light 1", json!({}));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, _) = request_row(&st, id);
    assert_eq!(status, "failed");
    assert!(message.contains("no download sources enabled"), "{message}");
    // a link needs yt-dlp's plugin
    let t = encore_db::now();
    let link = st.db.execute("INSERT INTO requests (user_id, kind, url, status, message, detail, created_at, updated_at) VALUES (1,'url','https://soundcloud.com/a/b','queued','','{}',?,?)", p![t, t]).unwrap();
    encore_requests::worker::step(st.clone(), link).await;
    let (status, message, _) = request_row(&st, link);
    assert_eq!((status.as_str(), message.as_str()), ("failed", "Links need the yt-dlp plugin – install it under Settings → Plugins"));
    // the scheduled task that kept the download tools fresh keeps the plugins fresh now: nothing to do here
    let task = encore_jobs::scheduler::TASKS.iter().find(|t| t.0 == "tools").unwrap();
    assert_eq!(task.1, "Update plugins");
    assert_eq!(encore_system::tools::scheduled(&st).await.unwrap(), "No plugins installed");
    ts.remove();
}

fn has_python() -> bool {
    let ok = encore_plugins::process::find_python().is_some_and(|p| {
        std::process::Command::new(p).args(["-c", "import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)"]).status().is_ok_and(|s| s.success())
    });
    if !ok {
        eprintln!("skipped: no Python 3.11 or newer here");
    }
    ok
}

/// The test plugin of encore-plugins (testdata/test-tone) with the SDK beside it, as an upload.
fn tone_zip(dir: &Path) -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = crate_dir.join("../plugins/testdata/test-tone");
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
    for (name, path) in [("plugin.toml", src.join("plugin.toml")), ("plugin.py", src.join("plugin.py")), ("encore_plugin.py", crate_dir.join("../../../plugins/sdk/encore_plugin.py"))] {
        z.start_file(name, o).unwrap();
        z.write_all(&std::fs::read(path).unwrap()).unwrap();
    }
    let file = dir.join("tone.zip");
    std::fs::write(&file, z.finish().unwrap().into_inner()).unwrap();
    file
}

/// An uploaded plugin's source: listed in the picker after the builtin tabs, searched there through the
/// registry, picked and downloaded by the worker; named in `source_order`, the worker finds and downloads the
/// best match itself; switched off, neither offers it any more.
#[tokio::test]
async fn a_plugin_programs_source_answers_the_picker_and_the_worker() {
    if !has_python() {
        return;
    }
    let ts = TestServer::new("p2-tone");
    let st = ts.st.clone();
    user(&st, "boss", "admin");
    let mut s = plain_settings(&ts.dir);
    s["source_order"] = json!("test-tone");
    st.cfg.set_many(&s);
    std::fs::create_dir_all(ts.dir.join("lib")).unwrap();
    let card = st.plugins.upload(&tone_zip(&ts.dir)).await.unwrap();
    assert_eq!(card["installed"], true);
    // the picker: a tab after the builtin ones, ready
    let sources = picker::available(&st);
    let tone = sources.iter().find(|x| x["id"] == "test-tone").unwrap();
    assert_eq!((tone["name"].as_str(), tone["state"].as_str()), (Some("Test tone"), Some("ready")));
    assert_eq!(sources.iter().position(|x| x["id"] == "test-tone"), Some(8), "after the builtin tabs and the catalog's zotify and gamdl");
    let b = SearchBody { source: "test-tone".into(), kind: "track".into(), artist: "Test Tones".into(), title: "Middle C".into(), ..Default::default() };
    let a = picker::run_search(&st, &b).await.unwrap();
    let first = &a.items[0];
    assert_eq!((first["source"].as_str(), first["key"].as_str(), first["quality"].as_str()), (Some("test-tone"), Some("test-tone:tone:c262"), Some("WAV 16-bit")), "{first}");
    assert_eq!(first["payload"]["plugin_key"], "tone:c262");
    // picked: downloaded and imported by the worker
    let id = queue(&st, "track", "Test Tones", "Middle C", json!({ "pick": { "source": "test-tone", "payload": first["payload"], "title": "Middle C" } }));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert_eq!(message, "Downloaded from Test tone");
    let files: Vec<PathBuf> = walkdir::WalkDir::new(ts.dir.join("lib")).into_iter().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "wav")).map(|e| e.into_path()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    // in the source order: the worker searches it and downloads the best match – one request at a time per
    // plugin: while another holds the plugin's slot it waits in the plugin's line, and Download now offers to
    // move it up there
    let line = st.requests.lines.plugin("test-tone");
    let slot = line.acquire(-7, "Test tone", &st.requests.lines.pressed, || {}).await;
    let id = queue(&st, "track", "Test Tones", "A 440", json!({}));
    let task = tokio::spawn(encore_requests::worker::step(st.clone(), id));
    for _ in 0..200 {
        if line.waiting() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let (status, message, _) = request_row(&st, id);
    assert_eq!((status.as_str(), message.as_str(), line.running()), ("queued", "In line for Test tone", 1));
    let r = encore_requests::worker::get(&st, id).unwrap();
    let o = encore_requests::worker::now::offer(&st, &r).unwrap();
    assert_eq!((o.kind(), o.title.as_str()), ("line", "Go first in line for Test tone"));
    let place = st.requests.lines.place(id).unwrap();
    assert_eq!((place.source, place.text(), place.limit_text()), ("plugin", "Next in line for Test tone".to_string(), "1 download at a time".to_string()));
    drop(slot);
    task.await.unwrap();
    let (status, message, d) = request_row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert_eq!((line.running(), line.waiting()), (0, 0), "the slot is given back");
    assert_eq!(st.db.one("SELECT COUNT(*) n FROM tracks WHERE request_id=?", p![id]).unwrap().unwrap().i("n"), 1);
    // nothing that matches: the next source, with the plugin's words
    let id = queue(&st, "track", "Somebody", "Nothing Like It", json!({}));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, _, d) = request_row(&st, id);
    assert_eq!(status, "failed");
    assert!(log_text(&d).contains("test-tone: nothing found on Test tone"), "{}", log_text(&d));
    // switched off: not offered, not tried
    st.plugins.enable("test-tone", false).await.unwrap();
    assert_eq!(picker::source_state(&st, "test-tone").state, "off");
    assert!(picker::run_search(&st, &b).await.is_err());
    let id = queue(&st, "track", "Test Tones", "E 330", json!({}));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, _) = request_row(&st, id);
    assert_eq!(status, "failed");
    assert!(message.contains("no download sources enabled"), "{message}");
    ts.remove();
}

/// The builtin adapters through the registry (the Plugins page's status and Test, a search, a download), the
/// engine Cadenza's plugin gives server processing, the Translator's program, and the old Download tools routes
/// in their old shape, answered from the plugins.
#[tokio::test]
async fn the_builtin_adapters_answer_through_the_registry() {
    let ts = TestServer::new("p2-adapters").with_plugins(&["ytdlp", "torznab", "qbittorrent", "cadenza", "translator", "jellyfin"]);
    let st = ts.st.clone();
    user(&st, "boss", "admin");
    // yt-dlp: the plugin's own install – a stand-in that answers --version and writes the song into the folder
    // its output template names
    let fixture = ts.dir.join("fixture");
    let have_ffmpeg = album_fixture(&fixture);
    let ytdlp = ts.dir.join("plugins/ytdlp/2026.10.14/venv/bin/yt-dlp");
    script(&ytdlp, &format!(r#"[ "$1" = --version ] && {{ echo 2026.10.14; exit 0; }}
out=""; while [ $# -gt 0 ]; do [ "$1" = -o ] && {{ out=$2; shift; }}; shift; done
d=$(dirname "$out"); cp '{}' "$d/Duumu - Light 1.flac" 2>/dev/null || printf 'fLaC' > "$d/Duumu - Light 1.flac"
echo 'JSON {{"title":"Light 1","artist":"Duumu"}}'"#, fixture.join("01. Duumu - Light 1.flac").display()));
    std::fs::write(ts.dir.join("plugins/ytdlp/current"), "2026.10.14\n").unwrap();
    let s = st.plugins.status("ytdlp").await.unwrap();
    assert_eq!((s["ok"].as_bool(), s["message"].as_str()), (Some(true), Some("yt-dlp 2026.10.14 (installed by the plugin)")), "{s}");
    let folder = ts.dir.join("dl");
    let got = st.plugins.download("ytdlp", &json!({ "kind": "track", "payload": { "url": "https://music.youtube.com/watch?v=abc" } }), &folder, None).await.unwrap();
    assert_eq!(got.files.len(), 1);
    assert_eq!(got.files[0].path.file_name().unwrap(), "Duumu - Light 1.flac");
    assert_eq!(got.source_url.as_deref(), Some("https://music.youtube.com/watch?v=abc"));
    // switched off: the registry refuses, the source is off
    st.plugins.enable("ytdlp", false).await.unwrap();
    let e = st.plugins.download("ytdlp", &json!({ "payload": { "url": "https://x" } }), &folder, None).await.unwrap_err();
    assert!(e.message.contains("not installed or switched off"), "{}", e.message);
    assert!(!encore_requests::sources::ytdlp::enabled(&st));
    st.plugins.enable("ytdlp", true).await.unwrap();
    // a link through the worker runs the plugin's yt-dlp
    if have_ffmpeg {
        st.cfg.set_many(&plain_settings(&ts.dir));
        std::fs::create_dir_all(ts.dir.join("lib")).unwrap();
        let t = encore_db::now();
        let link = st.db.execute("INSERT INTO requests (user_id, kind, url, status, message, detail, created_at, updated_at) VALUES (1,'url','https://music.youtube.com/watch?v=abc','queued','','{}',?,?)", p![t, t]).unwrap();
        encore_requests::worker::step(st.clone(), link).await;
        let (status, message, d) = request_row(&st, link);
        assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    }
    // Torrents & Usenet and its client: their state in plain words; the Test says what is missing
    let s = st.plugins.status("torznab").await.unwrap();
    assert_eq!((s["message"].as_str(), s["signed_in"].as_bool()), (Some("Prowlarr needs its address and API key"), Some(false)));
    let s = st.plugins.status("qbittorrent").await.unwrap();
    assert_eq!(s["message"], "qBittorrent is chosen but not set up yet");
    let t = st.plugins.test("torznab").await.unwrap();
    assert_eq!(t["ok"], false, "{t}");
    // Torrents & Usenet answers searches in the picker's rows; slskd and torznab downloads are the worker's
    let e = st.plugins.download("torznab", &json!({}), &folder, None).await.unwrap_err();
    assert_eq!(e.code, encore_plugins::Code::Unsupported);
    // a media server: its address is on the Integrations page
    let s = st.plugins.status("jellyfin").await.unwrap();
    assert_eq!(s["message"], "Jellyfin needs its address (Settings → Integrations)");
    // Cadenza: the program it adopted runs server processing while the plugin is on; off, the built-in engine
    let describe = r#"{"name":"cadenza","version":"2.1.0","protocol":1,"settings":[]}"#;
    script(&ts.dir.join("tools/cadenza/2.1.0/cadenza"), &format!("echo '{describe}'"));
    std::fs::write(ts.dir.join("tools/cadenza/current"), "2.1.0\n").unwrap();
    st.playback.cadenza.forget();
    let engine = |st: &crate::state::AppState| encore_playback::dsp::Engine::of(st);
    let st2 = st.clone();
    assert_eq!(tokio::task::spawn_blocking(move || engine(&st2).version()).await.unwrap(), "2.1.0");
    st.plugins.enable("cadenza", false).await.unwrap();
    let st2 = st.clone();
    assert_eq!(tokio::task::spawn_blocking(move || engine(&st2).source()).await.unwrap(), "bundled", "switched off: the engine built into Encore");
    st.plugins.enable("cadenza", true).await.unwrap();
    // the Translator: llama.cpp and the model it adopted; off, it does not run
    use encore_library::translate::llama;
    script(&ts.dir.join("tools/translator/llama/b6800/build/bin/llama-server"), "echo 'version: 6800'");
    std::fs::write(ts.dir.join("tools/translator/llama/current"), "b6800\n").unwrap();
    std::fs::create_dir_all(ts.dir.join("tools/translator/models")).unwrap();
    std::fs::write(ts.dir.join("tools/translator/models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf"), b"GGUF\x03").unwrap();
    assert_eq!(llama::installed(&st).map(|(_, t)| t).as_deref(), Some("b6800"));
    assert!(llama::ready(&st));
    // Settings → Language's local model line follows the plugin
    let model_line = |st: &crate::state::AppState| {
        let t = encore_library::translate::tiers::states(st).into_iter().find(|t| t["id"] == "model").unwrap();
        (t["state"].as_str().unwrap().to_string(), t["ready"].as_bool())
    };
    assert_eq!(model_line(&st), ("Local model: ready (small model) – starts when there is something to translate".to_string(), Some(true)));
    std::fs::rename(ts.dir.join("tools/translator/models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf"), ts.dir.join("model.gguf")).unwrap();
    assert_eq!(model_line(&st), ("Local model: installed, the small model is not downloaded yet – download it in Plugins".to_string(), Some(false)));
    std::fs::rename(ts.dir.join("model.gguf"), ts.dir.join("tools/translator/models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf")).unwrap();
    let t = st.plugins.test("translator").await.unwrap();
    assert_eq!((t["ok"].as_bool(), t["message"].as_str()), (Some(true), Some("llama.cpp b6800 runs")), "{t}");
    st.plugins.enable("translator", false).await.unwrap();
    assert!(!llama::ready(&st));
    assert_eq!(model_line(&st), ("Local model: switched off in Plugins – switch the Translator on there".to_string(), Some(false)));
    // the old Download tools routes, in their old shape, from the plugins
    let app = ts.app();
    let (_, _, cookie) = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": "boss", "password": "a-long-password-1" }))).await;
    let cookie = cookie.split(';').next().unwrap_or("").to_string();
    let (code, v, _) = call(&app, "GET", "/api/system/tools", &cookie, None).await;
    assert_eq!(code, axum::http::StatusCode::OK);
    let tool = |id: &str| v.as_array().unwrap().iter().find(|t| t["id"] == id).unwrap().clone();
    assert_eq!((tool("ytdlp")["installed"].clone(), tool("ytdlp")["source"].clone()), (json!("2026.10.14"), json!("updated")));
    assert_eq!(tool("streamrip")["source"], "missing");
    assert!(tool("streamrip")["note"].as_str().unwrap().contains("Settings → Plugins"));
    assert_eq!((tool("cadenza")["installed"].clone(), tool("cadenza")["source"].clone()), (json!("2.1.0"), json!("updated")));
    assert_eq!(tool("slskd")["source"], "missing");
    ts.remove();
}
