//! Download now (the owner on Oct 21: "for the que can i get a download now button"): a waiting or queued
//! request starts right away and skips the line, and a Soulseek download stuck in someone's upload queue
//! switches to a user with a free slot.
//!
//! The rule over all of it: Download now never breaks a limit or a ban. It only moves a request to the front
//! of its source's line ([`crate::line`]), makes a timer due now where nothing outside Encore asked it to
//! wait (the storage's 10 minutes, the downloads folders' probe, Encore's own Soulseek schedule – never
//! sooner than every 2 minutes, never where the Soulseek server asked Encore to wait), swaps a remote
//! Soulseek queue for a user with a free upload slot under the worker's own ranking rules, or gives up a
//! torrent or Usenet download that has no progress and goes to the request's next source. It never gives a
//! source more slots, never approves, never tries a blocked file, user or release again and never turns the
//! audio check off.
//!
//! [`offer`] decides, for the list (`can_download_now`, `download_now` in `GET /api/requests`, through
//! [`row_fields`]) and for `POST /api/requests/{id}/download-now` ([`download_now`]) alike; [`download_all_now`]
//! is the admins' Download all now.
use super::*;
use crate::line::{self, Place};
use crate::slskd_session::{self as session, AttemptNow, Reason, State as SState};
use std::time::Duration;

/// How long Download now waits for a request's running step to end before it answers "busy".
pub const IDLE_WAIT: Duration = Duration::from_secs(15);
/// How long it watches a request it woke before it answers.
pub const WATCH: Duration = Duration::from_secs(10);
/// How long it waits for a switch to a user with a free upload slot before it answers "switching".
pub const SWITCH_WAIT: Duration = Duration::from_secs(20);
/// A torrent or Usenet download offers the next source after this long without progress (seconds).
pub const NO_PROGRESS: f64 = 300.0;
/// How many users with a free slot are asked, best first, before the switch gives up.
const ASK_USERS: usize = 3;

pub const BUSY: &str = "The request is busy – try again in a moment";
pub const CHANGED: &str = "The request changed meanwhile – try again";
pub const ONLY_WAITING: &str = "Only a request that waits can be downloaded now";
pub const PENDING: &str = "It waits for an admin's approval";
pub const SEARCHING: &str = "It is searching now";
pub const DOWNLOADING: &str = "It is downloading now";
pub const IMPORTING: &str = "It is being imported now";
pub const IMPORT_AGAIN: &str = "Its files are downloaded – Import again tries the import now";
pub const NO_NEXT_SOURCE: &str = "No other source to try – the download keeps waiting for seeds";
pub const STARTED_MEANWHILE: &str = "It started downloading meanwhile";
pub const STARTING: &str = "Starting now";

/// Where Soulseek stands for a request waiting for it.
#[derive(Clone, Debug, PartialEq)]
pub enum Sk {
    LoggedIn,
    Connecting,
    /// Encore's own schedule holds it (a lost connection, nothing planned): an attempt can be made due
    Attempt,
    /// the server isn't answering logins: Encore tries again then (unix seconds) so it isn't shut out
    Timeout(f64),
    /// another client logged in with the username: Encore tries again then
    Kicked(f64),
    /// Encore doesn't connect (why): the request goes on to its next source
    Stopped(String),
    /// the VPN reconnects
    Vpn,
    /// the Soulseek engine is starting (or starting again)
    Engine,
}

/// What Download now does for a request.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// move it to the front of its line
    Line,
    /// a request queued but in no line: the queue's loop looks at once (no button: it starts within 5 s)
    Start,
    /// a download waiting for its staging folder: the storage checked now
    Storage,
    /// a request waiting for a downloads folder: the folders probed now
    Disk,
    /// a request waiting for Soulseek
    Soulseek(Sk),
    /// a Soulseek download queued at the user: a user with a free upload slot
    Switch,
    /// a torrent or Usenet download with no progress: the next source (its words), after `minutes`
    NextSource { next: String, minutes: i64 },
}

/// The button a request row offers, and what pressing it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub act: Act,
    /// the button's tooltip ("Go first in line for Deezer")
    pub title: String,
    /// the time a press that can't make it sooner would name (Soulseek's next attempt), unix seconds
    pub until: Option<f64>,
    /// the row shows the button (a queued request in no line starts within 5 s: no button, but a press works)
    pub button: bool,
}

impl Offer {
    fn new(act: Act, title: impl Into<String>) -> Offer {
        Offer { act, title: title.into(), until: None, button: true }
    }

    /// `download_now.act` in the list: line, wake, switch or next_source.
    pub fn kind(&self) -> &'static str {
        match self.act {
            Act::Line => "line",
            Act::Switch => "switch",
            Act::NextSource { .. } => "next_source",
            _ => "wake",
        }
    }
}

fn no(text: impl Into<String>) -> Result<Offer, (u16, String)> {
    Err((409, text.into()))
}

/// The time of day of a unix time, on this server's clock ("14:20").
fn clock(t: f64) -> String {
    session::clock(t)
}

/// Where Soulseek stands now, for a request that waits for it.
pub fn soulseek_state(st: &impl Ctx) -> Sk {
    if let Some(why) = slskd::not_ready(st) {
        // the worker goes on to the next source for all but the first two; the words say why, as the card does
        return match st.requests().slskd.state() {
            "waiting_vpn" => Sk::Vpn,
            "crashed" => Sk::Engine,
            _ if !crate::slskd_managed::configured(st) => Sk::Stopped("Soulseek is off".into()),
            "blocked" => Sk::Stopped(why),
            _ if why == crate::slskd_managed::NOT_INSTALLED => Sk::Stopped("Soulseek is not installed".into()),
            _ => Sk::Stopped("the Soulseek engine isn't running".into()),
        };
    }
    if st.requests().slskd.state() != "running" {
        return Sk::Engine;
    }
    let c = st.requests().slskd.session().snapshot();
    let next = |c: &session::Conn| c.next_at.map(session::unix_at).unwrap_or_else(now);
    match (c.state, c.reason) {
        (SState::LoggedIn, _) if !c.lost => Sk::LoggedIn,
        (SState::LoggedIn | SState::Connecting, _) => Sk::Connecting,
        (SState::Stopped, Some(r)) => Sk::Stopped(session::describe(r, c.message.as_deref())),
        (SState::Stopped, None) => Sk::Stopped(c.message.clone().unwrap_or_else(|| "Soulseek is off".into())),
        (SState::Waiting, Some(Reason::Timeout)) => Sk::Timeout(next(&c)),
        (SState::Waiting, Some(Reason::Kicked)) => Sk::Kicked(next(&c)),
        _ => Sk::Attempt,
    }
}

