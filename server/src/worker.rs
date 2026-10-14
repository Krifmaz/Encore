//! Request pipeline: every enabled source in the admin's order (streamrip, HiFi API, Soulseek,
//! Prowlarr → qBittorrent/SABnzbd, YouTube), each download verified, tagged and imported.
use crate::db::{now, Row, RowExt};
use crate::sources::{slskd, streamrip, torznab, ytdlp, ytmusic};
use crate::state::AppState;
use crate::tagger::{self, Rejected};
use crate::{library, mb, notify, p};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Semaphore;

pub const ACTIVE: [&str; 5] = ["queued", "searching", "downloading", "monitoring", "importing"];
static STREAMRIP: Semaphore = Semaphore::const_new(1);
static YTDLP: Semaphore = Semaphore::const_new(2);
static RIP_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"qobuz\.com|tidal\.com|deezer\.com").unwrap());

/// Running request tasks, so a cancel can stop them.
#[derive(Clone, Default)]
pub struct Tasks(Arc<Mutex<HashMap<i64, tokio::task::AbortHandle>>>);

/// The request was cancelled/removed while we worked on it.
#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}
impl std::error::Error for Cancelled {}

pub struct Req {
    pub id: i64,
    pub row: Row,
    pub d: Value,
}

impl Req {
    fn s(&self, k: &str) -> String {
        self.row.s(k)
    }
    fn opt(&self, k: &str) -> Option<String> {
        Some(self.row.s(k)).filter(|x| !x.is_empty())
    }
    fn album_title(&self) -> String {
        self.opt("album").unwrap_or_else(|| self.s("title"))
    }
    fn length(&self) -> Option<f64> {
        self.d["length"].as_f64().filter(|l| *l > 0.0)
    }
    fn label(&self) -> String {
        match self.s("kind").as_str() {
            "album" => format!("{} – {}", self.s("artist"), self.album_title()),
            "artist" => self.s("artist"),
            "url" => self.opt("title").unwrap_or_else(|| self.s("url")),
            _ => format!("{} – {}", self.s("artist"), self.s("title")),
        }
    }
    fn hint(&self) -> Option<String> {
        self.opt("cover_url")
    }
}

pub fn get(st: &AppState, id: i64) -> Option<Req> {
    let row = st.db.one("SELECT * FROM requests WHERE id=?", p![id]).ok()??;
    let d = serde_json::from_str(&row.s("detail")).unwrap_or(json!({}));
    let d = if d.is_object() { d } else { json!({}) };
    Some(Req { id, row, d })
}

/// Update columns (detail is always written along).
fn upd(st: &AppState, r: &mut Req, fields: &[(&str, Value)]) {
    let mut sets = vec![];
    let mut params = vec![];
    for (k, v) in fields {
        sets.push(format!("{k}=?"));
        params.push(v.clone());
        r.row.insert(k.to_string(), v.clone());
    }
    sets.push("detail=?".into());
    params.push(json!(r.d.to_string()));
    sets.push("updated_at=?".into());
    params.push(json!(now()));
    params.push(json!(r.id));
    let _ = st.db.execute(&format!("UPDATE requests SET {} WHERE id=?", sets.join(", ")), params);
}

fn check(st: &AppState, id: i64) -> anyhow::Result<()> {
    match get(st, id) {
        Some(r) if ACTIVE.contains(&r.s("status").as_str()) => Ok(()),
        _ => Err(Cancelled.into()),
    }
}

fn staging(st: &AppState, r: &Req) -> PathBuf {
    PathBuf::from(st.cfg.str("staging_path")).join(format!("request-{}", r.id))
}

fn rm(p: &Path) {
    let _ = std::fs::remove_dir_all(p);
}

/// What happened when trying a source.
enum Out {
    Done,
    Wait,
    Next(String),
}

// ---- lifecycle ----------------------------------------------------------------------------------

/// Remember where each new file came from (shown on the album page).
fn record_origin(st: &AppState, paths: &[PathBuf], origin: &Value, rid: i64, since: f64) {
    for pth in paths {
        let _ = st.db.execute("UPDATE tracks SET source=?, source_detail=?, source_url=?, request_id=?, downloaded_at=?
                               WHERE path=? AND (downloaded_at IS NULL OR downloaded_at >= ?) AND added_at >= ?",
            p![origin["source"], origin["detail"], origin["url"], rid, now(), pth.to_string_lossy(), since, since - 1.0]);
    }
}

async fn complete(st: &AppState, r: &mut Req, message: &str, new: &[PathBuf], origin: Value) {
    if !new.is_empty() {
        record_origin(st, new, &origin, r.id, r.row.f("created_at"));
    }
    upd(st, r, &[("status", json!("completed")), ("progress", json!(100)), ("message", json!(message))]);
    if !new.is_empty() {
        crate::playlists::rematch(st, None, true); // playlists pick up the new songs
    }
    st.db.log("info", &format!("Request completed: {} ({message})", r.label()));
    notify::send(st, "✅ Now available", &format!("{}\n{message}", r.label()), r.hint().as_deref()).await;
    if !new.is_empty() {
        notify::refresh_media_servers(st).await;
        if st.cfg.bool("lyrics_on_import") {
            let ids: Vec<i64> = new.iter().filter_map(|p| st.db.one("SELECT id FROM tracks WHERE path=?", p![p.to_string_lossy()]).ok().flatten()).map(|r| r.i("id")).collect();
            let st = st.clone();
            tokio::spawn(async move {
                crate::lyrics::fetch_missing(&st, Some(&ids), ids.len()).await;
            });
        }
    }
}

async fn fail(st: &AppState, r: &mut Req, message: &str) {
    if r.d["lossless_only"].as_bool().unwrap_or(false) {
        upd(st, r, &[("status", json!("failed")), ("message", json!("No lossless copy available yet – will try again later"))]);
        return;
    }
    upd(st, r, &[("status", json!("failed")), ("message", json!(message))]);
    st.db.log("error", &format!("Request failed: {} – {message}", r.label()));
    notify::send(st, "❌ Request failed", &format!("{}\n{message}", r.label()), r.hint().as_deref()).await;
}

fn log_push(r: &mut Req, line: String) {
    if !r.d["log"].is_array() {
        r.d["log"] = json!([]);
    }
    r.d["log"].as_array_mut().unwrap().push(json!(line));
}

