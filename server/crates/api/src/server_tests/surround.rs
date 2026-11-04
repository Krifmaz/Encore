//! The Surround library on a whole server (0.35): a scanned surround song never shows in the stereo library's
//! lists, search, shuffle or plays, plays and shows through the library's own routes (by its id), its album page
//! links to the stereo album and back, and Get Dolby Atmos version says what it needs. A stray 5.1 file a stereo
//! download brings goes to the Surround library and the request looks further.
use crate::test_support::{call, send, user, TestServer};
use axum::http::StatusCode;
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::path::Path;

async fn login(app: &axum::Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

/// A short tone with tags: stereo FLAC, or 5.1 E-AC-3 in an .m4a (Apple Music's Atmos files).
fn tone(path: &Path, surround: bool, title: &str, album: &str) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut c = std::process::Command::new("ffmpeg");
    c.args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=330:duration=2", "-ac", if surround { "6" } else { "2" }]);
    if surround {
        c.args(["-c:a", "eac3", "-b:a", "384k", "-f", "mp4"]);
    }
    for (k, v) in [("title", title), ("artist", "Tone"), ("album_artist", "Tone"), ("album", album), ("track", "1")] {
        c.args(["-metadata", &format!("{k}={v}")]);
    }
    c.arg(path).status().is_ok_and(|s| s.success())
}

fn ids(v: &Value) -> Vec<String> {
    let list = v.as_array().or_else(|| v["albums"].as_array()).or_else(|| v["tracks"].as_array()).or_else(|| v["items"].as_array()).cloned().unwrap_or_default();
    list.iter().map(|x| x["id"].to_string().trim_matches('"').to_string()).collect()
}

