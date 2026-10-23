//! Collaborative playlists: the owner turns Collaborative on (`PATCH /api/playlists/{pid} {collaborative}`)
//! and invites other accounts on this server by user name, or hands out an invite link. Whoever joins sees
//! the playlist among their own and can add, remove and reorder its songs; only the owner renames, shares,
//! syncs, follows or deletes it. Turning Collaborative off pauses it: members and invites stay but see
//! nothing until it is on again, and the invite link stops working. A disabled or deleted owner ends it.
//!
//! Rows: `playlists.collaborative` (0/1), `playlists.invite_token` (the link's token, NULL for none) and
//! `playlist_collaborators (playlist_id, user_id, status 'invited' | 'joined', invited_by, …)`; the owner
//! is never in it. Only kind `playlist` can be collaborative (not Liked Songs, not someone else's playlist
//! added from a share link). docs/contracts/collaborative-playlists.md.
use crate::auth::User;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use encore_core::error::{err, ApiResult};
use encore_db::{now, p, Row, RowExt};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::LazyLock;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/playlists/{pid}/collaborators", get(members).post(invite))
        .route("/api/playlists/{pid}/collaborators/{uid}", delete(remove_member))
        .route("/api/playlists/{pid}/invite-link", post(make_link).delete(drop_link))
        .route("/api/collab/invites", get(my_invites))
        .route("/api/collab/invites/{pid}/accept", post(accept))
        .route("/api/collab/invites/{pid}/decline", post(decline))
        .route("/api/collab/join", post(join))
        .route("/api/collab/join/check", post(join_check))
}

/// Who the user is to a playlist they may open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Owner,
    Collaborator,
}

