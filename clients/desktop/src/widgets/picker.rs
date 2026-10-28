//! The download picker (web-rs/src/pages/picker.rs `picker_modal`): search every source by hand,
//! see quality, size and uploader, and download exactly the result you choose. A page holds
//! `Option<State>`, routes its messages here with [`update`], draws [`view`] from its `overlay`,
//! reloads its list when `picked` is set and drops the state once `closed` is set.
//!
//! `opts` is the web's: `{ kind: 'album'|'track', artist, title, album, mbid, artist_mbid,
//! track_count, length, year, cover_url, isrc, request_id, ext }`.
//!
//! Since R2 the server keeps Soulseek and Torrents & Usenet searches (`POST /api/picker/search`
//! answers `{items, from, cached_at, found, query, can_search, notice}`): a kept answer shows at
//! once with "Found at 14:02 · Search again" under its source, and Search again asks that one
//! source anew (`fresh`). The switch "Don't check the audio" (beside Lossless only, kept on this
//! computer) sends the pick with `no_check`, so the audio check can't turn it away.

use super::catalog;
use super::{badge, btn, btn_icon, btn_sm, h3, modal, muted, small_muted, t, El, Icon};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, Page, Section};
use crate::theme::{self, Badge, Btn};
use iced::widget::{button, checkbox, column, container, row, text_input, tooltip};
use iced::{Alignment, Border, Length, Task};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Where the "Don't check the audio" switch is kept on this computer (the config's copy of the
/// web's localStorage): "1" on, anything else off.
pub const NO_CHECK_KEY: &str = "encore.pickNoCheck";

/// The switch beside Lossless only, and its tooltip.
pub const NO_CHECK: &str = "Don't check the audio";
pub const NO_CHECK_TIP: &str = "Imports what you pick even when the audio check says it is another song";

/// The last line of the dialog, with the switch off and on (the web's `note_text`).
pub const FOOT: &str = "The file you pick is still fingerprint-checked, tagged with MusicBrainz data and de-duplicated. If it fails, the request fails instead of trying other sources.";
pub const FOOT_NO_CHECK: &str = "The audio check is off: the file you pick is imported even when the check says it is another song. It is still tagged with MusicBrainz data and de-duplicated. If it fails, the request fails instead of trying other sources.";

/// Another Soulseek search holds the turn (one runs at a time): this one waits for it.
pub const SLSK_WAITING: &str = "Waiting for another Soulseek search, then searching…";

/// A failed search on one source: the message and, when the server pointed at a setting (a 409:
/// the source is signed out, stopped or off), the settings link and its label.
#[derive(Clone, Debug)]
struct PickErr {
    message: String,
    settings: String,
    label: String,
}

impl PickErr {
    fn from_api(e: &ApiError) -> PickErr {
        PickErr {
            message: e.message.clone(),
            settings: fmt::s(&e.data, "settings"),
            label: fmt::s(&e.data, "settings_label"),
        }
    }
}

/// The word after a tab's name for a source that cannot search right now.
fn state_word(state: &str) -> &'static str {
    match state {
        "sign_in" => "sign in",
        "not_connected" => "not connected",
        "not_installed" => "not installed",
        "off" => "off",
        _ => "",
    }
}

/// Whether the picker asks a source: a ready one, and Soulseek while it is not connected too –
/// a server that keeps searches still answers from them (one that keeps none says 409, and the
/// source's hint shows as before).
fn searches(s: &Value) -> bool {
    let st = fmt::s(s, "state");
    st.is_empty() || st == "ready" || (fmt::s(s, "id") == "slskd" && st == "not_connected")
}

/// "Searching Soulseek… (Soulseek takes a few seconds)" while a first search runs.
pub fn searching_text(id: &str, name: &str) -> String {
    format!("Searching {name}…{}", if id == "slskd" { " (Soulseek takes a few seconds)" } else { "" })
}

/// While Search again runs, the kept rows stay and this line shows.
pub fn searching_again_text(name: &str) -> String {
    format!("Searching {name} again…")
}

/// The words of a search, compared without case, punctuation or word order.
fn words(s: &str) -> Vec<String> {
    let mut w: Vec<String> = s
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    w.sort();
    w
}

/// Whether two searches are the same words.
pub fn same_query(a: &str, b: &str) -> bool {
    words(a) == words(b)
}

/// Whether an answer came from a kept search.
pub fn is_kept(answer: &Value) -> bool {
    answer["from"].as_str() == Some("cache")
}

/// Whether Search again can run now: true unless the server says it can't (`can_search`
/// false or 0, Soulseek not connected).
pub fn can_search(answer: &Value) -> bool {
    match &answer["can_search"] {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        _ => true,
    }
}

/// The line under a source whose rows came from a kept search, and whether Search again follows
/// it; None for a live answer (nothing to say):
/// - "Found at 14:02" (the server's words, on its clock – the clock of its "next attempt at")
/// - "No results. · Found at 14:02" when the kept search found nothing
/// - "Found at 14:02 for "Illuminate EP"" when it was searched with other words than these
/// - "Found at 14:02 · Soulseek isn't connected – next attempt at 14:20", with no Search again,
///   while it can't search
pub fn found_line(answer: &Value, searched: &str, empty: bool) -> Option<(String, bool)> {
    if !is_kept(answer) {
        return None;
    }
    let mut parts: Vec<String> = vec![];
    if empty {
        parts.push("No results.".into());
    }
    let mut found = fmt::s(answer, "found").trim().to_string();
    if found.is_empty() && fmt::f(answer, "cached_at") > 0.0 {
        // a server that sends no sentence: the time in UTC, as the app's other times
        found = format!("Found at {}", fmt::stamp(fmt::f(answer, "cached_at")));
    }
    let query = fmt::s(answer, "query");
    if !found.is_empty() && !query.trim().is_empty() && !same_query(&query, searched) {
        found = format!("{found} for \"{}\"", query.trim());
    }
    if !found.is_empty() {
        parts.push(found);
    }
    let again = can_search(answer);
    if !again && fmt::truthy(answer, "notice") {
        parts.push(fmt::s(answer, "notice").trim().to_string());
    }
    Some((parts.join(" · "), again))
}

