//! /api/picker: search one source by hand, then download exactly the chosen result.
use crate::auth::{self, User};
use encore_db::RowExt;
use encore_core::error::{err, ApiError, ApiResult};
use encore_library::matcher::{compare, describe};
use crate::routes::requests::get_owned;
use encore_requests::create::{quota, NewRequest};
use encore_http::{deezer, tidal, ytmusic};
use encore_requests::search_cache;
use encore_requests::sources::{slskd, streamrip, torznab, ytdlp};
use crate::state::AppState;
use encore_db::p;
use encore_requests::worker;
use axum::extract::State;
use encore_core::audio::LOSSLESS;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/picker/sources", get(sources))
        .route("/api/picker/search", post(search))
        .route("/api/picker/pick", post(pick))
}

/// The picker's sources in tab order: (id, name, the settings section that sets it up).
const SOURCES: [(&str, &str, &str); 6] = [
    ("slskd", "Soulseek", "slskd"),
    ("torznab", "Torrents & Usenet", "torznab"),
    ("qobuz", "Qobuz", "streamrip"),
    ("tidal", "Tidal", "streamrip"),
    ("deezer", "Deezer", "streamrip"),
    ("ytmusic", "YouTube Music", "ytdlp"),
];

fn source_name(id: &str) -> &'static str {
    SOURCES.iter().find(|s| s.0 == id).map(|s| s.1).unwrap_or("")
}

/// What a source can do right now. `state`: ready | sign_in (a login is missing) | not_connected (set up, but
/// not reachable right now) | not_installed | off. `hint` says why in a few words when not ready; `section`
/// is the settings section where it is fixed.
pub struct SourceState {
    pub state: &'static str,
    pub hint: String,
    pub section: &'static str,
}

fn state(state: &'static str, hint: impl Into<String>, section: &'static str) -> SourceState {
    SourceState { state, hint: hint.into(), section }
}

/// The settings link for a section, as the web UI addresses it, and its label.
fn settings_link(section: &str) -> (String, &'static str) {
    let label = match section {
        "plugins" => "Settings → Plugins",
        "slskd" => "Settings → Plugins → Soulseek",
        "torznab" => "Settings → Plugins → Torrents & Usenet",
        "streamrip" => "Settings → Plugins → streamrip",
        "ytdlp" => "Settings → Plugins → yt-dlp",
        "pipeline" => "Settings → Downloads → Download pipeline",
        _ => "Settings",
    };
    (format!("#/settings?s={section}"), label)
}

/// The plugin a picker source belongs to (the registry: `deezer` → streamrip, `ytmusic` → ytdlp).
fn plugin_of(id: &str) -> Option<&'static encore_plugins::Manifest> {
    encore_plugins::catalog::for_source(id)
}

/// A builtin source whose plugin is not installed or is switched off: the state that says so, with the
/// Plugins page to fix it on.
fn plugin_state(st: &AppState, id: &str) -> Option<SourceState> {
    let m = plugin_of(id)?;
    let name = &m.plugin.name;
    if !st.plugins.installed(m.id()) {
        return Some(state("not_installed", format!("Install the {name} plugin under Settings → Plugins"), "plugins"));
    }
    if !st.plugins.enabled(m.id()) {
        return Some(state("off", format!("The {name} plugin is switched off"), "plugins"));
    }
    None
}

/// One streamrip service (Qobuz, Tidal, Deezer), its plugin installed and on ([`source_state`]): streamrip on and
/// its program there, the service in the pipeline, its account signed in.
fn streamrip_state(st: &AppState, id: &str) -> SourceState {
    let name = source_name(id);
    if !st.cfg.bool("streamrip_enabled") {
        return state("off", format!("{name} downloads need streamrip switched on"), "streamrip");
    }
    if streamrip::binary(st).is_none() {
        return state("not_installed", streamrip::NOT_INSTALLED_MSG, "plugins");
    }
    if !streamrip::services(st).iter().any(|s| s == id) {
        return state("off", format!("{name} is not in the download pipeline"), "pipeline");
    }
    match id {
        "deezer" if !streamrip::deezer_arl_set(st) => state("sign_in", "Deezer needs an ARL cookie", "streamrip"),
        "tidal" if !streamrip::tidal_signed_in(st) => state("sign_in", "Sign in to Tidal", "streamrip"),
        "qobuz" if streamrip::qobuz_login(st).is_none() => state("sign_in", "Sign in to Qobuz (user id and token)", "streamrip"),
        _ => state("ready", "", "streamrip"),
    }
}

/// A source's state right now (see `SourceState`): first its plugin (installed and switched on), then its own
/// settings and service.
pub fn source_state(st: &AppState, id: &str) -> SourceState {
    if let Some(s) = plugin_state(st, id) {
        return s;
    }
    match id {
        "slskd" => {
            if !st.cfg.bool("slskd_enabled") {
                return state("off", "Soulseek is off", "slskd");
            }
            if !encore_requests::slskd_managed::configured(st) {
                return state("sign_in", "Soulseek needs a username and password", "slskd");
            }
            if encore_requests::slskd_managed::installed_version(st).is_none() {
                return state("not_installed", "Soulseek's program is not installed yet – Update the Soulseek (slskd) plugin under Settings → Plugins", "plugins");
            }
            if st.requests.slskd.status()["state"].as_str() == Some("running") {
                // the engine runs; the session may not (Encore's reconnect schedule says when it is back)
                let c = st.requests.slskd.session().snapshot();
                use encore_requests::slskd_session::State;
                if c.checked.is_some() && matches!(c.state, State::Waiting | State::Stopped | State::Connecting) {
                    return state("not_connected", format!("Soulseek isn't connected – {}", encore_requests::slskd_session::when_text(&c)), "slskd");
                }
                return state("ready", "", "slskd");
            }
            state("not_connected", slskd::not_ready(st).unwrap_or_else(|| "Soulseek is starting – try again in a moment".into()), "slskd")
        }
        "torznab" => {
            if !st.cfg.bool("torznab_enabled") {
                return state("off", "Torrents & Usenet are off", "torznab");
            }
            if st.cfg.str("prowlarr_url").is_empty() || st.cfg.str("prowlarr_api_key").is_empty() {
                return state("sign_in", "Prowlarr needs its address and API key", "torznab");
            }
            if !(torznab::qbit_ready(st) || torznab::sab_ready(st)) {
                return state("not_connected", "No torrent or Usenet client is set up", "torznab");
            }
            state("ready", "", "torznab")
        }
        "qobuz" | "tidal" | "deezer" => streamrip_state(st, id),
        "ytmusic" => if ytdlp::enabled(st) { state("ready", "", "ytdlp") } else { state("off", "YouTube Music is off (yt-dlp)", "ytdlp") },
        other => match encore_requests::sources::plugin::find(st, other) {
            Some(p) if p.enabled => state("ready", "", "plugins"),
            Some(p) => state("off", format!("The {} plugin is switched off", p.name), "plugins"),
            None => state("off", "Unknown source", "pipeline"),
        },
    }
}

/// Every source the picker knows, with its state: nothing is hidden, so a signed-out or stopped source shows
/// why and where to fix it – the builtin ones in their tab order, then the sources of the plugin programs
/// installed (uploaded or from a URL). `{id, name, state, hint, settings, settings_label}`.
pub fn available(st: &AppState) -> Vec<Value> {
    let row = |id: &str, name: &str| {
        let s = source_state(st, id);
        let (settings, label) = settings_link(s.section);
        json!({ "id": id, "name": name, "state": s.state, "hint": s.hint, "settings": settings, "settings_label": label })
    };
    let mut out: Vec<Value> = SOURCES.iter().map(|(id, name, _)| row(id, name)).collect();
    for p in st.plugins.plugin_sources() {
        if p.kinds.iter().any(|k| k == "track" || k == "album") && !out.iter().any(|x| x["id"] == p.picker) {
            out.push(row(&p.picker, &p.name));
        }
    }
    out
}

