//! /api/group: group listens (0.31). Several people on this server hear the same queue at the same time,
//! each on their own device. The groups and their rules are in `encore_playback::group`; this module is the
//! routes, the guests' tokens, the rate limits, the event streams and the loop that moves the groups on
//! (the end of a song, members gone for 2 minutes, idle groups, saving them for a restart). The shapes are
//! docs/contracts/client-shapes-0.31-0.33.md §2.2, §2.3 and §3.6–3.19.
//!
//! Who may use what:
//! - accounts: every route (any signed-in user can host; there is nothing admin-only but removing a guest
//!   from someone else's group);
//! - guests (a token of kind 'guest' from `POST /api/group/guest-join`, scoped to one group): that group's
//!   state (`GET /api/group/current`), its event stream, leaving it, and the songs and covers of its queue
//!   (`/api/group/{id}/tracks/{track}/stream`, `/api/group/{id}/albums/{album}/cover`); every other route
//!   answers 401, as the token signs nobody in anywhere else.
use crate::auth::{self, ClientInfo, MaybeUser, User};
use crate::enrich::{enrich, enrich_now};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, SameSite};
use axum_extra::extract::CookieJar;
use encore_core::error::{err, ApiError, ApiResult};
use encore_db::{now, p, Row, RowExt};
use encore_playback::group::{self as gl, AddedBy, Caller, Event, Group, Inner, Member, NewGroup};
use encore_playback::stream::StreamQ;
use futures_util::stream::{self, Stream};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;

pub const OFF: &str = "Group listens are turned off on this server";
pub const NO_CODE: &str = "No group listen with that code";
pub const FULL: &str = "This group listen is full";
pub const TOO_MANY: &str = "Too many tries. Wait a moment and try again.";
pub const STARTED_TOO_MANY: &str = "You started 10 group listens in the last hour. Try again later.";
pub const NOT_IN: &str = "You aren't in this group listen";
pub const NO_GUESTS: &str = "Only people with an account on this server can join this group listen";
/// The guest's token as a cookie, sent only to these routes.
pub const GUEST_COOKIE: &str = "encore_guest";

/// Group listens one account may start in an hour.
pub const STARTS_PER_HOUR: usize = 10;
/// Wrong codes (lookups and joins that answered 404) per account and per address in an hour; past it every
/// lookup and join answers 429 until the hour has passed.
pub const FAILED_PER_USER: usize = 30;
pub const FAILED_PER_IP: usize = 60;
/// Wrong codes of everyone without an account together in an hour (the signed-out lookup and the guests'
/// join): an address is whatever a caller's proxy says, so this budget is the one no caller can step around.
pub const FAILED_SIGNED_OUT: usize = 300;
/// Guests who may join one group in an hour, and from one address.
pub const GUESTS_PER_GROUP: usize = 20;
pub const GUESTS_PER_IP: usize = 60;
const HOUR: f64 = 3600.0;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/time", get(time))
        .route("/api/group", get(list).post(start))
        .route("/api/group/current", get(current))
        .route("/api/group/join", post(join_code))
        .route("/api/group/guest-join", post(guest_join))
        .route("/api/group/code/{code}", get(lookup))
        .route("/api/group/{id}", axum::routing::patch(patch))
        .route("/api/group/{id}/join", post(join_id))
        .route("/api/group/{id}/leave", post(leave))
        .route("/api/group/{id}/end", post(end))
        .route("/api/group/{id}/command", post(command))
        .route("/api/group/{id}/members/{member}", delete(remove_member))
        .route("/api/group/{id}/events", get(events))
        .route("/api/group/{id}/tracks/{track}/stream", get(stream_track))
        .route("/api/group/{id}/albums/{album}/cover", get(cover))
}

fn now_ms() -> i64 {
    (now() * 1000.0) as i64
}

fn server_max(st: &AppState) -> i64 {
    st.cfg.int("group_listen_max").clamp(2, 100)
}

fn guests_possible(st: &AppState) -> bool {
    st.cfg.bool("group_listen_guests")
}

/// The saved groups come back on first use after a start (`group_listen_keep` on), or what an earlier run
/// saved is cleared (off); the tokens of guests whose group did not come back stop working.
pub fn ready(st: &AppState) {
    let mut inner = st.playback.groups.lock();
    if inner.restored {
        return;
    }
    inner.restored = true;
    if st.cfg.bool("group_listen_keep") {
        for g in gl::load(&st.db, now_ms(), |u| avatar_of(st, u)) {
            inner.saved.insert(g.id.clone());
            inner.groups.insert(g.id.clone(), g);
        }
    } else if let Err(e) = gl::clear(&st.db) {
        tracing::warn!("group listens: clearing the saved ones failed: {e}");
    }
    let tokens = st.db.query("SELECT token, device_id, device_name FROM sessions WHERE kind='guest'", p![]).unwrap_or_default();
    for t in tokens {
        let alive = inner.groups.get(&t.s("device_id")).and_then(|g| g.member(&t.s("device_name"))).is_some_and(Member::is_guest);
        if !alive {
            let _ = st.db.execute("DELETE FROM sessions WHERE token=? AND kind='guest'", p![t.s("token")]);
        }
    }
}

/// Group listens are on (and restored); off: every group ends ("off") and the route answers 403.
fn on(st: &AppState) -> ApiResult<()> {
    ready(st);
    if st.cfg.bool("group_listen") {
        return Ok(());
    }
    end_all(st, "off");
    Err(err(403, OFF))
}

/// Ends every group with `reason`.
fn end_all(st: &AppState, reason: &'static str) {
    let ids: Vec<String> = {
        let mut inner = st.playback.groups.lock();
        let ids: Vec<String> = inner.groups.keys().cloned().collect();
        for id in &ids {
            inner.end(id, reason);
        }
        ids
    };
    for id in ids {
        drop_guest_tokens(st, &id, None);
    }
}

