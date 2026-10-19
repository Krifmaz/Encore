//! /api/soulseek: the Soulseek page – search the network and watch answers arrive, browse a user's
//! shares, download files or folders as requests (the worker fetches, tags and imports them like a
//! manual pick), and follow the transfers – all through the Soulseek engine Encore runs (slskd).
use crate::auth::{self, Admin, User};
use encore_db::{Row, RowExt};
use encore_core::error::{err, ApiError, ApiResult};
use crate::routes::picker::create_pick;
use encore_requests::create::{quota, NewRequest};
use encore_requests::slskd_managed;
use encore_requests::sources::slskd;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/soulseek/status", get(status))
        .route("/api/soulseek/start", post(start))
        .route("/api/soulseek/search", post(search_start))
        .route("/api/soulseek/search/{id}", get(search_poll))
        .route("/api/soulseek/browse/{username}", get(browse))
        .route("/api/soulseek/download", post(download))
        .route("/api/soulseek/transfers", get(transfers))
        .route("/api/soulseek/transfers/clear", post(clear))
        .route("/api/soulseek/transfers/{username}/{id}", delete(cancel))
}

/// A search the page is polling: what was asked and when, so GET /search/{id} can say and expire it.
pub(crate) struct Session {
    q: String,
    started: Instant,
    done: bool,
}
const SESSION_TTL: Duration = Duration::from_secs(300);
/// Share lists are slow to fetch and don't change much.
const BROWSE_CACHE_SECS: u64 = 600;

fn gate(st: &AppState, u: &Row) -> ApiResult<()> {
    if !auth::can(st, u, "pick") {
        return Err(err(403, "Ask an admin for permission to pick downloads"));
    }
    if !slskd::enabled(st) {
        return Err(err(400, "Soulseek is not enabled"));
    }
    Ok(())
}

/// Soulseek (not Encore) failed: 424, because the web app treats 502–504 as "the server is down". The
/// session being down is 409 ("Soulseek isn't connected – next attempt at 14:20"), a search that can't run
/// now 429 ("Soulseek is busy with another search", "Rate limited … next search at 14:05").
fn upstream(e: anyhow::Error) -> ApiError {
    let code = if e.is::<slskd::NotConnected>() { 409 } else if e.is::<slskd::Busy>() { 429 } else { 424 };
    err(code, e.to_string().chars().take(200).collect::<String>())
}

/// Forget searches older than five minutes; slskd is told to drop its copy too.
fn sweep(st: &AppState) {
    let mut gone = vec![];
    st.web.soulseek_searches.lock().unwrap().retain(|k, s| {
        let keep = s.started.elapsed() < SESSION_TTL;
        if !keep {
            gone.push(k.clone());
        }
        keep
    });
    for id in gone {
        let st = st.clone();
        tokio::spawn(async move { slskd::search_end(&st, &id).await });
    }
}

// ---- status --------------------------------------------------------------------------------------

