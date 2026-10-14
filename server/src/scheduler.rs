//! Scheduled tasks: one place to switch background jobs on/off, set their intervals and run them now.
use crate::db::{now, RowExt};
use crate::routes::requests::{create_request, NewRequest};
use crate::state::AppState;
use crate::{acoustid, dupes, jobs, lyrics, mb, p, tagger};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::LazyLock;

/// (key, name, description)
pub const TASKS: [(&str, &str, &str); 11] = [
    ("scan", "Scan the library", "Pick up files added, changed or removed outside Encore."),
    ("sync", "Sync likes & playlists", "Two-way liked songs and playlist sync with Jellyfin, Plex, Navidrome, ListenBrainz, Spotify…"),
    ("lossy", "Lossless upgrades", "Re-check albums you only have as MP3/YouTube rips for a lossless copy."),
    ("discover", "Auto-discover", "Request the top songs of each user's ListenBrainz Weekly Exploration (count set in Auto-download)."),
    ("lyrics", "Fetch missing lyrics", "Download lyrics (.lrc) from LRCLIB for songs that don't have any yet."),
    ("artwork", "Find missing artwork", "Look up and embed cover art for albums without any."),
    ("retag", "Re-tag albums added to MusicBrainz", "Albums tagged from Deezer/Apple/Discogs get proper MusicBrainz tags once they appear there."),
    ("verify", "Verify audio fingerprints", "Fingerprint unchecked songs with AcoustID and flag wrong audio."),
    ("cleanup", "Clean up", "Remove finished downloads older than the cleanup age from the list and delete leftover staging files."),
    ("tools", "Update download tools", "Check yt-dlp and streamrip for new versions and install them – no new Encore image needed. yt-dlp changes often as sites change; streamrip only when its maintainers publish a release."),
    ("backup", "Back up the database", "Copy the database to config/backups (kept for 7 days, then one a week for 8 weeks, plus the last 3 pre-upgrade copies)."),
];

static BOOT: LazyLock<f64> = LazyLock::new(now);

/// Settings behind each task: (on/off key, minutes key, minutes scale). Old settings use interval 0 = off.
fn keys(key: &str) -> (&'static str, &'static str, i64) {
    match key {
        "scan" => ("", "scan_interval_minutes", 1),
        "sync" => ("", "sync_interval_minutes", 1),
        "lossy" => ("upgrade_lossy", "task_lossy_minutes", 1),
        "discover" => ("discover_auto_count", "task_discover_minutes", 1),
        "lyrics" => ("task_lyrics_enabled", "task_lyrics_minutes", 1),
        "artwork" => ("task_artwork_enabled", "task_artwork_minutes", 1),
        "retag" => ("task_retag_enabled", "task_retag_minutes", 1),
        "verify" => ("task_verify_enabled", "task_verify_minutes", 1),
        "tools" => ("task_tools_enabled", "task_tools_minutes", 1),
        "backup" => ("task_backup_enabled", "task_backup_minutes", 1),
        _ => ("task_cleanup_enabled", "task_cleanup_minutes", 1),
    }
}

fn enabled(st: &AppState, key: &str) -> bool {
    let (flag, mins, _) = keys(key);
    match key {
        "scan" | "sync" => st.cfg.int(mins) > 0,
        "discover" => st.cfg.int(flag) > 0,
        _ => st.cfg.bool(flag),
    }
}

fn minutes(st: &AppState, key: &str) -> i64 {
    let (_, mins, scale) = keys(key);
    st.cfg.int(mins).max(0) * scale
}

pub fn update(st: &AppState, key: &str, on: Option<bool>, mins: Option<i64>) {
    let (flag, mk, scale) = keys(key);
    if let Some(m) = mins {
        let m = m.clamp(5, 60 * 24 * 30);
        st.cfg.set_many(&json!({ mk: (m / scale).max(1) }));
    }
    if let Some(on) = on {
        match key {
            "scan" | "sync" => {
                if !on {
                    st.cfg.set_many(&json!({ mk: 0 }));
                } else if st.cfg.int(mk) <= 0 {
                    let default = match key { "sync" => 15, _ => 60 };
                    st.cfg.set_many(&json!({ mk: default / scale }));
                }
            }
            "discover" => {
                let cur = st.cfg.int(flag);
                st.cfg.set_many(&json!({ flag: if on { if cur > 0 { cur } else { 3 } } else { 0 } }));
            }
            _ => st.cfg.set_many(&json!({ flag: on })),
        }
    }
}

