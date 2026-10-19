//! Share links for songs, albums and artists through the real router: one token per user and thing,
//! the public page and JSON, streams limited to the listed songs, the preview tags each
//! `share_preview` setting gives, and the preview video (when this machine has ffmpeg).
use super::{share_page_html, share_video_budget};
use encore_db::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;
use crate::test_support::{send, Resp};

const PW: &str = "sharer-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: PathBuf,
    music: PathBuf,
    cookie: String,
    next: i64,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-share-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut st = crate::test_support::state_in(&dir);
    // the real share page template (three folders up from crates/api: the repository root)
    st.static_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../web-rs/static");
    crate::auth::create_user(&st, "sharer", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "sharer", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    let music = dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    Env { st, app, dir, music, cookie, next: 1 }
}

async fn call(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Resp {
    send(app, method, uri, cookie, body, &[]).await
}

impl Env {
    /// A song whose file exists (an .mp3 is served as is, so the bytes don't have to be real audio).
    fn add(&mut self, title: &str, artist: &str, album: &str, albumartist: &str, year: &str) -> i64 {
        let id = self.next;
        self.next += 1;
        let aid = encore_library::album_key(albumartist, album, "");
        let folder = self.music.join(encore_library::album_key(albumartist, album, ""));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("{id}.mp3"));
        std::fs::write(&path, b"ID3 not really audio").unwrap();
        self.st.db.execute(
            "INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, bitrate, duration, size, year, disc, track, added_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![id, path.to_string_lossy(), title, artist, album, albumartist, aid, "mp3", 320, 180.0, 20, year, 1, id, id as f64]).unwrap();
        self.st.library.artists.invalidate();
        id
    }
    fn album_id(&self, id: i64) -> String {
        self.st.db.one("SELECT album_id FROM tracks WHERE id=?", p![id]).unwrap().unwrap()["album_id"].as_str().unwrap().to_string()
    }
    async fn post(&self, uri: &str, body: Option<Value>) -> Resp {
        call(&self.app, "POST", uri, &self.cookie, body).await
    }
    /// Public: no cookie.
    async fn public(&self, uri: &str) -> Resp {
        call(&self.app, "GET", uri, "", None).await
    }
}

#[tokio::test]
async fn album_and_artist_links_list_and_play_only_their_songs() {
    let mut e = setup("links").await;
    let a1 = e.add("Hey Mama", "Black Eyed Peas", "Elephunk", "Black Eyed Peas", "2003");
    let a2 = e.add("Shut Up", "Black Eyed Peas", "Elephunk", "Black Eyed Peas", "2003");
    let b1 = e.add("Pump It", "The Black Eyed Peas", "Monkey Business", "The Black Eyed Peas", "2005");
    let other = e.add("Bookends Theme", "Simon & Garfunkel", "Bookends", "Simon & Garfunkel", "1968");
    let elephunk = e.album_id(a1);
    std::fs::write(e.music.join(&elephunk).join("cover.jpg"), b"\xff\xd8\xff not a real jpeg").unwrap();

    // album: one token per user and album
    let r = e.post(&format!("/api/library/albums/{elephunk}/share"), None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    let tok = j["token"].as_str().unwrap().to_string();
    assert_eq!(j["path"], format!("/s/{tok}"));
    assert!(j["url"].as_str().unwrap().ends_with(&format!("/s/{tok}")));
    assert_eq!(e.post(&format!("/api/library/albums/{elephunk}/share"), None).await.json()["token"], tok.as_str());
    assert_eq!(e.post("/api/library/albums/nope/share", None).await.status, StatusCode::NOT_FOUND);
    // signed out: no new links
    assert_eq!(call(&e.app, "POST", &format!("/api/library/albums/{elephunk}/share"), "", None).await.status, StatusCode::UNAUTHORIZED);

    let m = e.public(&format!("/api/share/{tok}")).await;
    assert_eq!(m.status, StatusCode::OK);
    let m = m.json();
    assert_eq!(m["kind"], "album");
    assert_eq!(m["title"], "Elephunk");
    assert_eq!(m["artist"], "Black Eyed Peas");
    assert_eq!(m["year"], "2003");
    assert_eq!(m["count"], 2);
    let ids: Vec<i64> = m["tracks"].as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![a1, a2]);
    assert!(!m.to_string().contains("/music/"), "no file paths: {m}");

    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={a2}")).await.status, StatusCode::OK);
    assert_eq!(e.public(&format!("/api/share/{tok}/stream")).await.status, StatusCode::OK);
    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={b1}")).await.status, StatusCode::NOT_FOUND);
    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={other}")).await.status, StatusCode::NOT_FOUND);
    assert_eq!(e.public(&format!("/api/share/{tok}/cover")).await.status, StatusCode::OK);

    let page = e.public(&format!("/s/{tok}")).await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    assert!(html.contains(r#"<meta property="og:title" content="Elephunk – Black Eyed Peas">"#), "{html}");
    assert!(html.contains(r#"<meta property="og:type" content="music.album">"#), "{html}");
    assert!(html.contains("2 songs"), "{html}");

    // artist: any spelling finds the merged artist; its songs from both spellings, most played first
    e.st.db.execute("INSERT INTO plays (user_id, track_id, seconds, played_at) VALUES (1, ?, 100, 1), (1, ?, 100, 2)", p![b1, b1]).unwrap();
    let r = e.post("/api/library/artists/share", Some(json!({ "name": "the black eyed peas" }))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let atok = r.json()["token"].as_str().unwrap().to_string();
    assert_ne!(atok, tok);
    assert_eq!(e.post("/api/library/artists/share", Some(json!({ "name": "Black Eyed Peas" }))).await.json()["token"], atok.as_str());
    assert_eq!(e.post("/api/library/artists/share", Some(json!({ "name": "Nobody" }))).await.status, StatusCode::NOT_FOUND);
    assert_eq!(e.post("/api/library/artists/share", Some(json!({ "name": " " }))).await.status, StatusCode::BAD_REQUEST);
    let m = e.public(&format!("/api/share/{atok}")).await.json();
    assert_eq!(m["kind"], "artist");
    assert_eq!(m["title"], "Black Eyed Peas");
    let ids: Vec<i64> = m["tracks"].as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert_eq!(ids.first(), Some(&b1), "most played first: {ids:?}");
    assert_eq!(ids.len(), 3, "{ids:?}");
    assert!(ids.contains(&a1) && ids.contains(&a2));
    assert_eq!(e.public(&format!("/api/share/{atok}/stream?track={a1}")).await.status, StatusCode::OK);
    assert_eq!(e.public(&format!("/api/share/{atok}/stream?track={other}")).await.status, StatusCode::NOT_FOUND);
    let html = e.public(&format!("/s/{atok}")).await.text();
    assert!(html.contains(r#"<meta property="og:title" content="Black Eyed Peas">"#), "{html}");

    // songs work as before, and a song link ignores other ids
    let r = e.post(&format!("/api/library/tracks/{a1}/share"), None).await;
    let stok = r.json()["token"].as_str().unwrap().to_string();
    assert!(stok != tok && stok != atok);
    assert_eq!(e.post(&format!("/api/library/tracks/{a1}/share"), None).await.json()["token"], stok.as_str());
    let m = e.public(&format!("/api/share/{stok}")).await.json();
    assert_eq!((m["kind"].as_str(), m["title"].as_str()), (Some("track"), Some("Hey Mama")));
    assert_eq!(e.public(&format!("/api/share/{stok}/stream")).await.status, StatusCode::OK);
    assert_eq!(e.public(&format!("/api/share/{stok}/stream?track={a2}")).await.status, StatusCode::NOT_FOUND);

    // unknown links
    assert_eq!(e.public("/api/share/nothing-here").await.status, StatusCode::NOT_FOUND);
    assert_eq!(e.public("/api/share/nothing-here/stream").await.status, StatusCode::NOT_FOUND);
    assert_eq!(e.public("/api/share/nothing-here/video.mp4").await.status, StatusCode::NOT_FOUND);
}

fn meta_song() -> Value {
    json!({ "kind": "track", "title": "Hey Mama", "artist": "Black Eyed Peas", "album": "Elephunk", "year": "2003", "duration": 214.0,
            "format": "mp3", "bitrate": 320, "lossless": false, "app_name": "Encore" })
}

#[test]
fn preview_tags_follow_the_setting() {
    let html = "<title>Shared song</title>\n<!--META-->";
    let small = share_page_html(html, &meta_song(), "https://m.example", "tok", "small", false);
    assert!(small.contains(r#"<meta name="twitter:card" content="summary">"#), "{small}");
    assert!(small.contains(r#"<meta property="og:audio" content="https://m.example/api/share/tok/stream">"#));
    assert!(small.contains(r#"<meta property="og:type" content="music.song">"#));
    assert!(small.contains("Elephunk (2003) · MP3 · 320 kbps · 3:34 · Shared from Encore"), "{small}");
    assert!(!small.contains("og:video"));
    let large = share_page_html(html, &meta_song(), "https://m.example", "tok", "large", false);
    assert!(large.contains(r#"<meta name="twitter:card" content="summary_large_image">"#), "{large}");
    // playable, made: a video card that keeps the audio
    let play = share_page_html(html, &meta_song(), "https://m.example", "tok", "playable", true);
    for tag in [
        r#"<meta name="twitter:card" content="player">"#,
        r#"<meta property="og:video" content="https://m.example/api/share/tok/video.mp4">"#,
        r#"<meta property="og:video:secure_url" content="https://m.example/api/share/tok/video.mp4">"#,
        r#"<meta property="og:video:type" content="video/mp4">"#,
        r#"<meta property="og:video:width" content="600"><meta property="og:video:height" content="600">"#,
        r#"<meta name="twitter:player:stream" content="https://m.example/api/share/tok/video.mp4">"#,
        r#"<meta property="og:audio" content="https://m.example/api/share/tok/stream">"#,
    ] {
        assert!(play.contains(tag), "{tag} missing: {play}");
    }
    // playable but ffmpeg failed: the small card
    let failed = share_page_html(html, &meta_song(), "https://m.example", "tok", "playable", false);
    assert!(failed.contains(r#"<meta name="twitter:card" content="summary">"#) && !failed.contains("og:video"), "{failed}");
    // an unknown value: small
    assert!(share_page_html(html, &meta_song(), "b", "tok", "huge", false).contains(r#"content="summary">"#));
    // text is escaped
    let mut m = meta_song();
    m["title"] = json!("<b>\"x\"</b>");
    let esc = share_page_html(html, &m, "b", "tok", "small", false);
    assert!(!esc.contains("<b>") && esc.contains("&lt;b&gt;"), "{esc}");
}

#[test]
fn video_budget_keeps_the_file_under_8_mb() {
    assert_eq!(share_video_budget(200.0), (128, 200.0));
    let (k, s) = share_video_budget(400.0);
    assert_eq!((k, s), (96, 400.0));
    let (k, s) = share_video_budget(3600.0);
    assert_eq!(k, 96);
    assert!(s < 3600.0 && s > 300.0, "{s}");
    let (k, s) = share_video_budget(0.0);
    assert_eq!(k, 96);
    assert!(s > 300.0 && s < 500.0);
    for d in [10.0, 200.0, 341.0, 400.0, 3600.0] {
        let (k, s) = share_video_budget(d);
        // audio + a generous allowance for the still picture
        assert!(s * (k as f64 + 48.0) * 1000.0 / 8.0 <= 7_500_001.0, "{d}");
    }
}

#[tokio::test]
async fn the_setting_picks_the_card_and_playable_serves_a_video() {
    let mut e = setup("video").await;
    let id = e.add("Tone", "Tester", "Tones", "Tester", "2024");
    let r = e.post(&format!("/api/library/tracks/{id}/share"), None).await;
    let tok = r.json()["token"].as_str().unwrap().to_string();
    let card = |html: &str| html.split(r#"<meta name="twitter:card" content=""#).nth(1).and_then(|s| s.split('"').next()).unwrap_or("").to_string();

    // default: small
    assert_eq!(e.st.cfg.str("share_preview"), "small");
    assert_eq!(card(&e.public(&format!("/s/{tok}")).await.text()), "summary");
    e.st.cfg.set_many(&json!({ "share_preview": "large" }));
    assert_eq!(card(&e.public(&format!("/s/{tok}")).await.text()), "summary_large_image");
    // no video outside "playable": a public link never starts ffmpeg then
    assert_eq!(e.public(&format!("/api/share/{tok}/video.mp4")).await.status, StatusCode::NOT_FOUND);
    // playable, but the "audio" can't be converted: the small card
    e.st.cfg.set_many(&json!({ "share_preview": "playable" }));
    let html = e.public(&format!("/s/{tok}")).await.text();
    assert_eq!(card(&html), "summary", "{html}");
    assert!(!html.contains("og:video"));

    if !encore_media::ffmpeg::available() {
        eprintln!("ffmpeg not found: skipping the preview video check");
        return;
    }
    // a real 3-second song and cover
    let path = e.st.db.one("SELECT path FROM tracks WHERE id=?", p![id]).unwrap().unwrap()["path"].as_str().unwrap().to_string();
    let folder = std::path::Path::new(&path).parent().unwrap().to_path_buf();
    let flac = folder.join("tone.flac");
    let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"]).arg(&flac)
        .status().unwrap().success();
    assert!(ok);
    let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "color=c=red:s=300x200", "-frames:v", "1"]).arg(folder.join("cover.jpg"))
        .status().unwrap().success();
    assert!(ok);
    e.st.db.execute("UPDATE tracks SET path=?, format='flac', duration=3 WHERE id=?", p![flac.to_string_lossy(), id]).unwrap();

    let html = e.public(&format!("/s/{tok}")).await.text();
    assert_eq!(card(&html), "player", "{html}");
    assert!(html.contains(&format!("/api/share/{tok}/video.mp4\">")), "{html}");
    assert!(html.contains(r#"<meta property="og:video:type" content="video/mp4">"#));
    let v = e.public(&format!("/api/share/{tok}/video.mp4")).await;
    assert_eq!(v.status, StatusCode::OK);
    assert_eq!(v.h("content-type"), "video/mp4");
    assert!(v.body.len() > 1000 && v.body.len() < 8_000_000, "{}", v.body.len());
    assert_eq!(&v.body[4..8], b"ftyp");
    // faststart: the index comes before the audio
    let moov = v.body.windows(4).position(|w| w == b"moov").unwrap();
    let mdat = v.body.windows(4).position(|w| w == b"mdat").unwrap();
    assert!(moov < mdat);
    // cached, and made again for a changed song
    let cache = e.st.db.data_dir.join("cache").join("share-video");
    let files = |c: &PathBuf| std::fs::read_dir(c).unwrap().flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
    let before = files(&cache);
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(before[0].starts_with(&format!("{tok}.{id}.")));
    let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "sine=frequency=220:duration=2"]).arg(&flac)
        .status().unwrap().success();
    assert!(ok);
    assert_eq!(e.public(&format!("/api/share/{tok}/video.mp4")).await.status, StatusCode::OK);
    let after = files(&cache);
    assert_eq!(after.len(), 1, "{after:?}");
    assert_ne!(after, before);

    // an album link previews its first song
    let aid = e.album_id(id);
    let atok = e.post(&format!("/api/library/albums/{aid}/share"), None).await.json()["token"].as_str().unwrap().to_string();
    assert_eq!(card(&e.public(&format!("/s/{atok}")).await.text()), "player");
    assert_eq!(e.public(&format!("/api/share/{atok}/video.mp4")).await.status, StatusCode::OK);
}
