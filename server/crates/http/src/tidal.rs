//! Tidal's API (openapi v2): catalogue lookups (tracks by ISRC or id, albums, playlists) and a listener's
//! collection, with a developer app's token or a signed-in account's. Its address is the server's
//! [`crate::Urls::tidal`] (a test points it at a stand-in).
use crate::Ctx;
use reqwest::{RequestBuilder, Response};
use serde_json::Value;

pub const API: &str = "https://openapi.tidal.com/v2";

/// A GET on the API with `token`: `path` is a path of the API (`/tracks`, `/albums/1/relationships/items`)
/// or a whole address (a `next` link of an earlier answer).
pub fn get(st: &impl Ctx, token: &str, path: &str) -> RequestBuilder {
    st.http().get(url(st, path)).bearer_auth(token).header("Accept", "application/vnd.api+json")
}

/// The address of `path`: a path of the API, or a link of an answer as it is (a `next` link may also come
/// as `/v2/…`, which is a path of the API too).
pub fn url(st: &impl Ctx, path: &str) -> String {
    if path.starts_with("http") {
        return path.to_string();
    }
    let base = st.urls().tidal.trim_end_matches('/');
    let p = if base.ends_with("/v2") { path.strip_prefix("/v2").filter(|p| p.starts_with('/')).unwrap_or(path) } else { path };
    format!("{base}{p}")
}

/// An answer as JSON; an error status is an error with Tidal's words ("Tidal API 404: …").
pub async fn read(r: Response) -> anyhow::Result<Value> {
    let code = r.status().as_u16();
    if code >= 400 {
        anyhow::bail!("Tidal API {code}: {}", r.text().await.unwrap_or_default().chars().take(200).collect::<String>());
    }
    Ok(r.json().await?)
}
