//! /api/plugins on a whole server: admins only with the CSRF header; the catalog with nothing installed and the
//! sandbox line; the test plugin uploaded and run (status, test, settings with a secret, logs, disable, remove);
//! uploads that are refused; URL installs that are not https; the owner's Internet Archive zip through Upload
//! (when the test is given it: ENCORE_TEST_IA_ZIP); the example plugin of docs/plugins when it is there; the
//! old /api/system/tools routes in their old shape.
use crate::test_support::{call, send, user, Resp, TestServer};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

struct Env {
    ts: TestServer,
    app: Router,
    cookie: String,
}

impl Env {
    async fn get(&self, uri: &str) -> Resp {
        send(&self.app, "GET", uri, &self.cookie, None, &[]).await
    }
    async fn post(&self, uri: &str, body: Value) -> Resp {
        send(&self.app, "POST", uri, &self.cookie, Some(body), &[]).await
    }
    async fn put(&self, uri: &str, body: Value) -> Resp {
        send(&self.app, "PUT", uri, &self.cookie, Some(body), &[]).await
    }
    /// A multipart upload of one file part.
    async fn upload(&self, bytes: Vec<u8>) -> Resp {
        let b = "encorePluginBoundary42";
        let mut body = Vec::new();
        body.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"plugin.zip\"\r\nContent-Type: application/zip\r\n\r\n").as_bytes());
        body.extend(bytes);
        body.extend(format!("\r\n--{b}--\r\n").as_bytes());
        let req = Request::builder().method("POST").uri("/api/plugins/upload").header("x-requested-with", "encore").header("cookie", &self.cookie)
            .header("content-type", format!("multipart/form-data; boundary={b}")).body(Body::from(body)).unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let body = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap().to_vec();
        Resp { status, headers, body }
    }
}

async fn login(app: &Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

async fn env(name: &str) -> Env {
    let ts = TestServer::new(&format!("plugins-{name}"));
    user(&ts.st, &format!("boss-{name}"), "admin");
    let app = ts.app();
    let cookie = login(&app, &format!("boss-{name}")).await;
    Env { ts, app, cookie }
}

fn plugins_crate() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins")
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// A zip of the files of a folder (flat), with the SDK beside them when the folder has none.
fn zip_folder(dir: &Path) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
    let mut has_sdk = false;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if !e.file_type().unwrap().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        has_sdk |= name == "encore_plugin.py";
        z.start_file(name, o).unwrap();
        z.write_all(&std::fs::read(e.path()).unwrap()).unwrap();
    }
    if !has_sdk {
        z.start_file("encore_plugin.py", o).unwrap();
        z.write_all(&std::fs::read(repo().join("plugins/sdk/encore_plugin.py")).unwrap()).unwrap();
    }
    z.finish().unwrap().into_inner()
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

