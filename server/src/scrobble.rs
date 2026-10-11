//! Scrobbling: what you play in Encore goes to your Last.fm and ListenBrainz accounts
//! ("now playing" when a song starts, a scrobble once you've heard half of it or 4 minutes).
use crate::connections;
use crate::db::{Row, RowExt};
use crate::state::AppState;
use serde_json::{json, Value};

const LASTFM: &str = "https://ws.audioscrobbler.com/2.0/";

/// Last.fm request signature: every parameter (except format) sorted by name, then the shared secret, md5.
pub fn lastfm_sig(params: &[(&str, String)], secret: &str) -> String {
    let mut p: Vec<&(&str, String)> = params.iter().filter(|(k, _)| *k != "format" && *k != "callback").collect();
    p.sort_by_key(|(k, _)| *k);
    let s: String = p.iter().map(|(k, v)| format!("{k}{v}")).collect::<String>() + secret;
    format!("{:x}", md5::compute(s.as_bytes()))
}

/// A signed POST to Last.fm (write calls: scrobbles, now playing, sessions).
pub async fn lastfm_signed(st: &AppState, method: &str, mut params: Vec<(&str, String)>) -> anyhow::Result<Value> {
    let (key, secret) = (st.cfg.str("lastfm_api_key"), st.cfg.str("lastfm_shared_secret"));
    if key.is_empty() || secret.is_empty() {
        anyhow::bail!("The admin needs to add the Last.fm API key and shared secret in Settings → Apps & sources → Integrations");
    }
    params.push(("method", method.into()));
    params.push(("api_key", key));
    let sig = lastfm_sig(&params, &secret);
    params.push(("api_sig", sig));
    params.push(("format", "json".into()));
    let d: Value = st.http.post(LASTFM).form(&params).send().await?.json().await?;
    if !d["error"].is_null() {
        anyhow::bail!("Last.fm: {}", d["message"].as_str().unwrap_or("error"));
    }
    Ok(d)
}

fn wants(c: &Row) -> Option<Value> {
    let d: Value = serde_json::from_str(&c.s("data")).unwrap_or(json!({}));
    (d["scrobble"] != json!(false)).then_some(d)
}

async fn send(st: &AppState, uid: i64, t: &Row, listened_at: Option<i64>) {
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
            let mut info = json!({ "media_player": "Encore", "submission_client": "Encore", "submission_client_version": crate::VERSION });
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
            let r = st.http.post("https://api.listenbrainz.org/1/submit-listens").header("Authorization", format!("Token {token}")).json(&body).send().await;
            if let Err(e) = r.and_then(|r| r.error_for_status()) {
                tracing::warn!("ListenBrainz submit failed: {e}");
            }
        }
    }
}

fn track(st: &AppState, id: i64) -> Option<Row> {
    st.db.one("SELECT artist, title, album, albumartist, duration, mb_trackid, mb_albumid FROM tracks WHERE id=?", crate::p![id]).ok().flatten()
}

/// A song started playing.
pub fn now_playing(st: &AppState, uid: i64, track_id: i64) {
    let st = st.clone();
    tokio::spawn(async move {
        if let Some(t) = track(&st, track_id) {
            send(&st, uid, &t, None).await;
        }
    });
}

/// A song was heard long enough to count; `started` is when it began (unix seconds).
pub fn scrobble(st: &AppState, uid: i64, track_id: i64, started: i64) {
    let st = st.clone();
    tokio::spawn(async move {
        if let Some(t) = track(&st, track_id) {
            send(&st, uid, &t, Some(started)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn signature_matches_lastfm_docs_shape() {
        // api_keyXmethodauth.getSessiontokenY + secret, md5
        let s = super::lastfm_sig(&[("method", "auth.getSession".into()), ("api_key", "X".into()), ("token", "Y".into()), ("format", "json".into())], "S");
        assert_eq!(s, format!("{:x}", md5::compute(b"api_keyXmethodauth.getSessiontokenYS")));
    }
}