#[derive(Debug)]
pub struct State {
    opts: Value,
    kind: &'static str,
    /// "Pick a download · <title>".
    pub title: String,
    /// `/api/picker/sources`; None until it answered.
    sources: Option<Vec<Value>>,
    q: String,
    link: String,
    tab: String,
    lossless: bool,
    /// "Don't check the audio": the pick goes with `no_check`.
    no_check: bool,
    /// Whether the switch is offered: to those who may take a download the check turned away (Keep
    /// anyway's people – admins, members with Fix their own downloads); the server refuses it from others.
    may_skip: bool,
    /// The words the shown rows were searched with.
    searched: String,
    results: HashMap<String, Vec<Value>>,
    /// The answer's fields (without its rows) for a source whose rows came from a kept search.
    kept: HashMap<String, Value>,
    errors: HashMap<String, PickErr>,
    loading: HashSet<String>,
    /// Search again is running for these sources (their kept rows stay meanwhile) …
    again: HashSet<String>,
    /// … and why one failed (the rows stay, the error shows under the line).
    again_errors: HashMap<String, PickErr>,
    /// Why a source's answer has no rows, in the server's words ("Prowlarr's indexers found
    /// nothing for …"): shown instead of a bare "No results." ([`empty_note`]).
    notes: HashMap<String, String>,
    /// Another Soulseek search held the turn when this one was sent (one runs at a time): say it waits.
    slsk_waiting: bool,
    /// The search generation: answers of an older search are dropped.
    gen: u64,
    /// A pick is being sent.
    sending: bool,
    /// Set when the dialog asked to be closed; the page drops the state.
    pub closed: bool,
    /// The server's answer to the pick (`{id, status}`); the page reloads its list.
    pub picked: Option<Value>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Sources(Result<Value, ApiError>),
    Query(String),
    Search,
    Link(String),
    DownloadLink,
    /// The "Download this link" row under the search box.
    GetLink,
    Tab(String),
    Lossless(bool),
    /// "Don't check the audio".
    NoCheck(bool),
    /// `(generation, source, answer)`.
    Searched(u64, String, Result<Value, ApiError>),
    /// Search again: a new search of this one source (`fresh`).
    Again(String),
    /// `(generation, source, answer)` of Search again.
    Agained(u64, String, Result<Value, ApiError>),
    /// GET /system/soulseek as a search starts: whether the Soulseek one waits for another search.
    SlskTurn(u64, Result<Value, ApiError>),
    /// A second after a Soulseek search that had to wait for its turn started: say it waits,
    /// unless it answered meanwhile (a kept answer comes at once).
    SlskWait(u64),
    /// Get the `i`th result of `source`.
    Get(String, usize),
    Picked(Result<Value, ApiError>),
    /// A settings link in a hint (`#/settings?s=slskd`).
    OpenSettings(String),
    /// Copy a source's error in full.
    CopyError(String),
    Close,
}

/// `/^https?:\/\/\S+$/i.test(v.trim())`.
pub fn is_url(v: &str) -> bool {
    let v = v.trim();
    let lower = v.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    matches!(rest, Some(r) if !r.is_empty() && !r.chars().any(char::is_whitespace))
}

/// The settings page of a web link (`#/settings?s=slskd`): a section the app has, or Plugins,
/// where the server's download sources are set up since 0.32 (on an older server that page
/// points to Downloads).
pub fn settings_page(hash: &str) -> Page {
    let id = hash
        .split_once("s=")
        .map(|(_, r)| r.split('&').next().unwrap_or(""))
        .unwrap_or("");
    Page::Settings(Section::by_id(id).unwrap_or(Section::Plugins))
}