/// A search on a source that is not ready: 409 with the state and the settings link, so the client shows the
/// hint for that one tab instead of a failed search.
fn state_error(id: &str, s: &SourceState) -> ApiError {
    let (settings, label) = settings_link(s.section);
    err(409, s.hint.clone()).with(json!({ "source": id, "state": s.state, "settings": settings, "settings_label": label }))
}

/// What someone who isn't an admin sees of a source that can't search: no link to a Server settings page
/// (only admins can open them, and the page would show them nothing to fix) – "ask an admin" instead. All
/// the apps leave the link out when `settings` is empty.
/// `x`: the source's `{settings, settings_label}` (blanked); → the hint.
fn without_settings_link(hint: &str, x: &mut Value) -> String {
    x["settings"] = json!("");
    x["settings_label"] = json!("");
    if hint.is_empty() { String::new() } else { format!("{hint} – ask an admin") }
}

/// On the blocking pool: whether streamrip is installed asks its program for its version (a Python start) the
/// first time after a start, or after the program changed. Members get no settings links ([`without_settings_link`]).
async fn sources(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let admin = encore_config::perms::is_admin(&u);
    st.blocking(move |st| {
        let mut list = available(st);
        if !admin {
            for s in list.iter_mut() {
                let hint = s["hint"].as_str().unwrap_or("").to_string();
                s["hint"] = json!(without_settings_link(&hint, s));
            }
        }
        Ok(Json(json!({ "sources": list, "allowed": auth::can(st, &u, "pick") })))
    }).await
}

fn quality(fmt: &str, bitrate: i64, depth: i64, rate: i64) -> String {
    let f = fmt.to_lowercase();
    if LOSSLESS.contains(&f.as_str()) {
        let bits = if depth > 0 { format!(" {depth}-bit") } else { String::new() };
        let khz = if rate > 0 && rate != 44100 { format!("/{}kHz", rate as f64 / 1000.0) } else { String::new() };
        return format!("{}{bits}{khz}", f.to_uppercase());
    }
    if bitrate > 0 { format!("{} {bitrate}k", f.to_uppercase()) } else { f.to_uppercase() }
}

fn score(want_t: &str, want_a: &str, t: &str, a: &str, len: Option<f64>, got: Option<f64>) -> f64 {
    let v = compare(&describe(want_t, want_a, len, None, None), &describe(t, a, got, None, None), true);
    if v.matched { (v.score * 100.0).round() / 100.0 } else { 0.0 }
}

/// The share of `needle`'s words in `hay`, the better of two readings of both: the plain words and the
/// symbols spelled out (Ke$ha → kesha, P!nk → pink, accents folded, apostrophes dropped:
/// [`torznab::match_words`]); a name written as one word counts (AC/DC as "ACDC"). 0 for a needle
/// without words.
fn contains(hay: &str, needle: &str) -> f64 {
    if encore_core::text::ascii_words(needle).is_empty() && torznab::match_words(needle).is_empty() {
        return 0.0;
    }
    torznab::share(hay, needle)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SearchBody {
    pub source: String,
    pub kind: String,
    pub artist: String,
    pub title: String,
    pub query: Option<String>,
    /// For a song: the album it is on, when the client knows it.
    pub album: Option<String>,
    pub mbid: Option<String>,
    pub track_count: Option<usize>,
    pub length: Option<f64>,
    /// The request the picker was opened for: its kept searches are looked up, and what this search finds
    /// is kept with it. Ignored when the caller may not see that request.
    pub request_id: Option<i64>,
    /// "Search again": what is kept is passed over and the source searched now (`true` or `1`).
    #[serde(deserialize_with = "flag")]
    pub fresh: bool,
}

/// `true`, `1` or "1" (a form's or a query string's way of saying yes).
fn flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => matches!(s.trim(), "1" | "true" | "yes"),
        _ => false,
    })
}

/// The search box holds other words than the picker was opened with ("artist title"): typed by hand.
fn typed(b: &SearchBody) -> bool {
    b.query.as_deref().map(str::trim).is_some_and(|q| !q.is_empty() && search_cache::words(q) != search_cache::words(&format!("{} {}", b.artist, b.title)))
}

/// The words Soulseek is asked: typed words as typed (symbols that stand for letters spelled out first,
/// Ke$ha → Kesha, the other symbols out), else the request's names as the worker searches them
/// (`slskd::query_text`, without anything in brackets), so the picker finds what the worker found.
fn slskd_text(b: &SearchBody, q: &str) -> String {
    let typed_words = || torznab::fold_symbols(q).chars().map(|c| if c.is_alphanumeric() || c.is_whitespace() || c == '\'' { c } else { ' ' }).collect::<String>()
        .split_whitespace().collect::<Vec<_>>().join(" ");
    if typed(b) {
        return typed_words();
    }
    let t = slskd::query_text(&[&b.artist, &b.title]);
    if t.is_empty() { typed_words() } else { t }
}

/// The request the picker was opened for, as far as its kept searches go.
#[derive(Debug, Clone, Default)]
pub struct ForRequest {
    pub id: i64,
    /// kept searches written for this id before then belong to an earlier request of that id (a restored
    /// database)
    pub created_at: f64,
    pub kind: String,
    pub artist: String,
    pub title: String,
    pub album: String,
}

impl ForRequest {
    /// From a request row (`get_owned`).
    pub fn of(r: &Value) -> ForRequest {
        let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
        ForRequest { id: r["id"].as_i64().unwrap_or(0), created_at: r["created_at"].as_f64().unwrap_or(0.0), kind: s("kind"), artist: s("artist"), title: s("title"), album: s("album") }
    }

    /// The keys of the request's own attempts' searches (`kind`: slskd or torznab).
    fn keys(&self, kind: &str) -> Vec<String> {
        search_cache::worker_texts(&self.kind, &self.artist, &self.title, &self.album).iter()
            .map(|t| if kind == "slskd" { search_cache::slskd_key(t) } else { search_cache::torznab_key(t, &["3000"]) }).collect()
    }
}

/// One source's answer: its rows, and where they came from.
#[derive(Debug, Default)]
pub struct Answer {
    pub items: Vec<Value>,
    /// when the kept search the rows come from ended (unix seconds); None for a live search
    pub cached_at: Option<f64>,
    /// the words the rows were searched with (a kept search may have other words than the box)
    pub query: String,
    /// "Search again" would run now (false while Soulseek isn't connected)
    pub can_search: bool,
    /// why it would not: the source's hint ("Soulseek isn't connected – next attempt at 14:20")
    pub notice: Option<String>,
}

impl Answer {
    fn live(items: Vec<Value>, query: String) -> Answer {
        Answer { items, cached_at: None, query, can_search: true, notice: None }
    }

    /// `{items, from: cache|live, cached_at, found: "Found at 14:02", query, can_search, notice}`.
    pub fn json(&self) -> Value {
        json!({ "items": self.items, "from": if self.cached_at.is_some() { "cache" } else { "live" },
            "cached_at": self.cached_at.map(|t| t.round() as i64),
            "found": self.cached_at.map(|t| search_cache::found_words(t, encore_core::time::now())),
            "query": self.query, "can_search": self.can_search, "notice": self.notice })
    }
}

