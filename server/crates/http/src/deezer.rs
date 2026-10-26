//! Deezer's public API (no account): search, albums, artists, tracks by id or ISRC, public profiles.
//!
//! The functions below hand out the request for each part of the API; the caller sets its own timeout and
//! reads the answer its way. [`get`] sends one paced: spaced out with every other paced call and tried
//! again when Deezer says it is over its quota.
use crate::Ctx;
use reqwest::RequestBuilder;
use serde_json::Value;
use std::fmt::Display;
use std::time::Duration;

/// Deezer's public API.
pub const API: &str = "https://api.deezer.com";

/// At most ~8 Deezer requests a second from this server.
const GAP: Duration = Duration::from_millis(125);

/// A GET on one path of the API (`album/302127`, `user/5/tracks`).
pub fn path(st: &impl Ctx, path: &str) -> RequestBuilder {
    st.http().get(format!("{}/{path}", st.urls().deezer.trim_end_matches('/')))
}

/// `search` (`kind` "") or `search/<kind>` (album, artist, track) for `q`.
pub fn search(st: &impl Ctx, kind: &str, q: &str, limit: u32) -> RequestBuilder {
    let p = if kind.is_empty() { "search".to_string() } else { format!("search/{kind}") };
    path(st, &p).query(&[("q", q.to_string()), ("limit", limit.to_string())])
}

/// An album with its first tracks.
pub fn album(st: &impl Ctx, id: impl Display) -> RequestBuilder {
    path(st, &format!("album/{id}"))
}

/// An album's tracks.
pub fn album_tracks(st: &impl Ctx, id: impl Display, limit: u32) -> RequestBuilder {
    path(st, &format!("album/{id}/tracks")).query(&[("limit", limit.to_string())])
}

/// An artist's releases.
pub fn artist_albums(st: &impl Ctx, id: impl Display, limit: u32) -> RequestBuilder {
    path(st, &format!("artist/{id}/albums")).query(&[("limit", limit.to_string())])
}

/// Artists like this one.
pub fn artist_related(st: &impl Ctx, id: impl Display, limit: u32) -> RequestBuilder {
    path(st, &format!("artist/{id}/related")).query(&[("limit", limit.to_string())])
}

/// Songs in the style of this artist (Deezer's artist radio).
pub fn artist_radio(st: &impl Ctx, id: impl Display, limit: u32) -> RequestBuilder {
    path(st, &format!("artist/{id}/radio")).query(&[("limit", limit.to_string())])
}

/// An artist's most played songs.
pub fn artist_top(st: &impl Ctx, id: impl Display, limit: u32) -> RequestBuilder {
    path(st, &format!("artist/{id}/top")).query(&[("limit", limit.to_string())])
}

/// One track (its ISRC, position and album).
pub fn track(st: &impl Ctx, id: impl Display) -> RequestBuilder {
    path(st, &format!("track/{id}"))
}

/// The track with this ISRC.
pub fn track_by_isrc(st: &impl Ctx, isrc: &str) -> RequestBuilder {
    path(st, &format!("track/isrc:{isrc}"))
}

/// A public profile.
pub fn user(st: &impl Ctx, id: impl Display) -> RequestBuilder {
    path(st, &format!("user/{id}"))
}

/// A profile's public playlists.
pub fn user_playlists(st: &impl Ctx, id: impl Display, limit: u32) -> RequestBuilder {
    path(st, &format!("user/{id}/playlists")).query(&[("limit", limit.to_string())])
}

/// Send `req` paced and read it as JSON. Deezer allows ~50 requests per 5 s and answers "Quota limit
/// exceeded" (HTTP 200) beyond that: requests are spaced out, and on that answer this waits and tries again.
pub async fn get(st: &impl Ctx, req: RequestBuilder) -> anyhow::Result<Value> {
    get_found(st, req).await?.ok_or_else(|| anyhow::anyhow!("Deezer: no data"))
}

/// Deezer's error code for "no such item" (`DataException`, "no data"): an answer, not a failure.
pub const NO_DATA: i64 = 800;

/// [`get`] that tells "Deezer has no such item" apart from a failure: `Ok(None)` for Deezer's
/// [`NO_DATA`] answer (an ISRC it doesn't know), `Err` for anything else that went wrong (the network, an
/// HTTP error, a quota that stayed exceeded after the retries, another error answer), so that a caller
/// never reports a failed lookup as "not on Deezer".
pub async fn get_found(st: &impl Ctx, req: RequestBuilder) -> anyhow::Result<Option<Value>> {
    for attempt in 0..4 {
        crate::take_slot(&st.pace().deezer, GAP).await;
        let r = req.try_clone().ok_or_else(|| anyhow::anyhow!("Deezer request can't be sent again"))?;
        let d: Value = r.send().await?.error_for_status()?.json().await?;
        match d["error"]["code"].as_i64() {
            None => return Ok(Some(d)),
            Some(NO_DATA) => return Ok(None),
            Some(4) | Some(700) if attempt < 3 => tokio::time::sleep(Duration::from_millis(1500 * (attempt + 1))).await,
            Some(_) => anyhow::bail!("Deezer: {}", d["error"]["message"].as_str().unwrap_or("error")),
        }
    }
    anyhow::bail!("Deezer is busy – try again in a moment")
}