/// A collaborative playlist `uid` joined, while it is collaborative and its owner's account is active.
fn joined(st: &AppState, pid: i64, uid: i64) -> ApiResult<Option<Row>> {
    Ok(st.db.one(
        "SELECT p.* FROM playlists p JOIN playlist_collaborators c ON c.playlist_id = p.id AND c.user_id = ? AND c.status = 'joined'
         JOIN users o ON o.id = p.user_id AND o.disabled = 0 WHERE p.id = ? AND p.kind = 'playlist' AND p.collaborative = 1", p![uid, pid])?)
}

/// A playlist the user may open: their own (any kind), or a collaborative one they joined. 404 otherwise.
pub(crate) fn member(st: &AppState, pid: i64, u: &Row) -> ApiResult<(Row, Role)> {
    if let Some(pl) = st.db.one("SELECT * FROM playlists WHERE id=? AND user_id=?", p![pid, u.i("id")])? {
        return Ok((pl, Role::Owner));
    }
    joined(st, pid, u.i("id"))?.map(|pl| (pl, Role::Collaborator)).ok_or_else(|| err(404, "Playlist not found"))
}

/// The owner's playlist that can be collaborative: kind `playlist` of theirs (404 for anyone else's, 400 for
/// Liked Songs or a playlist added from a link).
fn owned_playlist(st: &AppState, pid: i64, u: &Row) -> ApiResult<Row> {
    let pl = super::playlists::own(st, pid, u)?;
    if pl.s("kind") != "playlist" {
        return Err(err(400, "Only your own playlists can be collaborative"));
    }
    Ok(pl)
}

/// Turns Collaborative on or off (the owner's `PATCH {collaborative}`).
pub(crate) fn set_collaborative(st: &AppState, pl: &Row, on: bool) -> ApiResult<()> {
    if on && pl.s("kind") != "playlist" {
        return Err(err(400, "Only your own playlists can be collaborative"));
    }
    st.db.execute("UPDATE playlists SET collaborative=? WHERE id=?", p![on as i64, pl.i("id")])?;
    Ok(())
}

/// Everything about a playlist's collaboration goes with it (the playlist is being deleted).
pub(crate) fn forget(st: &AppState, pid: i64) -> ApiResult<()> {
    st.db.execute("DELETE FROM playlist_collaborators WHERE playlist_id=?", p![pid])?;
    Ok(())
}

/// A deleted account's memberships and invites go with it.
pub(crate) fn forget_user(st: &AppState, uid: i64) -> ApiResult<()> {
    st.db.execute("DELETE FROM playlist_collaborators WHERE user_id=?", p![uid])?;
    Ok(())
}

/// The collaborative playlists `uid` joined (rows as `SELECT p.*`), for the playlist list.
pub(crate) fn joined_rows(st: &AppState, uid: i64) -> ApiResult<Vec<Row>> {
    Ok(st.db.query(
        "SELECT p.* FROM playlists p JOIN playlist_collaborators c ON c.playlist_id = p.id AND c.user_id = ? AND c.status = 'joined'
         JOIN users o ON o.id = p.user_id AND o.disabled = 0 WHERE p.kind = 'playlist' AND p.collaborative = 1", p![uid])?)
}

/// The owner's user name.
fn owner_name(st: &AppState, pl: &Row) -> ApiResult<String> {
    Ok(st.db.one("SELECT username FROM users WHERE id=?", p![pl.i("user_id")])?.map(|r| r.s("username")).unwrap_or_default())
}

/// How many accounts joined (active ones).
fn joined_count(st: &AppState, pid: i64) -> ApiResult<i64> {
    Ok(st.db.one("SELECT COUNT(*) n FROM playlist_collaborators c JOIN users u ON u.id = c.user_id AND u.disabled = 0
                  WHERE c.playlist_id = ? AND c.status = 'joined'", p![pid])?.map_or(0, |r| r.i("n")))
}

/// The `collab` field of a playlist in the list and detail: null for a playlist that isn't collaborative
/// (and has never been), otherwise `{role, owner, members}` (`members`: how many joined).
pub(crate) fn collab_json(st: &AppState, pl: &Row, role: Role) -> ApiResult<Value> {
    if role == Role::Owner && pl.i("collaborative") == 0 {
        return Ok(Value::Null);
    }
    let role = if role == Role::Owner { "owner" } else { "collaborator" };
    Ok(json!({ "role": role, "owner": owner_name(st, pl)?, "members": joined_count(st, pl.i("id"))? }))
}

/// An invite link as the apps get it: `{token, path, url}`; the web app opens `/#/collab/<token>`.
fn link_json(st: &AppState, token: &str) -> Value {
    let base = st.cfg.str("base_url");
    json!({ "token": token, "path": format!("/#/collab/{token}"), "url": format!("{}/#/collab/{token}", base.trim_end_matches('/')) })
}

// ---- the owner and members -----------------------------------------------------------------------------

/// `GET /api/playlists/{pid}/collaborators`: `{name, collaborative, role, owner, members: [{user_id, username,
/// status}], invite_link}`. The owner sees invites and the link; a member sees who joined.
async fn members(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let (pl, role) = member(st, pid, &u)?;
        Ok(Json(members_json(st, &pl, role)?))
    }).await
}

fn members_json(st: &AppState, pl: &Row, role: Role) -> ApiResult<Value> {
    let owner = role == Role::Owner;
    let rows = st.db.query(
        "SELECT c.user_id, u.username, c.status FROM playlist_collaborators c JOIN users u ON u.id = c.user_id AND u.disabled = 0
         WHERE c.playlist_id = ? ORDER BY c.status = 'invited', COALESCE(c.joined_at, c.created_at), u.username", p![pl.i("id")])?;
    let list: Vec<Value> = rows.into_iter().filter(|r| owner || r.s("status") == "joined")
        .map(|r| json!({ "user_id": r.i("user_id"), "username": r.s("username"), "status": r.s("status") })).collect();
    let link = match pl.s("invite_token") {
        t if owner && !t.is_empty() => link_json(st, &t),
        _ => Value::Null,
    };
    Ok(json!({ "name": pl.s("name"), "collaborative": pl.i("collaborative") == 1, "role": if owner { "owner" } else { "collaborator" },
        "owner": owner_name(st, pl)?, "members": list, "invite_link": link }))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct InviteBody {
    username: String,
}

/// `POST /api/playlists/{pid}/collaborators {username}`: the owner invites an account on this server, which
/// turns Collaborative on. Inviting someone who joined already changes nothing. → the members' view.
async fn invite(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>, Json(b): Json<InviteBody>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let pl = owned_playlist(st, pid, &u)?;
        let name = b.username.trim().trim_start_matches('@');
        if name.is_empty() {
            return Err(err(400, "Enter a user name"));
        }
        let who = st.db.one("SELECT id, username FROM users WHERE username = ? COLLATE NOCASE AND disabled = 0", p![name])?
            .ok_or_else(|| err(404, format!("There's no account named “{name}” on this server")))?;
        if who.i("id") == u.i("id") {
            return Err(err(400, "This is your own playlist"));
        }
        set_collaborative(st, &pl, true)?;
        st.db.execute("INSERT OR IGNORE INTO playlist_collaborators (playlist_id, user_id, status, invited_by, created_at) VALUES (?, ?, 'invited', ?, ?)",
            p![pid, who.i("id"), u.i("id"), now()])?;
        let pl = owned_playlist(st, pid, &u)?;
        Ok(Json(members_json(st, &pl, Role::Owner)?))
    }).await
}

/// `DELETE /api/playlists/{pid}/collaborators/{uid}`: the owner removes a member or takes an invite back;
/// a member leaves (their own id), and an invited account declines. → `{ok}`
async fn remove_member(State(st): State<AppState>, User(u): User, Path((pid, uid)): Path<(i64, i64)>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let me = u.i("id");
        let is_owner = st.db.one("SELECT 1 FROM playlists WHERE id=? AND user_id=?", p![pid, me])?.is_some();
        if !is_owner && uid != me {
            return Err(err(404, "Playlist not found"));
        }
        st.db.execute("DELETE FROM playlist_collaborators WHERE playlist_id=? AND user_id=?", p![pid, uid])?;
        Ok(Json(json!({ "ok": true })))
    }).await
}

/// `POST /api/playlists/{pid}/invite-link`: the owner's invite link (asking again gives the same one), which
/// turns Collaborative on. → `{token, path, url}`
async fn make_link(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let pl = owned_playlist(st, pid, &u)?;
        set_collaborative(st, &pl, true)?;
        let token = match pl.s("invite_token") {
            t if !t.is_empty() => t,
            _ => {
                let t = encore_core::ids::random_token(12);
                st.db.execute("UPDATE playlists SET invite_token=? WHERE id=?", p![t, pid])?;
                t
            }
        };
        Ok(Json(link_json(st, &token)))
    }).await
}

