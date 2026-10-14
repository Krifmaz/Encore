//! Each download client against a small mock of its API (no network), plus the blackhole folders.
use super::*;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use std::sync::{Arc, Mutex};

struct Env {
    st: AppState,
    dir: PathBuf,
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-dl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        crate::build_state().unwrap()
    };
    Env { st, dir }
}

async fn serve(app: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    format!("http://{addr}")
}

type Log = Arc<Mutex<Vec<Value>>>;

fn basic(h: &HeaderMap) -> String {
    use base64::Engine;
    let v = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let b = v.strip_prefix("Basic ").unwrap_or("");
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b).unwrap_or_default()).unwrap_or_default()
}

/// A tiny but valid .torrent: one file "Some Album.flac".
fn torrent_bytes() -> Vec<u8> {
    let mut d = b"d8:announce17:http://t/announce4:infod6:lengthi1234e4:name15:Some Album.flac12:piece lengthi16384e6:pieces20:".to_vec();
    d.extend([7u8; 20]);
    d.extend(b"ee");
    d
}

#[test]
fn ids_name_their_client() {
    assert_eq!(Client::for_id("tr:ABC"), (Client::Transmission, "ABC"));
    assert_eq!(Client::for_id("nzbget:7"), (Client::NzbGet, "7"));
    assert_eq!(Client::for_id("sab:SABnzbd_nzo_1"), (Client::Sab, "SABnzbd_nzo_1"));
    assert_eq!(Client::for_id("qbit:encore-3"), (Client::Qbit, "encore-3"));
    assert_eq!(Client::for_id("tbh:Artist - Album: Deluxe"), (Client::TorrentHole, "Artist - Album: Deluxe"));
    assert_eq!(Client::for_id("encore-3"), (Client::Qbit, "encore-3"));
}

#[test]
fn torrent_and_magnet_hashes() {
    use sha1::{Digest, Sha1};
    let b = torrent_bytes();
    let (hash, name) = torrent_info(&b).unwrap();
    let start = b.windows(5).position(|w| w == b"4:inf").unwrap() + 6;
    assert_eq!(hash, hex::encode_upper(Sha1::digest(&b[start..b.len() - 1])));
    assert_eq!(name, "Some Album.flac");
    let (h, n) = magnet_info("magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=Some+Album%20FLAC").unwrap();
    assert_eq!(h, "C12FE1C06BBA254A9DC9F519B335AA7C1367A88A");
    assert_eq!(n, "Some Album FLAC");
    // base32 form of the same hash
    let (h32, _) = magnet_info("magnet:?xt=urn:btih:YEX6DQDLXISUVHOJ6UM3GNNKPQJWPKEK").unwrap();
    assert_eq!(h32, "C12FE1C06BBA254A9DC9F519B335AA7C1367A88A");
    assert!(magnet_info("magnet:?dn=x").is_none());
}

#[test]
fn xmlrpc_answers() {
    assert_eq!(rtorrent::parse("<methodResponse><params><param><value><i8>42</i8></value></param></params></methodResponse>").unwrap(), "42");
    assert_eq!(rtorrent::parse("<methodResponse><params><param><value><string>/a &amp; b</string></value></param></params></methodResponse>").unwrap(), "/a & b");
    assert_eq!(rtorrent::parse("<methodResponse><params><param><value>bare</value></param></params></methodResponse>").unwrap(), "bare");
    assert_eq!(rtorrent::parse("<methodResponse><params><param><value><string/></value></param></params></methodResponse>").unwrap(), "");
    let e = rtorrent::parse("<methodResponse><fault><value><struct><member><name>faultCode</name><value><i4>-501</i4></value></member><member><name>faultString</name><value><string>Could not find info-hash.</string></value></member></struct></value></fault></methodResponse>").unwrap_err();
    assert_eq!(e.to_string(), "rTorrent: Could not find info-hash.");
    let b = rtorrent::body("load.start", &[rtorrent::X::S("".into()), rtorrent::X::S("magnet:?a&b".into())]);
    assert!(b.contains("<methodName>load.start</methodName>") && b.contains("magnet:?a&amp;b"), "{b}");
}