/// What is kept for the picker's words (`key`), else – when that found nothing or nothing is kept – the
/// newest answer with results of the request it was opened for (`mine`: id, created_at, the keys of its
/// own attempts' searches), e.g. the worker's album search that fell back to the album's name alone.
/// On the blocking pool.
fn kept_for(st: &AppState, kind: &str, key: &str, mine: Option<(i64, f64, Vec<String>)>) -> Option<search_cache::Found> {
    let (cache, dir, now) = (&st.requests.search_cache, st.data_dir(), encore_core::time::now());
    let own = cache.get(dir, key, now);
    if own.as_ref().is_some_and(|f| !f.results.is_empty()) {
        return own;
    }
    mine.and_then(|(id, since, keys)| cache.get_for_request(dir, kind, Some(id), &keys, since, now)).or(own)
}

/// Picker rows from slskd's answers (live, or kept): per folder for an album, per file for a song, scored
/// against what the picker was opened for, the best 150.
fn slskd_rows(b: &SearchBody, responses: &[Value]) -> Vec<Value> {
    let mut out = vec![];
    for resp in responses {
        let user = resp["username"].as_str().unwrap_or("").to_string();
        let (free, queue, speed) = (resp["hasFreeUploadSlot"].as_bool().unwrap_or(false), resp["queueLength"].as_f64().unwrap_or(0.0), resp["uploadSpeed"].as_f64().unwrap_or(0.0));
        let audio: Vec<Value> = resp["files"].as_array().cloned().unwrap_or_default().into_iter().filter(|f| encore_core::audio::DOWNLOAD_EXTS.contains(&slskd::ext(f["filename"].as_str().unwrap_or("")).as_str())).collect();
        let mut groups: Vec<(String, Vec<Value>)> = vec![];
        for f in audio {
            let name = f["filename"].as_str().unwrap_or("").replace('\\', "/");
            let key = if b.kind == "album" { name.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default() } else { name };
            match groups.iter_mut().find(|(k, _)| *k == key) { Some((_, v)) => v.push(f), None => groups.push((key, vec![f])) }
        }
        for (d, mut files) in groups {
            let exts: std::collections::HashSet<String> = files.iter().map(|f| slskd::ext(f["filename"].as_str().unwrap_or(""))).collect();
            let fmt = if exts.len() == 1 { exts.into_iter().next().unwrap() } else { "mixed".into() };
            let parts: Vec<&str> = d.split('/').collect();
            let name = if b.kind == "track" { parts.last().unwrap_or(&"").to_string() } else { parts[parts.len().saturating_sub(2)..].join("/") };
            let rel = contains(&name, &b.title);
            if rel < 0.4 {
                continue;
            }
            let br = files.iter().map(|f| f["bitRate"].as_i64().unwrap_or(0)).min().unwrap_or(0);
            let depth = files.iter().map(|f| f["bitDepth"].as_i64().unwrap_or(0)).min().unwrap_or(0);
            let rate = files.iter().map(|f| f["sampleRate"].as_i64().unwrap_or(0)).max().unwrap_or(0);
            let lossless = LOSSLESS.contains(&fmt.as_str());
            let mut sc = rel * 40.0 + contains(&d, &b.artist) * 10.0 + if lossless { 30.0 } else if br >= 320 { 15.0 } else { 5.0 };
            sc += if depth >= 24 { 6.0 } else { 0.0 } + if free { 8.0 } else { 0.0 } - queue.min(50.0) / 5.0;
            if let (true, Some(tc)) = (b.kind == "album", b.track_count) {
                sc += if files.len() == tc { 15.0 } else if files.len() < tc { -10.0 } else { 0.0 };
            }
            if let (true, Some(l), Some(fl)) = (b.kind == "track", b.length, files[0]["length"].as_f64()) {
                sc += if (fl - l).abs() < 5.0 { 10.0 } else { -10.0 };
            }
            files.sort_by_key(|f| f["filename"].as_str().unwrap_or("").to_string());
            let info = [user.clone(), if free { "free slot".into() } else { format!("queue {queue}") }, if speed > 0.0 { format!("{:.1} MB/s", speed / 1e6) } else { String::new() }]
                .into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            let dir = if b.kind == "track" { d.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default() } else { d.clone() };
            out.push(json!({ "source": "slskd", "key": format!("{user}|{d}"), "title": name, "subtitle": d.replace('/', " / "),
                "quality": quality(&fmt, br, depth, rate), "lossless": lossless, "tracks": files.len(),
                "size": files.iter().map(|f| f["size"].as_i64().unwrap_or(0)).sum::<i64>(),
                "duration": if b.kind == "track" { files[0]["length"].clone() } else { Value::Null }, "info": info, "score": (sc * 10.0).round() / 10.0,
                "payload": { "username": user, "directory": dir, "format": fmt, "bitrate": br,
                    "files": files.iter().map(|f| json!({ "filename": f["filename"], "size": f["size"] })).collect::<Vec<_>>() } }));
        }
    }
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(150);
    out
}

/// Soulseek for the picker. Unless it searches again (`fresh`), a kept search answers at once: the one for
/// these words, else (opened for a request, nothing typed) the newest with results the request's own
/// attempts or pickers made – also while Soulseek isn't connected (the rows can still be picked: the
/// download waits for Soulseek), then without "Search again". Otherwise it searches through the gate (one
/// search at a time, 3 s apart, never while disconnected) as a task of its own: a picker closed meanwhile
/// doesn't stop it, it is kept, and a second picker asking the same words waits for the same search.
async fn slskd_answer(st: &AppState, b: &SearchBody, q: &str, s: &SourceState, req: Option<&ForRequest>) -> ApiResult<Answer> {
    let ready = s.state == "ready";
    if !ready && (s.state != "not_connected" || b.fresh) {
        return Err(state_error("slskd", s));
    }
    let text = slskd_text(b, q);
    if text.is_empty() {
        return Err(err(400, "Type something to search for"));
    }
    let key = search_cache::slskd_key(&text);
    if !b.fresh {
        let mine = req.filter(|_| !typed(b)).map(|r| (r.id, r.created_at, r.keys("slskd")));
        let k = key.clone();
        let kept = st.blocking(move |st| Ok(kept_for(st, "slskd", &k, mine))).await?;
        if let Some(f) = kept {
            // the words after "Found at 14:02 for …": the box's own when the kept search is the one they make
            // (brackets and symbols aside), the other search's when it came from the request's own attempts
            let query = if f.key == key { q.to_string() } else { f.query };
            return Ok(Answer { items: slskd_rows(b, &f.results), cached_at: Some(f.searched_at), query, can_search: ready, notice: (!ready).then(|| s.hint.clone()) });
        }
    }
    if !ready {
        return Err(state_error("slskd", s));
    }
    let rid = req.map(|r| r.id);
    let (st2, text2, key2) = (st.clone(), text.clone(), key.clone());
    let found = st.requests.search_cache.single(&key, async move {
        match slskd::search_kept(&st2, &text2, slskd::Who::Person, rid, |_| {}).await {
            Ok(results) => Ok(search_cache::Found { key: key2, query: text2, searched_at: encore_core::time::now(), request_id: rid, results }),
            Err(e) => Err(search_cache::Failure::of(&e)),
        }
    }).await.map_err(|f| search_error(st, "slskd", f.into_error()))?;
    Ok(Answer::live(slskd_rows(b, &found.results), found.query.clone()))
}

