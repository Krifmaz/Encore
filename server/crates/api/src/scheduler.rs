//! The server's scheduled tasks. The schedule (switches, intervals, last runs, what is due) lives in
//! encore_jobs::scheduler; each task here is run by the crate it belongs to.
use encore_db::{now, RowExt};
use encore_requests::create::{create_request, NewRequest};
use crate::state::AppState;
use encore_library::acoustid;
use encore_library::dupes;
use encore_library::lyrics;
use encore_db::p;
use encore_jobs::scheduler::Tasks;
use serde_json::Value;

/// Every task with its switch, interval, last run and result (Settings → Tasks).
pub fn listing(st: &AppState) -> Vec<Value> {
    st.scheduler.listing()
}

/// Switch a task on or off and set how often it runs.
pub fn update(st: &AppState, key: &str, on: Option<bool>, mins: Option<i64>) {
    st.scheduler.update(key, on, mins)
}

/// The scheduler runs the tasks through the server, which knows every crate's jobs.
impl Tasks for AppState {
    fn scheduler(&self) -> &encore_jobs::Scheduler {
        &self.scheduler
    }
    fn run_task(&self, key: &str) -> impl std::future::Future<Output = anyhow::Result<String>> + Send {
        run(self, key)
    }
}

/// Run the scheduled task `key`: its summary line.
async fn run(st: &AppState, key: &str) -> anyhow::Result<String> {
    Ok(match key {
        "scan" => {
            st.library.scan_library(st.db.clone(), &st.cfg, false).await;
            st.library.scan.lock().unwrap().message.clone()
        }
        "sync" => {
            encore_sync::sync_all(st.clone()).await;
            "done".into()
        }
        "lossy" => format!("{} upgrade(s) queued", encore_requests::upkeep::queue_lossy_upgrades(st, 20)?),
        "hires" => format!("{} hi-res upgrade(s) queued", encore_requests::upgrade::queue_hires_upgrades(st, 20)?),
        "discover" => format!("{} song(s) requested", auto_discover(st).await),
        "lyrics" => {
            let r = lyrics::fetch_missing(st, None, 150).await;
            format!("lyrics found for {} of {}", r["found"], r["checked"])
        }
        "artwork" => crate::routes::tagging::find_missing_artwork(st.clone()).await,
        "retag" => format!("{} album(s) re-tagged", encore_library::tagger::retag_external(st).await),
        "verify" => {
            if !acoustid::available(st) {
                anyhow::bail!("needs an AcoustID key and fpcalc");
            }
            dupes::verify_library(st.clone(), true).await;
            let j = st.jobs.get("verify");
            format!("{} verified, {} wrong", j["verified"], j["mismatch"])
        }
        "tools" => encore_system::tools::scheduled(st).await?,
        "backup" => {
            // VACUUM INTO of a big library takes seconds: not on a runtime worker thread
            let db = st.db.clone();
            let b = tokio::task::spawn_blocking(move || db.backup(encore_db::backup::Kind::Scheduled)).await??;
            st.db.log("info", &format!("Database backed up to backups/{}", b.name));
            let gone = encore_db::backup::prune(st.data_dir());
            format!("backups/{}{}", b.name, if gone.is_empty() { String::new() } else { format!(", {} old backup(s) removed", gone.len()) })
        }
        "quality" => encore_media::quality::scan(st, 300).await?,
        "loudness" => encore_library::loudness::scheduled(st).await?,
        "types" => encore_library::album_types::scheduled(st).await?,
        "update" => encore_system::update::scheduled(st).await?,
        "atmos" => {
            let batch = st.cfg.int("surround_sweep_batch").max(1) as usize;
            encore_requests::surround::sweep(st, None, batch).await?.line()
        }
        _ => {
            let mut out = encore_requests::upkeep::cleanup(st)?;
            // the file manager's Trash keeps items `trash_days` days
            let gone = encore_files::trash::purge(st);
            if gone > 0 {
                out.push_str(&format!(", {gone} item(s) older than {} days deleted from the Trash", encore_library::folders::trash_days(&st.cfg)));
            }
            out
        }
    })
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
        if !(encore_config::perms::is_admin(&u) || u.b("auto_download")) {
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
