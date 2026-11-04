//! Search: reading the query and ranking (Discover and the library) live in encore_library::search; here
//! the Discover answer gains library ids and request states, and the library search gets its route.
use encore_core::json::s;
use encore_library::search::*;

use crate::auth::User;
use encore_core::error::ApiResult;
use crate::routes::catalog;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// Discover search: ranked (encore_library::search::discover_ranked, cached), then annotated with the
/// library and requests.
pub async fn discover(st: &AppState, q: &str, alt: Option<&str>, t: &str) -> Result<Value, String> {
    let out = discover_ranked(st, q, alt, t).await?;
    // the library and the requests, on the blocking pool
    st.blocking(move |st| {
        let mut out = out;
        annotate(st, &mut out);
        Ok(out)
    }).await.map_err(|e| e.detail)
}

/// Library ids and request statuses on every hit, and the Top result's item.
pub fn annotate(st: &AppState, out: &mut Value) {
    use crate::routes::discover::{annotate_albums, req_status};
    let idx = st.library.artists.index(&st.db);
    if let Some(arts) = out["artists"].as_array_mut() {
        for a in arts.iter_mut() {
            let lib = idx.find(a["name"].as_str().unwrap_or(""));
            a["in_library"] = json!(lib.is_some());
            a["library_name"] = lib.map(|l| json!(l.name)).unwrap_or(Value::Null);
        }
    }
    if let Some(albums) = out["albums"].as_array_mut() {
        // MusicBrainz release groups and other catalogues' albums, each kind annotated in one go
        let (plain, ext): (Vec<usize>, Vec<usize>) = (0..albums.len()).partition(|i| albums[*i]["ext"].is_null());
        for (idx, plain) in [(plain, true), (ext, false)] {
            let mut part: Vec<Value> = idx.iter().map(|i| std::mem::take(&mut albums[*i])).collect();
            if plain {
                annotate_albums(st, &mut part);
            } else {
                catalog::annotate(st, &mut part);
            }
            for (i, a) in idx.into_iter().zip(part) {
                albums[i] = a;
            }
        }
    }
    if let Some(tracks) = out["tracks"].as_array_mut() {
        use encore_library::lookup;
        let ids: Vec<String> = tracks.iter().filter_map(|t| t["id"].as_str().map(String::from)).collect();
        let rs = req_status(st, &ids);
        let keys: Vec<(String, String, String)> = tracks.iter()
            .map(|t| (t["id"].as_str().unwrap_or("").to_string(), t["artist"].as_str().unwrap_or("").to_string(), t["title"].as_str().unwrap_or("").to_string())).collect();
        let libs = lookup::find_tracks(&st.db, &keys);
        // hits without a recording id: their request by artist and title
        let pairs: Vec<(String, String)> = tracks.iter().filter(|t| t["id"].as_str().unwrap_or("").is_empty()).map(|t| (s(&t["artist"]), s(&t["title"]))).collect();
        let by_name = lookup::Requests::read(&st.db, "track", &pairs, &[]);
        for (t, lib) in tracks.iter_mut().zip(libs) {
            let id = t["id"].as_str().unwrap_or("").to_string();
            t["in_library"] = json!(lib.is_some());
            t["library"] = json!(lib);
            t["request_status"] = if id.is_empty() {
                by_name.by_name(&s(&t["artist"]), &s(&t["title"]), true, false).map(|r| json!(r.0)).unwrap_or(Value::Null)
            } else {
                rs.get(&id).map(|x| json!(x)).unwrap_or(Value::Null)
            };
        }
    }
    let kind = out["top"]["kind"].as_str().unwrap_or("").to_string();
    let list = match kind.as_str() { "song" => "tracks", "album" => "albums", "artist" => "artists", _ => "" };
    let item = if list.is_empty() { None } else { out[list].as_array().and_then(|a| a.first()).cloned() };
    out["top"] = match item { Some(it) => json!({ "kind": kind, "item": it }), None => Value::Null };
}

/// `GET /api/library/search` query.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LibSearchQ {
    /// the search text
    q: String,
    /// how many per kind
    limit: Option<usize>,
}

/// GET /api/library/search?q=&limit=: {top: {kind, item}, songs, albums, artists, counts} over the library
/// (the Library page's results view, the phone's Search tab, Discover's "In your library"); `limit` per
/// section, 1–50 (default 30).
pub async fn library_search_route(State(st): State<AppState>, User(u): User, Query(a): Query<LibSearchQ>) -> ApiResult<Json<Value>> {
    let q = a.q.trim().to_string();
    let n = a.limit.unwrap_or(30).clamp(1, SEARCH_MAX);
    st.blocking(move |st| {
        let mut v = library_search(st, &q, n)?;
        crate::enrich::enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