/// One indexer result as a picker row. `album` marks a release of the album the song is on: only the
/// song's file downloads from it. Prowlarr's API key in the release's link is sealed
/// ([`torznab::seal_key`]): the worker puts it back when the pick is downloaded.
fn torznab_row(st: &AppState, r: &Value, sc: f64, album: Option<(&str, &str, Option<u32>)>) -> Value {
    let proto = r["protocol"].as_str().unwrap_or("torrent").to_string();
    let url = torznab::seal_key(st, r["magnetUrl"].as_str().filter(|m| !m.is_empty()).or(r["downloadUrl"].as_str()).unwrap_or(""));
    let title = r["title"].as_str().unwrap_or("").to_string();
    let lossless = regex::Regex::new(r"(?i)\b(flac|lossless|alac|24[\s-]?bit|hi-?res)\b").unwrap().is_match(&title);
    let seeds = r["seeders"].as_i64();
    let age = r["age"].as_i64().map(|d| format!("{d} days old")).unwrap_or_default();
    let indexer = r["indexer"].as_str().unwrap_or("").to_string();
    let mut info = vec![proto.clone(), seeds.map(|s| format!("{s} seeders")).unwrap_or_default(), age];
    let mut payload = json!({ "title": title, "indexer": r["indexer"], "protocol": proto, "size": r["size"], "seeders": r["seeders"], "url": url, "guid": r["guid"], "lossless": lossless });
    let subtitle = match album {
        Some((name, song, no)) => {
            info.insert(0, indexer);
            payload["album"] = json!(name);
            payload["song_only"] = json!(true);
            if let Some(n) = no {
                payload["track_no"] = json!(n);
            }
            format!("from the album {name} · only {song} downloads")
        }
        None => indexer,
    };
    let info = info.into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    json!({ "source": "torznab", "key": format!("torznab:{}", r["guid"].as_str().unwrap_or(&title)), "title": title,
        "subtitle": subtitle, "quality": if lossless { "FLAC" } else { "" }, "lossless": lossless, "tracks": null, "size": r["size"], "info": info,
        "score": (sc * 10.0).round() / 10.0, "payload": payload })
}

fn torznab_usable(st: &AppState, r: &Value) -> bool {
    let proto = r["protocol"].as_str().unwrap_or("torrent");
    let url = r["magnetUrl"].as_str().filter(|m| !m.is_empty()).or(r["downloadUrl"].as_str()).unwrap_or("");
    !url.is_empty() && !(proto == "torrent" && !torznab::qbit_ready(st)) && !(proto == "usenet" && !torznab::sab_ready(st))
}

fn by_score(out: &mut [Value]) {
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
}

/// A release title that says it is lossless (FLAC, ALAC, 24-bit, hi-res).
fn lossless_re() -> regex::Regex {
    regex::Regex::new(r"(?i)\b(flac|lossless|alac|24[\s-]?bit|hi-?res)\b").unwrap()
}

/// The picker's Prowlarr searches: answered from what is kept unless it searches again (`fresh`), else
/// searched and kept (Prowlarr asks every indexer, which can take tens of seconds, and many indexers allow
/// only so many searches a day). Notes the oldest kept answer used, for the answer's "Found at" line.
struct Prowlarr<'a> {
    st: &'a AppState,
    fresh: bool,
    /// kept with what this search finds
    request_id: Option<i64>,
    /// opened for a request with nothing typed: (id, created_at, the keys of the request's own album
    /// searches), looked up when these words have nothing kept
    mine: Option<(i64, f64, Vec<String>)>,
    /// (when, words) of the oldest kept answer used; the words only when it is another search than the one
    /// asked for (the request's own album search under other words)
    kept: std::sync::Mutex<Option<(f64, Option<String>)>>,
}

