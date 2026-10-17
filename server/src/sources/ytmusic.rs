//! YouTube Music search through its web API (what ytmusicapi does): official songs and albums for downloads.
use crate::matcher::{compare, describe};
use crate::state::AppState;
use serde_json::{json, Value};

const API: &str = "https://music.youtube.com/youtubei/v1";

/// The web API's base. `ENCORE_YTMUSIC_API` points it at a stand-in (the tests' local mock); never set it in use.
fn api() -> String {
    std::env::var("ENCORE_YTMUSIC_API").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| API.into())
}
const SONGS: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";
const ALBUMS: &str = "EgWKAQIYAWoMEA4QChADEAQQCRAF";

async fn call(st: &AppState, endpoint: &str, mut body: Value) -> anyhow::Result<Value> {
    body["context"] = json!({ "client": { "clientName": "WEB_REMIX", "clientVersion": "1.20250101.01.00", "hl": "en", "gl": "US" } });
    let r = st.http.post(format!("{}/{endpoint}?prettyPrint=false", api())).header("Origin", "https://music.youtube.com")
        .header("Referer", "https://music.youtube.com/").json(&body).send().await?;
    Ok(r.error_for_status()?.json().await?)
}

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
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let mut o = vec![];
    walk(v, key, &mut o);
    o.into_iter().next()
}

