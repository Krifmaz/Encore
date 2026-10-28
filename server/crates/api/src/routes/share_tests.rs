//! Share links for songs, albums, artists and playlists through the real router: one token per user and
//! thing, the public page and JSON, streams limited to the listed songs, the preview tags each
//! `share_preview` setting gives, and the preview video (when this machine has ffmpeg). Playlist links
//! (Liked Songs too) show the playlist as it is now, page long lists, end with Stop sharing, a deleted
//! playlist or a disabled owner, and can be saved as a copy by a signed-in listener.
use super::{share_page_html, share_video_budget, SHARE_VIDEO_BLANK};
use encore_db::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;
use crate::test_support::{send, Resp};
use super::public_description;

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
    async fn get(&self, uri: &str) -> Resp {
        call(&self.app, "GET", uri, &self.cookie, None).await
    }
    async fn del(&self, uri: &str) -> Resp {
        call(&self.app, "DELETE", uri, &self.cookie, None).await
    }
    /// A playlist of the signed-in user's with these library songs, in this order.
    async fn playlist(&self, name: &str, ids: &[i64]) -> i64 {
        let r = self.post("/api/playlists", Some(json!({ "name": name }))).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        let pid = r.json()["id"].as_i64().unwrap();
        if !ids.is_empty() {
            let r = self.post(&format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": ids }))).await;
            assert_eq!(r.json()["added"], ids.len(), "{}", r.text());
        }
        pid
    }
    /// Shares a playlist: its token.
    async fn share_playlist(&self, pid: i64) -> String {
        let r = self.post(&format!("/api/playlists/{pid}/share"), None).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        r.json()["token"].as_str().unwrap().to_string()
    }
    /// A second account, signed in: its session cookie.
    async fn login(&self, name: &str) -> String {
        crate::auth::create_user(&self.st, name, PW, "user", None, None).unwrap();
        let r = call(&self.app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": PW }))).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        r.h("set-cookie").split(';').next().unwrap().to_string()
    }
    /// The song ids a link's page lists.
    async fn listed(&self, uri: &str) -> Vec<i64> {
        let r = self.public(uri).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}: {}", r.text());
        ids_of(&r.json())
    }
    /// The playlist's item ids, in its order.
    async fn items(&self, pid: i64) -> Vec<i64> {
        self.get(&format!("/api/playlists/{pid}")).await.json()["items"].as_array().unwrap().iter().map(|i| i["id"].as_i64().unwrap()).collect()
    }
}

fn ids_of(m: &Value) -> Vec<i64> {
    m["tracks"].as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect()
}