/// Try the remaining sources in order until one downloads or waits.
async fn next_source(st: &AppState, r: &mut Req, reason: &str) -> anyhow::Result<()> {
    let mut reason = reason.to_string();
    loop {
        if !reason.is_empty() {
            let src = r.opt("source").unwrap_or("start".into());
            log_push(r, format!("{src}: {reason}"));
        }
        let mut srcs: Vec<String> = r.d["sources"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        if srcs.is_empty() {
            let log: Vec<String> = r.d["log"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let tried = if log.is_empty() { "no download sources enabled".to_string() } else { log[log.len().saturating_sub(3)..].join("; ") };
            fail(st, r, &format!("All sources exhausted ({tried})")).await;
            return Ok(());
        }
        let src = srcs.remove(0);
        r.d["sources"] = json!(srcs);
        upd(st, r, &[("source", json!(src)), ("progress", json!(0)), ("status", json!("searching")), ("message", json!(format!("Trying {src}…")))]);
        let res = match src.as_str() {
            "slskd" => start_slskd(st, r, false).await,
            "slskd_lossless" => start_slskd(st, r, true).await,
            "torznab" => start_torznab(st, r, false).await,
            "ytdlp" => start_ytdlp(st, r, None).await,
            "streamrip" => start_streamrip(st, r, None).await,
            other => Ok(Out::Next(format!("unknown source {other}"))),
        };
        match res {
            Ok(Out::Done) | Ok(Out::Wait) => return Ok(()),
            Ok(Out::Next(why)) => reason = why,
            Err(e) if e.is::<Cancelled>() => return Err(e),
            Err(e) => reason = format!("error: {e}"),
        }
    }
}

fn source_order(st: &AppState, r: &Req) -> Vec<String> {
    // (HiFi API was removed – streamrip covers TIDAL)
    let mut order: Vec<String> = st.cfg.str("source_order").split(',').map(|s| s.trim().to_lowercase().replace("lidarr", "torznab"))
        // qobuz / tidal / deezer are streamrip services: the first one marks where streamrip runs
        .map(|s| if streamrip::SERVICES.contains(&s.as_str()) { "streamrip".to_string() } else { s })
        .filter(|s| !s.is_empty() && s != "hifi").collect();
    for (src, before) in [("torznab", "ytdlp")] {
        if !order.iter().any(|s| s == src) {
            // saved orders that predate this source
            let at = order.iter().position(|s| s == before).unwrap_or(order.len());
            order.insert(at, src.into());
        }
    }
    let lossless_only = r.d["lossless_only"].as_bool().unwrap_or(false);
    if st.cfg.bool("quality_first") || lossless_only {
        // every lossless option first (in the admin's order), then lossy fallbacks
        let mut lossless: Vec<String> = order.iter().filter(|s| *s == "streamrip").cloned().collect();
        if order.iter().any(|s| s == "slskd") {
            lossless.push("slskd_lossless".into());
        }
        lossless.extend(order.iter().filter(|s| *s != "streamrip").cloned());
        order = lossless;
    }
    if lossless_only {
        order.retain(|s| ["streamrip", "slskd_lossless"].contains(&s.as_str()));
    } else if st.cfg.bool("ytdlp_last_resort") {
        // downloads should basically never fail: YouTube Music is always the final fallback
        order.retain(|s| s != "ytdlp");
        order.push("ytdlp".into());
    }
    let album = r.s("kind") == "album";
    let avail = |s: &str| match s {
        "torznab" => torznab::enabled(st) && album,
        "slskd" | "slskd_lossless" => slskd::enabled(st),
        "ytdlp" => ytdlp::enabled(st) || st.cfg.bool("ytdlp_last_resort"),
        "streamrip" => streamrip::enabled(st),
        _ => false,
    };
    let mut seen = std::collections::HashSet::new();
    order.into_iter().filter(|s| avail(s) && seen.insert(s.clone())).collect()
}

async fn start(st: &AppState, r: &mut Req) -> anyhow::Result<()> {
    let kind = r.s("kind");
    if kind == "playlist" {
        upd(st, r, &[("status", json!("completed"))]);
        return Ok(());
    }
    if kind == "url" {
        let url = r.s("url");
        if RIP_LINK.is_match(&url) {
            if !streamrip::enabled(st) {
                fail(st, r, "Qobuz/Tidal/Deezer links need streamrip enabled in Settings → Downloaders").await;
                return Ok(());
            }
            upd(st, r, &[("source", json!("streamrip")), ("status", json!("downloading")), ("message", json!("Downloading with streamrip…"))]);
            return run_streamrip_url(st, r).await;
        }
        upd(st, r, &[("source", json!("ytdlp")), ("status", json!("downloading")), ("message", json!("Starting download…"))]);
        return run_url(st, r).await;
    }
    if r.d["pick"].is_object() {
        return start_pick(st, r).await;
    }
    let lossless_only = r.d["lossless_only"].as_bool().unwrap_or(false);
    // already there? (lossless upgrade requests replace what's there)
    if kind == "album" && !lossless_only && (library::has_release_group(&st.db, &r.s("mbid")) || library::has_album(&st.db, &r.s("artist"), &r.album_title())) {
        complete(st, r, "Already in library", &[], Value::Null).await;
        return Ok(());
    }
    if kind == "track" && track_in_library(st, r) {
        complete(st, r, "Already in library", &[], Value::Null).await;
        return Ok(());
    }
    if kind == "track" && try_album_upgrade(st, r).await? {
        return Ok(());
    }
    if kind == "artist" {
        // always one request per release, so each one goes through every source
        return expand_artist(st, r).await;
    }
    if kind == "album" && r.s("mbid").is_empty() && r.d["ext"].is_object() {
        release_info(st, r).await; // store track list: used for matching downloads and tagging
    }
    r.d["sources"] = json!(source_order(st, r));
    r.d["log"] = json!([]);
    next_source(st, r, "").await
}

async fn expand_artist(st: &AppState, r: &mut Req) -> anyhow::Result<()> {
    upd(st, r, &[("status", json!("searching")), ("message", json!("Looking up discography…"))]);
    let art = mb::artist(st, &r.s("mbid")).await?;
    let mut kinds = vec!["Album", "EP"];
    if st.cfg.bool("discography_singles") {
        kinds.push("Single");
    }
    let t = now();
    let mut n = 0;
    for g in art["release_groups"].as_array().cloned().unwrap_or_default() {
        let id = g["id"].as_str().unwrap_or("");
        if !kinds.contains(&g["type"].as_str().unwrap_or("")) || g["secondary"].as_array().is_some_and(|a| !a.is_empty()) {
            continue;
        }
        if library::has_release_group(&st.db, id)
            || st.db.one("SELECT 1 FROM requests WHERE mbid=? AND status NOT IN ('failed','denied','cancelled')", p![id])?.is_some() {
            continue;
        }
        st.db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, detail, parent_id, created_at, updated_at)
                       VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![r.row.get("user_id"), "album", id, art["name"], g["title"], g["title"], g["year"], g["cover"], "queued", "{}", r.id, t, t])?;
        n += 1;
    }
    let msg = if n > 0 { format!("Queued {n} release(s) from the discography") } else { "Nothing new – you already have (or requested) every release".into() };
    complete(st, r, &msg, &[], Value::Null).await;
    Ok(())
}

// ---- song → whole album -------------------------------------------------------------------------

fn track_in_library(st: &AppState, r: &Req) -> bool {
    let mbid = r.opt("mbid").or(r.d["recording"].as_str().map(String::from));
    let want = crate::matcher::describe(&r.s("title"), &r.s("artist"), "", r.length(), r.d["isrc"].as_str(), mbid.as_deref());
    crate::matcher::LibraryIndex::load(&st.db).find(&want).is_some()
}

