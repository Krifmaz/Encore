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

/// Tidal's search caps the query at 256 characters and refuses an empty one.
pub const SEARCH_QUERY_MAX: usize = 256;

/// A search query as Tidal takes it: trimmed, at most [`SEARCH_QUERY_MAX`] characters; None when nothing
/// is left (Tidal answers an empty `filter[query]` with 400, so no call is made).
pub fn search_query(q: &str) -> Option<String> {
    let q: String = q.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(SEARCH_QUERY_MAX).collect();
    let q = q.trim().to_string();
    (!q.is_empty()).then_some(q)
}

/// Tidal's catalogue search (openapi v2 since mid-2026): `GET /searchResults?filter[query]=…`. The query
/// goes in `filter[query]`, form-encoded by reqwest, whatever characters it has ("AC/DC", "#1", "100%",
/// "?", "&", accents). The older path form (`/searchResults/{query}`) is read by Tidal as a resource id
/// and answers every query with 400 INVALID_RESOURCE_ID (pointer data/id).
pub fn search(st: &impl Ctx, token: &str, query: &str, country: &str, include: &str) -> RequestBuilder {
    get(st, token, "/searchResults").query(&[("filter[query]", query), ("countryCode", country), ("include", include)])
}

/// The ids of `typ` ("albums", "tracks") in a search answer, in Tidal's order. `data` is an array of
/// search-result resources now; the older answer's single object is still read.
pub fn search_ids(d: &Value, typ: &str) -> Vec<String> {
    let res: Vec<&Value> = match &d["data"] {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![&d["data"]],
        _ => vec![],
    };
    res.iter().flat_map(|r| r["relationships"][typ]["data"].as_array().cloned().unwrap_or_default())
        .filter(|x| x["type"].as_str() == Some(typ)).filter_map(|x| x["id"].as_str().map(String::from)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_ids_read_the_array_answer_in_tidals_order() {
        let rel = |typ: &str, ids: &[&str]| json!({ "data": ids.iter().map(|i| json!({ "id": i, "type": typ })).collect::<Vec<_>>() });
        // the current answer: data is an array of search-result resources
        let now = json!({ "data": [{ "id": "Duumu Illuminate EP", "type": "searchResults", "relationships": {
            "albums": rel("albums", &["a3", "a1", "a2"]), "tracks": rel("tracks", &["t1"]) } }] });
        assert_eq!(search_ids(&now, "albums"), ["a3", "a1", "a2"]);
        assert_eq!(search_ids(&now, "tracks"), ["t1"]);
        // the older answer: one object at data
        let old = json!({ "data": { "id": "q", "type": "searchResults", "relationships": { "albums": rel("albums", &["x1", "x2"]) } } });
        assert_eq!(search_ids(&old, "albums"), ["x1", "x2"]);
        // nothing found, or another type listed under the name
        assert!(search_ids(&json!({ "data": [] }), "albums").is_empty());
        assert!(search_ids(&json!({}), "albums").is_empty());
        let mixed = json!({ "data": [{ "relationships": { "albums": { "data": [{ "id": "p1", "type": "playlists" }, { "id": "a9", "type": "albums" }] } } }] });
        assert_eq!(search_ids(&mixed, "albums"), ["a9"]);
    }

    #[test]
    fn a_search_query_is_trimmed_capped_and_never_empty() {
        assert_eq!(search_query("  Duumu   Illuminate EP "), Some("Duumu Illuminate EP".into()));
        assert_eq!(search_query("   "), None);
        assert_eq!(search_query(""), None);
        let long = "é".repeat(300);
        assert_eq!(search_query(&long).unwrap().chars().count(), SEARCH_QUERY_MAX);
        assert_eq!(search_query("AC/DC #1 Ke$ha 100% Sigur Rós? a&b"), Some("AC/DC #1 Ke$ha 100% Sigur Rós? a&b".into()));
    }
}
