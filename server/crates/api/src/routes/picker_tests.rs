//! The picker against mock services (no network): song picks from torrent and Usenet indexers (a mock
//! Prowlarr), the source state table, and the Tidal and Qobuz catalogue searches with their quality labels.
use super::*;
use axum::extract::{Path, Query, RawQuery};
use axum::http::StatusCode;
use axum::http::HeaderMap;
use std::sync::{Arc, Mutex};

fn state(name: &str) -> (AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("encore-picker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    (st, dir)
}

/// Point the server's service addresses at stand-ins (`f` changes the real hosts).
fn set_urls(st: &mut AppState, f: impl FnOnce(&mut encore_http::Urls)) {
    let mut u = (*st.urls).clone();
    f(&mut u);
    st.urls = Arc::new(u);
}

fn rel(guid: &str, title: &str, mb: f64, seeds: i64) -> Value {
    json!({ "guid": guid, "title": title, "size": mb * 1e6, "seeders": seeds, "indexer": "Tracker", "protocol": "torrent", "magnetUrl": format!("magnet:?xt=urn:btih:{guid}") })
}

#[tokio::test]
async fn song_search_finds_singles_and_the_album() {
    let (st, dir) = state("song");
    let queries: Arc<Mutex<Vec<String>>> = Default::default();
    let q2 = queries.clone();
    let app = Router::new().route("/api/v1/search", get(move |RawQuery(q): RawQuery| {
        let queries = q2.clone();
        async move {
            let q = q.unwrap_or_default();
            queries.lock().unwrap().push(q.clone());
            let list = if q.contains("Rodeo") {
                vec![rel("a1", "Travis Scott - Rodeo (2015) [FLAC]", 700.0, 40), rel("a2", "Travis Scott - Rodeo (Deluxe) [MP3 320]", 180.0, 5), rel("x", "Other Band - Rodeo [FLAC]", 300.0, 9)]
            } else {
                vec![rel("s1", "Travis Scott - 90210 [FLAC] WEB", 45.0, 12), rel("s2", "Travis Scott - 90210 (Karaoke) [MP3]", 8.0, 30), rel("s3", "Travis Scott - Antidote [FLAC]", 40.0, 50),
                     rel("a1", "Travis Scott - Rodeo (2015) [FLAC]", 700.0, 40)]
            };
            axum::Json(json!(list))
        }
    }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    st.cfg.set_many(&json!({ "torznab_enabled": true, "prowlarr_url": url, "prowlarr_api_key": "k", "qbit_url": "http://qbit.invalid" }));
    let b = SearchBody { source: "torznab".into(), kind: "track".into(), artist: "Travis Scott".into(), title: "90210".into(), album: Some("Rodeo".into()), ..Default::default() };
    let out = search_torznab(&st, &b, "Travis Scott 90210").await.unwrap();
    let titles: Vec<&str> = out.iter().map(|x| x["title"].as_str().unwrap()).collect();
    assert_eq!(titles[0], "Travis Scott - 90210 [FLAC] WEB", "{titles:?}");
    assert!(!titles.contains(&"Travis Scott - Antidote [FLAC]") && !titles.contains(&"Other Band - Rodeo [FLAC]"), "{titles:?}");
    // another version than the song asked for (release types: song versions) is never offered
    assert!(!titles.iter().any(|t| t.contains("Karaoke")), "{titles:?}");
    // the album's releases are offered once each, marked, and carry what the worker needs
    let albums: Vec<&Value> = out.iter().filter(|x| x["payload"]["song_only"] == true).collect();
    assert_eq!(albums.len(), 2, "{out:?}");
    assert_eq!(albums.iter().filter(|x| x["title"] == "Travis Scott - Rodeo (2015) [FLAC]").count(), 1);
    assert_eq!(albums[0]["subtitle"], "from the album Rodeo · only 90210 downloads");
    assert_eq!(albums[0]["payload"]["album"], "Rodeo");
    assert!(out.iter().filter(|x| x["title"] == "Travis Scott - Rodeo (2015) [FLAC]").count() == 1);
    // the song search asks for the audio, MP3 and lossless categories
    let qs = queries.lock().unwrap().clone();
    let song_q = qs.iter().find(|q| !q.contains("Rodeo")).unwrap();
    for c in ["categories=3000", "categories=3010", "categories=3040"] {
        assert!(song_q.contains(c), "{song_q}");
    }
    // albums still search as before; no more "whole albums only"
    let ab = SearchBody { source: "torznab".into(), kind: "album".into(), artist: "Travis Scott".into(), title: "Rodeo".into(), ..Default::default() };
    let out = search_torznab(&st, &ab, "Travis Scott Rodeo").await.unwrap();
    assert!(out.iter().all(|x| x["payload"]["song_only"].is_null()) && !out.is_empty());
    // Ke$ha reaches the indexers as "Kesha", the way they spell her
    let kb = SearchBody { source: "torznab".into(), kind: "album".into(), artist: "Ke$ha".into(), title: "Cannibal".into(), ..Default::default() };
    search_torznab(&st, &kb, "Ke$ha Cannibal").await.unwrap();
    let last = queries.lock().unwrap().last().cloned().unwrap();
    assert!(last.contains("query=Kesha+Cannibal"), "{last}");
    assert_eq!(contains("Kesha - Cannibal (2010) [FLAC]", "Ke$ha"), 1.0);
    assert_eq!(contains("Ke$ha - Cannibal [FLAC]", "Kesha"), 1.0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Serve `app` on a free local port; the base URL.
async fn serve(app: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    url
}

/// A fake streamrip binary: any file the `streamrip_bin` setting can point at.
fn fake_streamrip(dir: &std::path::Path) -> String {
    let p = dir.join("rip");
    std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
    p.to_string_lossy().into_owned()
}

fn write_config(st: &AppState, text: &str) {
    let p = streamrip::config_path(st);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn state_of(st: &AppState, id: &str) -> (String, String, String) {
    let s = available(st).into_iter().find(|s| s["id"] == id).unwrap_or_else(|| panic!("{id} listed"));
    (s["state"].as_str().unwrap().to_string(), s["hint"].as_str().unwrap().to_string(), s["settings"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn every_source_is_listed_with_its_state() {
    let (st, dir) = state("states");
    let ids: Vec<String> = available(&st).iter().map(|s| s["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["slskd", "torznab", "qobuz", "tidal", "deezer", "ytmusic"]);
    // nothing set up (yt-dlp is on by default): every source is still listed, off, with a hint and a settings link
    st.cfg.set_many(&json!({ "ytdlp_enabled": false }));
    for s in available(&st) {
        assert_eq!(s["state"], "off", "{s}");
        assert!(!s["hint"].as_str().unwrap().is_empty() && !s["name"].as_str().unwrap().is_empty(), "{s}");
        assert!(s["settings"].as_str().unwrap().starts_with("#/settings?s="), "{s}");
        assert!(s["settings_label"].as_str().unwrap().starts_with("Settings → "), "{s}");
    }
    // streamrip on and installed: the services want their accounts (Qobuz first has to be in the pipeline)
    st.cfg.set_many(&json!({ "streamrip_enabled": true, "streamrip_bin": fake_streamrip(&dir), "source_order": "qobuz,tidal,deezer,slskd,torznab,ytdlp" }));
    assert_eq!(state_of(&st, "deezer"), ("sign_in".into(), "Deezer needs an ARL cookie".into(), "#/settings?s=streamrip".into()));
    assert_eq!(state_of(&st, "tidal").0, "sign_in");
    assert_eq!(state_of(&st, "tidal").1, "Sign in to Tidal");
    assert_eq!(state_of(&st, "qobuz").0, "sign_in");
    // a search on a source that is not ready: 409 with the state and the link, never a failed search
    let b = SearchBody { source: "deezer".into(), kind: "track".into(), artist: "Travis Scott".into(), title: "90210".into(), ..Default::default() };
    let e = run_search(&st, &b).await.expect_err("409");
    assert_eq!(e.status.as_u16(), 409);
    assert_eq!(e.detail, "Deezer needs an ARL cookie");
    let x = e.extra.unwrap();
    assert_eq!((x["state"].as_str(), x["settings"].as_str(), x["settings_label"].as_str()), (Some("sign_in"), Some("#/settings?s=streamrip"), Some("Settings → streamrip")));
    // the accounts: the ARL in the settings, a Tidal token and a Qobuz login in config.toml
    st.cfg.set_many(&json!({ "streamrip_deezer_arl": "arl-cookie" }));
    assert_eq!(state_of(&st, "deezer").0, "ready");
    write_config(&st, "[qobuz]\nuser_id = \"u1\"\nauth_token = \"tok-q\"\n\n[tidal]\naccess_token = \"user-token-abcdef\"\nhires_client = false\n");
    assert_eq!(state_of(&st, "tidal").0, "ready");
    assert_eq!(state_of(&st, "qobuz").0, "ready");
    // a service left out of the pipeline is off, pointing at the pipeline setting
    st.cfg.set_many(&json!({ "source_order": "tidal,deezer,slskd" }));
    assert_eq!(state_of(&st, "qobuz"), ("off".into(), "Qobuz is not in the download pipeline".into(), "#/settings?s=pipeline".into()));
    // streamrip switched on but not installed
    st.cfg.set_many(&json!({ "streamrip_bin": dir.join("missing").to_string_lossy() }));
    assert_eq!(state_of(&st, "tidal").0, "not_installed");
    // Soulseek: off → a login missing → the engine not installed (the status is "off" in tests: never started)
    st.cfg.set_many(&json!({ "slskd_enabled": true }));
    assert_eq!(state_of(&st, "slskd"), ("sign_in".into(), "Soulseek needs a username and password".into(), "#/settings?s=slskd".into()));
    st.cfg.set_many(&json!({ "soulseek_username": "alice", "soulseek_password": "pw" }));
    assert_eq!(state_of(&st, "slskd").0, "not_installed");
    // Prowlarr: off → address missing → no download client → ready
    st.cfg.set_many(&json!({ "torznab_enabled": true }));
    assert_eq!(state_of(&st, "torznab").0, "sign_in");
    st.cfg.set_many(&json!({ "prowlarr_url": "http://prowlarr.invalid", "prowlarr_api_key": "k" }));
    assert_eq!(state_of(&st, "torznab"), ("not_connected".into(), "No torrent or Usenet client is set up".into(), "#/settings?s=torznab".into()));
    st.cfg.set_many(&json!({ "qbit_url": "http://qbit.invalid" }));
    assert_eq!(state_of(&st, "torznab").0, "ready");
    // yt-dlp
    st.cfg.set_many(&json!({ "ytdlp_enabled": true }));
    assert_eq!(state_of(&st, "ytmusic"), ("ready".into(), String::new(), "#/settings?s=ytdlp".into()));
    // Soulseek's 409 (the engine runs without a server session) reads as not connected, other errors as before
    assert_eq!(slskd::http_error(409, "The server connection must be connected and logged in").to_string(), slskd::NOT_CONNECTED);
    assert_eq!(slskd::http_error(500, "boom").to_string(), "Soulseek engine returned 500 Internal Server Error: boom");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for Tidal's v2 API: a search naming three albums and one track, and the listings with the
/// artist (and album) included. Records every request's path, query and Authorization header.
fn mock_tidal(seen: Arc<Mutex<Vec<(String, String)>>>) -> Router {
    let note = move |path: String, q: Option<String>, h: &HeaderMap| {
        seen.lock().unwrap().push((format!("{path}?{}", q.unwrap_or_default()), h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string()));
    };
    let (n1, n2, n3) = (note.clone(), note.clone(), note);
    let img = json!([{ "href": "https://img/80", "meta": { "width": 80, "height": 80 } }, { "href": "https://img/320", "meta": { "width": 320, "height": 320 } }]);
    let album = |id: &str, title: &str, tags: Value, img: &Value| json!({ "id": id, "type": "albums", "attributes": { "title": title, "mediaTags": tags, "numberOfItems": 16,
        "duration": "PT1H6M", "releaseDate": "2015-09-04", "type": "ALBUM", "explicit": true, "imageLinks": img }, "relationships": { "artists": { "data": [{ "id": "ar1", "type": "artists" }] } } });
    let albums = json!({ "data": [album("a1", "Rodeo", json!(["HIRES_LOSSLESS", "LOSSLESS"]), &img), album("a2", "Rodeo (Deluxe)", json!(["MQA", "LOSSLESS"]), &img), album("a3", "Rodeo", json!(["LOSSLESS"]), &img)],
        "included": [{ "id": "ar1", "type": "artists", "attributes": { "name": "Travis Scott" } }] });
    let tracks = json!({ "data": [{ "id": "t1", "type": "tracks", "attributes": { "title": "90210", "version": "", "isrc": "USQ4E1500401", "duration": "PT5M39S", "mediaTags": ["HIRES_LOSSLESS"], "explicit": false },
        "relationships": { "artists": { "data": [{ "id": "ar1", "type": "artists" }] }, "albums": { "data": [{ "id": "a1", "type": "albums" }] } } }],
        "included": [{ "id": "ar1", "type": "artists", "attributes": { "name": "Travis Scott" } }, { "id": "a1", "type": "albums", "attributes": { "title": "Rodeo", "imageLinks": img } }] });
    Router::new()
        // the search as Tidal answers it since mid-2026: the query in filter[query], `data` an array
        .route("/searchResults", get(move |Query(q): Query<HashMap<String, String>>, RawQuery(rq): RawQuery, h: HeaderMap| {
            let n1 = n1.clone();
            async move {
                n1("/searchResults".into(), rq, &h);
                let Some(query) = q.get("filter[query]").filter(|v| !v.is_empty()) else {
                    return (StatusCode::BAD_REQUEST, Json(json!({ "errors": [{ "code": "VALUE_REQUIRED", "detail": "filter[query] is required" }] })));
                };
                (StatusCode::OK, Json(json!({ "data": [{ "id": query, "type": "searchResults", "relationships": {
                    "albums": { "data": [{ "id": "a1", "type": "albums" }, { "id": "a2", "type": "albums" }, { "id": "a3", "type": "albums" }] },
                    "tracks": { "data": [{ "id": "t1", "type": "tracks" }] } } }] })))
            }
        }))
        // the retired path form answers every query the way the owner's screenshot shows
        .route("/searchResults/{q}", get(|Path(_q): Path<String>| async move {
            (StatusCode::BAD_REQUEST, Json(json!({ "errors": [{ "code": "INVALID_RESOURCE_ID", "detail": "Invalid resource ID", "source": { "pointer": "data/id" } }] })))
        }))
        .route("/albums", get(move |RawQuery(rq): RawQuery, h: HeaderMap| { let (n2, albums) = (n2.clone(), albums.clone()); async move { n2("/albums".into(), rq, &h); Json(albums) } }))
        .route("/tracks", get(move |RawQuery(rq): RawQuery, h: HeaderMap| { let (n3, tracks) = (n3.clone(), tracks.clone()); async move { n3("/tracks".into(), rq, &h); Json(tracks) } }))
}

#[tokio::test]
async fn tidal_search_reads_the_quality_tier_and_hands_streamrip_links() {
    let (mut st, dir) = state("tidal");
    let seen: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let url = serve(mock_tidal(seen.clone())).await;
    set_urls(&mut st, |u| u.tidal = url.clone());
    // a Tidal Client ID is set: the developer app's token (here already cached) does the searching
    st.cfg.set_many(&json!({ "tidal_client_id": "cid", "tidal_client_secret": "sec" }));
    st.cache_put("tidal_token", json!("app-token"), 600);
    let b = SearchBody { source: "tidal".into(), kind: "album".into(), artist: "Travis Scott".into(), title: "Rodeo".into(), ..Default::default() };
    let out = search_tidal(&st, &b, "Travis Scott Rodeo").await.unwrap();
    let row = |key: &str| out.iter().find(|x| x["key"] == key).cloned().unwrap_or_else(|| panic!("{key} in {out:?}"));
    assert_eq!(out.len(), 3);
    assert_eq!(row("tidal:album:a1")["quality"], "FLAC 24-bit");
    assert_eq!(row("tidal:album:a2")["quality"], "MQA");
    assert_eq!(row("tidal:album:a3")["quality"], "FLAC 16/44.1");
    assert!(out.iter().all(|x| x["lossless"] == true && x["source"] == "tidal"));
    let a1 = row("tidal:album:a1");
    assert_eq!(a1["payload"]["url"], "https://tidal.com/browse/album/a1");
    assert_eq!((a1["subtitle"].as_str(), a1["tracks"].as_u64(), a1["info"].as_str(), a1["cover"].as_str()), (Some("Travis Scott"), Some(16), Some("Album · 2015 · explicit"), Some("https://img/320")));
    assert_eq!(a1["duration"].as_f64(), Some(3960.0));
    assert_eq!(row("tidal:album:a2")["title"], "Rodeo (Deluxe)");
    // the exact match ranks above the deluxe edition
    assert!(out[0]["key"] != "tidal:album:a2", "{out:?}");
    {
        let calls = seen.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls.iter().all(|(_, auth)| auth == "Bearer app-token"), "{calls:?}");
        // the query in filter[query] (form-encoded), not in the path
        assert!(calls[0].0.starts_with("/searchResults?") && calls[0].0.contains("filter%5Bquery%5D=Travis+Scott+Rodeo") && calls[0].0.contains("countryCode=US") && calls[0].0.contains("include=albums"), "{calls:?}");
        assert!(calls[1].0.starts_with("/albums?") && calls[1].0.contains("filter%5Bid%5D=a1%2Ca2%2Ca3") && calls[1].0.contains("include=artists"), "{calls:?}");
    }
    // a song: its album and artist from the included resources, the ISO duration in seconds
    let t = SearchBody { source: "tidal".into(), kind: "track".into(), artist: "Travis Scott".into(), title: "90210".into(), length: Some(339.0), ..Default::default() };
    let out = search_tidal(&st, &t, "Travis Scott 90210").await.unwrap();
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!((out[0]["title"].as_str(), out[0]["subtitle"].as_str(), out[0]["quality"].as_str(), out[0]["duration"].as_f64()), (Some("90210"), Some("Travis Scott · Rodeo"), Some("FLAC 24-bit"), Some(339.0)));
    assert_eq!(out[0]["payload"]["url"], "https://tidal.com/browse/track/t1");
    assert_eq!(out[0]["cover"], "https://img/320");
    assert!(out[0]["score"].as_f64().unwrap() > 0.0);
    // without a Client ID the account streamrip signed in with searches, in its own country
    seen.lock().unwrap().clear();
    st.cfg.set_many(&json!({ "tidal_client_id": "", "tidal_client_secret": "" }));
    write_config(&st, "[tidal]\naccess_token = \"user-token-abcdef\"\ncountry_code = \"NL\"\nhires_client = false\n");
    let out = search_tidal(&st, &b, "Travis Scott Rodeo").await.unwrap();
    assert_eq!(out.len(), 3);
    let calls = seen.lock().unwrap().clone();
    assert!(calls.iter().all(|(_, auth)| auth == "Bearer user-token-abcdef") && calls[0].0.contains("countryCode=NL"), "{calls:?}");
    // characters that need escaping reach Tidal unchanged, and an empty query asks nothing
    seen.lock().unwrap().clear();
    let odd = "AC/DC #1 Ke$ha 100% Sigur Rós? a&b";
    assert_eq!(search_tidal(&st, &b, odd).await.unwrap().len(), 3);
    {
        let calls = seen.lock().unwrap().clone();
        let u = reqwest::Url::parse(&format!("http://tidal{}", calls[0].0)).unwrap();
        let q: HashMap<String, String> = u.query_pairs().into_owned().collect();
        assert_eq!(q["filter[query]"], odd, "{calls:?}");
        assert_eq!(q["include"], "albums");
    }
    seen.lock().unwrap().clear();
    assert!(search_tidal(&st, &b, "   ").await.unwrap().is_empty());
    assert!(seen.lock().unwrap().is_empty(), "no call for an empty query");
    // the old path form is the owner's 400 INVALID_RESOURCE_ID: Encore no longer asks it
    let old = encore_http::tidal::get(&st, "user-token-abcdef", "/searchResults/Duumu%20Illuminate%20EP").send().await.unwrap();
    assert_eq!(old.status().as_u16(), 400);
    // signed out of both: the search says so
    write_config(&st, "[tidal]\naccess_token = \"\"\n");
    assert_eq!(search_tidal(&st, &b, "Travis Scott Rodeo").await.err().unwrap().to_string(), "Sign in to Tidal");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for Qobuz's API: album and track searches, and the email + password login. Records the
/// X-App-Id and X-User-Auth-Token headers and the login query.
fn mock_qobuz(seen: Arc<Mutex<Vec<(String, String, String)>>>) -> Router {
    let note = move |path: &str, q: Option<String>, h: &HeaderMap| {
        let hv = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        seen.lock().unwrap().push((format!("{path}?{}", q.unwrap_or_default()), hv("x-app-id"), hv("x-user-auth-token")));
    };
    let (n1, n2, n3) = (note.clone(), note.clone(), note);
    let albums = json!({ "albums": { "limit": 25, "offset": 0, "total": 2, "items": [
        { "id": "0886443927087", "title": "Rodeo", "version": "", "artist": { "name": "Travis Scott" }, "maximum_bit_depth": 24, "maximum_sampling_rate": 192.0, "hires": true,
          "tracks_count": 16, "duration": 3980, "image": { "small": "https://q/small.jpg" }, "release_date_original": "2015-09-04", "parental_warning": true, "streamable": true },
        { "id": "x2", "title": "Rodeo", "version": "Deluxe", "artist": { "name": "Travis Scott" }, "maximum_bit_depth": 16, "maximum_sampling_rate": 44.1, "hires": false,
          "tracks_count": 18, "duration": 4400, "image": { "small": "https://q/x2.jpg" }, "release_date_original": "2015-09-04", "parental_warning": false, "streamable": false } ] } });
    let tracks = json!({ "tracks": { "limit": 25, "offset": 0, "total": 1, "items": [
        { "id": 12345, "title": "90210", "version": "", "performer": { "name": "Travis Scott" }, "album": { "title": "Rodeo", "image": { "small": "https://q/small.jpg" } },
          "duration": 339, "isrc": "USQ4E1500401", "maximum_bit_depth": 24, "maximum_sampling_rate": 96.0, "streamable": true } ] } });
    Router::new()
        .route("/album/search", get(move |RawQuery(rq): RawQuery, h: HeaderMap| { let (n, a) = (n1.clone(), albums.clone()); async move { n("/album/search", rq, &h); Json(a) } }))
        .route("/track/search", get(move |RawQuery(rq): RawQuery, h: HeaderMap| { let (n, t) = (n2.clone(), tracks.clone()); async move { n("/track/search", rq, &h); Json(t) } }))
        .route("/user/login", get(move |RawQuery(rq): RawQuery, h: HeaderMap| { let n = n3.clone(); async move { n("/user/login", rq, &h); Json(json!({ "user_auth_token": "tok-login", "user": { "id": 7 } })) } }))
}

#[tokio::test]
async fn qobuz_search_reads_bit_depth_and_rate_with_streamrips_app_id() {
    let (mut st, dir) = state("qobuz");
    let seen: Arc<Mutex<Vec<(String, String, String)>>> = Default::default();
    let url = serve(mock_qobuz(seen.clone())).await;
    set_urls(&mut st, |u| u.qobuz_api = url.clone());
    // the app id streamrip stored after its first run, and the user id + token login (streamrip's naming)
    write_config(&st, "[qobuz]\nquality = 4\nuser_id = \"u1\"\nauth_token = \"tok-q\"\napp_id = \"123456789\"\n");
    let b = SearchBody { source: "qobuz".into(), kind: "album".into(), artist: "Travis Scott".into(), title: "Rodeo".into(), ..Default::default() };
    let out = search_qobuz(&st, &b, "Travis Scott Rodeo").await.unwrap();
    assert_eq!(out.len(), 2, "{out:?}");
    let row = |key: &str| out.iter().find(|x| x["key"] == key).cloned().unwrap_or_else(|| panic!("{key} in {out:?}"));
    let a = row("qobuz:album:0886443927087");
    assert_eq!((a["quality"].as_str(), a["lossless"].as_bool(), a["subtitle"].as_str(), a["tracks"].as_u64()), (Some("FLAC 24/192"), Some(true), Some("Travis Scott"), Some(16)));
    assert_eq!((a["info"].as_str(), a["cover"].as_str(), a["warning"].as_str()), (Some("2015 · explicit"), Some("https://q/small.jpg"), Some("")));
    assert_eq!(a["payload"]["url"], "https://open.qobuz.com/album/0886443927087");
    let x2 = row("qobuz:album:x2");
    assert_eq!((x2["quality"].as_str(), x2["title"].as_str(), x2["warning"].as_str()), (Some("FLAC 16/44.1"), Some("Rodeo (Deluxe)"), Some("Not streamable on Qobuz with this account")));
    assert_eq!(out[0]["key"], "qobuz:album:0886443927087", "the exact match first: {out:?}");
    // a song: performer and album, the id as a number
    let t = SearchBody { source: "qobuz".into(), kind: "track".into(), artist: "Travis Scott".into(), title: "90210".into(), length: Some(339.0), ..Default::default() };
    let out = search_qobuz(&st, &t, "Travis Scott 90210").await.unwrap();
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!((out[0]["quality"].as_str(), out[0]["subtitle"].as_str(), out[0]["duration"].as_f64(), out[0]["isrc"].as_str()), (Some("FLAC 24/96"), Some("Travis Scott · Rodeo"), Some(339.0), Some("USQ4E1500401")));
    assert_eq!(out[0]["payload"]["url"], "https://open.qobuz.com/track/12345");
    {
        let calls = seen.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls.iter().all(|(_, app, tok)| app == "123456789" && tok == "tok-q"), "{calls:?}");
        assert!(calls[0].0.starts_with("/album/search?") && calls[0].0.contains("query=Travis+Scott+Rodeo") && calls[0].0.contains("limit=25"), "{calls:?}");
        assert!(calls[1].0.starts_with("/track/search?"), "{calls:?}");
    }
    // streamrip 2.x naming with an email + md5 password: Qobuz's user/login turns it into a token, once
    seen.lock().unwrap().clear();
    write_config(&st, "[qobuz]\nuse_auth_token = false\nemail_or_userid = \"me@example.org\"\npassword_or_token = \"5f4dcc3b5aa765d61d8327deb882cf99\"\napp_id = \"123456789\"\n");
    assert_eq!(search_qobuz(&st, &b, "Travis Scott Rodeo").await.unwrap().len(), 2);
    assert_eq!(search_qobuz(&st, &t, "Travis Scott 90210").await.unwrap().len(), 1);
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(calls[0].0.starts_with("/user/login?") && calls[0].0.contains("email=me%40example.org") && calls[0].0.contains("password=5f4dcc3b5aa765d61d8327deb882cf99") && calls[0].0.contains("app_id=123456789"), "{calls:?}");
    assert!(calls[1..].iter().all(|(_, app, tok)| app == "123456789" && tok == "tok-login"), "{calls:?}");
    // Qobuz unreachable during the login: the error names no e-mail or password hash (reqwest's own message
    // would carry the whole login URL, and the picker shows the text to everyone allowed to pick)
    let closed = { let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); format!("http://{}", l.local_addr().unwrap()) };
    set_urls(&mut st, |u| u.qobuz_api = closed.clone());
    st.cache_put("qobuz_user_token", Value::Null, 0); // the token from the login above is kept for twelve hours: expire it
    let msg = search_qobuz(&st, &b, "Travis Scott Rodeo").await.unwrap_err().to_string();
    assert!(msg.starts_with("Qobuz login: ") && !msg.contains("password=") && !msg.contains("5f4dcc3b5aa765d61d8327deb882cf99") && !msg.contains("example.org") && !msg.contains(&closed), "{msg}");
    set_urls(&mut st, |u| u.qobuz_api = url.clone());
    // the login in Encore's settings wins over the file
    st.cfg.set_many(&json!({ "streamrip_qobuz_user": "u9", "streamrip_qobuz_token": "tok-settings" }));
    assert_eq!(streamrip::qobuz_login(&st), Some(("u9".into(), "tok-settings".into(), true)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn qobuz_app_id_is_read_from_the_web_player_when_streamrip_has_not_stored_one() {
    let (mut st, dir) = state("qobuz-app-id");
    let hits = Arc::new(Mutex::new(0usize));
    let (h1, h2) = (hits.clone(), hits.clone());
    let app = Router::new()
        .route("/login", get(move || { *h1.lock().unwrap() += 1; async { axum::response::Html(r#"<html><head><script src="/resources/7.1.3-b011/bundle.js"></script></head></html>"#) } }))
        .route("/resources/7.1.3-b011/bundle.js", get(move || { *h2.lock().unwrap() += 1; async { r#"var x=1;production:{api:{appId:"950096963",appSecret:"abcdefabcdefabcdefabcdefabcdefab"},a.initialSeed("Zm9v",window.utimezone.berlin)"# } }));
    let url = serve(app).await;
    set_urls(&mut st, |u| u.qobuz_player = url.clone());
    assert_eq!(streamrip::qobuz_app_id(&st).await.unwrap(), "950096963");
    assert_eq!(streamrip::qobuz_app_id(&st).await.unwrap(), "950096963");
    assert_eq!(*hits.lock().unwrap(), 2, "read once, then kept");
    // the id streamrip stored in config.toml wins
    write_config(&st, "[qobuz]\napp_id = \"111111111\"\n");
    assert_eq!(streamrip::qobuz_app_id(&st).await.unwrap(), "111111111");
    let _ = std::fs::remove_dir_all(&dir);
}
