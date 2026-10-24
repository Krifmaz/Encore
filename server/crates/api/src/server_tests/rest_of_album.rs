//! "Get the rest of the album" for an album MusicBrainz doesn't list (the owner on Oct 24: "what is this about not
//! being able to find the rest of an album because it's not on MusicBrainz"), against a stand-in MusicBrainz,
//! Deezer and Apple Music on 127.0.0.1: the lookup finds the album on Deezer when MusicBrainz has nothing (or is
//! down), with the library's songs marked; Apple Music is asked when Deezer doesn't have it; no catalogue having
//! it is an answer that says what to do instead, not an error; the missing songs – a catalogue's or typed in by
//! hand – are requested one by one, the owned ones left out; and an album asked for by name to fill it in isn't
//! turned away as "Already in library".
use crate::test_support::{call, user, TestServer};
use axum::http::StatusCode;
use axum::Router;
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A stand-in MusicBrainz whose release-group search finds nothing, or answers 500 while `down` is set.
async fn musicbrainz(down: Arc<AtomicBool>) -> String {
    use axum::http::StatusCode;
    use axum::routing::get;
    let app = axum::Router::new().route("/ws/2/release-group", get(move || {
        let down = down.clone();
        async move {
            if down.load(Ordering::SeqCst) {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            Ok(axum::Json(json!({ "release-groups": [] })))
        }
    }));
    serve(app).await
}

/// A stand-in Deezer: its album search finds "Lost Tapes" by Band (album 777, three songs) and nothing else.
async fn deezer() -> String {
    use axum::extract::{Path as P, Query};
    use axum::routing::get;
    use std::collections::HashMap;
    let app = axum::Router::new()
        .route("/search/album", get(|Query(q): Query<HashMap<String, String>>| async move {
            let q = q.get("q").cloned().unwrap_or_default().to_lowercase();
            let data = if q.contains("lost tapes") {
                json!([{ "id": 777, "title": "Lost Tapes", "artist": { "name": "Band" }, "release_date": "2019-02-01", "nb_tracks": 3,
                         "record_type": "album", "cover_medium": "http://127.0.0.1:9/c.jpg" }])
            } else {
                json!([])
            };
            axum::Json(json!({ "data": data }))
        }))
        .route("/album/{id}/tracks", get(|P(id): P<i64>| async move {
            assert_eq!(id, 777);
            axum::Json(json!({ "data": [
                { "id": 1, "title": "Owned Song", "artist": { "name": "Band" }, "duration": 200, "isrc": "USX111111111", "track_position": 1, "disk_number": 1 },
                { "id": 2, "title": "Missing One", "artist": { "name": "Band" }, "duration": 210, "isrc": "USX222222222", "track_position": 2, "disk_number": 1 },
                { "id": 3, "title": "Missing Two", "artist": { "name": "Band" }, "duration": 220, "isrc": "USX333333333", "track_position": 3, "disk_number": 1 },
            ] }))
        }));
    serve(app).await
}

/// A stand-in Apple Music: its search finds "Apple Only" by Band (collection 4242) and nothing else; its
/// lookup gives that album's two songs.
async fn apple() -> String {
    use axum::extract::Query;
    use axum::routing::get;
    use std::collections::HashMap;
    let app = axum::Router::new()
        .route("/search", get(|Query(q): Query<HashMap<String, String>>| async move {
            let term = q.get("term").cloned().unwrap_or_default().to_lowercase();
            let results = if term.contains("apple only") {
                json!([{ "collectionId": 4242, "collectionName": "Apple Only", "artistName": "Band", "releaseDate": "2020-01-01T00:00:00Z", "trackCount": 2 }])
            } else {
                json!([])
            };
            axum::Json(json!({ "resultCount": results.as_array().unwrap().len(), "results": results }))
        }))
        .route("/lookup", get(|| async {
            axum::Json(json!({ "results": [
                { "wrapperType": "collection", "collectionId": 4242 },
                { "wrapperType": "track", "trackName": "First", "artistName": "Band", "trackNumber": 1, "trackTimeMillis": 180000 },
                { "wrapperType": "track", "trackName": "Second", "artistName": "Band", "trackNumber": 2, "trackTimeMillis": 190000 },
            ] }))
        }));
    serve(app).await
}

/// Serve `app` on a free port of 127.0.0.1 → its address.
async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    url
}