#[tokio::test]
async fn surround_songs_stay_out_of_the_stereo_library_and_play_by_id() {
    if encore_core::which::which("ffmpeg").is_none() {
        return;
    }
    let ts = TestServer::new("surround");
    let lib = ts.dir.join("music");
    let ts = ts.with(json!({ "library_path": lib.to_string_lossy(), "surround_path": "" }));
    let st = ts.st.clone();
    user(&st, "boss", "admin");
    user(&st, "member", "user");
    assert!(tone(&lib.join("Tone/Waves/01 Sine.flac"), false, "Sine", "Waves"));
    assert!(tone(&lib.join("Surround/Tone/Waves (Dolby Atmos)/01 Sine.m4a"), true, "Sine", "Waves (Dolby Atmos)"));
    st.library.scan_library(st.db.clone(), &st.cfg, false).await;
    let stereo = st.db.one("SELECT id, album_id FROM tracks", p![]).unwrap().unwrap();
    let sur = st.db.one("SELECT id, album_id FROM surround_tracks", p![]).unwrap().unwrap();
    let (sid, salbum) = (sur.i("id"), sur.s("album_id"));
    assert!(encore_library::surround::is_track(sid));
    let app = ts.app();
    let boss = login(&app, "boss").await;
    let member = login(&app, "member").await;

    // the stereo lists, search and shuffle know one song and one album
    for uri in ["/api/library/albums", "/api/library/tracks", "/api/library/recent", "/api/library/search?q=Sine", "/api/library/shuffle?limit=50"] {
        let (code, v, _) = call(&app, "GET", uri, &member, None).await;
        assert_eq!(code, StatusCode::OK, "{uri}");
        let text = v.to_string();
        assert!(!text.contains(&sid.to_string()) && !text.contains(&salbum), "{uri} shows the surround song: {text}");
    }
    // the Surround list has it, for everyone
    let (code, v, _) = call(&app, "GET", "/api/surround/albums", &member, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(ids(&v), [salbum.as_str()]);
    assert_eq!(v["albums"][0]["quality"], "E-AC-3 5.1");
    assert_eq!(v["stats"]["tracks"], 1);

    // the stereo album page names its surround version, each song too
    let (_, a, _) = call(&app, "GET", &format!("/api/library/albums/{}", stereo.s("album_id")), &member, None).await;
    assert_eq!(a["surround"]["have"][0]["album_id"], salbum.as_str());
    assert_eq!(a["tracks"][0]["surround_id"], sid);
    // and the surround album page the stereo one
    let (code, s, _) = call(&app, "GET", &format!("/api/library/albums/{salbum}"), &member, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!((s["class"].as_str(), s["stereo"]["album_id"].as_str()), (Some("surround"), Some(stereo.s("album_id").as_str())));
    assert_eq!(s["tracks"][0]["codec"], "eac3");
    assert!(s["tracks"][0]["path"].is_null(), "members never see paths");

    // it plays by its id: a browser gets FLAC (no browser decodes E-AC-3), a client with an E-AC-3 decoder the file
    let r = send(&app, "GET", &format!("/api/library/tracks/{sid}/stream"), &member, None, &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.h("x-encore-stream"), "flac");
    let r = send(&app, "GET", &format!("/api/library/tracks/{sid}/stream"), &member, None, &[("x-encore-decoders", "eac3,aac")]).await;
    assert_eq!((r.status, r.h("x-encore-stream").as_str()), (StatusCode::OK, "original"));
    let (code, info, _) = call(&app, "GET", &format!("/api/library/tracks/{sid}/info"), &member, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!((info["codec"].as_str(), info["channels"].as_u64(), info["class"].as_str()), (Some("eac3"), Some(6), Some("surround")));
    let (code, t, _) = call(&app, "GET", &format!("/api/library/tracks/{sid}"), &member, None).await;
    assert_eq!((code, t["class"].as_str()), (StatusCode::OK, Some("surround")));

    // played, but not counted: plays and stats are the stereo library's
    let (code, v, _) = call(&app, "POST", "/api/library/plays", &member, Some(json!({ "track_id": sid, "seconds": 120 }))).await;
    assert_eq!((code, v["saved"].as_i64()), (StatusCode::OK, Some(0)));
    assert_eq!(st.db.one("SELECT COUNT(*) n FROM plays", p![]).unwrap().unwrap().i("n"), 0);

    // Get Dolby Atmos version: admins only; without gamdl and a wrapper it says what it needs
    let uri = format!("/api/library/albums/{}/surround", stereo.s("album_id"));
    let (code, v, _) = call(&app, "GET", &uri, &member, None).await;
    assert_eq!((code, v["can_fetch"].as_bool()), (StatusCode::OK, Some(false)));
    let (code, _, _) = call(&app, "POST", &uri, &member, None).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (code, v, _) = call(&app, "POST", &uri, &boss, None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(v["detail"].as_str().unwrap_or("").contains("wrapper-v2"), "{v}");
    // the library sweep: admins only, and the same need
    let (code, _, _) = call(&app, "GET", "/api/surround/sweep", &member, None).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (code, v, _) = call(&app, "GET", "/api/surround/sweep", &boss, None).await;
    assert_eq!((code, v["running"].as_bool(), v["can_fetch"].as_bool()), (StatusCode::OK, Some(false), Some(false)));
    let (code, v, _) = call(&app, "POST", "/api/surround/sweep", &boss, None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(v["detail"].as_str().unwrap_or("").contains("wrapper-v2"), "{v}");
    let (code, v, _) = call(&app, "DELETE", "/api/surround/sweep", &boss, None).await;
    assert_eq!((code, v["stopping"].as_bool()), (StatusCode::OK, Some(false)));

    // an import: of a download's files, the surround one is filed in the Surround folder (its name from its
    // tags) and linked to the stereo album the request names; the stereo one stays for the stereo import
    let staging = ts.dir.join("staging/request-9");
    assert!(tone(&staging.join("x.m4a"), true, "Arpeggi", "Waves (Dolby Atmos)"));
    assert!(tone(&staging.join("y.flac"), false, "Arpeggi", "Waves"));
    let want = encore_library::surround::Import { artist: "Tone".into(), album: "Waves".into(), stereo_album_id: Some(stereo.s("album_id")), stereo_track_ids: vec![] };
    let (new, others) = encore_library::surround::import(&st, &[staging.join("x.m4a"), staging.join("y.flac")], &want).await.unwrap();
    assert_eq!(others, 1);
    assert_eq!(new, [lib.join("Surround/Tone/Waves (Dolby Atmos)/01 - Arpeggi.m4a")]);
    assert!(!staging.join("x.m4a").exists() && staging.join("y.flac").exists());
    let row = st.db.one("SELECT * FROM surround_tracks WHERE title='Arpeggi'", p![]).unwrap().unwrap();
    assert_eq!((row.s("stereo_album_id"), row.i("channels")), (stereo.s("album_id"), 6));
    ts.remove();
}