/// If the song is on an official album, fetch the whole album instead (best quality available).
async fn try_album_upgrade(st: &AppState, r: &mut Req) -> anyhow::Result<bool> {
    let mode = match r.d["album_mode"].as_str() { Some("song") => "off".to_string(), Some("album") => "album_ep".to_string(), _ => st.cfg.str("album_upgrade") };
    if mode == "off" || r.d["no_album"].as_bool().unwrap_or(false) {
        return Ok(false);
    }
    upd(st, r, &[("status", json!("searching")), ("message", json!("Looking up which album this song is on…"))]);
    let (mut rid, mut alb) = (None, None);
    let found: anyhow::Result<()> = async {
        rid = match r.opt("mbid") { Some(m) => Some(m), None => mb::resolve_recording(st, &r.s("artist"), &r.s("title"), r.length(), r.d["isrc"].as_str()).await? };
        if let Some(id) = &rid {
            alb = mb::album_for_recording(st, id, mode == "album_ep").await?;
        }
        Ok(())
    }.await;
    if let Err(e) = found {
        st.db.log("warning", &format!("Album lookup failed for {}: {e}", r.label()));
    }
    r.d["recording"] = json!(rid);
    let Some(alb) = alb.filter(|a| !library::has_release_group(&st.db, a["id"].as_str().unwrap_or(""))) else {
        r.d["no_album"] = json!(true);
        upd(st, r, &[]);
        return Ok(false);
    };
    let typ = alb["type"].as_str().unwrap_or("album").to_lowercase();
    let aid = match st.db.one("SELECT id FROM requests WHERE kind='album' AND mbid=? AND status NOT IN ('failed','denied','cancelled')", p![alb["id"]])? {
        Some(e) => e.i("id"),
        None => {
            let t = now();
            let artist = if alb["artist"].as_str().unwrap_or("").is_empty() { json!(r.s("artist")) } else { alb["artist"].clone() };
            let id = st.db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, message, detail, created_at, updated_at)
                                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                p![r.row.get("user_id"), "album", alb["id"], artist, alb["title"], alb["title"], alb["year"], alb["cover"], "queued",
                   format!("Full album for “{}”", r.s("title")), "{}", t, t])?;
            st.db.log("info", &format!("“{}” is on {typ} “{}” – downloading the whole {typ}", r.s("title"), alb["title"].as_str().unwrap_or("")));
            id
        }
    };
    r.d["album_request"] = json!(aid);
    let cover = r.opt("cover_url").map(|c| json!(c)).unwrap_or(alb["cover"].clone());
    upd(st, r, &[("status", json!("monitoring")), ("source", json!("album")), ("cover_url", cover),
                 ("message", json!(format!("Getting the whole {typ} “{}”", alb["title"].as_str().unwrap_or(""))))]);
    Ok(true)
}

async fn poll_album(st: &AppState, r: &mut Req) -> anyhow::Result<()> {
    let a = get(st, r.d["album_request"].as_i64().unwrap_or(0));
    let reason = match &a {
        Some(a) if a.s("status") == "completed" => {
            if track_in_library(st, r) {
                complete(st, r, &format!("Downloaded with the album “{}”", a.s("title")), &[], Value::Null).await;
                return Ok(());
            }
            "the album didn't contain this exact version"
        }
        None => "the album couldn't be downloaded",
        Some(a) if ["failed", "denied", "cancelled"].contains(&a.s("status").as_str()) => "the album couldn't be downloaded",
        Some(a) => {
            let msg = a.opt("message").unwrap_or(a.s("status"));
            if a.row.f("progress") != r.row.f("progress") || r.d["last_msg"].as_str() != Some(&msg) {
                r.d["last_msg"] = json!(msg);
                upd(st, r, &[("progress", json!(a.row.f("progress"))), ("message", json!(format!("Album “{}”: {msg}", a.s("title"))))]);
            }
            return Ok(());
        }
    };
    r.d["no_album"] = json!(true);
    upd(st, r, &[("status", json!("queued")), ("source", Value::Null), ("message", json!(format!("Getting the song on its own ({reason})")))]);
    Ok(())
}

// ---- release info --------------------------------------------------------------------------------

async fn release_info(st: &AppState, r: &mut Req) -> Option<Value> {
    if r.d.get("release").is_none() && r.s("kind") == "album" {
        if r.s("mbid").is_empty() && r.d["ext"].is_object() {
            // not in MusicBrainz: track list from Deezer/Apple/…
            r.d["release"] = match crate::routes::catalog::tracklist(st, &r.d["ext"]).await {
                Ok(t) if !t.is_empty() => json!({ "id": null, "tracks": t }),
                Ok(_) => Value::Null,
                Err(e) => {
                    st.db.log("warning", &format!("Tracklist lookup failed for {}: {e}", r.label()));
                    Value::Null
                }
            };
        } else if !r.s("mbid").is_empty() {
            let res: anyhow::Result<Value> = async {
                let Some(rid) = mb::best_release_for_group(st, &r.s("mbid"), None).await? else { return Ok(Value::Null) };
                let rel = mb::release(st, &rid).await?;
                let tracks: Vec<Value> = rel["tracks"].as_array().cloned().unwrap_or_default().iter()
                    .map(|t| json!({ "title": t["title"], "artist": t["artist"], "length": t["length"], "recording_id": t["recording_id"], "isrc": t["isrc"] })).collect();
                Ok(json!({ "id": rel["id"], "tracks": tracks }))
            }.await;
            r.d["release"] = res.unwrap_or_else(|e| {
                st.db.log("warning", &format!("MusicBrainz release lookup failed: {e}"));
                Value::Null
            });
        }
    }
    r.d.get("release").filter(|v| !v.is_null()).cloned()
}

fn n_tracks(rel: &Option<Value>) -> Option<usize> {
    rel.as_ref().and_then(|r| r["tracks"].as_array().map(|a| a.len()))
}

/// Track list/year from Deezer or Apple for releases MusicBrainz doesn't have (used to tag the files).
fn source_meta(r: &Req) -> Option<Value> {
    if !r.s("mbid").is_empty() || !r.d["ext"].is_object() {
        return None;
    }
    Some(json!({ "tracks": r.d["release"]["tracks"].as_array().cloned().unwrap_or_default(), "year": r.s("year"), "upc": r.d["ext"]["upc"] }))
}

