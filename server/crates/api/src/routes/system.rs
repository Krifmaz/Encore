//! /api/system: connection tests, downloader queues, database backups; /api/discover/probe.
use crate::enrich::enrich;
use crate::auth::{Admin, User};
use encore_db::backup;
use encore_db::RowExt;
use encore_core::error::{err, ApiError, ApiResult};
use encore_requests::sources::{slskd, streamrip, torznab, ytdlp};
use crate::state::AppState;
use crate::email;
use encore_library::acoustid;
use encore_requests::notify;
use encore_db::p;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::header;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

/// The system routes: service tests, VPN and Soulseek state, streamrip's Tidal sign-in, downloads, tools, backups and updates.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/system/test/{service}", post(test))
        .route("/api/system/vpn", get(vpn_status))
        .route("/api/system/soulseek", get(soulseek_status))
        .route("/api/system/soulseek/reconnect", post(soulseek_reconnect))
        .route("/api/system/vpn/test", post(vpn_test))
        .route("/api/system/streamrip/tidal-login", post(tidal_login_start).get(tidal_login_status))
        .route("/api/system/streamrip/tidal-tokens", post(tidal_tokens_save).delete(tidal_tokens_clear))
        .route("/api/system/streamrip/reset-config", post(streamrip_reset_config))
        .route("/api/system/downloads", get(downloads))
        .route("/api/system/downloads/slskd/clear", post(clear_slskd))
        .route("/api/system/tools", get(tools_list))
        .route("/api/system/tools/check", post(tools_check))
        .route("/api/system/tools/{id}/update", post(tools_update))
        .route("/api/system/tools/{id}/reset", post(tools_reset))
        .route("/api/system/backups", get(backups).post(backup_now))
        .route("/api/system/backups/upload", post(backup_upload).layer(DefaultBodyLimit::max(4 << 30)))
        .route("/api/system/backups/{name}", get(backup_download))
        .route("/api/system/backups/{name}/restore", post(backup_restore))
        .route("/api/discover/probe", get(probe))
        // Encore's own updates and "Restart server" (update.rs)
        .route("/api/system/update", get(super::update::get_update).post(super::update::post_update))
        .route("/api/system/restart", post(super::update::post_restart))
}

// ---- the old Download tools card: answered from the plugins for one more release ---------------

