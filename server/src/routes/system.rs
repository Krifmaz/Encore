//! /api/system: service status, connection tests, downloader queues, database backups; /api/discover/probe.
use crate::auth::{Admin, User};
use crate::backup;
use crate::db::RowExt;
use crate::error::{err, ApiError, ApiResult};
use crate::sources::{slskd, streamrip, torznab, ytdlp};
use crate::state::AppState;
use crate::{acoustid, email, notify, p};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::header;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/system/status", get(status))
        .route("/api/system/test/{service}", post(test))
        .route("/api/system/streamrip/tidal-login", post(tidal_login_start).get(tidal_login_status))
        .route("/api/system/downloads", get(downloads))
        .route("/api/system/downloads/slskd/clear", post(clear_slskd))
        .route("/api/system/backups", get(backups).post(backup_now))
        .route("/api/system/backups/upload", post(backup_upload).layer(DefaultBodyLimit::max(4 << 30)))
        .route("/api/system/backups/{name}", get(backup_download))
        .route("/api/system/backups/{name}/restore", post(backup_restore))
        .route("/api/discover/probe", get(probe))
}

// ---- database backups (Settings → Diagnostics → Backups) ----------------------------------------

fn backups_json(st: &AppState) -> Value {
    json!({ "backups": backup::list(st.data_dir()).iter().map(|b| b.json()).collect::<Vec<_>>(), "dir": backup::dir(st.data_dir()), "schema": crate::db::SCHEMA_VERSION })
}

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
    let path = backup::dir(st.data_dir()).join(&name);
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
    let path = backup::dir(st.data_dir()).join(&name);
    if !path.is_file() {
        return Err(err(404, "No such backup"));
    }
    let db = st.db.clone();
    let safety = tokio::task::spawn_blocking(move || db.restore(&path)).await.map_err(|e| err(500, e.to_string()))?.map_err(|e| err(400, e.to_string()))?;
    // everything cached from the old database is stale now
    st.cfg.reload()?;
    st.cache.lock().unwrap().clear();
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
    let tmp = dir.join(format!("upload-{}.part", crate::auth::random_hex(8)));
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
    let name = backup::file_name(backup::Kind::Uploaded, crate::db::now());
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
    out["created"] = json!({ "name": name, "size": size, "time": crate::db::now(), "kind": backup::Kind::Uploaded.label(), "schema": version });
    Ok(Json(out))
}

fn which(name: &str) -> bool {
    std::env::var("PATH").map(|p| p.split(':').any(|d| std::path::Path::new(d).join(name).is_file())).unwrap_or(false)
}

fn disk(path: &str) -> Value {
    let o = std::process::Command::new("df").args(["-Pk", path]).output();
    let Ok(o) = o else { return Value::Null };
    let text = String::from_utf8_lossy(&o.stdout);
    let Some(line) = text.lines().nth(1) else { return Value::Null };
    let f: Vec<u64> = line.split_whitespace().skip(1).take(3).filter_map(|x| x.parse().ok()).collect();
    if f.len() < 3 { Value::Null } else { json!({ "total": f[0] * 1024, "free": f[2] * 1024 }) }
}