/// The guests' tokens of a group (or of one guest) stop working.
fn drop_guest_tokens(st: &AppState, group: &str, member: Option<&str>) {
    let r = match member {
        Some(m) => st.db.execute("DELETE FROM sessions WHERE kind='guest' AND device_id=? AND device_name=?", p![group, m]),
        None => st.db.execute("DELETE FROM sessions WHERE kind='guest' AND device_id=?", p![group]),
    };
    if let Err(e) = r {
        tracing::warn!("group listens: a guest's token could not be deleted: {e}");
    }
}

/// An account's picture as a server path, or "".
fn avatar_of(st: &AppState, user: i64) -> String {
    let has = st.db.one("SELECT avatar_at FROM users WHERE id=?", p![user]).ok().flatten().is_some_and(|r| r.get("avatar_at").is_some_and(|v| !v.is_null()));
    if has { format!("/api/users/{user}/avatar") } else { String::new() }
}

fn device(id: &str) -> ApiResult<String> {
    let id = id.trim();
    if id.is_empty() || id.chars().count() > 64 {
        return Err(err(400, "Bad device id"));
    }
    Ok(id.to_string())
}

fn device_name(name: &str) -> String {
    name.trim().chars().take(60).collect()
}

fn by_of(u: &Row) -> AddedBy {
    AddedBy { user: u.i("id"), name: u.s("username"), guest: false }
}

fn member_of(st: &AppState, u: &Row, device: &str, name: &str, t: f64) -> Member {
    Member {
        member: String::new(), user: u.i("id"), guest: None, name: u.s("username"), avatar: avatar_of(st, u.i("id")), device_id: device.to_string(),
        device_name: device_name(name), joined_at: t, host: false, added_by: None, streams: 0, gone_since: Some(t), expires_at: None,
    }
}

// ---- who is asking ------------------------------------------------------------------------------------------

/// A guest, by the token it got from guest-join (a bearer token, or the `encore_guest` cookie).
#[derive(Clone, Debug)]
pub struct Guest {
    pub group: String,
    pub member: String,
}

/// The signed-in account, or else a guest (only the routes a guest may use take this).
pub enum Who {
    Account(Row),
    Guest(Guest),
}

impl FromRequestParts<AppState> for Who {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        match User::from_request_parts(parts, st).await {
            Ok(User(u)) => Ok(Who::Account(u)),
            Err(e) if e.status == StatusCode::UNAUTHORIZED => guest_of(st, parts)?.map(Who::Guest).ok_or(e),
            Err(e) => Err(e),
        }
    }
}

/// A guest's token in a request: the bearer token, else the `encore_guest` cookie (true).
fn guest_token(headers: &HeaderMap) -> Option<(String, bool)> {
    match auth::bearer(headers) {
        Some(t) => Some((t, false)),
        None => CookieJar::from_headers(headers).get(GUEST_COOKIE).map(|c| (c.value().to_string(), true)),
    }
}

/// The guest session a token names (`device_id` = the group, `device_name` = the membership), while its 12
/// hours are not over.
fn guest_session(st: &AppState, token: &str) -> ApiResult<Option<Row>> {
    let s = st.db.one("SELECT device_id, device_name, expires_at FROM sessions WHERE token=? AND kind='guest'", p![auth::app_token_key(token)])?;
    Ok(s.filter(|s| s.f("expires_at") >= now()))
}

/// The guest a request's token names, while guests may join on this server, its group has it as a member and
/// its 12 hours are not over.
fn guest_of(st: &AppState, parts: &Parts) -> ApiResult<Option<Guest>> {
    if !guests_possible(st) {
        return Ok(None);
    }
    let Some((token, cookie)) = guest_token(&parts.headers) else { return Ok(None) };
    let Some(s) = guest_session(st, &token)? else { return Ok(None) };
    ready(st);
    let g = Guest { group: s.s("device_id"), member: s.s("device_name") };
    let alive = st.playback.groups.lock().groups.get(&g.group).and_then(|x| x.member(&g.member)).is_some_and(Member::is_guest);
    if !alive {
        return Ok(None);
    }
    // a cookie needs the CSRF header on writes, as the session cookie does
    if cookie && !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS)
        && parts.headers.get("x-requested-with").and_then(|v| v.to_str().ok()) != Some("encore") {
        return Err(err(403, "Missing CSRF header"));
    }
    Ok(Some(g))
}

/// Who a state is made for: the caller, its account (translations) and the address invite links start with.
#[derive(Clone)]
struct Viewer {
    caller: Caller,
    user: Option<Row>,
    base_url: String,
}

impl Viewer {
    fn account(st: &AppState, u: &Row, device: &str, c: &ClientInfo) -> Viewer {
        Viewer { caller: Caller::Account { user: u.i("id"), device: device.to_string() }, user: Some(u.clone()), base_url: base_url(st, c) }
    }

    fn of(st: &AppState, who: &Who, device: &str, c: &ClientInfo) -> Viewer {
        match who {
            Who::Account(u) => Viewer::account(st, u, device, c),
            Who::Guest(g) => Viewer { caller: Caller::Guest { member: g.member.clone() }, user: None, base_url: base_url(st, c) },
        }
    }
}

/// `<public origin>`: the `base_url` setting, else the request's own scheme and host; "" when unknown.
fn base_url(st: &AppState, c: &ClientInfo) -> String {
    let b = st.cfg.str("base_url");
    if !b.trim().is_empty() {
        return b.trim().trim_end_matches('/').to_string();
    }
    if c.base_url.ends_with("://") { String::new() } else { c.base_url.clone() }
}

// ---- the state ----------------------------------------------------------------------------------------------

/// Moves a group on to where it is now (the end of a song) and announces that.
fn catch_up(g: &mut Group) {
    if g.advance(now_ms()) {
        g.changed(now());
    }
}