/// The offer for a request waiting for Soulseek (§1.2's table): every state has the button; its title says
/// when a press can't make it sooner.
fn soulseek_offer(st: &impl Ctx) -> Offer {
    let sk = soulseek_state(st);
    let (title, until) = match &sk {
        Sk::LoggedIn => ("Search Soulseek now".to_string(), None),
        Sk::Connecting => ("Search Soulseek once it is connected".to_string(), None),
        Sk::Attempt => {
            // what a press would get, without pressing
            let until = match st.requests().slskd.session().snapshot().attempt_now(tokio::time::Instant::now()) {
                AttemptNow::Later(t) => Some(session::unix_at(t)),
                _ => None,
            };
            ("Connect to Soulseek now and search".to_string(), until)
        }
        Sk::Timeout(at) | Sk::Kicked(at) => (format!("Soulseek asked Encore to wait – next attempt at {}", clock(*at)), Some(*at)),
        Sk::Stopped(_) => ("Try the next source now".to_string(), None),
        Sk::Vpn | Sk::Engine => ("Search Soulseek once it is back".to_string(), None),
    };
    Offer { act: Act::Soulseek(sk), title, until, button: true }
}

/// The source a request would try next, in words ("Deezer", "Soulseek (lossless)").
pub fn source_words(st: &impl Ctx, src: &str) -> String {
    match src {
        "slskd" => "Soulseek".into(),
        "slskd_lossless" => "Soulseek (lossless)".into(),
        "streamrip" => line::streamrip_label(None, &streamrip::services(st)),
        "ytdlp" => line::YOUTUBE.into(),
        "torznab" => line::TORRENTS.into(),
        // a plugin program's source: its name ("Internet Archive")
        other => crate::sources::plugin::find(st, other).map(|p| p.name).unwrap_or_else(|| other.to_string()),
    }
}

/// A Soulseek download whose every file waits in the user's own upload queue and of which nothing arrived
/// (`detail.slskd.remote`, kept by the poll); slskd's own queue and a download it hasn't taken yet are not.
fn remote_queued(r: &Req) -> bool {
    r.s("status") == "downloading" && r.s("source").starts_with("slskd") && r.d["slskd"]["remote"].is_object()
        && !r.d["slskd"]["pending"].is_object() && r.d["slskd"]["last_bytes"].as_f64().unwrap_or(0.0) <= 0.0
}

/// The user whose upload queue a download waits in.
fn current_candidate(r: &Req) -> Option<Value> {
    let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(-1);
    r.d["slskd"]["candidates"].get(idx.max(0) as usize).filter(|_| idx >= 0).cloned()
}

/// Files a person chose: a hand pick (the picker, Download anyway) or a download with the audio check off.
/// Download now never swaps them for another user's files – those would come in unchecked, or as nobody's pick.
fn chosen_files(r: &Req) -> bool {
    r.d["pick"].is_object() || rejected::skips_check(&r.d)
}

/// "12th in alice's queue", "in alice's queue": where a download queued at its user waits.
fn queue_at(r: &Req) -> String {
    let user = current_candidate(r).and_then(|c| c["username"].as_str().map(String::from)).unwrap_or_default();
    match r.d["slskd"]["remote"]["place"].as_u64() {
        Some(p) => format!("{} in {user}'s queue", line::ordinal(p as usize)),
        None => format!("in {user}'s queue"),
    }
}

/// Why a download of chosen files ([`chosen_files`]) isn't switched to another user.
fn chosen_text(r: &Req) -> String {
    let at = queue_at(r);
    if r.d["skip_check"]["why"] == "download" {
        format!("Download anyway asked for exactly these files – it waits {at}")
    } else {
        format!("It was picked by hand – it waits {at}; pick another file in the picker to change it")
    }
}

/// "12th in alice's upload queue", "in alice's upload queue".
fn remote_text(user: &str, place: Option<u64>) -> String {
    match place {
        Some(p) => format!("{} in {user}'s upload queue", line::ordinal(p as usize)),
        None => format!("in {user}'s upload queue"),
    }
}

/// "it stays 12th in alice's queue", "it stays in alice's queue".
fn stays(user: &str, place: Option<u64>) -> String {
    match place {
        Some(p) => format!("it stays {} in {user}'s queue", line::ordinal(p as usize)),
        None => format!("it stays in {user}'s queue"),
    }
}

/// A torrent or Usenet download that has had no progress for [`NO_PROGRESS`]: its client answers, it isn't
/// in the client's own queue, it was added two minutes ago or more, and the client isn't busy with the files
/// it has (`torznab.busy`, kept by the poll: it checks, repairs, unpacks or moves them – a finished download,
/// which a press would throw away). → since when (unix seconds)
pub fn stalled_since(r: &Req, t: f64) -> Option<f64> {
    let tz = &r.d["torznab"];
    let ok = matches!(r.s("status").as_str(), "downloading" | "monitoring") && r.s("source") == "torznab" && tz["id"].is_string()
        && tz["err_since"].is_null() && tz["queued_since"].is_null() && tz["busy"].is_null() && t - tz["started"].as_f64().unwrap_or(t) >= CLIENT_TAKES;
    let last = tz["last_change"].as_f64()?;
    (ok && t - last >= NO_PROGRESS).then_some(last)
}