async fn status(State(st): State<AppState>, User(_): User) -> ApiResult<Json<Value>> {
    let mbv = tokio::time::timeout(std::time::Duration::from_secs(10), st.mb.get(&st, "artist", &[("query", "test".into()), ("limit", "1".into())])).await;
    let mb = match mbv { Ok(Ok(_)) => json!({ "ok": true }), Ok(Err(e)) => json!({ "ok": false, "error": e.to_string() }), Err(_) => json!({ "ok": false, "error": "timed out" }) };
    let sl = if slskd::enabled(&st) {
        match tokio::time::timeout(std::time::Duration::from_secs(10), slskd::status(&st)).await {
            Ok(Ok(v)) => json!({ "ok": true, "version": v["version"]["full"].as_str().or(v["version"].as_str()) }),
            Ok(Err(e)) => json!({ "ok": false, "error": e.to_string() }),
            Err(_) => json!({ "ok": false, "error": "timed out" }),
        }
    } else {
        json!({ "ok": false, "enabled": false })
    };
    let services = json!({
        "musicbrainz": mb, "slskd": sl,
        "ytdlp": { "ok": ytdlp::enabled(&st), "version": ytdlp::version().await, "enabled": ytdlp::enabled(&st) },
        "torznab": { "ok": torznab::enabled(&st), "enabled": st.cfg.bool("torznab_enabled") },
        "lidarr": { "ok": false, "enabled": false },
        "acoustid": { "ok": acoustid::available(&st), "enabled": acoustid::available(&st) },
        "streamrip": { "ok": streamrip::enabled(&st), "enabled": st.cfg.bool("streamrip_enabled") },
        "ffmpeg": { "ok": which("ffmpeg") },
    });
    Ok(Json(json!({ "version": crate::VERSION, "server": "rust", "services": services, "disk": disk(&st.cfg.str("library_path")), "app_name": st.cfg.str("app_name") })))
}

/// Settings → streamrip → "Sign in to Tidal": starts the one-time account sign-in and returns the link to open.
async fn tidal_login_start(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    match streamrip::tidal_login_start(&st).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Ok(Json(json!({ "status": "failed", "message": e.to_string(), "signed_in": streamrip::tidal_signed_in(&st) }))),
    }
}

async fn tidal_login_status(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(streamrip::tidal_login_status(&st)))
}

fn msg(ok: bool, m: impl Into<String>) -> Value {
    json!({ "ok": ok, "message": m.into() })
}