/// The content of an Open Graph tag, None when the page has none.
fn og(html: &str, prop: &str) -> Option<String> {
    html.split(&format!(r#"<meta property="{prop}" content=""#)).nth(1).and_then(|s| s.split('"').next()).map(String::from)
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
    // the page wears the app's theme (its stylesheet, the "encore" theme and the brand accent) with the tags filled in
    assert!(html.contains(r#"<html lang="en" data-theme="encore" data-brand data-bg="solid">"#), "{html}");
    assert!(html.contains(r#"<link rel="stylesheet" href="/css/app.css">"#), "{html}");
    assert!(!html.contains("<!--META-->"), "{html}");

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

fn meta_playlist() -> Value {
    json!({ "kind": "playlist", "liked": false, "title": "Road trip", "owner": "anna", "description": "", "count": 2, "duration": 400.0,
            "tracks": [{ "id": 1 }, { "id": 2 }], "format": "flac", "app_name": "Encore" })
}

#[test]
fn preview_tags_follow_the_setting() {
    let html = "<title>Shared song</title>\n<!--META-->";
    let small = share_page_html(html, &meta_song(), "https://m.example", "tok", "small", false, false);
    assert!(small.contains(r#"<meta name="twitter:card" content="summary">"#), "{small}");
    assert!(small.contains(r#"<meta property="og:audio" content="https://m.example/api/share/tok/stream">"#));
    assert!(small.contains(r#"<meta property="og:type" content="music.song">"#));
    assert!(small.contains("Elephunk (2003) · MP3 · 320 kbps · 3:34 · Shared from Encore"), "{small}");
    assert!(!small.contains("og:video"));
    let large = share_page_html(html, &meta_song(), "https://m.example", "tok", "large", false, false);
    assert!(large.contains(r#"<meta name="twitter:card" content="summary_large_image">"#), "{large}");
    // playable, made: a video card that keeps the audio
    let play = share_page_html(html, &meta_song(), "https://m.example", "tok", "playable", true, false);
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
    let failed = share_page_html(html, &meta_song(), "https://m.example", "tok", "playable", false, false);
    assert!(failed.contains(r#"<meta name="twitter:card" content="summary">"#) && !failed.contains("og:video"), "{failed}");
    // an unknown value: small
    assert!(share_page_html(html, &meta_song(), "b", "tok", "huge", false, false).contains(r#"content="summary">"#));
    // text is escaped
    let mut m = meta_song();
    m["title"] = json!("<b>\"x\"</b>");
    let esc = share_page_html(html, &m, "b", "tok", "small", false, false);
    assert!(!esc.contains("<b>") && esc.contains("&lt;b&gt;"), "{esc}");

    // a playlist: its name and owner, and how many songs (not how long)
    let pl = share_page_html(html, &meta_playlist(), "https://m.example", "tok", "small", false, false);
    for tag in [
        "<title>Road trip · anna</title>",
        r#"<meta property="og:title" content="Road trip · anna">"#,
        r#"<meta property="og:type" content="music.playlist">"#,
        r#"<meta property="og:description" content="2 songs · Shared from Encore">"#,
        r#"<meta property="og:image" content="https://m.example/api/share/tok/cover?s=600">"#,
        r#"<meta property="og:audio" content="https://m.example/api/share/tok/stream">"#,
        r#"<meta property="og:audio:type" content="audio/flac">"#,
        r#"<meta name="twitter:image" content="https://m.example/api/share/tok/cover?s=600">"#,
        r#"<meta name="twitter:card" content="summary">"#,
    ] {
        assert!(pl.contains(tag), "{tag} missing: {pl}");
    }
    assert!(!pl.contains("music:musician") && !pl.contains("music:duration"), "{pl}");
    let mut one = meta_playlist();
    one["count"] = json!(1);
    assert!(share_page_html(html, &one, "b", "tok", "small", false, false).contains("1 song · Shared from Encore"));
    // the playable card works as for the others
    let play = share_page_html(html, &meta_playlist(), "b", "tok", "playable", true, false);
    assert!(play.contains(r#"<meta property="og:type" content="video.other">"#) && play.contains(r#"content="player">"#), "{play}");
    // nothing in the library: no audio and no picture, the rest as usual
    let mut empty = meta_playlist();
    empty["count"] = json!(0);
    empty["tracks"] = json!([]);
    let empty = share_page_html(html, &empty, "b", "tok", "large", false, false);
    assert!(empty.contains(r#"<meta property="og:title" content="Road trip · anna">"#) && empty.contains("0 songs · Shared from Encore"), "{empty}");
    assert!(!empty.contains("og:audio") && !empty.contains("og:image") && !empty.contains("twitter:image"), "{empty}");
    assert!(empty.contains(r#"<meta name="twitter:card" content="summary_large_image">"#) && empty.contains(r#"<meta name="description""#), "{empty}");
    // the name and the owner are escaped
    let mut m = meta_playlist();
    m["title"] = json!("<i>Mix</i>");
    m["owner"] = json!("\"a&b\"");
    let esc = share_page_html(html, &m, "b", "tok", "small", false, false);
    assert!(esc.contains("<title>&lt;i&gt;Mix&lt;/i&gt; · &quot;a&amp;b&quot;</title>") && !esc.contains("<i>"), "{esc}");
}

#[test]
fn discord_gets_a_card_without_audio() {
    use super::is_discord_bot;
    assert!(is_discord_bot("Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)"));
    assert!(!is_discord_bot("WhatsApp/2.23") && !is_discord_bot("facebookexternalhit/1.1"));
    let html = "<title>Shared song</title>\n<!--META-->";
    for meta in [meta_song(), meta_playlist()] {
        for (mode, video) in [("small", false), ("large", false), ("playable", true)] {
            let d = share_page_html(html, &meta, "https://m.example", "tok", mode, video, true);
            assert!(!d.contains("og:audio") && !d.contains("music:") && !d.contains("music."), "{d}");
            assert!(d.contains(r#"<meta property="og:image" content="https://m.example/api/share/tok/cover?s=600">"#), "{d}");
            assert!(d.contains(if video { r#"content="video.other">"# } else { r#"<meta property="og:type" content="website">"# }), "{d}");
        }
    }
    // everyone else keeps the audio
    assert!(share_page_html(html, &meta_song(), "b", "tok", "small", false, false).contains("og:audio"));
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
    assert_eq!(v.h("cache-control"), "public, max-age=3600");
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

    // a playlist link previews its first song, with its album's cover; an empty one has no video
    let ptok = e.share_playlist(e.playlist("One song", &[id]).await).await;
    let html = e.public(&format!("/s/{ptok}")).await.text();
    assert_eq!(card(&html), "player", "{html}");
    assert!(html.contains(r#"<meta property="og:type" content="video.other">"#), "{html}");
    let v = e.public(&format!("/api/share/{ptok}/video.mp4")).await;
    assert_eq!((v.status, v.h("content-type")), (StatusCode::OK, "video/mp4".to_string()));
    // a playlist's video changes with the list and ends with Stop sharing: no shared cache keeps it
    assert_eq!(v.h("cache-control"), "private, no-cache");
    let same = send(&e.app, "GET", &format!("/api/share/{ptok}/video.mp4"), "", None, &[("if-none-match", &v.h("etag"))]).await;
    assert_eq!(same.status, StatusCode::NOT_MODIFIED);
    let etok = e.share_playlist(e.playlist("Nothing yet", &[]).await).await;
    assert_eq!(card(&e.public(&format!("/s/{etok}")).await.text()), "summary");
    assert_eq!(e.public(&format!("/api/share/{etok}/video.mp4")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn playlist_links_show_the_playlist_as_it_is_now() {
    let mut e = setup("pl-live").await;
    let a = e.add("Hey Mama", "Black Eyed Peas", "Elephunk", "Black Eyed Peas", "2003");
    let b = e.add("Pump It", "Black Eyed Peas", "Monkey Business", "Black Eyed Peas", "2005");
    let c = e.add("Bookends Theme", "Simon & Garfunkel", "Bookends", "Simon & Garfunkel", "1968");
    let d = e.add("Mrs. Robinson", "Simon & Garfunkel", "Bookends", "Simon & Garfunkel", "1968");
    let never = e.add("Not Listed", "Nobody", "Nothing", "Nobody", "2000");
    std::fs::write(e.music.join(e.album_id(a)).join("cover.jpg"), b"\xff\xd8\xff not a real jpeg").unwrap();
    let pid = e.playlist("Road trip", &[a, b, c]).await;

    // one token per user and playlist, as songs, albums and artists
    let r = e.post(&format!("/api/playlists/{pid}/share"), None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    let tok = j["token"].as_str().unwrap().to_string();
    assert_eq!(j["path"], format!("/s/{tok}"));
    assert!(j["url"].as_str().unwrap().ends_with(&format!("/s/{tok}")));
    assert_eq!(e.share_playlist(pid).await, tok);
    assert_eq!(e.post("/api/playlists/999/share", None).await.status, StatusCode::NOT_FOUND);

    let m = e.public(&format!("/api/share/{tok}")).await.json();
    assert_eq!((m["kind"].as_str(), m["title"].as_str(), m["owner"].as_str()), (Some("playlist"), Some("Road trip"), Some("sharer")));
    assert_eq!(m["liked"], false);
    assert_eq!(m["count"], 3);
    assert_eq!(m["duration"].as_f64(), Some(540.0));
    assert_eq!(m["has_more"], false);
    assert_eq!(ids_of(&m), vec![a, b, c]);
    assert!(!m.to_string().contains("/music/"), "no file paths: {m}");
    assert!(m.get("user_id").is_none() && !m.to_string().contains("\"user_id\""), "{m}");
    let url = format!("/api/share/{tok}");

    // moved: the order follows
    let items = e.items(pid).await;
    assert_eq!(e.post(&format!("/api/playlists/{pid}/move"), Some(json!({ "item_id": items[2], "to": 0 }))).await.status, StatusCode::OK);
    assert_eq!(e.listed(&url).await, vec![c, a, b]);
    // taken out: gone from the list, and its stream with it
    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={a}")).await.status, StatusCode::OK);
    assert_eq!(e.del(&format!("/api/playlists/{pid}/items/{}", items[0])).await.status, StatusCode::OK);
    assert_eq!(e.listed(&url).await, vec![c, b]);
    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={a}")).await.status, StatusCode::NOT_FOUND);
    // added: there at once
    e.post(&format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": [d] }))).await;
    assert_eq!(e.listed(&url).await, vec![c, b, d]);

    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={b}")).await.status, StatusCode::OK);
    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={never}")).await.status, StatusCode::NOT_FOUND);
    assert_eq!(e.public(&format!("/api/share/{tok}/stream")).await.status, StatusCode::OK);
    // a song of the list whose file is gone from the library: not listed, not counted
    e.st.db.execute("DELETE FROM tracks WHERE id=?", p![d]).unwrap();
    let m = e.public(&url).await.json();
    assert_eq!((ids_of(&m), m["count"].as_u64()), (vec![c, b], Some(2)));
    // the cover: fewer than four albums, so the first album's that has one
    e.post(&format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": [a] }))).await;
    assert_eq!(e.public(&format!("/api/share/{tok}/cover")).await.status, StatusCode::OK);

    // renamed: the title follows
    assert_eq!(call(&e.app, "PATCH", &format!("/api/playlists/{pid}"), &e.cookie, Some(json!({ "name": "Night drive" }))).await.status, StatusCode::OK);
    assert_eq!(e.public(&url).await.json()["title"], "Night drive");
    let html = e.public(&format!("/s/{tok}")).await.text();
    assert_eq!(og(&html, "og:title").as_deref(), Some("Night drive · sharer"), "{html}");

    // the description: plain text (a service's HTML tags taken out), never the note an older Spotify import
    // left there for the owner
    let describe = |d: &str| e.st.db.execute("UPDATE playlists SET description=? WHERE id=?", p![d, pid]).unwrap();
    describe(r#"Fresh <a href="spotify:genre:pop">pop</a>   and <b>more</b>"#);
    assert_eq!(e.public(&url).await.json()["description"], "Fresh pop and more");
    describe("Imported without Spotify login – connect Spotify for ISRCs and >100-track playlists");
    assert_eq!(e.public(&url).await.json()["description"], "");
}

#[test]
fn a_playlist_description_is_shown_as_plain_text() {
    assert_eq!(public_description("Rock <3 and <i>roll</i>\n\nforever"), "Rock <3 and roll forever");
    assert_eq!(public_description("  "), "");
    let d = public_description(&"word ".repeat(100));
    assert!(d.ends_with('…') && d.chars().count() <= 301, "{d}");
}

#[tokio::test]
async fn liked_songs_link_lists_newest_first_and_follows_likes() {
    let mut e = setup("pl-liked").await;
    let a1 = e.add("One", "A", "First", "A", "2001");
    let a2 = e.add("Two", "A", "First", "A", "2001");
    let a3 = e.add("Three", "B", "Second", "B", "2002");
    for id in [a1, a2, a3] {
        assert_eq!(e.post(&format!("/api/likes/{id}"), None).await.status, StatusCode::OK);
    }
    let lists = e.get("/api/playlists").await.json();
    let liked = lists.as_array().unwrap().iter().find(|p| p["kind"] == "liked").unwrap()["id"].as_i64().unwrap();
    let tok = e.share_playlist(liked).await;
    let url = format!("/api/share/{tok}");
    let m = e.public(&url).await.json();
    assert_eq!((m["title"].as_str(), m["liked"].as_bool()), (Some("Liked Songs"), Some(true)));
    assert_eq!(ids_of(&m), vec![a3, a2, a1], "newest like first");
    // the page and its preview
    let html = e.public(&format!("/s/{tok}")).await.text();
    assert_eq!(og(&html, "og:title").as_deref(), Some("Liked Songs · sharer"), "{html}");
    assert_eq!(og(&html, "og:type").as_deref(), Some("music.playlist"));
    assert_eq!(og(&html, "og:description").as_deref(), Some("3 songs · Shared from Encore"));
    assert!(html.contains("<title>Liked Songs · sharer</title>"), "{html}");
    assert!(!html.contains("music:musician"), "{html}");

    // unliked: gone from the link and its stream at once; a new like comes first
    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={a2}")).await.status, StatusCode::OK);
    assert_eq!(e.del(&format!("/api/likes/{a2}")).await.status, StatusCode::OK);
    assert_eq!(e.listed(&url).await, vec![a3, a1]);
    assert_eq!(e.public(&format!("/api/share/{tok}/stream?track={a2}")).await.status, StatusCode::NOT_FOUND);
    e.post(&format!("/api/likes/{a2}"), None).await;
    assert_eq!(e.listed(&url).await, vec![a2, a3, a1]);
    // without a song asked for, the newest like plays
    assert_eq!(e.public(&format!("/api/share/{tok}/stream")).await.status, StatusCode::OK);
}

#[tokio::test]
async fn only_the_owner_shares_a_playlist() {
    let mut e = setup("pl-owner").await;
    let a = e.add("One", "A", "First", "A", "2001");
    let pid = e.playlist("Mine", &[a]).await;
    let other = e.login("other").await;
    for m in ["POST", "DELETE"] {
        let r = call(&e.app, m, &format!("/api/playlists/{pid}/share"), &other, None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{m}: {}", r.text());
        assert_eq!(r.json()["detail"], "Playlist not found");
        assert_eq!(call(&e.app, m, &format!("/api/playlists/{pid}/share"), "", None).await.status, StatusCode::UNAUTHORIZED, "{m}");
    }
    let tok = e.share_playlist(pid).await;
    // the other user's own list shows nothing of it, and their Liked Songs is not shared
    let theirs = call(&e.app, "GET", "/api/playlists", &other, None).await.json();
    assert!(theirs.as_array().unwrap().iter().all(|p| p["id"].as_i64() != Some(pid) && p["share"].is_null()), "{theirs}");
    assert!(!theirs.to_string().contains(&tok));
    assert_eq!(call(&e.app, "GET", &format!("/api/playlists/{pid}"), &other, None).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stop_sharing_ends_the_link_and_sharing_again_makes_a_new_one() {
    let mut e = setup("pl-stop").await;
    let a = e.add("One", "A", "First", "A", "2001");
    std::fs::write(e.music.join(e.album_id(a)).join("cover.jpg"), b"\xff\xd8\xff not a real jpeg").unwrap();
    let pid = e.playlist("Mine", &[a]).await;
    let other = e.login("other").await;
    // not shared yet
    let listed = |v: Value| v.as_array().unwrap().iter().find(|p| p["id"].as_i64() == Some(pid)).unwrap()["share"].clone();
    assert!(listed(e.get("/api/playlists").await.json()).is_null());
    assert!(e.get(&format!("/api/playlists/{pid}")).await.json()["share"].is_null());
    let tok = e.share_playlist(pid).await;
    let share = listed(e.get("/api/playlists").await.json());
    assert_eq!((share["token"].as_str(), share["path"].as_str()), (Some(tok.as_str()), Some(format!("/s/{tok}").as_str())));
    assert!(share["url"].as_str().unwrap().ends_with(&format!("/s/{tok}")));
    assert_eq!(e.get(&format!("/api/playlists/{pid}")).await.json()["share"], share);
    assert_eq!(e.public(&format!("/api/share/{tok}/cover")).await.status, StatusCode::OK);

    let r = e.del(&format!("/api/playlists/{pid}/share")).await;
    assert_eq!((r.status, r.json()), (StatusCode::OK, json!({ "ok": true })));
    let r = e.public(&format!("/api/share/{tok}")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["detail"], "This link doesn't work any more");
    e.st.cfg.set_many(&json!({ "share_preview": "playable" }));
    for path in ["stream", &format!("stream?track={a}"), "cover", "video.mp4"] {
        assert_eq!(e.public(&format!("/api/share/{tok}/{path}")).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(call(&e.app, "POST", &format!("/api/share/{tok}/copy"), &other, Some(json!({}))).await.status, StatusCode::NOT_FOUND);
    let page = e.public(&format!("/s/{tok}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(og(&page.text(), "og:title").is_none(), "{}", page.text());
    assert!(listed(e.get("/api/playlists").await.json()).is_null());
    assert!(e.get(&format!("/api/playlists/{pid}")).await.json()["share"].is_null());
    // again: still ok; sharing again makes a new link, and the old one stays dead
    assert_eq!(e.del(&format!("/api/playlists/{pid}/share")).await.status, StatusCode::OK);
    let tok2 = e.share_playlist(pid).await;
    assert_ne!(tok2, tok);
    assert_eq!(e.public(&format!("/api/share/{tok2}")).await.status, StatusCode::OK);
    assert_eq!(e.public(&format!("/api/share/{tok}")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_deleted_playlist_or_disabled_owner_ends_the_link() {
    let mut e = setup("pl-gone").await;
    let a = e.add("One", "A", "First", "A", "2001");
    let b = e.add("Two", "B", "Second", "B", "2002");
    let rows = |st: &AppState, pid: i64| st.db.one("SELECT COUNT(*) n FROM shares WHERE kind='playlist' AND ref=?", p![pid.to_string()]).unwrap().unwrap()["n"].as_i64().unwrap();

    let p1 = e.playlist("Deleted", &[a]).await;
    let t1 = e.share_playlist(p1).await;
    assert_eq!(e.del(&format!("/api/playlists/{p1}")).await.status, StatusCode::OK);
    assert_eq!(e.public(&format!("/api/share/{t1}")).await.status, StatusCode::NOT_FOUND);
    assert_eq!(rows(&e.st, p1), 0);

    // merged with "Delete the original playlists": both links end; the merged one is not shared
    let (p2, p3) = (e.playlist("Two", &[a]).await, e.playlist("Three", &[b]).await);
    let (t2, t3) = (e.share_playlist(p2).await, e.share_playlist(p3).await);
    let r = e.post("/api/playlists/merge", Some(json!({ "ids": [p2, p3], "name": "Both", "delete_sources": true }))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    for t in [&t2, &t3] {
        assert_eq!(e.public(&format!("/api/share/{t}")).await.status, StatusCode::NOT_FOUND);
    }
    assert_eq!((rows(&e.st, p2), rows(&e.st, p3)), (0, 0));
    assert!(e.get(&format!("/api/playlists/{}", r.json()["id"])).await.json()["share"].is_null());

    // a disabled owner: the link answers as gone until they are enabled again
    let p4 = e.playlist("Kept", &[a, b]).await;
    let t4 = e.share_playlist(p4).await;
    e.st.db.execute("UPDATE users SET disabled=1 WHERE username='sharer'", p![]).unwrap();
    let r = e.public(&format!("/api/share/{t4}")).await;
    assert_eq!((r.status, r.json()["detail"].as_str()), (StatusCode::NOT_FOUND, Some("This link doesn't work any more")));
    assert_eq!(e.public(&format!("/api/share/{t4}/stream?track={a}")).await.status, StatusCode::NOT_FOUND);
    assert!(og(&e.public(&format!("/s/{t4}")).await.text(), "og:title").is_none());
    e.st.db.execute("UPDATE users SET disabled=0 WHERE username='sharer'", p![]).unwrap();
    assert_eq!(e.public(&format!("/api/share/{t4}")).await.status, StatusCode::OK);
    // a deleted owner (their rows stay behind): gone
    e.st.db.execute("DELETE FROM users WHERE username='sharer'", p![]).unwrap();
    assert_eq!(e.public(&format!("/api/share/{t4}")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn large_playlist_links_come_in_pages() {
    let mut e = setup("pl-pages").await;
    let album = e.add("Album song", "A", "An album", "A", "2001");
    let pid = e.playlist("Long", &[]).await;
    // 450 songs straight into the database (only streamed songs need files)
    let ids: Vec<i64> = (1000..1450).collect();
    for (i, id) in ids.iter().enumerate() {
        e.st.db.execute("INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, duration, added_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            p![id, format!("/nowhere/{id}.flac"), format!("Song {id}"), "Many", format!("Album {}", id / 10), "Many", format!("{:016x}", id / 10), "flac", (i + 1) as f64, 1.0]).unwrap();
        e.st.db.execute("INSERT INTO playlist_items (playlist_id, position, title, artist, source, track_id, added_at) VALUES (?,?,?,?,?,?,?)",
            p![pid, i as i64, format!("Song {id}"), "Many", "library", id, 1.0]).unwrap();
    }
    let tok = e.share_playlist(pid).await;
    let url = format!("/api/share/{tok}");
    let m = e.public(&url).await.json();
    assert_eq!((m["count"].as_u64(), m["has_more"].as_bool(), m["offset"].as_u64(), m["limit"].as_u64()), (Some(450), Some(true), Some(0), Some(200)));
    assert_eq!(ids_of(&m), ids[..200]);
    assert_eq!(m["duration"].as_f64(), Some((1..=450).sum::<i64>() as f64), "the whole list's length");
    assert!(m["shuffle"].is_null());
    let m = e.public(&format!("{url}?offset=400")).await.json();
    assert_eq!((ids_of(&m), m["has_more"].as_bool()), (ids[400..].to_vec(), Some(false)));
    let m = e.public(&format!("{url}?limit=1000")).await.json();
    assert_eq!((m["limit"].as_u64(), ids_of(&m).len(), m["has_more"].as_bool()), (Some(500), 450, Some(false)));
    let m = e.public(&format!("{url}?limit=0&offset=-5")).await.json();
    assert_eq!((m["limit"].as_u64(), m["offset"].as_u64(), ids_of(&m)), (Some(1), Some(0), vec![ids[0]]));
    let m = e.public(&format!("{url}?offset=9999")).await.json();
    assert_eq!((ids_of(&m), m["has_more"].as_bool(), m["count"].as_u64()), (vec![], Some(false), Some(450)));

    // a shuffle: the same songs in another order, the same order for the same seed, and its pages fit together
    let shuffled = e.listed(&format!("{url}?shuffle=7&limit=500")).await;
    assert_ne!(shuffled, ids);
    let mut sorted = shuffled.clone();
    sorted.sort();
    assert_eq!(sorted, ids);
    assert_eq!(e.listed(&format!("{url}?shuffle=7&limit=500")).await, shuffled);
    assert_ne!(e.listed(&format!("{url}?shuffle=8&limit=500")).await, shuffled);
    let mut pages = vec![];
    for offset in [0, 200, 400] {
        let m = e.public(&format!("{url}?shuffle=7&offset={offset}")).await.json();
        assert_eq!(m["shuffle"].as_u64(), Some(7));
        pages.extend(ids_of(&m));
    }
    assert_eq!(pages, shuffled, "the pages of one shuffle cover every song once");

    // album and artist links don't page
    let aid = e.album_id(album);
    let atok = e.post(&format!("/api/library/albums/{aid}/share"), None).await.json()["token"].as_str().unwrap().to_string();
    let m = e.public(&format!("/api/share/{atok}?limit=1&offset=5&shuffle=3")).await.json();
    assert_eq!((ids_of(&m), m.get("has_more").is_none()), (vec![album], true));
}

#[tokio::test]
async fn save_a_copy_makes_a_playlist_for_the_viewer() {
    let mut e = setup("pl-copy").await;
    let a1 = e.add("One", "A", "First", "A", "2001");
    let a2 = e.add("Two", "A", "First", "A", "2001");
    let a3 = e.add("Three", "B", "Second", "B", "2002");
    for id in [a1, a2, a3] {
        e.post(&format!("/api/likes/{id}"), None).await;
    }
    let liked = e.get("/api/playlists").await.json().as_array().unwrap().iter().find(|p| p["kind"] == "liked").unwrap()["id"].as_i64().unwrap();
    let tok = e.share_playlist(liked).await;
    let other = e.login("other").await;
    let url = format!("/api/share/{tok}");

    // who is looking
    assert_eq!(e.public(&url).await.json()["viewer"], json!({ "signed_in": false, "own": false }));
    assert_eq!(e.get(&url).await.json()["viewer"], json!({ "signed_in": true, "own": true }));
    assert_eq!(call(&e.app, "GET", &url, &other, None).await.json()["viewer"], json!({ "signed_in": true, "own": false, "added": false, "playlist_id": null }));

    let r = call(&e.app, "POST", &format!("{url}/copy"), &other, Some(json!({}))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!((j["name"].as_str(), j["added"].as_u64()), (Some("Liked Songs · sharer"), Some(3)));
    let copy = j["id"].as_i64().unwrap();
    let row = e.st.db.one("SELECT p.*, u.username FROM playlists p JOIN users u ON u.id=p.user_id WHERE p.id=?", p![copy]).unwrap().unwrap();
    assert_eq!((row["username"].as_str(), row["kind"].as_str(), row["description"].as_str()), (Some("other"), Some("playlist"), Some("Shared by sharer")));
    let theirs = call(&e.app, "GET", &format!("/api/playlists/{copy}"), &other, None).await.json();
    let order: Vec<i64> = theirs["items"].as_array().unwrap().iter().map(|i| i["track_id"].as_i64().unwrap()).collect();
    assert_eq!(order, vec![a3, a2, a1], "in the link's order");
    assert!(theirs["share"].is_null());
    // the viewer's own Liked Songs is untouched
    let their_liked = call(&e.app, "GET", "/api/playlists", &other, None).await.json().as_array().unwrap().iter().find(|p| p["kind"] == "liked").unwrap()["total"].clone();
    assert_eq!(their_liked, 0);
    // a snapshot: a later like doesn't reach the copy
    let a4 = e.add("Four", "B", "Second", "B", "2002");
    e.post(&format!("/api/likes/{a4}"), None).await;
    assert_eq!(call(&e.app, "GET", &format!("/api/playlists/{copy}"), &other, None).await.json()["items"].as_array().unwrap().len(), 3);

    // another name, and an ordinary playlist keeps its own
    let r = call(&e.app, "POST", &format!("{url}/copy"), &other, Some(json!({ "name": "  Anna's likes " }))).await;
    assert_eq!(r.json()["name"], "Anna's likes");
    let pid = e.playlist("Road trip", &[a2, a1]).await;
    let ptok = e.share_playlist(pid).await;
    let r = call(&e.app, "POST", &format!("/api/share/{ptok}/copy"), &other, None).await;
    assert_eq!((r.status, r.json()["name"].as_str(), r.json()["added"].as_u64()), (StatusCode::OK, Some("Road trip"), Some(2)), "{}", r.text());
    // the owner may copy their own link (a plain duplicate)
    assert_eq!(e.post(&format!("/api/share/{ptok}/copy"), Some(json!({}))).await.status, StatusCode::OK);

    // signed out, no CSRF header, not a playlist, a stopped link
    assert_eq!(call(&e.app, "POST", &format!("{url}/copy"), "", Some(json!({}))).await.status, StatusCode::UNAUTHORIZED);
    let raw = Request::builder().method("POST").uri(format!("{url}/copy")).header("cookie", &other).header("content-type", "application/json").body(Body::from("{}")).unwrap();
    assert_eq!(e.app.clone().oneshot(raw).await.unwrap().status(), StatusCode::FORBIDDEN);
    let aid = e.album_id(a1);
    let atok = e.post(&format!("/api/library/albums/{aid}/share"), None).await.json()["token"].as_str().unwrap().to_string();
    let r = call(&e.app, "POST", &format!("/api/share/{atok}/copy"), &other, Some(json!({}))).await;
    assert_eq!((r.status, r.json()["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("Only playlists can be copied")));
    e.del(&format!("/api/playlists/{liked}/share")).await;
    assert_eq!(call(&e.app, "POST", &format!("{url}/copy"), &other, Some(json!({}))).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_empty_playlist_link_stays_up() {
    let e = setup("pl-empty").await;
    let pid = e.playlist("Soon", &[]).await;
    // a song that isn't in the library is not listed either
    e.st.db.execute("INSERT INTO playlist_items (playlist_id, position, title, artist, source, added_at) VALUES (?,0,'Missing','Nobody','spotify',1)", p![pid]).unwrap();
    let tok = e.share_playlist(pid).await;
    let r = e.public(&format!("/api/share/{tok}")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let m = r.json();
    assert_eq!((m["count"].as_u64(), m["tracks"].clone(), m["has_more"].as_bool(), m["format"].as_str()), (Some(0), json!([]), Some(false), Some("")));
    let s = e.public(&format!("/api/share/{tok}/stream")).await;
    assert_eq!((s.status, s.json()["detail"].as_str()), (StatusCode::NOT_FOUND, Some("Track not found")));
    let c = e.public(&format!("/api/share/{tok}/cover")).await;
    assert_eq!((c.status, c.json()["detail"].as_str()), (StatusCode::NOT_FOUND, Some("No cover")));
    let html = e.public(&format!("/s/{tok}")).await.text();
    assert_eq!(og(&html, "og:title").as_deref(), Some("Soon · sharer"), "{html}");
    assert_eq!(og(&html, "og:description").as_deref(), Some("0 songs · Shared from Encore"));
    assert!(og(&html, "og:audio").is_none() && og(&html, "og:image").is_none() && !html.contains("twitter:image"), "{html}");
}

/// A cover file of one solid colour in the album's folder.
fn solid_cover(e: &Env, track: i64, rgb: [u8; 3]) {
    image::RgbImage::from_pixel(320, 240, image::Rgb(rgb)).save(e.music.join(e.album_id(track)).join("cover.jpg")).unwrap();
}

/// The colour at the middle of each quadrant of a 600x600 picture.
fn quadrants(jpeg: &[u8]) -> Vec<[u8; 3]> {
    let img = image::load_from_memory(jpeg).unwrap().to_rgb8();
    assert_eq!((img.width(), img.height()), (600, 600));
    [(150, 150), (450, 150), (150, 450), (450, 450)].iter().map(|(x, y)| img.get_pixel(*x, *y).0).collect()
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() < 30)
}

#[tokio::test]
async fn playlist_cover_is_a_mosaic_of_four_albums() {
    let mut e = setup("pl-mosaic").await;
    let (red, green, blue, white) = ([220, 20, 20], [20, 200, 40], [30, 40, 220], [250, 250, 250]);
    let s1 = e.add("Red", "A", "Red album", "A", "2001");
    let s2 = e.add("Green", "B", "Green album", "B", "2002");
    let s2b = e.add("Green again", "B", "Green album", "B", "2002");
    let s3 = e.add("Blue", "C", "Blue album", "C", "2003");
    let s4 = e.add("White", "D", "White album", "D", "2004");
    for (t, c) in [(s1, red), (s2, green), (s3, blue), (s4, white)] {
        solid_cover(&e, t, c);
    }
    // the same album twice counts once
    let pid = e.playlist("Colours", &[s1, s2, s2b, s3, s4]).await;
    let tok = e.share_playlist(pid).await;
    let cache = e.st.db.data_dir.join("cache").join("share-cover");
    let files = || std::fs::read_dir(&cache).map(|d| d.flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>()).unwrap_or_default();

    let r = e.public(&format!("/api/share/{tok}/cover?s=200")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.h("content-type"), "image/jpeg");
    // the list changes under the same URL and Stop sharing ends it: never kept by a shared cache, and the
    // browser asks again each time (a 304 while it is the same picture)
    assert_eq!(r.h("cache-control"), "private, no-cache");
    let mosaic_tag = r.h("etag");
    assert!(!mosaic_tag.is_empty());
    let same = send(&e.app, "GET", &format!("/api/share/{tok}/cover"), "", None, &[("if-none-match", &mosaic_tag)]).await;
    assert_eq!(same.status, StatusCode::NOT_MODIFIED);
    let mosaic_date = r.h("last-modified");
    let q = quadrants(&r.body);
    for (got, want) in q.iter().zip([red, green, blue, white]) {
        assert!(near(*got, want), "{q:?}");
    }
    let first = files();
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].starts_with(&format!("{tok}.")) && first[0].ends_with(".jpg"), "{first:?}");
    // asked again: the same file
    assert_eq!(e.public(&format!("/api/share/{tok}/cover")).await.body, r.body);
    assert_eq!(files(), first);

    // three albums: the first album's own cover, as its album page serves it
    let items = e.items(pid).await;
    e.del(&format!("/api/playlists/{pid}/items/{}", items[4])).await;
    let single = e.public(&format!("/api/share/{tok}/cover")).await;
    assert_eq!(single.status, StatusCode::OK);
    assert!(!single.h("etag").is_empty());
    assert_eq!(single.h("cache-control"), "private, no-cache");
    let album_cover = e.get(&format!("/api/library/albums/{}/cover", e.album_id(s1))).await;
    assert_eq!(single.body, album_cover.body);
    // a browser that has the mosaic gets the album's cover, though that file is older than the mosaic
    let asked = send(&e.app, "GET", &format!("/api/share/{tok}/cover"), "", None, &[("if-none-match", &mosaic_tag), ("if-modified-since", &mosaic_date)]).await;
    assert_eq!(asked.status, StatusCode::OK);
    assert_eq!(asked.body, album_cover.body);
    let same = send(&e.app, "GET", &format!("/api/share/{tok}/cover"), "", None, &[("if-none-match", &single.h("etag"))]).await;
    assert_eq!(same.status, StatusCode::NOT_MODIFIED);
    // another album's cover under the same link is not the same picture
    let s1_items = e.items(pid).await;
    e.del(&format!("/api/playlists/{pid}/items/{}", s1_items[0])).await;
    let other = send(&e.app, "GET", &format!("/api/share/{tok}/cover"), "", None, &[("if-none-match", &single.h("etag")), ("if-modified-since", &single.h("last-modified"))]).await;
    assert_eq!(other.status, StatusCode::OK);
    assert_ne!(other.body, single.body);
    e.post(&format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": [s1] }))).await;
    let items = e.items(pid).await;
    e.post(&format!("/api/playlists/{pid}/move"), Some(json!({ "item_id": items[items.len() - 1], "to": 0 }))).await;

    // four again, in another order: a new picture, and this link's old one is gone
    e.post(&format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": [s4] }))).await;
    let items = e.items(pid).await;
    e.post(&format!("/api/playlists/{pid}/move"), Some(json!({ "item_id": items[1], "to": 0 }))).await;
    let q = quadrants(&e.public(&format!("/api/share/{tok}/cover")).await.body);
    for (got, want) in q.iter().zip([green, red, blue, white]) {
        assert!(near(*got, want), "{q:?}");
    }
    let now = files();
    assert_eq!(now.len(), 1, "{now:?}");
    assert_ne!(now, first);
    // the link's preview names the picture
    let html = e.public(&format!("/s/{tok}")).await.text();
    assert_eq!(og(&html, "og:image").map(|u| u.ends_with(&format!("/api/share/{tok}/cover?s=600"))), Some(true), "{html}");
    // a link that is gone loses its picture at the next build
    let other = e.playlist("Other", &[s4, s3, s2, s1]).await;
    let otok = e.share_playlist(other).await;
    e.del(&format!("/api/playlists/{pid}/share")).await;
    assert_eq!(e.public(&format!("/api/share/{otok}/cover")).await.status, StatusCode::OK);
    let left = files();
    assert!(left.len() == 1 && left[0].starts_with(&format!("{otok}.")), "{left:?}");
}

#[tokio::test]
async fn merge_into_liked_songs_skips_dupes_and_keeps_missing_songs() {
    let mut e = setup("pl-merge-into").await;
    let a = e.add("One", "A", "First", "A", "2001");
    let b = e.add("Two", "B", "Second", "B", "2002");
    assert_eq!(e.post(&format!("/api/likes/{a}"), None).await.status, StatusCode::OK);
    let liked = e.get("/api/playlists").await.json().as_array().unwrap().iter().find(|p| p["kind"] == "liked").unwrap()["id"].as_i64().unwrap();
    let src = e.playlist("Imported list", &[a, b]).await;
    let r = e.post("/api/playlists/import", Some(json!({ "text": "Nobody Here - Not In The Library", "into": src, "follow": false, "request_missing": false }))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());

    // nothing to merge, or a target that isn't the user's: refused
    let r = e.post("/api/playlists/merge", Some(json!({ "ids": [liked], "into": liked }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    let other = e.login("other").await;
    let theirs = call(&e.app, "POST", "/api/playlists", &other, Some(json!({ "name": "Theirs" }))).await.json()["id"].as_i64().unwrap();
    assert_eq!(e.post("/api/playlists/merge", Some(json!({ "ids": [src], "into": theirs }))).await.status, StatusCode::NOT_FOUND);

    let r = e.post("/api/playlists/merge", Some(json!({ "ids": [src], "into": liked, "delete_sources": true }))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!((j["id"].as_i64(), j["total"].as_i64(), j["added"].as_i64(), j["duplicates_skipped"].as_i64()), (Some(liked), Some(3), Some(2), Some(1)), "{j}");
    let d = e.get(&format!("/api/playlists/{liked}")).await.json();
    let items = d["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{d}");
    assert!(items.iter().any(|i| i["track_id"].is_null() && i["title"] == "Not In The Library"), "the missing song stays: {d}");
    // the source is gone, Liked Songs is not
    assert_eq!(e.get(&format!("/api/playlists/{src}")).await.status, StatusCode::NOT_FOUND);
    assert_eq!(e.get(&format!("/api/playlists/{liked}")).await.status, StatusCode::OK);
}

/// A preview video without a cover is the card colour of the "encore" theme: `SHARE_VIDEO_BLANK` is the
/// `--panel` of `:root[data-theme="encore"]` in the web app's stylesheet.
#[test]
fn the_blank_preview_video_is_the_encore_theme_card_colour() {
    let css = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../web-rs/static/css/app.css")).unwrap();
    let theme = &css[css.find(r#":root[data-theme="encore"] {"#).expect("encore theme in app.css")..];
    let theme = &theme[..theme.find('}').unwrap()];
    let panel = theme.split("--panel:").nth(1).expect("--panel in the encore theme").split(';').next().unwrap().trim();
    assert_eq!(panel, SHARE_VIDEO_BLANK);
}

// ---- opening a link where the listener already listens (share_open, 0.42.5) ----------------------------------

/// Chrome on an Android phone.
const ANDROID_CHROME: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36";
/// An app's built-in WebView on Android (`; wv)`), which can't open intent links.
const ANDROID_WEBVIEW: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/130.0.0.0 Mobile Safari/537.36";
/// Safari on an iPhone (there is no iOS app).
const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
/// Firefox on a desktop.
const DESKTOP: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";

impl Env {
    /// GET `uri` with this user agent, as the user whose session `cookie` is ("" for none), and extra `headers`.
    async fn page_as(&self, uri: &str, ua: &str, cookie: &str, headers: &[(&str, &str)]) -> Resp {
        let mut h = vec![("user-agent", ua)];
        h.extend_from_slice(headers);
        send(&self.app, "GET", uri, cookie, None, &h).await
    }
}

#[test]
fn the_intent_link_opens_the_app_and_falls_back_to_the_page() {
    use super::share_open::{android_intent, no_app_link};
    let i = android_intent("https://music.example/", "tok");
    assert!(i.starts_with("intent://music.example/s/tok#Intent;scheme=encore;"), "{i}");
    assert!(i.contains("S.link=https%3A%2F%2Fmusic.example%2Fs%2Ftok;"), "{i}");
    assert!(i.contains("S.browser_fallback_url=https%3A%2F%2Fmusic.example%2Fs%2Ftok%3Fapp%3D0;"), "{i}");
    assert!(i.ends_with(";end"), "{i}");
    // no package: the release and debug builds both answer it
    assert!(!i.contains("package="), "{i}");
    // a server under a path, on plain http
    let i = android_intent("http://192.168.1.5:8080/encore", "t2");
    assert!(i.starts_with("intent://192.168.1.5:8080/encore/s/t2#Intent;"), "{i}");
    assert!(i.contains("S.link=http%3A%2F%2F192.168.1.5%3A8080%2Fencore%2Fs%2Ft2;"), "{i}");
    assert_eq!(no_app_link("https://m.example/", "t"), "https://m.example/s/t?app=0");
}

#[test]
fn web_paths_for_every_kind() {
    use super::share_open::web_path;
    assert_eq!(web_path(&json!({ "kind": "track", "track_id": 7, "album_id": "ab12" })).as_deref(), Some("/library/album/ab12?track=7"));
    assert_eq!(web_path(&json!({ "kind": "album", "album_id": "ab12" })).as_deref(), Some("/library/album/ab12"));
    assert_eq!(web_path(&json!({ "kind": "artist", "name": "Simon & Garfunkel" })).as_deref(), Some("/artist-name/Simon%20%26%20Garfunkel"));
    assert_eq!(web_path(&json!({ "kind": "playlist", "playlist_id": 4 })).as_deref(), Some("/playlists/4"));
    // someone else's playlist not added: the share page offers to add it
    assert_eq!(web_path(&json!({ "kind": "playlist", "playlist_id": null })), None);
    assert_eq!(web_path(&json!({ "kind": "album", "album_id": "" })), None);
}

#[test]
fn bots_and_webviews_never_jump() {
    use super::share_open::{is_android_browser, is_preview_bot};
    assert!(is_android_browser(ANDROID_CHROME));
    assert!(is_android_browser("Mozilla/5.0 (Android 14; Mobile; rv:131.0) Gecko/131.0 Firefox/131.0"));
    assert!(!is_android_browser(ANDROID_WEBVIEW));
    assert!(!is_android_browser(IPHONE));
    assert!(!is_android_browser(DESKTOP));
    // Googlebot's phone crawler says Android and Chrome
    assert!(!is_android_browser("Mozilla/5.0 (Linux; Android 6.0.1; Nexus 5X Build/MMB29P) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)"));
    for ua in ["Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)", "Twitterbot/1.0", "facebookexternalhit/1.1", "WhatsApp/2.23.20.0 A", "TelegramBot (like TwitterBot)", "Slackbot-LinkExpanding 1.0"] {
        assert!(is_preview_bot(ua), "{ua}");
    }
    assert!(!is_preview_bot(ANDROID_CHROME));
    assert!(!is_preview_bot(DESKTOP));
}

#[tokio::test]
async fn android_browsers_go_to_the_app_and_back_with_app_0() {
    let mut e = setup("open-android").await;
    let id = e.add("Hey Mama", "Black Eyed Peas", "Elephunk", "Black Eyed Peas", "2003");
    let tok = e.post(&format!("/api/library/tracks/{id}/share"), None).await.json()["token"].as_str().unwrap().to_string();
    let page = format!("/s/{tok}");

    // not signed in, on Android Chrome: the intent link, which the browser follows to the app (or to ?app=0)
    let r = e.page_as(&page, ANDROID_CHROME, "", &[]).await;
    assert_eq!(r.status, StatusCode::FOUND, "{}", r.text());
    assert!(r.h("location").starts_with("intent://"), "{}", r.h("location"));
    assert!(r.h("location").contains(&format!("/s/{tok}#Intent;scheme=encore;")), "{}", r.h("location"));
    assert_eq!(r.h("cache-control"), "no-store");
    assert!(r.h("vary").contains("User-Agent") && r.h("vary").contains("Cookie"));
    // signed in too: the app comes first
    let r = e.page_as(&page, ANDROID_CHROME, &e.cookie, &[]).await;
    assert!(r.h("location").starts_with("intent://"), "{}", r.h("location"));

    // the fallback (no app): the share page, or the web player when signed in
    let r = e.page_as(&format!("{page}?app=0"), ANDROID_CHROME, "", &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.text().contains("og:title"));
    let r = e.page_as(&format!("{page}?app=0"), ANDROID_CHROME, &e.cookie, &[]).await;
    assert_eq!(r.status, StatusCode::FOUND);
    assert_eq!(r.h("location"), format!("/#/library/album/{}?track={id}", e.album_id(id)));

    // a WebView, a frame (X's player card), a preview bot and a request without a user agent keep the page
    assert_eq!(e.page_as(&page, ANDROID_WEBVIEW, "", &[]).await.status, StatusCode::OK);
    assert_eq!(e.page_as(&page, ANDROID_CHROME, "", &[("sec-fetch-dest", "iframe")]).await.status, StatusCode::OK);
    assert_eq!(e.page_as(&page, "Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)", "", &[]).await.status, StatusCode::OK);
    assert_eq!(e.public(&page).await.status, StatusCode::OK);
    // a link that doesn't work never jumps: the page says so
    let r = e.page_as("/s/nope", ANDROID_CHROME, &e.cookie, &[]).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn signed_in_browsers_open_the_web_player() {
    let mut e = setup("open-web").await;
    let a1 = e.add("Hey Mama", "Black Eyed Peas", "Elephunk", "Black Eyed Peas", "2003");
    let a2 = e.add("Shut Up", "Black Eyed Peas", "Elephunk", "Black Eyed Peas", "2003");
    let elephunk = e.album_id(a1);
    let song = e.post(&format!("/api/library/tracks/{a2}/share"), None).await.json()["token"].as_str().unwrap().to_string();
    let album = e.post(&format!("/api/library/albums/{elephunk}/share"), None).await.json()["token"].as_str().unwrap().to_string();
    let artist = e.post("/api/library/artists/share", Some(json!({ "name": "Black Eyed Peas" }))).await.json()["token"].as_str().unwrap().to_string();
    let pid = e.playlist("Road trip", &[a1, a2]).await;
    let list = e.share_playlist(pid).await;
    let other = e.login("listener").await;

    for (ua, cookie) in [(DESKTOP, &e.cookie), (IPHONE, &other)] {
        let loc = |r: Resp| { assert_eq!(r.status, StatusCode::FOUND, "{}", r.text()); r.h("location") };
        assert_eq!(loc(e.page_as(&format!("/s/{song}"), ua, cookie, &[]).await), format!("/#/library/album/{elephunk}?track={a2}"));
        assert_eq!(loc(e.page_as(&format!("/s/{album}"), ua, cookie, &[]).await), format!("/#/library/album/{elephunk}"));
        assert_eq!(loc(e.page_as(&format!("/s/{artist}"), ua, cookie, &[]).await), "/#/artist-name/Black%20Eyed%20Peas");
    }
    // the owner's own playlist opens; someone else's stays on the share page (Add to my playlists) until added
    assert_eq!(e.page_as(&format!("/s/{list}"), DESKTOP, &e.cookie, &[]).await.h("location"), format!("/#/playlists/{pid}"));
    assert_eq!(e.page_as(&format!("/s/{list}"), IPHONE, &other, &[]).await.status, StatusCode::OK);
    // signed out: the share page, as before
    let r = e.page_as(&format!("/s/{song}"), IPHONE, "", &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.text().contains("og:title"));
}

#[tokio::test]
async fn the_app_asks_where_a_link_opens() {
    let mut e = setup("open-api").await;
    let a1 = e.add("Hey Mama", "Black Eyed Peas", "Elephunk", "Black Eyed Peas", "2003");
    let song = e.post(&format!("/api/library/tracks/{a1}/share"), None).await.json()["token"].as_str().unwrap().to_string();
    let pid = e.playlist("Road trip", &[a1]).await;
    let list = e.share_playlist(pid).await;
    let other = e.login("listener").await;

    // only for someone signed in
    assert_eq!(e.public(&format!("/api/share/{song}/open")).await.status, StatusCode::UNAUTHORIZED);
    let j = e.get(&format!("/api/share/{song}/open")).await.json();
    assert_eq!(j, json!({ "kind": "track", "track_id": a1, "album_id": e.album_id(a1) }));
    let j = e.get(&format!("/api/share/{list}/open")).await.json();
    assert_eq!(j, json!({ "kind": "playlist", "own": true, "added": false, "playlist_id": pid, "name": "Road trip" }));
    let j = call(&e.app, "GET", &format!("/api/share/{list}/open"), &other, None).await.json();
    assert_eq!(j, json!({ "kind": "playlist", "own": false, "added": false, "playlist_id": null, "name": "Road trip" }));
    // a link that ended
    assert_eq!(e.get("/api/share/nope/open").await.status, StatusCode::NOT_FOUND);
}
