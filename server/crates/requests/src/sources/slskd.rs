//! Soulseek for the worker, the picker and the Soulseek page: result ranking, and slskd's API –
//! the slskd Encore runs itself (`crate::slskd_managed`), on 127.0.0.1 with its own API key.
use crate::Ctx;
use crate::slskd_managed;
use encore_core::text::ascii_words;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::time::Instant;

use encore_core::audio::{DOWNLOAD_EXTS, DOWNLOAD_LOSSLESS};
pub const DONE_OK: &str = "Succeeded";
pub const DONE_BAD: [&str; 6] = ["Errored", "Rejected", "TimedOut", "Cancelled", "Aborted", "Failed"];
/// slskd runs but has no Soulseek server session yet (its API answers 409 to searches and transfers).
pub const NOT_CONNECTED: &str = "Soulseek isn't connected yet";

pub use crate::slskd_session::{Busy, NotConnected, Who};

/// Soulseek is on, has a login and its engine is installed – a source the worker can use.
pub fn enabled(st: &impl Ctx) -> bool {
    slskd_managed::configured(st) && slskd_managed::installed_version(st).is_some()
}

/// Why slskd can't answer right now, in words for the user (None while it runs or is starting).
pub fn not_ready(st: &impl Ctx) -> Option<String> {
    if !slskd_managed::configured(st) {
        return Some("Soulseek: switch it on and set a username and password in Settings → Plugins → Soulseek (an account is created on first login)".into());
    }
    let s = st.requests().slskd.status();
    match s["state"].as_str().unwrap_or("off") {
        "running" | "starting" => None,
        "not_installed" => Some(slskd_managed::NOT_INSTALLED.into()),
        "waiting_vpn" => Some(slskd_managed::WAITING_FOR_VPN.into()),
        "crashed" => Some(format!("Soulseek engine stopped: {}", s["error"].as_str().unwrap_or("unknown problem"))),
        "blocked" => Some(format!("Soulseek can't start – {}", s["error"].as_str().unwrap_or("see Settings → Plugins → Soulseek"))),
        _ if slskd_managed::installed_version(st).is_none() => Some(slskd_managed::NOT_INSTALLED.into()),
        _ => Some("Soulseek is starting – try again in a moment".into()),
    }
}

async fn req(st: &impl Ctx, method: reqwest::Method, path: &str, body: Option<Value>, query: &[(&str, &str)]) -> anyhow::Result<Value> {
    req_within(st, method, path, body, query, None).await
}