/// The Soulseek line for the Soulseek page and Settings: whether it's on, the engine's state
/// (off | not_installed | starting | running | crashed | waiting_vpn) and, once it runs, the sign-in.
async fn status(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "pick") {
        return Err(err(403, "Ask an admin for permission to pick downloads"));
    }
    let m = st.requests.slskd.status();
    let state = m["state"].as_str().unwrap_or("off");
    let installed = slskd_managed::installed_version(&st);
    let mut out = json!({ "enabled": slskd::enabled(&st), "configured": slskd_managed::configured(&st), "client": "managed",
        "session": encore_requests::slskd_session::status(&st),
        "state": state, "version": m["version"].clone(), "installed": installed, "vpn": m["vpn"], "hint": "",
        "logged_in": false, "username": "", "error": m["error"].as_str().unwrap_or(""), "warning": m["warning"], "download_dir": m["download_dir"],
        "shares": { "dirs": 0, "files": 0 }, "port": m["listen_port"], "scanning": m["scanning"], "parent": null, "downloads": 0, "uploads": 0 });
    if installed.is_none() {
        out["hint"] = json!("Install or update it under Download tools");
    }

    if !slskd::enabled(&st) || state != "running" {
        return Ok(Json(out));
    }
    // slskd's own answers (its status, the transfer counts): kept a few seconds and asked again in the
    // background while the last ones are shown, so the page's polling never waits on slskd; a new engine
    // run (another state, version or port) asks at once
    let key = format!("{state}\u{1f}{}\u{1f}{}", m["version"], m["listen_port"]);
    let st2 = st.clone();
    let probe = st.web.slskd_probe.get(&key, move || async move {
        let status = match tokio::time::timeout(Duration::from_secs(10), slskd::status(&st2)).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("timed out".to_string()),
        };
        match status {
            Ok(v) => {
                let (d, up) = tokio::join!(slskd::all_downloads(&st2), slskd::all_uploads(&st2));
                json!({ "status": v, "downloads": d.map(|d| d.len()).unwrap_or(0), "uploads": up.map(|u| u.len()).unwrap_or(0) })
            }
            Err(e) => json!({ "error": e }),
        }
    }).await;
    if let Some(e) = probe["error"].as_str() {
        out["error"] = json!(e);
        return Ok(Json(out));
    }
    let s = &probe["status"];
    // slskd's /application: server.state reads "Connected, LoggedIn" once it is in
    let srv = &s["server"];
    let sstate = srv["state"].as_str().unwrap_or("");
    let logged_in = srv["isLoggedIn"].as_bool().unwrap_or_else(|| sstate.contains("LoggedIn"));
    out["logged_in"] = json!(logged_in);
    out["username"] = srv["username"].as_str().or(s["user"]["username"].as_str()).unwrap_or("").into();
    // not signed in: say why in words, never just "Disconnected" – Encore's own view of the session first
    // (the reason and the next attempt: what the phone and older apps show in this field), else slskd's log
    let session = &out["session"];
    let session_down = session["state"].as_str().is_some_and(|s| s != "logged_in");
    out["error"] = json!(if logged_in {
        String::new()
    } else if session_down {
        encore_requests::slskd_session::text_elsewhere(session["text"].as_str().unwrap_or(""))
    } else if let Some(why) = m["server_error"].as_str().filter(|w| !w.is_empty()) {
        format!("not signed in to the Soulseek network – {why}")
    } else {
        let st_word = if sstate.is_empty() { "unknown".to_string() } else { sstate.to_string() };
        format!("not signed in to the Soulseek network yet ({st_word}) – it signs in once the share scan is done; if this stays, the engine log below says why")
    });
    if encore_config::perms::is_admin(&u) {
        out["log"] = json!(st.requests.slskd.log_tail());
    }
    out["shares"] = json!({ "dirs": s["shares"]["directories"].as_u64().unwrap_or(0), "files": s["shares"]["files"].as_u64().unwrap_or(0) });
    out["parent"] = s["distributedNetwork"]["parent"].as_str().filter(|p| !p.is_empty()).into();
    if let Some(v) = s["version"]["current"].as_str().or(s["version"]["full"].as_str()) {
        out["version"] = json!(encore_core::version::extract_version(v).unwrap_or_else(|| v.to_string()));
    }
    out["downloads"] = probe["downloads"].clone();
    out["uploads"] = probe["uploads"].clone();
    Ok(Json(out))
}

/// Start now (the button on the Soulseek card after a crash or a blocked start): try at once instead of
/// after the backoff. The supervisor does the starting; the card's status poll shows how it went.
async fn start(State(st): State<AppState>, Admin(me): Admin) -> ApiResult<Json<Value>> {
    if !slskd_managed::configured(&st) {
        return Err(err(400, "Soulseek is off or has no username and password – fill them in and save first"));
    }
    if slskd_managed::installed_version(&st).is_none() {
        return Err(err(400, slskd_managed::NOT_INSTALLED));
    }
    st.db.log("info", &format!("{} started Soulseek", me.s("username")));
    st.requests.slskd.start_now();
    Ok(Json(json!({ "ok": true, "state": st.requests.slskd.status()["state"] })))
}

