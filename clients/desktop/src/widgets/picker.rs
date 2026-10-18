//! The download picker (web-rs/src/pages/picker.rs `picker_modal`): search every source by hand,
//! see quality, size and uploader, and download exactly the result you choose. A page holds
//! `Option<State>`, routes its messages here with [`update`], draws [`view`] from its `overlay`,
//! reloads its list when `picked` is set and drops the state once `closed` is set.
//!
//! `opts` is the web's: `{ kind: 'album'|'track', artist, title, album, mbid, artist_mbid,
//! track_count, length, year, cover_url, isrc, request_id, ext }`.

use super::catalog;
use super::{badge, btn, btn_icon, btn_sm, h3, modal, muted, small_muted, t, El, Icon};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, Page, Section};
use crate::theme::{self, Badge, Btn};
use iced::widget::{button, checkbox, column, container, row, space, text_input};
use iced::{Alignment, Border, Length, Task};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

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
    results: HashMap<String, Vec<Value>>,
    errors: HashMap<String, PickErr>,
    loading: HashSet<String>,
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
    /// `(generation, source, answer)`.
    Searched(u64, String, Result<Value, ApiError>),
    /// Get the `i`th result of `source`.
    Get(String, usize),
    Picked(Result<Value, ApiError>),
    /// A settings link in a hint (`#/settings?s=slskd`).
    OpenSettings(String),
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

/// The settings page of a web link (`#/settings?s=slskd`): a section the app has, or Apps &
/// sources, where the server's download sources are set up.
pub fn settings_page(hash: &str) -> Page {
    let id = hash
        .split_once("s=")
        .map(|(_, r)| r.split('&').next().unwrap_or(""))
        .unwrap_or("");
    Page::Settings(Section::by_id(id).unwrap_or(Section::Apps))
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
    let state = State {
        title: format!("Pick a download · {}", js(&toa)),
        opts,
        kind,
        sources: None,
        q: q0,
        link: String::new(),
        tab: "all".into(),
        lossless: false,
        results: HashMap::new(),
        errors: HashMap::new(),
        loading: HashSet::new(),
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

/// `run(q)`: one search per source, painting as each one lands.
fn run(state: &mut State, q: &str, ctx: &Ctx) -> Task<Message> {
    state.gen += 1;
    state.results.clear();
    state.errors.clear();
    state.loading.clear();
    let o = &state.opts;
    let mut tasks = Vec::new();
    for s in state.sources().to_vec() {
        let id = fmt::s(&s, "id");
        if !state.ready(&id) {
            continue;
        }
        state.loading.insert(id.clone());
        let mut body = Map::new();
        if s.get("id").is_some() {
            body.insert("source".into(), s["id"].clone());
        }
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
        let api = ctx.api.clone();
        let gen = state.gen;
        tasks.push(Task::perform(
            async move { api.post("/picker/search", Value::Object(body)).await },
            move |r| Message::Searched(gen, id.clone(), r),
        ));
    }
    Task::batch(tasks)
}

/// `download(source, payload, title)`: `POST /api/picker/pick`.
fn download(state: &mut State, source: &str, payload: Option<Value>, title: Option<Value>, ctx: &Ctx) -> Task<Message> {
    if state.sending {
        return Task::none();
    }
    state.sending = true;
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
    let api = ctx.api.clone();
    Task::perform(
        async move { api.post("/picker/pick", Value::Object(body)).await },
        Message::Picked,
    )
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
        Message::Searched(gen, src, r) => {
            if gen != state.gen {
                return Task::none();
            }
            state.loading.remove(&src);
            match r {
                Ok(v) => {
                    state.results.insert(src, fmt::items(&v));
                }
                // a 409 carries the source's state and settings link
                Err(e) => {
                    state.errors.insert(src, PickErr::from_api(&e));
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
    let mut col = column![].spacing(8);
    if state.tab == "all" {
        let mut head = row![h3(name.clone())].spacing(8).align_y(Alignment::Center);
        if state.ready(src) {
            head = head.push(small_muted(list.len().to_string()).color(p.faint));
        }
        col = col.push(container(head).padding(super::pad(8.0, 0.0, 0.0, 0.0)));
    }
    // a signed-out, stopped or switched-off source: "Sign in to Tidal – Settings → streamrip"
    if let Some(s) = state.source(src).filter(|_| !state.ready(src)) {
        return col
            .push(hint_line(&fmt::s(s, "hint"), &fmt::s(s, "settings"), &fmt::s(s, "settings_label"), false))
            .into();
    }
    if let Some(err) = state.errors.get(src).filter(|e| !e.message.is_empty()) {
        return col
            .push(
                container(hint_line(&err.message, &err.settings, &err.label, true))
                    .padding([8, 12])
                    .width(Length::Fill)
                    .style(theme::error_box),
            )
            .into();
    }
    if state.loading.contains(src) {
        return col
            .push(small_muted(format!(
                "Searching {name}…{}",
                if src == "slskd" { " (Soulseek takes a few seconds)" } else { "" }
            )))
            .into();
    }
    if list.is_empty() {
        return col.push(small_muted("No results.")).into();
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
        let word = if state.ready(&id) { "" } else { state_word(&fmt::s(s, "state")) };
        let suffix = if state.loading.contains(&id) {
            "…".to_string()
        } else if word.is_empty() {
            n.to_string()
        } else {
            word.to_string()
        };
        entries.push((id, fmt::s(s, "name"), suffix));
    }
    body = body.push(
        row![
            count_tabs(&entries, &state.tab, Message::Tab),
            space().width(Length::Fill),
            checkbox(state.lossless)
                .label("Lossless only")
                .on_toggle(Message::Lossless)
                .size(18)
                .text_size(13),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
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
        super::faint(
            "The file you pick is still fingerprint-checked, tagged with MusicBrainz data and de-duplicated. If it fails, the request fails instead of trying other sources.",
        )
        .size(12),
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
        assert_eq!(settings_page("#/settings?s=slskd"), Page::Settings(Section::Apps));
        assert_eq!(settings_page("#/settings"), Page::Settings(Section::Apps));
        assert_eq!(state_word("sign_in"), "sign in");
        assert_eq!(state_word("ready"), "");
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