async fn test(State(st): State<AppState>, Admin(u): Admin, Path(service): Path<String>) -> ApiResult<Json<Value>> {
    let res: anyhow::Result<Value> = async {
        Ok(match service.as_str() {
            "slskd" => {
                let s = slskd::status(&st).await?;
                if s["soulseek"].is_object() {
                    let x = &s["soulseek"];
                    let ok = x["logged_in"].as_bool().unwrap_or(false);
                    let err = x["error"].as_str().unwrap_or("");
                    let text = if ok {
                        format!("Built-in Soulseek client signed in as {} · sharing {} files in {} folders · port {}{}", x["username"].as_str().unwrap_or(""),
                            x["shares"]["files"], x["shares"]["dirs"], x["listen_port"], if x["parent"].is_string() { "" } else { " · no distributed parent yet (searches from others arrive once one is found)" })
                    } else if !err.is_empty() { format!("Not signed in: {err}") } else { "Connecting to the Soulseek server…".to_string() };
                    msg(ok, text)
                } else {
                    msg(true, format!("slskd {} – server {}", s["version"]["full"].as_str().unwrap_or(""), s["server"]["state"].as_str().unwrap_or("unknown")))
                }
            }
            "musicbrainz" => {
                st.mb.get(&st, "artist", &[("query", "test".into()), ("limit", "1".into())]).await?;
                msg(true, "MusicBrainz reachable")
            }
            "torznab" | "prowlarr" | "qbit" | "sab" => torznab::status(&st).await,
            "email" => {
                let to = st.db.one("SELECT email FROM users WHERE id=?", p![u.i("id")])?.map(|r| r.s("email")).filter(|e| !e.is_empty()).unwrap_or(st.cfg.str("smtp_from"));
                if to.is_empty() {
                    msg(false, "Add an email address to your account (Account page) to receive the test")
                } else {
                    email::send(&st, &to, &format!("{} test email", st.cfg.str("app_name")), "Email works – password resets will be sent from this address.").await?;
                    msg(true, format!("Sent to {to}"))
                }
            }
            "roon" => {
                let r = crate::roon::status(&st);
                let phase = r["phase"].as_str().unwrap_or("");
                let zones = crate::roon::zones();
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
                    match crate::dsp::self_test(&st).await {
                        Ok(m) => msg(true, m),
                        Err(e) => msg(false, format!("Cadenza: {e}")),
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
                        match st.http.get("https://ws.audioscrobbler.com/2.0/").query(&[("method", "artist.getinfo"), ("artist", "Cher"), ("api_key", lk.as_str()), ("format", "json")]).send().await {
                            Ok(r) => { let d: Value = r.json().await.unwrap_or(Value::Null); if d["error"].is_null() { Ok("key (add the shared secret for scrobbling)".into()) } else { Err(d["message"].as_str().unwrap_or("rejected").to_string()) } }
                            Err(e) => Err(e.to_string()),
                        }
                    } else {
                        // a signed call proves the secret too
                        crate::scrobble::lastfm_signed(&st, "auth.getToken", vec![]).await.map(|_| "key + secret".to_string()).map_err(|e| e.to_string().replace("Last.fm: ", ""))
                    });
                }
                let dt = st.cfg.str("discogs_token");
                if !dt.is_empty() {
                    say("Discogs", match st.http.get("https://api.discogs.com/oauth/identity").header("Authorization", format!("Discogs token={dt}")).header("User-Agent", "Encore/2 (+https://github.com/Krifmaz/Encore)").send().await {
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
                None => msg(false, "streamrip is not installed in this container (use the default Docker image or set its path)"),
                Some(bin) => {
                    let path = streamrip::apply_credentials(&st).await?;
                    let o = tokio::process::Command::new(bin).arg("--version").output().await?;
                    let v: String = String::from_utf8_lossy(&[o.stdout, o.stderr].concat()).trim().chars().take(120).collect();
                    let tidal = if streamrip::tidal_signed_in(&st) { "Tidal: signed in" } else { "Tidal: not signed in yet – use “Sign in to Tidal” below" };
                    msg(o.status.success(), format!("{v} · credentials written to {} · {tidal}", path.display()))
                }
            },
            "acoustid" => {
                if !acoustid::available(&st) {
                    msg(false, "Needs an API key and the fpcalc tool (included in the Docker image)")
                } else {
                    let d: Value = st.http.get("https://api.acoustid.org/v2/lookup").query(&[("client", st.cfg.str("acoustid_api_key")), ("trackid", "9ff43b6a-4f16-427c-93c2-92307ca505e0".into())])
                        .send().await?.json().await?;
                    let ok = d["status"] == "ok";
                    msg(ok, if ok { "AcoustID key valid".to_string() } else { d["error"]["message"].as_str().unwrap_or("invalid key").to_string() })
                }
            }
            _ => return Err(anyhow::anyhow!("__unknown")),
        })
    }.await;
    match res {
        Ok(v) => Ok(Json(v)),
        Err(e) if e.to_string() == "__unknown" => Err(err(404, "Unknown service")),
        Err(e) => Ok(Json(msg(false, e.to_string().chars().take(400).collect::<String>()))),
    }
}

async fn downloads(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let mut out = json!({ "slskd": [], "lidarr": [], "torrents": [], "errors": {} });
    if slskd::enabled(&st) {
        match slskd::all_downloads(&st).await {
            Ok(v) => out["slskd"] = json!(v),
            Err(e) => out["errors"]["slskd"] = json!(e.to_string().chars().take(200).collect::<String>()),
        }
    }
    // torrent / Usenet downloads Encore is waiting on
    out["torrents"] = json!(st.db.query("SELECT id, artist, title, progress, message FROM requests WHERE source='torznab' AND status IN ('downloading','importing') ORDER BY id", p![])?);
    Ok(Json(out))
}

async fn clear_slskd(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    slskd::clear_completed(&st).await.map_err(|e| err(502, e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ProbeQ {
    url: String,
}

async fn probe(User(_): User, Query(q): Query<ProbeQ>) -> ApiResult<Json<Value>> {
    let mut info = ytdlp::probe(&q.url).await.map_err(|e| err(400, format!("Unsupported or unreachable URL: {}", e.to_string().chars().take(200).collect::<String>())))?;
    info["source"] = json!("ytdlp");
    Ok(Json(info))
}
