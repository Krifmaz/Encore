//! Translated and romanized titles, per listener.
//!
//! Each listener picks (Settings → Language, account prefs `translate_mode` / `translate_lang`):
//! off · show the translation under the original · show it instead · romanization only, and a target
//! language (English unless chosen). JSON answers to that listener's GETs then carry `title_t`,
//! `album_t`, `artist_t`, `albumartist_t` (and `name_t` on artist objects) with the translation and
//! the same names ending in `_r` with the romanization, wherever one is known; clients choose what to
//! show from the mode.
//!
//! Where translations come from, strongest first: a manual correction (⋯ → Edit translation, shared by
//! everyone reading the same language), official ones (MusicBrainz pseudo-releases and aliases, Apple's
//! English names for Japanese releases; see official.rs), then the local model (llama.cpp with a Qwen3
//! GGUF, the "Translator" download tool; see llama.rs). The background worker goes through the library
//! in small batches at low priority; titles a listener has on screen are queued first.
pub mod llama;
pub mod official;
pub mod prompt;
pub mod script;
pub mod store;

use crate::auth::{self, User};
use crate::db::{Db, Row, RowExt};
use crate::error::{err, ApiResult};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use prompt::{Item, Kind};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

// ---- listener settings --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Under,
    Instead,
    Romanized,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "under" => Mode::Under,
            "instead" => Mode::Instead,
            "romanized" => Mode::Romanized,
            _ => Mode::Off,
        }
    }
}

/// The modes Settings → Language offers (id, label).
pub const MODES: [(&str, &str); 4] = [
    ("off", "Off"),
    ("under", "Show translation under the original"),
    ("instead", "Show translation instead"),
    ("romanized", "Romanization only"),
];

/// A user's (mode, target language) from their account prefs.
pub fn prefs_of(u: &Row) -> (Mode, &'static str) {
    let p: Value = serde_json::from_str(&u.s("prefs")).unwrap_or(json!({}));
    (Mode::parse(p["translate_mode"].as_str().unwrap_or("off")), script::target(p["translate_lang"].as_str().unwrap_or(script::DEFAULT_TARGET)))
}

/// The target languages of everyone who has translations on.
fn active_targets(db: &Db) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = db.query("SELECT prefs FROM users WHERE disabled=0 AND prefs LIKE '%translate_mode%'", crate::p![]).unwrap_or_default()
        .iter().map(prefs_of).filter(|(m, _)| *m != Mode::Off).map(|(_, t)| t).collect();
    out.sort();
    out.dedup();
    out
}

// ---- stamping JSON answers ----------------------------------------------------------------------

/// The fields that get `<field>_t` / `<field>_r`.
const FIELDS: [(&str, Kind); 4] = [("title", Kind::Title), ("album", Kind::Album), ("artist", Kind::Artist), ("albumartist", Kind::Artist)];

/// Artist objects (library artists, search results) name themselves in `name`.
fn is_artist(o: &serde_json::Map<String, Value>) -> bool {
    !o.contains_key("username") && !o.contains_key("title") && ["cover_album", "albums", "aliases", "disambiguation"].iter().any(|k| o.contains_key(*k))
}

fn texts_of(o: &serde_json::Map<String, Value>) -> impl Iterator<Item = (&'static str, Kind, &str)> {
    let artist_name = is_artist(o);
    FIELDS.iter().map(|(f, k)| (*f, *k)).chain(artist_name.then_some(("name", Kind::Artist)))
        .filter_map(move |(f, k)| o.get(f).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|s| (f, k, s)))
}

fn collect(v: &Value, out: &mut Vec<(String, Kind, Option<String>)>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Value::Object(o) => {
            let artist = o.get("artist").or_else(|| o.get("albumartist")).and_then(|a| a.as_str()).map(String::from);
            for (_, k, s) in texts_of(o) {
                out.push((s.to_string(), k, if k == Kind::Artist { None } else { artist.clone() }));
            }
            o.values().filter(|x| x.is_array() || x.is_object()).for_each(|x| collect(x, out));
        }
        _ => {}
    }
}

fn apply(v: &mut Value, found: &HashMap<String, store::Shown>) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| apply(x, found)),
        Value::Object(o) => {
            let adds: Vec<(String, Value)> = texts_of(o).filter_map(|(f, _, s)| found.get(&script::normalize(s)).map(|sh| (f, sh))).flat_map(|(f, sh)| {
                let mut a = vec![];
                if let Some(t) = &sh.translation { a.push((format!("{f}_t"), json!(t))); }
                if let Some(r) = &sh.romanization { a.push((format!("{f}_r"), json!(r))); }
                a
            }).collect();
            for (k, val) in adds {
                o.entry(k).or_insert(val);
            }
            o.values_mut().filter(|x| x.is_array() || x.is_object()).for_each(|x| apply(x, found));
        }
        _ => {}
    }
}

