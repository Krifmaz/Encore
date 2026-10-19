//! Scrobbling: what you play in Encore goes to your Last.fm and ListenBrainz accounts
//! ("now playing" when a song starts, a scrobble once you've heard half of it or 4 minutes).
use crate::connections;
use encore_db::{Row, RowExt};
use crate::Ctx;
use serde_json::{json, Value};

use encore_http::lastfm::signed as lastfm_signed;

fn wants(c: &Row) -> Option<Value> {
    let d: Value = serde_json::from_str(&c.s("data")).unwrap_or(json!({}));
    (d["scrobble"] != json!(false)).then_some(d)
}

async fn send(st: &impl Ctx, uid: i64, t: &Row, listened_at: Option<i64>) {
    let (artist, title, album) = (t.s("artist"), t.s("title"), t.s("album"));
    if artist.is_empty() || title.is_empty() {
        return;
    }
    let dur = t.f("duration").round() as i64;
    if let Some(d) = connections::get(st, uid, "lastfm").as_ref().and_then(wants) {
        if let Some(sk) = d["sk"].as_str().filter(|s| !s.is_empty()) {
            let mut p: Vec<(&str, String)> = vec![("artist", artist.clone()), ("track", title.clone()), ("sk", sk.into())];
            if !album.is_empty() {
                p.push(("album", album.clone()));
            }
            if !t.s("albumartist").is_empty() {
                p.push(("albumArtist", t.s("albumartist")));
            }
            if dur > 0 {
                p.push(("duration", dur.to_string()));
            }
            if !t.s("mb_trackid").is_empty() {
                p.push(("mbid", t.s("mb_trackid")));
            }
            let method = match listened_at {
                Some(ts) => { p.push(("timestamp", ts.to_string())); "track.scrobble" }
                None => "track.updateNowPlaying",
            };
            if let Err(e) = lastfm_signed(st, method, p).await {
                tracing::warn!("Last.fm {method} failed: {e}");
            }
        }
    }
    if let Some(d) = connections::get(st, uid, "listenbrainz").as_ref().and_then(wants) {
        if let Some(token) = d["token"].as_str().filter(|s| !s.is_empty()) {
            let mut info = json!({ "media_player": "Encore", "submission_client": "Encore", "submission_client_version": encore_core::VERSION });
            if dur > 0 {
                info["duration_ms"] = json!(dur * 1000);
            }
            if !t.s("mb_trackid").is_empty() {
                info["recording_mbid"] = json!(t.s("mb_trackid"));
            }
            if !t.s("mb_albumid").is_empty() {
                info["release_mbid"] = json!(t.s("mb_albumid"));
            }
            let mut listen = json!({ "track_metadata": { "artist_name": artist, "track_name": title, "additional_info": info } });
            if !album.is_empty() {
                listen["track_metadata"]["release_name"] = json!(album);
            }
            if let Some(ts) = listened_at {
                listen["listened_at"] = json!(ts);
            }
            let body = json!({ "listen_type": if listened_at.is_some() { "single" } else { "playing_now" }, "payload": [listen] });
            let r = encore_http::listenbrainz::submit_listens(st, token, &body).send().await;
            if let Err(e) = r.and_then(|r| r.error_for_status()) {
                tracing::warn!("ListenBrainz submit failed: {e}");
            }
        }
    }
}

fn track(st: &impl Ctx, id: i64) -> Option<Row> {
    st.db().one("SELECT artist, title, album, albumartist, duration, mb_trackid, mb_albumid FROM tracks WHERE id=?", encore_db::p![id]).ok().flatten()
}

/// A song started playing.
pub fn now_playing(st: &impl Ctx, uid: i64, track_id: i64) {
    let st = st.clone();
    tokio::spawn(async move {
        if let Some(t) = track(&st, track_id) {
            send(&st, uid, &t, None).await;
        }
    });
}

/// A song was heard long enough to count; `started` is when it began (unix seconds).
pub fn scrobble(st: &impl Ctx, uid: i64, track_id: i64, started: i64) {
    let st = st.clone();
    tokio::spawn(async move {
        if let Some(t) = track(&st, track_id) {
            send(&st, uid, &t, Some(started)).await;
        }
    });
}