/// The picker's options from `(key, value)` pairs, as the web's `copy`/`put` build them: a
/// value that is absent (null) is left out.
pub fn opts(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

/// `opts.title || opts.album`.
fn title_or_album(opts: &Value) -> Value {
    if fmt::truthy(opts, "title") {
        opts["title"].clone()
    } else {
        opts["album"].clone()
    }
}

/// A JSON scalar as JavaScript prints it ("" for null).
fn js(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Open the picker for a release or a song; the sources load first.
pub fn open(opts: Value, ctx: &Ctx) -> (State, Task<Message>) {
    let kind = if opts["kind"].as_str() == Some("track") { "track" } else { "album" };
    let toa = title_or_album(&opts);
    let q0 = [opts["artist"].clone(), toa.clone()]
        .iter()
        .filter(|v| !js(v).is_empty())
        .map(js)
        .collect::<Vec<_>>()
        .join(" ");
    let may_skip = super::request_list::may_fix(ctx.is_admin(), ctx.session.perm("fix"));
    let state = State {
        title: format!("Pick a download · {}", js(&toa)),
        opts,
        kind,
        sources: None,
        q: q0,
        link: String::new(),
        tab: "all".into(),
        lossless: false,
        no_check: may_skip && ctx.config.local_str(NO_CHECK_KEY) == "1",
        may_skip,
        searched: String::new(),
        results: HashMap::new(),
        kept: HashMap::new(),
        errors: HashMap::new(),
        loading: HashSet::new(),
        again: HashSet::new(),
        again_errors: HashMap::new(),
        notes: HashMap::new(),
        slsk_waiting: false,
        gen: 0,
        sending: false,
        closed: false,
        picked: None,
    };
    let api = ctx.api.clone();
    (state, Task::perform(async move { api.get("/picker/sources").await }, Message::Sources))
}

impl State {
    fn sources(&self) -> &[Value] {
        self.sources.as_deref().unwrap_or(&[])
    }

    /// The source as `/picker/sources` listed it: `{id, name, state, hint, settings, settings_label}`.
    fn source(&self, id: &str) -> Option<&Value> {
        self.sources().iter().find(|s| s["id"].as_str() == Some(id))
    }

    /// The source's display name (the server's; the id when it is not listed).
    fn name(&self, id: &str) -> String {
        self.source(id)
            .map(|s| fmt::s(s, "name"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.to_string())
    }

    /// A source searches only when the server says it is ready; anything else (signed out, not
    /// connected, not installed, off) shows its hint and settings link under its tab instead.
    fn ready(&self, id: &str) -> bool {
        self.source(id).is_some_and(|s| {
            let st = fmt::s(s, "state");
            st.is_empty() || st == "ready"
        })
    }

    fn result(&self, src: &str, i: usize) -> Option<&Value> {
        self.results.get(src).and_then(|l| l.get(i))
    }
}

/// The body of `POST /api/picker/search` for one source: the opts' words, the search text, the
/// request it is for (the server finds the request's own kept searches) and, for Search again,
/// `fresh`.
fn search_body(state: &State, src: &str, q: &str, fresh: bool) -> Value {
    let o = &state.opts;
    let mut body = Map::new();
    body.insert("source".into(), json!(src));
    body.insert("kind".into(), json!(state.kind));
    body.insert(
        "artist".into(),
        if fmt::truthy(o, "artist") { o["artist"].clone() } else { json!("") },
    );
    body.insert(
        "title".into(),
        if fmt::truthy(o, "title") {
            o["title"].clone()
        } else if fmt::truthy(o, "album") {
            o["album"].clone()
        } else {
            json!("")
        },
    );
    body.insert("query".into(), json!(q));
    if state.kind == "track" && o.get("album").is_some() {
        body.insert("album".into(), o["album"].clone());
    }
    // JSON.stringify drops undefined: only keys opts has go out
    for k in ["mbid", "artist_mbid", "track_count", "length"] {
        if o.get(k).is_some() {
            body.insert(k.into(), o[k].clone());
        }
    }
    if fmt::truthy(o, "request_id") {
        body.insert("request_id".into(), o["request_id"].clone());
    }
    if fresh {
        body.insert("fresh".into(), json!(true));
    }
    Value::Object(body)
}

/// One search of one source; `done` wraps the answer. A Soulseek search that can search live
/// first asks whether another search holds the turn (asked before this one is sent, so it can't
/// be this one).
fn search_task(
    state: &State,
    src: &str,
    q: &str,
    fresh: bool,
    ctx: &Ctx,
    done: impl Fn(u64, String, Result<Value, ApiError>) -> Message + Send + 'static,
) -> Task<Message> {
    let api = ctx.api.clone();
    let gen = state.gen;
    let id = src.to_string();
    let body = search_body(state, src, q, fresh);
    let search = Task::perform(async move { api.post("/picker/search", body).await }, move |r| done(gen, id, r));
    if src == "slskd" && state.ready(src) {
        let api = ctx.api.clone();
        return Task::perform(async move { api.get("/system/soulseek").await }, move |r| Message::SlskTurn(gen, r))
            .chain(search);
    }
    search
}

/// `run(q)`: one search per source, painting as each one lands.
fn run(state: &mut State, q: &str, ctx: &Ctx) -> Task<Message> {
    state.gen += 1;
    state.results.clear();
    state.kept.clear();
    state.errors.clear();
    state.loading.clear();
    state.again.clear();
    state.again_errors.clear();
    state.notes.clear();
    state.slsk_waiting = false;
    state.searched = q.to_string();
    let mut tasks = Vec::new();
    for s in state.sources().to_vec() {
        if !searches(&s) {
            continue;
        }
        let id = fmt::s(&s, "id");
        state.loading.insert(id.clone());
        tasks.push(search_task(state, &id, q, false, ctx, Message::Searched));
    }
    Task::batch(tasks)
}

/// An answer in place: its rows, and the kept search's fields when it came from one.
fn land(state: &mut State, src: String, v: &Value) {
    if is_kept(v) {
        let mut meta = v.clone();
        if let Some(o) = meta.as_object_mut() {
            o.remove("items");
        }
        state.kept.insert(src.clone(), meta);
    } else {
        state.kept.remove(&src);
    }
    state.errors.remove(&src);
    match empty_note(v) {
        Some(n) => state.notes.insert(src.clone(), n),
        None => state.notes.remove(&src),
    };
    state.results.insert(src, fmt::items(v));
}

/// The server's reason why an answer has no rows (its `notice`), when it has none and sent one –
/// not for a kept answer that can't search again, whose line already shows the notice.
pub fn empty_note(answer: &Value) -> Option<String> {
    let note = fmt::s(answer, "notice").trim().to_string();
    let shown_by_line = is_kept(answer) && !can_search(answer);
    (fmt::items(answer).is_empty() && !note.is_empty() && !shown_by_line).then_some(note)
}

/// `download(source, payload, title)`: `POST /api/picker/pick`.
fn download(state: &mut State, source: &str, payload: Option<Value>, title: Option<Value>, ctx: &Ctx) -> Task<Message> {
    if state.sending {
        return Task::none();
    }
    state.sending = true;
    let body = pick_body(state, source, payload, title);
    let api = ctx.api.clone();
    Task::perform(async move { api.post("/picker/pick", body).await }, Message::Picked)
}

/// The body of `POST /api/picker/pick`: the result, and the request it is for (or the request to
/// make), with `no_check` when "Don't check the audio" is on.
fn pick_body(state: &State, source: &str, payload: Option<Value>, title: Option<Value>) -> Value {
    let o = &state.opts;
    let mut req = Map::new();
    req.insert("kind".into(), json!(state.kind));
    req.insert(
        "mbid".into(),
        if fmt::truthy(o, "mbid") { o["mbid"].clone() } else { Value::Null },
    );
    if o.get("artist").is_some() {
        req.insert("artist".into(), o["artist"].clone());
    }
    // title: opts.title || opts.album
    if fmt::truthy(o, "title") {
        req.insert("title".into(), o["title"].clone());
    } else if o.get("album").is_some() {
        req.insert("title".into(), o["album"].clone());
    }
    // album: opts.album || (kind === 'album' ? opts.title : null)
    if fmt::truthy(o, "album") {
        req.insert("album".into(), o["album"].clone());
    } else if state.kind == "album" {
        if o.get("title").is_some() {
            req.insert("album".into(), o["title"].clone());
        }
    } else {
        req.insert("album".into(), Value::Null);
    }
    for k in ["year", "cover_url", "length", "isrc"] {
        if o.get(k).is_some() {
            req.insert(k.into(), o[k].clone());
        }
    }
    req.insert("album_mode".into(), json!("song"));
    if fmt::truthy(o, "ext") {
        req.insert("ext".into(), o["ext"].clone());
    }
    let mut body = Map::new();
    body.insert("source".into(), json!(source));
    if let Some(p) = payload {
        body.insert("payload".into(), p);
    }
    if let Some(t) = title {
        body.insert("title".into(), t);
    }
    if fmt::truthy(o, "request_id") {
        body.insert("request_id".into(), o["request_id"].clone());
    } else {
        body.insert("request".into(), Value::Object(req));
    }
    if state.no_check && state.may_skip {
        body.insert("no_check".into(), json!(true));
    }
    Value::Object(body)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Sources(Ok(info)) => {
            if !fmt::truthy(&info, "allowed") {
                ctx.toast_err("Manual search is limited to admins");
                state.closed = true;
                return Task::none();
            }
            state.sources = Some(fmt::items(&info["sources"]));
            let q = state.q.clone();
            return run(state, &q, ctx);
        }
        Message::Sources(Err(e)) => {
            ctx.toast_err(e.message);
            state.closed = true;
        }
        Message::Query(q) => state.q = q,
        Message::Search => {
            let v = state.q.trim().to_string();
            if is_url(&v) {
                return download(state, "link", Some(json!({ "url": v })), Some(json!(v)), ctx);
            }
            return run(state, &v, ctx);
        }
        Message::GetLink => {
            let v = state.q.trim().to_string();
            if is_url(&v) {
                return download(state, "link", Some(json!({ "url": v })), Some(json!(v)), ctx);
            }
        }
        Message::Link(l) => state.link = l,
        Message::DownloadLink => {
            let url = state.link.trim().to_string();
            if !url.is_empty() {
                return download(state, "link", Some(json!({ "url": url })), Some(json!(url)), ctx);
            }
        }
        Message::Tab(t) => state.tab = t,
        Message::Lossless(on) => state.lossless = on,
        Message::NoCheck(on) => {
            state.no_check = on;
            ctx.edit_config(move |c| {
                c.local.insert(NO_CHECK_KEY.into(), Value::String(if on { "1" } else { "0" }.into()));
            });
        }
        Message::SlskTurn(gen, r) => {
            if gen == state.gen && r.is_ok_and(|v| fmt::truthy(&v, "search_running")) {
                // the waiting line shows only when the answer takes more than a moment
                return Task::perform(tokio::time::sleep(Duration::from_secs(1)), move |_| Message::SlskWait(gen));
            }
        }
        Message::SlskWait(gen) => {
            if gen == state.gen && (state.loading.contains("slskd") || state.again.contains("slskd")) {
                state.slsk_waiting = true;
            }
        }
        Message::Searched(gen, src, r) => {
            if gen != state.gen {
                return Task::none();
            }
            state.loading.remove(&src);
            if src == "slskd" {
                state.slsk_waiting = false;
            }
            match r {
                Ok(v) => land(state, src, &v),
                // a 409 carries the source's state and settings link
                Err(e) => {
                    state.errors.insert(src, PickErr::from_api(&e));
                }
            }
        }
        Message::Again(src) => {
            if state.loading.contains(&src) || !state.again.insert(src.clone()) {
                return Task::none();
            }
            state.again_errors.remove(&src);
            let q = state.searched.clone();
            return search_task(state, &src, &q, true, ctx, Message::Agained);
        }
        Message::Agained(gen, src, r) => {
            // a new search started meanwhile: its answer wins
            if gen != state.gen {
                return Task::none();
            }
            state.again.remove(&src);
            if src == "slskd" {
                state.slsk_waiting = false;
            }
            match r {
                Ok(v) => land(state, src, &v),
                // the kept rows stay, with why it could not search again
                Err(e) => {
                    state.again_errors.insert(src, PickErr::from_api(&e));
                }
            }
        }
        Message::Get(src, i) => {
            let Some(x) = state.result(&src, i).cloned() else {
                return Task::none();
            };
            return download(state, &src, x.get("payload").cloned(), x.get("title").cloned(), ctx);
        }
        Message::Picked(Ok(r)) => {
            ctx.toast_ok(if r["status"].as_str() == Some("pending") {
                "Sent for approval – your pick will be used"
            } else {
                "Downloading your pick"
            });
            state.sending = false;
            state.picked = Some(r);
            state.closed = true;
        }
        Message::Picked(Err(e)) => {
            ctx.toast_err(e.message);
            state.sending = false;
        }
        Message::OpenSettings(hash) => {
            ctx.navigate(settings_page(&hash));
            state.closed = true;
        }
        Message::CopyError(text) => ctx.copy_text(text),
        Message::Close => state.closed = true,
    }
    Task::none()
}

// ----- view -----------------------------------------------------------------------------

/// A small accent link.
fn link<'a, M: Clone + 'a>(label: String, on: M) -> El<'a, M> {
    let p = theme::look();
    button(t(label).size(12.5).color(p.accent))
        .padding(0)
        .style(theme::btn(Btn::Row))
        .on_press(on)
        .into()
}

/// `hint – Settings → X`: a hint with its settings link (the link alone when the hint is empty).
fn hint_line<'a>(hint: &str, settings: &str, label: &str, err: bool) -> El<'a, Message> {
    let p = theme::look();
    let mut r = row![].spacing(4).align_y(Alignment::Center);
    if !hint.is_empty() {
        let mut txt = t(hint.to_string()).size(12.5);
        txt = if err { txt.color(p.err) } else { txt.color(p.muted) };
        r = r.push(txt);
    }
    if !settings.is_empty() {
        if !hint.is_empty() {
            r = r.push(small_muted("–"));
        }
        let words = if label.is_empty() { "Settings" } else { label };
        r = r.push(link(words.to_string(), Message::OpenSettings(settings.to_string())));
    }
    r.wrap().into()
}

/// A tab row whose labels carry a count or a word (the picker's sources, Soulseek's
/// Transfers): `items` are `(id, label, suffix)`.
pub fn count_tabs<'a, M: Clone + 'a>(
    items: &[(String, String, String)],
    active: &str,
    on: impl Fn(String) -> M,
) -> El<'a, M> {
    let p = theme::look();
    let mut r = row![].spacing(4);
    for (id, label, suffix) in items {
        let is = id == active;
        let mut inner = row![t(label.clone()).size(13.5).font(theme::semibold())]
            .spacing(6)
            .align_y(Alignment::Center);
        if !suffix.is_empty() {
            inner = inner.push(t(suffix.clone()).size(12).color(p.faint));
        }
        r = r.push(
            button(inner)
                .padding([7, 14])
                .style(theme::btn(if is { Btn::TabOn } else { Btn::Tab }))
                .on_press(on(id.clone())),
        );
    }
    container(r.wrap()).padding(4).style(theme::tabs_frame).into()
}

