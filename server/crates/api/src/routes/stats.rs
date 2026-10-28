//! /api/stats (home numbers, leaderboard), /api/system/diagnostics, /api/admin/autodownload and /api/admin/tasks.
use crate::enrich::enrich_now;
use crate::auth::{Admin, User};
use encore_db::{now, Row, RowExt};
use encore_core::error::{err, ApiResult};
use encore_requests::sources::dl::Client;
use encore_requests::sources::streamrip;
use crate::state::AppState;
use crate::scheduler;
use encore_db::p;
use axum::extract::{Path, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

/// The stats, diagnostics, auto-download and scheduled task routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/stats/me", get(me))
        .route("/api/stats/leaderboard", get(leaderboard))
        .route("/api/system/diagnostics", get(diagnostics))
        .route("/api/diagnostics/client-crash", get(client_crash::list).post(client_crash::upload).delete(client_crash::clear))
        .route("/api/admin/autodownload", get(get_auto).put(put_auto))
        .route("/api/admin/autodownload/upgrade-now", post(upgrade_now))
        .route("/api/admin/autodownload/run-now", post(run_sync_now))
        .route("/api/admin/tasks", get(tasks))
        .route("/api/admin/tasks/{key}", patch(patch_task))
        .route("/api/admin/tasks/{key}/run", post(run_task))
}

#[path = "client_crash.rs"]
mod client_crash;

#[cfg(test)]
#[path = "stats_tests.rs"]
mod stats_tests;

// ---- stats --------------------------------------------------------------------------------------

/// Everyone's totals (requests, plays, time listened, likes, playlists): subqueries over every request, play
/// and playlist, so the answer is reused until one of those is written (`Db::stats_version` moves), and for
/// at most `STATS_TTL` seconds. A like, a new playlist, a request or a play shows on the boards and in the
/// rank on Home at once, as it always did.
fn ranking(st: &AppState) -> Vec<Row> {
    // read before the totals: a write that lands while they are counted makes the next call count again
    let version = st.db.stats_version();
    if let Some(Value::Object(mut cached)) = st.cache_get("stats:ranking") {
        if cached.get("v").and_then(Value::as_u64) == Some(version) {
            if let Some(Value::Array(rows)) = cached.remove("rows") {
                return rows.into_iter().filter_map(|r| match r { Value::Object(o) => Some(o), _ => None }).collect();
            }
        }
    }
    let rows = ranking_now(st);
    st.cache_put("stats:ranking", json!({ "v": version, "rows": rows }), STATS_TTL);
    rows
}

/// Every user's request counts for the leaderboard, worked out now.
fn ranking_now(st: &AppState) -> Vec<Row> {
    st.db.query("SELECT u.id, u.username,
        (SELECT COUNT(*) FROM requests r WHERE r.user_id=u.id AND r.parent_id IS NULL) requests,
        (SELECT COUNT(*) FROM requests r WHERE r.user_id=u.id AND r.status='completed') completed,
        (SELECT COUNT(*) FROM plays p WHERE p.user_id=u.id) plays,
        (SELECT COALESCE(SUM(seconds),0) FROM plays p WHERE p.user_id=u.id) listened,
        (SELECT COUNT(*) FROM playlist_items i JOIN playlists pl ON pl.id=i.playlist_id WHERE pl.user_id=u.id AND pl.kind='liked') likes,
        (SELECT COUNT(*) FROM playlists pl WHERE pl.user_id=u.id AND pl.kind='playlist') playlists
        FROM users u WHERE u.disabled=0", p![]).unwrap_or_default()
}

/// Seconds as "m:ss", or "h:mm:ss" from an hour.
fn dur(s: f64) -> String {
    let s = s as i64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Seconds the library totals and facts are reused for.
const STATS_TTL: u64 = 60;

/// The library facts, cached for [`STATS_TTL`].
fn cached_facts(st: &AppState) -> Vec<Value> {
    if let Some(Value::Array(v)) = st.cache_get("stats:facts") {
        return v;
    }
    let v = facts(st);
    st.cache_put("stats:facts", json!(v), STATS_TTL);
    v
}

/// Facts about the library: its size, its length, its biggest artists and the like.
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
    out.push(json!({ "label": "floppy disks to hold the library", "value": thousands(floppies) }));
    out.push(json!({ "label": "times through the library to listen to a year non-stop", "value": times_through(31_536_000.0 / lib.f("d").max(1.0)) }));
    out
}

/// 1234567 → "1,234,567".
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let grouped = digits.as_bytes().rchunks(3).rev().map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join(",");
    if n < 0 { format!("-{grouped}") } else { grouped }
}

