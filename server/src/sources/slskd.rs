//! Soulseek for the worker, the picker and the Soulseek page: result ranking, and slskd's API –
//! the slskd Encore runs itself (`crate::slskd_managed`), on 127.0.0.1 with its own API key.
use crate::slskd_managed;
use crate::state::AppState;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

pub const AUDIO_EXT: [&str; 12] = ["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "alac", "aiff", "wma", "ape", "wv"];
pub const LOSSLESS: [&str; 6] = ["flac", "wav", "alac", "aiff", "ape", "wv"];
pub const DONE_OK: &str = "Succeeded";
pub const DONE_BAD: [&str; 6] = ["Errored", "Rejected", "TimedOut", "Cancelled", "Aborted", "Failed"];

/// Soulseek is on, has a login and its engine is installed – a source the worker can use.
pub fn enabled(st: &AppState) -> bool {
    slskd_managed::configured(st) && slskd_managed::installed_version(st).is_some()
}

/// Why slskd can't answer right now, in words for the user (None while it runs or is starting).
fn not_ready(st: &AppState) -> Option<String> {
    if !slskd_managed::configured(st) {
        return Some("Soulseek: switch it on and set a username and password in Settings → Apps & sources → Soulseek (an account is created on first login)".into());
    }
    let s = slskd_managed::status();
    match s["state"].as_str().unwrap_or("off") {
        "running" | "starting" => None,
        "not_installed" => Some(slskd_managed::NOT_INSTALLED.into()),
        "waiting_vpn" => Some(slskd_managed::WAITING_FOR_VPN.into()),
        "crashed" => Some(format!("Soulseek engine stopped: {}", s["error"].as_str().unwrap_or("unknown problem"))),
        _ if slskd_managed::installed_version(st).is_none() => Some(slskd_managed::NOT_INSTALLED.into()),
        _ => Some("Soulseek is starting – try again in a moment".into()),
    }
}

async fn req(st: &AppState, method: reqwest::Method, path: &str, body: Option<Value>, query: &[(&str, &str)]) -> anyhow::Result<Value> {
    req_within(st, method, path, body, query, None).await
}

/// `timeout` overrides the http client's 30 s for calls slskd itself waits on a peer for.
async fn req_within(st: &AppState, method: reqwest::Method, path: &str, body: Option<Value>, query: &[(&str, &str)], timeout: Option<std::time::Duration>) -> anyhow::Result<Value> {
    if let Some(why) = not_ready(st) {
        anyhow::bail!(why);
    }
    let url = format!("{}/api/v0/{}", slskd_managed::base_url(), path.trim_start_matches('/'));
    let mut rb = st.http.request(method, &url).header("X-API-Key", slskd_managed::api_key(st)).query(query);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    if let Some(t) = timeout {
        rb = rb.timeout(t);
    }
    let r = rb.send().await.map_err(|e| {
        if e.is_connect() { anyhow::anyhow!("Soulseek is starting – try again in a moment") } else { anyhow::anyhow!("Soulseek engine didn't answer: {e}") }
    })?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if status.as_u16() >= 400 {
        anyhow::bail!("Soulseek engine returned {status}: {}", text.chars().take(200).collect::<String>());
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// slskd's /application: version, server.state ("Connected, LoggedIn"), shares, …
pub async fn status(st: &AppState) -> anyhow::Result<Value> {
    req(st, reqwest::Method::GET, "application", None, &[]).await
}

pub fn search_timeout(st: &AppState) -> i64 {
    st.cfg.int("slskd_search_timeout").max(5)
}

fn search_body(sid: &str, text: &str, timeout: i64) -> Value {
    json!({ "id": sid, "searchText": text, "searchTimeout": timeout * 1000, "responseLimit": 200, "fileLimit": 20000,
        "filterResponses": true, "minimumResponseFileCount": 1 })
}

/// Run a search and wait for it to finish. Returns the responses.
pub async fn search(st: &AppState, text: &str) -> anyhow::Result<Vec<Value>> {
    let timeout = search_timeout(st);
    let sid = uuid();
    req(st, reqwest::Method::POST, "searches", Some(search_body(&sid, text, timeout)), &[]).await?;
    for _ in 0..timeout + 20 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Ok(s) = req(st, reqwest::Method::GET, &format!("searches/{sid}"), None, &[]).await {
            if s["isComplete"].as_bool().unwrap_or(false) {
                break;
            }
        }
    }
    let res = req(st, reqwest::Method::GET, &format!("searches/{sid}/responses"), None, &[]).await;
    let _ = req(st, reqwest::Method::DELETE, &format!("searches/{sid}"), None, &[]).await;
    Ok(res?.as_array().cloned().unwrap_or_default())
}

fn uuid() -> String {
    let b: [u8; 16] = rand::random();
    let h = hex::encode(b);
    format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

/// Start a search for a page to poll with `search_poll`; it runs for `slskd_search_timeout` seconds.
pub async fn search_start(st: &AppState, text: &str) -> anyhow::Result<String> {
    let sid = uuid();
    req(st, reqwest::Method::POST, "searches", Some(search_body(&sid, text, search_timeout(st))), &[]).await?;
    Ok(sid)
}

/// The responses a search has so far and whether it is over ("search expired" once slskd forgot it).
pub async fn search_poll(st: &AppState, id: &str) -> anyhow::Result<(Vec<Value>, bool)> {
    let s = req(st, reqwest::Method::GET, &format!("searches/{id}"), None, &[]).await
        .map_err(|e| if e.to_string().contains("returned 404") { anyhow::anyhow!("search expired") } else { e })?;
    let res = req(st, reqwest::Method::GET, &format!("searches/{id}/responses"), None, &[]).await?;
    Ok((res.as_array().cloned().unwrap_or_default(), s["isComplete"].as_bool().unwrap_or(false)))
}

/// Forget a polled search (slskd keeps them until told).
pub async fn search_end(st: &AppState, id: &str) {
    let _ = req(st, reqwest::Method::DELETE, &format!("searches/{id}"), None, &[]).await;
}

/// A user's share list: [{ name, files: [{ filename (full path), size, bitRate, … }] }].
pub async fn browse(st: &AppState, user: &str) -> anyhow::Result<Vec<Value>> {
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

pub async fn enqueue(st: &AppState, username: &str, files: &[Value]) -> anyhow::Result<()> {
    let body: Vec<Value> = files.iter().map(|f| json!({ "filename": f["filename"], "size": f["size"] })).collect();
    req(st, reqwest::Method::POST, &format!("transfers/downloads/{}", urlencoding::encode(username)), Some(json!(body)), &[]).await?;
    Ok(())
}

pub async fn user_downloads(st: &AppState, username: &str) -> Vec<Value> {
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

pub async fn all_downloads(st: &AppState) -> anyhow::Result<Vec<Value>> {
    Ok(flatten_transfers(&req(st, reqwest::Method::GET, "transfers/downloads", None, &[]).await?))
}

/// Other people fetching from the shared library.
pub async fn all_uploads(st: &AppState) -> anyhow::Result<Vec<Value>> {
    Ok(flatten_transfers(&req(st, reqwest::Method::GET, "transfers/uploads", None, &[]).await?))
}

pub async fn cancel(st: &AppState, username: &str, file_id: &str) {
    let _ = req(st, reqwest::Method::DELETE, &format!("transfers/downloads/{}/{file_id}", urlencoding::encode(username)), None, &[("remove", "true")]).await;
}

pub async fn clear_completed(st: &AppState) -> anyhow::Result<()> {
    req(st, reqwest::Method::DELETE, "transfers/downloads/all/completed", None, &[]).await.map(|_| ())
}

// ---- ranking ------------------------------------------------------------------------------------

static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[].*?[\)\]]").unwrap());
static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
static ALBUM_BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix|karaoke|instrumental|8d|nightcore)\b").unwrap());
static ALBUM_OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix|karaoke|instrumental)\b").unwrap());
static TRACK_BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix|karaoke|instrumental|8d|nightcore|cover)\b").unwrap());
static TRACK_OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix|instrumental)\b").unwrap());