/// `timeout` overrides the http client's 30 s for calls slskd itself waits on a peer for.
async fn req_within(st: &impl Ctx, method: reqwest::Method, path: &str, body: Option<Value>, query: &[(&str, &str)], timeout: Option<std::time::Duration>) -> anyhow::Result<Value> {
    if let Some(why) = not_ready(st) {
        anyhow::bail!(why);
    }
    let (status, text) = slskd_managed::api(st, method, path, body.as_ref(), query, timeout).await?;
    if status >= 400 {
        return Err(http_error(status, &text));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// slskd's answer says the server session is down: 409, or the error Soulseek.NET throws when the session
/// ends between slskd's check and the call ("The server connection must be connected and logged in …").
pub fn says_not_connected(status: u16, text: &str) -> bool {
    status == 409 || (status >= 500 && text.contains("must be connected and logged in"))
}

/// slskd's answer turns a search away for being one too many: 429 ("Only one concurrent operation is
/// permitted"), or the words of another version ("Only one concurrent search").
pub fn says_too_many(status: u16, text: &str) -> bool {
    status == 429 || text.to_lowercase().contains("concurrent search")
}

/// An error answer from slskd's API as a message for the user: 409 means the engine has no server session
/// (not signed in to the Soulseek network yet: a [`NotConnected`]), anything else is reported with its
/// status and text.
pub fn http_error(status: u16, text: &str) -> anyhow::Error {
    if says_not_connected(status, text) {
        return NotConnected(NOT_CONNECTED.into()).into();
    }
    let reason = reqwest::StatusCode::from_u16(status).map(|s| s.to_string()).unwrap_or_else(|_| status.to_string());
    anyhow::anyhow!("Soulseek engine returned {reason}: {}", text.chars().take(200).collect::<String>())
}

/// slskd's /application: version, server.state ("Connected, LoggedIn"), shares, …
pub async fn status(st: &impl Ctx) -> anyhow::Result<Value> {
    req(st, reqwest::Method::GET, "application", None, &[]).await
}

pub fn search_timeout(st: &impl Ctx) -> i64 {
    st.cfg().int("slskd_search_timeout").max(5)
}

fn search_body(sid: &str, text: &str, timeout: i64) -> Value {
    json!({ "id": sid, "searchText": text, "searchTimeout": timeout * 1000, "responseLimit": 200, "fileLimit": 20000,
        "filterResponses": true, "minimumResponseFileCount": 1 })
}

/// The request worker's search: waits its turn (one search at a time, 3 s apart), never runs while the
/// session is down ([`NotConnected`]), and sits out "too many searches" answers. Returns the responses
/// once the search is over.
pub async fn search(st: &impl Ctx, text: &str) -> anyhow::Result<Vec<Value>> {
    search_as(st, text, Who::Worker, |_| {}).await
}

/// A person's search (the picker): goes before the worker's, and is refused at once when the session is
/// down ([`NotConnected`]) or can't start within 30 s ([`Busy`]).
pub async fn search_now(st: &impl Ctx, text: &str) -> anyhow::Result<Vec<Value>> {
    search_as(st, text, Who::Person, |_| {}).await
}

/// [`search`] for `who`. `note` says what the search is doing, for the worker's request: `None` when it
/// searches (its turn has come, or the server's pause is over: "Searching Soulseek…"), `Some(words)` while
/// it sits out a pause of the server's ("Rate limited by the Soulseek server – searching again at 14:05").
/// The answers of a search that ends well are kept for the picker ([`crate::search_cache`]).
pub async fn search_as(st: &impl Ctx, text: &str, who: Who, note: impl Fn(Option<&str>) + Send + Sync) -> anyhow::Result<Vec<Value>> {
    search_kept(st, text, who, None, note).await
}

/// [`search_as`], kept with the request it was made for (`request_id`), so the picker opened for that
/// request finds it. Errors and searches cut short are never kept.
pub async fn search_kept(st: &impl Ctx, text: &str, who: Who, request_id: Option<i64>, note: impl Fn(Option<&str>) + Send + Sync) -> anyhow::Result<Vec<Value>> {
    let res = search_live(st, text, who, request_id, note).await?;
    crate::search_cache::keep_slskd(st, text, request_id, &res).await;
    Ok(res)
}

/// The words Soulseek (and the indexers) are asked for a request's names: bracketed parts and symbols
/// out, the symbols that stand for letters spelled out first (Ke$ha → Kesha, P!nk → Pink; see
/// `torznab::fold_symbols`), so they get "Kesha Cannibal" and not "Ke ha Cannibal". The worker's searches
/// and the picker's first search use it, so the picker finds what the worker searched.
pub fn query_text(parts: &[&str]) -> String {
    static NW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\s']").unwrap());
    let s = parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" ");
    let s = super::torznab::fold_symbols(&s);
    let s = BRACKETS.replace_all(&s, " ");
    NW.replace_all(&s, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The search itself, behind [`search_kept`]. The worker's search for a request waits in the Soulseek line
/// under the request's id ([`crate::line`]), so Download now can move it up.
async fn search_live(st: &impl Ctx, text: &str, who: Who, request_id: Option<i64>, note: impl Fn(Option<&str>) + Send + Sync) -> anyhow::Result<Vec<Value>> {
    if let Some(why) = not_ready(st) {
        anyhow::bail!(why);
    }
    let session = st.requests().slskd.session();
    let turn = session.turn(st, who, request_id).await?;
    note(None);
    let deadline = turn.deadline();
    let timeout = search_timeout(st);
    let sid = uuid();
    let mut run = Running { st: st.clone(), sid: sid.clone(), turn: Some(turn), armed: true };
    if let Err(e) = post_search(st, &sid, text, timeout, deadline, &note).await {
        run.armed = false; // slskd didn't take it
        return Err(e);
    }
    let mut last = Value::Null;
    let mut gone = false;
    for _ in 0..timeout + 20 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match req(st, reqwest::Method::GET, &format!("searches/{sid}"), None, &[]).await {
            Ok(s) => {
                let done = s["isComplete"].as_bool().unwrap_or(false);
                last = s;
                if done {
                    break;
                }
            }
            Err(e) if engine_lost(&e) => {
                gone = true;
                break;
            }
            Err(_) => {}
        }
    }
    let res = if gone { None } else { Some(req(st, reqwest::Method::GET, &format!("searches/{sid}/responses"), None, &[]).await) };
    let _ = req(st, reqwest::Method::DELETE, &format!("searches/{sid}"), None, &[]).await;
    run.armed = false;
    drop(run);
    // the session ended under the search (slskd cancels the searches that run when it does, or slskd
    // itself went away): the caller waits for Soulseek instead of taking a cut-short answer for "nothing
    // found"
    let cut_short = last["state"].as_str().is_some_and(|s| s.contains("Cancelled") || s.contains("Errored"));
    match res {
        None => Err(session.search_found_it_down().into()),
        Some(Err(e)) if engine_lost(&e) => Err(session.search_found_it_down().into()),
        Some(_) if cut_short || not_ready(st).is_some() => Err(session.search_found_it_down().into()),
        Some(r) => Ok(r?.as_array().cloned().unwrap_or_default()),
    }
}

/// slskd went away under a search: it doesn't answer, or no longer knows the search.
fn engine_lost(e: &anyhow::Error) -> bool {
    e.is::<slskd_managed::Unreachable>() || e.to_string().contains("returned 404")
}

/// A search slskd may be running for Encore, with the turn it holds. When the caller goes away before
/// the search is over (a cancelled request, a closed picker), dropping this stops the search in slskd
/// before the turn passes on, so the next search never runs beside it.
struct Running<C: Ctx> {
    st: C,
    sid: String,
    turn: Option<crate::slskd_session::Turn>,
    /// the search may be running in slskd (posted, or on its way)
    armed: bool,
}

impl<C: Ctx> Drop for Running<C> {
    fn drop(&mut self) {
        let Some(turn) = self.turn.take() else { return };
        if !self.armed {
            return;
        }
        let (st, sid) = (self.st.clone(), self.sid.clone());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                stop_search(&st, &sid).await;
                drop(turn);
            });
        }
    }
}