/// What is installed and what the last check found (no network).
async fn tools_list(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Vec<encore_system::tools::ToolInfo>>> {
    Ok(Json(tokio::task::spawn_blocking(move || encore_system::tools::cached(&st)).await.map_err(|e| err(500, e.to_string()))?))
}

/// "Check for updates".
async fn tools_check(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Vec<encore_system::tools::ToolInfo>>> {
    Ok(Json(encore_system::tools::check(&st).await))
}

/// "Update to X" / "Install": the plugin's update, or its install when it is not installed.
async fn tools_update(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<encore_system::tools::ToolInfo>> {
    if !encore_system::tools::IDS.contains(&id.as_str()) {
        return Err(err(404, "Unknown tool"));
    }
    st.db.log("info", &format!("{} started an update of {}", u.s("username"), encore_system::tools::label(&id)));
    encore_system::tools::update(&st, &id).await.map(Json).map_err(|e| err(400, e.to_string()))
}

/// "Use the bundled version" / "Use the built-in engine" (Cadenza) / "Remove slskd": removes the plugin (its own
/// install; adopted folders and settings stay).
async fn tools_reset(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Vec<encore_system::tools::ToolInfo>>> {
    if !encore_system::tools::IDS.contains(&id.as_str()) {
        return Err(err(404, "Unknown tool"));
    }
    let st2 = st.clone();
    let id2 = id.clone();
    encore_system::tools::reset(&st2, &id2).await.map_err(|e| err(400, e.to_string()))?;
    let what = match id.as_str() {
        "cadenza" => "went back to the built-in Cadenza engine".to_string(),
        "slskd" => "removed slskd".to_string(),
        "translator" => "removed the Translator and its model".to_string(),
        _ => format!("removed the {} plugin", encore_system::tools::label(&id)),
    };
    st.db.log("info", &format!("{} {what}", u.s("username")));
    Ok(Json(tokio::task::spawn_blocking(move || encore_system::tools::cached(&st)).await.map_err(|e| err(500, e.to_string()))?))
}

// ---- database backups (Settings → Diagnostics → Backups) ----------------------------------------

/// The backups list: `{backups, dir, schema}`.
fn backups_json(st: &AppState) -> Value {
    json!({ "backups": backup::list(st.data_dir()).iter().map(|b| b.json()).collect::<Vec<_>>(), "dir": backup::dir(st.data_dir()), "schema": encore_db::SCHEMA_VERSION })
}

/// GET /api/system/backups: the database backups.
async fn backups(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(backups_json(&st)))
}

/// "Back up now".
async fn backup_now(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    let db = st.db.clone();
    let b = tokio::task::spawn_blocking(move || db.backup(backup::Kind::Scheduled)).await.map_err(|e| err(500, e.to_string()))??;
    st.db.log("info", &format!("{} backed up the database to backups/{}", u.s("username"), b.name));
    backup::prune(st.data_dir());
    let mut out = backups_json(&st);
    out["created"] = b.json();
    Ok(Json(out))
}

/// The backup file itself, for safekeeping somewhere else.
async fn backup_download(State(st): State<AppState>, Admin(_): Admin, Path(name): Path<String>) -> ApiResult<Response> {
    if !backup::valid_name(&name) {
        return Err(err(400, "Not a backup name"));
    }
    // the backups folder or the spill folder on the music disk
    let path = backup::find(st.data_dir(), &name).ok_or_else(|| err(404, "No such backup"))?;
    let f = tokio::fs::File::open(&path).await.map_err(|_| err(404, "No such backup"))?;
    let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(f));
    Response::builder()
        .header(header::CONTENT_TYPE, "application/vnd.sqlite3")
        .header(header::CONTENT_LENGTH, len)
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))
        .body(body)
        .map_err(|e| err(500, e.to_string()))
}

/// Replace the database with a backup: a safety copy of the current one is taken first.
async fn backup_restore(State(st): State<AppState>, Admin(u): Admin, Path(name): Path<String>) -> ApiResult<Json<Value>> {
    if !backup::valid_name(&name) {
        return Err(err(400, "Not a backup name"));
    }
    // the backups folder or the spill folder on the music disk
    let path = backup::find(st.data_dir(), &name).ok_or_else(|| err(404, "No such backup"))?;
    let db = st.db.clone();
    let safety = tokio::task::spawn_blocking(move || db.restore(&path)).await.map_err(|e| err(500, e.to_string()))?.map_err(|e| err(400, e.to_string()))?;
    // everything cached from the old database is stale now
    st.cfg.reload()?;
    st.cache.clear();
    st.web.lists.clear();
    // kept searches name requests by id, and the ids may point at other requests now
    st.requests.search_cache.clear(st.data_dir());
    crate::auth::ensure_owner(&st)?;
    st.db.log("warning", &format!("{} restored the database from backups/{} (the data from before is in backups/{})", u.s("username"), name, safety.name));
    tracing::info!("database restored from backups/{name} by {}", u.s("username"));
    let mut out = backups_json(&st);
    out["ok"] = json!(true);
    out["safety"] = safety.json();
    Ok(Json(out))
}