fn state(st: &AppState, key: &str) -> crate::db::Row {
    st.db.one("SELECT * FROM task_runs WHERE key=?", p![key]).ok().flatten().unwrap_or_default()
}

pub fn listing(st: &AppState) -> Vec<Value> {
    TASKS.iter().map(|(key, name, desc)| {
        let s = state(st, key);
        let every = minutes(st, key);
        let on = enabled(st, key);
        let last = s.get("last_run").and_then(|v| v.as_f64());
        let (flag, mk, _) = keys(key);
        let locked = [flag, mk].iter().any(|k| !k.is_empty() && st.cfg.env_locked(k));
        json!({ "key": key, "name": name, "desc": desc, "enabled": on, "minutes": every, "last_run": last, "duration": s.get("duration"),
            "result": s.get("result"), "error": s.get("error"), "running": jobs::running(&format!("task:{key}")),
            "next_run": if on { Some(last.unwrap_or(*BOOT) + every as f64 * 60.0) } else { None },
            "locked": locked })
    }).collect()
}

/// Start a task in the background; false when it's already running.
pub fn run_now(st: &AppState, key: &str) -> bool {
    let job = format!("task:{key}");
    if !jobs::start(&job, json!({})) {
        return false;
    }
    let (st, key) = (st.clone(), key.to_string());
    tokio::spawn(async move {
        let start = now();
        let _ = st.db.execute("INSERT OR REPLACE INTO task_runs (key, last_run, duration, result, error) VALUES (?,?,?,?,?)", p![key, start, None::<f64>, "running…", None::<String>]);
        let (res, error) = match run(&st, &key).await { Ok(r) => (Some(r), None), Err(e) => (None, Some(e.to_string().chars().take(300).collect::<String>())) };
        let _ = st.db.execute("UPDATE task_runs SET duration=?, result=?, error=? WHERE key=?", p![now() - start, res, error, key]);
        jobs::set(&job, json!({ "running": false }));
    });
    true
}

/// Called by the worker loop every few seconds: start every task that's due.
pub fn tick(st: &AppState) {
    let t = now();
    for (key, _, _) in TASKS {
        if !enabled(st, key) {
            continue;
        }
        let every = minutes(st, key).max(1) as f64 * 60.0;
        // after a restart the first runs are staggered instead of all at once
        let first = match key { "scan" => 30.0, "sync" => 120.0, "cleanup" => 900.0, _ => 1800.0 };
        let mut last = state(st, key).get("last_run").and_then(|v| v.as_f64()).unwrap_or(*BOOT - every + first);
        if key == "backup" {
            // a copy taken at start (or by hand) counts: no second one half an hour later
            if let Some(b) = crate::backup::newest_copy(st.data_dir()) {
                last = last.max(b.time);
            }
        }
        if t - last >= every {
            run_now(st, key);
        }
    }
}

async fn run(st: &AppState, key: &str) -> anyhow::Result<String> {
    Ok(match key {
        "scan" => {
            st.library.scan(st.db.clone(), st.cfg.str("library_path"), false).await;
            st.library.scan.lock().unwrap().message.clone()
        }
        "sync" => {
            crate::sync::sync_all(st.clone()).await;
            "done".into()
        }
        "lossy" => format!("{} upgrade(s) queued", queue_lossy_upgrades(st, 20)?),
        "discover" => format!("{} song(s) requested", auto_discover(st).await),
        "lyrics" => {
            let r = lyrics::fetch_missing(st, None, 150).await;
            format!("lyrics found for {} of {}", r["found"], r["checked"])
        }
        "artwork" => crate::routes::tagging::find_missing_artwork(st.clone()).await,
        "retag" => format!("{} album(s) re-tagged", retag_external(st).await),
        "verify" => {
            if !acoustid::available(st) {
                anyhow::bail!("needs an AcoustID key and fpcalc");
            }
            dupes::verify_library(st.clone(), true).await;
            let j = jobs::get("verify");
            format!("{} verified, {} wrong", j["verified"], j["mismatch"])
        }
        "tools" => crate::tools::scheduled(st).await?,
        "backup" => {
            // VACUUM INTO of a big library takes seconds: not on a runtime worker thread
            let db = st.db.clone();
            let b = tokio::task::spawn_blocking(move || db.backup(crate::backup::Kind::Scheduled)).await??;
            st.db.log("info", &format!("Database backed up to backups/{}", b.name));
            let gone = crate::backup::prune(st.data_dir());
            format!("backups/{}{}", b.name, if gone.is_empty() { String::new() } else { format!(", {} old backup(s) removed", gone.len()) })
        }
        _ => cleanup(st)?,
    })
}

