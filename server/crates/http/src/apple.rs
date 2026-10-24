//! Apple's iTunes Search API (Apple Music's catalog, no account): search and lookup.
//!
//! [`search`] and [`lookup`] hand out the requests; the caller sets its own timeout and reads the answer its
//! way, or sends it paced through [`get`].
use crate::Ctx;
use reqwest::RequestBuilder;
use serde_json::Value;
use std::time::Duration;

/// The iTunes Search API.
pub const API: &str = "https://itunes.apple.com";

/// The iTunes Search API allows about 20 calls a minute: paced calls are spaced by this much.
const GAP: Duration = Duration::from_secs(3);
/// How long to wait after Apple says "too many requests" before the one retry.
const BACKOFF: Duration = Duration::from_secs(60);

/// The API's address: [`API`], or the stand-in a test points `Urls::apple` at.
fn base(st: &impl Ctx) -> &str {
    st.urls().apple.trim_end_matches('/')
}

/// Search the catalog for `term` as `entity` (song, album, musicArtist).
pub fn search(st: &impl Ctx, term: &str, entity: &str, limit: u32) -> RequestBuilder {
    st.http().get(format!("{}/search", base(st))).query(&[("term", term.to_string()), ("entity", entity.to_string()), ("limit", limit.to_string())])
}

/// Look up an artist or album by id with what belongs to it as `entity` (album, song); Apple's own limit
/// when `limit` is None.
pub fn lookup(st: &impl Ctx, id: &str, entity: &str, limit: Option<u32>) -> RequestBuilder {
    let mut q = vec![("id", id.to_string()), ("entity", entity.to_string())];
    if let Some(n) = limit {
        q.push(("limit", n.to_string()));
    }
    st.http().get(format!("{}/lookup", base(st))).query(&q)
}

/// Send `req` paced with every other paced caller (20 s at most) and read it as JSON. A throttled answer
/// (403/429) holds every paced iTunes call back for a while, then this one is tried once more.
pub async fn get(st: &impl Ctx, req: RequestBuilder) -> anyhow::Result<Value> {
    let mut retried = false;
    loop {
        crate::take_slot(&st.pace().itunes, GAP).await;
        let r = req.try_clone().ok_or_else(|| anyhow::anyhow!("iTunes request can't be sent again"))?;
        let r = r.timeout(Duration::from_secs(20)).send().await?;
        let code = r.status().as_u16();
        if (code == 403 || code == 429) && !retried {
            retried = true;
            *st.pace().itunes.lock().await = Some(tokio::time::Instant::now() + BACKOFF);
            continue;
        }
        return Ok(r.error_for_status()?.json().await?);
    }
}