// ---- search --------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchBody {
    q: String,
}

async fn search_start(State(st): State<AppState>, User(u): User, Json(b): Json<SearchBody>) -> ApiResult<Json<Value>> {
    gate(&st, &u)?;
    let q = b.q.trim().to_string();
    if q.is_empty() {
        return Err(err(400, "Type something to search for"));
    }
    sweep(&st);
    let id = slskd::search_start(&st, &q).await.map_err(upstream)?;
    st.web.soulseek_searches.lock().unwrap().insert(id.clone(), Session { q, started: Instant::now(), done: false });
    Ok(Json(json!({ "id": id, "seconds": slskd::search_timeout(&st) })))
}

async fn search_poll(State(st): State<AppState>, User(u): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    gate(&st, &u)?;
    sweep(&st);
    let over = || err(404, "That search is over – start a new one");
    let (q, was_done) = st.web.soulseek_searches.lock().unwrap().get(&id).map(|s| (s.q.clone(), s.done)).ok_or_else(over)?;
    let (responses, done) = match slskd::search_poll(&st, &id).await {
        Ok(x) => x,
        Err(e) if e.to_string().contains("expired") => { st.web.soulseek_searches.lock().unwrap().remove(&id); return Err(over()); }
        Err(e) => return Err(upstream(e)),
    };
    let done = done || was_done; // once over, stays over
    if done {
        if let Some(s) = st.web.soulseek_searches.lock().unwrap().get_mut(&id) {
            s.done = true;
        }
    }
    Ok(Json(json!({ "id": id, "q": q, "done": done, "results": group(&responses) })))
}

/// slskd-shaped responses → one entry per user, their audio files by folder.
fn group(responses: &[Value]) -> Vec<Value> {
    let mut users = vec![];
    for r in responses {
        let mut folders: Vec<(String, Vec<Value>)> = vec![];
        for f in r["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let Some(v) = file_json(f) else { continue };
            let dir = v["filename"].as_str().unwrap_or("").rsplit_once('\\').map(|(d, _)| d).unwrap_or("").to_string();
            match folders.iter_mut().find(|(d, _)| *d == dir) { Some((_, fs)) => fs.push(v), None => folders.push((dir, vec![v])) }
        }
        if folders.is_empty() {
            continue;
        }
        users.push(json!({ "username": r["username"], "hasFreeUploadSlot": r["hasFreeUploadSlot"].as_bool().unwrap_or(false),
            "uploadSpeed": r["uploadSpeed"].as_u64().unwrap_or(0), "queueLength": r["queueLength"].as_u64().unwrap_or(0), "folders": folders_json(folders) }));
    }
    users
}

/// Folders by name, each one's files by name.
fn folders_json(mut folders: Vec<(String, Vec<Value>)>) -> Vec<Value> {
    folders.sort_by_cached_key(|(d, _)| d.to_lowercase());
    folders.into_iter().map(|(d, mut files)| {
        files.sort_by_cached_key(|f| f["filename"].as_str().unwrap_or("").to_lowercase());
        json!({ "directory": d, "files": files })
    }).collect()
}

/// One file as the page shows it; None for anything that isn't audio.
fn file_json(f: &Value) -> Option<Value> {
    let name = f["filename"].as_str().unwrap_or("");
    let ext = slskd::ext(name);
    if !encore_core::audio::DOWNLOAD_EXTS.contains(&ext.as_str()) {
        return None;
    }
    Some(json!({ "filename": name, "name": name.rsplit('\\').next().unwrap_or(name), "size": f["size"].as_u64().unwrap_or(0), "extension": ext,
        "bitRate": f["bitRate"], "bitDepth": f["bitDepth"], "sampleRate": f["sampleRate"], "length": f["length"] }))
}