fn q(parts: &[&str]) -> String {
    static BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[].*?[\)\]]").unwrap());
    static NW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\s']").unwrap());
    let s = parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" ");
    let s = BR.replace_all(&s, " ");
    NW.replace_all(&s, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Import downloaded files for this request; a wrong-audio rejection becomes Err(Rejected).
async fn import(st: &AppState, r: &Req, files: &[PathBuf], root: &Path) -> anyhow::Result<(Vec<PathBuf>, String)> {
    let hint = r.hint();
    if r.s("kind") == "album" {
        let mbid = r.opt("mbid");
        let new = tagger::import_album_files(st, files, &r.s("artist"), &r.album_title(), mbid.as_deref(), root, true, source_meta(r).as_ref(), hint.as_deref()).await?;
        Ok((new, String::new()))
    } else {
        let f = files.first().ok_or_else(|| anyhow::anyhow!("no audio file"))?;
        let mbid = r.opt("mbid");
        let (p, note) = tagger::import_single(st, f, &r.s("artist"), &r.s("title"), mbid.as_deref(), root, r.length(), true, hint.as_deref()).await?;
        Ok((vec![p], note))
    }
}

fn with_note(msg: &str, note: &str) -> String {
    if note.is_empty() { msg.into() } else { format!("{msg} – {note}") }
}

// ---- manual picks -------------------------------------------------------------------------------

/// Download exactly the chosen result – no fallback to other sources.
async fn start_pick(st: &AppState, r: &mut Req) -> anyhow::Result<()> {
    let pick = r.d["pick"].clone();
    r.d["sources"] = json!([]);
    r.d["log"] = json!([]);
    let src = pick["source"].as_str().unwrap_or("");
    let url = pick["payload"]["url"].as_str().unwrap_or("").to_string();
    let out = match src {
        "slskd" => {
            r.d["slskd"] = json!({ "candidates": [pick["payload"]], "idx": -1 });
            upd(st, r, &[("source", json!("slskd"))]);
            slskd_try_next(st, r, "").await?
        }
        "torznab" => {
            r.d["torznab"] = json!({ "candidates": [pick["payload"]], "idx": -1 });
            upd(st, r, &[("source", json!("torznab"))]);
            torznab_try_next(st, r, "").await?
        }
        "deezer" | "link" if src == "deezer" || RIP_LINK.is_match(&url) => {
            if !streamrip::enabled(st) {
                fail(st, r, "Qobuz/Tidal/Deezer links need streamrip enabled in Settings → Downloaders").await;
                return Ok(());
            }
            upd(st, r, &[("source", json!("streamrip"))]);
            start_streamrip(st, r, Some(vec![url])).await?
        }
        "ytmusic" | "link" => {
            upd(st, r, &[("source", json!("ytdlp"))]);
            start_ytdlp(st, r, Some(url)).await?
        }
        other => Out::Next(format!("unknown source {other}")),
    };
    if let Out::Next(why) = out {
        next_source(st, r, &why).await?;
    }
    Ok(())
}

// ---- Soulseek -----------------------------------------------------------------------------------

async fn start_slskd(st: &AppState, r: &mut Req, lossless_only: bool) -> anyhow::Result<Out> {
    upd(st, r, &[("status", json!("searching")), ("message", json!(if lossless_only { "Searching Soulseek for lossless…" } else { "Searching Soulseek…" }))]);
    let mut cands = vec![];
    if r.s("kind") == "album" {
        let rel = release_info(st, r).await;
        let album = r.album_title();
        for query in [q(&[&r.s("artist"), &album]), q(&[&album])] {
            let res = slskd::search(st, &query).await?;
            cands = slskd::rank_album(st, &res, &r.s("artist"), &album, n_tracks(&rel), lossless_only);
            if !cands.is_empty() {
                break;
            }
        }
    } else {
        let res = slskd::search(st, &q(&[&r.s("artist"), &r.s("title")])).await?;
        cands = slskd::rank_track(st, &res, &r.s("artist"), &r.s("title"), r.length(), lossless_only);
    }
    check(st, r.id)?;
    let tried: Vec<String> = r.d["slskd_tried"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    cands.retain(|c| !tried.contains(&format!("{}|{}", c["username"].as_str().unwrap_or(""), c["directory"].as_str().unwrap_or(""))));
    if cands.is_empty() {
        return Ok(Out::Next(if lossless_only { "no lossless results on Soulseek" } else { "no suitable results on Soulseek" }.into()));
    }
    r.d["slskd"] = json!({ "candidates": cands, "idx": -1 });
    slskd_try_next(st, r, "").await
}

async fn slskd_cancel_current(st: &AppState, c: &Value) {
    let wanted: Vec<&str> = c["files"].as_array().map(|a| a.iter().filter_map(|f| f["filename"].as_str()).collect()).unwrap_or_default();
    let user = c["username"].as_str().unwrap_or("");
    for f in slskd::user_downloads(st, user).await {
        if wanted.contains(&f["filename"].as_str().unwrap_or("")) && !f["state"].as_str().unwrap_or("").starts_with("Completed") {
            slskd::cancel(st, user, &f["id"].as_str().map(String::from).unwrap_or(f["id"].to_string())).await;
        }
    }
}

async fn slskd_try_next(st: &AppState, r: &mut Req, reason: &str) -> anyhow::Result<Out> {
    let mut reason = reason.to_string();
    loop {
        let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(-1);
        let cands = r.d["slskd"]["candidates"].as_array().cloned().unwrap_or_default();
        if idx >= 0 {
            if let Some(c) = cands.get(idx as usize) {
                let key = format!("{}|{}", c["username"].as_str().unwrap_or(""), c["directory"].as_str().unwrap_or(""));
                if !r.d["slskd_tried"].is_array() {
                    r.d["slskd_tried"] = json!([]);
                }
                r.d["slskd_tried"].as_array_mut().unwrap().push(json!(key));
                slskd_cancel_current(st, c).await;
            }
        }
        let idx = idx + 1;
        r.d["slskd"]["idx"] = json!(idx);
        let Some(c) = cands.get(idx as usize).cloned() else {
            return Ok(Out::Next(if reason.is_empty() { "all Soulseek candidates failed".into() } else { reason }));
        };
        let user = c["username"].as_str().unwrap_or("").to_string();
        if let Err(e) = slskd::enqueue(st, &user, c["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[])).await {
            reason = e.to_string();
            continue;
        }
        let t = now();
        r.d["slskd"]["started"] = json!(t);
        r.d["slskd"]["last_change"] = json!(t);
        r.d["slskd"]["last_bytes"] = json!(0);
        let br = c["bitrate"].as_i64().unwrap_or(0);
        let msg = format!("Downloading {} file(s) from {user} ({}{})", c["files"].as_array().map(|a| a.len()).unwrap_or(0),
            c["format"].as_str().unwrap_or("").to_uppercase(), if br > 0 { format!(" {br}k") } else { String::new() });
        upd(st, r, &[("status", json!("downloading")), ("progress", json!(0)), ("message", json!(msg))]);
        return Ok(Out::Wait);
    }
}

async fn after_slskd(st: &AppState, r: &mut Req, out: Out) -> anyhow::Result<()> {
    if let Out::Next(why) = out {
        next_source(st, r, &why).await?;
    }
    Ok(())
}

async fn poll_slskd(st: &AppState, r: &mut Req) -> anyhow::Result<()> {
    let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(-1);
    let Some(c) = r.d["slskd"]["candidates"].get(idx.max(0) as usize).cloned() else {
        return next_source(st, r, "lost track of the Soulseek download").await;
    };
    let user = c["username"].as_str().unwrap_or("").to_string();
    let wanted: Vec<String> = c["files"].as_array().map(|a| a.iter().filter_map(|f| f["filename"].as_str().map(String::from)).collect()).unwrap_or_default();
    // one entry per file: re-queued files show up again, the newest counts
    let mut latest: HashMap<String, Value> = HashMap::new();
    for f in slskd::user_downloads(st, &user).await {
        let n = f["filename"].as_str().unwrap_or("").to_string();
        if wanted.contains(&n) {
            latest.insert(n, f);
        }
    }
    let t = now();
    if latest.is_empty() {
        // the transfer list can be cleared from the Soulseek page: finished files on disk still count
        if !wanted.is_empty() && wanted.iter().all(|n| slskd::local_path(st, n).is_some()) {
            upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Importing…"))]);
            return import_slskd(st, r, &c).await;
        }
        if t - r.d["slskd"]["started"].as_f64().unwrap_or(t) > 120.0 {
            let o = slskd_try_next(st, r, "transfers disappeared").await?;
            return after_slskd(st, r, o).await;
        }
        return Ok(());
    }
    let states: Vec<String> = latest.values().map(|f| f["state"].as_str().unwrap_or("").to_string()).collect();
    if states.iter().any(|s| slskd::DONE_BAD.iter().any(|b| s.contains(b))) {
        st.db.log("warning", &format!("Soulseek transfer from {user} failed for {}; trying next source", r.label()));
        let o = slskd_try_next(st, r, "transfer failed").await?;
        return after_slskd(st, r, o).await;
    }
    let total: f64 = latest.values().map(|f| f["size"].as_f64().unwrap_or(0.0)).sum::<f64>().max(1.0);
    let got: f64 = latest.values().map(|f| f["bytesTransferred"].as_f64().unwrap_or(0.0)).sum();
    if got > r.d["slskd"]["last_bytes"].as_f64().unwrap_or(0.0) {
        r.d["slskd"]["last_bytes"] = json!(got);
        r.d["slskd"]["last_change"] = json!(t);
    }
    let done = states.iter().filter(|s| s.contains(slskd::DONE_OK)).count();
    if done == wanted.len() {
        upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Importing…"))]);
        return import_slskd(st, r, &c).await;
    }
    if t - r.d["slskd"]["last_change"].as_f64().unwrap_or(t) > st.cfg.int("slskd_stall_minutes") as f64 * 60.0 {
        let o = slskd_try_next(st, r, "transfer stalled").await?;
        return after_slskd(st, r, o).await;
    }
    let queued = states.iter().all(|s| s.starts_with("Queued"));
    upd(st, r, &[("progress", json!((got / total * 1000.0).round() / 10.0)),
                 ("message", json!(format!("{} {user} – {done}/{} files", if queued { "Queued by" } else { "Downloading from" }, wanted.len())))]);
    Ok(())
}

async fn import_slskd(st: &AppState, r: &mut Req, c: &Value) -> anyhow::Result<()> {
    let names: Vec<String> = c["files"].as_array().map(|a| a.iter().filter_map(|f| f["filename"].as_str().map(String::from)).collect()).unwrap_or_default();
    let mut paths = vec![];
    for _ in 0..6 {
        let found: Vec<Option<PathBuf>> = names.iter().map(|n| slskd::local_path(st, n)).collect();
        let all = found.iter().all(|p| p.is_some());
        paths = found.into_iter().flatten().collect();
        if all {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    let root = PathBuf::from(st.cfg.str("slskd_download_path"));
    if paths.is_empty() {
        fail(st, r, &format!("Downloaded files not found under {} – check the shared volume mapping", root.display())).await;
        return Ok(());
    }
    let user = c["username"].as_str().unwrap_or("");
    match import(st, r, &paths, &root).await {
        Ok((new, note)) => {
            let origin = json!({ "source": "Soulseek", "detail": format!("user {user} · {}", c["directory"].as_str().unwrap_or("").replace('\\', "/")) });
            complete(st, r, &with_note(&format!("Downloaded from Soulseek ({})", c["format"].as_str().unwrap_or("").to_uppercase()), &note), &new, origin).await;
            Ok(())
        }
        Err(e) if e.is::<Rejected>() => {
            for p in &paths {
                let _ = std::fs::remove_file(p);
                library::prune_empty(p.parent().unwrap(), &root);
            }
            st.db.log("warning", &format!("Rejected download from {user} for {}: {e}", r.label()));
            upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("Rejected {user}'s files: {e}")))]);
            let o = slskd_try_next(st, r, &format!("wrong audio ({e})")).await?;
            after_slskd(st, r, o).await
        }
        Err(e) => Err(e),
    }
}

// ---- Prowlarr → qBittorrent / SABnzbd -----------------------------------------------------------

async fn start_torznab(st: &AppState, r: &mut Req, lossless_only: bool) -> anyhow::Result<Out> {
    let lossless_only = lossless_only || r.d["lossless_only"].as_bool().unwrap_or(false);
    upd(st, r, &[("status", json!("searching")), ("message", json!("Searching torrent and Usenet indexers (Prowlarr)…"))]);
    let album = r.album_title();
    let mut cands = vec![];
    for query in [q(&[&r.s("artist"), &album]), q(&[&album])] {
        let res = torznab::search(st, &query).await?;
        cands = torznab::rank_album(st, &res, &r.s("artist"), &album, lossless_only);
        if !cands.is_empty() {
            break;
        }
    }
    check(st, r.id)?;
    if cands.is_empty() {
        return Ok(Out::Next("no matching release on your indexers".into()));
    }
    r.d["torznab"] = json!({ "candidates": cands, "idx": -1 });
    torznab_try_next(st, r, "").await
}

async fn torznab_try_next(st: &AppState, r: &mut Req, reason: &str) -> anyhow::Result<Out> {
    let mut reason = reason.to_string();
    if let Some(id) = r.d["torznab"]["id"].as_str().map(String::from) {
        torznab::remove(st, &id).await;
        r.d["torznab"]["id"] = Value::Null;
    }
    loop {
        let idx = r.d["torznab"]["idx"].as_i64().unwrap_or(-1) + 1;
        r.d["torznab"]["idx"] = json!(idx);
        let Some(c) = r.d["torznab"]["candidates"].get(idx as usize).cloned() else {
            return Ok(Out::Next(if reason.is_empty() { "no indexer release worked".into() } else { reason }));
        };
        match torznab::add(st, &c, r.id).await {
            Ok(id) => {
                let t = now();
                r.d["torznab"]["id"] = json!(id);
                r.d["torznab"]["started"] = json!(t);
                r.d["torznab"]["last_change"] = json!(t);
                r.d["torznab"]["last_progress"] = json!(0);
                let via = torznab::client_name(st, c["protocol"].as_str().unwrap_or("torrent"));
                upd(st, r, &[("status", json!("downloading")), ("progress", json!(0)),
                             ("message", json!(format!("{via}: {} ({})", c["title"].as_str().unwrap_or(""), c["indexer"].as_str().unwrap_or(""))))]);
                return Ok(Out::Wait);
            }
            Err(e) => reason = e.to_string(),
        }
    }
}

async fn poll_torznab(st: &AppState, r: &mut Req) -> anyhow::Result<()> {
    let Some(id) = r.d["torznab"]["id"].as_str().map(String::from) else {
        let o = torznab_try_next(st, r, "lost track of the download").await?;
        return after_slskd(st, r, o).await;
    };
    let t = now();
    match torznab::poll(st, &id).await? {
        torznab::Poll::Progress(pct, msg) => {
            if pct > r.d["torznab"]["last_progress"].as_f64().unwrap_or(0.0) {
                r.d["torznab"]["last_progress"] = json!(pct);
                r.d["torznab"]["last_change"] = json!(t);
            }
            if t - r.d["torznab"]["last_change"].as_f64().unwrap_or(t) > st.cfg.int("torznab_timeout_minutes") as f64 * 60.0 {
                let o = torznab_try_next(st, r, "download stalled").await?;
                return after_slskd(st, r, o).await;
            }
            upd(st, r, &[("progress", json!(pct)), ("message", json!(msg))]);
            Ok(())
        }
        torznab::Poll::Failed(why) => {
            let o = torznab_try_next(st, r, &why).await?;
            after_slskd(st, r, o).await
        }
        torznab::Poll::Done(path) => {
            upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
            let files = if path.is_file() { vec![path.clone()] } else { streamrip::audio_files(&path, &slskd::AUDIO_EXT) };
            if files.is_empty() {
                let o = torznab_try_next(st, r, "the download had no audio files").await?;
                return after_slskd(st, r, o).await;
            }
            let root = path.parent().map(|p| p.to_path_buf()).unwrap_or(path.clone());
            let cand = r.d["torznab"]["candidates"][r.d["torznab"]["idx"].as_u64().unwrap_or(0) as usize].clone();
            match import(st, r, &files, &root).await {
                Ok((new, _)) => {
                    torznab::remove(st, &id).await;
                    let via = if cand["protocol"] == "usenet" { "Usenet" } else { "torrent" };
                    let origin = json!({ "source": cand["indexer"].as_str().unwrap_or(via), "detail": format!("{via}: {}", cand["title"].as_str().unwrap_or("")) });
                    complete(st, r, &format!("Downloaded via {via} ({})", cand["indexer"].as_str().unwrap_or("Prowlarr")), &new, origin).await;
                    Ok(())
                }
                Err(e) if e.is::<Rejected>() => {
                    st.db.log("warning", &format!("Rejected {} for {}: {e}", cand["title"].as_str().unwrap_or(""), r.label()));
                    let o = torznab_try_next(st, r, &format!("wrong audio ({e})")).await?;
                    after_slskd(st, r, o).await
                }
                Err(e) => Err(e),
            }
        }
    }
}

// ---- streamrip (Qobuz / Tidal / Deezer) ---------------------------------------------------------

async fn isrc_for(st: &AppState, r: &mut Req) -> Option<String> {
    if let Some(i) = r.d["isrc"].as_str() {
        return Some(i.to_string());
    }
    if r.s("kind") == "track" && !r.s("mbid").is_empty() {
        if let Ok(rec) = mb::recording(st, &r.s("mbid")).await {
            r.d["isrc"] = rec["isrc"].clone();
        }
    }
    r.d["isrc"].as_str().map(String::from)
}

async fn start_streamrip(st: &AppState, r: &mut Req, urls: Option<Vec<String>>) -> anyhow::Result<Out> {
    let _slot = STREAMRIP.acquire().await?;
    check(st, r.id)?;
    upd(st, r, &[("status", json!("searching")), ("message", json!("Looking for an exact lossless match…"))]);
    let mut urls = urls.unwrap_or_default();
    if urls.is_empty() {
        if let (Some(dz), true) = (r.d["ext"]["deezer"].as_i64().map(|x| x.to_string()).or(r.d["ext"]["deezer"].as_str().map(String::from)), streamrip::services(st).iter().any(|s| s == "deezer")) {
            urls.push(format!("https://www.deezer.com/album/{dz}"));
        }
    }
    if urls.is_empty() {
        if r.s("kind") == "album" {
            let rel = release_info(st, r).await;
            let mut first_isrc = rel.as_ref().and_then(|x| x["tracks"].as_array()?.iter().find_map(|t| t["isrc"].as_str().map(String::from)));
            if first_isrc.is_none() {
                if let Some(id) = rel.as_ref().and_then(|x| x["id"].as_str()) {
                    if let Ok(full) = mb::release(st, id).await {
                        first_isrc = full["tracks"].as_array().and_then(|a| a.iter().find_map(|t| t["isrc"].as_str().map(String::from)));
                    }
                }
            }
            urls = streamrip::resolve_album(st, &r.s("artist"), &r.album_title(), n_tracks(&rel), first_isrc.as_deref()).await;
        } else {
            let isrc = isrc_for(st, r).await;
            urls = streamrip::resolve_track(st, &r.s("artist"), &r.s("title"), isrc.as_deref(), r.length()).await;
        }
    }
    if urls.is_empty() {
        return Ok(Out::Next("not found on Deezer/Tidal".into()));
    }
    let mut last = String::new();
    for url in urls {
        check(st, r.id)?;
        let out = staging(st, r).join("streamrip");
        rm(&out);
        upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("streamrip: {url}")))]);
        let res: anyhow::Result<(Vec<PathBuf>, String)> = async {
            let files = streamrip::download(st, &url, &out).await?;
            upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
            import(st, r, &files, &out).await
        }.await;
        match res {
            Ok((new, note)) => {
                rm(&staging(st, r));
                let svc = if url.contains("deezer") { "Deezer" } else if url.contains("tidal") { "Tidal" } else { "Qobuz" };
                complete(st, r, &with_note(&format!("Downloaded from {svc} via streamrip"), &note), &new, json!({ "source": svc, "detail": "via streamrip", "url": url })).await;
                return Ok(Out::Done);
            }
            Err(e) if e.is::<Cancelled>() => return Err(e),
            Err(e) => {
                st.db.log("warning", &format!("streamrip failed for {} ({url}): {e}", r.label()));
                last = e.to_string();
            }
        }
    }
    rm(&staging(st, r));
    Ok(Out::Next(format!("streamrip: {}", last.chars().take(150).collect::<String>())))
}