/// Stop a search in slskd and forget it (slskd's DELETE alone forgets a search that goes on running).
async fn stop_search(st: &impl Ctx, sid: &str) {
    let _ = req(st, reqwest::Method::PUT, &format!("searches/{sid}"), None, &[]).await;
    let _ = req(st, reqwest::Method::DELETE, &format!("searches/{sid}"), None, &[]).await;
}

/// POST /searches, holding the turn: after the 3 s spacing and any rate-limit pause, only while logged in.
/// "Too many searches" pauses searches for 60 s: the worker tries again (5 times at most, saying so with
/// `note`), a person (with a `deadline`) is told when the next search can go ([`Busy`]). "Not connected",
/// or slskd not answering, marks the session down and wakes its supervisor ([`NotConnected`]).
async fn post_search(st: &impl Ctx, sid: &str, text: &str, timeout: i64, deadline: Option<Instant>, note: &(impl Fn(Option<&str>) + Sync)) -> anyhow::Result<()> {
    let session = st.requests().slskd.session();
    let body = search_body(sid, text, timeout);
    let who = if deadline.is_some() { Who::Person } else { Who::Worker };
    let mut limited = 0;
    loop {
        // the worker's request says it sits out the server's pause, and searches again after it
        let paused = session.rate_limited_until().filter(|_| who == Who::Worker);
        if let Some(until) = paused {
            note(Some(&format!("Rate limited by the Soulseek server – searching again at {}", crate::slskd_session::clock(crate::slskd_session::unix_at(until)))));
        }
        session.before_search(st, deadline).await?;
        if paused.is_some() {
            note(None);
        }
        let (status, answer) = match slskd_managed::api(st, reqwest::Method::POST, "searches", Some(&body), &[], None).await {
            Ok(a) => a,
            Err(e) if e.is::<slskd_managed::Unreachable>() => return Err(session.search_found_it_down().into()),
            Err(e) => return Err(e),
        };
        if status < 400 {
            return Ok(());
        }
        if says_too_many(status, &answer) {
            let busy = session.rate_limited();
            limited += 1;
            if who == Who::Person {
                return Err(busy.into());
            }
            if limited > crate::slskd_session::RATE_LIMIT_TRIES {
                anyhow::bail!("the Soulseek server kept turning searches away (rate limited) – try again later");
            }
            continue;
        }
        if says_not_connected(status, &answer) {
            return Err(session.search_found_it_down().into());
        }
        return Err(http_error(status, &answer));
    }
}