/// The JS `row(x, i)`: one result with its Get button.
fn result_row<'a>(ctx: &Ctx<'a>, state: &State, src: &str, i: usize, x: &Value) -> El<'a, Message> {
    let p = ctx.look;
    let title = fmt::s(x, "title");
    let mut lines = column![t(title.clone()).size(14).font(theme::semibold())].spacing(2);
    if fmt::truthy(x, "subtitle") && x["subtitle"] != x["title"] {
        lines = lines.push(small_muted(fmt::s(x, "subtitle")));
    }
    let quality = if fmt::truthy(x, "quality") { fmt::s(x, "quality") } else { "?".into() };
    let mut meta = row![badge(
        quality,
        if fmt::truthy(x, "lossless") { Badge::Ok } else { Badge::Plain }
    )]
    .spacing(10)
    .align_y(Alignment::Center);
    if fmt::truthy(x, "tracks") {
        let n = fmt::i(x, "tracks");
        meta = meta.push(small_muted(format!("{n} file{}", if n == 1 { "" } else { "s" })));
    }
    if fmt::truthy(x, "duration") {
        meta = meta.push(small_muted(fmt::mmss(fmt::f(x, "duration"))));
    }
    if fmt::truthy(x, "size") {
        meta = meta.push(small_muted(fmt::size(fmt::f(x, "size"))));
    }
    if fmt::truthy(x, "info") {
        meta = meta.push(small_muted(fmt::s(x, "info")));
    }
    lines = lines.push(meta.wrap());
    if fmt::truthy(x, "warning") {
        lines = lines.push(t(fmt::s(x, "warning")).size(12).color(p.warn));
    }
    let mut r = row![].spacing(12).align_y(Alignment::Center);
    if fmt::truthy(x, "cover") {
        r = r.push(catalog::art(ctx, &fmt::s(x, "cover"), 44.0, false, Icon::Disc));
    }
    r = r.push(lines.width(Length::Fill));
    r = r.push(btn_icon(
        Icon::Download,
        "Get",
        Btn::Primary,
        (!state.sending).then_some(Message::Get(src.to_string(), i)),
    ));
    column![container(r).padding([8, 4]), super::divider()].into()
}

