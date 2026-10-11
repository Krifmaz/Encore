//! /api/system: service status, connection tests, downloader queues; /api/discover/probe.
use crate::auth::{Admin, User};
use crate::db::RowExt;
use crate::error::{err, ApiResult};
use crate::sources::{hifi, slskd, streamrip, torznab, ytdlp};
use crate::state::AppState;
use crate::{acoustid, email, notify, p};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/system/status", get(status))
        .route("/api/system/test/{service}", post(test))
        .route("/api/system/downloads", get(downloads))
        .route("/api/system/downloads/slskd/clear", post(clear_slskd))
        .route("/api/discover/probe", get(probe))
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
        "hifi": { "ok": hifi::enabled(&st), "enabled": st.cfg.bool("hifi_enabled") },
        "ffmpeg": { "ok": which("ffmpeg") },
    });
    Ok(Json(json!({ "version": env!("CARGO_PKG_VERSION"), "server": "rust", "services": services, "disk": disk(&st.cfg.str("library_path")), "app_name": st.cfg.str("app_name") })))
}

fn msg(ok: bool, m: impl Into<String>) -> Value {
    json!({ "ok": ok, "message": m.into() })
}

async fn test(State(st): State<AppState>, Admin(u): Admin, Path(service): Path<String>) -> ApiResult<Json<Value>> {
    let res: anyhow::Result<Value> = async {
        Ok(match service.as_str() {
            "slskd" => {
                let s = slskd::status(&st).await?;
                msg(true, format!("slskd {} – server {}", s["version"]["full"].as_str().unwrap_or(""), s["server"]["state"].as_str().unwrap_or("unknown")))
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
            "notify" => {
                notify::send(&st, "🔔 Test notification", &format!("{} notifications are working", st.cfg.str("app_name")), None).await;
                msg(true, "Sent")
            }
            "mediaservers" => {
                let done = notify::refresh_media_servers(&st).await;
                msg(!done.is_empty(), if done.is_empty() { "No media servers configured".into() } else { format!("Refreshed {}", done.join(", ")) })
            }
            "spotify" => {
                let (cid, sec) = (st.cfg.str("spotify_client_id"), st.cfg.str("spotify_client_secret"));
                if cid.is_empty() || sec.is_empty() {
                    return Ok(msg(false, "Enter both the Spotify Client ID and Client secret, then test again"));
                }
                let ok = st.http.post("https://accounts.spotify.com/api/token").basic_auth(cid, Some(sec)).form(&[("grant_type", "client_credentials")])
                    .send().await.map(|r| r.status().is_success()).unwrap_or(false);
                msg(ok, if ok { "Spotify credentials valid" } else { "Spotify rejected the Client ID/Secret" })
            }
            "ytdlp" | "youtube" => ytdlp::test_cookies(&st).await?,
            "tidal" => {
                streamrip::tidal_token(&st).await?;
                msg(true, "Tidal credentials valid")
            }
            "hifi" => {
                if hifi::endpoints(&st).is_empty() { msg(false, "Enter the URL of your HiFi API instance") } else { hifi::status(&st).await }
            }
            "streamrip" => match streamrip::binary(&st) {
                None => msg(false, "streamrip is not installed in this container (use the default Docker image or set its path)"),
                Some(bin) => {
                    let path = streamrip::apply_credentials(&st).await?;
                    let o = tokio::process::Command::new(bin).arg("--version").output().await?;
                    let v: String = String::from_utf8_lossy(&[o.stdout, o.stderr].concat()).trim().chars().take(120).collect();
                    msg(o.status.success(), format!("{v} · credentials written to {}. For Tidal, sign in once from a terminal: docker exec -it encore streamrip --config-path /config/streamrip/config.toml url <any tidal link>", path.display()))
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