// ---- browse --------------------------------------------------------------------------------------

async fn browse(State(st): State<AppState>, User(u): User, Path(username): Path<String>) -> ApiResult<Json<Value>> {
    gate(&st, &u)?;
    // share lists are big: their own cache, held to a number of bytes and of lists, rather than the general one
    let dirs = match st.web.soulseek_browsed.get(&username) {
        Some(d) => d,
        None => {
            let d = Arc::new(slskd::browse(&st, &username).await
                .map_err(|e| if e.to_string().contains("did not send") { err(408, e.to_string()) } else { upstream(e) })?);
            let bytes = username.len() + d.iter().map(encore_core::cache::json_bytes).sum::<usize>();
            st.web.soulseek_browsed.insert(username.clone(), d.clone(), bytes, Some(Duration::from_secs(BROWSE_CACHE_SECS)), 0);
            d
        }
    };
    let folders = dirs.iter().filter_map(|d| {
        let files: Vec<Value> = d["files"].as_array().map(|a| a.iter().filter_map(file_json).collect()).unwrap_or_default();
        (!files.is_empty()).then(|| (d["name"].as_str().unwrap_or("").to_string(), files))
    }).collect();
    Ok(Json(json!({ "username": username, "folders": folders_json(folders) })))
}

// ---- download ------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct DownloadBody {
    username: String,
    directory: String,
    /// { filename, size } – a bitRate, when the page has one, only decorates the request's message
    files: Vec<Value>,
    kind: Option<String>,
    artist: Option<String>,
    title: Option<String>,
    album: Option<String>,
}

fn given(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|x| !x.is_empty()).map(String::from)
}

/// Download chosen files from one user: a request locked to them, exactly like a Soulseek pick.
async fn download(State(st): State<AppState>, User(u): User, Json(b): Json<DownloadBody>) -> ApiResult<Json<Value>> {
    gate(&st, &u)?;
    let username = b.username.trim().to_string();
    let files: Vec<&Value> = b.files.iter().filter(|f| !f["filename"].as_str().unwrap_or("").is_empty()).collect();
    if username.is_empty() || files.is_empty() {
        return Err(err(400, "Pick a user and at least one file"));
    }
    let first = files[0]["filename"].as_str().unwrap_or("");
    let directory = if b.directory.is_empty() { first.rsplit_once('\\').map(|(d, _)| d.to_string()).unwrap_or_default() } else { b.directory.clone() };
    let kind = match b.kind.as_deref() {
        Some("album") => "album",
        Some("track") => "track",
        Some(_) => return Err(err(400, "kind is album or track")),
        None => if files.len() > 1 { "album" } else { "track" },
    };
    let (g_artist, g_album, g_title) = guess(&directory, first);
    // the import tags from the files themselves, so a guess only has to be a label, never a blocker
    let artist = given(&b.artist).unwrap_or(if g_artist.is_empty() { "Unknown artist".into() } else { g_artist });
    let album = given(&b.album).unwrap_or(g_album);
    let mut title = given(&b.title).unwrap_or(if kind == "album" { album.clone() } else { g_title });
    if title.is_empty() {
        title = first.rsplit('\\').next().unwrap_or(first).rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| first.to_string());
    }
    if title.is_empty() {
        return Err(err(400, "That file has no name to request it by"));
    }
    quota(&st, &u, 1)?;
    let exts: HashSet<String> = files.iter().map(|f| slskd::ext(f["filename"].as_str().unwrap_or(""))).collect();
    let format = if exts.len() == 1 { exts.into_iter().next().unwrap_or_default() } else { "mixed".into() };
    let bitrate = files.iter().filter_map(|f| f["bitRate"].as_i64()).filter(|b| *b > 0).min().unwrap_or(0);
    let payload = json!({ "username": username, "directory": directory, "format": format, "bitrate": bitrate,
        "files": files.iter().map(|f| json!({ "filename": f["filename"], "size": f["size"].as_u64().unwrap_or(0) })).collect::<Vec<_>>() });
    let req = NewRequest { kind: kind.into(), artist: Some(artist.clone()), title: Some(title.clone()), album: Some(album).filter(|a| !a.is_empty()), ..Default::default() };
    let (id, status) = create_pick(&st, &u, &req, "slskd", payload, &format!("{artist} – {title}"))?;
    Ok(Json(json!({ "request_id": id, "status": status })))
}

