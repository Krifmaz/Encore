//! YouTube Music through its web API (what ytmusicapi does): searching songs and albums, album pages, and
//! calls as a signed-in user (their likes and playlists). The matching searches (the best official song or
//! album for a request) are in encore_library::ytmusic.
use crate::Ctx;
use serde_json::{json, Value};

/// The web API's base (in use; the server's [`crate::Urls`] carry the one its calls go to).
pub const API: &str = "https://music.youtube.com/youtubei/v1";
/// Search filter: songs only.
const SONGS: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";
/// Search filter: albums only.
const ALBUMS: &str = "EgWKAQIYAWoMEA4QChADEAQQCRAF";

/// POST `body` to a web API `endpoint` as the web client does (signed out).
async fn call(st: &impl Ctx, endpoint: &str, mut body: Value) -> anyhow::Result<Value> {
    body["context"] = json!({ "client": { "clientName": "WEB_REMIX", "clientVersion": "1.20250101.01.00", "hl": "en", "gl": "US" } });
    let r = st.http().post(format!("{}/{endpoint}?prettyPrint=false", st.urls().ytmusic)).header("Origin", "https://music.youtube.com")
        .header("Referer", "https://music.youtube.com/").json(&body).send().await?;
    Ok(r.error_for_status()?.json().await?)
}

/// Every value under `key` anywhere in `v` (a match is not searched further).
fn walk<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == key {
                    out.push(x);
                } else {
                    walk(x, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| walk(x, key, out)),
        _ => {}
    }
}
/// The first value under `key` anywhere in `v`.
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let mut o = vec![];
    walk(v, key, &mut o);
    o.into_iter().next()
}

