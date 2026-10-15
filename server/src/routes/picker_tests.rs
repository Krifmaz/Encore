//! Song picks from torrent and Usenet indexers, against a mock Prowlarr (no network).
use super::*;
use axum::extract::RawQuery;
use std::sync::{Arc, Mutex};

fn state(name: &str) -> (AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("encore-picker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        crate::build_state().unwrap()
    };
    (st, dir)
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
    let karaoke = titles.iter().position(|t| t.contains("Karaoke")).unwrap();
    assert!(karaoke > 0);
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
    let _ = std::fs::remove_dir_all(&dir);
}