/// Add the known translations for `target` to every title, album and artist in `v`; returns the texts
/// worth translating that have no row yet (to queue).
pub fn stamp(db: &Db, target: &str, v: &mut Value) -> Vec<(String, Kind, Option<String>)> {
    let mut all = vec![];
    collect(v, &mut all);
    if all.is_empty() {
        return vec![];
    }
    let texts: Vec<String> = all.iter().map(|(t, _, _)| t.clone()).collect();
    let found = store::lookup(db, target, &texts);
    if !found.is_empty() {
        apply(v, &found);
    }
    let mut seen = HashSet::new();
    let wanted: Vec<_> = all.into_iter().filter(|(t, _, _)| script::candidate(t, target) && seen.insert(script::normalize(t))).collect();
    if wanted.is_empty() {
        return vec![];
    }
    let keys: Vec<String> = wanted.iter().map(|(t, _, _)| script::normalize(t)).collect();
    let known = store::known(db, target, &keys);
    wanted.into_iter().filter(|(t, _, _)| !known.contains(&script::normalize(t))).collect()
}

/// Middleware: JSON answers to a signed-in listener's GETs gain the translations they asked for.
pub async fn stamp_layer(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let wanted = req.method() == Method::GET && path.starts_with("/api/") && !path.starts_with("/api/remote/events") && !path.starts_with("/api/translate/");
    if !wanted {
        return next.run(req).await;
    }
    let headers = req.headers().clone();
    let resp = next.run(req).await;
    let json = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("application/json"));
    if !json || resp.status() != StatusCode::OK {
        return resp;
    }
    let st2 = st.clone();
    let user = tokio::task::spawn_blocking(move || auth::current_user(&st2, &headers)).await.ok().flatten();
    let Some((_, target)) = user.as_ref().map(prefs_of).filter(|(m, _)| *m != Mode::Off) else { return resp };
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 32 << 20).await else {
        return Response::from_parts(parts, Body::empty());
    };
    let Ok(mut v) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let st2 = st.clone();
    let out = tokio::task::spawn_blocking(move || {
        let missing = stamp(&st2.db, target, &mut v);
        enqueue(target, missing);
        serde_json::to_vec(&v).ok()
    }).await.ok().flatten();
    match out {
        Some(b) => {
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from(b))
        }
        None => Response::from_parts(parts, Body::from(bytes)),
    }
}

// ---- the work queue -----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct Pending {
    text: String,
    kind: Kind,
    artist: Option<String>,
    target: &'static str,
}