impl<'a> Prowlarr<'a> {
    fn of(st: &'a AppState, b: &SearchBody, req: Option<&ForRequest>) -> Prowlarr<'a> {
        let mine = req.filter(|_| !typed(b)).map(|r| (r.id, r.created_at, r.keys("torznab")));
        Prowlarr { st, fresh: b.fresh, request_id: req.map(|r| r.id), mine, kept: Default::default() }
    }

    /// Releases for `q` in `cats`; `own`: the album search the request's own attempts make too.
    async fn search(&self, q: &str, cats: &[&str], own: bool) -> anyhow::Result<Vec<Value>> {
        if !self.fresh {
            let key = search_cache::torznab_key(q, cats);
            let mine = self.mine.clone().filter(|_| own);
            let k = key.clone();
            let found = self.st.blocking(move |st| Ok(kept_for(st, "torznab", &k, mine))).await.ok().flatten();
            if let Some(f) = found {
                let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
                if kept.as_ref().is_none_or(|(at, _)| f.searched_at < *at) {
                    *kept = Some((f.searched_at, (f.key != key).then_some(f.query)));
                }
                return Ok(f.results);
            }
        }
        let res = torznab::search_in(self.st, q, cats).await?;
        search_cache::keep_releases(self.st, q, cats, self.request_id, &res).await;
        Ok(res)
    }
}

/// The indexers' rows for `b` (kept answers as the body says), without a request: the tests' way in.
#[cfg(test)]
async fn search_torznab(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    search_torznab_in(&Prowlarr::of(st, b, None), b, q).await
}

async fn search_torznab_in(p: &Prowlarr<'_>, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    let st = p.st;
    if b.kind == "track" {
        return search_torznab_song(p, b, q).await;
    }
    let mut out = vec![];
    let lossless_re = lossless_re();
    for r in p.search(q, &["3000"], true).await? {
        if !torznab_usable(st, &r) {
            continue;
        }
        let title = r["title"].as_str().unwrap_or("");
        let lossless = lossless_re.is_match(title);
        // the album's title without its edition counts: releases are named "Rodeo", not "Rodeo (Expanded Edition)"
        let sc = torznab::album_share(title, &b.title) * 40.0 + contains(title, &b.artist) * 15.0 + if lossless { 30.0 } else { 0.0 } + (r["seeders"].as_i64().unwrap_or(0) as f64 + 1.0).log2().min(10.0);
        out.push(torznab_row(st, &r, sc, None));
    }
    by_score(&mut out);
    Ok(out)
}

/// The album a song is on, and its track number when known: what the client sent, else MusicBrainz
/// (the song's first studio album or EP), else Deezer – looked up once a day per song (kept in memory).
async fn song_album(st: &AppState, b: &SearchBody) -> Option<(String, Option<u32>)> {
    if let Some(a) = b.album.as_deref().map(str::trim).filter(|a| !a.is_empty() && !a.eq_ignore_ascii_case(&b.title)) {
        return Some((a.to_string(), None));
    }
    let key = format!("picker:song_album:{}|{}|{}", b.artist.to_lowercase(), b.title.to_lowercase(), b.mbid.as_deref().unwrap_or(""));
    if let Some(v) = st.cache_get(&key) {
        return serde_json::from_value(v).ok();
    }
    let found = song_album_lookup(st, b).await;
    if let Some(f) = &found {
        st.cache_put(&key, json!(f), 86_400);
    }
    found
}

/// [`song_album`] from MusicBrainz, else Deezer.
async fn song_album_lookup(st: &AppState, b: &SearchBody) -> Option<(String, Option<u32>)> {
    let t = std::time::Duration::from_secs(4);
    if let Some(id) = b.mbid.as_deref().filter(|m| !m.is_empty()) {
        if let Ok(Ok(Some(g))) = tokio::time::timeout(t, encore_http::mb::album_for_recording(st, id, true)).await {
            if let Some(name) = g["title"].as_str().filter(|n| !n.is_empty()) {
                return Some((name.to_string(), None));
            }
        }
    }
    if b.artist.is_empty() || b.title.is_empty() {
        return None;
    }
    let q = format!("artist:\"{}\" track:\"{}\"", b.artist.replace('"', ""), b.title.replace('"', ""));
    let d: Value = deezer::search(st, "track", &q, 10).timeout(t).send().await.ok()?.json().await.ok()?;
    let hit = d["data"].as_array()?.iter().find(|x| {
        score(&b.title, &b.artist, x["title"].as_str().unwrap_or(""), x["artist"]["name"].as_str().unwrap_or(""), b.length, x["duration"].as_f64()) > 0.0
            && x["album"]["title"].as_str().is_some_and(|a| !a.eq_ignore_ascii_case(&b.title))
    })?.clone();
    let album = hit["album"]["title"].as_str()?.to_string();
    let no = match deezer::track(st, &hit["id"]).timeout(t).send().await {
        Ok(r) => r.json::<Value>().await.ok().and_then(|v| v["track_position"].as_u64()).map(|n| n as u32),
        Err(_) => None,
    };
    Some((album, no))
}

/// A song: single-track releases of it (searched in the audio, MP3 and lossless categories), plus
/// releases of the album it is on, from which only the song's file downloads.
async fn search_torznab_song(p: &Prowlarr<'_>, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    let st = p.st;
    let custom = typed(b);
    let (songs, album) = tokio::join!(p.search(q, &torznab::SONG_CATS, false), async {
        let (name, no) = song_album(st, b).await?;
        // the album's names as the worker searches them: "Travis Scott Rodeo", not "… Rodeo (Expanded Edition)"
        let res = p.search(&slskd::query_text(&[&b.artist, &name]), &["3000"], false).await;
        Some((name, no, res))
    });
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    let song_err = match songs {
        Ok(list) => {
            for r in list {
                if !torznab_usable(st, &r) {
                    continue;
                }
                let title = r["title"].as_str().unwrap_or("");
                let scored = torznab::score_song(title, r["size"].as_f64().unwrap_or(0.0), r["seeders"].as_i64(), r["protocol"].as_str().unwrap_or("torrent"), &b.artist, &b.title);
                // a typed query is shown as found, the song's own search only what matches the song
                let sc = match scored {
                    Some((sc, _)) => sc,
                    None if custom => contains(title, q) * 40.0,
                    None => continue,
                };
                seen.insert(r["guid"].as_str().unwrap_or(title).to_string());
                out.push(torznab_row(st, &r, sc, None));
            }
            None
        }
        Err(e) => Some(e),
    };
    if let Some((name, no, Ok(list))) = album {
        let lossless_re = lossless_re();
        for r in list {
            let title = r["title"].as_str().unwrap_or("");
            if !torznab_usable(st, &r) || !seen.insert(r["guid"].as_str().unwrap_or(title).to_string()) {
                continue;
            }
            let (a, ar) = (torznab::album_share(title, &name), contains(title, &b.artist));
            if a < 0.8 || ar < 0.5 {
                continue;
            }
            let lossless = lossless_re.is_match(title);
            // below the song's own releases of the same quality: the whole album has to arrive first
            let sc = a * 30.0 + ar * 15.0 + if lossless { 30.0 } else { 0.0 } + (r["seeders"].as_i64().unwrap_or(0) as f64 + 1.0).log2().min(10.0) - 10.0;
            out.push(torznab_row(st, &r, sc, Some((&name, &b.title, no))));
        }
    }
    if out.is_empty() {
        if let Some(e) = song_err {
            return Err(e);
        }
    }
    by_score(&mut out);
    Ok(out)
}

async fn search_deezer(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    let kind = if b.kind == "album" { "album" } else { "track" };
    let d: Value = deezer::search(st, kind, q, 25).send().await?.json().await?;
    let mut out: Vec<Value> = d["data"].as_array().cloned().unwrap_or_default().iter().map(|x| {
        let a = x["artist"]["name"].as_str().unwrap_or("");
        let explicit = if x["explicit_lyrics"].as_bool().unwrap_or(false) { "explicit" } else { "" };
        if kind == "album" {
            let rt = x["record_type"].as_str().unwrap_or("");
            let rt = rt.chars().next().map(|c| c.to_uppercase().collect::<String>() + &rt[1..]).unwrap_or_default();
            let info = [rt, explicit.to_string()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            json!({ "source": "deezer", "key": format!("deezer:album:{}", x["id"]), "title": x["title"], "subtitle": a, "quality": "FLAC (Deezer HiFi)",
                "lossless": true, "tracks": x["nb_tracks"], "size": null, "info": info,
                "cover": x["cover_medium"], "score": score(&b.title, &b.artist, x["title"].as_str().unwrap_or(""), a, None, None),
                "payload": { "url": format!("https://www.deezer.com/album/{}", x["id"]) } })
        } else {
            let mut t = x["title"].as_str().unwrap_or("").to_string();
            if let Some(v) = x["title_version"].as_str().filter(|v| !v.is_empty() && !t.to_lowercase().contains(&v.to_lowercase())) {
                t = format!("{t} {v}");
            }
            json!({ "source": "deezer", "key": format!("deezer:track:{}", x["id"]), "title": t, "subtitle": format!("{a} · {}", x["album"]["title"].as_str().unwrap_or("")),
                "quality": "FLAC (Deezer HiFi)", "lossless": true, "tracks": 1, "size": null, "duration": x["duration"], "info": explicit, "cover": x["album"]["cover_medium"],
                "score": score(&b.title, &b.artist, &t, a, b.length, x["duration"].as_f64()), "payload": { "url": format!("https://www.deezer.com/track/{}", x["id"]) } })
        }
    }).collect();
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

async fn search_ytmusic(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    let album = b.kind == "album";
    Ok(ytmusic::search(st, q, !album).await?.into_iter().take(20).filter_map(|x| {
        let (t, a) = (x["title"].as_str().unwrap_or("").to_string(), x["artist"].as_str().unwrap_or("").to_string());
        if album {
            let bid = x["browseId"].as_str().filter(|b| b.starts_with("MPRE"))?;
            Some(json!({ "source": "ytmusic", "key": format!("ytm:{bid}"), "title": t, "subtitle": a, "quality": "Opus ~160k / AAC 256k", "lossless": false,
                "tracks": null, "size": null, "info": "", "cover": x["cover"], "score": score(&b.title, &b.artist, &t, &a, None, None), "payload": { "browseId": bid } }))
        } else {
            let vid = x["videoId"].as_str()?;
            let sub = format!("{a} · {}", x["album"].as_str().unwrap_or("")).trim_end_matches(" · ").to_string();
            Some(json!({ "source": "ytmusic", "key": format!("ytm:{vid}"), "title": t, "subtitle": sub, "quality": "Opus ~160k / AAC 256k", "lossless": false,
                "tracks": 1, "size": null, "duration": x["duration"], "info": "", "cover": x["cover"],
                "score": score(&b.title, &b.artist, &t, &a, b.length, x["duration"].as_f64()), "payload": { "url": format!("https://music.youtube.com/watch?v={vid}") } }))
        }
    }).collect())
}

// ---- Tidal (openapi v2) --------------------------------------------------------------------------

/// The token the search runs with, and the country: the developer app's client-credentials token when a
/// Tidal Client ID is set (Settings → Integrations), else the account streamrip signed in with.
async fn tidal_credentials(st: &AppState) -> anyhow::Result<(String, String)> {
    let client = !st.cfg.str("tidal_client_id").is_empty() && !st.cfg.str("tidal_client_secret").is_empty();
    // the account's token (refreshed when lapsed); with a Client ID a lapsed account only costs its country
    let user = match streamrip::tidal_user_token(st).await {
        Ok(u) => u,
        Err(e) if client => {
            st.db.log("warning", &format!("picker: Tidal account token: {e}"));
            None
        }
        Err(e) => return Err(e),
    };
    let cc = user.as_ref().map(|u| u.1.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "US".into());
    if client {
        return Ok((streamrip::tidal_token(st).await?, cc));
    }
    match user {
        Some((tok, _)) => Ok((tok, cc)),
        None => anyhow::bail!("Sign in to Tidal"),
    }
}

/// A path of Tidal's API with the search's token; a rejected token means signing in again.
async fn tidal_get(st: &AppState, tok: &str, path: &str, query: &[(&str, &str)]) -> anyhow::Result<Value> {
    tidal_read(tidal::get(st, tok, path).query(query)).await
}

/// Send a request to Tidal's API: its answer, or Tidal's words; a rejected token means signing in again.
async fn tidal_read(req: reqwest::RequestBuilder) -> anyhow::Result<Value> {
    let r = req.send().await?;
    let code = r.status().as_u16();
    if code == 401 || code == 403 {
        anyhow::bail!("Tidal rejected the sign-in token – sign in again in Settings → Plugins → streamrip");
    }
    tidal::read(r).await
}

/// Tidal's `mediaTags` as a quality label: (label, lossless). Tidal's search gives the tier, not the exact
/// rate: hi-res is 24-bit at up to 192 kHz.
fn tidal_quality(tags: Option<&Vec<Value>>) -> (&'static str, bool) {
    let Some(tags) = tags else { return ("FLAC", true) };
    let has = |t: &str| tags.iter().any(|x| x.as_str() == Some(t));
    if has("HIRES_LOSSLESS") {
        ("FLAC 24-bit", true)
    } else if has("MQA") {
        ("MQA", true)
    } else if has("LOSSLESS") {
        ("FLAC 16/44.1", true)
    } else {
        ("AAC", false)
    }
}

/// Picker rows from a `/albums` or `/tracks` listing (`included` carries the artists and albums), in the
/// order the search gave the ids.
fn tidal_rows(b: &SearchBody, d: &Value, order: &[String]) -> Vec<Value> {
    let album = b.kind == "album";
    let inc: HashMap<(String, String), Value> = d["included"].as_array().cloned().unwrap_or_default().into_iter()
        .map(|x| ((x["type"].as_str().unwrap_or("").to_string(), x["id"].as_str().unwrap_or("").to_string()), x["attributes"].clone())).collect();
    let items = d["data"].as_array().cloned().unwrap_or_default();
    let mut out = vec![];
    for id in order {
        let Some(x) = items.iter().find(|x| x["id"].as_str() == Some(id)) else { continue };
        let a = &x["attributes"];
        let rel = |name: &str, typ: &str| -> Vec<Value> {
            x["relationships"][name]["data"].as_array().cloned().unwrap_or_default().iter()
                .filter_map(|r| inc.get(&(typ.to_string(), r["id"].as_str().unwrap_or("").to_string())).cloned()).collect()
        };
        let artists = rel("artists", "artists").iter().filter_map(|v| v["name"].as_str().map(String::from)).collect::<Vec<_>>().join(", ");
        let mut title = a["title"].as_str().unwrap_or("").to_string();
        if let Some(v) = a["version"].as_str().filter(|v| !v.is_empty() && !title.to_lowercase().contains(&v.to_lowercase())) {
            title = format!("{title} ({v})");
        }
        let (quality, lossless) = tidal_quality(a["mediaTags"].as_array());
        let dur = a["duration"].as_str().and_then(encore_core::text::iso_duration);
        let explicit = if a["explicit"].as_bool().unwrap_or(false) { "explicit" } else { "" };
        let cover = |links: &Value| -> Value {
            let l = links.as_array().cloned().unwrap_or_default();
            l.iter().find(|i| i["meta"]["width"].as_u64() == Some(320)).or(l.first()).and_then(|i| i["href"].as_str()).map(|h| json!(h)).unwrap_or(Value::Null)
        };
        if album {
            let year = a["releaseDate"].as_str().map(|d| d.chars().take(4).collect::<String>()).unwrap_or_default();
            let kind = a["type"].as_str().map(|t| { let t = t.to_lowercase(); t.chars().next().map(|c| c.to_uppercase().collect::<String>() + &t[1..]).unwrap_or_default() }).unwrap_or_default();
            let info = [kind, year, explicit.to_string()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            out.push(json!({ "source": "tidal", "key": format!("tidal:album:{id}"), "title": title, "subtitle": artists, "quality": quality, "lossless": lossless,
                "tracks": a["numberOfItems"], "size": null, "duration": dur, "info": info, "cover": cover(&a["imageLinks"]),
                "score": score(&b.title, &b.artist, &title, &artists, None, None), "payload": { "url": format!("https://tidal.com/browse/album/{id}") } }));
        } else {
            let alb = rel("albums", "albums");
            let album_title = alb.first().and_then(|v| v["title"].as_str()).unwrap_or("");
            let sub = format!("{artists} · {album_title}").trim_end_matches(" · ").to_string();
            out.push(json!({ "source": "tidal", "key": format!("tidal:track:{id}"), "title": title, "subtitle": sub, "quality": quality, "lossless": lossless,
                "tracks": 1, "size": null, "duration": dur, "info": explicit, "cover": alb.first().map(|v| cover(&v["imageLinks"])).unwrap_or(Value::Null), "isrc": a["isrc"],
                "score": score(&b.title, &b.artist, &title, &artists, b.length, dur), "payload": { "url": format!("https://tidal.com/browse/track/{id}") } }));
        }
    }
    out
}

/// Tidal's catalogue through openapi v2: the search gives ids, the listing their details with artists (and
/// albums). Picks hand the worker a tidal.com link, which streamrip downloads with the signed-in account.
async fn search_tidal(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    // Tidal refuses an empty query (400): nothing to search for, nothing asked
    let Some(q) = tidal::search_query(q) else { return Ok(vec![]) };
    let (tok, cc) = tidal_credentials(st).await?;
    let typ = if b.kind == "album" { "albums" } else { "tracks" };
    // the query in `filter[query]`: the older path form (/searchResults/{q}) answers 400 INVALID_RESOURCE_ID
    let found = tidal_read(tidal::search(st, &tok, &q, &cc, typ)).await?;
    let ids: Vec<String> = tidal::search_ids(&found, typ).into_iter().take(25).collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let include = if typ == "albums" { "artists" } else { "artists,albums" };
    let listing = tidal_get(st, &tok, &format!("/{typ}"), &[("countryCode", cc.as_str()), ("filter[id]", ids.join(",").as_str()), ("include", include)]).await?;
    let mut out = tidal_rows(b, &listing, &ids);
    by_score(&mut out);
    Ok(out)
}

// ---- Qobuz ----------------------------------------------------------------------------------------

/// "FLAC 24/192", "FLAC 16/44.1": Qobuz says the exact bit depth and sample rate (kHz) per release.
fn qobuz_quality(depth: i64, rate: f64) -> String {
    if depth <= 0 || rate <= 0.0 {
        return "FLAC".into();
    }
    let rate = if rate.fract() == 0.0 { format!("{}", rate as i64) } else { format!("{rate:.1}") };
    format!("FLAC {depth}/{rate}")
}

/// A Qobuz id as text (albums have string ids, tracks numbers).
fn idstr(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), other => other.to_string().trim_matches('"').to_string() }
}

/// Picker rows from an `album/search` or `track/search` answer.
fn qobuz_rows(b: &SearchBody, d: &Value) -> Vec<Value> {
    let album = b.kind == "album";
    let items = d[if album { "albums" } else { "tracks" }]["items"].as_array().cloned().unwrap_or_default();
    items.iter().filter(|x| !x["id"].is_null()).map(|x| {
        let id = idstr(&x["id"]);
        let mut title = x["title"].as_str().unwrap_or("").to_string();
        if let Some(v) = x["version"].as_str().filter(|v| !v.is_empty() && !title.to_lowercase().contains(&v.to_lowercase())) {
            title = format!("{title} ({v})");
        }
        let quality = qobuz_quality(x["maximum_bit_depth"].as_i64().unwrap_or(0), x["maximum_sampling_rate"].as_f64().unwrap_or(0.0));
        let explicit = if x["parental_warning"].as_bool().unwrap_or(false) { "explicit" } else { "" };
        let warning = if x["streamable"].as_bool() == Some(false) { "Not streamable on Qobuz with this account" } else { "" };
        let dur = x["duration"].as_f64();
        if album {
            let artist = x["artist"]["name"].as_str().unwrap_or("");
            let year = x["release_date_original"].as_str().or(x["released_at"].as_str()).map(|d| d.chars().take(4).collect::<String>()).unwrap_or_default();
            let info = [year, explicit.to_string()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            json!({ "source": "qobuz", "key": format!("qobuz:album:{id}"), "title": title, "subtitle": artist, "quality": quality, "lossless": true,
                "tracks": x["tracks_count"], "size": null, "duration": dur, "info": info, "warning": warning, "cover": x["image"]["small"],
                "score": score(&b.title, &b.artist, &title, artist, None, None), "payload": { "url": format!("https://open.qobuz.com/album/{id}") } })
        } else {
            let artist = x["performer"]["name"].as_str().or(x["album"]["artist"]["name"].as_str()).unwrap_or("");
            let sub = format!("{artist} · {}", x["album"]["title"].as_str().unwrap_or("")).trim_end_matches(" · ").to_string();
            json!({ "source": "qobuz", "key": format!("qobuz:track:{id}"), "title": title, "subtitle": sub, "quality": quality, "lossless": true,
                "tracks": 1, "size": null, "duration": dur, "info": explicit, "warning": warning, "cover": x["album"]["image"]["small"], "isrc": x["isrc"],
                "score": score(&b.title, &b.artist, &title, artist, b.length, dur), "payload": { "url": format!("https://open.qobuz.com/track/{id}") } })
        }
    }).collect()
}

/// Qobuz's catalogue with the app id and login streamrip uses. Picks hand the worker an open.qobuz.com link.
async fn search_qobuz(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    let kind = if b.kind == "album" { "album" } else { "track" };
    let d = streamrip::qobuz_search(st, kind, q).await?;
    let mut out = qobuz_rows(b, &d);
    by_score(&mut out);
    Ok(out)
}

// ---- the search route -----------------------------------------------------------------------------

/// One source's search. A source that is not ready answers 409 with its state and settings link (see
/// `state_error`), as does Soulseek when its engine runs without a server session, so the client shows the
/// hint under that tab alone – unless Soulseek has a kept search to show (`slskd_answer`).
pub async fn run_search(st: &AppState, b: &SearchBody) -> ApiResult<Answer> {
    run_search_for(st, b, None).await
}

/// [`run_search`] for the picker opened for request `req` (the caller may see it).
pub async fn run_search_for(st: &AppState, b: &SearchBody, req: Option<&ForRequest>) -> ApiResult<Answer> {
    if b.kind != "album" && b.kind != "track" {
        return Err(err(400, "Manual search works for albums and songs"));
    }
    if !SOURCES.iter().any(|s| s.0 == b.source) {
        // a plugin program's source: searched through the registry
        return match encore_requests::sources::plugin::find(st, &b.source) {
            Some(p) => plugin_answer(st, b, &p).await,
            None => Err(err(400, format!("Unknown source {}", b.source))),
        };
    }
    let source = b.source.clone();
    let s = st.blocking(move |st| Ok(source_state(st, &source))).await?;
    let q = b.query.clone().map(|q| q.trim().to_string()).filter(|q| !q.is_empty())
        .unwrap_or_else(|| [b.artist.as_str(), b.title.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" "));
    if b.source == "slskd" {
        return slskd_answer(st, b, &q, &s, req).await;
    }
    if s.state != "ready" {
        return Err(state_error(&b.source, &s));
    }
    if b.source == "torznab" {
        // the names the picker was opened with, as the worker searches them: bracketed parts ("(feat. …)",
        // "(Expanded Edition)") and symbols out – indexers want every word of the search in a release's
        // name, and releases are named without them. Words typed by hand go as typed.
        let boxed = q.clone();
        let q = if typed(b) { q } else { Some(slskd::query_text(&[&b.artist, &b.title])).filter(|t| !t.is_empty()).unwrap_or(q) };
        let p = Prowlarr::of(st, b, req);
        let items = search_torznab_in(&p, b, &q).await.map_err(|e| search_error(st, &b.source, e))?;
        let kept = p.kept.into_inner().unwrap_or_else(|e| e.into_inner());
        // kept: the box's words, unless the rows came from another search (as for Soulseek); live: the words sent
        let query = match &kept { Some((_, Some(other))) => other.clone(), Some(_) => boxed, None => q };
        return Ok(Answer { items, cached_at: kept.map(|k| k.0), query, can_search: true, notice: None });
    }
    // the streaming catalogues answer in a second or two: always live
    let res = match b.source.as_str() {
        "qobuz" => search_qobuz(st, b, &q).await,
        "tidal" => search_tidal(st, b, &q).await,
        "deezer" => search_deezer(st, b, &q).await,
        _ => search_ytmusic(st, b, &q).await,
    };
    res.map(|items| Answer::live(items, q)).map_err(|e| search_error(st, &b.source, e))
}

/// A plugin program's search (uploaded or installed from a URL): the box's words or the names the picker was
/// opened with, its results as rows (always live: the plugin keeps its own pace).
async fn plugin_answer(st: &AppState, b: &SearchBody, p: &encore_requests::sources::plugin::PluginSource) -> ApiResult<Answer> {
    use encore_requests::sources::plugin::{self as plug, Ask};
    let s = source_state(st, &p.picker);
    if s.state != "ready" {
        return Err(state_error(&p.picker, &s));
    }
    if !p.kinds.contains(&b.kind) {
        return Err(err(400, format!("{} does not search for {}s", p.name, if b.kind == "album" { "album" } else { "song" })));
    }
    let query = b.query.clone().map(|q| q.trim().to_string()).filter(|q| !q.is_empty());
    let shown = query.clone().unwrap_or_else(|| [b.artist.as_str(), b.title.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" "));
    let ask = Ask {
        kind: b.kind.clone(), artist: b.artist.clone(), title: b.title.clone(), album: b.album.clone().unwrap_or_default(),
        query: query.filter(|_| typed(b)), length: b.length, track_count: b.track_count, lossless_only: false, isrc: None, limit: 50,
    };
    match plug::search(st, p, &ask, None).await {
        Ok(results) => Ok(Answer::live(plug::picker_rows(p, &results), shown)),
        Err(f) if f.code == encore_plugins::Code::RateLimited => Err(err(429, format!("{}: {}", p.name, f.message))),
        Err(f) => Err(err(502, format!("{} search failed: {}", p.name, f.message.chars().take(160).collect::<String>()))),
    }
}

/// A failed search as the answer: Soulseek not connected is 409 with its state (the client shows the hint
/// under its tab), Soulseek busy 429, anything else 502 with the source's words.
fn search_error(st: &AppState, source: &str, e: anyhow::Error) -> ApiError {
    let msg = e.to_string();
    if source == "slskd" && (e.is::<slskd::NotConnected>() || slskd::not_ready(st).is_some()) {
        return state_error("slskd", &state("not_connected", msg, "slskd"));
    }
    if source == "slskd" && e.is::<slskd::Busy>() {
        return err(429, msg); // another search held the turn for 30 s, or searches are rate limited
    }
    err(502, format!("{} search failed: {}", source_name(source), msg.chars().take(160).collect::<String>()))
}

/// POST /api/picker/search: one source's rows, `{items, from, cached_at, found, query, can_search,
/// notice}` (see [`Answer::json`]).
async fn search(State(st): State<AppState>, User(u): User, Json(b): Json<SearchBody>) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "pick") {
        return Err(err(403, "Ask an admin for permission to pick downloads"));
    }
    // the request the picker was opened for, when this person may see it (else as if none was sent)
    let req = b.request_id.and_then(|id| get_owned(&st, id, &u).ok()).map(|r| ForRequest::of(&r));
    let admin = encore_config::perms::is_admin(&u);
    run_search_for(&st, &b, req.as_ref()).await.map(|a| Json(a.json())).map_err(|mut e| {
        // a source that can't search (409 with its settings link): members get "ask an admin" instead
        if let Some(x) = e.extra.as_mut().filter(|x| !admin && x["settings"].is_string()) {
            e.detail = without_settings_link(&e.detail, x);
        }
        e
    })
}

#[derive(Deserialize)]
struct PickBody {
    source: String,
    payload: Value,
    title: Option<String>,
    request_id: Option<i64>,
    request: Option<NewRequest>,
    /// "Don't check the audio": the pick is imported without the audio check (and its songs confirmed by
    /// the caller); off, a pick the check turns away is kept for Keep anyway
    #[serde(default)]
    no_check: bool,
}

/// Create a request locked to the chosen result. An existing request's row is reused (stopped first when
/// it is running), so the song keeps one row with its attempt count.
async fn pick(State(st): State<AppState>, User(u): User, Json(mut b): Json<PickBody>) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "pick") {
        return Err(err(403, "Ask an admin for permission to pick downloads"));
    }
    // "Don't check the audio" is Keep anyway's and Download anyway's power: admins, and members with Fix
    // their own downloads
    if b.no_check && !auth::can(&st, &u, "fix") {
        return Err(err(403, "Ask an admin to pick without the audio check"));
    }
    if b.source == "link" {
        let url = b.payload["url"].as_str().unwrap_or("").trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(err(400, "Paste a full http(s) link"));
        }
        b.payload = json!({ "url": url });
    }
    // YouTube Music albums → their playlist link
    if b.source == "ytmusic" {
        if let Some(bid) = b.payload["browseId"].as_str().map(String::from) {
            let (title, pl, _) = ytmusic::album(&st, &bid).await.map_err(|e| err(502, e.to_string()))?;
            let pl = pl.ok_or_else(|| err(502, "YouTube Music has no playable playlist for this album"))?;
            b.payload = json!({ "url": format!("https://music.youtube.com/playlist?list={pl}"), "title": title });
        }
    }
    // a running request's place in the queue stays taken from its stop until the pick is written: a step
    // started in between would act on the download stopped here
    let mut hold: Option<worker::Hold> = None;
    let req = if let Some(rid) = b.request_id {
        let old = get_owned(&st, rid, &u)?;
        let s = |k: &str| old[k].as_str().map(String::from);
        let req = NewRequest { kind: s("kind").unwrap_or_default(), mbid: s("mbid"), artist: s("artist"), title: s("title"), album: s("album"), year: s("year"),
            cover_url: s("cover_url"), length: old["detail"]["length"].as_f64(), isrc: old["detail"]["isrc"].as_str().map(String::from), ..Default::default() };
        let status = old["status"].as_str().unwrap_or("");
        // a pick starts the request over, and its downloaded files would go with it: they are imported (or
        // the request cancelled) first
        if matches!(status, "failed" | "waiting" | "importing" | "completed") && worker::storage::has_files(&old["detail"]) {
            return Err(err(409, "This request has downloaded files waiting to be imported – Import again or cancel it first"));
        }
        if worker::ACTIVE.contains(&status) {
            hold = Some(worker::cancel_holding(&st, rid).await);
        }
        req
    } else if let Some(r) = b.request.clone() {
        quota(&st, &u, 1)?;
        r
    } else {
        return Err(err(400, "Nothing to download"));
    };
    let label = b.title.clone().or(b.payload["title"].as_str().map(String::from)).unwrap_or_default();
    let skip = b.no_check.then(|| json!({ "by": u.s("username"), "why": "pick", "at": encore_db::now() }));
    let (id, status) = create_pick_checked(&st, &u, &req, &b.source, b.payload, &label, b.request_id, skip)?;
    drop(hold);
    Ok(Json(json!({ "id": id, "status": status })))
}