// ---- YouTube (yt-dlp) ---------------------------------------------------------------------------

fn progress_cb(st: &AppState, id: i64) -> impl Fn(f64) + Send + Sync {
    let db = st.db.clone();
    move |pct| {
        let _ = db.execute("UPDATE requests SET progress=?, updated_at=? WHERE id=?", p![pct, now(), id]);
    }
}

async fn start_ytdlp(st: &AppState, r: &mut Req, pick_url: Option<String>) -> anyhow::Result<Out> {
    let _slot = YTDLP.acquire().await?;
    check(st, r.id)?;
    let out = staging(st, r);
    let cb = progress_cb(st, r.id);
    if r.s("kind") == "track" {
        upd(st, r, &[("status", json!("searching")), ("message", json!("Searching YouTube Music…"))]);
        let (url, from_ytm, cover) = match &pick_url {
            Some(u) => (Some(u.clone()), u.contains("music.youtube.com"), None),
            None => match ytmusic::find_song(st, &r.s("artist"), &r.s("title"), r.length()).await {
                Some(hit) => (Some(format!("https://music.youtube.com/watch?v={}", hit["videoId"].as_str().unwrap_or(""))), true, hit["cover"].as_str().map(String::from)),
                None => (ytdlp::find_best(&r.s("artist"), &r.s("title"), r.length()).await, false, None),
            },
        };
        let Some(url) = url else { return Ok(Out::Next("no match on YouTube".into())) };
        upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("Downloading {url}")))]);
        let res = ytdlp::download(st, &url, &out, false, Some(&cb)).await?;
        let Some(f) = res.files.first() else { return Ok(Out::Next("yt-dlp produced no audio".into())) };
        let hint = r.hint().or(cover);
        let mbid = r.opt("mbid");
        match tagger::import_single(st, f, &r.s("artist"), &r.s("title"), mbid.as_deref(), &out, r.length(), true, hint.as_deref()).await {
            Ok((new, note)) => {
                rm(&out);
                let src = if from_ytm { "YouTube Music" } else { "YouTube" };
                complete(st, r, &with_note(&format!("Downloaded from {src}"), &note), &[new], json!({ "source": src, "url": url })).await;
                Ok(Out::Done)
            }
            Err(e) if e.is::<Rejected>() => {
                rm(&out);
                Ok(Out::Next(format!("YouTube match was the wrong audio ({e})")))
            }
            Err(e) => Err(e),
        }
    } else {
        let rel = release_info(st, r).await;
        let album = r.album_title();
        upd(st, r, &[("status", json!("searching")), ("message", json!("Searching YouTube Music for the album…"))]);
        let ext = r.d["ext"].clone();
        let yt_album = if let Some(u) = &pick_url {
            Some(json!({ "title": r.d["pick"]["title"].as_str().map(String::from).unwrap_or(album.clone()), "playlist_url": u }))
        } else if ext["ytm"].is_string() || ext["bandcamp"].is_string() {
            // release MusicBrainz lacks: download exactly the YouTube Music / Bandcamp release it was found on
            let mut a = json!({ "title": album, "playlist_url": ext["bandcamp"] });
            if let Some(b) = ext["ytm"].as_str() {
                match ytmusic::album(st, b).await {
                    Ok((_, Some(pl), _)) => a["playlist_url"] = json!(format!("https://music.youtube.com/playlist?list={pl}")),
                    Ok(_) => {}
                    Err(e) => st.db.log("warning", &format!("YouTube Music album lookup failed for {}: {e}", r.label())),
                }
            }
            Some(a)
        } else {
            ytmusic::find_album(st, &r.s("artist"), &album, n_tracks(&rel)).await
        };
        if let Some(pl) = yt_album.as_ref().and_then(|a| a["playlist_url"].as_str().map(String::from)) {
            let title = yt_album.as_ref().unwrap()["title"].as_str().unwrap_or("").to_string();
            upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("YouTube Music album: {title}")))]);
            let res: anyhow::Result<Option<Vec<PathBuf>>> = async {
                let dl = ytdlp::download(st, &pl, &out.join("album"), true, Some(&cb)).await?;
                let n = n_tracks(&rel);
                let enough = pick_url.is_some() || n.is_none_or(|n| dl.files.len() + (n / 5).max(1) >= n);
                if dl.files.is_empty() || !enough {
                    return Ok(None);
                }
                upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
                Ok(Some(import(st, r, &dl.files, &out).await?.0))
            }.await;
            match res {
                Ok(Some(new)) => {
                    rm(&out);
                    let via = if pl.contains("bandcamp.com") { "Bandcamp" } else { "YouTube Music" };
                    complete(st, r, &format!("Downloaded from {via}"), &new, json!({ "source": via, "detail": format!("album {title}"), "url": pl })).await;
                    return Ok(Out::Done);
                }
                Ok(None) => {}
                Err(e) if e.is::<Cancelled>() => return Err(e),
                Err(e) => st.db.log("warning", &format!("YouTube Music album for {} failed: {e}", r.label())),
            }
            rm(&out);
            if pick_url.is_some() {
                return Ok(Out::Next("the chosen YouTube album could not be downloaded or didn't match".into()));
            }
        }
        let Some(rel) = rel else { return Ok(Out::Next("no MusicBrainz tracklist for this album".into())) };
        let tracks = rel["tracks"].as_array().cloned().unwrap_or_default();
        let n = tracks.len();
        let (mut files, mut missing) = (vec![], vec![]);
        for (i, t) in tracks.iter().enumerate() {
            check(st, r.id)?;
            let title = t["title"].as_str().unwrap_or("").to_string();
            let artist = t["artist"].as_str().filter(|a| !a.is_empty()).map(String::from).unwrap_or(r.s("artist"));
            upd(st, r, &[("status", json!("downloading")), ("progress", json!((i as f64 / n as f64 * 1000.0).round() / 10.0)),
                         ("message", json!(format!("YouTube: track {}/{n} – {title}", i + 1)))]);
            let len = t["length"].as_f64().filter(|l| *l > 0.0);
            let url = match ytmusic::find_song(st, &artist, &title, len).await {
                Some(h) => Some(format!("https://music.youtube.com/watch?v={}", h["videoId"].as_str().unwrap_or(""))),
                None => ytdlp::find_best(&artist, &title, len).await,
            };
            let got = match url { Some(u) => ytdlp::download(st, &u, &out.join(format!("{:02}", i + 1)), false, None).await.map(|d| d.files).unwrap_or_default(), None => vec![] };
            if got.is_empty() {
                missing.push(title);
            }
            files.extend(got);
        }
        if missing.len() > (n / 5).max(1) {
            rm(&out);
            return Ok(Out::Next(format!("YouTube missing {}/{n} tracks", missing.len())));
        }
        upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
        match import(st, r, &files, &out).await {
            Ok((new, _)) => {
                rm(&out);
                let m = if missing.is_empty() { "Downloaded from YouTube".to_string() } else { format!("Downloaded from YouTube ({} track(s) missing)", missing.len()) };
                complete(st, r, &m, &new, json!({ "source": "YouTube Music", "detail": "matched track by track" })).await;
                Ok(Out::Done)
            }
            Err(e) if e.is::<Rejected>() => {
                rm(&out);
                Ok(Out::Next(format!("YouTube matches were wrong ({e})")))
            }
            Err(e) => Err(e),
        }
    }
}