fn secs(s: &str) -> Option<f64> {
    let p: Vec<&str> = s.split(':').collect();
    if p.len() < 2 || p.iter().any(|x| x.is_empty() || !x.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some(p.iter().fold(0.0, |a, x| a * 60.0 + x.parse::<f64>().unwrap_or(0.0)))
}

/// One search row → {title, artist, album, album_id, duration, videoId, browseId, cover}
fn item(r: &Value) -> Value {
    let cols = r["flexColumns"].as_array().cloned().unwrap_or_default();
    let runs = |i: usize| cols.get(i).and_then(|c| c["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"].as_array().cloned()).unwrap_or_default();
    let title: String = runs(0).iter().filter_map(|x| x["text"].as_str()).collect();
    let (mut artists, mut album, mut album_id, mut dur) = (vec![], Value::Null, Value::Null, Value::Null);
    let meta = runs(1);
    for x in &meta {
        let t = x["text"].as_str().unwrap_or("");
        let page = x["navigationEndpoint"]["browseEndpoint"]["browseEndpointContextSupportedConfigs"]["browseEndpointContextMusicConfig"]["pageType"].as_str().unwrap_or("");
        if page == "MUSIC_PAGE_TYPE_ARTIST" || page == "MUSIC_PAGE_TYPE_USER_CHANNEL" {
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
            "browseId": r["navigationEndpoint"]["browseEndpoint"]["browseId"], "cover": cover })
}

pub async fn search(st: &AppState, q: &str, songs: bool) -> anyhow::Result<Vec<Value>> {
    let d = call(st, "search", json!({ "query": q, "params": if songs { SONGS } else { ALBUMS } })).await?;
    let mut rows = vec![];
    walk(&d, "musicResponsiveListItemRenderer", &mut rows);
    Ok(rows.into_iter().map(item).collect())
}

/// Best official YouTube Music song (not a video/cover/remix unless asked for).
pub async fn find_song(st: &AppState, artist: &str, title: &str, length: Option<f64>) -> Option<Value> {
    let want = describe(title, artist, "", length, None, None);
    let mut best: Option<(f64, Value)> = None;
    for x in search(st, &format!("{artist} {title}"), true).await.ok()?.into_iter().take(10) {
        if x["videoId"].is_null() {
            continue;
        }
        let v = compare(&want, &describe(x["title"].as_str().unwrap_or(""), x["artist"].as_str().unwrap_or(""), "", x["duration"].as_f64(), None, None), true);
        if v.matched && best.as_ref().is_none_or(|(s, _)| v.score > *s) {
            best = Some((v.score, x));
        }
    }
    best.map(|(_, x)| x)
}

/// Album page → (title, audio playlist id, track count)
pub async fn album(st: &AppState, browse_id: &str) -> anyhow::Result<(String, Option<String>, usize)> {
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

/// → {browseId, title, playlist_url, tracks} for the matching album.
pub async fn find_album(st: &AppState, artist: &str, album_title: &str, track_count: Option<usize>) -> Option<Value> {
    let want = describe(album_title, artist, "", None, None, None);
    for x in search(st, &format!("{artist} {album_title}"), false).await.ok()?.into_iter().take(10) {
        let Some(bid) = x["browseId"].as_str().filter(|b| b.starts_with("MPRE")) else { continue };
        if !compare(&want, &describe(x["title"].as_str().unwrap_or(""), x["artist"].as_str().unwrap_or(""), "", None, None, None), true).matched {
            continue;
        }
        let Ok((title, pl, n)) = album(st, bid).await else { continue };
        if let Some(tc) = track_count.filter(|t| *t > 0) {
            if (n as i64 - tc as i64).unsigned_abs() as usize > (tc / 5).max(1) {
                continue;
            }
        }
        return Some(json!({ "browseId": bid, "title": if title.is_empty() { x["title"].clone() } else { json!(title) }, "tracks": n,
            "playlist_url": pl.map(|p| format!("https://music.youtube.com/playlist?list={p}")) }));
    }
    None
}

// ---- signed-in calls (a user's YouTube Music connection) -----------------------------------------

/// Call YouTube Music as the user: pasted browser cookies (SAPISIDHASH) or a Google sign-in token.
pub async fn authed(st: &AppState, data: &Value, endpoint: &str, mut body: Value) -> anyhow::Result<Value> {
    body["context"] = json!({ "client": { "clientName": "WEB_REMIX", "clientVersion": "1.20250101.01.00", "hl": "en", "gl": "US" } });
    let mut rb = st.http.post(format!("{}/{endpoint}?prettyPrint=false", api())).header("Origin", "https://music.youtube.com")
        .header("X-Origin", "https://music.youtube.com").header("Referer", "https://music.youtube.com/").json(&body);
    if data["mode"] == "browser" {
        let h = &data["headers"];
        let cookie = h["cookie"].as_str().unwrap_or("");
        let sapisid = cookie.split(';').map(|c| c.trim()).find_map(|c| c.strip_prefix("__Secure-3PAPISID=").or(c.strip_prefix("SAPISID="))).unwrap_or("");
        let ts = crate::db::now() as i64;
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

/// Google sign-in tokens expire hourly: refresh (and store) before calls.
pub async fn fresh_data(st: &AppState, conn: &crate::db::Row) -> anyhow::Result<Value> {
    use crate::db::RowExt;
    let mut d = conn["data"].clone();
    if d["mode"] != "oauth" || d["token"]["expires_at"].as_f64().unwrap_or(0.0) > crate::db::now() + 60.0 {
        return Ok(d);
    }
    let rt = d["token"]["refresh_token"].as_str().unwrap_or("").to_string();
    let t: Value = st.http.post("https://oauth2.googleapis.com/token").form(&[("client_id", st.cfg.str("ytmusic_client_id")), ("client_secret", st.cfg.str("ytmusic_client_secret")),
        ("refresh_token", rt), ("grant_type", "refresh_token".into())]).send().await?.json().await?;
    let Some(access) = t["access_token"].as_str() else { anyhow::bail!("YouTube Music sign-in expired – reconnect") };
    d["token"]["access_token"] = json!(access);
    d["token"]["expires_at"] = json!(crate::db::now() as i64 + t["expires_in"].as_i64().unwrap_or(3600));
    crate::connections::update_data(st, conn.i("id"), &d);
    Ok(d)
}

fn continuation(v: &Value) -> Option<String> {
    let mut c = vec![];
    walk(v, "continuation", &mut c);
    c.into_iter().find_map(|x| x.as_str().map(String::from))
        .or_else(|| { let mut t = vec![]; walk(v, "token", &mut t); t.into_iter().find_map(|x| x.as_str().map(String::from)) })
}

/// All rows of a signed-in browse page (liked songs, a playlist …), following continuations.
pub async fn browse_rows(st: &AppState, data: &Value, browse_id: &str, max_pages: usize) -> anyhow::Result<Vec<Value>> {
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

pub async fn library_playlists(st: &AppState, data: &Value) -> anyhow::Result<Vec<Value>> {
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

pub async fn rate(st: &AppState, data: &Value, video: &str, like: bool) -> anyhow::Result<()> {
    authed(st, data, if like { "like/like" } else { "like/removelike" }, json!({ "target": { "videoId": video } })).await.map(|_| ())
}

pub async fn create_playlist(st: &AppState, data: &Value, name: &str, videos: &[String]) -> anyhow::Result<Option<String>> {
    let d = authed(st, data, "playlist/create", json!({ "title": name, "description": "Synced by Encore", "privacyStatus": "PRIVATE", "videoIds": videos })).await?;
    Ok(d["playlistId"].as_str().map(String::from))
}

pub async fn edit_playlist(st: &AppState, data: &Value, id: &str, actions: Vec<Value>) -> anyhow::Result<()> {
    if actions.is_empty() {
        return Ok(());
    }
    authed(st, data, "browse/edit_playlist", json!({ "playlistId": id, "actions": actions })).await.map(|_| ())
}