/// How many times a year of listening goes through the library: one decimal below 10 ("2.4"), a whole number
/// with thousands separators above ("157,680" for a small library, not "157680.0").
fn times_through(x: f64) -> String {
    if x < 10.0 { format!("{x:.1}") } else { thousands(x.round() as i64) }
}

/// One listener's own numbers on Home (plays, time listened, top artist, requests): the plays and requests
/// of that user, read again only after a request, play, like or playlist was written (`Db::stats_version`),
/// and at most `STATS_TTL` seconds apart.
fn my_numbers(st: &AppState, uid: i64) -> ApiResult<Value> {
    // read before the counts, as `ranking` does: a write meanwhile makes the next call count again
    let version = st.db.stats_version();
    let key = format!("stats:me:{uid}");
    if let Some(Value::Object(mut cached)) = st.cache_get(&key) {
        if cached.get("v").and_then(Value::as_u64) == Some(version) {
            if let Some(v) = cached.remove("me") {
                return Ok(v);
            }
        }
    }
    let pl = st.db.one("SELECT COUNT(*) plays, COALESCE(SUM(seconds),0) listened FROM plays WHERE user_id=?", p![uid])?.unwrap_or_default();
    let top = st.db.one("SELECT artist, COUNT(*) n FROM plays WHERE user_id=? GROUP BY lower(artist) ORDER BY n DESC LIMIT 1", p![uid])?;
    let reqs = st.db.one("SELECT COUNT(*) n FROM requests WHERE user_id=? AND parent_id IS NULL", p![uid])?.map(|r| r.i("n")).unwrap_or(0);
    let mut mev = Value::Object(pl);
    mev["requests"] = json!(reqs);
    mev["top_artist"] = top.map(|t| json!(t.s("artist"))).unwrap_or(Value::Null);
    st.cache_put(&key, json!({ "v": version, "me": mev }), STATS_TTL);
    Ok(mev)
}

/// Home's numbers: the library's totals, your own and your rank, and a fun fact. Every part reads whole
/// tables when its cache is cold, so it all runs on the blocking pool.
async fn me(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let uid = u.i("id");
        // the library's totals and fun facts read every song: once a minute is plenty for the home page
        let lib = match st.cache_get("stats:library") {
            Some(v) => v,
            None => {
                let v = json!(st.db.one("SELECT COUNT(*) tracks, COALESCE(SUM(duration),0) duration, COUNT(DISTINCT albumartist) artists FROM tracks", p![])?.unwrap_or_default());
                st.cache_put("stats:library", v.clone(), STATS_TTL);
                v
            }
        };
        let mine = my_numbers(st, uid)?;
        let mut rank = ranking(st);
        rank.sort_by_key(|r| -(r.i("requests") + r.i("plays")));
        let pos = rank.iter().position(|r| r.i("id") == uid).map(|i| i + 1).filter(|_| rank.len() > 1);
        let f = cached_facts(st);
        let fun = if f.is_empty() { Value::Null } else { f[rand::random_range(0..f.len())].clone() };
        let mut mev = mine;
        mev["rank"] = json!(pos);
        Ok(Json(json!({ "library": lib, "me": mev, "fun": fun })))
    }).await
}