fn uuid() -> String {
    let b: [u8; 16] = rand::random();
    let h = hex::encode(b);
    format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

/// Start a person's search for a page to poll with `search_poll`; it runs for `slskd_search_timeout`
/// seconds and holds the turn until slskd says it is over (or it is ended), so the next search waits.
/// Refused at once when the session is down ([`NotConnected`]) or the turn doesn't come within 30 s
/// ([`Busy`]).
pub async fn search_start(st: &impl Ctx, text: &str) -> anyhow::Result<String> {
    if let Some(why) = not_ready(st) {
        anyhow::bail!(why);
    }
    let turn = st.requests().slskd.session().turn(st, Who::Person, None).await?;
    let deadline = turn.deadline();
    let sid = uuid();
    let timeout = search_timeout(st);
    let mut run = Running { st: st.clone(), sid: sid.clone(), turn: Some(turn), armed: true };
    if let Err(e) = post_search(st, &sid, text, timeout, deadline, &|_| {}).await {
        run.armed = false;
        return Err(e);
    }
    let st2 = st.clone();
    tokio::spawn(async move {
        for _ in 0..timeout + 20 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            match req(&st2, reqwest::Method::GET, &format!("searches/{}", run.sid), None, &[]).await {
                Ok(s) if s["isComplete"].as_bool().unwrap_or(false) => {
                    run.armed = false;
                    break;
                }
                // ended (search_end stopped it first) or forgotten
                Err(e) if e.to_string().contains("returned 404") => {
                    run.armed = false;
                    break;
                }
                _ => {}
            }
        }
        drop(run); // still running after all that time: stopped before the turn passes on
    });
    Ok(sid)
}

/// The responses a search has so far and whether it is over ("search expired" once slskd forgot it).
pub async fn search_poll(st: &impl Ctx, id: &str) -> anyhow::Result<(Vec<Value>, bool)> {
    search_poll_whole(st, id).await.map(|(res, done, _)| (res, done))
}

/// [`search_poll`], and whether a search that is over ran to its end – not cut short by the session
/// ending under it (slskd cancels the searches then): only such an answer is kept for the picker.
pub async fn search_poll_whole(st: &impl Ctx, id: &str) -> anyhow::Result<(Vec<Value>, bool, bool)> {
    let s = req(st, reqwest::Method::GET, &format!("searches/{id}"), None, &[]).await
        .map_err(|e| if e.to_string().contains("returned 404") { anyhow::anyhow!("search expired") } else { e })?;
    let res = req(st, reqwest::Method::GET, &format!("searches/{id}/responses"), None, &[]).await?;
    let done = s["isComplete"].as_bool().unwrap_or(false);
    let cut_short = s["state"].as_str().is_some_and(|x| x.contains("Cancelled") || x.contains("Errored"));
    Ok((res.as_array().cloned().unwrap_or_default(), done, done && !cut_short && not_ready(st).is_none()))
}

/// End a polled search: stopped if it still runs, then forgotten (slskd keeps them until told).
pub async fn search_end(st: &impl Ctx, id: &str) {
    stop_search(st, id).await;
}