/// "Upload a backup…": the file is kept as a backup once it proves to be a sound SQLite database.
async fn backup_upload(State(st): State<AppState>, Admin(u): Admin, mut form: Multipart) -> ApiResult<Json<Value>> {
    let dir = backup::dir(st.data_dir());
    std::fs::create_dir_all(&dir).map_err(|e| err(500, e.to_string()))?;
    let tmp = dir.join(format!("upload-{}.part", encore_core::ids::random_hex(8)));
    let mut size: u64 = 0;
    let mut head: Vec<u8> = Vec::new();
    // every way out of the streaming loop with an error removes the .part file (prune never would)
    let streamed: Result<(), ApiError> = async {
        use tokio::io::AsyncWriteExt;
        let mut out = tokio::fs::File::create(&tmp).await.map_err(|e| err(500, e.to_string()))?;
        while let Some(mut f) = form.next_field().await.map_err(|e| err(400, e.to_string()))? {
            if size > 0 {
                break;
            }
            while let Some(chunk) = f.chunk().await.map_err(|e| err(400, e.to_string()))? {
                if head.len() < 16 {
                    head.extend_from_slice(&chunk[..chunk.len().min(16 - head.len())]);
                }
                size += chunk.len() as u64;
                out.write_all(&chunk).await.map_err(|e| err(500, e.to_string()))?;
            }
        }
        out.flush().await.map_err(|e| err(500, e.to_string()))?;
        out.sync_all().await.map_err(|e| err(500, e.to_string()))?;
        Ok(())
    }
    .await;
    if let Err(e) = streamed {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    if size == 0 || head.len() < 16 || head.as_slice() != backup::SQLITE_HEADER {
        let _ = std::fs::remove_file(&tmp);
        return Err(err(400, "That file isn't a SQLite database (an Encore backup is encore-….db)"));
    }
    let check = tmp.clone();
    let version = match tokio::task::spawn_blocking(move || backup::validate_file(&check)).await.map_err(|e| err(500, e.to_string()))? {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(err(400, format!("That database doesn't pass SQLite's integrity check: {e}")));
        }
    };
    let name = backup::file_name(backup::Kind::Uploaded, encore_db::now());
    if let Err(e) = std::fs::rename(&tmp, dir.join(&name)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err(500, e.to_string()));
    }
    if let Ok(d) = std::fs::File::open(&dir) {
        let _ = d.sync_all();
    }
    st.db.log("info", &format!("{} uploaded a database backup as backups/{name}", u.s("username")));
    tracing::info!("backup uploaded as backups/{name} ({size} bytes, schema {version})");
    let mut out = backups_json(&st);
    out["created"] = json!({ "name": name, "size": size, "time": encore_db::now(), "kind": backup::Kind::Uploaded.label(), "schema": version });
    Ok(Json(out))
}

// ---- Diagnostics → Server ------------------------------------------------------------------------

/// What the machine looks like from inside the container: CPUs, memory (the container's limit when there
/// is one), the file systems of /config and /music (local or network), the database's size, whether any
/// account has translations on, and the database read connections in effect. Plain facts the owner can
/// quote when a page feels slow (`/api/system/diagnostics`, key `machine`).
pub(crate) fn server_block(st: &AppState) -> Value {
    let fs = |path: &str| -> Value {
        if path.is_empty() {
            return Value::Null;
        }
        match encore_core::fsinfo::of(std::path::Path::new(path)) {
            Some(f) => json!({ "path": path, "fs": f.name, "network": f.network }),
            None => json!({ "path": path }),
        }
    };
    let (memory, memory_source) = memory_limit();
    let (readers, source) = st.db.readers_in_effect();
    // the last update check (kept in the database, no network): the Settings banner reads it from here
    let (update_available, latest) = encore_system::update::machine_fields(st);
    json!({
        "cpus": std::thread::available_parallelism().map(|n| n.get()).ok(),
        "memory": memory,
        "memory_source": memory_source,
        "config": fs(&st.data_dir().to_string_lossy()),
        "music": fs(&st.cfg.str("library_path")),
        "database_bytes": st.db.file_size(),
        "translations_on": encore_library::translate::anyone_on(st),
        "db_readers": readers,
        "db_readers_source": source.as_str(),
        "encore_version": crate::VERSION,
        "encore_update_available": update_available,
        "encore_latest": latest,
    })
}