/// GET /api/stats/leaderboard: the users ranked by requests and more.
async fn leaderboard(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let users = ranking(st);
        let mut boards = serde_json::Map::new();
        for (key, label) in [("requests", "Most requests"), ("plays", "Most songs played"), ("listened", "Most time listening"), ("likes", "Most liked songs"), ("playlists", "Playlist builders")] {
            let mut rows: Vec<&Row> = users.iter().filter(|r| r.f(key) > 0.0).collect();
            rows.sort_by(|a, b| b.f(key).partial_cmp(&a.f(key)).unwrap_or(std::cmp::Ordering::Equal));
            boards.insert(key.into(), json!({ "label": label, "rows": rows.iter().take(10).map(|r| json!({ "username": r.s("username"), "value": r[key], "me": r.i("id") == u.i("id") })).collect::<Vec<_>>() }));
        }
        // the same for everyone: read once a minute (plays and songs grouped over every row)
        let shared = match st.cache_get("stats:leaderboard") {
            Some(v) => v,
            None => {
                let week = now() - 7.0 * 86400.0;
                let v = json!({
                    "top_songs": st.db.query("SELECT p.title, p.artist, MAX(t.album_id) album_id, COUNT(*) plays FROM plays p LEFT JOIN tracks t ON t.id=p.track_id
                                              GROUP BY lower(p.artist), lower(p.title) ORDER BY plays DESC LIMIT 10", p![])?,
                    "top_artists": st.db.query("SELECT artist, COUNT(*) plays, COUNT(DISTINCT user_id) listeners FROM plays GROUP BY lower(artist) ORDER BY plays DESC LIMIT 10", p![])?,
                    "this_week": st.db.query("SELECT u.username, COUNT(*) plays FROM plays p JOIN users u ON u.id=p.user_id WHERE p.played_at > ? GROUP BY p.user_id ORDER BY plays DESC LIMIT 5", p![week])?,
                    "library_artists": st.db.query("SELECT MAX(albumartist) artist, COUNT(DISTINCT album_id) albums, COUNT(*) songs FROM tracks
                                                    WHERE albumartist NOT IN ('', 'Unknown Artist', 'Various Artists') GROUP BY lower(albumartist) ORDER BY songs DESC LIMIT 10", p![])?,
                });
                st.cache_put("stats:leaderboard", v.clone(), STATS_TTL);
                v
            }
        };
        let mut out = shared;
        out["boards"] = Value::Object(boards);
        out["facts"] = json!(cached_facts(st));
        enrich_now(st, Some(&u), &mut out);
        Ok(Json(out))
    }).await
}

// ---- diagnostics --------------------------------------------------------------------------------

/// `text` with every secret setting's value replaced.
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

/// Check a service with a GET: whether it answered with an `ok` status, how fast, and the redacted request.
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

/// Check a folder: set, there, a folder and (with `write`) writable.
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
    // what `id -u` / `id -g` print, without running them
    let (ids, gid) = (effective_uid().to_string(), effective_gid().to_string());
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

/// Encore's effective user id (what `id -u` prints).
fn effective_uid() -> u32 {
    // SAFETY: only reads the process's credentials; it cannot fail
    unsafe { libc::geteuid() }
}

/// Encore's effective group id (what `id -g` prints).
fn effective_gid() -> u32 {
    // SAFETY: only reads the process's credentials; it cannot fail
    unsafe { libc::getegid() }
}

/// A tool's line in Diagnostics: its version line, kept by the tools card's probe while its program file is
/// the same (the version line of an unchanged program doesn't change, and asking it, a Python start for
/// yt-dlp and streamrip, took most of the page's time) and asked again in the background after 10 minutes.
async fn tool(st: &AppState, name: &str, cmd: &str, arg: &'static str) -> Value {
    let (tools, c) = (st.system.tools.clone(), cmd.to_string());
    let (ok, d) = tokio::task::spawn_blocking(move || tools.version_line(&c, arg)).await.unwrap_or_else(|e| (false, e.to_string()));
    json!({ "name": name, "kind": "tool", "request": format!("{cmd} {arg}"), "ok": ok, "detail": d })
}