/// A user's share list: [{ name, files: [{ filename (full path), size, bitRate, … }] }].
pub async fn browse(st: &impl Ctx, user: &str) -> anyhow::Result<Vec<Value>> {
    let b = req_within(st, reqwest::Method::GET, &format!("users/{}/browse", urlencoding::encode(user)), None, &[], Some(std::time::Duration::from_secs(50))).await
        .map_err(|e| if e.to_string().contains("timed out") { anyhow::anyhow!("{user} did not send their file list within 45 s") } else { e })?;
    // most clients list bare file names under the folder; downloads need the full remote path
    let mut dirs = b["directories"].as_array().cloned().unwrap_or_default();
    for d in &mut dirs {
        let dir = d["name"].as_str().unwrap_or("").trim_end_matches('\\').to_string();
        if let Some(files) = d["files"].as_array_mut() {
            for f in files {
                let name = f["filename"].as_str().unwrap_or("").to_string();
                if !name.contains('\\') && !name.contains('/') && !dir.is_empty() {
                    f["filename"] = json!(format!("{dir}\\{name}"));
                }
            }
        }
    }
    Ok(dirs)
}

/// How long slskd may take to take a download: it asks the Soulseek server for the user's address, then
/// the user's client (30 s to connect). Longer than the client's 30 s, so Encore's turn to enqueue covers
/// all of slskd's work.
const ENQUEUE_WAIT: Duration = Duration::from_secs(90);
/// slskd is taking other downloads (429): ask again this long after, this many times.
pub const ENQUEUE_PAUSE: Duration = Duration::from_secs(2);
pub const ENQUEUE_TRIES: u32 = 5;

/// slskd kept answering that it is taking other downloads (429 "Only one concurrent operation is
/// permitted"): the download was not refused by the user, and is asked for again later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueBusy(pub String);

impl std::fmt::Display for EnqueueBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EnqueueBusy {}

/// Ask slskd to download `files` from `username`. One at a time (slskd 0.26 takes two at once and answers
/// 429 to a third, for as long as it waits on the server and the user's client); a 429 all the same is
/// asked again 2 s later, five times, then [`EnqueueBusy`]. A session that is down is [`NotConnected`].
pub async fn enqueue(st: &impl Ctx, username: &str, files: &[Value]) -> anyhow::Result<()> {
    if let Some(why) = not_ready(st) {
        anyhow::bail!(why);
    }
    let body = json!(files.iter().map(|f| json!({ "filename": f["filename"], "size": f["size"] })).collect::<Vec<_>>());
    let path = format!("transfers/downloads/{}", urlencoding::encode(username));
    let _one = st.requests().slskd.session().enqueue_turn().await;
    let mut busy = 0;
    loop {
        let (status, text) = slskd_managed::api(st, reqwest::Method::POST, &path, Some(&body), &[], Some(ENQUEUE_WAIT)).await?;
        if status < 400 {
            return Ok(());
        }
        if !says_too_many(status, &text) {
            return Err(http_error(status, &text));
        }
        busy += 1;
        if busy > ENQUEUE_TRIES {
            return Err(EnqueueBusy(format!("the Soulseek engine is busy taking other downloads ({})", text.chars().take(120).collect::<String>())).into());
        }
        tokio::time::sleep(ENQUEUE_PAUSE).await;
    }
}

/// How long a user is asked about their upload slots, or a download's place in their queue.
pub const USER_WAIT: Duration = Duration::from_secs(10);

/// What the user's client says about it (GET users/{u}/info, 10 s at most): `hasFreeUploadSlot`,
/// `queueLength`, `uploadSlots`. An offline user, or one whose client doesn't answer, is an error.
pub async fn user_info(st: &impl Ctx, username: &str) -> anyhow::Result<Value> {
    let path = format!("users/{}/info", urlencoding::encode(username));
    match tokio::time::timeout(USER_WAIT, req_within(st, reqwest::Method::GET, &path, None, &[], Some(USER_WAIT))).await {
        Ok(r) => r,
        Err(_) => anyhow::bail!("{username} did not answer within {} s", USER_WAIT.as_secs()),
    }
}

