//! GET /api/library/index: the sections of a library list for the side scrubber, `[{label, offset}]` in
//! list order. `offset` is where the section's first item sits in exactly the ordering (and filters) the
//! list endpoint uses, so a client can jump to it with the list's own paging.
//!
//! - `kind=albums` (all albums, like the phone), `kind=singles`, or `kind=albums&album_kind=albums`
//!   (the web's Albums tab: `kind=albums` on `/albums`); `filter=` and `artist=` as `/albums` takes them
//! - `kind=artists`, `kind=tracks`
//! - `sort=` / `dir=`: the list's sort keys
//!
//! Labels: name/title/artist sorts → "#" (digits and symbols) and A–Z (accents folded, by the first
//! character the list sorts on); release date → years ("?" unknown); date added → months ("Oct 2026"),
//! and a month that holds a big part of the list (a library added in one scan) split by day, hour or
//! minute ("14 Oct 2026, 15:42", with `short: "15:42"` for the rail); length / size / counts → ranges.
//! A label that already had a section (an accented name sorts after "z") stays in the section it falls in.
//! A filtered list (`q=`) is ranked, not sorted: no sections.
//!
//! Each section: `{label, offset}`, plus `short` when the rail shows a shorter text than the bubble.
use crate::auth::User;
use encore_core::error::ApiResult;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
// the section builders live in encore_library; the tests below reach them through here
use encore_library::sections::{sections_for, Spec};
#[cfg(test)]
use encore_library::sections::{label, letters, By};

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct IndexQ {
    kind: String,
    sort: String,
    dir: String,
    artist: String,
    filter: String,
    album_kind: String,
    q: String,
}

pub async fn index(State(st): State<AppState>, User(_): User, Query(a): Query<IndexQ>) -> ApiResult<Json<Value>> {
    if !a.q.trim().is_empty() {
        return Ok(Json(json!([])));
    }
    // the same sections until the songs change (see super::cached_list); built on the blocking pool
    st.blocking(move |st| {
        let key = format!("index\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}", a.kind, a.sort, a.dir, a.artist, a.filter, a.album_kind);
        let spec = Spec { kind: &a.kind, sort: &a.sort, dir: &a.dir, artist: &a.artist, filter: &a.filter, album_kind: &a.album_kind };
        let v = super::cached_list(st, &key, || sections_for(&st.db, &st.library.artists, &spec))?;
        Ok(Json((*v).clone()))
    }).await
}

#[cfg(test)]
#[path = "library_index_tests.rs"]
mod tests;