/// GET /api/system/diagnostics: every service and folder checked at once (admins).
async fn diagnostics(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let g = |k: &str| st.cfg.str(k).trim_end_matches('/').to_string();
    let mbu = if g("musicbrainz_url").is_empty() { "https://musicbrainz.org".into() } else { g("musicbrainz_url") };
    let mut futs: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + '_>>> = vec![
        Box::pin(http_check(&st, "MusicBrainz", format!("{mbu}/ws/2/artist?query=test&limit=1&fmt=json"), vec![], &[200])),
    ];
    if !g("prowlarr_url").is_empty() {
        futs.push(Box::pin(http_check(&st, "Prowlarr", format!("{}/api/v1/system/status", g("prowlarr_url")), vec![("X-Api-Key", st.cfg.str("prowlarr_api_key"))], &[200])));
    }
    // the chosen download clients (a blackhole folder is checked as a path below)
    let basic = |u: &str, p: &str| ("Authorization", format!("Basic {}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("{}:{}", st.cfg.str(u), st.cfg.str(p)))));
    match Client::torrent(&st) {
        Client::Qbit if !g("qbit_url").is_empty() => futs.push(Box::pin(http_check(&st, "qBittorrent", format!("{}/api/v2/app/version", g("qbit_url")), vec![], &[200, 403]))),
        // without a session id Transmission answers 409, which proves the address and the login
        Client::Transmission if !g("transmission_url").is_empty() => {
            let u = g("transmission_url");
            let u = if u.ends_with("/rpc") { u } else { format!("{u}/transmission/rpc") };
            futs.push(Box::pin(http_check(&st, "Transmission", u, vec![basic("transmission_user", "transmission_password")], &[200, 409])))
        }
        Client::Deluge if !g("deluge_url").is_empty() => futs.push(Box::pin(http_check(&st, "Deluge", g("deluge_url"), vec![], &[200]))),
        Client::Rtorrent if !g("rtorrent_url").is_empty() => futs.push(Box::pin(http_check(&st, "rTorrent", g("rtorrent_url"), vec![basic("rtorrent_user", "rtorrent_password")], &[200, 405]))),
        _ => {}
    }
    match Client::usenet(&st) {
        Client::Sab if !g("sab_url").is_empty() => futs.push(Box::pin(http_check(&st, "SABnzbd", format!("{}/api?mode=version&output=json&apikey={}", g("sab_url"), st.cfg.str("sab_api_key")), vec![], &[200]))),
        Client::NzbGet if !g("nzbget_url").is_empty() => {
            let u = g("nzbget_url");
            let u = if u.ends_with("/jsonrpc") { format!("{u}/version") } else { format!("{u}/jsonrpc/version") };
            futs.push(Box::pin(http_check(&st, "NZBGet", u, vec![basic("nzbget_user", "nzbget_password")], &[200])))
        }
        _ => {}
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
        futs.push(Box::pin(http_check(&st, "AcoustID", encore_http::acoustid::check_key_url(&st.cfg.str("acoustid_api_key")), vec![], &[200])));
    }
    futs.push(Box::pin(http_check(&st, "Cover Art Archive", format!("{}/", encore_http::coverartarchive::BASE), vec![], &[200, 301, 302, 307, 308])));
    futs.push(Box::pin(http_check(&st, "YouTube Music", "https://music.youtube.com/".into(), vec![], &[200, 301, 302, 303])));
    let mut checks: Vec<Value> = futures_util::future::join_all(futs).await;
    // the Soulseek session as Encore keeps it (no call: the session's supervisor reads slskd all the time);
    // the card's sentence, pointing at the card ("fix them in Settings → Plugins → Soulseek", not "below")
    if encore_requests::slskd_managed::configured(&st) {
        let s = encore_requests::slskd_session::status(&st);
        let detail = encore_requests::slskd_session::text_elsewhere(s["text"].as_str().unwrap_or(""));
        checks.push(json!({ "name": "Soulseek", "kind": "service", "request": "GET /api/system/soulseek", "ok": s["state"] == "logged_in",
            "detail": detail, "soulseek": s }));
    }
    // the folders: a write test in each (on the blocking pool; these are checks of the folders as they are now)
    let mut paths: Vec<(&str, String)> = vec![("Music library", st.cfg.str("library_path")), ("Staging folder", st.cfg.str("staging_path"))];
    if st.cfg.bool("slskd_enabled") {
        paths.push(("Soulseek downloads", st.cfg.str("slskd_download_path")));
    }
    if torznab_on(&st) {
        let c = Client::torrent(&st);
        if c == Client::TorrentHole {
            paths.push(("Torrent blackhole watch folder", st.cfg.str("torrent_blackhole_watch")));
        }
        paths.push(("Torrent downloads (as seen by Encore)", st.cfg.str(c.download_path_key())));
    }
    if st.cfg.bool("torznab_enabled") && Client::usenet(&st).ready(&st) && Client::usenet(&st) != Client::Sab {
        let c = Client::usenet(&st);
        if c == Client::NzbHole {
            paths.push(("Usenet blackhole watch folder", st.cfg.str("nzb_blackhole_watch")));
        }
        paths.push(("Usenet downloads (as seen by Encore)", st.cfg.str(c.download_path_key())));
    }
    paths.push(("Config folder", st.data_dir().to_string_lossy().to_string()));
    let owned: Vec<(String, String)> = paths.into_iter().map(|(l, p)| (l.to_string(), p)).collect();
    checks.extend(tokio::task::spawn_blocking(move || owned.iter().map(|(l, p)| path_check(l, p, true)).collect::<Vec<_>>()).await.unwrap_or_default());
    checks.push(cover_check(&st));
    checks.push(tool(&st, "ffmpeg", "ffmpeg", "-version").await);
    checks.push(tool(&st, "fpcalc (audio fingerprints)", "fpcalc", "-version").await);
    // which yt-dlp and streamrip run (the updated copy when it is newer: their versions are asked) on the
    // blocking pool
    let (ytdlp_bin, streamrip_bin) = st.blocking(|st| Ok((encore_requests::sources::ytdlp::bin(st), streamrip::binary(st)))).await?;
    checks.push(tool(&st, "yt-dlp", &ytdlp_bin, "--version").await);
    checks.push(tool(&st, "streamrip", streamrip_bin.as_deref().unwrap_or("streamrip"), "--version").await);
    if streamrip_bin.is_some() {
        checks.push(streamrip::config_loads_check(&st).await);
    }
    let dump = st.cfg.public_dump();
    let mut settings = vec![];
    for (k, d) in st.cfg.defs.iter() {
        let src = dump["_meta"][k]["source"].as_str().unwrap_or("default");
        if src == "default" || k == "api_key" || k == "plex_client_id" {
            continue;
        }
        settings.push(json!({ "key": k, "source": src, "env": if src == "env" { json!(d.env) } else { Value::Null }, "value": dump[k] }));
    }
    let uid = Some(effective_uid() as i64);
    // every folder downloads are saved in, with its last probe's result and time (the probe runs every minute)
    let folders = st.blocking(|st| Ok(encore_requests::worker::downloads::status(st, true))).await?;
    Ok(Json(json!({ "version": crate::VERSION, "server": "rust", "uid": uid, "checks": checks, "settings": settings, "machine": super::system::server_block(&st),
                    "download_folders": folders, "time": now() })))
}

/// How far the cover thumbnail pass is: "Cover thumbnails: 12 340 / 18 000".
fn cover_check(st: &AppState) -> Value {
    let (done, total, running) = st.library.covers.progress();
    let group = |n: usize| {
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(' ');
            }
            out.push(c);
        }
        out
    };
    let detail = if total == 0 && !running {
        "Not started yet".to_string()
    } else if running {
        format!("{} / {} albums looked at, making the rest now", group(done.min(total)), group(total))
    } else {
        format!("{} / {} albums looked at", group(done.min(total)), group(total))
    };
    json!({ "name": "Cover thumbnails", "kind": "path", "request": "covers/thumbs", "ok": true, "detail": detail, "done": done, "total": total, "running": running })
}

