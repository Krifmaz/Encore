//! Someone else's shared playlist in my playlists: "Add to my playlists" on a playlist link (the share page,
//! a pasted link, a link shared into the phone app) makes a live, read-only entry among the signed-in user's
//! playlists. It is a `playlists` row of theirs with `kind = 'linked'` and no items of its own: every read
//! takes the owner's playlist through the link's token, as the share page does, so it always shows the
//! playlist as it is now (Liked Songs newest first). Stop sharing, a deleted playlist, or a disabled or deleted
//! owner ends it: the entry stays, with the last name and owner it showed and no songs, until it is removed.
//! Sharing again makes a new link, which does not bring it back; adding the new link takes the old entry
//! over. The owner sees how many people added it (`share.added_by`), never who.
//!
//! The row: `source 'share'`, `source_url` = the link's token (never sent to clients), `follow 0`,
//! `sync_targets '[]'`, `remote_ids` = `{"linked": {owner, owner_id, playlist, liked}}` ([`LinkSnapshot`]).
//! The words follow / followed are not used here: `follow` already means "pull new tracks from the source".
use super::library::{self, SharedPlaylist};
use crate::auth::User;
use crate::enrich::enrich_now;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use encore_core::error::{err, ApiResult};
use encore_db::{now, p, Row, RowExt};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::LazyLock;

/// The `kind` of an entry added from someone else's link.
pub const LINKED: &str = "linked";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/playlists/shared", post(add_shared))
        .route("/api/playlists/shared/check", post(check_shared))
        .route("/api/playlists/{pid}/copy", post(copy_linked))
        .route("/api/share/{token}/add", post(share_add))
}

// ---- reading a pasted link ---------------------------------------------------------------------------

/// A share link's path anywhere in the text: `/s/<token>` (the page) or `/api/share/<token>` (its data,
/// cover, stream and video).
static IN_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(?:api/share|s)/([A-Za-z0-9_-]{4,64})(?:[^A-Za-z0-9_-]|$)").unwrap());
/// A bare token.
static BARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{8,64}$").unwrap());

/// The share token in text as a user pasted or shared it: a link the server or an app made (with or without
/// the scheme, the public URL's path, a query, a fragment, a full stop after it, `<…>` or quotes around it,
/// the phone's "name\nlink"), one of the page's API addresses, or a bare token. The host is not checked: a
/// token of another server is simply not found. None when there is no token.
pub(crate) fn share_token_of(text: &str) -> Option<String> {
    let mut t = text.trim();
    for (open, close) in [('<', '>'), ('"', '"'), ('\'', '\'')] {
        if t.len() >= 2 && t.starts_with(open) && t.ends_with(close) {
            t = t[1..t.len() - 1].trim();
            break;
        }
    }
    if let Some(c) = IN_LINK.captures(t) {
        return Some(c[1].to_string());
    }
    BARE.is_match(t).then(|| t.to_string())
}

// ---- the entry's row ---------------------------------------------------------------------------------

/// What an entry knew of the owner's playlist when it was added (or last seen while the link worked): the
/// ended entry shows it, and the refusals name the owner. `owner_id` and `playlist` never reach clients.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LinkSnapshot {
    pub owner: String,
    pub owner_id: i64,
    pub playlist: i64,
    pub liked: bool,
}

impl LinkSnapshot {
    /// The snapshot in a linked row's `remote_ids`.
    pub fn of(row: &Row) -> Option<Self> {
        let v: Value = serde_json::from_str(&row.s("remote_ids")).ok()?;
        let l = v.get("linked")?;
        Some(LinkSnapshot {
            owner: l["owner"].as_str().unwrap_or("").to_string(),
            owner_id: l["owner_id"].as_i64()?,
            playlist: l["playlist"].as_i64()?,
            liked: l["liked"].as_bool().unwrap_or(false),
        })
    }

    pub fn to_json(&self) -> String {
        json!({ "linked": { "owner": self.owner, "owner_id": self.owner_id, "playlist": self.playlist, "liked": self.liked } }).to_string()
    }