const QUEUE_MAX: usize = 2000;
static QUEUE: LazyLock<Mutex<(VecDeque<Pending>, HashSet<(String, &'static str)>)>> = LazyLock::new(Default::default);
static WAKE: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

/// Queue titles a listener has on screen (they go before the library pass).
fn enqueue(target: &'static str, items: Vec<(String, Kind, Option<String>)>) {
    if items.is_empty() {
        return;
    }
    let mut q = QUEUE.lock().unwrap();
    let mut added = false;
    for (text, kind, artist) in items {
        let text = script::normalize(&text);
        if q.0.len() >= QUEUE_MAX || !q.1.insert((text.clone(), target)) {
            continue;
        }
        q.0.push_back(Pending { text, kind, artist, target });
        added = true;
    }
    drop(q);
    if added {
        WAKE.notify_one();
    }
}

fn take_queued(n: usize) -> Vec<Pending> {
    let mut q = QUEUE.lock().unwrap();
    let n = n.min(q.0.len());
    let items: Vec<Pending> = q.0.drain(..n).collect();
    for p in &items {
        q.1.remove(&(p.text.clone(), p.target));
    }
    items
}

/// Start a pass now (a listener turned translations on, the Translator was installed).
pub fn wake() {
    WAKE.notify_one();
}

/// Items per background batch, and the pause between batches (the work stays in the background).
const BATCH: usize = 40;
const PAUSE: Duration = Duration::from_secs(2);

/// Library rows that mention any of these texts (title, album, artist or album artist).
fn rows_for(db: &Db, texts: &[String]) -> Vec<Row> {
    let mut out = vec![];
    for chunk in texts.chunks(200) {
        let m = crate::artists::marks(chunk.len());
        let mut params = vec![];
        for _ in 0..4 {
            params.extend(chunk.iter().map(|t| json!(t)));
        }
        let sql = format!("SELECT title, artist, album, albumartist, mb_albumid, mb_artistid, mb_albumartistid FROM tracks
                           WHERE title IN ({m}) OR album IN ({m}) OR artist IN ({m}) OR albumartist IN ({m}) LIMIT 5000");
        out.extend(db.query(&sql, params).unwrap_or_default());
    }
    out
}

fn is_mbid(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Translate these texts for `target`: official sources (by the library's MusicBrainz ids) first,
/// then the model for what is still missing.
async fn process(st: &AppState, target: &'static str, items: Vec<Pending>, model_ok: &mut bool) {
    if items.is_empty() {
        return;
    }
    let want: HashMap<String, &Pending> = items.iter().map(|p| (p.text.clone(), p)).collect();
    let texts: Vec<String> = want.keys().cloned().collect();
    let db = st.db.clone();
    let rows = {
        let t = texts.clone();
        tokio::task::spawn_blocking(move || rows_for(&db, &t)).await.unwrap_or_default()
    };
    // official: one release lookup covers the album title and every song on it
    let mut albums: Vec<(String, String, String)> = vec![];
    let mut artists: Vec<(String, String)> = vec![];
    for r in &rows {
        let (title, album) = (script::normalize(&r.s("title")), script::normalize(&r.s("album")));
        let rid = r.s("mb_albumid");
        if (want.contains_key(&title) || want.contains_key(&album)) && !albums.iter().any(|(i, _, a)| *i == rid && *a == album) {
            albums.push((rid, r.s("albumartist"), r.s("album")));
        }
        for (name, id) in [(r.s("artist"), r.s("mb_artistid")), (r.s("albumartist"), r.s("mb_albumartistid"))] {
            if want.contains_key(&script::normalize(&name)) && is_mbid(&id) && !artists.iter().any(|(i, _)| *i == id) {
                artists.push((id, name));
            }
        }
    }
    let mut found: Vec<official::Found> = vec![];
    for (rid, artist, album) in albums.iter().take(BATCH) {
        if is_mbid(rid) {
            match official::release(st, rid, target).await {
                Ok(f) => found.extend(f),
                Err(e) => tracing::debug!("translate: MusicBrainz release {rid}: {e}"),
            }
        }
        match official::apple(st, artist, album, target).await {
            Ok(f) => found.extend(f),
            Err(e) => tracing::debug!("translate: Apple {artist} – {album}: {e}"),
        }
    }
    for (id, name) in artists.iter().take(BATCH) {
        match official::artist(st, id, name, target).await {
            Ok((t, r)) if t.is_some() || r.is_some() => found.push((Kind::Artist, name.clone(), t, r)),
            Ok(_) => {}
            Err(e) => tracing::debug!("translate: MusicBrainz artist {id}: {e}"),
        }
    }
    {
        let db = st.db.clone();
        let items2 = items.clone();
        let _ = tokio::task::spawn_blocking(move || {
            for (kind, text, t, r) in &found {
                let _ = store::put(&db, text, target, kind.as_str(), t.as_deref(), r.as_deref(), store::Source::Official);
            }
            for p in &items2 {
                let _ = store::mark(&db, &p.text, target, p.kind.as_str(), store::Source::Official);
            }
        }).await;
    }
    model_pass(st, target, items, model_ok).await;
}

/// Ask the model for the items whose row still misses a translation or a romanization.
async fn model_pass(st: &AppState, target: &'static str, items: Vec<Pending>, model_ok: &mut bool) {
    if !*model_ok || items.is_empty() || !llama::ready(st) {
        return;
    }
    let db = st.db.clone();
    let texts: Vec<String> = items.iter().map(|p| p.text.clone()).collect();
    let have = tokio::task::spawn_blocking(move || store::lookup(&db, target, &texts)).await.unwrap_or_default();
    let todo: Vec<&Pending> = items.iter().filter(|p| {
        let romanize = !matches!(script::detect(&p.text), "latn" | "");
        match have.get(&p.text) {
            Some(s) => s.translation.is_none() || (romanize && s.romanization.is_none()),
            None => true,
        }
    }).collect();
    let thinking = llama::model(st).thinking;
    for chunk in todo.chunks(prompt::BATCH) {
        let batch: Vec<Item> = chunk.iter().enumerate().map(|(i, p)| Item { id: i + 1, kind: p.kind, text: p.text.clone(), artist: p.artist.clone() }).collect();
        let reply = match llama::chat(st, prompt::messages(&batch, target, thinking), thinking).await {
            Ok(r) => r,
            Err(e) => {
                st.db.log("warning", &format!("Translator: {e}"));
                *model_ok = false;
                return;
            }
        };
        let got = prompt::parse(&reply, &batch);
        let db = st.db.clone();
        let _ = tokio::task::spawn_blocking(move || {
            for it in &batch {
                let (t, r) = got.get(&it.id).cloned().unwrap_or((None, None));
                let _ = store::put(&db, &it.text, target, it.kind.as_str(), t.as_deref(), r.as_deref(), store::Source::Model);
            }
        }).await;
    }
}

/// Library texts worth translating for `target` that have no row yet (at most `limit`), with their
/// kind and artist.
fn library_backlog(db: &Db, target: &'static str, limit: usize) -> Vec<Pending> {
    let rows = db.query("SELECT DISTINCT title, artist, album, albumartist FROM tracks", crate::p![]).unwrap_or_default();
    let mut seen = HashSet::new();
    let mut cands: Vec<Pending> = vec![];
    for r in &rows {
        let artist = Some(r.s("albumartist")).filter(|a| !a.is_empty()).or_else(|| Some(r.s("artist")).filter(|a| !a.is_empty()));
        for (text, kind, ctx) in [(r.s("albumartist"), Kind::Artist, None), (r.s("artist"), Kind::Artist, None), (r.s("album"), Kind::Album, artist.clone()), (r.s("title"), Kind::Title, Some(r.s("artist")))] {
            let t = script::normalize(&text);
            if !t.is_empty() && script::candidate(&t, target) && seen.insert(t.clone()) {
                cands.push(Pending { text: t, kind, artist: ctx, target });
            }
        }
    }
    let keys: Vec<String> = cands.iter().map(|p| p.text.clone()).collect();
    let known = store::known(db, target, &keys);
    cands.into_iter().filter(|p| !known.contains(&p.text)).take(limit).collect()
}

/// One pass: queued titles first, then the library in batches, then rows that wait for the model.
async fn run_pass(st: &AppState) {
    let db = st.db.clone();
    let targets = tokio::task::spawn_blocking(move || active_targets(&db)).await.unwrap_or_default();
    let mut model_ok = true;
    loop {
        let queued = take_queued(BATCH);
        if !queued.is_empty() {
            let mut by_target: HashMap<&'static str, Vec<Pending>> = HashMap::new();
            for p in queued {
                by_target.entry(p.target).or_default().push(p);
            }
            for (t, items) in by_target {
                process(st, t, items, &mut model_ok).await;
            }
            continue;
        }
        if !st.cfg.bool("translate_background") {
            break;
        }
        let mut did = false;
        for &t in &targets {
            let db = st.db.clone();
            let batch = tokio::task::spawn_blocking(move || library_backlog(&db, t, BATCH)).await.unwrap_or_default();
            if !batch.is_empty() {
                did = true;
                process(st, t, batch, &mut model_ok).await;
                tokio::time::sleep(PAUSE).await;
            }
        }
        if !did {
            break;
        }
    }
    // rows the official pass looked at before the Translator was installed
    if st.cfg.bool("translate_background") && llama::ready(st) {
        for &t in &targets {
            loop {
                if !model_ok {
                    return;
                }
                let db = st.db.clone();
                let rows = tokio::task::spawn_blocking(move || store::waiting_for_model(&db, t, prompt::BATCH * 2)).await.unwrap_or_default();
                if rows.is_empty() {
                    break;
                }
                let items: Vec<Pending> = rows.into_iter().map(|(text, kind)| Pending { text, kind: Kind::parse(&kind), artist: None, target: t }).collect();
                let texts: Vec<String> = items.iter().map(|p| p.text.clone()).collect();
                model_pass(st, t, items, &mut model_ok).await;
                if !model_ok {
                    // the Translator failed: leave these waiting for a later pass
                    return;
                }
                // whatever the answer, these were asked
                let db = st.db.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    for text in texts {
                        let _ = store::mark(&db, &text, t, "title", store::Source::Model);
                    }
                }).await;
                if !QUEUE.lock().unwrap().0.is_empty() {
                    break;
                }
                tokio::time::sleep(PAUSE).await;
            }
        }
    }
}

/// The background worker: a pass when woken (titles on screen, settings changed) and every 30
/// minutes (new library songs).
pub async fn worker(st: AppState) {
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        run_pass(&st).await;
        tokio::select! {
            _ = WAKE.notified() => tokio::time::sleep(Duration::from_secs(2)).await,
            _ = tokio::time::sleep(Duration::from_secs(1800)) => {}
        }
    }
}

// ---- routes -------------------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/translate/languages", get(languages))
        .route("/api/translate/status", get(status))
        .route("/api/translate/entry", get(get_entry).put(put_entry).delete(delete_entry))
        .route("/api/translate/run", post(run_now))
}

/// The choices Settings → Language offers.
async fn languages(User(_): User) -> Json<Value> {
    Json(json!({
        "languages": script::LANGUAGES.iter().map(|(c, n, _)| json!({ "code": c, "name": n })).collect::<Vec<_>>(),
        "modes": MODES.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
        "default": script::DEFAULT_TARGET,
    }))
}

/// Whether the Translator can run, and (for admins) how far the cache is.
async fn status(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let admin = auth::is_admin(&u);
    let st2 = st.clone();
    let (ready, counts) = tokio::task::spawn_blocking(move || (llama::ready(&st2), store::counts(&st2.db))).await.map_err(|e| err(500, e.to_string()))?;
    let m = llama::model(&st);
    let mut out = json!({
        "translator": ready, "model": m.id, "running": llama::running(),
        "can_edit": auth::can(&st, &u, "translate"), "background": st.cfg.bool("translate_background"),
    });
    if admin {
        out["rows"] = json!(counts.0);
        out["shown"] = json!(counts.1);
        out["waiting"] = json!(counts.2);
        out["queued"] = json!(QUEUE.lock().unwrap().0.len());
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct EntryQ {
    text: String,
    #[serde(default)]
    target: String,
}

fn target_for(u: &Row, asked: &str) -> &'static str {
    if asked.trim().is_empty() { prefs_of(u).1 } else { script::target(asked) }
}

/// One text's translation for the edit dialog.
async fn get_entry(State(st): State<AppState>, User(u): User, Query(q): Query<EntryQ>) -> ApiResult<Json<Value>> {
    if q.text.trim().is_empty() {
        return Err(err(400, "Nothing to look up"));
    }
    let target = target_for(&u, &q.target);
    Ok(Json(tokio::task::spawn_blocking(move || store::entry(&st.db, &q.text, target)).await.map_err(|e| err(500, e.to_string()))?))
}

#[derive(Deserialize)]
struct EntryBody {
    text: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    target: String,
    translation: Option<String>,
    romanization: Option<String>,
}

/// A manual correction: it wins over every source and stays (empty text = show nothing).
async fn put_entry(State(st): State<AppState>, User(u): User, Json(b): Json<EntryBody>) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "translate") {
        return Err(err(403, "You can't edit translations"));
    }
    if b.text.trim().is_empty() || b.text.len() > 1000 {
        return Err(err(400, "Nothing to translate"));
    }
    let long = |s: &Option<String>| s.as_ref().is_some_and(|x| x.len() > 500);
    if long(&b.translation) || long(&b.romanization) {
        return Err(err(400, "That is too long for a title"));
    }
    let target = target_for(&u, &b.target);
    let kind = Kind::parse(&b.kind).as_str();
    let who = u.s("username");
    let out = tokio::task::spawn_blocking(move || -> rusqlite::Result<Value> {
        store::set_manual(&st.db, &b.text, target, kind, Some(b.translation.as_deref().unwrap_or("")), Some(b.romanization.as_deref().unwrap_or("")))?;
        st.db.log("info", &format!("{who} corrected the {} translation of “{}”", script::language_name(target), script::normalize(&b.text)));
        Ok(store::entry(&st.db, &b.text, target))
    }).await.map_err(|e| err(500, e.to_string()))??;
    Ok(Json(out))
}

/// Remove a manual correction: what the sources found shows again.
async fn delete_entry(State(st): State<AppState>, User(u): User, Query(q): Query<EntryQ>) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "translate") {
        return Err(err(403, "You can't edit translations"));
    }
    let target = target_for(&u, &q.target);
    let out = tokio::task::spawn_blocking(move || -> rusqlite::Result<Value> {
        store::set_manual(&st.db, &q.text, target, "title", None, None)?;
        Ok(store::entry(&st.db, &q.text, target))
    }).await.map_err(|e| err(500, e.to_string()))??;
    Ok(Json(out))
}

/// Start a pass now (Settings → Language after a change; Download tools after an install).
async fn run_now(User(_): User) -> Json<Value> {
    wake();
    Json(json!({ "ok": true }))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