/// Any link: downloaded as-is with yt-dlp, then tagged from what the files say.
async fn run_url(st: &AppState, r: &mut Req) -> anyhow::Result<()> {
    let _slot = YTDLP.acquire().await?;
    let out = staging(st, r);
    let cb = progress_cb(st, r.id);
    let res = ytdlp::download(st, &r.s("url"), &out, true, Some(&cb)).await?;
    check(st, r.id)?;
    if res.files.is_empty() {
        fail(st, r, "yt-dlp produced no audio files").await;
        return Ok(());
    }
    upd(st, r, &[("status", json!("importing")), ("message", json!("Tagging and importing…"))]);
    let hint = r.hint().or_else(|| (res.entries.len() == 1).then(|| res.entries[0]["thumbnail"].as_str().map(String::from)).flatten());
    let albums: std::collections::HashSet<String> = res.entries.iter().filter_map(|e| e["album"].as_str().filter(|a| !a.is_empty()).map(String::from)).collect();
    let mut new = vec![];
    if res.files.len() > 1 && albums.len() == 1 {
        let artist = res.entries[0]["artist"].as_str().map(String::from).or(r.opt("artist")).unwrap_or("Unknown Artist".into());
        new = tagger::import_album_files(st, &res.files, &artist, albums.iter().next().unwrap(), None, &out, false, None, hint.as_deref()).await?;
    } else {
        for f in &res.files {
            let t = library::read_tags(&f.to_string_lossy());
            let (mut artist, mut title) = (t.s("artist"), t.s("title"));
            let base = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if (artist.is_empty() || artist == "Unknown Artist") && base.contains(" - ") {
                let (a, b) = base.split_once(" - ").unwrap();
                (artist, title) = (a.to_string(), b.to_string());
            }
            let prefix = format!("{} - ", artist.to_lowercase());
            if !artist.is_empty() && title.to_lowercase().starts_with(&prefix) {
                title = title[prefix.len()..].to_string();
            }
            let artist = if artist.is_empty() { "Unknown Artist".to_string() } else { artist };
            match tagger::import_single(st, f, &artist, &title, None, &out, None, false, hint.as_deref()).await {
                Ok((p, _)) => new.push(p),
                Err(e) => st.db.log("warning", &format!("Skipped {base}: {e}")),
            }
        }
    }
    rm(&out);
    let url = r.s("url");
    let host = reqwest::Url::parse(&url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or("link".into());
    let src = if host.contains("music.youtube") { "YouTube Music".to_string() } else { host.trim_start_matches("www.").to_string() };
    complete(st, r, &format!("Downloaded {} file(s)", new.len()), &new, json!({ "source": src, "detail": "direct link", "url": url })).await;
    Ok(())
}

async fn run_streamrip_url(st: &AppState, r: &mut Req) -> anyhow::Result<()> {
    let _slot = STREAMRIP.acquire().await?;
    let out = staging(st, r);
    let url = r.s("url");
    let files = streamrip::download(st, &url, &out).await?;
    upd(st, r, &[("status", json!("importing")), ("message", json!("Tagging and importing…"))]);
    let mut groups: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
    for f in files {
        groups.entry(f.parent().unwrap().to_path_buf()).or_default().push(f);
    }
    let mut new = vec![];
    for fs in groups.values() {
        let t = library::read_tags(&fs[0].to_string_lossy());
        if fs.len() > 1 {
            new.extend(tagger::import_album_files(st, fs, &t.s("albumartist"), &t.s("album"), None, &out, false, None, None).await?);
        } else {
            new.push(tagger::import_single(st, &fs[0], &t.s("artist"), &t.s("title"), None, &out, None, false, None).await?.0);
        }
    }
    rm(&out);
    let svc = if url.contains("deezer") { "Deezer" } else if url.contains("tidal") { "Tidal" } else { "Qobuz" };
    complete(st, r, &format!("Downloaded {} file(s) via streamrip", new.len()), &new, json!({ "source": svc, "detail": "via streamrip", "url": url })).await;
    Ok(())
}

// ---- loop ---------------------------------------------------------------------------------------

async fn step(st: AppState, id: i64) {
    let Some(mut r) = get(&st, id) else { return };
    let status = r.s("status");
    if !ACTIVE.contains(&status.as_str()) {
        return;
    }
    let source = r.s("source");
    let res = match (status.as_str(), source.as_str()) {
        ("queued", _) => start(&st, &mut r).await,
        ("downloading", "slskd" | "slskd_lossless") => poll_slskd(&st, &mut r).await,
        ("downloading" | "monitoring", "torznab") => poll_torznab(&st, &mut r).await,
        ("monitoring", "album") => poll_album(&st, &mut r).await,
        _ => Ok(()), // searching/importing: handled inside a running task; stale ones are reset on startup
    };
    if let Err(e) = res {
        if e.is::<Cancelled>() {
            return;
        }
        tracing::warn!("request {id}: {e:#}");
        if let Some(mut r) = get(&st, id) {
            if ACTIVE.contains(&r.s("status").as_str()) {
                if !r.s("source").is_empty() && r.d["sources"].is_array() {
                    let _ = next_source(&st, &mut r, &format!("error: {e}")).await;
                } else {
                    fail(&st, &mut r, &e.to_string()).await;
                }
            }
        }
    }
}

/// Reset requests that were mid-task when the server stopped.
fn recover(st: &AppState) {
    for r in st.db.query("SELECT id, status, source FROM requests WHERE status IN ('searching','importing','downloading')", p![]).unwrap_or_default() {
        if r.s("status") == "downloading" && ["slskd", "slskd_lossless", "torznab"].contains(&r.s("source").as_str()) {
            continue;
        }
        let _ = st.db.execute("UPDATE requests SET status='queued', message='Restarted after server restart', detail='{}', updated_at=? WHERE id=?", p![now(), r.i("id")]);
    }
}

pub async fn run(st: AppState) {
    recover(&st);
    loop {
        let ids: Vec<i64> = st.db.query(&format!("SELECT id FROM requests WHERE status IN ({}) ORDER BY id", vec!["?"; ACTIVE.len()].join(",")),
            ACTIVE.iter().map(|s| json!(s)).collect()).unwrap_or_default().into_iter().map(|r| r.i("id")).collect();
        {
            let mut tasks = st.tasks.0.lock().unwrap();
            tasks.retain(|_, h| !h.is_finished());
            for id in ids {
                if tasks.contains_key(&id) {
                    continue;
                }
                let h = tokio::spawn(step(st.clone(), id));
                tasks.insert(id, h.abort_handle());
            }
        }
        crate::scheduler::tick(&st);
        let _ = st.db.execute("DELETE FROM sessions WHERE expires_at < ?", p![now()]);
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Stop work on a request (its downloads are cancelled in slskd / the download client).
pub async fn cancel(st: &AppState, id: i64) {
    let Some(r) = get(st, id) else { return };
    let src = r.s("source");
    if src.starts_with("slskd") {
        let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(-1);
        if let Some(c) = r.d["slskd"]["candidates"].get(idx.max(0) as usize).filter(|_| idx >= 0) {
            slskd_cancel_current(st, c).await;
        }
    }
    if let Some(tid) = r.d["torznab"]["id"].as_str() {
        torznab::remove(st, tid).await;
    }
    if let Some(h) = st.tasks.0.lock().unwrap().remove(&id) {
        h.abort();
    }
}