/// `DELETE /api/playlists/{pid}/invite-link`: the link stops working; those who joined with it stay. A new
/// link is a new token.
async fn drop_link(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        super::playlists::own(st, pid, &u)?;
        st.db.execute("UPDATE playlists SET invite_token=NULL WHERE id=?", p![pid])?;
        Ok(Json(json!({ "ok": true })))
    }).await
}

// ---- the invited -----------------------------------------------------------------------------------------

/// `GET /api/collab/invites`: the invites waiting for the user, newest first: `[{playlist_id, name, owner,
/// count}]` (only for playlists that are collaborative now, of active owners).
async fn my_invites(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let rows = st.db.query(
            "SELECT p.id, p.name, o.username owner, (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id) count
             FROM playlist_collaborators c JOIN playlists p ON p.id = c.playlist_id AND p.kind = 'playlist' AND p.collaborative = 1
             JOIN users o ON o.id = p.user_id AND o.disabled = 0
             WHERE c.user_id = ? AND c.status = 'invited' ORDER BY c.created_at DESC", p![u.i("id")])?;
        Ok(Json(json!(rows.into_iter().map(|r| json!({ "playlist_id": r.i("id"), "name": r.s("name"), "owner": r.s("owner"), "count": r.i("count") })).collect::<Vec<_>>())))
    }).await
}