/// A server asking the stand-ins, with "Owned Song" by Band (of "Lost Tapes") in its library and no download
/// source → (the server, its router, `down` for MusicBrainz).
async fn server(name: &str) -> (TestServer, Router, Arc<AtomicBool>) {
    let down = Arc::new(AtomicBool::new(false));
    let mb_url = musicbrainz(down.clone()).await;
    let urls = encore_http::Urls { deezer: deezer().await, apple: apple().await, ..Default::default() };
    let ts = TestServer::new(&format!("rest-{name}")).with_urls(urls);
    ts.st.cfg.set_many(&json!({ "musicbrainz_url": mb_url, "musicbrainz_rate": 50.0, "auto_approve": true, "discogs_token": "",
        "slskd_enabled": false, "ytdlp_enabled": false, "ytdlp_last_resort": false, "streamrip_enabled": false }));
    ts.st.db.execute("INSERT INTO tracks (path, title, artist, albumartist, album, album_id, format, duration) VALUES
                      ('/m/band/lost/1.flac', 'Owned Song', 'Band', 'Band', 'Lost Tapes', 'lost', 'flac', 200)", p![]).unwrap();
    let app = ts.app();
    (ts, app, down)
}

/// Sign `name` in → its session cookie.
async fn login(app: &Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

/// The titles of the track requests, in order.
fn requested(ts: &TestServer) -> Vec<String> {
    ts.st.db.query("SELECT title FROM requests WHERE kind='track' ORDER BY id", p![]).unwrap().iter().map(|r| r.s("title")).collect()
}

#[tokio::test]
async fn an_album_musicbrainz_lacks_is_found_on_deezer_and_its_missing_songs_requested() {
    let (ts, app, down) = server("deezer").await;
    user(&ts.st, "owner", "admin");
    let c = login(&app, "owner").await;
    let (code, v, _) = call(&app, "GET", "/api/discover/album-lookup?artist=Band&album=Lost%20Tapes", &c, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["found"], "elsewhere");
    assert_eq!(v["rgid"], Value::Null);
    assert_eq!(v["source"], "Deezer");
    assert_eq!(v["ext"], json!({ "deezer": 777 }));
    assert_eq!(v["owned_tracks"], 1);
    let marks: Vec<(String, bool)> = v["tracks"].as_array().unwrap().iter().map(|t| (t["title"].as_str().unwrap().to_string(), t["in_library"] == true)).collect();
    assert_eq!(marks, vec![("Owned Song".into(), true), ("Missing One".into(), false), ("Missing Two".into(), false)]);

    // MusicBrainz down: the same answer, not an error
    down.store(true, Ordering::SeqCst);
    let (code, v2, _) = call(&app, "GET", "/api/discover/album-lookup?artist=Band&album=Lost%20Tapes", &c, None).await;
    assert_eq!(code, StatusCode::OK, "{v2}");
    assert_eq!(v2["found"], "elsewhere");

    // the missing two are requested, the owned one isn't
    let body = json!({ "artist": "Band", "album": "Lost Tapes", "year": "2019", "tracks": v["tracks"] });
    let (code, r, _) = call(&app, "POST", "/api/discover/album-rest/request", &c, Some(body.clone())).await;
    assert_eq!(code, StatusCode::OK, "{r}");
    assert_eq!((r["requested"].as_i64(), r["in_library"].as_i64(), r["already_requested"].as_i64()), (Some(2), Some(1), Some(0)));
    assert_eq!(r["status"], "queued");
    assert_eq!(requested(&ts), vec!["Missing One", "Missing Two"]);
    let d: Value = serde_json::from_str(&ts.st.db.one("SELECT detail FROM requests WHERE title='Missing One'", p![]).unwrap().unwrap().s("detail")).unwrap();
    assert_eq!(d["album_mode"], "song");
    assert_eq!(d["isrc"], "USX222222222");
    // asked again: nothing new, both counted as already requested
    let (_, r, _) = call(&app, "POST", "/api/discover/album-rest/request", &c, Some(body)).await;
    assert_eq!((r["requested"].as_i64(), r["already_requested"].as_i64()), (Some(0), Some(2)));
    ts.remove();
}

#[tokio::test]
async fn apple_music_is_asked_when_deezer_lacks_it() {
    let (ts, app, _) = server("apple").await;
    user(&ts.st, "owner", "admin");
    let c = login(&app, "owner").await;
    let (code, v, _) = call(&app, "GET", "/api/discover/album-lookup?artist=Band&album=Apple%20Only", &c, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["found"], "elsewhere");
    assert_eq!(v["source"], "Apple Music");
    assert_eq!(v["ext"], json!({ "apple": 4242 }));
    let titles: Vec<&str> = v["tracks"].as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap()).collect();
    assert_eq!(titles, vec!["First", "Second"]);
    ts.remove();
}

#[tokio::test]
async fn no_catalogue_is_an_answer_and_a_typed_track_list_is_requested() {
    let (ts, app, _) = server("typed").await;
    user(&ts.st, "member", "user");
    let c = login(&app, "member").await;
    let (code, v, _) = call(&app, "GET", "/api/discover/album-lookup?artist=Band&album=Nowhere%20Album", &c, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["found"], "none");
    assert_eq!(v["rgid"], Value::Null);
    let msg = v["message"].as_str().unwrap();
    assert!(msg.contains("Nowhere Album") && msg.contains("Type its songs in"), "{msg}");

    // typed in by hand: numbers and lengths read off, the owned song left out
    let text = "1. Owned Song (3:20)\n2. Typed Song 4:01\n\n3) Another One\n";
    let (code, r, _) = call(&app, "POST", "/api/discover/album-rest/request", &c, Some(json!({ "artist": "Band", "album": "Lost Tapes", "text": text }))).await;
    assert_eq!(code, StatusCode::OK, "{r}");
    assert_eq!((r["requested"].as_i64(), r["in_library"].as_i64()), (Some(2), Some(1)));
    assert_eq!(requested(&ts), vec!["Typed Song", "Another One"]);
    let d: Value = serde_json::from_str(&ts.st.db.one("SELECT detail FROM requests WHERE title='Typed Song'", p![]).unwrap().unwrap().s("detail")).unwrap();
    assert_eq!(d["length"], json!(241.0));
    // nothing typed: a plain 400 that says what to do
    let (code, e, _) = call(&app, "POST", "/api/discover/album-rest/request", &c, Some(json!({ "artist": "Band", "album": "Lost Tapes", "text": "\n \n" }))).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(e["detail"].as_str().unwrap_or("").contains("one per line"), "{e}");
    ts.remove();
}

#[tokio::test]
async fn an_album_asked_for_by_name_to_fill_it_in_is_not_already_in_the_library() {
    let (ts, app, _) = server("by-name").await;
    user(&ts.st, "owner", "admin");
    let c = login(&app, "owner").await;
    // no MusicBrainz id and no catalogue entry: refused without fill_in, as before
    let (code, _, _) = call(&app, "POST", "/api/requests", &c, Some(json!({ "kind": "album", "artist": "Band", "title": "Lost Tapes" }))).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, r, _) = call(&app, "POST", "/api/requests", &c, Some(json!({ "kind": "album", "artist": "Band", "title": "Lost Tapes", "fill_in": true }))).await;
    assert_eq!(code, StatusCode::OK, "{r}");
    let id = r["id"].as_i64().unwrap();
    assert_eq!(r["detail"]["fill_in"], true);
    // asked again by name while it runs: the usual duplicate
    let (code, _, _) = call(&app, "POST", "/api/requests", &c, Some(json!({ "kind": "album", "artist": "Band", "title": "Lost Tapes", "fill_in": true }))).await;
    assert_eq!(code, StatusCode::CONFLICT);
    // the worker goes on to the sources (none here) instead of "Already in library"
    encore_requests::worker::step(ts.st.clone(), id).await;
    let row = ts.st.db.one("SELECT status, message FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    assert_ne!(row.s("message"), "Already in library", "{} {}", row.s("status"), row.s("message"));
    ts.remove();
}