/// A source's error in full, wrapped, with its settings link and Copy for a bug report.
fn error_line<'a>(name: &str, err: &PickErr) -> El<'a, Message> {
    let copy = super::details::copy_btn(Message::CopyError(format!("{name}: {}", err.message)));
    container(
        row![
            container(hint_line(&err.message, &err.settings, &err.label, true)).width(Length::Fill),
            copy
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(theme::error_box)
    .into()
}

/// The line of a kept answer: "Found at 14:02 · Search again"; while Search again runs
/// "Searching Soulseek again…" (or the wait for another Soulseek search); then, when it failed,
/// why, with the kept rows still shown.
fn found_row<'a>(state: &State, src: &str, name: &str, answer: &Value, empty: bool) -> El<'a, Message> {
    let mut col = column![].spacing(6);
    if state.again.contains(src) {
        col = col.push(small_muted(if src == "slskd" && state.slsk_waiting {
            SLSK_WAITING.to_string()
        } else {
            searching_again_text(name)
        }));
    } else if let Some((text, again)) = found_line(answer, &state.searched, empty) {
        let mut r = row![].spacing(4).align_y(Alignment::Center);
        if !text.is_empty() {
            r = r.push(small_muted(text.clone()));
        }
        if again {
            if !text.is_empty() {
                r = r.push(small_muted("·"));
            }
            r = r.push(link("Search again".into(), Message::Again(src.to_string())));
        }
        col = col.push(r.wrap());
    }
    if let Some(err) = state.again_errors.get(src).filter(|e| !e.message.is_empty() && !state.again.contains(src)) {
        col = col.push(error_line(name, err));
    }
    col.into()
}

/// The JS `section(src)`.
fn section<'a>(ctx: &Ctx<'a>, state: &State, src: &str) -> El<'a, Message> {
    let p = ctx.look;
    let full: &[Value] = state.results.get(src).map(|v| v.as_slice()).unwrap_or(&[]);
    // (index in the unfiltered list, x)
    let list: Vec<(usize, &Value)> = full
        .iter()
        .enumerate()
        .filter(|(_, x)| !state.lossless || fmt::truthy(x, "lossless"))
        .collect();
    let name = state.name(src);
    let kept = state.kept.get(src);
    let mut col = column![].spacing(8);
    if state.tab == "all" {
        let mut head = row![h3(name.clone())].spacing(8).align_y(Alignment::Center);
        if state.ready(src) || kept.is_some() {
            head = head.push(small_muted(list.len().to_string()).color(p.faint));
        }
        col = col.push(container(head).padding(super::pad(8.0, 0.0, 0.0, 0.0)));
    }
    // a signed-out, stopped or switched-off source: "Sign in to Tidal – Settings → streamrip" –
    // unless the server answered from a kept search (Soulseek while it is not connected)
    if let Some(s) = state.source(src).filter(|_| !state.ready(src) && kept.is_none()) {
        return col
            .push(hint_line(&fmt::s(s, "hint"), &fmt::s(s, "settings"), &fmt::s(s, "settings_label"), false))
            .into();
    }
    if let Some(err) = state.errors.get(src).filter(|e| !e.message.is_empty()) {
        // the whole error, wrapped, and Copy for a bug report
        return col.push(error_line(&name, err)).into();
    }
    if state.loading.contains(src) && src == "slskd" && state.slsk_waiting {
        return col.push(small_muted(SLSK_WAITING)).into();
    }
    if state.loading.contains(src) {
        return col.push(small_muted(searching_text(src, &name))).into();
    }
    if let Some(answer) = kept {
        // "Found at 14:02 · Search again" ("No results. · Found at …" when it found nothing)
        col = col.push(found_row(state, src, &name, answer, list.is_empty()));
        if list.is_empty() {
            // the line says "No results."; the server's reason under it
            if let Some(n) = state.notes.get(src) {
                col = col.push(small_muted(n.clone()));
            }
            return col.into();
        }
    }
    if list.is_empty() {
        return col.push(small_muted(match state.notes.get(src) {
            Some(n) => format!("No results – {n}"),
            None => "No results.".to_string(),
        })).into();
    }
    let shown = &list[..list.len().min(if state.tab == "all" { 8 } else { 150 })];
    let mut rows = column![];
    for (i, x) in shown {
        rows = rows.push(result_row(ctx, state, src, *i, x));
    }
    col = col.push(rows);
    if state.tab == "all" && list.len() > shown.len() {
        col = col.push(btn_sm(
            format!("Show all {} from {name}", list.len()),
            Btn::Ghost,
            Some(Message::Tab(src.to_string())),
        ));
    }
    col.into()
}

