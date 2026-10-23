//! LRCLIB (free, no key): song lyrics, time-synced or plain.
use crate::Ctx;
use regex::Regex;
use reqwest::RequestBuilder;
use serde_json::Value;
use std::sync::LazyLock;

/// The LRCLIB lyrics API.
pub const API: &str = "https://lrclib.net/api";

/// A title without "(feat. …)", "[with …]" or "(… remaster)", the way LRCLIB files songs.
fn clean(s: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*[\(\[](feat|ft|with|remaster)[^\)\]]*[\)\]]").unwrap());
    RE.replace_all(s, "").trim().to_string()
}

/// LRCLIB's free-text search (words of the lyrics, a title): a list of songs.
pub fn search(st: &impl Ctx, q: &str) -> RequestBuilder {
    st.http().get(format!("{API}/search")).query(&[("q", q)])
}

/// LRCLIB's lyrics for a song: the exact match (artist, title, album, length) first, else the best search
/// hit within 3 s of the length (time-synced before plain).
pub async fn lookup(st: &impl Ctx, artist: &str, title: &str, album: &str, duration: Option<f64>) -> anyhow::Result<Option<Value>> {
    let mut q = vec![("artist_name", artist.to_string()), ("track_name", clean(title))];
    if !album.is_empty() {
        q.push(("album_name", album.into()));
    }
    if let Some(d) = duration.filter(|d| *d > 0.0) {
        q.push(("duration", (d.round() as i64).to_string()));
    }
    let r = st.http().get(format!("{API}/get")).query(&q).send().await?;
    if r.status().is_success() {
        let d: Value = r.json().await?;
        if d["syncedLyrics"].is_string() || d["plainLyrics"].is_string() || d["instrumental"].as_bool().unwrap_or(false) {
            return Ok(Some(d));
        }
    }
    let r = st.http().get(format!("{API}/search")).query(&[("artist_name", artist.to_string()), ("track_name", clean(title))]).send().await?;
    let mut hits: Vec<Value> = if r.status().is_success() { r.json::<Value>().await?.as_array().cloned().unwrap_or_default() } else { vec![] };
    if let Some(d) = duration.filter(|d| *d > 0.0) {
        hits.retain(|h| h["duration"].as_f64().is_none_or(|x| (x - d).abs() <= 3.0));
    }
    hits.sort_by_key(|h| (h["syncedLyrics"].is_null(), h["plainLyrics"].is_null()));
    Ok(hits.into_iter().next())
}