/// Joins: the user becomes a member (an invite, if any, is used up). → `{id, name, owner}`
fn join_now(st: &AppState, uid: i64, pl: &Row) -> ApiResult<Value> {
    let t = now();
    st.db.execute("INSERT INTO playlist_collaborators (playlist_id, user_id, status, created_at, joined_at) VALUES (?, ?, 'joined', ?, ?)
                   ON CONFLICT(playlist_id, user_id) DO UPDATE SET status='joined', joined_at=COALESCE(joined_at, excluded.joined_at)",
        p![pl.i("id"), uid, t, t])?;
    Ok(json!({ "id": pl.i("id"), "name": pl.s("name"), "owner": owner_name(st, pl)? }))
}

/// `POST /api/collab/invites/{pid}/accept`: join a playlist the user was invited to.
async fn accept(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let pl = st.db.one(
            "SELECT p.* FROM playlists p JOIN playlist_collaborators c ON c.playlist_id = p.id AND c.user_id = ?
             JOIN users o ON o.id = p.user_id AND o.disabled = 0 WHERE p.id = ? AND p.kind = 'playlist' AND p.collaborative = 1", p![u.i("id"), pid])?
            .ok_or_else(|| err(404, "This invite is no longer open"))?;
        join_now(st, u.i("id"), &pl).map(Json)
    }).await
}

/// `POST /api/collab/invites/{pid}/decline`: say no to an invite (a member who joined leaves). → `{ok}`
async fn decline(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        st.db.execute("DELETE FROM playlist_collaborators WHERE playlist_id=? AND user_id=?", p![pid, u.i("id")])?;
        Ok(Json(json!({ "ok": true })))
    }).await
}

/// An invite link's path anywhere in the text (`…/#/collab/<token>`, `/collab/<token>`), or a bare token.
static IN_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"collab/([A-Za-z0-9_-]{8,64})(?:[^A-Za-z0-9_-]|$)").unwrap());
static BARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{8,64}$").unwrap());

/// The invite token in text as the user pasted or shared it. None when there is none.
pub(crate) fn invite_token_of(text: &str) -> Option<String> {
    let t = text.trim().trim_matches(|c| c == '<' || c == '>' || c == '"' || c == '\'').trim();
    if let Some(c) = IN_LINK.captures(t) {
        return Some(c[1].to_string());
    }
    BARE.is_match(t).then(|| t.to_string())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LinkBody {
    link: String,
}

/// The playlist a working invite link opens (collaborative, of an active owner).
fn by_link(st: &AppState, b: &LinkBody) -> ApiResult<Row> {
    let token = invite_token_of(&b.link).ok_or_else(|| err(400, "That isn't an invite link"))?;
    st.db.one("SELECT p.* FROM playlists p JOIN users o ON o.id = p.user_id AND o.disabled = 0
               WHERE p.invite_token = ? AND p.kind = 'playlist' AND p.collaborative = 1", p![token])?
        .ok_or_else(|| err(404, "This invite link no longer works"))
}

/// `POST /api/collab/join {link}`: join with an invite link. → `{id, name, owner}`
async fn join(State(st): State<AppState>, User(u): User, Json(b): Json<LinkBody>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let pl = by_link(st, &b)?;
        if pl.i("user_id") == u.i("id") {
            return Err(err(400, "This is your own playlist"));
        }
        join_now(st, u.i("id"), &pl).map(Json)
    }).await
}

/// `POST /api/collab/join/check {link}`: what an invite link opens, without joining: `{id, name, owner,
/// count, own, joined}`.
async fn join_check(State(st): State<AppState>, User(u): User, Json(b): Json<LinkBody>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let pl = by_link(st, &b)?;
        let own = pl.i("user_id") == u.i("id");
        let joined = !own && joined(st, pl.i("id"), u.i("id"))?.is_some();
        let count = st.db.one("SELECT COUNT(*) n FROM playlist_items WHERE playlist_id=?", p![pl.i("id")])?.map_or(0, |r| r.i("n"));
        Ok(Json(json!({ "id": pl.i("id"), "name": pl.s("name"), "owner": owner_name(st, &pl)?, "count": count, "own": own, "joined": joined })))
    }).await
}

#[cfg(test)]
#[path = "playlist_collab_tests.rs"]
mod playlist_collab_tests;