/// The request behind a manual pick: the worker downloads exactly `payload` from `source`, then tags
/// and imports it like any other request. Returns (id, status).
pub fn create_pick(st: &AppState, u: &encore_db::Row, req: &NewRequest, source: &str, payload: Value, label: &str) -> ApiResult<(i64, String)> {
    create_pick_for(st, u, req, source, payload, label, None)
}

/// `create_pick` for an existing request (`reuse`): its row is reset and locked to the pick.
pub fn create_pick_for(st: &AppState, u: &encore_db::Row, req: &NewRequest, source: &str, payload: Value, label: &str, reuse: Option<i64>) -> ApiResult<(i64, String)> {
    create_pick_checked(st, u, req, source, payload, label, reuse, None)
}

/// `create_pick_for` with the audio check off for the pick when `skip_check` ({by, why: "pick", at}) is
/// given: the picker's "Don't check the audio".
#[allow(clippy::too_many_arguments)]
fn create_pick_checked(st: &AppState, u: &encore_db::Row, req: &NewRequest, source: &str, payload: Value, label: &str, reuse: Option<i64>,
                       skip_check: Option<Value>) -> ApiResult<(i64, String)> {
    if req.kind != "album" && req.kind != "track" {
        return Err(err(400, "Manual picks work for albums and songs"));
    }
    let unchecked = skip_check.is_some();
    let mut extra = json!({ "pick": { "source": source, "payload": payload, "title": label } });
    if let Some(s) = skip_check {
        extra["skip_check"] = s;
    }
    let id = encore_requests::create::create_or_reuse(st, u, req, None, Some(extra), reuse)?;
    let status = st.db.one("SELECT status FROM requests WHERE id=?", p![id])?.map(|r| r.s("status")).unwrap_or_default();
    if status == "queued" {
        let msg: String = if label.is_empty() { "Manual pick".into() } else { format!("Manual pick: {label}").chars().take(200).collect() };
        st.db.execute("UPDATE requests SET message=? WHERE id=?", p![msg, id])?;
    }
    st.db.log("info", &format!("{} manually picked {source} result for {}: {label}{}", u.s("username"), req.title.clone().or(req.album.clone()).unwrap_or_default(),
        if unchecked { " (without the audio check)" } else { "" }));
    Ok((id, status))
}

#[cfg(test)]
#[path = "picker_tests.rs"]
mod tests;