/// A queued download's place in the user's upload queue, asked of the user (GET
/// transfers/downloads/{u}/{id}/position, 10 s at most): slskd answers the place, or the transfer with its
/// `placeInQueue`. None when it couldn't be asked.
pub async fn place_in_queue(st: &impl Ctx, username: &str, id: &str) -> Option<u64> {
    let path = format!("transfers/downloads/{}/{}/position", urlencoding::encode(username), urlencoding::encode(id));
    let v = tokio::time::timeout(USER_WAIT, req_within(st, reqwest::Method::GET, &path, None, &[], Some(USER_WAIT))).await.ok()?.ok()?;
    v.as_u64().or_else(|| v["placeInQueue"].as_u64()).filter(|p| *p > 0)
}

pub async fn user_downloads(st: &impl Ctx, username: &str) -> Vec<Value> {
    let Ok(d) = req(st, reqwest::Method::GET, &format!("transfers/downloads/{}", urlencoding::encode(username)), None, &[]).await else { return vec![] };
    d["directories"].as_array().cloned().unwrap_or_default().iter().flat_map(|d| d["files"].as_array().cloned().unwrap_or_default()).collect()
}

fn tail(s: &str) -> String {
    s.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string()
}

/// slskd's per-user transfer lists, flattened: one entry per file.
fn flatten_transfers(d: &Value) -> Vec<Value> {
    let mut out = vec![];
    for u in d.as_array().cloned().unwrap_or_default() {
        for dir in u["directories"].as_array().cloned().unwrap_or_default() {
            for f in dir["files"].as_array().cloned().unwrap_or_default() {
                // path/bytes/error: the full remote name and the raw counters, for the Soulseek page
                out.push(json!({ "id": f["id"], "username": u["username"], "filename": tail(f["filename"].as_str().unwrap_or("")),
                    "directory": tail(dir["directory"].as_str().unwrap_or("")), "state": f["state"].as_str().unwrap_or(""),
                    "progress": (f["percentComplete"].as_f64().unwrap_or(0.0) * 10.0).round() / 10.0, "size": f["size"], "speed": f["averageSpeed"],
                    "path": f["filename"], "bytes": f["bytesTransferred"], "error": f["exception"] }));
            }
        }
    }
    out
}

pub async fn all_downloads(st: &impl Ctx) -> anyhow::Result<Vec<Value>> {
    Ok(flatten_transfers(&req(st, reqwest::Method::GET, "transfers/downloads", None, &[]).await?))
}

/// Other people fetching from the shared library.
pub async fn all_uploads(st: &impl Ctx) -> anyhow::Result<Vec<Value>> {
    Ok(flatten_transfers(&req(st, reqwest::Method::GET, "transfers/uploads", None, &[]).await?))
}

pub async fn cancel(st: &impl Ctx, username: &str, file_id: &str) {
    let _ = req(st, reqwest::Method::DELETE, &format!("transfers/downloads/{}/{file_id}", urlencoding::encode(username)), None, &[("remove", "true")]).await;
}

pub async fn clear_completed(st: &impl Ctx) -> anyhow::Result<()> {
    req(st, reqwest::Method::DELETE, "transfers/downloads/all/completed", None, &[]).await.map(|_| ())
}

// ---- ranking ------------------------------------------------------------------------------------

static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[].*?[\)\]]").unwrap());
static ALBUM_BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix|karaoke|instrumental|8d|nightcore)\b").unwrap());
static ALBUM_OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix|karaoke|instrumental)\b").unwrap());
static TRACK_BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix|karaoke|instrumental|8d|nightcore|cover)\b").unwrap());
static TRACK_OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix|instrumental)\b").unwrap());

/// The words of a name without anything in brackets ([`encore_core::text::ascii_words`]).
fn plain_words(s: &str) -> String {
    ascii_words(&BRACKETS.replace_all(&s.to_lowercase(), " "))
}

