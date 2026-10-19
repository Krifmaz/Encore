//! Finding the official YouTube Music song or album for a request (the web API client is encore_http::ytmusic).
use crate::matcher::{compare, describe};
use encore_http::ytmusic::{album, search};
use encore_http::Ctx;
use serde_json::{json, Value};

/// Best official YouTube Music song (not a video/cover/remix unless asked for).
pub async fn find_song(st: &impl Ctx, artist: &str, title: &str, length: Option<f64>) -> Option<Value> {
    let want = describe(title, artist, length, None, None);
    let mut best: Option<(f64, Value)> = None;
    for x in search(st, &format!("{artist} {title}"), true).await.ok()?.into_iter().take(10) {
        if x["videoId"].is_null() {
            continue;
        }
        let v = compare(&want, &describe(x["title"].as_str().unwrap_or(""), x["artist"].as_str().unwrap_or(""), x["duration"].as_f64(), None, None), true);
        if v.matched && best.as_ref().is_none_or(|(s, _)| v.score > *s) {
            best = Some((v.score, x));
        }
    }
    best.map(|(_, x)| x)
}

/// → {browseId, title, playlist_url, tracks} for the matching album.
pub async fn find_album(st: &impl Ctx, artist: &str, album_title: &str, track_count: Option<usize>) -> Option<Value> {
    let want = describe(album_title, artist, None, None, None);
    for x in search(st, &format!("{artist} {album_title}"), false).await.ok()?.into_iter().take(10) {
        let Some(bid) = x["browseId"].as_str().filter(|b| b.starts_with("MPRE")) else { continue };
        if !compare(&want, &describe(x["title"].as_str().unwrap_or(""), x["artist"].as_str().unwrap_or(""), None, None, None), true).matched {
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