/// "01 - ", "1. ", "A1 " and the like in front of a file name.
static TRACK_NO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[A-Da-d]?\d{1,3}\s*[.\-_)]\s*|[A-Da-d]\d{1,2}\s+|\d{2,3}\s+)").unwrap());
/// "2001 - " or "(2001) " in front of a folder name.
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\(\[]?(?:19|20)\d{2}[\)\]]?\s*[-–.]?\s+").unwrap());
/// "[FLAC]", "(2001)", "(Remastered)" at the end of a folder name.
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[][^\)\]]*[\)\]]\s*$").unwrap());

/// (artist, album, title) read off a remote path: the album is the last folder and the artist the
/// one before it; when the folders don't say, the "Artist - Title" halves of the file name (or of a
/// lone "Artist - Album" folder) do.
fn guess(directory: &str, filename: &str) -> (String, String, String) {
    // "@@abcd" is a client's virtual share root, not a folder anyone named
    let parts: Vec<&str> = directory.split('\\').map(str::trim).filter(|p| !p.is_empty() && !p.starts_with("@@")).collect();
    let mut album = parts.last().map(|s| s.to_string()).unwrap_or_default();
    let mut artist = parts.len().checked_sub(2).map(|i| parts[i].to_string()).unwrap_or_default();
    let base = filename.rsplit('\\').next().unwrap_or(filename);
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let stem = TRACK_NO.replace(stem, "").trim().to_string();
    let mut title = stem.clone();
    if let Some((a, t)) = stem.split_once(" - ").map(|(a, t)| (a.trim(), t.trim())).filter(|(a, t)| !a.is_empty() && !t.is_empty()) {
        if artist.is_empty() {
            artist = a.to_string();
            title = t.to_string();
        } else if a.eq_ignore_ascii_case(&artist) {
            title = t.to_string(); // "Artist - Title" repeats the folder's artist
        }
    }
    if artist.is_empty() {
        if let Some((a, al)) = album.split_once(" - ").map(|(a, al)| (a.trim(), al.trim())).filter(|(a, al)| !a.is_empty() && !al.is_empty()) {
            (artist, album) = (a.to_string(), al.to_string());
        }
    }
    let mut clean = YEAR.replace(&album, "").to_string();
    while let Some(m) = TAG.find(&clean) {
        if m.start() == 0 {
            break; // the whole name is bracketed: leave it
        }
        clean.truncate(m.start());
    }
    if !clean.trim().is_empty() {
        album = clean.trim().to_string();
    }
    (artist, album, title)
}

// ---- transfers -----------------------------------------------------------------------------------

async fn transfers(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    gate(&st, &u)?;
    let dir_of = |n: &str| n.rsplit_once('\\').map(|(d, _)| d.to_string()).unwrap_or_default();
    let downloads: Vec<Value> = slskd::all_downloads(&st).await.map_err(upstream)?.into_iter().map(|d| {
        let name = d["path"].as_str().unwrap_or("");
        json!({ "id": d["id"], "username": d["username"], "filename": name, "directory": dir_of(name), "basename": d["filename"], "state": d["state"],
            "progress": d["progress"], "bytes": d["bytes"].as_u64().unwrap_or(0), "size": d["size"], "speed": d["speed"], "error": d["error"].as_str().unwrap_or("") })
    }).collect();
    // uploads are a nicety: a failure there doesn't hide the downloads
    let uploads: Vec<Value> = slskd::all_uploads(&st).await.unwrap_or_default().into_iter().map(|x| {
        let name = x["path"].as_str().unwrap_or("");
        json!({ "id": x["id"], "username": x["username"], "filename": name, "basename": x["filename"], "state": x["state"],
            "bytes": x["bytes"].as_u64().unwrap_or(0), "size": x["size"] })
    }).collect();
    Ok(Json(json!({ "downloads": downloads, "uploads": uploads })))
}

