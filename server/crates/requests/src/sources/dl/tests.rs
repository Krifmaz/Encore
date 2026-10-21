//! Each download client against a small mock of its API (no network), plus the blackhole folders.
use super::*;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use std::sync::{Arc, Mutex};

struct Env {
    st: crate::test_ctx::TestCtx,
    dir: PathBuf,
}

fn setup(name: &str) -> Env {
    let st = crate::test_ctx::TestCtx::new(&format!("dl-{name}"));
    let dir = st.dir.clone();
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

const NZB: &str = "<?xml version=\"1.0\"?><nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\"></nzb>";

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

/// A finished download's path as the client reports it, where Encore sees it: the same path, else under
/// the folder Encore sees with the folders the client added (SABnzbd's category folder), and a Windows
/// client's path too (diagnosis item 23: only the last name was kept, so "complete/music/encore-12" seen
/// from "complete", and any "D:\…" path, were never found).
#[test]
fn a_finished_path_is_found_under_the_folder_encore_sees() {
    let env = setup("local");
    let st = &env.st;
    let root = env.dir.join("complete");
    std::fs::create_dir_all(root.join("music/encore-12")).unwrap();
    std::fs::create_dir_all(root.join("encore-13")).unwrap();
    st.cfg().set_many(&json!({ "sab_download_path": root.to_string_lossy() }));
    let at = |remote: &str| local(st, remote, "sab_download_path");
    // mounted the same way
    assert_eq!(at(&root.join("music/encore-12").to_string_lossy()), root.join("music/encore-12"));
    // another mount: the name under the folder Encore sees, with the category folder when that is where it is
    assert_eq!(at("/data/usenet/complete/encore-13"), root.join("encore-13"));
    assert_eq!(at("/data/usenet/complete/music/encore-12"), root.join("music/encore-12"));
    assert_eq!(at("D:\\Usenet\\complete\\music\\encore-12"), root.join("music/encore-12"));
    assert_eq!(at("D:\\Usenet\\complete\\encore-13"), root.join("encore-13"));
    // nowhere to be seen: the name under the folder (what the request names), never above it
    assert_eq!(at("/data/../../etc/encore-14"), root.join("encore-14"));
    assert_eq!(at("D:\\Usenet\\encore-14"), root.join("encore-14"));
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
                // q1 waits in Transmission's queue, s1 was stopped before it finished
                "torrent-get" if v["arguments"]["ids"] == json!(["q1"]) => json!({ "torrents": [{ "hashString": "q1", "name": "A", "percentDone": 0.1, "status": 3, "error": 0, "downloadDir": "/x", "rateDownload": 0, "peersSendingToUs": 0 }] }),
                "torrent-get" if v["arguments"]["ids"] == json!(["s1"]) => json!({ "torrents": [{ "hashString": "s1", "name": "A", "percentDone": 0.2, "status": 0, "error": 0, "downloadDir": "/x", "rateDownload": 0, "peersSendingToUs": 0 }] }),
                "torrent-get" => json!({ "torrents": [{ "hashString": "abc123", "name": "Album", "percentDone": 1.0, "status": 6, "error": 0, "downloadDir": "/elsewhere/torrents", "rateDownload": 0, "peersSendingToUs": 0 }] }),
                _ => json!({}),
            };
            axum::Json(json!({ "result": "success", "arguments": args })).into_response()
        }
    }));
    let url = serve(app).await;
    let st = &env.st;
    st.cfg().set_many(&json!({ "torrent_client": "transmission", "transmission_url": format!("{url}/"), "transmission_user": "me", "transmission_password": "secret", "transmission_download_path": "/downloads/tr" }));
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
    // queued: no stall; stopped: started again (diagnosis item 15, as for qBittorrent)
    assert!(matches!(c2.poll(st, "q1").await.unwrap(), Poll::Queued(p, m) if p == 10.0 && m == "Transmission: queued – its limit of downloads at once is reached"));
    assert!(matches!(c2.poll(st, "s1").await.unwrap(), Poll::Progress(p, m) if p == 20.0 && m == "Transmission: starting the download (it was stopped)"));
    c2.remove(st, own).await;
    let calls = log.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c["method"] == "torrent-start" && c["arguments"] == json!({ "ids": ["s1"] })));
    let add = calls.iter().find(|c| c["method"] == "torrent-add").unwrap();
    assert_eq!(add["arguments"]["download-dir"], "/downloads/tr");
    assert_eq!(add["arguments"]["labels"], json!(["encore"]));
    assert_eq!(add["arguments"]["paused"], false, "started whatever Transmission's own setting says");
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
                // q1 waits in Deluge's queue, p1 was paused before it finished
                "core.get_torrent_status" if v["params"][0] == "q1" => json!({ "name": "A", "progress": 10.0, "state": "Queued", "save_path": "/x", "download_payload_rate": 0, "message": "OK" }),
                "core.get_torrent_status" if v["params"][0] == "p1" => json!({ "name": "A", "progress": 20.0, "state": "Paused", "save_path": "/x", "download_payload_rate": 0, "message": "OK" }),
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
    st.cfg().set_many(&json!({ "torrent_client": "deluge", "deluge_url": url, "deluge_password": "pw", "deluge_download_path": "/downloads/deluge" }));
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
    // queued: no stall; paused: resumed (diagnosis item 15, as for qBittorrent)
    assert!(matches!(c.poll(st, "q1").await.unwrap(), Poll::Queued(p, m) if p == 10.0 && m == "Deluge: queued – its limit of downloads at once is reached"));
    assert!(matches!(c.poll(st, "p1").await.unwrap(), Poll::Progress(p, m) if p == 20.0 && m == "Deluge: starting the download (it was paused)"));
    c.remove(st, "hash1").await;
    let calls = log.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c["method"] == "core.resume_torrents" && c["params"] == json!([["p1"]])));
    assert!(calls.iter().any(|c| c["method"] == "web.connect" && c["params"] == json!(["host1"])));
    let add = calls.iter().find(|c| c["method"] == "core.add_torrent_magnet").unwrap();
    assert_eq!(add["params"][1]["download_location"], "/downloads/deluge");
    assert_eq!(add["params"][1]["add_paused"], false, "started whatever Deluge's own setting says");
    assert!(calls.iter().any(|c| c["method"] == "core.remove_torrent" && c["params"] == json!(["hash1", true])));
    // a wrong password is a plain message
    st.cfg().set_many(&json!({ "deluge_password": "nope" }));
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
    st.cfg().set_many(&json!({ "torrent_client": "rtorrent", "rtorrent_url": format!("{url}/RPC2"), "rtorrent_user": "rt", "rtorrent_password": "pw", "rtorrent_download_path": "/downloads/rt" }));
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
    let app = Router::new().route("/get/1.nzb", get(|| async { NZB })).route("/jsonrpc", post(move |h: HeaderMap, body: String| {
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
                "listgroups" => json!([{ "NZBID": 3, "FileSizeMB": 100, "RemainingSizeMB": 50, "Status": "DOWNLOADING" }, { "NZBID": 4, "FileSizeMB": 100, "RemainingSizeMB": 100, "Status": "QUEUED" }]),
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
    st.cfg().set_many(&json!({ "nzb_client": "nzbget", "nzbget_url": url, "nzbget_password": "tegbzn", "nzbget_download_path": done.to_string_lossy() }));
    let c = Client::usenet(st);
    assert_eq!(c, Client::NzbGet);
    assert_eq!(c.status(st).await.unwrap(), " 24.3");
    let id = c.add(st, &format!("{url}/get/1.nzb"), "Album", 1).await.unwrap();
    assert_eq!(id, "nzbget:7");
    match c.poll(st, "7").await.unwrap() {
        Poll::Done(p) => assert_eq!(p, done.join("encore-1")),
        _ => panic!("expected done"),
    }
    match c.poll(st, "3").await.unwrap() {
        Poll::Progress(p, m) => assert_eq!((p, m.as_str()), (50.0, "NZBGet: downloading")),
        _ => panic!("expected progress"),
    }
    assert!(matches!(c.poll(st, "4").await.unwrap(), Poll::Queued(_, m) if m == "NZBGet: queued behind its other downloads"));
    c.remove(st, "7").await;
    assert!(!done.join("encore-1").exists(), "the finished folder is cleaned up");
    assert!(done.exists(), "never the download folder itself");
    let calls = log.lock().unwrap().clone();
    let add = calls.iter().find(|c| c["method"] == "append").unwrap();
    assert_eq!(add["params"][0], "encore-1.nzb");
    // Encore fetched the NZB: NZBGet gets its content, not Prowlarr's link
    use base64::Engine;
    assert_eq!(add["params"][1], base64::engine::general_purpose::STANDARD.encode(NZB));
    assert_eq!(add["params"][2], "encore");
    assert!(calls.iter().any(|c| c["method"] == "editqueue" && c["params"] == json!(["GroupFinalDelete", "", [7]])));
    // the next release for the request is never a "duplicate" of one Encore turned away
    assert_eq!(add["params"][8], "FORCE");
    // unpacked whatever the category says: Encore can't use archives
    assert_eq!(add["params"][9], json!([{ "Name": "*Unpack:", "Value": "yes" }]));
    assert!(calls.iter().any(|c| c["method"] == "editqueue" && c["params"] == json!(["HistoryFinalDelete", "", [7]])));
    assert!(!calls.iter().any(|c| c["method"] == "editqueue" && c["params"][0] == "HistoryDelete"), "hidden history entries still count as duplicates");
}

#[tokio::test]
async fn blackhole_torrent_and_nzb() {
    let env = setup("blackhole");
    let app = Router::new().route("/get.nzb", get(|| async { "<?xml version=\"1.0\"?><nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\"></nzb>" }));
    let url = serve(app).await;
    let st = &env.st;
    let (watch, done) = (env.dir.join("watch"), env.dir.join("done"));
    std::fs::create_dir_all(&done).unwrap();
    st.cfg().set_many(&json!({ "torrent_client": "blackhole", "torrent_blackhole_watch": watch.to_string_lossy(), "torrent_blackhole_completed": done.to_string_lossy(),
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

// ---- one song out of a multi-file torrent ----------------------------------------------------------

const ALBUM_FILES: [&str; 4] = ["Rodeo/01 Pornography.flac", "Rodeo/02 Oh My Dis Side.flac", "Rodeo/04 90210.flac", "Rodeo/cover.jpg"];

#[tokio::test]
async fn qbit_selects_one_file() {
    let env = setup("qbit-select");
    let log: Log = Default::default();
    let (l1, l2) = (log.clone(), log.clone());
    let app = Router::new()
        .route("/api/v2/auth/login", post(|| async { ([("Set-Cookie", "SID=x; path=/")], "Ok.") }))
        .route("/api/v2/torrents/info", get(|| async { axum::Json(json!([{ "hash": "h1", "state": "downloading", "progress": 0.1 }])) }))
        .route("/api/v2/torrents/files", get(move |q: axum::extract::Query<std::collections::HashMap<String, String>>| {
            let log = l1.clone();
            async move {
                log.lock().unwrap().push(json!({ "files": q.get("hash") }));
                axum::Json(json!(ALBUM_FILES.iter().enumerate().map(|(i, n)| json!({ "index": i, "name": n })).collect::<Vec<_>>()))
            }
        }))
        .route("/api/v2/torrents/filePrio", post(move |mut m: axum::extract::Multipart| {
            let log = l2.clone();
            async move {
                let mut v = json!({});
                while let Ok(Some(f)) = m.next_field().await {
                    let k = f.name().unwrap_or("").to_string();
                    v[k] = json!(f.text().await.unwrap_or_default());
                }
                log.lock().unwrap().push(v);
                "Ok."
            }
        }));
    let url = serve(app).await;
    let st = &env.st;
    st.cfg().set_many(&json!({ "torrent_client": "qbittorrent", "qbit_url": url }));
    let id = "qbit:encore-5";
    match crate::sources::torznab::select_song(st, id, "Travis Scott", "90210", None).await.unwrap() {
        crate::sources::torznab::SongFile::Only(n) => assert_eq!(n, "Rodeo/04 90210.flac"),
        _ => panic!("expected one file"),
    }
    let calls = log.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c["files"] == "h1"));
    assert!(calls.iter().any(|c| c["hash"] == "h1" && c["id"] == "0|1|3" && c["priority"] == "0"), "{calls:?}");
    assert!(calls.iter().any(|c| c["hash"] == "h1" && c["id"] == "2" && c["priority"] == "1"), "{calls:?}");
    // a song that isn't on the album
    assert!(matches!(crate::sources::torznab::select_song(st, id, "Travis Scott", "Sicko Mode", None).await.unwrap(), crate::sources::torznab::SongFile::Missing));
}

#[tokio::test]
async fn transmission_deluge_rtorrent_select_files() {
    // Transmission: files-unwanted / files-wanted by index
    let env = setup("select-others");
    let st = &env.st;
    let log: Log = Default::default();
    let l = log.clone();
    let tr = Router::new().route("/transmission/rpc", post(move |h: HeaderMap, body: String| {
        let log = l.clone();
        async move {
            if h.get("x-transmission-session-id").is_none() {
                return (StatusCode::CONFLICT, [("X-Transmission-Session-Id", "s")], "").into_response();
            }
            let v: Value = serde_json::from_str(&body).unwrap();
            log.lock().unwrap().push(v.clone());
            let args = match v["method"].as_str().unwrap() {
                "torrent-get" => json!({ "torrents": [{ "hashString": "abc", "files": ALBUM_FILES.iter().map(|n| json!({ "name": n, "length": 1 })).collect::<Vec<_>>() }] }),
                _ => json!({}),
            };
            axum::Json(json!({ "result": "success", "arguments": args })).into_response()
        }
    }));
    let url = serve(tr).await;
    st.cfg().set_many(&json!({ "torrent_client": "transmission", "transmission_url": url }));
    match crate::sources::torznab::select_song(st, "tr:abc", "Travis Scott", "Oh My Dis Side", Some(2)).await.unwrap() {
        crate::sources::torznab::SongFile::Only(n) => assert_eq!(n, "Rodeo/02 Oh My Dis Side.flac"),
        _ => panic!("expected one file"),
    }
    let set = log.lock().unwrap().iter().find(|c| c["method"] == "torrent-set").cloned().unwrap();
    assert_eq!(set["arguments"], json!({ "ids": ["abc"], "files-wanted": [1], "files-unwanted": [0, 2, 3] }));

    // Deluge: file_priorities, 0 = skip
    let log: Log = Default::default();
    let l = log.clone();
    let dl = Router::new().route("/json", post(move |body: String| {
        let log = l.clone();
        async move {
            let v: Value = serde_json::from_str(&body).unwrap();
            log.lock().unwrap().push(v.clone());
            let r = match v["method"].as_str().unwrap() {
                "auth.login" => return ([("Set-Cookie", "_session_id=c; Path=/json")], axum::Json(json!({ "result": true, "error": null, "id": 1 }))).into_response(),
                "web.connected" => json!(true),
                "core.get_torrent_status" => json!({ "files": ALBUM_FILES.iter().enumerate().map(|(i, n)| json!({ "index": i, "path": n })).collect::<Vec<_>>() }),
                _ => json!(null),
            };
            axum::Json(json!({ "result": r, "error": null, "id": 1 })).into_response()
        }
    }));
    let url = serve(dl).await;
    st.cfg().set_many(&json!({ "torrent_client": "deluge", "deluge_url": url, "deluge_password": "pw" }));
    assert!(matches!(crate::sources::torznab::select_song(st, "deluge:h", "Travis Scott", "90210", None).await.unwrap(), crate::sources::torznab::SongFile::Only(_)));
    let set = log.lock().unwrap().iter().find(|c| c["method"] == "core.set_torrent_options").cloned().unwrap();
    assert_eq!(set["params"], json!([["h"], { "file_priorities": [0, 0, 4, 0] }]));

    // rTorrent: f.priority.set per file, then d.update_priorities
    let log: Log = Default::default();
    let l = log.clone();
    let rt = Router::new().route("/RPC2", post(move |body: String| {
        let log = l.clone();
        async move {
            let m = body.split("<methodName>").nth(1).and_then(|s| s.split("</methodName>").next()).unwrap_or("").to_string();
            log.lock().unwrap().push(json!({ "method": m, "body": body }));
            let v = match m.as_str() {
                "d.size_bytes" => "<i8>5000</i8>".to_string(),
                "d.size_files" => format!("<i8>{}</i8>", ALBUM_FILES.len()),
                "f.path" => {
                    let i: usize = body.split(":f").nth(1).and_then(|s| s.split('<').next()).and_then(|s| s.parse().ok()).unwrap_or(0);
                    format!("<string>{}</string>", ALBUM_FILES[i])
                }
                _ => "<i8>0</i8>".to_string(),
            };
            ([("Content-Type", "text/xml")], format!("<?xml version=\"1.0\"?><methodResponse><params><param><value>{v}</value></param></params></methodResponse>")).into_response()
        }
    }));
    let url = serve(rt).await;
    st.cfg().set_many(&json!({ "torrent_client": "rtorrent", "rtorrent_url": format!("{url}/RPC2") }));
    assert!(matches!(crate::sources::torznab::select_song(st, "rt:HASH", "Travis Scott", "90210", None).await.unwrap(), crate::sources::torznab::SongFile::Only(_)));
    let calls = log.lock().unwrap().clone();
    let prios: Vec<String> = calls.iter().filter(|c| c["method"] == "f.priority.set").map(|c| c["body"].as_str().unwrap().to_string()).collect();
    assert_eq!(prios.len(), 4);
    assert!(prios[2].contains("HASH:f2") && prios[2].contains("<i8>1</i8>"), "{}", prios[2]);
    assert!(prios[0].contains("HASH:f0") && prios[0].contains("<i8>0</i8>"));
    assert!(calls.iter().any(|c| c["method"] == "d.update_priorities"));

    // Usenet and blackholes download whole releases
    st.cfg().set_many(&json!({ "nzb_client": "sabnzbd" }));
    assert!(matches!(crate::sources::torznab::select_song(st, "sab:SABnzbd_nzo_1", "Travis Scott", "90210", None).await.unwrap(), crate::sources::torznab::SongFile::Whole));
    assert!(matches!(crate::sources::torznab::select_song(st, "tbh:Rodeo", "Travis Scott", "90210", None).await.unwrap(), crate::sources::torznab::SongFile::Whole));
}

// ---- qBittorrent: Encore fetches the release, follows it by hash ------------------------------------

/// What the qBittorrent stand-in was sent and what it answers.
#[derive(Default)]
struct Qb {
    /// torrents/add's fields (a file part as "<file N bytes>")
    adds: Vec<Value>,
    /// every call: (path, query or form)
    calls: Vec<(String, Value)>,
    /// what torrents/info lists for the added hash
    listed: Option<Value>,
    /// answer torrents/add with "Fails."
    fails: bool,
    /// the session cookie qBittorrent knows (a restart forgets it)
    sid: String,
    logins: usize,
}

type QbState = Arc<Mutex<Qb>>;

async fn form_of(mut m: axum::extract::Multipart) -> Value {
    let mut v = json!({});
    while let Ok(Some(f)) = m.next_field().await {
        let k = f.name().unwrap_or("").to_string();
        let file = f.file_name().is_some();
        let bytes = f.bytes().await.unwrap_or_default();
        v[k] = if file { json!(format!("<file {} bytes>", bytes.len())) } else { json!(String::from_utf8_lossy(&bytes)) };
    }
    v
}

fn fake_qbit(q: QbState, tb: Vec<u8>) -> Router {
    let authed = |q: &QbState, h: &HeaderMap| h.get("cookie").and_then(|c| c.to_str().ok()) == Some(q.lock().unwrap().sid.as_str());
    let (q1, q2, q3, q4) = (q.clone(), q.clone(), q.clone(), q.clone());
    Router::new()
        .route("/api/v2/auth/login", post(move || {
            let q = q1.clone();
            async move {
                let mut g = q.lock().unwrap();
                g.logins += 1;
                g.sid = format!("SID=s{}", g.logins);
                ([("Set-Cookie", format!("{}; path=/", g.sid))], "Ok.")
            }
        }))
        .route("/api/v2/torrents/add", post(move |h: HeaderMap, m: axum::extract::Multipart| {
            let q = q2.clone();
            async move {
                if !authed(&q, &h) {
                    return (StatusCode::FORBIDDEN, "Forbidden").into_response();
                }
                let v = form_of(m).await;
                let mut g = q.lock().unwrap();
                g.calls.push(("torrents/add".into(), v.clone()));
                g.adds.push(v);
                (if g.fails { "Fails." } else { "Ok." }).into_response()
            }
        }))
        .route("/api/v2/torrents/info", get(move |h: HeaderMap, axum::extract::Query(p): axum::extract::Query<std::collections::HashMap<String, String>>| {
            let q = q3.clone();
            async move {
                if !authed(&q, &h) {
                    return (StatusCode::FORBIDDEN, "Forbidden").into_response();
                }
                let mut g = q.lock().unwrap();
                g.calls.push(("torrents/info".into(), json!(p)));
                let list = match &g.listed {
                    Some(t) if p.get("hashes").is_some_and(|x| Some(x.as_str()) == t["hash"].as_str()) || p.get("tag").is_some_and(|x| t["tags"].as_str().unwrap_or("").contains(x.as_str())) => json!([t]),
                    _ => json!([]),
                };
                axum::Json(list).into_response()
            }
        }))
        .route("/api/v2/torrents/{op}", post(move |h: HeaderMap, axum::extract::Path(op): axum::extract::Path<String>, m: axum::extract::Multipart| {
            let q = q4.clone();
            async move {
                if !authed(&q, &h) {
                    return (StatusCode::FORBIDDEN, "Forbidden").into_response();
                }
                let v = form_of(m).await;
                q.lock().unwrap().calls.push((format!("torrents/{op}"), v));
                "Ok.".into_response()
            }
        }))
        // Prowlarr's proxy links: one redirects to a magnet, one sends the .torrent
        .route("/prowlarr/1/download", get(|| async { (StatusCode::MOVED_PERMANENTLY, [("Location", "magnet:?xt=urn:btih:C12FE1C06BBA254A9DC9F519B335AA7C1367A88A&dn=Some+Album")]) }))
        .route("/prowlarr/2/download", get(move || {
            let tb = tb.clone();
            async move { tb }
        }))
}

/// qBittorrent gets the magnet or the .torrent from Encore, never Prowlarr's link (which it often can't
/// reach), started at once at the top of its queue; the download is followed by its hash, and a hash it
/// doesn't list yet is "missing" (the worker waits a while for it) (diagnosis item 2).
#[tokio::test]
async fn qbit_gets_the_release_from_encore_and_is_followed_by_hash() {
    let env = setup("qbit-add");
    let q: QbState = Default::default();
    let url = serve(fake_qbit(q.clone(), torrent_bytes())).await;
    let st = &env.st;
    st.cfg().set_many(&json!({ "torrent_client": "qbittorrent", "qbit_url": url, "qbit_category": "music", "qbit_download_path": "/downloads/torrents" }));
    let c = Client::torrent(st);
    // a link that redirects to a magnet: the magnet goes in `urls`
    let id = c.add(st, &format!("{url}/prowlarr/1/download?apikey=secret&link=x"), "Some Album", 5).await.unwrap();
    assert_eq!(id, "qbit:c12fe1c06bba254a9dc9f519b335aa7c1367a88a");
    let add = q.lock().unwrap().adds[0].clone();
    assert_eq!(add["urls"], "magnet:?xt=urn:btih:C12FE1C06BBA254A9DC9F519B335AA7C1367A88A&dn=Some+Album", "{add}");
    for (k, v) in [("category", "music"), ("tags", "encore-5"), ("savepath", "/downloads/torrents"), ("stopped", "false"), ("paused", "false"),
                   ("stopCondition", "None"), ("addToTopOfQueue", "true"), ("contentLayout", "Original")] {
        assert_eq!(add[k], v, "{k}: {add}");
    }
    // a link that sends the .torrent: the file goes as the `torrents` part
    let id2 = c.add(st, &format!("{url}/prowlarr/2/download?apikey=secret"), "Some Album", 6).await.unwrap();
    let (hash, _) = torrent_info(&torrent_bytes()).unwrap();
    assert_eq!(id2, format!("qbit:{}", hash.to_lowercase()));
    let add = q.lock().unwrap().adds[1].clone();
    assert_eq!(add["torrents"], format!("<file {} bytes>", torrent_bytes().len()), "{add}");
    assert!(add["urls"].is_null());

    let own = Client::for_id(&id).1.to_string();
    // not listed yet
    assert!(matches!(c.poll(st, &own).await.unwrap(), Poll::Missing(_)));
    let listed = |state: &str, progress: f64| json!({ "hash": own, "tags": "encore-5", "state": state, "progress": progress, "dlspeed": 2_000_000, "content_path": "/downloads/torrents/Some Album" });
    q.lock().unwrap().listed = Some(listed("downloading", 0.5));
    match c.poll(st, &own).await.unwrap() {
        Poll::Progress(p, m) => assert_eq!((p, m.as_str()), (50.0, "qBittorrent: 2.0 MB/s")),
        _ => panic!("expected progress"),
    }
    assert!(q.lock().unwrap().calls.iter().any(|(p, v)| p == "torrents/info" && v["hashes"] == own), "polled by hash");
    // waiting in qBittorrent's own queue (diagnosis item 15)
    q.lock().unwrap().listed = Some(listed("queuedDL", 0.0));
    match c.poll(st, &own).await.unwrap() {
        Poll::Queued(_, m) => assert_eq!(m, "qBittorrent: queued – its limit of downloads at once is reached"),
        _ => panic!("expected queued"),
    }
    // stopped: started again (5.x start, 4.x resume)
    q.lock().unwrap().listed = Some(listed("stoppedDL", 0.1));
    assert!(matches!(c.poll(st, &own).await.unwrap(), Poll::Progress(_, m) if m.contains("starting the download")));
    assert!(q.lock().unwrap().calls.iter().any(|(p, v)| p == "torrents/start" && v["hashes"] == own));
    // at 100 % but still moving out of the folder for incomplete downloads, or checking the files: not done
    // yet, and busy with them (no stall Download now may give up)
    q.lock().unwrap().listed = Some(listed("moving", 1.0));
    assert!(matches!(c.poll(st, &own).await.unwrap(), Poll::Busy(_, m) if m == "qBittorrent: moving the files"));
    q.lock().unwrap().listed = Some(listed("checkingUP", 1.0));
    assert!(matches!(c.poll(st, &own).await.unwrap(), Poll::Busy(p, _) if p == 100.0));
    q.lock().unwrap().listed = Some(listed("stalledUP", 1.0));
    match c.poll(st, &own).await.unwrap() {
        Poll::Done(p) => assert_eq!(p, PathBuf::from("/downloads/torrents/Some Album")),
        _ => panic!("expected done"),
    }
    // removed by hash, with its files
    c.remove(st, &own).await;
    assert!(q.lock().unwrap().calls.iter().any(|(p, v)| p == "torrents/delete" && v["hashes"] == own && v["deleteFiles"] == "true"));
    // the ids of older versions (a tag) are still followed by their tag
    assert!(matches!(c.poll(st, "encore-5").await.unwrap(), Poll::Done(_)));
}

/// qBittorrent restarted and forgot Encore's session: Encore logs in again and asks once more, instead of
/// failing the download (diagnosis item 13).
#[tokio::test]
async fn qbit_logs_in_again_when_it_forgot_the_session() {
    let env = setup("qbit-relogin");
    let q: QbState = Default::default();
    let url = serve(fake_qbit(q.clone(), torrent_bytes())).await;
    let st = &env.st;
    st.cfg().set_many(&json!({ "torrent_client": "qbittorrent", "qbit_url": url }));
    let c = Client::torrent(st);
    assert!(matches!(c.poll(st, "abc").await.unwrap(), Poll::Missing(_)));
    assert_eq!(q.lock().unwrap().logins, 1);
    q.lock().unwrap().sid = "SID=after-a-restart".into();
    assert!(matches!(c.poll(st, "abc").await.unwrap(), Poll::Missing(_)), "answered after a new login");
    assert_eq!(q.lock().unwrap().logins, 2);
}

/// A release qBittorrent already has: this request's earlier attempt (Retry) is taken over; another
/// request's download of it and one added outside Encore are left alone.
#[tokio::test]
async fn qbit_takes_over_its_own_earlier_torrent_only() {
    let env = setup("qbit-again");
    let q: QbState = Default::default();
    let url = serve(fake_qbit(q.clone(), torrent_bytes())).await;
    let st = &env.st;
    st.cfg().set_many(&json!({ "torrent_client": "qbittorrent", "qbit_url": url }));
    let hash = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
    {
        let mut g = q.lock().unwrap();
        g.fails = true;
        g.listed = Some(json!({ "hash": hash, "tags": "encore-5", "state": "downloading", "progress": 0.2 }));
    }
    let c = Client::torrent(st);
    let magnet = format!("magnet:?xt=urn:btih:{hash}");
    assert_eq!(c.add(st, &magnet, "x", 5).await.unwrap(), format!("qbit:{hash}"));
    // another request's download of the same release (an album and a song picked from it): left to it
    q.lock().unwrap().listed = Some(json!({ "hash": hash, "tags": "encore-50, music", "state": "downloading", "progress": 0.2 }));
    assert_eq!(c.add(st, &magnet, "x", 5).await.unwrap_err().to_string(),
               "qBittorrent is already downloading this release for another request – pick another release");
    assert!(!q.lock().unwrap().calls.iter().any(|(p, _)| p == "torrents/addTags"), "never tagged for this request");
    q.lock().unwrap().listed = Some(json!({ "hash": hash, "tags": "sonarr", "state": "downloading", "progress": 0.2 }));
    assert_eq!(c.add(st, &magnet, "x", 5).await.unwrap_err().to_string(), "qBittorrent already has this torrent (added outside Encore)");
}

// ---- SABnzbd ---------------------------------------------------------------------------------------------

/// The SABnzbd stand-in: the API key "full" may do anything, the NZB key "nzb" only add; a finished job
/// sits in the archive (SABnzbd 4.2+ set to archive finished jobs). `hostcheck`: SABnzbd's host name
/// check refuses every call.
fn fake_sab(hostcheck: bool, uploads: Log) -> Router {
    Router::new().route("/get.nzb", get(|| async { NZB })).route("/api", post(move |axum::extract::Query(p): axum::extract::Query<std::collections::HashMap<String, String>>, m: axum::extract::Multipart| {
        let uploads = uploads.clone();
        async move {
            if p.get("mode").map(String::as_str) != Some("addfile") || p.get("apikey").map(String::as_str) != Some("full") {
                return axum::Json(json!({ "status": false, "error": "API Key Incorrect" })).into_response();
            }
            let mut v = form_of(m).await;
            v["nzbname"] = json!(p.get("nzbname"));
            v["cat"] = json!(p.get("cat"));
            v["pp"] = json!(p.get("pp"));
            uploads.lock().unwrap().push(v);
            axum::Json(json!({ "status": true, "nzo_ids": ["SABnzbd_nzo_9"] })).into_response()
        }
    }).get(move |axum::extract::Query(p): axum::extract::Query<std::collections::HashMap<String, String>>| async move {
        if hostcheck {
            return (StatusCode::FORBIDDEN, "Access denied - Hostname verification failed: https://sabnzbd.org/hostname-check").into_response();
        }
        let key = p.get("apikey").map(String::as_str).unwrap_or("");
        let mode = p.get("mode").map(String::as_str).unwrap_or("");
        let v = match (mode, key) {
            ("version", _) => json!({ "version": "4.3.2" }),
            ("addurl", "full" | "nzb") => json!({ "status": true, "nzo_ids": ["SABnzbd_nzo_1"] }),
            (_, "") => json!({ "status": false, "error": "API Key Required" }),
            (_, "full") => match mode {
                "queue" if p.get("nzo_ids").map(String::as_str) == Some("SABnzbd_nzo_2") => json!({ "queue": { "paused": false, "slots": [{ "status": "Queued", "percentage": "0", "timeleft": "0:00:00" }] } }),
                "queue" if p.get("nzo_ids").map(String::as_str) == Some("SABnzbd_nzo_3") => json!({ "queue": { "paused": true, "slots": [{ "status": "Queued", "percentage": "10", "timeleft": "0:00:00" }] } }),
                "queue" => json!({ "queue": { "slots": [] } }),
                "history" if p.get("archive").map(String::as_str) == Some("1") => json!({ "history": { "slots": [{ "nzo_id": "SABnzbd_nzo_1", "status": "Completed", "storage": "/sab/complete/encore-1" }] } }),
                "history" if p.get("nzo_ids").map(String::as_str) == Some("SABnzbd_nzo_4") => json!({ "history": { "slots": [{ "nzo_id": "SABnzbd_nzo_4", "status": "Repairing", "storage": "" }] } }),
                "history" => json!({ "history": { "slots": [] } }),
                _ => json!({ "status": true }),
            },
            _ => json!({ "status": false, "error": "API Key Incorrect" }),
        };
        axum::Json(v).into_response()
    }))
}

/// Test connection with the NZB Key fails and says which key to use (it passed before, and then every
/// download failed on its first look); the host name check is named with the fix; a finished job in
/// SABnzbd's archive is found; the API key never shows in a message (diagnosis items 14, N1, 19, 20).
#[tokio::test]
async fn sab_test_needs_the_api_key_and_says_what_is_wrong() {
    let env = setup("sab");
    let uploads: Log = Default::default();
    let url = serve(fake_sab(false, uploads.clone())).await;
    let st = &env.st;
    st.cfg().set_many(&json!({ "nzb_client": "sabnzbd", "sab_url": url, "sab_api_key": "nzb", "sab_download_path": "/downloads/usenet", "sab_category": "music" }));
    let c = Client::usenet(st);
    assert_eq!(c.status(st).await.unwrap_err().to_string(), "SABnzbd: wrong API key – use the API Key from SABnzbd → Config → General, not the NZB Key");
    st.cfg().set_many(&json!({ "sab_api_key": "" }));
    assert_eq!(c.status(st).await.unwrap_err().to_string(), "SABnzbd: no API key – paste the API Key from SABnzbd → Config → General");
    st.cfg().set_many(&json!({ "sab_api_key": "full" }));
    assert_eq!(c.status(st).await.unwrap(), " 4.3.2");
    // Encore fetches the NZB and uploads it, so SABnzbd needn't reach Prowlarr's link; unpacked whatever
    // the category says (pp 3: repair, unpack, delete the archives), since Encore can't use archives
    assert_eq!(c.add(st, &format!("{url}/get.nzb?apikey=prowlarr-key"), "Album", 1).await.unwrap(), "sab:SABnzbd_nzo_9");
    let up = uploads.lock().unwrap().clone();
    assert_eq!(up, [json!({ "name": format!("<file {} bytes>", NZB.len()), "nzbname": "encore-1", "cat": "music", "pp": "3" })]);
    // finished, and moved to SABnzbd's archive
    match c.poll(st, "SABnzbd_nzo_1").await.unwrap() {
        Poll::Done(p) => assert_eq!(p, PathBuf::from("/downloads/usenet/encore-1")),
        _ => panic!("expected done"),
    }
    // behind SABnzbd's other downloads: queued, not stalled
    assert!(matches!(c.poll(st, "SABnzbd_nzo_2").await.unwrap(), Poll::Queued(_, m) if m == "SABnzbd: queued behind its other downloads"));
    // the whole queue paused (SABnzbd's own pause on a low disk): nothing moves, so the stall time runs
    assert!(matches!(c.poll(st, "SABnzbd_nzo_3").await.unwrap(), Poll::Progress(_, m) if m == "SABnzbd: its queue is paused"));
    // downloaded, and SABnzbd repairs it: busy with the files, not stalled
    assert!(matches!(c.poll(st, "SABnzbd_nzo_4").await.unwrap(), Poll::Busy(p, m) if p == 99.0 && m == "SABnzbd: Repairing"));
    // the host name check
    let url = serve(fake_sab(true, Default::default())).await;
    st.cfg().set_many(&json!({ "sab_url": url }));
    let e = c.status(st).await.unwrap_err().to_string();
    assert_eq!(e, "SABnzbd refused the host name Encore uses (127.0.0.1) – add 127.0.0.1 to SABnzbd → Config → Special → host_whitelist, or use its IP address in the SABnzbd URL");
    // not running: the message names the address without its query, where the key is
    st.cfg().set_many(&json!({ "sab_url": "http://127.0.0.1:9", "sab_api_key": "very-secret-key" }));
    let e = c.poll(st, "SABnzbd_nzo_1").await.unwrap_err().to_string();
    assert!(e.starts_with("SABnzbd unreachable: ") && e.contains("http://127.0.0.1:9/api") && !e.contains("very-secret-key"), "{e}");
}

/// Prowlarr's download links carry its API key: a link that can't be fetched is named without it.
#[tokio::test]
async fn a_release_link_that_fails_is_named_without_its_key() {
    let env = setup("leak");
    let e = fetch_torrent(&env.st, "http://127.0.0.1:9/1/download?apikey=very-secret-key&link=abc").await.err().unwrap().to_string();
    assert!(e.starts_with("couldn't fetch the torrent: ") && !e.contains("very-secret-key"), "{e}");
}