    fn of_live(owner_id: i64, pl: &SharedPlaylist) -> Self {
        LinkSnapshot { owner: pl.owner.clone(), owner_id, playlist: pl.id, liked: pl.liked }
    }
}

pub(super) fn is_linked(pl: &Row) -> bool {
    pl.s("kind") == LINKED
}

/// Refuses any change to someone else's playlist added from a link: only its owner changes it (403).
pub(super) fn read_only(pl: &Row) -> ApiResult<()> {
    if !is_linked(pl) {
        return Ok(());
    }
    Err(match LinkSnapshot::of(pl).map(|s| s.owner).filter(|o| !o.is_empty()) {
        Some(owner) => err(403, format!("Only {owner} can change this playlist")),
        None => err(403, "Only its owner can change this playlist"),
    })
}

/// The name an entry (and a saved copy) gets: the owner's name for the playlist, "Liked Songs · <owner>" for
/// Liked Songs.
pub(super) fn link_playlist_name(pl: &SharedPlaylist) -> String {
    if pl.liked { format!("Liked Songs · {}", pl.owner) } else { pl.name.clone() }
}

/// The owner's playlist an entry shows, when its link still works: the token is there, still shares that
/// playlist of that owner, and the playlist and its owner are still there (see `library::shared_playlist`).
/// None when the entry has ended.
fn resolve(st: &AppState, row: &Row) -> Option<SharedPlaylist> {
    let snap = LinkSnapshot::of(row)?;
    let s = library::share_row(st, &row.s("source_url")).ok()?;
    if s.kind != "playlist" || s.r != snap.playlist.to_string() || s.user_id != snap.owner_id {
        return None;
    }
    library::shared_playlist(st, &s).ok()
}

/// How many songs a playlist lists now (those in the library, a song listed twice counted twice).
fn listed(st: &AppState, pid: i64) -> ApiResult<i64> {
    Ok(st.db.one("SELECT COUNT(*) n FROM playlist_items i JOIN tracks t ON t.id = i.track_id WHERE i.playlist_id = ?", p![pid])?.map_or(0, |r| r.i("n")))
}

/// An entry as the lists show it, and the owner's playlist when its link still works. While it works, a new
/// name or owner name is written back to the row (without moving `updated_at`, so the list keeps its order),
/// so an entry that ends later keeps the last name its user saw.
fn view(st: &AppState, row: &Row) -> ApiResult<(Value, Option<SharedPlaylist>)> {
    let snap = LinkSnapshot::of(row);
    let live = resolve(st, row);
    let mut v = Value::Object(row.clone());
    v["source_url"] = Value::Null;
    v["remote_ids"] = json!("{}");
    v["follow"] = json!(0);
    v["sync_targets"] = json!([]);
    v["share"] = Value::Null;
    v["read_only"] = json!(true);
    match &live {
        Some(pl) => {
            let name = link_playlist_name(pl);
            let n = listed(st, pl.id)?;
            let owner_id = snap.as_ref().map_or(0, |s| s.owner_id);
            if name != row.s("name") || snap.as_ref().is_none_or(|s| s.owner != pl.owner) {
                st.db.execute("UPDATE playlists SET name=?, remote_ids=? WHERE id=? AND kind='linked'",
                    p![name, LinkSnapshot::of_live(owner_id, pl).to_json(), row.i("id")])?;
            }
            v["name"] = json!(name);
            v["description"] = json!(pl.description);
            v["total"] = json!(n);
            v["matched"] = json!(n);
            v["cover_url"] = if n > 0 { json!(format!("/api/share/{}/cover", row.s("source_url"))) } else { Value::Null };
            v["linked"] = json!({ "owner": pl.owner, "active": true, "liked": pl.liked });
        }
        None => {
            v["description"] = json!("");
            v["total"] = json!(0);
            v["matched"] = json!(0);
            v["cover_url"] = Value::Null;
            v["linked"] = json!({ "owner": snap.as_ref().map(|s| s.owner.clone()).unwrap_or_default(), "active": false, "liked": snap.is_some_and(|s| s.liked) });
        }
    }
    Ok((v, live))
}

