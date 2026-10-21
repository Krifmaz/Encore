//! Album types and who added the songs on a whole server (0.36, docs/contracts/library-types.md §9.2): the
//! "Sort albums by type" task against a stand-in MusicBrainz (the likely One-offs first, at background priority,
//! "not found" stamped, a failure not, at most the limit), a request's import stamping who asked for it (a
//! request, a link, a Soulseek-page download; one without a user keeps what the song inherited), and the
//! release's facts kept at import.
use crate::test_support::{user, TestServer};
use encore_db::{now, p, RowExt};
use encore_http::Ctx as _;
use encore_library::album_types;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// A stand-in MusicBrainz: release group `rg-live` is a live album, `rg-gone` isn't known (404), `rg-down`
/// fails (500); recording `rec-1` is on "Best Of Everything" (a compilation of 30) and "Now 99" (a compilation of
/// 40). → (its address, what it was asked)
async fn musicbrainz() -> (String, Arc<Mutex<Vec<String>>>) {
    use axum::extract::{Path as P, State};
    use axum::http::StatusCode;
    use axum::routing::get;
    let hits: Arc<Mutex<Vec<String>>> = Arc::default();
    let app = axum::Router::new()
        .route("/ws/2/release-group/{id}", get(|State(h): State<Arc<Mutex<Vec<String>>>>, P(id): P<String>| async move {
            h.lock().unwrap().push(format!("release-group/{id}"));
            match id.as_str() {
                "rg-live" => Ok(axum::Json(json!({ "id": id, "title": "Live", "primary-type": "Album", "secondary-types": ["Live"] }))),
                "rg-gone" => Err(StatusCode::NOT_FOUND),
                _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }))
        .route("/ws/2/recording/{id}", get(|State(h): State<Arc<Mutex<Vec<String>>>>, P(id): P<String>| async move {
            h.lock().unwrap().push(format!("recording/{id}"));
            axum::Json(json!({ "id": id, "title": "Hit", "length": 200000, "artist-credit": [{ "name": "Solo", "artist": { "id": "solo-1", "name": "Solo" } }],
                "releases": [
                    { "id": "r1", "title": "Best Of Everything", "release-group": { "id": "rg-best", "primary-type": "Album", "secondary-types": ["Compilation"] }, "media": [{ "track-count": 30 }] },
                    { "id": "r2", "title": "Now 99", "release-group": { "id": "rg-now", "primary-type": "Album", "secondary-types": ["Compilation"] }, "media": [{ "track-count": 20 }, { "track-count": 20 }] },
                ] }))
        }))
        .with_state(hits.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    (url, hits)
}

fn add(ts: &TestServer, album_id: &str, album: &str, n: usize, rgid: &str, rec: &str) {
    for i in 0..n {
        ts.st.db.execute("INSERT INTO tracks (path, album_id, album, albumartist, title, track, mb_releasegroupid, mb_trackid, releasetype, tracktotal, compilation, added_via)
                          VALUES (?,?,?,?,?,?,?,?,'',0,0,'scan')",
            p![format!("/m/{album_id}/{i}.flac"), album_id, album, "Solo", format!("Song {i}"), i as i64 + 1, rgid, rec]).unwrap();
    }
}

#[tokio::test]
async fn the_types_pass_asks_musicbrainz_in_the_background() {
    let (url, hits) = musicbrainz().await;
    let ts = TestServer::new("types-pass").with(json!({ "musicbrainz_url": url, "musicbrainz_rate": 50.0 }));
    add(&ts, "a-live", "Live Album", 2, "rg-live", "");
    add(&ts, "b-lone", "Now 99", 1, "", "rec-1");
    add(&ts, "c-gone", "Gone", 5, "rg-gone", "");
    add(&ts, "d-down", "Down", 4, "rg-down", "");
    // at most the limit: the likely One-offs first
    let (looked, unknown) = album_types::look_up_missing(&ts.st, 2).await;
    assert_eq!((looked, unknown), (2, 0));
    assert_eq!(*hits.lock().unwrap(), vec!["release-group/rg-live".to_string(), "recording/rec-1".to_string()]);
    let stats = ts.st.mb().line_stats();
    assert_eq!(stats["background"]["turns"], 2, "{stats}");
    let row = |a: &str| ts.st.db.one("SELECT * FROM album_types WHERE album_id=?", p![a]).unwrap();
    let live = row("a-live").unwrap();
    assert_eq!((live.s("release_source"), live.s("release_type"), live.s("release_secondary")), ("musicbrainz".into(), "Album".into(), "[\"Live\"]".into()));
    // the release whose title is the album's, its songs over all media
    let lone = row("b-lone").unwrap();
    assert_eq!((lone.s("release_id"), lone.i("release_tracks")), ("rg-now".into(), 40));
    let types = ts.st.library.types.get(&ts.st.db);
    assert_eq!((types["a-live"].kind, types["a-live"].format), ("live", "album"));
    assert_eq!((types["b-lone"].kind, types["b-lone"].format, types["b-lone"].why), ("compilation", "one_off", "partial"));
    // the rest: not found is stamped (not asked again for 30 days), a failure isn't (asked next run)
    let line = album_types::scheduled(&ts.st).await.unwrap();
    assert_eq!(line, "0 song tags read, 1 albums looked up (1 not on MusicBrainz)");
    let gone = row("c-gone").unwrap();
    assert!(gone.get("release_type").unwrap().is_null() && gone.f("looked_up_at") > 0.0);
    assert!(row("d-down").is_none(), "a failure writes nothing");
    let before = hits.lock().unwrap().len();
    album_types::scheduled(&ts.st).await.unwrap();
    assert_eq!(hits.lock().unwrap()[before..], ["release-group/rg-down".to_string()], "only the one that failed is asked again");
    assert_eq!(ts.st.db.query("SELECT message FROM activity WHERE message LIKE 'Sort albums by type:%'", p![]).unwrap().len(), 1, "one line for the run that looked up");
    ts.remove();
}

/// A request's import stamps who asked (its user, how, when) on the new songs; one without a user (a lossless
/// upgrade) leaves what the new copy inherited from the copy it replaced; a report's replacement keeps the first
/// adder.
#[tokio::test]
async fn imports_say_who_asked() {
    let ts = TestServer::new("who-asked");
    let alex = user(&ts.st, "alex", "user").i("id");
    let db = &ts.st.db;
    let req = |kind: &str, by: Option<i64>, detail: serde_json::Value| -> i64 {
        db.execute("INSERT INTO requests (user_id, kind, status, created_at, updated_at, detail) VALUES (?,?,'completed',?,?,?)",
            p![by, kind, 1500.0, 1500.0, detail.to_string()]).unwrap()
    };
    let song = |name: &str| -> std::path::PathBuf {
        let path = std::path::PathBuf::from(format!("/m/{name}.flac"));
        db.execute("INSERT INTO tracks (path, album_id, title, added_at, added_via) VALUES (?, 'x', ?, ?, 'scan')", p![path.to_string_lossy(), name, now()]).unwrap();
        path
    };
    let origin = json!({ "source": "Soulseek" });
    let stamped = |p: &std::path::Path| {
        let r = db.one("SELECT added_by, added_via, requested_at, request_id FROM tracks WHERE path=?", p![p.to_string_lossy()]).unwrap().unwrap();
        (r.get("added_by").and_then(|v| v.as_i64()), r.s("added_via"), r.get("requested_at").and_then(|v| v.as_f64()), r.i("request_id"))
    };
    for (kind, detail, via) in [("album", json!({}), "request"), ("url", json!({}), "link"), ("track", json!({ "via": "soulseek" }), "soulseek")] {
        let id = req(kind, Some(alex), detail.clone());
        let row = db.one("SELECT * FROM requests WHERE id=?", p![id]).unwrap().unwrap();
        let path = song(&format!("{kind}-song"));
        encore_requests::worker::record_origin(&ts.st, std::slice::from_ref(&path), &origin, id, 1000.0, encore_requests::worker::adder_of(&row, &detail));
        assert_eq!(stamped(&path), (Some(alex), via.to_string(), Some(1500.0), id), "{kind}");
    }
    // a lossless upgrade (no user) replacing alex's song: the new copy keeps "Requested by alex"
    let old = song("old");
    db.execute("UPDATE tracks SET added_by=?, added_via='request', requested_at=1200 WHERE path=?", p![alex, old.to_string_lossy()]).unwrap();
    let new = song("new");
    let (oid, nid) = (db.one("SELECT id FROM tracks WHERE path=?", p![old.to_string_lossy()]).unwrap().unwrap().i("id"),
                      db.one("SELECT id FROM tracks WHERE path=?", p![new.to_string_lossy()]).unwrap().unwrap().i("id"));
    encore_library::delete_tracks(&ts.st, &[oid], Some(nid));
    let upgrade = req("album", None, json!({ "lossless_only": true }));
    let row = db.one("SELECT * FROM requests WHERE id=?", p![upgrade]).unwrap().unwrap();
    assert!(encore_requests::worker::adder_of(&row, &json!({})).is_none());
    encore_requests::worker::record_origin(&ts.st, std::slice::from_ref(&new), &origin, upgrade, 1000.0, None);
    assert_eq!(stamped(&new), (Some(alex), "request".to_string(), Some(1200.0), upgrade));
    ts.remove();
}

/// The facts of the release an album is tagged from are kept for the album it lands in, and the files get
/// Picard's RELEASETYPE (and the compilation flag of a various-artists album).
#[tokio::test]
async fn imports_keep_the_release_facts() {
    let ts = TestServer::new("release-facts");
    let rel = json!({ "provider": "MusicBrainz", "id": "r1", "release_group_id": "rg-1", "title": "Hits Live", "artist": "Various Artists",
                      "artist_id": album_types::VA_MBID, "type": "Album", "secondary": ["Compilation", "Live"], "track_count": 12 });
    let vals = encore_library::tagger::release_values(&rel, &json!({ "title": "Song", "track": 1, "disc": 1 }));
    assert_eq!(vals.get("releasetype"), Some(&json!("album; compilation; live")));
    assert_eq!(vals.get("compilation"), Some(&json!("1")));
    let store = json!({ "provider": "Deezer", "id": "deezer:5", "title": "Song - Single", "type": "Single" });
    assert!(encore_library::tagger::release_values(&store, &json!({ "title": "Song" })).get("releasetype").is_none(), "only MusicBrainz's words");
    ts.st.db.execute("INSERT INTO tracks (path, album_id, title) VALUES ('/m/hits/1.flac', 'hits', 'Song')", p![]).unwrap();
    let pv = json!({ "release": rel, "tracks": [{}, {}] });
    encore_library::tagger::record_release(&ts.st, &pv, &[std::path::PathBuf::from("/m/hits/1.flac")]);
    let r = ts.st.db.one("SELECT * FROM album_types WHERE album_id='hits'", p![]).unwrap().unwrap();
    assert_eq!((r.s("release_source"), r.s("release_id"), r.s("release_type"), r.i("release_tracks")), ("musicbrainz".into(), "rg-1".into(), "Album".into(), 12));
    let c = ts.st.library.types.of(&ts.st.db, "hits");
    assert_eq!((c.kind, c.format, c.why), ("compilation", "one_off", "partial"), "1 of its 12 songs");
    ts.remove();
}