fn torrent_offer(st: &impl Ctx, r: &Req) -> Result<Offer, (u16, String)> {
    let tz = &r.d["torznab"];
    if tz["id"].is_string() && tz["err_since"].is_null() && !tz["queued_since"].is_null() {
        return no(format!("It waits in {}'s own queue – Encore doesn't change the client's limit", torznab_client(st, r)));
    }
    if tz["id"].is_string() && tz["err_since"].is_null() && !tz["busy"].is_null() {
        return no(format!("{} is busy with the downloaded files – it is imported once they are ready", torznab_client(st, r)));
    }
    let t = now();
    let Some(since) = stalled_since(r, t) else { return no(DOWNLOADING) };
    let Some(next) = r.d["sources"].as_array().and_then(|a| a.first()).and_then(|x| x.as_str()).map(|s| source_words(st, s)) else {
        return no(NO_NEXT_SOURCE);
    };
    let minutes = ((t - since) / 60.0).round() as i64;
    Ok(Offer::new(Act::NextSource { next: next.clone(), minutes }, format!("Stop this download (no progress for {minutes} minutes) and try the next source: {next}")))
}

/// What Download now offers for a request (Ok: the button and what it does; Err: no button, and the HTTP
/// status and words a press is answered with). The list and the press both ask this.
pub fn offer(st: &impl Ctx, r: &Req) -> Result<Offer, (u16, String)> {
    let status = r.s("status");
    match status.as_str() {
        "pending" => return no(PENDING),
        s if !ACTIVE.contains(&s) => return no(ONLY_WAITING),
        _ => {}
    }
    let lines = &st.requests().lines;
    if let Some(p) = lines.place(r.id) {
        return Ok(Offer::new(Act::Line, format!("Go first in line for {}", p.label)));
    }
    if lines.soulseek.holds(r.id) && matches!(status.as_str(), "searching" | "queued") {
        // its turn in the line came: it waits for Soulseek's own turn (a person's search, the 3 s spacing, the
        // server's pause)
        let pause = st.requests().slskd.session().rate_limited_until().map(|t| format!(" – next search at {}", clock(session::unix_at(t)))).unwrap_or_default();
        return no(format!("It searches as soon as Soulseek allows{pause}"));
    }
    let soulseek_wait = r.d["soulseek_wait"].is_object();
    let reason = r.d["wait"]["reason"].as_str().unwrap_or("");
    match status.as_str() {
        // parked for Soulseek by an older version
        "queued" if soulseek_wait && r.s("source").starts_with("slskd") => Ok(soulseek_offer(st)),
        "queued" => Ok(Offer { act: Act::Start, title: String::new(), until: None, button: false }),
        "waiting" => match reason {
            storage::STORAGE_DOWNLOAD => Ok(Offer::new(Act::Storage, "Check the storage now and start the download if it can be written")),
            storage::STORAGE_IMPORT => no(IMPORT_AGAIN),
            downloads::REASON => Ok(Offer::new(Act::Disk, "Check the downloads folder now and start the download if it can be written")),
            storage::SOULSEEK => Ok(soulseek_offer(st)),
            storage::PLUGIN_INSTALL => no(installing_text(r)),
            _ if soulseek_wait => Ok(soulseek_offer(st)),
            _ if r.d["kept"].is_object() => no(IMPORT_AGAIN),
            _ => Ok(Offer { act: Act::Start, title: String::new(), until: None, button: false }),
        },
        "searching" => no(SEARCHING),
        "importing" => no(IMPORTING),
        "monitoring" if r.s("source") == "album" => {
            let album = get(st, r.d["album_request"].as_i64().unwrap_or(0)).map(|a| a.album_title()).unwrap_or_default();
            no(format!("Its album “{album}” brings it – use Download now on the album"))
        }
        _ if r.s("source") == "torznab" => torrent_offer(st, r),
        "downloading" if remote_queued(r) && chosen_files(r) => no(chosen_text(r)),
        "downloading" if remote_queued(r) => Ok(Offer::new(Act::Switch, format!("Switch to a user with a free upload slot – it is {}", queue_at(r)))),
        _ => no(DOWNLOADING),
    }
}

/// A request waiting for its source's plugin, which Encore is still installing after the upgrade
/// (`plugin_install`, pick.rs `park_for_install`): no button. The install is the Plugins page's (admins) and
/// Encore tries it again by itself; the request goes on as soon as the plugin is there (after an hour without it,
/// without it), and nothing a press could make sooner is Encore's own to give.
fn installing_text(r: &Req) -> String {
    let src = r.d["install_wait"]["source"].as_str().map(String::from).unwrap_or_else(|| r.s("source"));
    let name = if src == "ytdlp" { "yt-dlp" } else { src.as_str() };
    format!("It waits for the {name} plugin to finish installing – it starts as soon as the plugin is installed")
}

/// What the requests list adds to a row (`GET /api/requests`): `can_download_now`, `download_now` ({act,
/// title, until} or null), `line` (where it waits: {source, label, position, waiting, running, limit,
/// limit_text, first, text} or null), `remote_queue` ({user, place, since, text} or null), `stalled_since`
/// and `pressed` ({by, at} while a press is kept). The list sets the message to `line.text` while it is set.
pub fn row_fields(st: &impl Ctx, r: &Req) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    let offer = offer(st, r).ok().filter(|o| o.button);
    m.insert("can_download_now".into(), json!(offer.is_some()));
    m.insert("download_now".into(), offer.map(|o| json!({ "act": o.kind(), "title": o.title, "until": o.until })).unwrap_or(Value::Null));
    let lines = &st.requests().lines;
    m.insert("line".into(), lines.place(r.id).map(|p| p.json()).unwrap_or(Value::Null));
    let remote = if remote_queued(r) {
        let user = current_candidate(r).and_then(|c| c["username"].as_str().map(String::from)).unwrap_or_default();
        let place = r.d["slskd"]["remote"]["place"].as_u64();
        json!({ "user": user, "place": place, "since": r.d["slskd"]["remote"]["since"], "text": remote_text(&user, place) })
    } else {
        Value::Null
    };
    m.insert("remote_queue".into(), remote);
    m.insert("stalled_since".into(), json!(stalled_since(r, now())));
    m.insert("pressed".into(), lines.pressed(r.id).map(|p| json!({ "by": p.by, "at": p.at })).unwrap_or(Value::Null));
    m
}