#[tokio::test]
async fn plugins_are_for_admins_with_the_csrf_header() {
    let e = env("auth").await;
    user(&e.ts.st, "plain-auth", "user");
    let plain = login(&e.app, "plain-auth").await;
    assert_eq!(send(&e.app, "GET", "/api/plugins", "", None, &[]).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(send(&e.app, "GET", "/api/plugins", &plain, None, &[]).await.status, StatusCode::FORBIDDEN);
    assert_eq!(send(&e.app, "POST", "/api/plugins/torznab/install", &plain, None, &[]).await.status, StatusCode::FORBIDDEN);
    // a cookie request that changes something without the CSRF header
    let req = Request::builder().method("POST").uri("/api/plugins/torznab/install").header("cookie", &e.cookie).body(Body::empty()).unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(!e.ts.st.plugins.installed("torznab"));
    assert_eq!(e.get("/api/plugins/nothing-here").await.status, StatusCode::NOT_FOUND);
    e.ts.remove();
}

#[tokio::test]
async fn the_list_has_the_catalog_and_the_sandbox_line() {
    let e = env("list").await;
    let r = e.get("/api/plugins").await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    let plugins = v["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 19);
    assert!(plugins.iter().all(|p| p["installed"] == false), "a fresh install has nothing installed");
    assert!(!plugins.iter().any(|p| p["id"] == "internet-archive"), "the owner's plugin is not in the catalog");
    assert!(matches!(v["sandbox"]["level"].as_str(), Some("full" | "limits")));
    assert!(matches!(v["sandbox"]["text"].as_str(), Some("Full isolation" | "Limits only, see the compose option")));
    assert_eq!(v["upload_max_mb"], 200);
    let yt = &plugins[0];
    assert_eq!((yt["id"].as_str(), yt["kind"].as_str(), yt["origin"].as_str(), yt["builtin"].as_bool()), (Some("ytdlp"), Some("source"), Some("catalog"), Some(true)));
    // a plugin without a program: Install switches it on, its settings card is the settings table's
    let r = e.post("/api/plugins/torznab/install", json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["enabled"], true);
    let s = e.get("/api/plugins/torznab/settings").await.json();
    let key = s["fields"].as_array().unwrap().iter().find(|f| f["key"] == "prowlarr_api_key").unwrap().clone();
    assert_eq!((key["type"].as_str(), key["secret"].as_bool()), (Some("password"), Some(true)));
    let r = e.put("/api/plugins/torznab/settings", json!({ "prowlarr_api_key": "abc-key", "prowlarr_url": "http://prowlarr:9696" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["values"]["prowlarr_api_key"], "•••• (saved)");
    assert_eq!(e.ts.st.cfg.str("prowlarr_api_key"), "abc-key", "a builtin plugin's settings are Encore's own keys");
    assert_eq!(e.post("/api/plugins/torznab/remove", json!({})).await.status, StatusCode::OK);
    assert_eq!(e.ts.st.cfg.str("prowlarr_url"), "http://prowlarr:9696", "Remove keeps Encore's settings");
    e.ts.remove();
}

#[tokio::test]
async fn the_test_plugin_uploads_runs_and_goes() {
    if !has_python() {
        return;
    }
    let e = env("upload").await;
    let zip = zip_folder(&plugins_crate().join("testdata/test-tone"));
    let r = e.upload(zip).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let c = r.json();
    assert_eq!((c["id"].as_str(), c["origin"].as_str(), c["trust"].as_str(), c["installed_version"].as_str()), (Some("test-tone"), Some("upload"), Some("third-party"), Some("0.1.0")));
    assert!(c["warning"].as_str().unwrap().contains("Install only plugins you trust"));
    let one = e.get("/api/plugins/test-tone").await.json();
    assert_eq!(one["status"]["message"], "Ready · three test tones");
    let r = e.put("/api/plugins/test-tone/settings", json!({ "values": { "token": "tok-secret-9", "seconds": 2 } })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["values"]["token"], "•••• (saved)");
    let r = e.put("/api/plugins/test-tone/settings", json!({ "seconds": 500 })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let t = e.post("/api/plugins/test-tone/test", json!({})).await.json();
    assert_eq!(t["ok"], true, "{t}");
    assert!(t["message"].as_str().unwrap().contains("token seen: True · in env: False · in argv: False"), "{t}");
    let logs = e.get("/api/plugins/test-tone/logs?lines=50").await.json();
    assert!(logs["lines"].as_array().unwrap().len() >= 2, "{logs}");
    assert!(!logs.to_string().contains("tok-secret-9"));
    assert_eq!(e.post("/api/plugins/test-tone/disable", json!({})).await.json()["enabled"], false);
    assert_eq!(e.post("/api/plugins/test-tone/enable", json!({})).await.json()["enabled"], true);
    assert_eq!(e.post("/api/plugins/test-tone/auto-update", json!({ "on": true })).await.json()["auto_update"], true);
    let list = e.get("/api/plugins").await.json();
    assert!(list["plugins"].as_array().unwrap().iter().any(|p| p["id"] == "test-tone" && p["installed"] == true));
    let r = e.post("/api/plugins/test-tone/remove", json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(!e.ts.dir.join("plugins/test-tone").exists());
    assert_eq!(e.get("/api/plugins/test-tone").await.status, StatusCode::NOT_FOUND);
    e.ts.remove();
}

#[tokio::test]
async fn bad_uploads_and_urls_are_refused() {
    let e = env("refused").await;
    let r = e.upload(b"not a zip at all".to_vec()).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    // a manifest with an unknown key: 422 with the problems listed
    let toml = std::fs::read_to_string(plugins_crate().join("testdata/test-tone/plugin.toml")).unwrap().replace("[run]\n", "[run]\nshel = true\n");
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    z.start_file("plugin.toml", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(toml.as_bytes()).unwrap();
    let r = e.upload(z.finish().unwrap().into_inner()).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.text());
    assert!(r.json()["problems"][0].as_str().unwrap().contains("shel"), "{}", r.text());
    // an empty upload
    let r = e.upload(Vec::new()).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // URLs: https only in a real build, owner/repo@ref checked
    let r = e.post("/api/plugins/install-url", json!({ "url": "http://example.org/plugin.zip" })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("only https"), "{}", r.text());
    for bad in ["", "file:///etc/passwd", "not a url", "o/r@../x"] {
        let r = e.post("/api/plugins/install-url", json!({ "url": bad })).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.text());
    }
    assert!(e.ts.st.plugins.rows().is_empty());
    e.ts.remove();
}

/// The owner's Internet Archive plugin, uploaded as they made it: installed (not built in, not from the
/// catalog), `describe` passed, its status and its settings card. Search and download need archive.org, which
/// this test does not reach. Give it the zip with ENCORE_TEST_IA_ZIP=<path>.
#[tokio::test]
async fn the_owners_internet_archive_zip_installs_through_upload() {
    let Some(path) = std::env::var_os("ENCORE_TEST_IA_ZIP").map(PathBuf::from).filter(|p| p.is_file()) else {
        eprintln!("skipped: ENCORE_TEST_IA_ZIP names no zip");
        return;
    };
    if !has_python() {
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let before = encore_core::ids::sha256_hex(&bytes);
    let e = env("ia").await;
    let r = e.upload(bytes).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let c = r.json();
    assert_eq!((c["id"].as_str(), c["name"].as_str(), c["installed_version"].as_str()), (Some("internet-archive"), Some("Internet Archive"), Some("0.1.0")));
    assert_eq!((c["origin"].as_str(), c["trust"].as_str(), c["builtin"].as_bool()), (Some("upload"), Some("third-party"), Some(false)));
    assert_eq!(c["hosts"], json!(["archive.org", "*.archive.org"]));
    assert_eq!(c["network"], true);
    assert_eq!(c["kinds"], json!(["track", "album"]));
    assert_eq!(e.ts.st.plugins.row("internet-archive").map(|r| encore_db::RowExt::s(&r, "sha256")), Some(before.clone()));
    let one = e.get("/api/plugins/internet-archive").await.json();
    assert_eq!(one["status"]["ok"], true, "{one}");
    assert!(one["status"]["message"].as_str().unwrap().starts_with("Ready"), "{one}");
    let s = e.get("/api/plugins/internet-archive/settings").await.json();
    let keys: Vec<&str> = s["fields"].as_array().unwrap().iter().map(|f| f["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["format", "collections", "max_items"]);
    assert_eq!((s["values"]["format"].as_str(), s["values"]["max_items"].as_i64()), (Some("flac"), Some(8)));
    let r = e.put("/api/plugins/internet-archive/settings", json!({ "format": "mp3", "max_items": 3 })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(e.ts.st.cfg.def("plugin.internet-archive.format").map(|_| ()), None, "its keys are its own, not Encore's");
    let logs = e.get("/api/plugins/internet-archive/logs").await.json();
    assert!(logs["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("installed 0.1.0 (upload")), "{logs}");
    assert_eq!(encore_core::ids::sha256_hex(std::fs::read(&path).unwrap()), before, "the owner's zip is not changed");
    let catalog = e.get("/api/plugins").await.json();
    let ia = catalog["plugins"].as_array().unwrap().iter().find(|p| p["id"] == "internet-archive").unwrap().clone();
    assert_eq!(ia["origin"], "upload");
    e.ts.remove();
}

/// docs/plugins/example-tone, when the docs have it: zipped as an author would, uploaded, described, searched
/// and downloaded through the registry.
#[tokio::test]
async fn the_example_plugin_of_the_docs_installs_when_it_is_there() {
    let dir = repo().join("docs/plugins/example-tone");
    if !dir.join("plugin.toml").is_file() {
        eprintln!("skipped: docs/plugins/example-tone is not there yet");
        return;
    }
    if !has_python() {
        return;
    }
    let e = env("example").await;
    let r = e.upload(zip_folder(&dir)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let one = e.get(&format!("/api/plugins/{id}")).await.json();
    assert_eq!(one["status"]["ok"], true, "{one}");
    let results = e.ts.st.plugins.search(&id, &json!({ "kind": "track", "query": "tone" }), None).await.unwrap_or_default();
    if let Some(first) = results.first() {
        let folder = e.ts.dir.join("downloads/example");
        let got = e.ts.st.plugins.download(&id, &json!({ "key": first["key"], "title": first["title"], "payload": first["payload"] }), &folder, None).await.unwrap();
        assert!(!got.files.is_empty());
    }
    e.ts.remove();
}

/// The old apps' Download tools auto-update switches (`tools_auto_<id>` in the settings) and the plugin card's
/// switch are one and the same for the alias release: either path moves the other.
#[tokio::test]
async fn the_old_auto_update_switch_and_the_plugins_are_one() {
    use encore_db::RowExt;
    let e = env("auto-keys").await;
    e.ts.st.plugins.adopt("ytdlp", None).unwrap();
    let auto = |e: &Env| e.ts.st.plugins.row("ytdlp").unwrap().b("auto_update");
    assert!(auto(&e) && e.ts.st.cfg.bool("tools_auto_ytdlp"));
    // the old apps: PUT the setting
    let r = e.put("/api/admin/settings", json!({ "tools_auto_ytdlp": false })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(!auto(&e), "the plugin's auto-update follows the old switch");
    // the Plugins page: the card's switch
    let r = e.post("/api/plugins/ytdlp/auto-update", json!({ "on": true })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(auto(&e));
    assert!(e.ts.st.cfg.bool("tools_auto_ytdlp"), "the old switch shows it");
    assert_eq!(e.get("/api/admin/settings").await.json()["tools_auto_ytdlp"], true);
    e.ts.remove();
}

/// The old Download tools routes answer in their old shape for one more release.
#[tokio::test]
async fn the_old_tools_routes_keep_their_shape() {
    let e = env("tools").await;
    let r = e.get("/api/system/tools").await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    let ids: Vec<&str> = v.as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["ytdlp", "streamrip", "cadenza", "slskd", "translator"]);
    for t in v.as_array().unwrap() {
        for k in ["id", "name", "installed", "source", "latest", "update_available", "checked_at", "error", "note", "updatable", "resettable"] {
            assert!(t.get(k).is_some(), "{k} missing in {t}");
        }
    }
    assert_eq!(e.post("/api/system/tools/nope/update", json!({})).await.status, StatusCode::NOT_FOUND);
    // yt-dlp and streamrip have no bundled copy to go back to: the old apps' "Use the bundled version" is not
    // offered, and asking anyway leaves the plugin installed
    for id in ["ytdlp", "streamrip"] {
        e.ts.st.plugins.adopt(id, None).unwrap();
    }
    let v = e.get("/api/system/tools").await.json();
    for t in v.as_array().unwrap().iter().filter(|t| t["id"] == "ytdlp" || t["id"] == "streamrip") {
        assert_eq!(t["resettable"], false, "{t}");
    }
    let r = e.post("/api/system/tools/ytdlp/reset", json!({})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("There is no bundled copy since 0.32"), "{}", r.text());
    assert!(e.ts.st.plugins.installed("ytdlp") && e.ts.st.plugins.installed("streamrip"));
    e.ts.remove();
}