pub fn ext(f: &str) -> String {
    f.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

fn contains(hay: &str, needle: &str) -> f64 {
    let n = plain_words(needle);
    let words: Vec<&str> = n.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let h = format!(" {} ", plain_words(hay));
    words.iter().filter(|w| h.contains(&format!(" {w} "))).count() as f64 / words.len() as f64
}

fn format_score(st: &impl encore_http::Ctx, files: &[&Value], lossless_only: bool) -> Option<f64> {
    let pref = st.cfg().str("slskd_preferred_format");
    let min_br = st.cfg().int("slskd_min_bitrate");
    let exts: std::collections::HashSet<String> = files.iter().map(|f| ext(f["filename"].as_str().unwrap_or(""))).collect();
    if exts.len() != 1 {
        return None;
    }
    let e = exts.into_iter().next().unwrap();
    let lossless = DOWNLOAD_LOSSLESS.contains(&e.as_str());
    if lossless_only && !lossless {
        return None;
    }
    if lossless {
        let hires = files.iter().all(|f| f["bitDepth"].as_i64().unwrap_or(16) >= 24);
        let base = match pref.as_str() { "flac" => 30.0, "mp3" => 8.0, _ => 25.0 };
        return Some(base + if hires && pref != "mp3" { 6.0 } else { 0.0 });
    }
    let br = files.iter().map(|f| f["bitRate"].as_i64().unwrap_or(0)).min().unwrap_or(0);
    if br > 0 && br < min_br {
        return None;
    }
    let mut base = if e == "mp3" { match pref.as_str() { "flac" => 10.0, "mp3" => 30.0, _ => 22.0 } } else { 12.0 };
    if br >= 320 {
        base += 5.0;
    } else if br > 0 && br < 256 {
        base -= 6.0;
    }
    Some(base)
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub fn rank_album(st: &impl encore_http::Ctx, responses: &[Value], artist: &str, album: &str, track_count: Option<usize>, lossless_only: bool) -> Vec<Value> {
    rank_album_where(st, responses, artist, album, track_count, lossless_only, |_| true)
}

/// [`rank_album`] of only the folders `keep` lets through – left out before each user's best folder is chosen
/// and before the cut to `slskd_max_candidates`, so what is left out can't take the places of folders that qualify.
pub fn rank_album_where(st: &impl encore_http::Ctx, responses: &[Value], artist: &str, album: &str, track_count: Option<usize>, lossless_only: bool,
                        keep: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut cands = vec![];
    for resp in responses {
        let mut dirs: Vec<(String, Vec<&Value>)> = vec![];
        for f in resp["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let name = f["filename"].as_str().unwrap_or("");
            if !DOWNLOAD_EXTS.contains(&ext(name).as_str()) {
                continue;
            }
            let n = name.replace('\\', "/");
            let d = n.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
            match dirs.iter_mut().find(|(k, _)| *k == d) { Some((_, v)) => v.push(f), None => dirs.push((d, vec![f])) }
        }
        for (d, mut files) in dirs {
            let Some(fmt) = format_score(st, &files, lossless_only) else { continue };
            let n = files.len();
            let mut score = fmt;
            if let Some(tc) = track_count.filter(|t| *t > 0) {
                if n < tc || n > tc * 2 + 2 {
                    continue;
                }
                score += if n == tc { 25.0 } else { (15.0 - (n - tc) as f64 * 3.0).max(0.0) };
            }
            let parts: Vec<&str> = d.split('/').collect();
            let path_tail = parts[parts.len().saturating_sub(2)..].join("/");
            let album_hit = contains(&path_tail, album);
            if album_hit < 0.5 {
                continue;
            }
            score += album_hit * 30.0 + contains(&d, artist) * 10.0;
            if ALBUM_BAD.is_match(&path_tail) && !ALBUM_OK.is_match(album) {
                score -= 15.0;
            }
            if resp["hasFreeUploadSlot"].as_bool().unwrap_or(false) {
                score += 8.0;
            }
            score -= resp["queueLength"].as_f64().unwrap_or(0.0).min(50.0) / 5.0;
            score += (resp["uploadSpeed"].as_f64().unwrap_or(0.0) / 1_000_000.0).min(10.0);
            files.sort_by_key(|f| f["filename"].as_str().unwrap_or("").to_string());
            cands.push(json!({
                "username": resp["username"], "directory": d,
                "files": files.iter().map(|f| json!({ "filename": f["filename"], "size": f["size"] })).collect::<Vec<_>>(),
                "format": ext(files[0]["filename"].as_str().unwrap_or("")),
                "bitrate": files.iter().map(|f| f["bitRate"].as_i64().unwrap_or(0)).min().unwrap_or(0),
                "score": round1(score) }));
        }
    }
    cands.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    cands.retain(&keep);
    let mut seen = std::collections::HashSet::new();
    cands.retain(|c| seen.insert(c["username"].as_str().unwrap_or("").to_string()));
    cands.truncate(st.cfg().int("slskd_max_candidates").max(1) as usize);
    cands
}

pub fn rank_track(st: &impl encore_http::Ctx, responses: &[Value], artist: &str, title: &str, length: Option<f64>, lossless_only: bool) -> Vec<Value> {
    rank_track_where(st, responses, artist, title, length, lossless_only, |_| true)
}

/// [`rank_track`] of only the files `keep` lets through – left out before the cut to `slskd_max_candidates`.
pub fn rank_track_where(st: &impl encore_http::Ctx, responses: &[Value], artist: &str, title: &str, length: Option<f64>, lossless_only: bool,
                        keep: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut cands = vec![];
    for resp in responses {
        for f in resp["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let raw = f["filename"].as_str().unwrap_or("");
            if !DOWNLOAD_EXTS.contains(&ext(raw).as_str()) {
                continue;
            }
            let Some(fmt) = format_score(st, &[f], lossless_only) else { continue };
            let name = raw.replace('\\', "/");
            let (dir, base) = name.rsplit_once('/').unwrap_or(("", &name));
            let t = contains(base, title);
            if t < 0.6 {
                continue;
            }
            // another version than the one asked for (live, remix, sped up …): never picked
            // (from the title on: "Artist - Album - NN - Title" has the album name before it)
            let tail = encore_library::matcher::from_title_on(title, base.rsplit_once('.').map(|x| x.0).unwrap_or(base));
            if encore_library::matcher::unwanted_version(title, artist, &tail).is_some() {
                continue;
            }
            let mut score = fmt + t * 40.0 + contains(&name, artist) * 15.0;
            if TRACK_BAD.is_match(base) && !TRACK_OK.is_match(title) {
                score -= 20.0;
            }
            if let (Some(l), Some(fl)) = (length, f["length"].as_f64()) {
                let diff = (fl - l).abs();
                score += if diff < 5.0 { 10.0 } else if diff > 20.0 { -10.0 } else { 0.0 };
            }
            if resp["hasFreeUploadSlot"].as_bool().unwrap_or(false) {
                score += 8.0;
            }
            score -= resp["queueLength"].as_f64().unwrap_or(0.0).min(50.0) / 5.0;
            cands.push(json!({ "username": resp["username"], "directory": dir, "files": [{ "filename": f["filename"], "size": f["size"] }],
                "format": ext(raw), "bitrate": f["bitRate"].as_i64().unwrap_or(0), "score": round1(score) }));
        }
    }
    cands.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    cands.retain(&keep);
    cands.truncate(st.cfg().int("slskd_max_candidates").max(1) as usize);
    cands
}

/// Locate a finished download on the shared volume: under the folder slskd downloads into right now (the
/// fallback under <config>/downloads/slskd when the configured one can't be written), not the setting.
pub fn local_path(st: &impl Ctx, remote: &str) -> Option<std::path::PathBuf> {
    local_path_in(&crate::slskd_managed::download_dir(st), remote)
}

/// `local_path` under the downloads folder `root`.
pub fn local_path_in(root: &std::path::Path, remote: &str) -> Option<std::path::PathBuf> {
    let n = remote.replace('\\', "/");
    let parts: Vec<&str> = n.split('/').collect();
    let base = *parts.last()?;
    let parent = if parts.len() > 1 { parts[parts.len() - 2] } else { "" };
    for c in [root.join(parent).join(base), root.join(base)] {
        if c.is_file() {
            return Some(c);
        }
    }
    // never a half-downloaded copy from slskd's incomplete folder (downloads/.incomplete)
    walkdir::WalkDir::new(root).into_iter().filter_entry(|e| e.depth() == 0 || e.file_name() != ".incomplete").flatten()
        .find(|e| e.file_type().is_file() && e.file_name() == base).map(|e| e.into_path())
}