/// The dialog. The page returns it from `overlay` mapped to its own message.
pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let search = row![
        text_input("Search, or paste a YouTube Music / Bandcamp / any link", &state.q)
            .on_input(Message::Query)
            .on_submit(Message::Search)
            .padding([9, 12])
            .size(14)
            .style(theme::input),
        btn("Search", Btn::Primary, Some(Message::Search)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut body = column![search].spacing(14).width(Length::Fill);
    if is_url(&state.q) {
        let accent = p.accent;
        body = body.push(
            container(
                row![
                    column![
                        t("Download this link").size(14).font(theme::semibold()),
                        small_muted(state.q.trim().to_string()),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    btn_icon(
                        Icon::Download,
                        "Get",
                        Btn::Primary,
                        (!state.sending).then_some(Message::GetLink)
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .padding([10, 12])
            .width(Length::Fill)
            .style(move |_| container::Style {
                border: Border {
                    color: accent,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                ..container::Style::default()
            }),
        );
    }
    // the tabs: every source has one; one that cannot search shows why in a word instead of a count
    let all: usize = state.results.values().map(Vec::len).sum();
    let busy = !state.loading.is_empty();
    let mut entries: Vec<(String, String, String)> =
        vec![("all".into(), "All".into(), if busy { "…".into() } else { all.to_string() })];
    for s in state.sources() {
        let id = fmt::s(s, "id");
        let n = state.results.get(&id).map(Vec::len).unwrap_or(0);
        // a source not ready that answered from a kept search counts its rows
        let word = if state.ready(&id) || state.kept.contains_key(&id) { "" } else { state_word(&fmt::s(s, "state")) };
        // (Search again keeps the rows, and the tab says it is searching, as the web's spinner)
        let suffix = if state.loading.contains(&id) || state.again.contains(&id) {
            "…".to_string()
        } else if word.is_empty() {
            n.to_string()
        } else {
            word.to_string()
        };
        entries.push((id, fmt::s(s, "name"), suffix));
    }
    // the switches sit beside the tabs, and under them when the tabs take the width; "Don't check the
    // audio" only for those who may take it
    let mut switches = row![checkbox(state.lossless)
        .label("Lossless only")
        .on_toggle(Message::Lossless)
        .size(18)
        .text_size(13)]
    .spacing(16)
    .align_y(Alignment::Center);
    if state.may_skip {
        switches = switches.push(
            tooltip(
                checkbox(state.no_check)
                    .label(NO_CHECK)
                    .on_toggle(Message::NoCheck)
                    .size(18)
                    .text_size(13),
                super::tip_box(NO_CHECK_TIP),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    body = body.push(
        row![count_tabs(&entries, &state.tab, Message::Tab), switches]
            .spacing(16)
            .align_y(Alignment::Center)
            .wrap()
            .vertical_spacing(10),
    );
    let out: El<'a, Message> = match &state.sources {
        None => super::skeleton_rows(3),
        Some(s) if s.is_empty() => muted("No download sources are enabled – set them up in Settings.")
            .size(13)
            .into(),
        Some(_) => {
            let ids: Vec<String> = if state.tab == "all" {
                state.sources().iter().map(|s| fmt::s(s, "id")).collect()
            } else {
                vec![state.tab.clone()]
            };
            let mut col = column![].spacing(10);
            for id in ids {
                col = col.push(section(ctx, state, &id));
            }
            col.into()
        }
    };
    body = body.push(container(out).width(Length::Fill));
    body = body.push(
        row![
            text_input(
                "…or paste a link: Qobuz, Tidal, Deezer, YouTube, Bandcamp, SoundCloud…",
                &state.link
            )
            .on_input(Message::Link)
            .on_submit(Message::DownloadLink)
            .padding([9, 12])
            .size(14)
            .style(theme::input),
            btn_icon(
                Icon::Download,
                "Download link",
                Btn::Default,
                (!state.sending).then_some(Message::DownloadLink)
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );
    body = body.push(
        super::faint(if state.no_check { FOOT_NO_CHECK } else { FOOT }).size(12),
    );
    modal(state.title.clone(), body, None, true, Message::Close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_settings_links() {
        assert!(is_url("https://music.youtube.com/watch?v=x"));
        assert!(is_url("  HTTP://a.b/c  "));
        assert!(!is_url("https://"));
        assert!(!is_url("https://a b"));
        assert!(!is_url("daft punk discovery"));
        assert_eq!(settings_page("#/settings?s=playback"), Page::Settings(Section::Playback));
        // the source cards are plugin cards since 0.32; the server's hints link the Plugins page itself
        assert_eq!(settings_page("#/settings?s=slskd"), Page::Settings(Section::Plugins));
        assert_eq!(settings_page("#/settings?s=plugins"), Page::Settings(Section::Plugins));
        assert_eq!(settings_page("#/settings"), Page::Settings(Section::Plugins));
        assert_eq!(state_word("sign_in"), "sign in");
        assert_eq!(state_word("ready"), "");
    }

    /// A picker state without a server (the tests build the bodies and lines from it).
    fn state(opts: Value, sources: Value) -> State {
        State {
            kind: if opts["kind"].as_str() == Some("track") { "track" } else { "album" },
            title: String::new(),
            opts,
            sources: Some(fmt::items(&sources)),
            q: String::new(),
            link: String::new(),
            tab: "all".into(),
            lossless: false,
            no_check: false,
            may_skip: true,
            searched: "Duumu Illuminate EP".into(),
            results: HashMap::new(),
            kept: HashMap::new(),
            errors: HashMap::new(),
            loading: HashSet::new(),
            again: HashSet::new(),
            again_errors: HashMap::new(),
            notes: HashMap::new(),
            slsk_waiting: false,
            gen: 0,
            sending: false,
            closed: false,
            picked: None,
        }
    }

    #[test]
    fn kept_searches_read_like_the_design() {
        let kept = json!({ "from": "cache", "cached_at": 1_760_875_320, "found": "Found at 14:02",
            "query": "Duumu Illuminate EP", "can_search": true, "notice": null });
        let typed = "Duumu Illuminate EP";
        assert_eq!(found_line(&kept, typed, false), Some(("Found at 14:02".into(), true)));
        // a kept search that found nothing
        assert_eq!(found_line(&kept, typed, true), Some(("No results. · Found at 14:02".into(), true)));
        // searched with other words (the worker's album-only search)
        let mut other = kept.clone();
        other["query"] = json!("Illuminate EP");
        assert_eq!(found_line(&other, typed, false), Some(("Found at 14:02 for \"Illuminate EP\"".into(), true)));
        // the same words in another order or case are the same search
        assert_eq!(found_line(&kept, "illuminate ep, DUUMU", false), Some(("Found at 14:02".into(), true)));
        // Soulseek not connected: the reason instead of Search again
        let mut down = kept.clone();
        down["can_search"] = json!(false);
        down["notice"] = json!("Soulseek isn't connected – next attempt at 14:20");
        assert_eq!(
            found_line(&down, typed, false),
            Some(("Found at 14:02 · Soulseek isn't connected – next attempt at 14:20".into(), false))
        );
        assert_eq!(
            found_line(&down, typed, true),
            Some(("No results. · Found at 14:02 · Soulseek isn't connected – next attempt at 14:20".into(), false))
        );
        down["can_search"] = json!(0);
        assert!(!can_search(&down));
        // a live answer, an older server's bare list: nothing to say
        assert_eq!(found_line(&json!({ "items": [], "from": "live", "cached_at": null }), typed, false), None);
        assert_eq!(found_line(&json!([]), typed, false), None);
        // the server's reason for an empty answer; none for a full one, nor where the kept line shows it
        let why = json!({ "items": [], "from": "live", "notice": "Prowlarr's indexers found nothing for \"x\"" });
        assert_eq!(empty_note(&why).as_deref(), Some("Prowlarr's indexers found nothing for \"x\""));
        assert_eq!(empty_note(&json!({ "items": [{}], "notice": "n" })), None);
        assert_eq!(empty_note(&json!({ "items": [], "from": "cache", "can_search": false, "notice": "n" })), None);
        assert!(can_search(&json!({ "items": [] })));
        // no sentence from the server: the time it was kept, in UTC
        assert_eq!(
            found_line(&json!({ "from": "cache", "cached_at": 1_792_427_497.2 }), typed, false),
            Some(("Found at 2026-10-19 16:31 UTC".into(), true))
        );
        assert_eq!(searching_text("slskd", "Soulseek"), "Searching Soulseek… (Soulseek takes a few seconds)");
        assert_eq!(searching_text("deezer", "Deezer"), "Searching Deezer…");
        assert_eq!(searching_again_text("Soulseek"), "Searching Soulseek again…");
        assert_eq!(SLSK_WAITING, "Waiting for another Soulseek search, then searching…");
    }

    #[test]
    fn soulseek_is_asked_while_it_is_not_connected() {
        assert!(searches(&json!({ "id": "deezer", "state": "ready" })));
        assert!(searches(&json!({ "id": "deezer" })), "an older server sends no state");
        assert!(searches(&json!({ "id": "slskd", "state": "not_connected" })), "a kept search can still answer");
        assert!(!searches(&json!({ "id": "slskd", "state": "off" })));
        assert!(!searches(&json!({ "id": "torznab", "state": "not_connected" })), "no download client: its rows can't be used");
        assert!(!searches(&json!({ "id": "qobuz", "state": "sign_in" })));
    }

    #[test]
    fn a_kept_answer_lands_with_its_fields() {
        let mut st = state(json!({ "kind": "album", "artist": "Duumu", "album": "Illuminate EP" }), json!([{ "id": "slskd" }]));
        land(&mut st, "slskd".into(), &json!({ "items": [{ "title": "Illuminate EP" }], "from": "cache", "found": "Found at 14:02" }));
        assert_eq!(st.results["slskd"].len(), 1);
        assert_eq!(st.kept["slskd"]["found"], "Found at 14:02");
        assert!(st.kept["slskd"].get("items").is_none(), "the rows are kept once");
        // Search again answered live: the line goes
        land(&mut st, "slskd".into(), &json!({ "items": [], "from": "live" }));
        assert!(st.kept.is_empty() && st.results["slskd"].is_empty());
        // today's bare list
        land(&mut st, "slskd".into(), &json!([{ "title": "x" }]));
        assert_eq!(st.results["slskd"].len(), 1);
        assert!(st.kept.is_empty());
    }

    #[test]
    fn searches_name_their_request_and_search_again_is_fresh() {
        let st = state(
            json!({ "kind": "album", "artist": "Duumu", "album": "Illuminate EP", "track_count": 4, "request_id": 123 }),
            json!([{ "id": "slskd" }]),
        );
        let b = search_body(&st, "slskd", "Duumu Illuminate EP", false);
        assert_eq!(
            b,
            json!({ "source": "slskd", "kind": "album", "artist": "Duumu", "title": "Illuminate EP",
                    "query": "Duumu Illuminate EP", "track_count": 4, "request_id": 123 })
        );
        assert_eq!(search_body(&st, "slskd", "Duumu Illuminate EP", true)["fresh"], true);
        // opened for an album, not a request: no request id
        let st = state(json!({ "kind": "album", "artist": "Duumu", "album": "Illuminate EP" }), json!([]));
        assert!(search_body(&st, "deezer", "x", false).get("request_id").is_none());
    }

    #[test]
    fn dont_check_the_audio_goes_with_the_pick() {
        let mut st = state(json!({ "kind": "track", "artist": "Machine Time", "title": "Moonlight Shadow (nightcore)", "request_id": 12 }), json!([]));
        let b = pick_body(&st, "ytmusic", Some(json!({ "url": "https://music.youtube.com/watch?v=x" })), Some(json!("Moonlight Shadow")));
        assert_eq!(b, json!({ "source": "ytmusic", "payload": { "url": "https://music.youtube.com/watch?v=x" }, "title": "Moonlight Shadow", "request_id": 12 }));
        st.no_check = true;
        let b = pick_body(&st, "ytmusic", None, None);
        assert_eq!(b["no_check"], true);
        assert_eq!(b["request_id"], 12);
        // without Fix their own downloads the switch isn't offered, and a switch left on on this computer
        // isn't sent (the server would refuse the pick)
        st.may_skip = false;
        assert!(pick_body(&st, "ytmusic", None, None).get("no_check").is_none());
        assert_eq!(NO_CHECK, "Don't check the audio");
        assert!(FOOT.starts_with("The file you pick is still fingerprint-checked"));
    }

    #[test]
    fn title_and_first_query_follow_the_web() {
        let opts = json!({ "kind": "track", "artist": "Daft Punk", "title": "Digital Love", "album": "Discovery" });
        assert_eq!(title_or_album(&opts), json!("Digital Love"));
        let opts = json!({ "kind": "album", "artist": "Daft Punk", "album": "Discovery" });
        assert_eq!(js(&title_or_album(&opts)), "Discovery");
        assert_eq!(js(&Value::Null), "");
        assert_eq!(js(&json!(3)), "3");
    }
}