/// Whether torrents can be fetched: the indexers on and a ready torrent client.
fn torznab_on(st: &AppState) -> bool {
    st.cfg.bool("torznab_enabled") && Client::torrent(st).ready(st)
}

// ---- auto-download (admin) ----------------------------------------------------------------------

/// The settings the auto-download page may change.
const AUTO_KEYS: [&str; 12] = ["auto_request_followed", "auto_request_likes", "album_upgrade", "quality_first",
    "upgrade_lossy", "upgrade_hires", "upgrade_lossy_days", "sync_interval_minutes", "auto_approve", "source_order", "discover_auto_count", "discover_catalog_merge"];

/// GET /api/admin/autodownload: the auto-download settings and what they did.
async fn get_auto(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let mut stats = st.db.one("SELECT
        SUM(CASE WHEN detail LIKE '%\"auto\": true%' AND status IN ('queued','searching','downloading','monitoring','importing','waiting') THEN 1 ELSE 0 END) auto_active,
        SUM(CASE WHEN source='album' AND status='monitoring' THEN 1 ELSE 0 END) album_waiting,
        SUM(CASE WHEN kind='album' AND detail LIKE '%lossless_only%' AND status IN ('queued','searching','downloading','importing','waiting') THEN 1 ELSE 0 END) upgrades_active,
        SUM(CASE WHEN kind='album' AND detail LIKE '%lossless_only%' AND status='completed' THEN 1 ELSE 0 END) upgrades_done
        FROM requests", p![])?.unwrap_or_default();
    let lossy = st.db.one("SELECT COUNT(*) n FROM (SELECT album_id FROM tracks GROUP BY album_id HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = 0)", p![])?
        .map(|r| r.i("n")).unwrap_or(0);
    stats.insert("lossy_albums".into(), json!(lossy));
    // albums only in CD quality (16-bit FLAC): what a hi-res upgrade could improve
    stats.insert("cd_albums".into(), json!(encore_requests::upgrade::cd_quality_albums(&st).map_err(|e| err(500, e.to_string()))?));
    let users = st.db.query("SELECT u.id, u.username, u.auto_download, u.role,
        (SELECT COUNT(*) FROM playlists p WHERE p.user_id=u.id AND p.follow=1) followed,
        (SELECT COUNT(*) FROM connections c WHERE c.user_id=u.id) connections
        FROM users u WHERE u.disabled=0 ORDER BY u.username", p![])?;
    let settings: serde_json::Map<String, Value> = AUTO_KEYS.iter().map(|k| (k.to_string(), st.cfg.get(k))).collect();
    Ok(Json(json!({ "settings": settings, "users": users, "stats": stats })))
}

/// `PUT /api/admin/autodownload` body.
#[derive(Deserialize, Default)]
#[serde(default)]
struct AutoBody {
    /// settings to change (only [`AUTO_KEYS`])
    settings: serde_json::Map<String, Value>,
    /// auto-download on or off per user id
    users: serde_json::Map<String, Value>,
}

/// PUT /api/admin/autodownload: change the auto-download settings and users.
async fn put_auto(State(st): State<AppState>, a: Admin, Json(b): Json<AutoBody>) -> ApiResult<Json<Value>> {
    let keep: serde_json::Map<String, Value> = b.settings.into_iter().filter(|(k, _)| AUTO_KEYS.contains(&k.as_str())).collect();
    st.cfg.set_many(&Value::Object(keep));
    for (uid, on) in b.users {
        st.db.execute("UPDATE users SET auto_download=? WHERE id=?", p![on.as_bool().unwrap_or(false) as i64, uid.parse::<i64>().unwrap_or(0)])?;
    }
    get_auto(State(st), a).await
}

/// POST /api/admin/autodownload/upgrade-now: queue up to 50 lossless upgrades of lossy albums and up to 50
/// hi-res upgrades of CD-quality albums now (the hi-res ones only with streamrip set up). → `{queued, hires}`:
/// how many of each; `queued` counts both, for older apps that only show one number.
async fn upgrade_now(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let lossless = encore_requests::upkeep::queue_lossy_upgrades(&st, 50).map_err(|e| err(500, e.to_string()))?;
    let hires = encore_requests::upgrade::queue_hires_upgrades(&st, 50).map_err(|e| err(500, e.to_string()))?;
    Ok(Json(json!({ "queued": lossless + hires, "lossless": lossless, "hires": hires })))
}

/// POST /api/admin/autodownload/run-now: sync the followed artists and likes now.
async fn run_sync_now(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    tokio::spawn(encore_sync::sync_all(st));
    Ok(Json(json!({ "ok": true })))
}

// ---- scheduled tasks ----------------------------------------------------------------------------

/// GET /api/admin/tasks: the scheduled tasks.
async fn tasks(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "tasks": scheduler::listing(&st), "cleanup_days": st.cfg.get("task_cleanup_days") })))
}