async fn cancel(State(st): State<AppState>, User(u): User, Path((username, id)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    gate(&st, &u)?;
    slskd::cancel(&st, &username, &id).await;
    Ok(Json(json!({ "ok": true })))
}

async fn clear(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    gate(&st, &u)?;
    slskd::clear_completed(&st).await.map_err(upstream)?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(dir: &str, file: &str) -> String {
        let (a, al, t) = guess(dir, file);
        format!("{a} | {al} | {t}")
    }

    #[test]
    fn guesses_artist_album_title_from_paths() {
        assert_eq!(g("@@music\\Daft Punk\\Discovery", "@@music\\Daft Punk\\Discovery\\01 - One More Time.flac"), "Daft Punk | Discovery | One More Time");
        assert_eq!(g("@@music\\Daft Punk\\(2001) Discovery [FLAC]", "@@music\\Daft Punk\\(2001) Discovery [FLAC]\\03. Digital Love.flac"), "Daft Punk | Discovery | Digital Love");
        assert_eq!(g("@@music\\Daft Punk\\Discovery", "@@music\\Daft Punk\\Discovery\\04 - daft punk - Harder Better Faster Stronger.mp3"), "Daft Punk | Discovery | Harder Better Faster Stronger");
        assert_eq!(g("@@music\\Singles", "@@music\\Singles\\Daft Punk - Digital Love.mp3"), "Daft Punk | Singles | Digital Love");
        assert_eq!(g("@@music\\Daft Punk - Discovery", "@@music\\Daft Punk - Discovery\\A1 Digital Love.flac"), "Daft Punk | Discovery | Digital Love");
        assert_eq!(g("Prince\\1999", "Prince\\1999\\07 Free.flac"), "Prince | 1999 | Free");
        assert_eq!(g("", "7 Rings.mp3"), " |  | 7 Rings");
    }

    #[test]
    fn groups_results_per_user_and_folder() {
        let r = json!([{ "username": "alice", "hasFreeUploadSlot": true, "uploadSpeed": 500, "queueLength": 2, "files": [
            { "filename": "@@m\\B\\Two\\02 - b.flac", "size": 2, "bitRate": 900 }, { "filename": "@@m\\A\\One\\cover.jpg", "size": 9 },
            { "filename": "@@m\\A\\One\\01 - a.mp3", "size": 1, "bitRate": 320 }, { "filename": "@@m\\B\\Two\\01 - a.flac", "size": 3 } ] },
            { "username": "bob", "files": [{ "filename": "x\\readme.txt", "size": 1 }] }]);
        let g = group(r.as_array().unwrap());
        assert_eq!(g.len(), 1, "bob shares no audio: {g:?}");
        assert_eq!(g[0]["username"], "alice");
        assert_eq!(g[0]["folders"][0]["directory"], "@@m\\A\\One");
        assert_eq!(g[0]["folders"][0]["files"][0]["name"], "01 - a.mp3");
        assert_eq!(g[0]["folders"][0]["files"][0]["extension"], "mp3");
        assert_eq!(g[0]["folders"][1]["files"].as_array().unwrap().len(), 2);
        assert_eq!(g[0]["folders"][1]["files"][0]["filename"], "@@m\\B\\Two\\01 - a.flac");
    }
}