fn cleanup(st: &AppState) -> anyhow::Result<String> {
    let days = st.cfg.int("task_cleanup_days").max(1);
    let cutoff = now() - days as f64 * 86400.0;
    let n = st.db.one("SELECT COUNT(*) n FROM requests WHERE status IN ('completed','denied','cancelled') AND updated_at < ?", p![cutoff])?.map(|r| r.i("n")).unwrap_or(0);
    st.db.execute("DELETE FROM requests WHERE status IN ('completed','denied','cancelled') AND updated_at < ?", p![cutoff])?;
    let active: std::collections::HashSet<String> = st.db.query("SELECT id FROM requests WHERE status IN ('queued','searching','downloading','monitoring','importing')", p![])?
        .iter().map(|r| format!("request-{}", r.i("id"))).collect();
    let mut removed = 0;
    if let Ok(rd) = std::fs::read_dir(st.cfg.str("staging_path")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|d| d.as_secs() > 3600);
            if name.starts_with("request-") && !active.contains(&name) && old {
                let _ = std::fs::remove_dir_all(e.path());
                removed += 1;
            }
        }
    }
    Ok(format!("{removed} leftover download folder(s) removed{}", if n > 0 { format!(", {n} old request(s) cleared") } else { String::new() }))
}

/// Re-request MusicBrainz-tagged albums that are only lossy (lossless sources only).
pub fn queue_lossy_upgrades(st: &AppState, limit: usize) -> anyhow::Result<usize> {
    let days = st.cfg.int("upgrade_lossy_days").max(1) as f64;
    let rows = st.db.query("SELECT mb_releasegroupid rg, MAX(albumartist) artist, MAX(album) album, MAX(year) year FROM tracks
        WHERE mb_releasegroupid IS NOT NULL AND mb_releasegroupid != '' GROUP BY album_id
        HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = 0", p![])?;
    let t = now();
    let mut n = 0;
    for a in rows {
        if n >= limit {
            break;
        }
        let rg = a.s("rg");
        let scope = format!("upg:{rg}");
        if let Some(last) = st.db.one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", p![scope])? {
            if t - last.s("value").parse::<f64>().unwrap_or(0.0) < days * 86400.0 {
                continue;
            }
        }
        if st.db.one("SELECT 1 FROM requests WHERE mbid=? AND status IN ('queued','searching','downloading','monitoring','importing','pending')", p![rg])?.is_some() {
            continue;
        }
        st.db.execute("INSERT OR REPLACE INTO sync_state VALUES (0,?,?)", p![scope, t.to_string()])?;
        st.db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, message, detail, created_at, updated_at)
                       VALUES (NULL,'album',?,?,?,?,?,?,'queued',?,?,?,?)",
            p![rg, a.s("artist"), a.s("album"), a.s("album"), a.s("year"), mb::cover_rg(&rg, 250), "Lossless upgrade", r#"{"lossless_only": true}"#, t, t])?;
        n += 1;
    }
    if n > 0 {
        st.db.log("info", &format!("Queued {n} album(s) for a lossless upgrade"));
    }
    Ok(n)
}