/// `PATCH /api/admin/tasks/{key}` body.
#[derive(Deserialize, Default)]
#[serde(default)]
struct TaskPatch {
    /// switch it on or off
    enabled: Option<bool>,
    /// how often it runs (minutes)
    minutes: Option<i64>,
}

/// PATCH /api/admin/tasks/{key}: change a scheduled task.
async fn patch_task(State(st): State<AppState>, Admin(_): Admin, Path(key): Path<String>, Json(b): Json<TaskPatch>) -> ApiResult<Json<Value>> {
    if !encore_jobs::scheduler::TASKS.iter().any(|t| t.0 == key) {
        return Err(err(404, "No such task"));
    }
    scheduler::update(&st, &key, b.enabled, b.minutes);
    Ok(Json(scheduler::listing(&st).into_iter().find(|t| t["key"] == key.as_str()).unwrap_or_default()))
}

/// POST /api/admin/tasks/{key}/run: run a scheduled task now.
async fn run_task(State(st): State<AppState>, Admin(_): Admin, Path(key): Path<String>) -> ApiResult<Json<Value>> {
    if !encore_jobs::scheduler::TASKS.iter().any(|t| t.0 == key) {
        return Err(err(404, "No such task"));
    }
    if !encore_jobs::scheduler::run_now(&st, &key) {
        return Err(err(409, "Already running"));
    }
    Ok(Json(json!({ "ok": true })))
}