/// The row of a request that has to wait in a line: `queued`, "In line for Deezer" (the list says where it
/// stands); `detail.in_line` marks it until the worker writes the request again (a restart starts it over).
pub(crate) fn in_line_row(st: &impl Ctx, id: i64, label: &str) {
    let _ = st.db().execute("UPDATE requests SET status='queued', progress=0, message=?, detail=json_set(detail, '$.in_line', 1), updated_at=?
                             WHERE id=? AND status IN ('queued','searching','downloading')", p![format!("In line for {label}"), now(), id]);
    crate::events::emit(st, id);
}

/// A request whose turn in a line came, written by the gate (the worker writes its own): `status`, `message`.
pub(crate) fn left_line_row(st: &impl Ctx, id: i64, status: &str, message: &str) {
    let _ = st.db().execute("UPDATE requests SET status=?, message=?, detail=json_remove(detail, '$.in_line'), updated_at=? WHERE id=? AND status='queued'",
        p![status, message, now(), id]);
    crate::events::emit(st, id);
}

// ---- pressing it ---------------------------------------------------------------------------------------

fn done(action: &str, text: impl Into<String>) -> Value {
    json!({ "ok": true, "action": action, "text": text.into() })
}

fn later(action: &str, text: impl Into<String>, at: Option<f64>) -> Value {
    json!({ "ok": false, "action": action, "text": text.into(), "at": at })
}

/// "Next in line for Deezer (1 download at a time)", "2nd in line for Deezer (1 download at a time), behind 1
/// request asked for earlier", with Soulseek's pause when it runs.
fn line_answer(st: &impl Ctx, p: &Place) -> String {
    let mut t = format!("{} ({})", p.text(), p.limit_text());
    if p.position > 1 {
        let n = p.position - 1;
        t += &format!(", behind {n} request{} asked for earlier", if n == 1 { "" } else { "s" });
    }
    if p.source == line::SOULSEEK {
        if let Some(until) = st.requests().slskd.session().rate_limited_until() {
            t += &format!(" – Soulseek asked Encore to slow down: next search at {}", clock(session::unix_at(until)));
        }
    }
    t
}

/// The first letter in lower case, for the activity log's line.
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `POST /api/requests/{id}/download-now` for `who` (the request's owner or an admin; `admin`: the words of a
/// storage problem with its folder). → `{ok, action, text, at?, line?, status}` or (HTTP status, why not)
pub async fn download_now<C: Ctx>(st: &C, id: i64, who: &str, admin: bool) -> Result<Value, (u16, String)> {
    let r = get(st, id).ok_or((404, "Request not found".to_string()))?;
    let o = offer(st, &r)?;
    let lines = &st.requests().lines;
    let (mut body, outcome) = match o.act.clone() {
        Act::Line => {
            lines.press(id, who, now());
            // a slot that is free now goes to it at once
            let mut place = lines.place(id);
            for _ in 0..15 {
                if place.is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
                place = lines.place(id);
            }
            match place {
                Some(p) => {
                    let text = line_answer(st, &p);
                    let mut b = done("line", &text);
                    b["line"] = p.json();
                    (b, lower_first(&p.text()))
                }
                None => (done("now", STARTING), "starting now".to_string()),
            }
        }
        Act::Start => {
            lines.press(id, who, now());
            st.requests().wake.notify_one();
            (done("now", STARTING), "starting now".to_string())
        }
        Act::Storage => storage_now(st, id, who, admin).await?,
        Act::Disk => disk_now(st, id, who, admin).await?,
        // why Soulseek can't start can name a server folder: not for members
        Act::Soulseek(Sk::Stopped(why)) if !admin => soulseek_now(st, id, who, Sk::Stopped(storage::hide_folders(&why))).await?,
        Act::Soulseek(sk) => soulseek_now(st, id, who, sk).await?,
        Act::Switch => switch_now(st, id, who).await?,
        Act::NextSource { next, minutes } => next_source_now(st, id, who, &next, minutes).await?,
    };
    let status = get(st, id).map(|r| r.s("status")).unwrap_or_default();
    body["status"] = json!(status);
    let line = format!("{who} asked to download request #{id} ({}) now: {outcome}", r.label());
    tracing::info!("{line}");
    st.db().log("info", &line);
    crate::events::emit(st, id);
    Ok(body)
}

/// Hold the request's place in the queue (its running step ends first), and check it still offers `act`.
async fn hold_same(st: &impl Ctx, id: i64, act: &Act) -> Result<Hold, (u16, String)> {
    let hold = hold_idle(st, id, IDLE_WAIT).await.ok_or((409, BUSY.to_string()))?;
    let r = get(st, id).ok_or((404, "Request not found".to_string()))?;
    match offer(st, &r) {
        Ok(o) if std::mem::discriminant(&o.act) == std::mem::discriminant(act) => Ok(hold),
        _ => Err((409, CHANGED.to_string())),
    }
}

/// A download waiting for its staging folder: its next try is now, and its step runs at once – the exact
/// health check of its own folder, then the download when it can be written.
async fn storage_now<C: Ctx>(st: &C, id: i64, who: &str, admin: bool) -> Result<(Value, String), (u16, String)> {
    let hold = hold_same(st, id, &Act::Storage).await?;
    let t = now();
    let _ = st.db().execute("UPDATE requests SET detail=json_set(detail, '$.wait.next_try_at', ?) WHERE id=? AND status='waiting'", p![t, id]);
    take_turn(st, id, hold, step(st.clone(), id));
    // what the step made of it
    let until = tokio::time::Instant::now() + WATCH;
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let Some(r) = get(st, id) else { return Err((404, "Request not found".into())) };
        let still = r.s("status") == "waiting" && r.d["wait"]["reason"] == storage::STORAGE_DOWNLOAD;
        if !still {
            st.requests().lines.press(id, who, t);
            return Ok((done("now", "The storage can be written again – downloading now"), "the storage can be written again".into()));
        }
        let next = r.d["wait"]["next_try_at"].as_f64().unwrap_or(0.0);
        if next > t + 1.0 {
            let text = if admin {
                r.s("message")
            } else {
                storage::plain_message("waiting", &r.s("message"), &r.d).unwrap_or_else(|| storage::hide_folders(&r.s("message")))
            };
            return Ok((later("later", text, Some(next)), format!("the storage still can't be written (next try at {})", clock(next))));
        }
        if tokio::time::Instant::now() >= until {
            st.requests().lines.press(id, who, t);
            return Ok((done("now", "Checking the storage now"), "checking the storage now".into()));
        }
    }
}

/// A request waiting for a downloads folder: the folders probed now (unless the last probe is fresh and found
/// them writable), then its step at once when its folder can be written.
async fn disk_now<C: Ctx>(st: &C, id: i64, who: &str, admin: bool) -> Result<(Value, String), (u16, String)> {
    let writable = || (done("now", "The downloads folder can be written again – downloading now"), "the downloads folder can be written again".to_string());
    if !downloads::probe_soon(st, 10.0).await {
        // a probe still runs (a network mount that doesn't answer): the queue goes on once it ends
        return Ok((done("now", "Checking the downloads folder now"), "checking the downloads folder now".into()));
    }
    // the queue's own look may have taken it on meanwhile
    let waits = |r: &Req| r.s("status") == "waiting" && r.d["wait"]["reason"] == downloads::REASON;
    if !get(st, id).is_some_and(|r| waits(&r)) {
        st.requests().lines.press(id, who, now());
        return Ok(writable());
    }
    let hold = hold_idle(st, id, IDLE_WAIT).await.ok_or((409, BUSY.to_string()))?;
    let r = get(st, id).ok_or((404, "Request not found".to_string()))?;
    if !waits(&r) {
        st.requests().lines.press(id, who, now());
        return Ok(writable());
    }
    let src = r.d["wait"]["source"].as_str().unwrap_or("").to_string();
    let failing = match r.d["wait"]["proto"].as_str() {
        Some(proto) => downloads::failing_client(st, proto).is_some(),
        None => downloads::failing_for(st, &src).is_none_or(|f| !f.is_empty()),
    };
    let h = take_turn(st, id, hold, step(st.clone(), id));
    if !failing {
        st.requests().lines.press(id, who, now());
        return Ok(writable());
    }
    // the step writes the folder's words again
    let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
    let msg = get(st, id).map(|r| r.s("message")).unwrap_or_default();
    // "The downloads folder still can't be written – /downloads/encore is read-only" (the row's words after its
    // status); other users than admins read no folder
    let shown = if admin && msg.starts_with(downloads::WAITING) { msg } else { downloads::PLAIN_WAITING.to_string() };
    let text = format!("{}{}", downloads::STILL_FAILING, shown.strip_prefix(downloads::WAITING).unwrap_or(""));
    Ok((later("later", text, None), "the downloads folder still can't be written".into()))
}

/// A request waiting for Soulseek (§1.2): its step at once where Soulseek can be searched now or won't come
/// back by itself; an attempt made due where Encore's own schedule holds the session; else when.
/// Download now's answer for a request whose Soulseek step goes on now (or once it is connected).
fn soulseek_answer(sk: &Sk) -> (Value, String) {
    match sk {
        Sk::LoggedIn => (done("now", "Soulseek is connected – searching now"), "searching Soulseek now".into()),
        Sk::Stopped(why) => (done("now", format!("Soulseek is stopped ({why}) – trying the next source")), "trying the next source".into()),
        _ => (done("now", "Connecting to Soulseek now – it searches once logged in"), "connecting to Soulseek now".into()),
    }
}

/// The request's step at once, in its place in the queue (Soulseek can be searched now, or won't come back by
/// itself and the request goes on to its next source).
async fn soulseek_step<C: Ctx>(st: &C, id: i64, who: &str, sk: Sk) -> Result<(Value, String), (u16, String)> {
    let hold = hold_idle(st, id, IDLE_WAIT).await.ok_or((409, BUSY.to_string()))?;
    take_turn(st, id, hold, step(st.clone(), id));
    st.requests().lines.press(id, who, now());
    Ok(soulseek_answer(&sk))
}

async fn soulseek_now<C: Ctx>(st: &C, id: i64, who: &str, sk: Sk) -> Result<(Value, String), (u16, String)> {
    match sk {
        Sk::LoggedIn | Sk::Stopped(_) => soulseek_step(st, id, who, sk).await,
        Sk::Connecting => {
            st.requests().lines.press(id, who, now());
            Ok(soulseek_answer(&Sk::Connecting))
        }
        Sk::Attempt => match st.requests().slskd.session().attempt_now() {
            AttemptNow::Due | AttemptNow::Connecting => {
                st.requests().lines.press(id, who, now());
                Ok(soulseek_answer(&Sk::Connecting))
            }
            AttemptNow::LoggedIn => soulseek_step(st, id, who, Sk::LoggedIn).await,
            AttemptNow::Stopped => soulseek_step(st, id, who, soulseek_state(st)).await,
            AttemptNow::Later(t) => {
                let at = session::unix_at(t);
                Ok((later("later", format!("Soulseek was tried moments ago – next attempt at {}", clock(at)), Some(at)), format!("Soulseek can be tried again at {}", clock(at))))
            }
        },
        Sk::Timeout(at) => Ok((later("later", format!("The Soulseek server isn't answering logins – Encore tries again at {} so it isn't shut out", clock(at)), Some(at)),
            format!("Soulseek asked Encore to wait until {}", clock(at)))),
        Sk::Kicked(at) => Ok((later("later", format!("Another Soulseek client logged in with this username – Encore tries again at {}", clock(at)), Some(at)),
            format!("Soulseek asked Encore to wait until {}", clock(at)))),
        Sk::Vpn => Ok((later("later", "Soulseek's VPN is reconnecting – it searches once it is back", None), "Soulseek's VPN is reconnecting".into())),
        Sk::Engine => Ok((later("later", "The Soulseek engine is starting – it searches once it is up", None), "the Soulseek engine is starting".into())),
    }
}

// ---- a user with a free upload slot ----------------------------------------------------------------------

/// What a request asked for, for ranking Soulseek answers as its own search ranks them.
#[derive(Clone, Debug)]
pub struct Want {
    pub album: bool,
    pub artist: String,
    /// the album's title, or the song's
    pub name: String,
    /// the release's tracks (an album)
    pub tracks: Option<usize>,
    /// the song's length (seconds)
    pub length: Option<f64>,
    /// only lossless files (a lossless step, or a request for a lossless copy)
    pub lossless_only: bool,
}

impl Want {
    pub fn of(r: &Req) -> Want {
        let album = r.s("kind") == "album";
        let lossless_only = r.s("source") == "slskd_lossless" || r.d["lossless_only"].as_bool().unwrap_or(false);
        Want { album, artist: r.s("artist"), name: if album { r.album_title() } else { r.s("title") }, tracks: n_tracks(&r.d.get("release").filter(|v| !v.is_null()).cloned()),
               length: r.length(), lossless_only }
    }
}

fn lossless(c: &Value) -> bool {
    encore_core::audio::DOWNLOAD_LOSSLESS.contains(&c["format"].as_str().unwrap_or(""))
}

/// The users a download waiting in `current`'s upload queue could switch to, best first: only answers that
/// said they have a free upload slot – kept before the ranking, so cutting it to `slskd_max_candidates`
/// can't drop them –, ranked by the worker's own rules (preferred format, minimum bit rate, track count,
/// album or title match, no unwanted versions), without the current user, what was tried (`tried`: "user|folder"),
/// what is blocked (`blocked`) and, for a lossless download, anything lossy – all of those left out before the
/// cut too, so they can't take the places of users who qualify; users a replacement avoids (`detail.avoid`) last.
pub fn free_slot_choice(st: &impl encore_http::Ctx, answers: &[Value], want: &Want, current: &Value, tried: &[String], blocked: impl Fn(&Value) -> bool, d: &Value) -> Vec<Value> {
    let user = current["username"].as_str().unwrap_or("");
    let free: Vec<Value> = answers.iter().filter(|a| a["hasFreeUploadSlot"].as_bool() == Some(true) && a["username"].as_str() != Some(user)).cloned().collect();
    let lossless_only = want.lossless_only || lossless(current);
    let keep = |c: &Value| c["username"].as_str() != Some(user) && !tried.contains(&slskd_key(c)) && !blocked(c);
    let mut cands = if want.album {
        slskd::rank_album_where(st, &free, &want.artist, &want.name, want.tracks, lossless_only, keep)
    } else {
        slskd::rank_track_where(st, &free, &want.artist, &want.name, want.length, lossless_only, keep)
    };
    crate::report::avoid_users(d, &mut cands);
    cands
}

/// Every file of candidate `c` still waits in its user's upload queue and nothing of it arrived (slskd asked
/// again, as the poll asks: the newest entry of each file counts).
async fn still_queued_remotely(st: &impl Ctx, c: &Value) -> bool {
    let wanted: Vec<&str> = c["files"].as_array().map(|a| a.iter().filter_map(|f| f["filename"].as_str()).collect()).unwrap_or_default();
    let mut latest: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for f in slskd::user_downloads(st, c["username"].as_str().unwrap_or("")).await {
        let n = f["filename"].as_str().unwrap_or("").to_string();
        if wanted.contains(&n.as_str()) {
            latest.insert(n, f);
        }
    }
    remote_queue(latest.values()).is_some()
}

/// What a switch came to.
enum Switched {
    /// switched: the answer's words, the log's
    Done(String, String),
    /// nothing changed: why
    Kept(String),
    /// it isn't queued at the user any more
    Changed,
}

/// The switch itself, as the request's task (no step runs meanwhile): the kept answers ranked, the best users
/// asked whether their slot is free now, the winner's files handed to slskd – and only once slskd took them,
/// the current user's queued transfers cancelled.
async fn switch<C: Ctx>(st: &C, id: i64, who: &str) -> Switched {
    let Some(mut r) = get(st, id) else { return Switched::Changed };
    if !remote_queued(&r) {
        return Switched::Changed;
    }
    if chosen_files(&r) {
        return Switched::Kept(chosen_text(&r));
    }
    let Some(cur) = current_candidate(&r) else { return Switched::Changed };
    let alice = cur["username"].as_str().unwrap_or("").to_string();
    let place = r.d["slskd"]["remote"]["place"].as_u64();
    let stays = stays(&alice, place);
    // the newest Soulseek answers kept for the request
    let keys: Vec<String> = crate::search_cache::worker_texts(&r.s("kind"), &r.s("artist"), &r.s("title"), &r.s("album")).iter().map(|t| crate::search_cache::slskd_key(t)).collect();
    let (cache, dir, since) = (st.requests().search_cache.clone(), st.data_dir().to_path_buf(), r.row.f("created_at"));
    let found = tokio::task::spawn_blocking(move || cache.get_for_request(&dir, "slskd", Some(id), &keys, since, now())).await.ok().flatten();
    let Some(found) = found else { return Switched::Kept(format!("No other Soulseek results are kept for it – {stays}")) };
    let tried: Vec<String> = r.d["slskd_tried"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let cands = free_slot_choice(st, &found.results, &Want::of(&r), &cur, &tried, |c| crate::report::slskd_blocked(st, c), &r.d);
    if cands.is_empty() {
        return Switched::Kept(format!("No one with a free upload slot has it – {stays}"));
    }
    if st.requests().slskd.session().check(st).await.is_err() {
        return Switched::Kept(format!("Soulseek isn't connected – {stays}"));
    }
    // a free slot now: the best few are asked (one that doesn't answer, or says no, is skipped)
    let mut winner = None;
    for c in cands.iter().take(ASK_USERS) {
        if let Ok(info) = slskd::user_info(st, c["username"].as_str().unwrap_or("")).await {
            if info["hasFreeUploadSlot"].as_bool() == Some(true) {
                winner = Some(c.clone());
                break;
            }
        }
    }
    let Some(w) = winner else { return Switched::Kept(format!("No one with a free upload slot has it – {stays}")) };
    let bob = w["username"].as_str().unwrap_or("").to_string();
    // asking the users took a while: alice's upload may have started meanwhile, and a download with progress
    // is never given up for one without
    if !still_queued_remotely(st, &cur).await {
        return Switched::Changed;
    }
    // enqueue first, cancel after: alice's place is never given up unless bob's download was taken
    if let Err(e) = slskd::enqueue(st, &bob, w["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[])).await {
        if e.is::<slskd::NotConnected>() || e.is::<crate::slskd_managed::Unreachable>() {
            return Switched::Kept(format!("Soulseek isn't connected – {stays}"));
        }
        return Switched::Kept(format!("{bob} couldn't be asked for the files ({}) – {stays}", enqueue_failure(&bob, &e.to_string())));
    }
    if !still_queued_remotely(st, &cur).await {
        // alice's upload started while bob's files were handed over: bob's go again, alice's stay
        slskd_cancel_current(st, &w).await;
        return Switched::Changed;
    }
    slskd_cancel_current(st, &cur).await;
    if !r.d["slskd_tried"].is_array() {
        r.d["slskd_tried"] = json!([]);
    }
    r.d["slskd_tried"].as_array_mut().unwrap().push(json!(slskd_key(&cur)));
    let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(0).max(0) as usize;
    if let Some(list) = r.d["slskd"]["candidates"].as_array_mut() {
        // bob next, and only once: his folder further down the list goes, so a failure isn't tried again
        let key = slskd_key(&w);
        let mut rest = list.split_off((idx + 1).min(list.len()));
        rest.retain(|c| slskd_key(c) != key);
        list.push(w.clone());
        list.extend(rest);
    }
    let t = now();
    r.d["slskd"]["idx"] = json!(idx + 1);
    r.d["slskd"]["started"] = json!(t);
    r.d["slskd"]["last_change"] = json!(t);
    r.d["slskd"]["last_bytes"] = json!(0);
    if let Some(o) = r.d["slskd"].as_object_mut() {
        o.remove("remote");
        o.remove("pending");
    }
    let was = match place {
        Some(p) => format!("{} in their queue", line::ordinal(p as usize)),
        None => "in their queue".to_string(),
    };
    log_push(&mut r, format!("slskd: switched from {alice} ({was}) to {bob} (free upload slot) – Download now by {who}"));
    upd(st, &mut r, &[("status", json!("downloading")), ("progress", json!(0)), ("message", json!(slskd_message(&w)))]);
    let n = w["files"].as_array().map(|a| a.len()).unwrap_or(0);
    Switched::Done(format!("Switched to {bob}, who has a free upload slot ({}, {n} file{})", slskd_format(&w), if n == 1 { "" } else { "s" }),
        format!("switched from {alice} to {bob}"))
}

async fn switch_now<C: Ctx>(st: &C, id: i64, who: &str) -> Result<(Value, String), (u16, String)> {
    let hold = hold_same(st, id, &Act::Switch).await?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (s, w) = (st.clone(), who.to_string());
    take_turn(st, id, hold, async move {
        let _ = tx.send(switch(&s, id, &w).await);
    });
    match tokio::time::timeout(SWITCH_WAIT, rx).await {
        Ok(Ok(Switched::Done(text, log))) => {
            st.requests().lines.press(id, who, now());
            Ok((done("switched", text), log))
        }
        Ok(Ok(Switched::Kept(text))) => {
            let log = lower_first(&text);
            Ok((later("kept", text, None), log))
        }
        Ok(Ok(Switched::Changed)) => Err((409, STARTED_MEANWHILE.to_string())),
        // the task goes on; the row shows what it came to
        _ => Ok((done("switching", "Looking for a user with a free upload slot…"), "looking for a user with a free upload slot".into())),
    }
}

/// A torrent or Usenet download with no progress: the client drops it (and its unfinished data), the other
/// releases of that search are skipped, and the request goes on to its next source.
async fn next_source_now<C: Ctx>(st: &C, id: i64, who: &str, next: &str, minutes: i64) -> Result<(Value, String), (u16, String)> {
    let hold = hold_same(st, id, &Act::NextSource { next: next.to_string(), minutes }).await?;
    st.requests().lines.press(id, who, now());
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    let (s, w) = (st.clone(), who.to_string());
    take_turn(st, id, hold, async move {
        let Some(mut r) = get(&s, id) else {
            let _ = tx.send(false);
            return;
        };
        let via = torznab_client(&s, &r);
        if let Some(tid) = r.d["torznab"]["id"].as_str().map(String::from) {
            torznab::remove(&s, &tid).await;
        }
        r.d["torznab"]["id"] = Value::Null;
        let n = r.d["torznab"]["candidates"].as_array().map(|a| a.len()).unwrap_or(0);
        r.d["torznab"]["idx"] = json!(n);
        let _ = tx.send(true);
        let reason = format!("no progress in {via} for {minutes} minutes – Download now by {w}");
        if let Err(e) = next_source(&s, &mut r, &reason).await {
            on_error(&s, id, e).await;
        }
    });
    match tokio::time::timeout(SWITCH_WAIT, rx).await {
        Ok(Ok(false)) => Err((404, "Request not found".into())),
        _ => Ok((done("next_source", format!("Trying the next source: {next}")), format!("trying the next source ({next}) after {minutes} minutes without progress"))),
    }
}

// ---- Download all now ------------------------------------------------------------------------------------

/// `POST /api/requests/download-now` (admins): every request in a line that it targets (everyone's, or only
/// `mine`'s – the admin's user id) is pressed in the order it stands, so nobody among them overtakes anybody;
/// the storage and the downloads folders are checked once, Soulseek's planned attempt made due once (as
/// Download now does) when a target waits for it, and the queue's loop is woken. It never switches Soulseek
/// users, never drops a torrent, never approves and never starts more than a line's limit.
/// → `{ok, pressed, woken, later, lines: [{source, label, limit, running, waiting}], text}`
pub async fn download_all_now<C: Ctx>(st: &C, who: &str, mine: Option<i64>) -> Value {
    let lines = &st.requests().lines;
    let t = now();
    let owner_of = |id: i64| st.db().one("SELECT user_id FROM requests WHERE id=?", p![id]).ok().flatten().map(|r| r.i("user_id"));
    let targets = |id: i64| mine.is_none_or(|u| owner_of(id) == Some(u));
    let mut touched: Vec<i64> = vec![];
    // the lines: pressed in the order they stand
    let mut pressed = 0;
    for l in lines.all() {
        let ids: Vec<i64> = l.waiters(&lines.pressed).into_iter().map(|w| w.0).filter(|id| *id > 0 && targets(*id)).collect();
        pressed += ids.len();
        lines.press_in_order(&ids, who, t);
        touched.extend(ids);
    }
    // the timers
    let mut sql = "SELECT id, status, source, detail FROM requests WHERE status IN ('queued','waiting')".to_string();
    let mut params = vec![];
    if let Some(u) = mine {
        sql += " AND user_id=?";
        params.push(json!(u));
    }
    let (mut storage_ids, mut disk_ids, mut soulseek_ids, mut start_ids) = (vec![], vec![], vec![], vec![]);
    for row in st.db().query(&sql, params).unwrap_or_default() {
        let id = row.i("id");
        if lines.place(id).is_some() || lines.holding(id).is_some() {
            continue;
        }
        let d: Value = serde_json::from_str(&row.s("detail")).unwrap_or(json!({}));
        let reason = d["wait"]["reason"].as_str().unwrap_or("");
        match (row.s("status").as_str(), reason) {
            ("waiting", storage::STORAGE_DOWNLOAD | storage::STORAGE_IMPORT) => storage_ids.push(id),
            ("waiting", downloads::REASON) => disk_ids.push(id),
            ("waiting", storage::SOULSEEK) => soulseek_ids.push(id),
            ("waiting", _) | ("queued", _) if d["soulseek_wait"].is_object() => soulseek_ids.push(id),
            ("queued", _) => start_ids.push(id),
            _ => {}
        }
    }
    let (mut woken, mut later) = (start_ids.len(), 0);
    let mut notes: Vec<String> = vec![];
    if !storage_ids.is_empty() || !disk_ids.is_empty() {
        let health = storage::check_now(st, true).await;
        if health["ok"] != json!(false) {
            storage::resume_waiting(st, false);
            woken += storage_ids.len();
        } else if !storage_ids.is_empty() {
            later += storage_ids.len();
            notes.push("The storage still can't be written".into());
        }
        if health["downloads"]["ok"] != json!(false) {
            woken += disk_ids.len();
        } else if !disk_ids.is_empty() {
            later += disk_ids.len();
            notes.push("A downloads folder still can't be written".into());
        }
    }
    if !soulseek_ids.is_empty() {
        let n = soulseek_ids.len();
        let sk = match soulseek_state(st) {
            Sk::Attempt => match st.requests().slskd.session().attempt_now() {
                AttemptNow::Later(at) => Err(format!("Soulseek was tried moments ago – next attempt at {}", clock(session::unix_at(at)))),
                _ => Ok(()),
            },
            Sk::Timeout(at) | Sk::Kicked(at) => Err(format!("Soulseek asked Encore to wait – next attempt at {}", clock(at))),
            Sk::Vpn => Err("Soulseek's VPN is reconnecting".to_string()),
            Sk::Engine => Err("The Soulseek engine is starting".to_string()),
            Sk::LoggedIn | Sk::Connecting | Sk::Stopped(_) => Ok(()),
        };
        match sk {
            Ok(()) => woken += n,
            Err(why) => {
                later += n;
                notes.push(why);
            }
        }
    }
    touched.extend(storage_ids.iter().chain(&disk_ids).chain(&soulseek_ids).chain(&start_ids));
    st.requests().wake.notify_one();
    let in_lines: Vec<Value> = lines.all().into_iter().filter(|l| l.waiting() > 0).map(|l| {
        let labels: Vec<String> = l.waiters(&lines.pressed).into_iter().map(|w| w.1).collect();
        let label = match labels.first() {
            Some(first) if labels.iter().all(|x| x == first) => first.clone(),
            _ => match l.source {
                line::STREAMRIP => line::streamrip_label(None, &streamrip::services(st)),
                line::YTDLP => line::YOUTUBE.into(),
                line::INDEXERS => line::TORRENTS.into(),
                // a plugin with several sources: their names
                line::PLUGIN => {
                    let mut names = labels.clone();
                    names.sort();
                    names.dedup();
                    names.join("/")
                }
                _ => line::SOULSEEK_LABEL.into(),
            },
        };
        json!({ "source": l.source, "label": label, "limit": l.limit, "running": l.running(), "waiting": l.waiting() })
    }).collect();
    let text = all_text(mine.is_some(), pressed, woken, &in_lines, &notes);
    for id in &touched {
        crate::events::emit(st, *id);
    }
    let line = format!("{who} asked to download {} waiting requests now: {text}", if mine.is_some() { "their" } else { "all" });
    tracing::info!("{line}");
    st.db().log("info", &line);
    json!({ "ok": pressed + woken > 0, "pressed": pressed, "woken": woken, "later": later, "lines": if pressed + woken + later == 0 { json!([]) } else { json!(in_lines) }, "text": text })
}

/// The answer's sentence: "Woke 3 waiting requests; 8 stay in line: Deezer 1 download at a time (6 in line),
/// YouTube 2 downloads at a time (2 in line). Soulseek asked Encore to wait – next attempt at 14:20".
pub fn all_text(mine: bool, pressed: usize, woken: usize, lines: &[Value], notes: &[String]) -> String {
    if pressed + woken == 0 && notes.is_empty() {
        return "Nothing is waiting – every request is downloading or done".into();
    }
    let s = |n: usize| if n == 1 { "" } else { "s" };
    let mut head: Vec<String> = vec![];
    if mine && pressed > 0 {
        head.push(format!("Moved {pressed} of your request{} to the front of their lines", s(pressed)));
    }
    if woken > 0 {
        head.push(format!("{} {woken} waiting request{}", if head.is_empty() { "Woke" } else { "woke" }, s(woken)));
    }
    let mut parts: Vec<String> = vec![];
    if !head.is_empty() {
        parts.push(head.join(" and "));
    }
    let in_line: usize = lines.iter().map(|l| l["waiting"].as_u64().unwrap_or(0) as usize).sum();
    if in_line > 0 {
        let each: Vec<String> = lines.iter().map(|l| format!("{} {} ({} in line)", l["label"].as_str().unwrap_or(""),
            line::limit_text(l["source"].as_str().unwrap_or(""), l["limit"].as_u64().unwrap_or(1) as usize), l["waiting"])).collect();
        parts.push(format!("{in_line} stay in line: {}", each.join(", ")));
    }
    let mut text = parts.join("; ");
    for n in notes {
        if text.is_empty() {
            text = n.clone();
        } else {
            text += &format!(". {n}");
        }
    }
    text
}

#[cfg(test)]
#[path = "now_tests.rs"]
mod tests;