#[tokio::test]
async fn transmission_session_id_add_poll_remove() {
    let env = setup("transmission");
    let log: Log = Default::default();
    let l2 = log.clone();
    let app = Router::new().route("/transmission/rpc", post(move |h: HeaderMap, body: String| {
        let log = l2.clone();
        async move {
            if basic(&h) != "me:secret" {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            if h.get("x-transmission-session-id").map(|v| v.to_str().unwrap()) != Some("sid-1") {
                return (StatusCode::CONFLICT, [("X-Transmission-Session-Id", "sid-1")], "").into_response();
            }
            let v: Value = serde_json::from_str(&body).unwrap();
            log.lock().unwrap().push(v.clone());
            let args = match v["method"].as_str().unwrap() {
                "session-get" => json!({ "version": "4.0.5 (a6fe2a64aa)" }),
                "torrent-add" => json!({ "torrent-added": { "hashString": "abc123", "id": 1, "name": "Album" } }),
                "torrent-get" => json!({ "torrents": [{ "hashString": "abc123", "name": "Album", "percentDone": 1.0, "status": 6, "error": 0, "downloadDir": "/elsewhere/torrents", "rateDownload": 0, "peersSendingToUs": 0 }] }),
                _ => json!({}),
            };
            axum::Json(json!({ "result": "success", "arguments": args })).into_response()
        }
    }));
    let url = serve(app).await;
    let st = &env.st;
    st.cfg.set_many(&json!({ "torrent_client": "transmission", "transmission_url": format!("{url}/"), "transmission_user": "me", "transmission_password": "secret", "transmission_download_path": "/downloads/tr" }));
    let c = Client::torrent(st);
    assert_eq!(c, Client::Transmission);
    assert!(c.ready(st));
    assert_eq!(c.status(st).await.unwrap(), " 4.0.5");
    let id = c.add(st, "magnet:?xt=urn:btih:abc", "Album", 5).await.unwrap();
    assert_eq!(id, "tr:abc123");
    let (c2, own) = Client::for_id(&id);
    match c2.poll(st, own).await.unwrap() {
        Poll::Done(p) => assert_eq!(p, PathBuf::from("/downloads/tr/Album")),
        _ => panic!("expected done"),
    }
    c2.remove(st, own).await;
    let calls = log.lock().unwrap().clone();
    let add = calls.iter().find(|c| c["method"] == "torrent-add").unwrap();
    assert_eq!(add["arguments"]["download-dir"], "/downloads/tr");
    assert_eq!(add["arguments"]["labels"], json!(["encore"]));
    let rm = calls.iter().find(|c| c["method"] == "torrent-remove").unwrap();
    assert_eq!(rm["arguments"], json!({ "ids": ["abc123"], "delete-local-data": true }));
}

#[tokio::test]
async fn deluge_login_connect_add_poll() {
    let env = setup("deluge");
    let log: Log = Default::default();
    let progress = Arc::new(Mutex::new(40.0));
    let (l2, p2) = (log.clone(), progress.clone());
    let app = Router::new().route("/json", post(move |h: HeaderMap, body: String| {
        let (log, progress) = (l2.clone(), p2.clone());
        async move {
            let v: Value = serde_json::from_str(&body).unwrap();
            let m = v["method"].as_str().unwrap().to_string();
            log.lock().unwrap().push(v.clone());
            if m == "auth.login" {
                let ok = v["params"][0] == "pw";
                return ([("Set-Cookie", "_session_id=s3cr3t; Path=/json")], axum::Json(json!({ "result": ok, "error": null, "id": 1 }))).into_response();
            }
            if h.get("cookie").map(|c| c.to_str().unwrap()) != Some("_session_id=s3cr3t") {
                return axum::Json(json!({ "result": null, "error": { "message": "Not authenticated", "code": 1 }, "id": 1 })).into_response();
            }
            let r = match m.as_str() {
                "web.connected" => json!(false),
                "web.get_hosts" => json!([["host1", "127.0.0.1", 58846, "localclient"]]),
                "web.connect" => json!([]),
                "daemon.info" => json!("2.1.1"),
                "core.add_torrent_magnet" => json!("hash1"),
                "label.add" | "label.set_torrent" => return axum::Json(json!({ "result": null, "error": { "message": "Unknown method", "code": 2 }, "id": 1 })).into_response(),
                "core.get_torrent_status" => {
                    let p = *progress.lock().unwrap();
                    json!({ "name": "Album", "progress": p, "state": if p >= 100.0 { "Seeding" } else { "Downloading" }, "save_path": "/deluge/dl", "download_payload_rate": 2_000_000, "message": "OK" })
                }
                _ => json!(true),
            };
            axum::Json(json!({ "result": r, "error": null, "id": 1 })).into_response()
        }
    }));
    let url = serve(app).await;
    let st = &env.st;
    st.cfg.set_many(&json!({ "torrent_client": "deluge", "deluge_url": url, "deluge_password": "pw", "deluge_download_path": "/downloads/deluge" }));
    let c = Client::torrent(st);
    assert_eq!(c.status(st).await.unwrap(), " 2.1.1");
    let id = c.add(st, "magnet:?xt=urn:btih:abc", "Album", 5).await.unwrap();
    assert_eq!(id, "deluge:hash1");
    match Client::for_id(&id).0.poll(st, "hash1").await.unwrap() {
        Poll::Progress(p, msg) => {
            assert_eq!(p, 40.0);
            assert_eq!(msg, "Deluge: 2.0 MB/s");
        }
        _ => panic!("expected progress"),
    }
    *progress.lock().unwrap() = 100.0;
    match c.poll(st, "hash1").await.unwrap() {
        Poll::Done(p) => assert_eq!(p, PathBuf::from("/downloads/deluge/Album")),
        _ => panic!("expected done"),
    }
    c.remove(st, "hash1").await;
    let calls = log.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c["method"] == "web.connect" && c["params"] == json!(["host1"])));
    let add = calls.iter().find(|c| c["method"] == "core.add_torrent_magnet").unwrap();
    assert_eq!(add["params"][1]["download_location"], "/downloads/deluge");
    assert!(calls.iter().any(|c| c["method"] == "core.remove_torrent" && c["params"] == json!(["hash1", true])));
    // a wrong password is a plain message
    st.cfg.set_many(&json!({ "deluge_password": "nope" }));
    assert_eq!(c.status(st).await.unwrap_err().to_string(), "Deluge login failed: wrong password");
}

