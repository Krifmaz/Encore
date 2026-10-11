//! /api/stats (home numbers, leaderboard), /api/system/diagnostics, /api/admin/autodownload and /api/admin/tasks.
use crate::auth::{Admin, User};
use crate::db::{now, Row, RowExt};
use crate::error::{err, ApiResult};
use crate::sources::streamrip;
use crate::state::AppState;
use crate::{p, scheduler};
use axum::extract::{Path, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/stats/me", get(me))
        .route("/api/stats/leaderboard", get(leaderboard))
        .route("/api/system/diagnostics", get(diagnostics))
        .route("/api/admin/autodownload", get(get_auto).put(put_auto))
        .route("/api/admin/autodownload/upgrade-now", post(upgrade_now))
        .route("/api/admin/autodownload/run-now", post(run_sync_now))
        .route("/api/admin/tasks", get(tasks))
        .route("/api/admin/tasks/{key}", patch(patch_task))
        .route("/api/admin/tasks/{key}/run", post(run_task))
}

// ---- stats --------------------------------------------------------------------------------------

fn ranking(st: &AppState) -> Vec<Row> {
    st.db.query("SELECT u.id, u.username,
        (SELECT COUNT(*) FROM requests r WHERE r.user_id=u.id AND r.parent_id IS NULL) requests,
        (SELECT COUNT(*) FROM requests r WHERE r.user_id=u.id AND r.status='completed') completed,
        (SELECT COUNT(*) FROM plays p WHERE p.user_id=u.id) plays,
        (SELECT COALESCE(SUM(seconds),0) FROM plays p WHERE p.user_id=u.id) listened,
        (SELECT COUNT(*) FROM playlist_items i JOIN playlists pl ON pl.id=i.playlist_id WHERE pl.user_id=u.id AND pl.kind='liked') likes,
        (SELECT COUNT(*) FROM playlists pl WHERE pl.user_id=u.id AND pl.kind!='liked') playlists
        FROM users u WHERE u.disabled=0", p![]).unwrap_or_default()
}

fn dur(s: f64) -> String {
    let s = s as i64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn facts(st: &AppState) -> Vec<Value> {
    let one = |sql: &str| st.db.one(sql, p![]).ok().flatten();
    let Some(lib) = one("SELECT COUNT(*) n, COALESCE(SUM(duration),0) d, COALESCE(SUM(size),0) s FROM tracks").filter(|l| l.i("n") > 0) else { return vec![] };
    let mut out = vec![];
    if let Some(t) = one("SELECT title, artist, duration FROM tracks ORDER BY duration DESC LIMIT 1") {
        out.push(json!({ "label": format!("longest song – {}: {}", t.s("artist"), t.s("title")), "value": dur(t.f("duration")) }));
    }
    if let Some(t) = one("SELECT title, artist, duration FROM tracks WHERE duration > 0 ORDER BY duration LIMIT 1") {
        out.push(json!({ "label": format!("shortest song – {}: {}", t.s("artist"), t.s("title")), "value": dur(t.f("duration")) }));
    }
    if let Some(t) = one("SELECT album, albumartist, year FROM tracks WHERE year GLOB '[12][0-9][0-9][0-9]*' ORDER BY year LIMIT 1") {
        out.push(json!({ "label": format!("oldest release – {}: {}", t.s("albumartist"), t.s("album")), "value": t.s("year").chars().take(4).collect::<String>() }));
    }
    let lossless = one("SELECT ROUND(100.0 * SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) / COUNT(*)) p FROM tracks").map(|r| r.f("p")).unwrap_or(0.0);
    out.push(json!({ "label": "of the library is lossless", "value": format!("{}%", lossless as i64) }));
    if let Some(d) = one("SELECT substr(year,1,3) || '0s' dec, COUNT(*) n FROM tracks WHERE year GLOB '[12][0-9][0-9][0-9]*' GROUP BY dec ORDER BY n DESC LIMIT 1") {
        out.push(json!({ "label": "is the library's favourite decade", "value": d.s("dec") }));
    }
    let love = one("SELECT COUNT(*) n FROM tracks WHERE lower(title) LIKE '%love%'").map(|r| r.i("n")).unwrap_or(0);
    out.push(json!({ "label": "songs with “love” in the title", "value": love.to_string() }));
    let floppies = (lib.f("s") / 1_474_560.0).round() as i64;
    out.push(json!({ "label": "floppy disks to hold the library", "value": floppies.to_string().as_bytes().rchunks(3).rev().map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(",") }));
    out.push(json!({ "label": "times through the library to listen to a year non-stop", "value": format!("{:.1}", 31_536_000.0 / lib.f("d").max(1.0)) }));
    out
}

async fn me(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let uid = u.i("id");
    let lib = st.db.one("SELECT COUNT(*) tracks, COALESCE(SUM(duration),0) duration, COUNT(DISTINCT albumartist) artists FROM tracks", p![])?.unwrap_or_default();
    let pl = st.db.one("SELECT COUNT(*) plays, COALESCE(SUM(seconds),0) listened FROM plays WHERE user_id=?", p![uid])?.unwrap_or_default();
    let top = st.db.one("SELECT artist, COUNT(*) n FROM plays WHERE user_id=? GROUP BY lower(artist) ORDER BY n DESC LIMIT 1", p![uid])?;
    let mut rank = ranking(&st);
    rank.sort_by_key(|r| -(r.i("requests") + r.i("plays")));
    let pos = rank.iter().position(|r| r.i("id") == uid).map(|i| i + 1).filter(|_| rank.len() > 1);
    let reqs = st.db.one("SELECT COUNT(*) n FROM requests WHERE user_id=? AND parent_id IS NULL", p![uid])?.map(|r| r.i("n")).unwrap_or(0);
    let f = facts(&st);
    let fun = if f.is_empty() { Value::Null } else { f[rand::random_range(0..f.len())].clone() };
    let mut mev = Value::Object(pl);
    mev["requests"] = json!(reqs);
    mev["rank"] = json!(pos);
    mev["top_artist"] = top.map(|t| json!(t.s("artist"))).unwrap_or(Value::Null);
    Ok(Json(json!({ "library": lib, "me": mev, "fun": fun })))
}

async fn leaderboard(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let users = ranking(&st);
    let mut boards = serde_json::Map::new();
    for (key, label) in [("requests", "Most requests"), ("plays", "Most songs played"), ("listened", "Most time listening"), ("likes", "Most liked songs"), ("playlists", "Playlist builders")] {
        let mut rows: Vec<&Row> = users.iter().filter(|r| r.f(key) > 0.0).collect();
        rows.sort_by(|a, b| b.f(key).partial_cmp(&a.f(key)).unwrap_or(std::cmp::Ordering::Equal));
        boards.insert(key.into(), json!({ "label": label, "rows": rows.iter().take(10).map(|r| json!({ "username": r.s("username"), "value": r[key], "me": r.i("id") == u.i("id") })).collect::<Vec<_>>() }));
    }
    let week = now() - 7.0 * 86400.0;
    Ok(Json(json!({
        "boards": boards,
        "top_songs": st.db.query("SELECT p.title, p.artist, MAX(t.album_id) album_id, COUNT(*) plays FROM plays p LEFT JOIN tracks t ON t.id=p.track_id
                                  GROUP BY lower(p.artist), lower(p.title) ORDER BY plays DESC LIMIT 10", p![])?,
        "top_artists": st.db.query("SELECT artist, COUNT(*) plays, COUNT(DISTINCT user_id) listeners FROM plays GROUP BY lower(artist) ORDER BY plays DESC LIMIT 10", p![])?,
        "this_week": st.db.query("SELECT u.username, COUNT(*) plays FROM plays p JOIN users u ON u.id=p.user_id WHERE p.played_at > ? GROUP BY p.user_id ORDER BY plays DESC LIMIT 5", p![week])?,
        "library_artists": st.db.query("SELECT MAX(albumartist) artist, COUNT(DISTINCT album_id) albums, COUNT(*) songs FROM tracks
                                        WHERE albumartist NOT IN ('', 'Unknown Artist', 'Various Artists') GROUP BY lower(albumartist) ORDER BY songs DESC LIMIT 10", p![])?,
        "facts": facts(&st),
    })))
}

// ---- diagnostics --------------------------------------------------------------------------------

fn redact(st: &AppState, text: &str) -> String {
    let mut t = text.to_string();
    for (k, d) in st.cfg.defs.iter() {
        if d.secret {
            let v = st.cfg.str(k);
            if v.len() >= 4 {
                t = t.replace(&v, "•••");
            }
        }
    }
    t
}

async fn http_check(st: &AppState, name: &str, url: String, headers: Vec<(&str, String)>, ok: &[u16]) -> Value {
    let mut out = json!({ "name": name, "kind": "service", "request": format!("GET {}", redact(st, &url)) });
    let t0 = Instant::now();
    let mut rb = st.http.get(&url).timeout(std::time::Duration::from_secs(10));
    for (k, v) in headers {
        rb = rb.header(k, v);
    }
    match rb.send().await {
        Ok(r) => {
            let code = r.status().as_u16();
            let body: String = r.text().await.unwrap_or_default().chars().take(600).collect();
            let good = ok.contains(&code);
            out["ok"] = json!(good);
            out["status"] = json!(code);
            out["body"] = json!(redact(st, &body));
            out["detail"] = json!(if good { "OK".to_string() } else if code == 401 || code == 403 { format!("{name} refused the credentials ({code}) – check the API key/token") } else { format!("{name} answered {code}") });
        }
        Err(e) => {
            out["ok"] = json!(false);
            out["detail"] = json!(format!("Can't reach {name} at {} – is the address right and the container on the same network? ({e})", redact(st, &url)));
        }
    }
    out["ms"] = json!(t0.elapsed().as_millis() as i64);
    out
}

fn path_check(label: &str, path: &str, write: bool) -> Value {
    let out = |ok: bool, d: String| json!({ "name": label, "kind": "path", "request": path, "ok": ok, "detail": d });
    if path.is_empty() {
        return out(false, "Not set".into());
    }
    let pth = std::path::Path::new(path);
    if !pth.is_dir() {
        return out(false, format!("{path} doesn't exist inside the container – check the volume mapping in your compose file"));
    }
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(pth).ok();
    let owner = meta.map(|m| format!("owned by uid {}/gid {}", m.uid(), m.gid())).unwrap_or_default();
    let ids = std::process::Command::new("id").args(["-u"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let gid = std::process::Command::new("id").args(["-g"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if write {
        let probe = pth.join(format!(".encore-write-test-{}", std::process::id()));
        if let Err(e) = std::fs::write(&probe, "x") {
            return out(false, format!("Not writable by Encore (uid {ids}/gid {gid}); folder is {owner}. Fix with: chown -R {ids}:{gid} <host folder> or set PUID/PGID ({e})"));
        }
        let _ = std::fs::remove_file(probe);
    }
    let n = std::fs::read_dir(pth).map(|r| r.count()).unwrap_or(0);
    out(true, format!("OK – {n} entries, {owner}, Encore runs as uid {ids}/gid {gid}"))
}

async fn tool(name: &str, cmd: &str, args: &[&str]) -> Value {
    let o = tokio::process::Command::new(cmd).args(args).output().await;
    let (ok, d) = match o {
        Ok(o) => {
            let text = String::from_utf8_lossy(if o.stdout.is_empty() { &o.stderr } else { &o.stdout }).to_string();
            (o.status.success(), text.lines().next().unwrap_or("").chars().take(160).collect::<String>())
        }
        Err(_) => (false, "Not installed".into()),
    };
    json!({ "name": name, "kind": "tool", "request": format!("{cmd} {}", args.join(" ")), "ok": ok, "detail": d })
}

async fn diagnostics(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let g = |k: &str| st.cfg.str(k).trim_end_matches('/').to_string();
    let mbu = if g("musicbrainz_url").is_empty() { "https://musicbrainz.org".into() } else { g("musicbrainz_url") };
    let mut futs: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + '_>>> = vec![
        Box::pin(http_check(&st, "MusicBrainz", format!("{mbu}/ws/2/artist?query=test&limit=1&fmt=json"), vec![], &[200])),
    ];
    if !g("prowlarr_url").is_empty() {
        futs.push(Box::pin(http_check(&st, "Prowlarr", format!("{}/api/v1/system/status", g("prowlarr_url")), vec![("X-Api-Key", st.cfg.str("prowlarr_api_key"))], &[200])));
    }
    if !g("qbit_url").is_empty() {
        futs.push(Box::pin(http_check(&st, "qBittorrent", format!("{}/api/v2/app/version", g("qbit_url")), vec![], &[200, 403])));
    }
    if !g("sab_url").is_empty() {
        futs.push(Box::pin(http_check(&st, "SABnzbd", format!("{}/api?mode=version&output=json&apikey={}", g("sab_url"), st.cfg.str("sab_api_key")), vec![], &[200])));
    }
    if !g("slskd_url").is_empty() {
        futs.push(Box::pin(http_check(&st, "slskd", format!("{}/api/v0/application", g("slskd_url")), vec![("X-API-Key", st.cfg.str("slskd_api_key"))], &[200])));
    }
    if !g("jellyfin_url").is_empty() {
        futs.push(Box::pin(http_check(&st, "Jellyfin", format!("{}/System/Info/Public", g("jellyfin_url")), vec![], &[200])));
    }
    if !g("plex_url").is_empty() {
        futs.push(Box::pin(http_check(&st, "Plex", format!("{}/identity", g("plex_url")), vec![("Accept", "application/json".into()), ("X-Plex-Token", st.cfg.str("plex_token"))], &[200])));
    }
    if !g("navidrome_url").is_empty() {
        futs.push(Box::pin(http_check(&st, "Navidrome", format!("{}/rest/ping?v=1.16.1&c=encore&f=json", g("navidrome_url")), vec![], &[200])));
    }
    if !st.cfg.str("acoustid_api_key").is_empty() {
        futs.push(Box::pin(http_check(&st, "AcoustID", format!("https://api.acoustid.org/v2/lookup?client={}&trackid=9ff43b6a-4f16-427c-93c2-92307ca505e0", st.cfg.str("acoustid_api_key")), vec![], &[200])));
    }
    futs.push(Box::pin(http_check(&st, "Cover Art Archive", "https://coverartarchive.org/".into(), vec![], &[200, 301, 302, 307, 308])));
    futs.push(Box::pin(http_check(&st, "YouTube Music", "https://music.youtube.com/".into(), vec![], &[200, 301, 302, 303])));
    let mut checks: Vec<Value> = futures_util::future::join_all(futs).await;
    checks.push(path_check("Music library", &st.cfg.str("library_path"), true));
    checks.push(path_check("Staging folder", &st.cfg.str("staging_path"), true));
    if st.cfg.bool("slskd_enabled") || !g("slskd_url").is_empty() {
        checks.push(path_check("slskd downloads (as seen by Encore)", &st.cfg.str("slskd_download_path"), true));
    }
    if torznab_on(&st) {
        checks.push(path_check("Torrent downloads (as seen by Encore)", &st.cfg.str("qbit_download_path"), true));
    }
    checks.push(path_check("Config folder", &st.data_dir().to_string_lossy(), true));
    checks.push(tool("ffmpeg", "ffmpeg", &["-version"]).await);
    checks.push(tool("fpcalc (audio fingerprints)", "fpcalc", &["-version"]).await);
    checks.push(tool("yt-dlp", &crate::sources::ytdlp::bin(), &["--version"]).await);
    checks.push(tool("streamrip", &streamrip::binary(&st).unwrap_or("streamrip".into()), &["--version"]).await);
    let dump = st.cfg.public_dump();
    let mut settings = vec![];
    for (k, d) in st.cfg.defs.iter() {
        let src = dump["_meta"][k]["source"].as_str().unwrap_or("default");
        if src == "default" || k == "api_key" || k == "plex_client_id" {
            continue;
        }
        settings.push(json!({ "key": k, "source": src, "env": if src == "env" { json!(d.env) } else { Value::Null }, "value": dump[k] }));
    }
    let uid = std::process::Command::new("id").arg("-u").output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().ok());
    Ok(Json(json!({ "version": env!("CARGO_PKG_VERSION"), "server": "rust", "uid": uid, "checks": checks, "settings": settings, "time": now() })))
}

fn torznab_on(st: &AppState) -> bool {
    st.cfg.bool("torznab_enabled") && !st.cfg.str("qbit_url").is_empty()
}

// ---- auto-download (admin) ----------------------------------------------------------------------

const AUTO_KEYS: [&str; 13] = ["auto_request_followed", "auto_request_likes", "auto_request_follows", "follow_check_hours", "album_upgrade", "quality_first",
    "upgrade_lossy", "upgrade_lossy_days", "sync_interval_minutes", "auto_approve", "source_order", "discover_auto_count", "discover_catalog_merge"];

async fn get_auto(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let mut stats = st.db.one("SELECT
        SUM(CASE WHEN detail LIKE '%\"auto\": true%' AND status IN ('queued','searching','downloading','monitoring','importing') THEN 1 ELSE 0 END) auto_active,
        SUM(CASE WHEN source='album' AND status='monitoring' THEN 1 ELSE 0 END) album_waiting,
        SUM(CASE WHEN kind='album' AND detail LIKE '%lossless_only%' AND status IN ('queued','searching','downloading','importing') THEN 1 ELSE 0 END) upgrades_active,
        SUM(CASE WHEN kind='album' AND detail LIKE '%lossless_only%' AND status='completed' THEN 1 ELSE 0 END) upgrades_done
        FROM requests", p![])?.unwrap_or_default();
    let lossy = st.db.one("SELECT COUNT(*) n FROM (SELECT album_id FROM tracks GROUP BY album_id HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = 0)", p![])?
        .map(|r| r.i("n")).unwrap_or(0);
    stats.insert("lossy_albums".into(), json!(lossy));
    let users = st.db.query("SELECT u.id, u.username, u.auto_download, u.role,
        (SELECT COUNT(*) FROM playlists p WHERE p.user_id=u.id AND p.follow=1) followed,
        (SELECT COUNT(*) FROM connections c WHERE c.user_id=u.id) connections,
        (SELECT COUNT(*) FROM follows f WHERE f.user_id=u.id) follows
        FROM users u WHERE u.disabled=0 ORDER BY u.username", p![])?;
    let settings: serde_json::Map<String, Value> = AUTO_KEYS.iter().map(|k| (k.to_string(), st.cfg.get(k))).collect();
    Ok(Json(json!({ "settings": settings, "users": users, "stats": stats })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AutoBody {
    settings: serde_json::Map<String, Value>,
    users: serde_json::Map<String, Value>,
}

async fn put_auto(State(st): State<AppState>, a: Admin, Json(b): Json<AutoBody>) -> ApiResult<Json<Value>> {
    let keep: serde_json::Map<String, Value> = b.settings.into_iter().filter(|(k, _)| AUTO_KEYS.contains(&k.as_str())).collect();
    st.cfg.set_many(&Value::Object(keep));
    for (uid, on) in b.users {
        st.db.execute("UPDATE users SET auto_download=? WHERE id=?", p![on.as_bool().unwrap_or(false) as i64, uid.parse::<i64>().unwrap_or(0)])?;
    }
    get_auto(State(st), a).await
}

async fn upgrade_now(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "queued": scheduler::queue_lossy_upgrades(&st, 50).map_err(|e| err(500, e.to_string()))? })))
}

async fn run_sync_now(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    tokio::spawn(crate::sync::sync_all(st));
    Ok(Json(json!({ "ok": true })))
}

// ---- scheduled tasks ----------------------------------------------------------------------------

async fn tasks(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "tasks": scheduler::listing(&st), "cleanup_days": st.cfg.get("task_cleanup_days") })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TaskPatch {
    enabled: Option<bool>,
    minutes: Option<i64>,
}

async fn patch_task(State(st): State<AppState>, Admin(_): Admin, Path(key): Path<String>, Json(b): Json<TaskPatch>) -> ApiResult<Json<Value>> {
    if !scheduler::TASKS.iter().any(|t| t.0 == key) {
        return Err(err(404, "No such task"));
    }
    scheduler::update(&st, &key, b.enabled, b.minutes);
    Ok(Json(scheduler::listing(&st).into_iter().find(|t| t["key"] == key.as_str()).unwrap_or_default()))
}

async fn run_task(State(st): State<AppState>, Admin(_): Admin, Path(key): Path<String>) -> ApiResult<Json<Value>> {
    if !scheduler::TASKS.iter().any(|t| t.0 == key) {
        return Err(err(404, "No such task"));
    }
    if !scheduler::run_now(&st, &key) {
        return Err(err(409, "Already running"));
    }
    Ok(Json(json!({ "ok": true })))
}