/// (bytes, "cgroup" | "total"): the container's memory limit when one is set (cgroup v2, then v1), else
/// the machine's memory; None when neither can be read (not Linux).
fn memory_limit() -> (Option<u64>, &'static str) {
    let read = |p: &str| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string());
    // "max" (v2) or a number near 2^63 (v1) means no limit
    let limit = read("/sys/fs/cgroup/memory.max").or_else(|| read("/sys/fs/cgroup/memory/memory.limit_in_bytes"))
        .and_then(|s| s.parse::<u64>().ok()).filter(|n| *n < 1 << 60);
    if let Some(n) = limit {
        return (Some(n), "cgroup");
    }
    let total = read("/proc/meminfo").and_then(|m| {
        m.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|k| k.parse::<u64>().ok()).map(|k| k * 1024)
    });
    (total, "total")
}

#[cfg(test)]
mod server_block_tests {
    #[test]
    fn the_server_block_names_every_fact() {
        let dir = std::env::temp_dir().join(format!("encore-srvblock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("music")).unwrap();
        let st = crate::test_support::state_in(&dir);
        st.cfg.set_many(&serde_json::json!({ "library_path": dir.join("music").to_string_lossy() }));
        let b = super::server_block(&st);
        assert!(b["cpus"].as_u64().unwrap_or(0) >= 1);
        assert_eq!(b["config"]["path"], serde_json::json!(dir.to_string_lossy()));
        assert_eq!(b["music"]["path"], serde_json::json!(dir.join("music").to_string_lossy()));
        if cfg!(target_os = "linux") {
            assert!(b["memory"].as_u64().unwrap_or(0) > 0, "{b}");
            assert!(["cgroup", "total"].contains(&b["memory_source"].as_str().unwrap()));
            assert!(b["config"]["fs"].is_string() && b["config"]["network"].is_boolean(), "{b}");
        }
        assert!(b["database_bytes"].as_u64().unwrap_or(0) > 0);
        assert_eq!(b["translations_on"], serde_json::json!(false));
        assert_eq!(b["db_readers"].as_u64().unwrap(), st.db.readers_in_effect().0 as u64);
        assert!(["env", "default", "network"].contains(&b["db_readers_source"].as_str().unwrap()));
        // the owner turns translations on: the block says so at once (no restart, no cache lag)
        st.db.execute("UPDATE users SET prefs=? WHERE role='owner'", encore_db::p![r#"{"translate_mode":"under","translate_lang":"en"}"#]).unwrap();
        assert_eq!(super::server_block(&st)["translations_on"], serde_json::json!(true));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The Soulseek session for the Soulseek card, the Soulseek page and Diagnostics (see
/// `encore_requests::slskd_session::status`): its state and reason, the next attempt, the waiting searches.
async fn soulseek_status(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    if !crate::auth::can(&st, &u, "pick") {
        return Err(err(403, "Ask an admin for permission to pick downloads"));
    }
    let mut v = encore_requests::slskd_session::status(&st);
    v["engine"] = json!(st.requests.slskd.state());
    Ok(Json(v))
}

/// Reconnect now (the Soulseek card's button): refused with "Wait N s" within 30 s of the last attempt,
/// else the reconnect schedule starts over and the supervisor connects at once.
async fn soulseek_reconnect(State(st): State<AppState>, Admin(me): Admin) -> ApiResult<Json<Value>> {
    if let Some(why) = slskd::not_ready(&st) {
        return Err(err(400, why));
    }
    if st.requests.slskd.state() != "running" {
        return Err(err(400, "The Soulseek engine is starting – it connects as soon as it is up"));
    }
    use encore_requests::slskd_session::Refused;
    match st.requests.slskd.session().reconnect_now() {
        Ok(()) => {}
        Err(Refused::LoggedIn) => return Err(err(400, "Soulseek is logged in")),
        Err(Refused::Connecting) => return Err(err(400, "Soulseek is connecting now")),
        Err(Refused::Wait(wait)) => {
            let n = wait.as_secs_f64().ceil().max(1.0) as u64;
            return Err(err(429, format!("Wait {n} s")).with(json!({ "wait": n })));
        }
    }
    st.db.log("info", &format!("{} pressed Reconnect now for Soulseek", me.s("username")));
    Ok(Json(encore_requests::slskd_session::status(&st)))
}

/// Settings → Soulseek → VPN for Soulseek: the tunnel's state (polled while the section is open).
async fn vpn_status(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let mut v = st.vpn.status();
    v["message"] = json!(encore_vpn::summary(&v));
    Ok(Json(v))
}

/// "Test VPN": apply the saved settings, wait up to 10 s for a handshake, map the port once.
async fn vpn_test(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(st.vpn.test(&st.cfg).await))
}

/// Settings → streamrip → "Sign in to Tidal": starts the one-time account sign-in and returns the link to open.
async fn tidal_login_start(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    match streamrip::tidal_login_start(&st).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Ok(Json(json!({ "status": "failed", "message": e.to_string(), "signed_in": streamrip::tidal_signed_in(&st) }))),
    }
}

/// GET /api/system/streamrip/tidal-login: where streamrip's Tidal sign-in stands.
async fn tidal_login_status(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(streamrip::tidal_login_status(&st)))
}

/// `POST /api/system/streamrip/tidal-tokens` body.
#[derive(Deserialize)]
struct TidalTokens {
    /// the `[tidal]` section of a signed-in streamrip config
    #[serde(default)]
    toml: String,
}

/// "Paste tokens instead": the `[tidal]` section of a streamrip config signed in on another computer.
async fn tidal_tokens_save(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<TidalTokens>) -> ApiResult<Json<Value>> {
    if b.toml.trim().is_empty() {
        return Err(err(400, "Paste the [tidal] section of streamrip's config.toml first"));
    }
    if !streamrip::tidal_toml_has_token(&b.toml) {
        return Err(err(400, streamrip::TIDAL_NO_TOKEN_MSG));
    }
    // from here on an error is the server's (config file not writable), not a bad paste
    let v = streamrip::tidal_tokens_save(&st, &b.toml).await.map_err(|e| err(500, format!("{e:#}")))?;
    st.db.log("info", &format!("{} pasted Tidal tokens for streamrip", u.s("username")));
    Ok(Json(v))
}

/// "Sign out of Tidal": blanks every token in streamrip's config.
async fn tidal_tokens_clear(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    let v = streamrip::tidal_sign_out(&st).await.map_err(|e| err(500, format!("{e:#}")))?;
    st.db.log("info", &format!("{} signed streamrip out of Tidal", u.s("username")));
    Ok(Json(v))
}

/// Settings → streamrip → "Reset streamrip config": a fresh config.toml from streamrip's defaults; the old
/// file is kept as a backup and its Tidal sign-in, Deezer ARL and Qobuz login are carried over.
async fn streamrip_reset_config(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    if streamrip::binary(&st).is_none() {
        return Err(err(400, streamrip::NOT_INSTALLED_MSG));
    }
    let c = streamrip::reset_config(&st).await.map_err(|e| err(500, format!("{e:#}")))?;
    st.db.log("info", &format!("{} reset streamrip's config ({})", u.s("username"), c.message()));
    Ok(Json(c.json()))
}

/// `{ok, message}`.
fn msg(ok: bool, m: impl Into<String>) -> Value {
    json!({ "ok": ok, "message": m.into() })
}

/// POST /api/system/test/{service}: test a service's settings (admins).
async fn test(State(st): State<AppState>, Admin(u): Admin, Path(service): Path<String>) -> ApiResult<Json<Value>> {
    match run_test(&st, &service, Some(&u)).await {
        Ok(v) => Ok(Json(v)),
        Err(e) if e.to_string() == "__unknown" => Err(err(404, "Unknown service")),
        // the whole chain, never cut: "streamrip config path: config failed: start /config/tools/venv/bin/rip: …"
        Err(e) => Ok(Json(msg(false, format!("{e:#}")))),
    }
}

/// A settings section's test (`{ok, message}`), as the admin `u` (the email test writes to them); the builtin
/// plugins' Test buttons run the same ones (crate::builtins). Err("__unknown") for a service there is no test
/// for.
pub async fn run_test(st: &AppState, service: &str, u: Option<&encore_db::Row>) -> anyhow::Result<Value> {
    let st = st.clone();
    async {
        Ok(match service {
            "slskd" => {
                let m = st.requests.slskd.status();
                let vpn = encore_vpn::summary(&st.vpn.status());
                let state = m["state"].as_str().unwrap_or("off");
                let ver = m["version"].as_str().map(|v| format!(" {v}")).unwrap_or_default();
                if !encore_requests::slskd_managed::configured(&st) {
                    msg(false, format!("Soulseek is off or has no username and password · {vpn}"))
                } else if state != "running" {
                    let what = match state {
                        "not_installed" => "not installed – install the Soulseek (slskd) plugin under Settings → Plugins".to_string(),
                        "starting" => format!("engine{ver} starting"),
                        "waiting_vpn" => "waiting for the VPN".to_string(),
                        "crashed" => format!("engine{ver} crashed"),
                        "blocked" => format!("engine{ver} can't start"),
                        _ => "off".to_string(),
                    };
                    let err = m["error"].as_str().filter(|e| !e.is_empty() && state != "not_installed" && state != "waiting_vpn").map(|e| format!(" – {e}")).unwrap_or_default();
                    msg(false, format!("Soulseek: {what}{err} · {vpn}"))
                } else {
                    let s = slskd::status(&st).await?;
                    let server = s["server"]["state"].as_str().unwrap_or("unknown");
                    let ok = s["server"]["isLoggedIn"].as_bool().unwrap_or_else(|| server.contains("LoggedIn"));
                    let port = m["listen_port"].as_u64().map(|p| format!(" · port {p}")).unwrap_or_default();
                    let scan = m["scanning"].as_u64().map(|p| format!(" · scanning shares {p}%")).unwrap_or_default();
                    // Encore's own view of the session: the reason and the next attempt
                    let session = encore_requests::slskd_session::why_not(&st.requests.slskd.session().snapshot());
                    let why = if ok { String::new() } else { format!(" – {session}") };
                    msg(ok, format!("Soulseek engine{ver} running{scan} · server: {server}{why}{port} · {vpn}"))
                }
            }
            "musicbrainz" => {
                st.mb.get(&st, "artist", &[("query", "test".into()), ("limit", "1".into())]).await?;
                msg(true, "MusicBrainz reachable")
            }
            "torznab" | "prowlarr" | "qbit" | "sab" => torznab::status(&st).await,
            "email" => {
                let to = st.db.one("SELECT email FROM users WHERE id=?", p![u.map(|u| u.i("id")).unwrap_or(0)])?.map(|r| r.s("email")).filter(|e| !e.is_empty()).unwrap_or(st.cfg.str("smtp_from"));
                if to.is_empty() {
                    msg(false, "Add an email address to your account (Account page) to receive the test")
                } else {
                    email::send(&st, &to, &format!("{} test email", st.cfg.str("app_name")), "Email works – password resets will be sent from this address.").await?;
                    msg(true, format!("Sent to {to}"))
                }
            }
            "roon" => {
                let r = encore_playback::roon::status(&st);
                let phase = r["phase"].as_str().unwrap_or("");
                let zones = st.playback.roon.zones();
                let names: Vec<String> = zones.iter().map(|z| z["name"].as_str().unwrap_or("").to_string()).collect();
                msg(phase == "connected", match phase {
                    "connected" => format!("{} · {} zone(s): {}", r["message"].as_str().unwrap_or(""), zones.len(), names.join(", ")),
                    "off" => "Turn Roon on and save first".to_string(),
                    _ => r["message"].as_str().unwrap_or("Not connected").to_string(),
                })
            }
            "cadenza" => {
                if !st.cfg.bool("cadenza_server_enabled") {
                    msg(false, "Turned off")
                } else {
                    match encore_playback::dsp::self_test(&st).await {
                        Ok(m) => msg(true, m),
                        Err(e) => msg(false, if e.starts_with("Cadenza") { e } else { format!("Cadenza: {e}") }),
                    }
                }
            }
            "notify" => {
                notify::send(&st, "🔔 Test notification", &format!("{} notifications are working", st.cfg.str("app_name")), None).await;
                msg(true, "Sent")
            }
            "mediaservers" => {
                let done = notify::refresh_media_servers(&st).await;
                msg(!done.is_empty(), if done.is_empty() { "No media servers configured".into() } else { format!("Refreshed {}", done.join(", ")) })
            }
            "spotify" => {
                // every integration that has a key filled in, one line each
                let mut parts = vec![];
                let mut all_ok = true;
                let mut say = |name: &str, r: Result<String, String>| match r {
                    Ok(m) => parts.push(format!("{name} ✓ {m}").trim_end().to_string()),
                    Err(e) => { all_ok = false; parts.push(format!("{name} ✗ {e}")); }
                };
                let (cid, sec) = (st.cfg.str("spotify_client_id"), st.cfg.str("spotify_client_secret"));
                if !cid.is_empty() || !sec.is_empty() {
                    say("Spotify", if cid.is_empty() || sec.is_empty() { Err("needs both Client ID and secret".into()) } else {
                        match st.http.post("https://accounts.spotify.com/api/token").basic_auth(&cid, Some(&sec)).form(&[("grant_type", "client_credentials")]).send().await {
                            Ok(r) if r.status().is_success() => Ok(String::new()),
                            Ok(r) => Err(format!("rejected ({})", r.status().as_u16())),
                            Err(e) => Err(e.to_string()),
                        }
                    });
                }
                if !st.cfg.str("tidal_client_id").is_empty() || !st.cfg.str("tidal_client_secret").is_empty() {
                    say("Tidal", streamrip::tidal_token(&st).await.map(|_| String::new()).map_err(|e| e.to_string().replace("Tidal needs a ", "needs a ")));
                }
                let (lk, ls) = (st.cfg.str("lastfm_api_key"), st.cfg.str("lastfm_shared_secret"));
                if !lk.is_empty() || !ls.is_empty() {
                    say("Last.fm", if lk.is_empty() { Err("API key missing".into()) } else if ls.is_empty() {
                        match encore_http::lastfm::check_key(&st, &lk).send().await {
                            Ok(r) => { let d: Value = r.json().await.unwrap_or(Value::Null); if d["error"].is_null() { Ok("key (add the shared secret for scrobbling)".into()) } else { Err(d["message"].as_str().unwrap_or("rejected").to_string()) } }
                            Err(e) => Err(e.to_string()),
                        }
                    } else {
                        // a signed call proves the secret too
                        encore_http::lastfm::signed(&st, "auth.getToken", vec![]).await.map(|_| "key + secret".to_string()).map_err(|e| e.to_string().replace("Last.fm: ", ""))
                    });
                }
                let dt = st.cfg.str("discogs_token");
                if !dt.is_empty() {
                    say("Discogs", match encore_http::discogs::identity(&st, &dt).send().await {
                        Ok(r) if r.status().is_success() => { let d: Value = r.json().await.unwrap_or(Value::Null); Ok(format!("as {}", d["username"].as_str().unwrap_or("?"))) }
                        Ok(r) if r.status().as_u16() == 401 => Err("token rejected – use Generate new token on discogs.com/settings/developers".into()),
                        Ok(r) => Err(format!("Discogs returned {}", r.status().as_u16())),
                        Err(e) => Err(e.to_string()),
                    });
                }
                let (yc, ys) = (st.cfg.str("ytmusic_client_id"), st.cfg.str("ytmusic_client_secret"));
                if !yc.is_empty() || !ys.is_empty() {
                    say("YouTube Music", if yc.is_empty() || ys.is_empty() { Err("needs both the OAuth Client ID and secret".into()) } else {
                        match st.http.post("https://oauth2.googleapis.com/device/code").form(&[("client_id", yc.as_str()), ("scope", "https://www.googleapis.com/auth/youtube")]).send().await {
                            Ok(r) if r.status().is_success() => Ok("client ID (the secret is checked when a user signs in)".into()),
                            Ok(r) => { let d: Value = r.json().await.unwrap_or(Value::Null); Err(format!("{} – is the client of type “TVs and Limited Input devices”?", d["error"].as_str().unwrap_or("rejected"))) }
                            Err(e) => Err(e.to_string()),
                        }
                    });
                }
                if parts.is_empty() {
                    return Ok(msg(false, "Nothing to test yet – fill in at least one integration's keys, save, then test"));
                }
                msg(all_ok, parts.join(" · "))
            }
            "ytdlp" | "youtube" => ytdlp::test_cookies(&st).await?,
            "tidal" => {
                streamrip::tidal_token(&st).await?;
                msg(true, "Tidal credentials valid")
            }
            "streamrip" => match streamrip::binary(&st) {
                None => msg(false, streamrip::NOT_INSTALLED_MSG),
                Some(bin) => {
                    let c = streamrip::check_config(&st, false).await?;
                    let (tools, exe) = (st.system.tools.clone(), std::path::PathBuf::from(&bin));
                    let Some(o) = tokio::task::spawn_blocking(move || tools.answer(&exe, "--version")).await? else {
                        anyhow::bail!("{bin} did not start");
                    };
                    let v: String = format!("{}{}", o.stdout, o.stderr).trim().chars().take(120).collect();
                    let tidal = if streamrip::tidal_signed_in(&st) { "Tidal: signed in" } else { "Tidal: not signed in yet – use “Sign in to Tidal” below" };
                    let reach = streamrip::tidal_probe(&st).await;
                    let config = if c.problem.is_some() { c.message() } else if c.regenerated { format!("config written fresh and credentials written to {}", c.path.display()) } else { format!("config loads, credentials written to {}", c.path.display()) };
                    msg(o.ok && c.problem.is_none(), format!("{v} · {config} · {tidal} · {reach}"))
                }
            },
            "acoustid" => {
                if !acoustid::available(&st) {
                    msg(false, "Needs an API key and the fpcalc tool (included in the Docker image)")
                } else {
                    let d: Value = encore_http::acoustid::check_key(&st, &st.cfg.str("acoustid_api_key")).send().await?.json().await?;
                    let ok = d["status"] == "ok";
                    msg(ok, if ok { "AcoustID key valid".to_string() } else { d["error"]["message"].as_str().unwrap_or("invalid key").to_string() })
                }
            }
            _ => return Err(anyhow::anyhow!("__unknown")),
        })
    }.await
}

/// GET /api/system/downloads: what slskd and the torrent and usenet clients are downloading.
async fn downloads(State(st): State<AppState>, Admin(viewer): Admin) -> ApiResult<Json<Value>> {
    let mut out = json!({ "slskd": [], "torrents": [], "errors": {} });
    if slskd::enabled(&st) {
        match slskd::all_downloads(&st).await {
            Ok(v) => out["slskd"] = json!(v),
            Err(e) => out["errors"]["slskd"] = json!(e.to_string().chars().take(200).collect::<String>()),
        }
    }
    // torrent / Usenet downloads Encore is waiting on
    out["torrents"] = json!(st.db.query("SELECT id, artist, title, progress, message FROM requests WHERE source='torznab' AND status IN ('downloading','importing','waiting') ORDER BY id", p![])?);
    enrich(&st, Some(&viewer), &mut out).await;
    Ok(Json(out))
}

/// POST /api/system/downloads/slskd/clear: take the finished downloads off slskd's list.
async fn clear_slskd(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    slskd::clear_completed(&st).await.map_err(|e| err(502, e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /api/discover/probe` query.
#[derive(Deserialize)]
struct ProbeQ {
    /// the link to read
    url: String,
}

/// GET /api/discover/probe?url=: what yt-dlp reads from a link, enriched.
async fn probe(State(st): State<AppState>, User(viewer): User, Query(q): Query<ProbeQ>) -> ApiResult<Json<Value>> {
    let mut info = ytdlp::probe(&st, &q.url).await.map_err(|e| err(400, format!("Unsupported or unreachable URL: {}", e.to_string().chars().take(200).collect::<String>())))?;
    info["source"] = json!("ytdlp");
    enrich(&st, Some(&viewer), &mut info).await;
    Ok(Json(info))
}