#[tokio::test]
async fn rtorrent_raw_start_and_poll() {
    let env = setup("rtorrent");
    let log: Log = Default::default();
    let l2 = log.clone();
    let tb = torrent_bytes();
    let app = Router::new()
        .route("/file.torrent", get(move || {
            let tb = tb.clone();
            async move { tb }
        }))
        .route("/go", get(|| async { (StatusCode::FOUND, [("Location", "/file.torrent")]) }))
        .route("/RPC2", post(move |h: HeaderMap, body: String| {
            let log = l2.clone();
            async move {
                if basic(&h) != "rt:pw" {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                let m = body.split("<methodName>").nth(1).and_then(|s| s.split("</methodName>").next()).unwrap_or("").to_string();
                log.lock().unwrap().push(json!({ "method": m, "body": body }));
                let v = match m.as_str() {
                    "system.client_version" => "<string>0.9.8</string>",
                    "load.raw_start" | "d.erase" => "<i4>0</i4>",
                    "d.size_bytes" => "<i8>1000</i8>",
                    "d.complete" => "<i8>1</i8>",
                    "d.base_path" => "<string>/rt/data/Some Album.flac</string>",
                    _ => "<i8>0</i8>",
                };
                ([("Content-Type", "text/xml")], format!("<?xml version=\"1.0\"?><methodResponse><params><param><value>{v}</value></param></params></methodResponse>")).into_response()
            }
        }));
    let url = serve(app).await;
    let st = &env.st;
    st.cfg.set_many(&json!({ "torrent_client": "rtorrent", "rtorrent_url": format!("{url}/RPC2"), "rtorrent_user": "rt", "rtorrent_password": "pw", "rtorrent_download_path": "/downloads/rt" }));
    let c = Client::torrent(st);
    assert_eq!(c, Client::Rtorrent);
    assert_eq!(c.status(st).await.unwrap(), " 0.9.8");
    let id = c.add(st, &format!("{url}/go"), "Some Album", 9).await.unwrap();
    let (hash, _) = torrent_info(&torrent_bytes()).unwrap();
    assert_eq!(id, format!("rt:{hash}"));
    match c.poll(st, &hash).await.unwrap() {
        Poll::Done(p) => assert_eq!(p, PathBuf::from("/downloads/rt/Some Album.flac")),
        _ => panic!("expected done"),
    }
    c.remove(st, &hash).await;
    let calls = log.lock().unwrap().clone();
    let load = calls.iter().find(|c| c["method"] == "load.raw_start").unwrap()["body"].as_str().unwrap().to_string();
    assert!(load.contains("<base64>") && load.contains("d.directory.set=\"/downloads/rt\"") && load.contains("d.custom1.set=\"encore\""), "{load}");
    assert!(calls.iter().any(|c| c["method"] == "d.erase" && c["body"].as_str().unwrap().contains(&hash)));
}

#[tokio::test]
async fn nzbget_append_history_remove() {
    let env = setup("nzbget");
    let log: Log = Default::default();
    let l2 = log.clone();
    let app = Router::new().route("/jsonrpc", post(move |h: HeaderMap, body: String| {
        let log = l2.clone();
        async move {
            if basic(&h) != "nzbget:tegbzn" {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let v: Value = serde_json::from_str(&body).unwrap();
            log.lock().unwrap().push(v.clone());
            let r = match v["method"].as_str().unwrap() {
                "version" => json!("24.3"),
                "append" => json!(7),
                "listgroups" => json!([{ "NZBID": 3, "FileSizeMB": 100, "RemainingSizeMB": 50, "Status": "DOWNLOADING" }]),
                "history" => json!([{ "NZBID": 7, "Status": "SUCCESS/ALL", "DestDir": "/nzbget/done/encore-1", "FinalDir": "" }]),
                _ => json!(true),
            };
            axum::Json(json!({ "version": "1.1", "result": r, "error": null })).into_response()
        }
    }));
    let url = serve(app).await;
    let st = &env.st;
    let done = env.dir.join("usenet");
    std::fs::create_dir_all(done.join("encore-1")).unwrap();
    std::fs::write(done.join("encore-1/01.flac"), b"x").unwrap();
    st.cfg.set_many(&json!({ "nzb_client": "nzbget", "nzbget_url": url, "nzbget_password": "tegbzn", "nzbget_download_path": done.to_string_lossy() }));
    let c = Client::usenet(st);
    assert_eq!(c, Client::NzbGet);
    assert_eq!(c.status(st).await.unwrap(), " 24.3");
    let id = c.add(st, "https://indexer/get/1.nzb", "Album", 1).await.unwrap();
    assert_eq!(id, "nzbget:7");
    match c.poll(st, "7").await.unwrap() {
        Poll::Done(p) => assert_eq!(p, done.join("encore-1")),
        _ => panic!("expected done"),
    }
    match c.poll(st, "3").await.unwrap() {
        Poll::Progress(p, m) => assert_eq!((p, m.as_str()), (50.0, "NZBGet: downloading")),
        _ => panic!("expected progress"),
    }
    c.remove(st, "7").await;
    assert!(!done.join("encore-1").exists(), "the finished folder is cleaned up");
    assert!(done.exists(), "never the download folder itself");
    let calls = log.lock().unwrap().clone();
    let add = calls.iter().find(|c| c["method"] == "append").unwrap();
    assert_eq!(add["params"][0], "encore-1.nzb");
    assert_eq!(add["params"][1], "https://indexer/get/1.nzb");
    assert_eq!(add["params"][2], "encore");
    assert!(calls.iter().any(|c| c["method"] == "editqueue" && c["params"] == json!(["GroupFinalDelete", "", [7]])));
    assert!(calls.iter().any(|c| c["method"] == "editqueue" && c["params"] == json!(["HistoryDelete", "", [7]])));
}

#[tokio::test]
async fn blackhole_torrent_and_nzb() {
    let env = setup("blackhole");
    let app = Router::new().route("/get.nzb", get(|| async { "<?xml version=\"1.0\"?><nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\"></nzb>" }));
    let url = serve(app).await;
    let st = &env.st;
    let (watch, done) = (env.dir.join("watch"), env.dir.join("done"));
    std::fs::create_dir_all(&done).unwrap();
    st.cfg.set_many(&json!({ "torrent_client": "blackhole", "torrent_blackhole_watch": watch.to_string_lossy(), "torrent_blackhole_completed": done.to_string_lossy(),
        "nzb_client": "blackhole", "nzb_blackhole_watch": watch.to_string_lossy(), "nzb_blackhole_completed": done.to_string_lossy() }));
    let c = Client::torrent(st);
    assert_eq!(c, Client::TorrentHole);
    assert!(c.status(st).await.is_ok());
    let id = c.add(st, "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=Artist+-+Album+(2020)+[FLAC]", "ignored", 4).await.unwrap();
    assert_eq!(id, "tbh:Artist - Album (2020) [FLAC]");
    assert!(std::fs::read_to_string(watch.join("Artist - Album (2020) [FLAC].magnet")).unwrap().starts_with("magnet:?xt="));
    let own = Client::for_id(&id).1;
    assert!(matches!(c.poll(st, own).await.unwrap(), Poll::Progress(..)), "nothing finished yet");
    let folder = done.join("Artist - Album (2020) [FLAC]");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("01.flac.part"), b"half").unwrap();
    assert!(matches!(c.poll(st, own).await.unwrap(), Poll::Progress(..)));
    assert!(matches!(c.poll(st, own).await.unwrap(), Poll::Progress(..)), "partial files still there");
    std::fs::remove_file(folder.join("01.flac.part")).unwrap();
    std::fs::write(folder.join("01.flac"), b"the whole file").unwrap();
    assert!(matches!(c.poll(st, own).await.unwrap(), Poll::Progress(..)), "size must hold still for one poll");
    match c.poll(st, own).await.unwrap() {
        Poll::Done(p) => assert_eq!(p, folder),
        _ => panic!("expected done"),
    }
    c.remove(st, own).await;
    assert!(!folder.exists() && !watch.join("Artist - Album (2020) [FLAC].magnet").exists());

    let u = Client::usenet(st);
    assert_eq!(u, Client::NzbHole);
    let id = u.add(st, &format!("{url}/get.nzb"), "Artist: Album / 2020", 2).await.unwrap();
    assert_eq!(id, "nbh:Artist_ Album _ 2020");
    assert!(watch.join("Artist_ Album _ 2020.nzb").exists());
    std::fs::write(done.join("Artist_ Album _ 2020.flac"), b"one file").unwrap();
    let own = Client::for_id(&id).1;
    assert!(matches!(u.poll(st, own).await.unwrap(), Poll::Progress(..)));
    assert!(matches!(u.poll(st, own).await.unwrap(), Poll::Done(_)));
    // a non-NZB answer is refused
    assert!(u.add(st, &format!("{url}/missing"), "x", 3).await.is_err());
}