/// The group state for one device (client-shapes §2.2), or None when the group is gone or the viewer is not a
/// member of it. Runs on a blocking thread: the queue's enrichment reads the database.
fn state_now(st: &AppState, id: &str, v: &Viewer) -> Option<Value> {
    let boot = st.playback.groups.boot.clone();
    let (cached, fresh, rev, you, code, cap) = {
        let mut inner = st.playback.groups.lock();
        let g = inner.groups.get_mut(id)?;
        catch_up(g);
        let you = g.you(&v.caller);
        if you.member.is_empty() {
            return None;
        }
        let cached = g.cache.as_ref().filter(|(r, _)| *r == g.rev).map(|(_, b)| b.clone());
        let fresh = if cached.is_none() { Some(g.base_json(&boot)) } else { None };
        (cached, fresh, g.rev, you, g.code.clone(), g.cap(server_max(st)))
    };
    let base = match (cached, fresh) {
        (Some(b), _) => b,
        (None, Some(mut b)) => {
            // covers, credits and loudness are the same for everyone: made once per rev
            enrich_now(st, None, &mut b);
            let b = std::sync::Arc::new(b);
            let mut inner = st.playback.groups.lock();
            if let Some(g) = inner.groups.get_mut(id).filter(|g| g.rev == rev) {
                g.cache = Some((rev, b.clone()));
            }
            b
        }
        (None, None) => return None,
    };
    let mut out = (*base).clone();
    out["invite_url"] = json!(if v.base_url.is_empty() { String::new() } else { format!("{}/#/group/{code}", v.base_url) });
    out["guests_possible"] = json!(guests_possible(st));
    out["max_members"] = json!(cap);
    out["server_max_members"] = json!(server_max(st));
    out["you"] = you.json();
    // translations are the listener's own
    if let Some(u) = &v.user {
        let (mode, target) = encore_library::translate::prefs_of(u);
        if mode != encore_library::translate::Mode::Off && st.library.translator.anyone_on(&st.db) {
            let missing = encore_library::translate::stamp(&st.db, target, &mut out);
            st.library.translator.enqueue(target, missing);
        }
    }
    Some(out)
}

async fn state_for(st: &AppState, id: &str, v: &Viewer) -> Option<Value> {
    let (st, id, v) = (st.clone(), id.to_string(), v.clone());
    tokio::task::spawn_blocking(move || state_now(&st, &id, &v)).await.ok().flatten()
}

/// The answer of a route that answers the state.
async fn answer(st: &AppState, id: &str, v: &Viewer) -> ApiResult<Json<Value>> {
    state_for(st, id, v).await.map(Json).ok_or_else(|| err(404, NOT_IN))
}

// ---- rate limits ------------------------------------------------------------------------------------------

/// The key of the signed-out callers' shared budget of wrong codes.
const SIGNED_OUT_KEY: &str = "group-code:signed-out";

fn tries_left(st: &AppState, user: Option<i64>, ip: &str) -> ApiResult<()> {
    let by_user = match user {
        Some(u) => st.limits.limited(&format!("group-code:u{u}"), FAILED_PER_USER, HOUR, false),
        None => st.limits.limited(SIGNED_OUT_KEY, FAILED_SIGNED_OUT, HOUR, false),
    };
    if by_user || st.limits.limited(&format!("group-code:ip{ip}"), FAILED_PER_IP, HOUR, false) {
        return Err(err(429, TOO_MANY));
    }
    Ok(())
}

fn failed_try(st: &AppState, user: Option<i64>, ip: &str) -> ApiError {
    match user {
        Some(u) => st.limits.record(&format!("group-code:u{u}")),
        None => st.limits.record(SIGNED_OUT_KEY),
    }
    st.limits.record(&format!("group-code:ip{ip}"));
    err(404, NO_CODE)
}

// ---- what happens to members ------------------------------------------------------------------------------

/// What taking a member out did.
#[derive(Debug, PartialEq)]
enum Out {
    /// nothing: it was not there
    Nothing,
    /// out; `true` when it was a guest (its token goes)
    Left(bool),
    /// out, and the group ended: no account member was left
    Ended,
}

/// Takes `member` out of group `id`. The host's leaving hands the group over; the last account member's
/// leaving ends it ("ended": the guests' streams are told).
fn take_out(inner: &mut Inner, id: &str, member: &str) -> Out {
    let Some(g) = inner.groups.get_mut(id) else { return Out::Nothing };
    let Some(guest) = g.member(member).map(Member::is_guest) else { return Out::Nothing };
    if g.remove(member) {
        inner.end(id, "ended");
        return Out::Ended;
    }
    g.changed(now());
    Out::Left(guest)
}

fn after_out(st: &AppState, id: &str, member: &str, out: &Out) {
    match out {
        Out::Left(true) => drop_guest_tokens(st, id, Some(member)),
        Out::Ended => drop_guest_tokens(st, id, None),
        _ => {}
    }
}

/// The account's device leaves every group it is in (it can be in one at a time).
fn leave_all(inner: &mut Inner, user: i64, device: &str, except: &str) -> Vec<(String, String, Out)> {
    let mut done = vec![];
    while let Some((id, m)) = inner.of(user, device).filter(|g| g.id != except).map(|g| {
        let c = Caller::Account { user, device: device.to_string() };
        (g.id.clone(), g.find(&c).map(|m| m.member.clone()).unwrap_or_default())
    }) {
        let out = take_out(inner, &id, &m);
        if out == Out::Nothing {
            break;
        }
        done.push((id, m, out));
    }
    done
}

// ---- the routes ---------------------------------------------------------------------------------------------

/// GET /api/time: the server's clock (Unix ms) and the process id. No sign-in, no rate limit.
async fn time(State(st): State<AppState>) -> Json<Value> {
    Json(json!({ "now_ms": now_ms(), "boot": &*st.playback.groups.boot }))
}