/// `m:ss` or `h:mm:ss` as seconds.
fn secs(s: &str) -> Option<f64> {
    let p: Vec<&str> = s.split(':').collect();
    if p.len() < 2 || p.iter().any(|x| x.is_empty() || !x.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some(p.iter().fold(0.0, |a, x| a * 60.0 + x.parse::<f64>().unwrap_or(0.0)))
}

/// One search row → {title, artist, album, album_id, duration, videoId, browseId, cover, artist_is_channel}.
/// `artist_is_channel`: the artist is a user channel (an uploaded video, not a song), so possibly not
/// the song's artist at all (encore_library::video_title reads the real one from the title).
fn item(r: &Value) -> Value {
    let cols = r["flexColumns"].as_array().cloned().unwrap_or_default();
    let runs = |i: usize| cols.get(i).and_then(|c| c["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"].as_array().cloned()).unwrap_or_default();
    let title: String = runs(0).iter().filter_map(|x| x["text"].as_str()).collect();
    let (mut artists, mut album, mut album_id, mut dur) = (vec![], Value::Null, Value::Null, Value::Null);
    // only user channels named the artist (no artist page among them)
    let (mut channels, mut artist_pages) = (0, 0);
    let meta = runs(1);
    for x in &meta {
        let t = x["text"].as_str().unwrap_or("");
        let page = x["navigationEndpoint"]["browseEndpoint"]["browseEndpointContextSupportedConfigs"]["browseEndpointContextMusicConfig"]["pageType"].as_str().unwrap_or("");
        if page == "MUSIC_PAGE_TYPE_ARTIST" || page == "MUSIC_PAGE_TYPE_USER_CHANNEL" {
            if page == "MUSIC_PAGE_TYPE_ARTIST" { artist_pages += 1 } else { channels += 1 }
            artists.push(t.to_string());
        } else if page == "MUSIC_PAGE_TYPE_ALBUM" {
            album = json!(t);
            album_id = x["navigationEndpoint"]["browseEndpoint"]["browseId"].clone();
        } else if let Some(s) = secs(t) {
            dur = json!(s);
        }
    }
    if artists.is_empty() {
        // unlinked artists: the first chunk before " • " after the type label
        let text: String = meta.iter().filter_map(|x| x["text"].as_str()).collect();
        let parts: Vec<&str> = text.split(" • ").collect();
        if let Some(a) = parts.get(if parts.len() > 2 { 1 } else { 0 }) {
            artists.push(a.to_string());
        }
    }
    let video = r["playlistItemData"]["videoId"].as_str().map(String::from).or_else(|| find(r, "videoId").and_then(|v| v.as_str()).map(String::from));
    let cover = r["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]["thumbnails"].as_array().and_then(|t| t.last()).map(|t| t["url"].clone()).unwrap_or(Value::Null);
    json!({ "title": title, "artist": artists.join(", "), "album": album, "album_id": album_id, "duration": dur, "videoId": video,
            "browseId": r["navigationEndpoint"]["browseEndpoint"]["browseId"], "cover": cover,
            "artist_is_channel": channels > 0 && artist_pages == 0 })
}

/// Search YouTube Music for songs (or albums): one row per result (see [`item`]).
pub async fn search(st: &impl Ctx, q: &str, songs: bool) -> anyhow::Result<Vec<Value>> {
    let d = call(st, "search", json!({ "query": q, "params": if songs { SONGS } else { ALBUMS } })).await?;
    let mut rows = vec![];
    walk(&d, "musicResponsiveListItemRenderer", &mut rows);
    Ok(rows.into_iter().map(item).collect())
}

/// Album page → (title, audio playlist id, track count)
pub async fn album(st: &impl Ctx, browse_id: &str) -> anyhow::Result<(String, Option<String>, usize)> {
    let d = call(st, "browse", json!({ "browseId": browse_id })).await?;
    let mut pls = vec![];
    walk(&d, "playlistId", &mut pls);
    let pl = pls.iter().filter_map(|p| p.as_str()).find(|p| p.starts_with("OLAK5uy_")).map(String::from);
    let mut rows = vec![];
    walk(&d, "musicResponsiveListItemRenderer", &mut rows);
    let n = rows.iter().filter(|r| !r["playlistItemData"]["videoId"].is_null()).count();
    let mut titles = vec![];
    walk(&d, "title", &mut titles);
    let title = titles.iter().find_map(|t| t["runs"][0]["text"].as_str()).unwrap_or("").to_string();
    Ok((title, pl, n))
}

// ---- signed-in calls (a user's YouTube Music connection) -----------------------------------------

/// Call YouTube Music as the user: pasted browser cookies (SAPISIDHASH) or a Google sign-in token.
pub async fn authed(st: &impl Ctx, data: &Value, endpoint: &str, mut body: Value) -> anyhow::Result<Value> {
    body["context"] = json!({ "client": { "clientName": "WEB_REMIX", "clientVersion": "1.20250101.01.00", "hl": "en", "gl": "US" } });
    let mut rb = st.http().post(format!("{}/{endpoint}?prettyPrint=false", st.urls().ytmusic)).header("Origin", "https://music.youtube.com")
        .header("X-Origin", "https://music.youtube.com").header("Referer", "https://music.youtube.com/").json(&body);
    if data["mode"] == "browser" {
        let h = &data["headers"];
        let cookie = h["cookie"].as_str().unwrap_or("");
        let sapisid = cookie.split(';').map(|c| c.trim()).find_map(|c| c.strip_prefix("__Secure-3PAPISID=").or(c.strip_prefix("SAPISID="))).unwrap_or("");
        let ts = encore_db::now() as i64;
        use sha1::{Digest, Sha1};
        let hash = hex::encode(Sha1::digest(format!("{ts} {sapisid} https://music.youtube.com").as_bytes()));
        rb = rb.header("Cookie", cookie).header("Authorization", format!("SAPISIDHASH {ts}_{hash}"))
            .header("X-Goog-AuthUser", h["x-goog-authuser"].as_str().unwrap_or("0"));
    } else {
        let tok = data["token"]["access_token"].as_str().unwrap_or("");
        rb = rb.bearer_auth(tok);
    }
    let r = rb.send().await?;
    if !r.status().is_success() {
        anyhow::bail!("YouTube Music answered {}", r.status().as_u16());
    }
    Ok(r.json().await?)
}

/// The next page's continuation token in an answer, if any.
fn continuation(v: &Value) -> Option<String> {
    let mut c = vec![];
    walk(v, "continuation", &mut c);
    c.into_iter().find_map(|x| x.as_str().map(String::from))
        .or_else(|| { let mut t = vec![]; walk(v, "token", &mut t); t.into_iter().find_map(|x| x.as_str().map(String::from)) })
}

/// All rows of a signed-in browse page (liked songs, a playlist …), following continuations.
pub async fn browse_rows(st: &impl Ctx, data: &Value, browse_id: &str, max_pages: usize) -> anyhow::Result<Vec<Value>> {
    let mut d = authed(st, data, "browse", json!({ "browseId": browse_id })).await?;
    let mut out = vec![];
    for _ in 0..max_pages {
        let mut rows = vec![];
        walk(&d, "musicResponsiveListItemRenderer", &mut rows);
        let n = rows.len();
        out.extend(rows.into_iter().map(|r| {
            let mut i = item(r);
            i["setVideoId"] = r["playlistItemData"]["playlistSetVideoId"].clone();
            i
        }));
        let Some(tok) = continuation(&d).filter(|_| n > 0) else { break };
        d = authed(st, data, "browse", json!({ "continuation": tok })).await?;
    }
    Ok(out)
}

/// The signed-in account's playlists.
pub async fn library_playlists(st: &impl Ctx, data: &Value) -> anyhow::Result<Vec<Value>> {
    let d = authed(st, data, "browse", json!({ "browseId": "FEmusic_liked_playlists" })).await?;
    let mut items = vec![];
    walk(&d, "musicTwoRowItemRenderer", &mut items);
    Ok(items.iter().filter_map(|x| {
        let bid = x["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str()?;
        let id = bid.strip_prefix("VL")?;
        if id == "LM" || id == "SE" {
            return None;
        }
        Some(json!({ "id": id, "name": x["title"]["runs"][0]["text"], "url": format!("https://music.youtube.com/playlist?list={id}") }))
    }).collect())
}

/// Like a video (or take the like away).
pub async fn rate(st: &impl Ctx, data: &Value, video: &str, like: bool) -> anyhow::Result<()> {
    authed(st, data, if like { "like/like" } else { "like/removelike" }, json!({ "target": { "videoId": video } })).await.map(|_| ())
}

/// Create a private playlist of `videos`; its id.
pub async fn create_playlist(st: &impl Ctx, data: &Value, name: &str, videos: &[String]) -> anyhow::Result<Option<String>> {
    let d = authed(st, data, "playlist/create", json!({ "title": name, "description": "Synced by Encore", "privacyStatus": "PRIVATE", "videoIds": videos })).await?;
    Ok(d["playlistId"].as_str().map(String::from))
}

/// Apply `actions` (add or remove videos) to playlist `id`.
pub async fn edit_playlist(st: &impl Ctx, data: &Value, id: &str, actions: Vec<Value>) -> anyhow::Result<()> {
    if actions.is_empty() {
        return Ok(());
    }
    authed(st, data, "browse/edit_playlist", json!({ "playlistId": id, "actions": actions })).await.map(|_| ())
}