/// An entry in `GET /api/playlists`.
pub(super) fn linked_summary(st: &AppState, row: &Row) -> ApiResult<Value> {
    Ok(view(st, row)?.0)
}

/// `GET /api/playlists/{pid}` of an entry: the owner's listed songs in the link's order (every one, as an own
/// playlist's detail), numbered 0, 1, 2 … The titles are the library's; nothing of the owner's imports or
/// requests is in them. An ended entry has none.
pub(super) fn linked_detail(st: &AppState, u: &Row, row: &Row) -> ApiResult<Value> {
    let (mut v, live) = view(st, row)?;
    v["remote_ids"] = json!({});
    let mut items = match &live {
        Some(pl) => st.db.query(&format!(
            "SELECT i.id, i.position, i.track_id, t.title, t.artist, t.album, t.albumartist, t.album_id, t.duration, t.format, t.bitrate, t.verify_status, t.track, t.disc, t.year
             FROM playlist_items i JOIN tracks t ON t.id = i.track_id WHERE i.playlist_id = ? ORDER BY {}", library::playlist_order(pl)), p![pl.id])?,
        None => vec![],
    };
    for (n, it) in items.iter_mut().enumerate() {
        it.insert("position".into(), json!(n));
    }
    v["items"] = json!(items);
    enrich_now(st, Some(u), &mut v);
    Ok(v)
}

/// How many people have an entry for this link: accounts that exist and are not disabled. Names are never
/// shown.
pub(super) fn added_by(st: &AppState, token: &str) -> ApiResult<i64> {
    Ok(st.db.one("SELECT COUNT(DISTINCT p.user_id) n FROM playlists p JOIN users u ON u.id = p.user_id AND u.disabled = 0
                  WHERE p.kind = 'linked' AND p.source_url = ?", p![token])?.map_or(0, |r| r.i("n")))
}

/// [`added_by`] of every playlist link of the user's, in one query: token → count (a missing token is 0).
pub(super) fn added_by_all(st: &AppState, uid: i64) -> ApiResult<HashMap<String, i64>> {
    Ok(st.db.query("SELECT p.source_url AS token, COUNT(DISTINCT p.user_id) AS n
                    FROM playlists p JOIN users u ON u.id = p.user_id AND u.disabled = 0
                    WHERE p.kind = 'linked' AND p.source_url IN (SELECT token FROM shares WHERE user_id = ? AND kind = 'playlist')
                    GROUP BY p.source_url", p![uid])?
        .into_iter().map(|r| (r.s("token"), r.i("n"))).collect())
}

/// The user's entry for this link's token, when they have one.
pub(super) fn entry_for(st: &AppState, uid: i64, token: &str) -> ApiResult<Option<i64>> {
    Ok(st.db.one("SELECT id FROM playlists WHERE user_id=? AND kind='linked' AND source_url=?", p![uid, token])?.map(|r| r.i("id")))
}

// ---- adding ------------------------------------------------------------------------------------------

/// The owner's playlist a working playlist link shares: 404 when the link is gone (or its playlist, or its
/// owner), 400 for a song, album or artist link.
fn live_link(st: &AppState, token: &str) -> ApiResult<(library::Shared, SharedPlaylist)> {
    let s = library::share_row(st, token)?;
    if s.kind != "playlist" {
        return Err(err(400, "Only playlists can be added"));
    }
    let pl = library::shared_playlist(st, &s)?;
    Ok((s, pl))
}

/// Add to my playlists: `u` gets an entry for the playlist this token shares. Adding the same link again
/// changes nothing (`added: false`, the same entry). An ended entry of an earlier link of the same playlist
/// is taken over by the new link (the same id). The owner can't add their own. → `{id, name, added}`
pub(super) fn add_link(st: &AppState, u: &Row, token: &str) -> ApiResult<Value> {
    let (s, pl) = live_link(st, token)?;
    let uid = u.i("id");
    if s.user_id == uid {
        return Err(err(400, "This is your own playlist"));
    }
    let name = link_playlist_name(&pl);
    let snap = LinkSnapshot::of_live(s.user_id, &pl);
    let (id, added) = st.db.write_now(|c| c.transaction(|c| {
        if let Some(r) = c.one("SELECT id FROM playlists WHERE user_id=? AND kind='linked' AND source_url=?", p![uid, token])? {
            return Ok((r.i("id"), false));
        }
        let t = now();
        let earlier = c.query("SELECT id, remote_ids FROM playlists WHERE user_id=? AND kind='linked'", p![uid])?.into_iter()
            .find(|r| LinkSnapshot::of(r).is_some_and(|x| x.playlist == snap.playlist && x.owner_id == snap.owner_id));
        if let Some(r) = earlier {
            c.execute("UPDATE playlists SET source_url=?, name=?, remote_ids=?, updated_at=? WHERE id=?", p![token, name, snap.to_json(), t, r.i("id")])?;
            return Ok((r.i("id"), true));
        }
        let id = c.execute("INSERT INTO playlists (user_id, name, description, kind, source, source_url, follow, sync_targets, cover_url, created_at, updated_at, remote_ids)
                            VALUES (?, ?, '', 'linked', 'share', ?, 0, '[]', NULL, ?, ?, ?)", p![uid, name, token, t, t, snap.to_json()])?;
        Ok::<_, rusqlite::Error>((id, true))
    }))?;
    if added {
        // the owner sees how many people added it, never who: an admin who shares reads this log too
        st.db.log("info", &format!("Someone added {}'s playlist '{name}' to their playlists", pl.owner));
    }
    Ok(json!({ "id": id, "name": name, "added": added }))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LinkBody {
    /// the text as the user pasted or shared it
    link: String,
}

fn token_in(body: &LinkBody) -> ApiResult<String> {
    share_token_of(&body.link).ok_or_else(|| err(400, "That isn't a share link"))
}

/// `POST /api/playlists/shared {link}`: Add a shared playlist from a pasted link (every app).
async fn add_shared(State(st): State<AppState>, User(u): User, Json(b): Json<LinkBody>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| add_link(st, &u, &token_in(&b)?).map(Json)).await
}

/// `POST /api/share/{token}/add`: the share page's Add to my playlists.
async fn share_add(State(st): State<AppState>, User(u): User, Path(token): Path<String>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| add_link(st, &u, &token).map(Json)).await
}

/// `POST /api/playlists/shared/check {link}`: what a link would add, without adding it (the phone asks before
/// it offers Add to my playlists for a link shared into the app). `own` with the owner's playlist id for the
/// owner; `added` and `id` give the user's entry when they have one.
async fn check_shared(State(st): State<AppState>, User(u): User, Json(b): Json<LinkBody>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let token = token_in(&b)?;
        let (s, pl) = live_link(st, &token)?;
        let own = s.user_id == u.i("id");
        let entry = if own { None } else { entry_for(st, u.i("id"), &token)? };
        Ok(Json(json!({
            "name": link_playlist_name(&pl), "title": pl.name, "owner": pl.owner, "liked": pl.liked, "count": listed(st, pl.id)?,
            "own": own, "added": entry.is_some(), "id": if own { Some(pl.id) } else { entry },
        })))
    }).await
}

/// `POST /api/playlists/{pid}/copy`: Save a copy of an entry, as the share page's Save a copy does.
async fn copy_linked(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let row = super::playlists::own(st, pid, &u)?;
        if !is_linked(&row) {
            return Err(err(400, "Only playlists added from a link can be copied here"));
        }
        let pl = resolve(st, &row).ok_or_else(library::link_gone)?;
        library::copy_link(st, &u, &pl, None).map(Json)
    }).await
}

#[cfg(test)]
#[path = "playlist_links_tests.rs"]
mod playlist_links_tests;