/// GET /api/group: every "anyone" group plus the groups the account is in (on any device), newest first.
async fn list(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    on(&st)?;
    let uid = u.i("id");
    let mut found: Vec<(f64, Value)> = {
        let mut inner = st.playback.groups.lock();
        inner.groups.values_mut().for_each(catch_up);
        inner.groups.values().filter(|g| g.anyone() || g.has_account(uid)).map(|g| (g.started_at, g.summary_json(uid))).collect()
    };
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut v = json!(found.into_iter().map(|(_, v)| v).collect::<Vec<_>>());
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DeviceQ {
    device_id: String,
}

/// GET /api/group/code/{code}: who hosts the group behind a code (also a link-only one: the code is the
/// secret). Without an account it answers only when a guest may join (both switches on). `guests`: whether
/// a guest may join now.
async fn lookup(State(st): State<AppState>, MaybeUser(u): MaybeUser, Path(code): Path<String>, c: ClientInfo) -> ApiResult<Json<Value>> {
    on(&st)?;
    let uid = u.as_ref().map(|u| u.i("id"));
    tries_left(&st, uid, &c.ip)?;
    let found = {
        let mut inner = st.playback.groups.lock();
        inner.groups.values_mut().for_each(catch_up);
        inner.by_code(&code).map(|g| (g.summary_json(uid.unwrap_or(0)), guests_possible(&st) && g.guests_allowed))
    };
    let Some((mut v, guests)) = found.filter(|(_, guests)| uid.is_some() || *guests) else { return Err(failed_try(&st, uid, &c.ip)) };
    v["guests"] = json!(guests);
    enrich(&st, u.as_ref(), &mut v).await;
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StartB {
    device_id: String,
    device_name: String,
    visibility: String,
    everyone_controls: bool,
    tracks: Vec<Value>,
    index: i64,
    position: f64,
    playing: bool,
}

fn visibility(v: &str) -> ApiResult<String> {
    match v {
        "" | gl::ANYONE => Ok(gl::ANYONE.into()),
        gl::LINK => Ok(gl::LINK.into()),
        _ => Err(err(400, "Who can join is either \"anyone\" or \"link\"")),
    }
}

/// POST /api/group: start a group listen; what plays on the host's device becomes the queue.
async fn start(State(st): State<AppState>, User(u): User, c: ClientInfo, Json(b): Json<StartB>) -> ApiResult<Json<Value>> {
    on(&st)?;
    let dev = device(&b.device_id)?;
    let uid = u.i("id");
    // checked and counted in one step, so parallel starts can't all pass before the first is counted (a start
    // that then fails counts too)
    if st.limits.limited(&format!("group-start:{uid}"), STARTS_PER_HOUR, HOUR, true) {
        return Err(err(429, STARTED_TOO_MANY));
    }
    let vis = visibility(&b.visibility)?;
    if b.tracks.len() > gl::START_MAX {
        return Err(err(400, format!("A group listen starts with at most {} songs", gl::START_MAX)));
    }
    let ids: Vec<i64> = b.tracks.iter().filter_map(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).collect();
    let rows = { let ids = ids.clone(); st.db.read(move |c| Ok::<_, rusqlite::Error>(gl::rows(c, &ids))).await? };
    let (t, t_ms) = (now(), now_ms());
    let me = member_of(&st, &u, &dev, &b.device_name, t);
    let (id, left) = {
        let mut inner = st.playback.groups.lock();
        let left = leave_all(&mut inner, uid, &dev, "");
        let code = inner.new_code();
        let id = format!("g_{}", encore_core::ids::random_hex(10));
        let mut g = Group::new(NewGroup { id: id.clone(), code, visibility: vis, everyone_controls: b.everyone_controls, guests_allowed: false, max_members: gl::MAX_MEMBERS }, t_ms);
        g.add(me);
        g.set_queue(&ids, &rows, gl::At { index: b.index.max(0) as usize, position: b.position, playing: b.playing }, &by_of(&u), t_ms);
        inner.groups.insert(id.clone(), g);
        (id, left)
    };
    for (gid, m, out) in &left {
        after_out(&st, gid, m, out);
    }
    answer(&st, &id, &Viewer::account(&st, &u, &dev, &c)).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JoinB {
    device_id: String,
    device_name: String,
    code: String,
}

/// Puts the account's device in group `id` (it leaves any other group first). Already in it: the state as it is.
async fn join(st: &AppState, u: &Row, id: &str, dev: &str, name: &str, c: &ClientInfo) -> ApiResult<Json<Value>> {
    let uid = u.i("id");
    let me = member_of(st, u, dev, name, now());
    let left = {
        let mut inner = st.playback.groups.lock();
        let caller = Caller::Account { user: uid, device: dev.to_string() };
        let g = inner.groups.get(id).ok_or_else(|| err(404, NO_CODE))?;
        if g.find(&caller).is_some() {
            vec![]
        } else {
            if g.full(server_max(st)) {
                return Err(err(409, FULL));
            }
            let left = leave_all(&mut inner, uid, dev, id);
            if let Some(g) = inner.groups.get_mut(id) {
                g.add(me);
                g.changed(now());
            }
            left
        }
    };
    for (gid, m, out) in &left {
        after_out(st, gid, m, out);
    }
    answer(st, id, &Viewer::account(st, u, dev, c)).await
}

/// POST /api/group/join {code}.
async fn join_code(State(st): State<AppState>, User(u): User, c: ClientInfo, Json(b): Json<JoinB>) -> ApiResult<Json<Value>> {
    on(&st)?;
    let dev = device(&b.device_id)?;
    let uid = u.i("id");
    tries_left(&st, Some(uid), &c.ip)?;
    let id = st.playback.groups.lock().by_code(&b.code).map(|g| g.id.clone());
    let Some(id) = id else { return Err(failed_try(&st, Some(uid), &c.ip)) };
    join(&st, &u, &id, &dev, &b.device_name, &c).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DeviceB {
    device_id: String,
    device_name: String,
}

fn device_body(body: &Bytes) -> DeviceB {
    serde_json::from_slice(body).unwrap_or_default()
}

/// POST /api/group/{id}/join: an "anyone" group from the list, without its code. A link-only group the
/// account is not in answers as an unknown code does (and counts as a wrong try), so it does not leak.
async fn join_id(State(st): State<AppState>, User(u): User, Path(id): Path<String>, c: ClientInfo, body: Bytes) -> ApiResult<Json<Value>> {
    on(&st)?;
    let b = device_body(&body);
    let dev = device(&b.device_id)?;
    let uid = u.i("id");
    tries_left(&st, Some(uid), &c.ip)?;
    let visible = st.playback.groups.lock().groups.get(&id).is_some_and(|g| g.anyone() || g.has_account(uid));
    if !visible {
        return Err(failed_try(&st, Some(uid), &c.ip));
    }
    join(&st, &u, &id, &dev, &b.device_name, &c).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GuestB {
    code: String,
    name: String,
}

/// POST /api/group/guest-join {code, name}: someone without an account joins with the invite link (the
/// web page only). Both switches must be on: the server's `group_listen_guests` (off: 403 whatever the code)
/// and the group's "Guests can join with the link" (off: answered as an unknown code is, and counted as a
/// wrong try, so this route tells a live code from a wrong one no better than the lookup). Answers the state
/// with `token` (also set as the `encore_guest` cookie, sent only to /api/group): it lasts until the guest
/// leaves or is removed, the group ends, an admin turns guests off, or 12 hours have passed.
async fn guest_join(State(st): State<AppState>, c: ClientInfo, Json(b): Json<GuestB>) -> ApiResult<Response> {
    on(&st)?;
    if !guests_possible(&st) {
        return Err(err(403, NO_GUESTS));
    }
    tries_left(&st, None, &c.ip)?;
    let name: String = b.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if !(2..=32).contains(&name.chars().count()) {
        return Err(err(400, "Your name needs 2 to 32 characters"));
    }
    let ip_key = format!("group-guest:ip{}", c.ip);
    if st.limits.limited(&ip_key, GUESTS_PER_IP, HOUR, false) {
        return Err(err(429, TOO_MANY));
    }
    let t = now();
    let (id, member) = {
        let mut inner = st.playback.groups.lock();
        let Some(id) = inner.by_code(&b.code).map(|g| g.id.clone()) else {
            drop(inner);
            return Err(failed_try(&st, None, &c.ip));
        };
        let g = inner.groups.get_mut(&id).ok_or_else(|| err(404, NO_CODE))?;
        if !g.guests_allowed {
            drop(inner);
            return Err(failed_try(&st, None, &c.ip));
        }
        if st.limits.limited(&format!("group-guest:g{id}"), GUESTS_PER_GROUP, HOUR, false) {
            return Err(err(429, TOO_MANY));
        }
        if g.full(server_max(&st)) {
            return Err(err(409, FULL));
        }
        let member = g.add(Member {
            member: String::new(), user: 0, guest: Some(encore_core::ids::random_hex(8)), name, avatar: String::new(), device_id: String::new(),
            device_name: String::new(), joined_at: t, host: false, added_by: None, streams: 0, gone_since: Some(t), expires_at: Some(t + gl::GUEST_SECS),
        });
        g.changed(t);
        (id, member)
    };
    st.limits.record(&ip_key);
    st.limits.record(&format!("group-guest:g{id}"));
    let token = encore_core::ids::random_token(32);
    let ua: String = c.ua.chars().take(200).collect();
    if let Err(e) = st.db.execute(
        "INSERT INTO sessions (token, user_id, created_at, expires_at, user_agent, ip, kind, device_id, device_name, last_seen_at) VALUES (?,0,?,?,?,?,'guest',?,?,?)",
        p![auth::app_token_key(&token), t, t + gl::GUEST_SECS, ua, c.ip, id, member, t],
    ) {
        let mut inner = st.playback.groups.lock();
        let out = take_out(&mut inner, &id, &member);
        drop(inner);
        after_out(&st, &id, &member, &out);
        return Err(e.into());
    }
    let v = Viewer { caller: Caller::Guest { member }, user: None, base_url: base_url(&st, &c) };
    let Json(mut state) = answer(&st, &id, &v).await?;
    state["token"] = json!(token);
    let cookie = Cookie::build((GUEST_COOKIE, token)).path("/api/group").http_only(true).same_site(SameSite::Lax).secure(c.https)
        .max_age(cookie::time::Duration::seconds(gl::GUEST_SECS as i64)).build();
    let mut r = Json(state).into_response();
    if let Ok(h) = HeaderValue::from_str(&cookie.to_string()) {
        r.headers_mut().append(header::SET_COOKIE, h);
    }
    Ok(r)
}

/// GET /api/group/current?device_id=: the group this device (or this guest) is in, else 204.
async fn current(State(st): State<AppState>, who: Who, Query(q): Query<DeviceQ>, c: ClientInfo) -> ApiResult<Response> {
    on(&st)?;
    let id = match &who {
        Who::Account(u) => st.playback.groups.lock().of(u.i("id"), q.device_id.trim()).map(|g| g.id.clone()),
        Who::Guest(g) => Some(g.group.clone()),
    };
    let Some(id) = id else { return Ok(StatusCode::NO_CONTENT.into_response()) };
    match state_for(&st, &id, &Viewer::of(&st, &who, q.device_id.trim(), &c)).await {
        Some(v) => Ok(Json(v).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

/// POST /api/group/{id}/leave: this device (or guest) leaves; 204 whether it was in the group or not. A
/// guest's token stops working.
async fn leave(State(st): State<AppState>, who: Who, Path(id): Path<String>, Query(q): Query<DeviceQ>, body: Bytes) -> ApiResult<Response> {
    ready(&st);
    let b = device_body(&body);
    let dev = if b.device_id.trim().is_empty() { q.device_id } else { b.device_id };
    let member = {
        let mut inner = st.playback.groups.lock();
        let m = match &who {
            Who::Account(u) => inner.groups.get(&id).and_then(|g| g.find(&Caller::Account { user: u.i("id"), device: dev.trim().to_string() })).map(|m| m.member.clone()),
            Who::Guest(g) => (g.group == id).then(|| g.member.clone()),
        };
        m.map(|m| {
            let out = take_out(&mut inner, &id, &m);
            (m, out)
        })
    };
    if let Some((m, out)) = member {
        after_out(&st, &id, &m, &out);
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    if matches!(who, Who::Guest(_)) {
        let gone = Cookie::build((GUEST_COOKIE, "")).path("/api/group").max_age(cookie::time::Duration::ZERO).build();
        if let Ok(h) = HeaderValue::from_str(&gone.to_string()) {
            r.headers_mut().append(header::SET_COOKIE, h);
        }
    }
    Ok(r)
}

/// The caller's membership in group `id` (by its device), else 404.
fn member_in(inner: &Inner, id: &str, caller: &Caller) -> ApiResult<String> {
    inner.groups.get(id).and_then(|g| g.find(caller)).map(|m| m.member.clone()).ok_or_else(|| err(404, NOT_IN))
}

/// POST /api/group/{id}/end: the host ends it for everyone.
async fn end(State(st): State<AppState>, User(u): User, Path(id): Path<String>, Query(q): Query<DeviceQ>, body: Bytes) -> ApiResult<StatusCode> {
    on(&st)?;
    let b = device_body(&body);
    let dev = if b.device_id.trim().is_empty() { q.device_id } else { b.device_id };
    let caller = Caller::Account { user: u.i("id"), device: dev.trim().to_string() };
    {
        let mut inner = st.playback.groups.lock();
        member_in(&inner, &id, &caller)?;
        if !inner.groups.get(&id).is_some_and(|g| g.you(&caller).host) {
            return Err(err(403, "Only the host can end the group listen"));
        }
        inner.end(&id, "ended");
    }
    drop_guest_tokens(&st, &id, None);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PatchB {
    device_id: String,
    everyone_controls: Option<bool>,
    visibility: Option<String>,
    guests_allowed: Option<bool>,
    max_members: Option<i64>,
}

/// PATCH /api/group/{id}: the host's settings.
async fn patch(State(st): State<AppState>, User(u): User, Path(id): Path<String>, c: ClientInfo, Json(b): Json<PatchB>) -> ApiResult<Json<Value>> {
    on(&st)?;
    let dev = b.device_id.trim().to_string();
    let caller = Caller::Account { user: u.i("id"), device: dev.clone() };
    let vis = b.visibility.as_deref().map(visibility).transpose()?;
    {
        let mut inner = st.playback.groups.lock();
        member_in(&inner, &id, &caller)?;
        let max = server_max(&st);
        let g = inner.groups.get_mut(&id).ok_or_else(|| err(404, NOT_IN))?;
        if !g.you(&caller).host {
            return Err(err(403, "Only the host can change the group listen"));
        }
        if let Some(x) = b.everyone_controls {
            g.everyone_controls = x;
        }
        if let Some(x) = vis {
            g.visibility = x;
        }
        if let Some(x) = b.guests_allowed {
            g.guests_allowed = x;
        }
        if let Some(x) = b.max_members {
            g.max_members = x.clamp(2, max).max(g.members.len() as i64);
        }
        g.changed(now());
    }
    answer(&st, &id, &Viewer::account(&st, &u, &dev, &c)).await
}

/// POST /api/group/{id}/command {op, …}: play, pause, seek, next, prev, jump, set_queue, remove, move (the
/// host, or everyone with "Everyone can control"), enqueue and play_next (every account member).
async fn command(State(st): State<AppState>, User(u): User, Path(id): Path<String>, c: ClientInfo, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    on(&st)?;
    let dev = b["device_id"].as_str().unwrap_or("").trim().to_string();
    let caller = Caller::Account { user: u.i("id"), device: dev.clone() };
    let op = b["op"].as_str().unwrap_or("");
    let needs = gl::needs_control(op).ok_or_else(|| err(400, "Unknown command"))?;
    let ids = if b.get("tracks").is_some() { gl::ids(&b)? } else { vec![] };
    let rows = st.db.read(move |c| Ok::<_, rusqlite::Error>(gl::rows(c, &ids))).await?;
    {
        let mut inner = st.playback.groups.lock();
        member_in(&inner, &id, &caller)?;
        let g = inner.groups.get_mut(&id).ok_or_else(|| err(404, NOT_IN))?;
        catch_up(g);
        if needs && !g.you(&caller).can_control {
            return Err(err(403, format!("Only {} controls playback", g.host_name())));
        }
        g.command(op, &b, &rows, &by_of(&u), now_ms())?;
        g.changed(now());
    }
    answer(&st, &id, &Viewer::account(&st, &u, &dev, &c)).await
}

/// DELETE /api/group/{id}/members/{member}?device_id=: "Remove from the group" (guests only; the host and
/// admins, who are in the group). The guest's token stops working and its stream gets `ended` "removed".
async fn remove_member(State(st): State<AppState>, User(u): User, Path((id, member)): Path<(String, String)>, Query(q): Query<DeviceQ>, c: ClientInfo) -> ApiResult<Json<Value>> {
    on(&st)?;
    let dev = q.device_id.trim().to_string();
    let caller = Caller::Account { user: u.i("id"), device: dev.clone() };
    let out = {
        let mut inner = st.playback.groups.lock();
        member_in(&inner, &id, &caller)?;
        let g = inner.groups.get(&id).ok_or_else(|| err(404, NOT_IN))?;
        let target = g.member(&member).ok_or_else(|| err(404, "That person isn't in the group listen any more"))?;
        if !target.is_guest() {
            return Err(err(403, "Only guests can be removed from a group listen"));
        }
        if !(g.you(&caller).host || encore_config::perms::is_admin(&u)) {
            return Err(err(403, "Only the host and admins can remove someone from a group listen"));
        }
        let _ = g.tx.send(Event::Ended { reason: "removed", member: Some(member.clone()) });
        take_out(&mut inner, &id, &member)
    };
    after_out(&st, &id, &member, &out);
    answer(&st, &id, &Viewer::account(&st, &u, &dev, &c)).await
}

/// Counts an open event stream of a member; when its last one closes, the 2 minutes start.
struct Watching {
    groups: encore_playback::group::Groups,
    group: String,
    member: String,
}

impl Drop for Watching {
    fn drop(&mut self) {
        let mut inner = self.groups.lock();
        if let Some(m) = inner.groups.get_mut(&self.group).and_then(|g| g.members.iter_mut().find(|m| m.member == self.member)) {
            m.streams = m.streams.saturating_sub(1);
            if m.streams == 0 {
                m.gone_since = Some(now());
            }
        }
    }
}

/// How often an open event stream checks that its caller may still be in the group.
const RECHECK: Duration = if cfg!(test) { Duration::from_millis(100) } else { Duration::from_secs(30) };

struct Feed {
    st: AppState,
    id: String,
    viewer: Viewer,
    member: String,
    rx: broadcast::Receiver<Event>,
    first: Option<Value>,
    last_rev: u64,
    done: bool,
    /// the request's headers, for [`still_allowed`] every [`RECHECK`]
    headers: HeaderMap,
    tick: tokio::time::Interval,
    _watching: Watching,
}

/// Whether the caller of an open event stream may still be in the group: the account is not disabled or
/// deleted and its session was not revoked ("Sign out everywhere"); a guest's token still works.
fn still_allowed(st: &AppState, headers: &HeaderMap, caller: &Caller) -> bool {
    match caller {
        Caller::Account { user, .. } => auth::current_user(st, headers).is_some_and(|u| u.i("id") == *user),
        Caller::Guest { member } => {
            guests_possible(st)
                && guest_token(headers).and_then(|(t, _)| guest_session(st, &t).ok().flatten()).is_some_and(|s| s.s("device_name") == *member)
        }
    }
}

/// GET /api/group/{id}/events?device_id=: `state` on connect and on every change (never an older `rev` after
/// a newer one), `ended` {reason} when the group ends for this member ("ended", "removed", "idle", "off",
/// and for a guest "expired"), then the stream closes; `: keep-alive` every 15 s. 404 when the group is gone
/// or this device is not in it (also after being dropped). Every 30 s the stream checks its caller again: an
/// account that was disabled or deleted or whose session was revoked, or a guest whose token stopped working,
/// is taken out of the group and gets `ended` "removed".
async fn events(State(st): State<AppState>, headers: HeaderMap, who: Who, Path(id): Path<String>, Query(q): Query<DeviceQ>, c: ClientInfo) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    on(&st)?;
    if let Who::Guest(g) = &who {
        if g.group != id {
            return Err(err(404, NOT_IN));
        }
    }
    let viewer = Viewer::of(&st, &who, q.device_id.trim(), &c);
    let (rx, member) = {
        let mut inner = st.playback.groups.lock();
        let g = inner.groups.get_mut(&id).ok_or_else(|| err(404, NOT_IN))?;
        let mid = g.find(&viewer.caller).map(|m| m.member.clone()).ok_or_else(|| err(404, NOT_IN))?;
        let m = g.members.iter_mut().find(|m| m.member == mid).ok_or_else(|| err(404, NOT_IN))?;
        m.streams += 1;
        m.gone_since = None;
        (g.tx.subscribe(), mid)
    };
    let watching = Watching { groups: st.playback.groups.clone(), group: id.clone(), member: member.clone() };
    let first = state_for(&st, &id, &viewer).await.ok_or_else(|| err(404, NOT_IN))?;
    let last_rev = first["rev"].as_u64().unwrap_or(0);
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + RECHECK, RECHECK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let feed = Feed { st, id, viewer, member, rx, first: Some(first), last_rev, done: false, headers, tick, _watching: watching };
    let s = stream::unfold(feed, |mut f| async move {
        if f.done {
            return None;
        }
        if let Some(v) = f.first.take() {
            return Some((Ok(SseEvent::default().event("state").data(v.to_string())), f));
        }
        loop {
            let got = tokio::select! {
                m = f.rx.recv() => m,
                _ = f.tick.tick() => {
                    if still_allowed(&f.st, &f.headers, &f.viewer.caller) {
                        continue;
                    }
                    // out of the group (a host hands it over), and the stream ends
                    let out = {
                        let mut inner = f.st.playback.groups.lock();
                        if let Some(g) = inner.groups.get(&f.id) {
                            let _ = g.tx.send(Event::Ended { reason: "removed", member: Some(f.member.clone()) });
                        }
                        take_out(&mut inner, &f.id, &f.member)
                    };
                    after_out(&f.st, &f.id, &f.member, &out);
                    f.done = true;
                    return Some((Ok(SseEvent::default().event("ended").data(json!({ "reason": "removed" }).to_string())), f));
                }
            };
            match got {
                Ok(Event::State) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    // the state as it is now
                    // (a device that left and joined again has a new membership: this stream was the old one's)
                    let Some(v) = state_for(&f.st, &f.id, &f.viewer).await.filter(|v| v["you"]["member"] == f.member.as_str()) else {
                        // a member no more, or the group is gone: the `ended` that says why may be waiting behind
                        // this change; then the stream closes
                        let reason = ended_waiting(&mut f.rx, &f.member)?;
                        f.done = true;
                        return Some((Ok(SseEvent::default().event("ended").data(json!({ "reason": reason }).to_string())), f));
                    };
                    let rev = v["rev"].as_u64().unwrap_or(0);
                    if rev > f.last_rev {
                        f.last_rev = rev;
                        return Some((Ok(SseEvent::default().event("state").data(v.to_string())), f));
                    }
                }
                Ok(Event::Ended { reason, member }) => {
                    if member.as_ref().is_none_or(|m| *m == f.member) {
                        f.done = true;
                        return Some((Ok(SseEvent::default().event("ended").data(json!({ "reason": reason }).to_string())), f));
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(s).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive")))
}

/// The reason of an `ended` for `member` (or for everyone) already in the channel, if there is one.
fn ended_waiting(rx: &mut broadcast::Receiver<Event>, member: &str) -> Option<&'static str> {
    loop {
        match rx.try_recv() {
            Ok(Event::Ended { reason, member: m }) if m.as_deref().is_none_or(|m| m == member) => return Some(reason),
            Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => return None,
        }
    }
}

/// Whether the caller may have the queue's song `track` (or an album of it): a guest of this group, or an
/// account in it on any device. A guest gets 401 for anything else.
fn may_have(st: &AppState, who: &Who, id: &str, wanted: impl Fn(&gl::Entry) -> bool) -> ApiResult<()> {
    let inner = st.playback.groups.lock();
    let g = inner.groups.get(id);
    let member = match who {
        Who::Guest(x) => x.group == id && g.and_then(|g| g.member(&x.member)).is_some(),
        Who::Account(u) => g.is_some_and(|g| g.has_account(u.i("id"))),
    };
    let queued = g.is_some_and(|g| g.queue.iter().any(wanted));
    match (member && queued, who) {
        (true, _) => Ok(()),
        (false, Who::Guest(_)) => Err(err(401, "That song isn't in the group listen")),
        (false, Who::Account(_)) => Err(err(404, "Track not found")),
    }
}

/// GET /api/group/{id}/tracks/{track}/stream: a song of the group's queue (the library's stream route, with
/// the same query: `q`, `kbps`, `t`, `transcode`; server processing only for accounts allowed it).
async fn stream_track(State(st): State<AppState>, who: Who, Path((id, track)): Path<(String, i64)>, Query(q): Query<StreamQ>, req: Request) -> ApiResult<Response> {
    on(&st)?;
    may_have(&st, &who, &id, |e| e.track == track)?;
    let user = match &who {
        Who::Account(u) => Some(u.clone()),
        Who::Guest(_) => None,
    };
    crate::routes::library::stream_track(&st, track, &q, req, user.as_ref()).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CoverQ {
    s: u32,
    v: String,
}

/// GET /api/group/{id}/albums/{album}/cover?s=&v=: the cover of an album of the group's queue.
async fn cover(State(st): State<AppState>, who: Who, Path((id, album)): Path<(String, String)>, Query(q): Query<CoverQ>, req: Request) -> ApiResult<Response> {
    on(&st)?;
    may_have(&st, &who, &id, |e| e.row.s("album_id") == album)?;
    crate::routes::library::cover_response(&st, &album, q.s, &q.v, req).await
}

// ---- the loop ------------------------------------------------------------------------------------------------

/// Moves every group on (a song ended: the next one; a member gone 2 minutes, a guest whose 12 hours are over,
/// every guest once an admin turned guests off: out; idle 12 hours: ended) and saves what changed while
/// `group_listen_keep` is on. Runs beside the web server, four times a second.
pub async fn run(st: AppState) {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let s = st.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || tick_once(&s, false)).await {
            tracing::warn!("group listens: {e}");
        }
    }
}

/// Before a restart: every group saved now (while `group_listen_keep` is on).
pub async fn flush(st: AppState) {
    let _ = tokio::task::spawn_blocking(move || save(&st, true)).await;
}

/// One pass of [`run`]; `force` saves every group.
pub fn tick_once(st: &AppState, force: bool) {
    ready(st);
    if !st.cfg.bool("group_listen") {
        end_all(st, "off");
    }
    let (t, t_ms) = (now(), now_ms());
    let guests_on = guests_possible(st);
    let mut outs: Vec<(String, String, Out)> = vec![];
    let mut ended: Vec<String> = vec![];
    {
        let mut inner = st.playback.groups.lock();
        let ids: Vec<String> = inner.groups.keys().cloned().collect();
        for id in ids {
            let Some(g) = inner.groups.get_mut(&id) else { continue };
            if g.advance(t_ms) {
                g.changed(t);
            }
            if t - g.active_at >= gl::IDLE_SECS {
                inner.end(&id, "idle");
                ended.push(id);
                continue;
            }
            // (the reason its stream is told; None: dropped, it has no stream)
            let gone: Vec<(String, Option<&'static str>)> = g.members.iter()
                .filter_map(|m| {
                    let dropped = m.streams == 0 && m.gone_since.is_some_and(|s| t - s >= gl::DROP_SECS);
                    if m.expires_at.is_some_and(|e| t >= e) {
                        Some((m.member.clone(), Some("expired")))
                    } else if m.is_guest() && !guests_on {
                        Some((m.member.clone(), Some("removed")))
                    } else {
                        dropped.then(|| (m.member.clone(), None))
                    }
                })
                .collect();
            for (m, reason) in gone {
                if let (Some(reason), Some(g)) = (reason, inner.groups.get(&id)) {
                    let _ = g.tx.send(Event::Ended { reason, member: Some(m.clone()) });
                }
                let out = take_out(&mut inner, &id, &m);
                let over = out == Out::Ended;
                outs.push((id.clone(), m, out));
                if over {
                    break;
                }
            }
        }
    }
    for id in ended {
        drop_guest_tokens(st, &id, None);
    }
    for (id, m, out) in &outs {
        after_out(st, id, m, out);
    }
    save(st, force);
}

/// Writes the groups that are due ([`Group::save_due`], or all with `force`) and deletes the rows of the ones
/// that ended. With `group_listen_keep` off nothing is kept: what was saved is deleted.
fn save(st: &AppState, force: bool) {
    let (t, t_ms) = (now(), now_ms());
    let (groups, gone) = {
        let mut inner = st.playback.groups.lock();
        if !st.cfg.bool("group_listen_keep") {
            if !inner.saved.is_empty() || !inner.ended.is_empty() {
                inner.saved.clear();
                inner.ended.clear();
                drop(inner);
                if let Err(e) = gl::clear(&st.db) {
                    tracing::warn!("group listens: clearing the saved ones failed: {e}");
                }
            }
            return;
        }
        let mut groups = vec![];
        let mut ids = vec![];
        for g in inner.groups.values_mut() {
            if force || g.save_due(t) {
                groups.push(g.saved(t_ms));
                g.unsaved = None;
                g.saved_at = t;
                ids.push(g.id.clone());
            }
        }
        inner.saved.extend(ids);
        (groups, std::mem::take(&mut inner.ended))
    };
    if groups.is_empty() && gone.is_empty() {
        return;
    }
    if let Err(e) = gl::save(&st.db, &groups, &gone) {
        tracing::warn!("group listens: saving failed: {e}");
        // tried again at the next pass
        let mut inner = st.playback.groups.lock();
        for s in &groups {
            if let Some(g) = inner.groups.get_mut(&s.id) {
                g.unsaved.get_or_insert(t);
            }
        }
        inner.ended.extend(gone);
    }
}

#[cfg(test)]
#[path = "group_tests.rs"]
mod tests;