fn norm(s: &str) -> String {
    let s = BRACKETS.replace_all(&s.to_lowercase(), " ").to_string();
    NON_ALNUM.replace_all(&s, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn ext(f: &str) -> String {
    f.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

fn contains(hay: &str, needle: &str) -> f64 {
    let n = norm(needle);
    let words: Vec<&str> = n.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let h = format!(" {} ", norm(hay));
    words.iter().filter(|w| h.contains(&format!(" {w} "))).count() as f64 / words.len() as f64
}

fn format_score(st: &AppState, files: &[&Value], lossless_only: bool) -> Option<f64> {
    let pref = st.cfg.str("slskd_preferred_format");
    let min_br = st.cfg.int("slskd_min_bitrate");
    let exts: std::collections::HashSet<String> = files.iter().map(|f| ext(f["filename"].as_str().unwrap_or(""))).collect();
    if exts.len() != 1 {
        return None;
    }
    let e = exts.into_iter().next().unwrap();
    let lossless = LOSSLESS.contains(&e.as_str());
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

pub fn rank_album(st: &AppState, responses: &[Value], artist: &str, album: &str, track_count: Option<usize>, lossless_only: bool) -> Vec<Value> {
    let mut cands = vec![];
    for resp in responses {
        let mut dirs: Vec<(String, Vec<&Value>)> = vec![];
        for f in resp["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let name = f["filename"].as_str().unwrap_or("");
            if !AUDIO_EXT.contains(&ext(name).as_str()) {
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
    let mut seen = std::collections::HashSet::new();
    cands.retain(|c| seen.insert(c["username"].as_str().unwrap_or("").to_string()));
    cands.truncate(st.cfg.int("slskd_max_candidates").max(1) as usize);
    cands
}

pub fn rank_track(st: &AppState, responses: &[Value], artist: &str, title: &str, length: Option<f64>, lossless_only: bool) -> Vec<Value> {
    let mut cands = vec![];
    for resp in responses {
        for f in resp["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let raw = f["filename"].as_str().unwrap_or("");
            if !AUDIO_EXT.contains(&ext(raw).as_str()) {
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
            let tail = crate::matcher::from_title_on(title, base.rsplit_once('.').map(|x| x.0).unwrap_or(base));
            if crate::matcher::unwanted_version(title, artist, &tail).is_some() {
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
    cands.truncate(st.cfg.int("slskd_max_candidates").max(1) as usize);
    cands
}

/// Locate a finished download on the shared volume.
pub fn local_path(st: &AppState, remote: &str) -> Option<std::path::PathBuf> {
    let root = std::path::PathBuf::from(st.cfg.str("slskd_download_path"));
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
    walkdir::WalkDir::new(&root).into_iter().filter_entry(|e| e.depth() == 0 || e.file_name() != ".incomplete").flatten()
        .find(|e| e.file_type().is_file() && e.file_name() == base).map(|e| e.into_path())
}