/// Albums imported from Deezer/Apple data: once the release is in MusicBrainz, tag them properly.
pub async fn retag_external(st: &AppState) -> usize {
    let rows = st.db.query("SELECT t.album_id, MIN(t.albumartist) artist, MIN(t.album) album FROM tracks t JOIN requests r ON r.id=t.request_id
        WHERE (t.mb_albumid IS NULL OR t.mb_albumid='') AND r.mbid IS NULL AND r.detail LIKE '%\"ext\"%' GROUP BY t.album_id LIMIT 25", p![]).unwrap_or_default();
    let mut done = 0;
    for a in rows {
        if tagger::is_manual(st, &a.s("artist"), &a.s("album")) {
            continue; // fixed by hand
        }
        let paths: Vec<PathBuf> = st.db.query("SELECT path FROM tracks WHERE album_id=?", p![a.s("album_id")]).unwrap_or_default().iter().map(|r| PathBuf::from(r.s("path"))).collect();
        let res: anyhow::Result<bool> = async {
            let Some(pv) = tagger::auto_match(st, &paths, &a.s("artist"), &a.s("album"), None).await? else { return Ok(false) };
            let new = tagger::apply_preview(st, &pv, true, None, None).await?;
            tagger::relink(st, &paths, &new);
            tagger::index_paths(st, &new);
            Ok(true)
        }.await;
        match res {
            Ok(true) => {
                st.db.log("info", &format!("{} – {} is in MusicBrainz now: re-tagged", a.s("artist"), a.s("album")));
                done += 1;
            }
            Ok(false) => {}
            Err(e) => st.db.log("warning", &format!("Re-tag of {} failed: {e}", a.s("album"))),
        }
    }
    done
}

/// Request the top N songs of each user's newest ListenBrainz Weekly Exploration (once per playlist).
pub async fn auto_discover(st: &AppState) -> usize {
    let n = st.cfg.int("discover_auto_count").max(0) as usize;
    if n == 0 {
        return 0;
    }
    let mut total = 0;
    for c in st.db.query("SELECT * FROM connections WHERE service='listenbrainz'", p![]).unwrap_or_default() {
        let Ok(Some(u)) = st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![c.i("user_id")]) else { continue };
        if !(crate::auth::is_admin(&u) || u.b("auto_download")) {
            continue;
        }
        let user = serde_json::from_str::<Value>(&c.s("data")).ok().and_then(|d| d["user"].as_str().map(String::from)).unwrap_or_default();
        let res: anyhow::Result<usize> = async {
            let pls = crate::routes::foryou::lb_playlists(st, &user).await?;
            let Some(pl) = pls.into_iter().find(|p| p["kind"] == "weekly-exploration") else { return Ok(0) };
            let scope = format!("discover:{}", pl["id"].as_str().unwrap_or(""));
            if st.db.one("SELECT 1 FROM sync_state WHERE connection_id=? AND scope=?", p![c.i("id"), scope])?.is_some() {
                return Ok(0);
            }
            let tracks = crate::routes::foryou::annotate_tracks(st, crate::routes::foryou::lb_playlist_tracks(st, pl["id"].as_str().unwrap_or("")).await?);
            let mut picked = 0;
            for t in tracks {
                if picked >= n {
                    break;
                }
                if !t["library"].is_null() || !t["request_status"].is_null() || (t["mbid"].is_null() && t["title"].as_str().unwrap_or("").is_empty()) {
                    continue;
                }
                let s = |k: &str| t[k].as_str().filter(|x| !x.is_empty()).map(String::from);
                let b = NewRequest { kind: "track".into(), mbid: s("mbid"), artist: s("artist"), title: s("title"), album: s("album"), cover_url: s("cover"), length: t["length"].as_f64(), ..Default::default() };
                if create_request(st, &u, &b, None, None).is_ok() {
                    picked += 1;
                }
            }
            st.db.execute("INSERT OR REPLACE INTO sync_state (connection_id, scope, value) VALUES (?,?,?)", p![c.i("id"), scope, now().to_string()])?;
            if picked > 0 {
                st.db.log("info", &format!("Auto-discover: requested {picked} song(s) from {}'s {}", u.s("username"), pl["title"].as_str().unwrap_or("")));
            }
            Ok(picked)
        }.await;
        match res {
            Ok(k) => total += k,
            Err(e) => st.db.log("warning", &format!("Auto-discover failed for {}: {e}", c.s("account"))),
        }
    }
    total
}
