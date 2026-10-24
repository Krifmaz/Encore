//! Scheduled tasks: one place to switch background jobs on or off, set how often they run and run them
//! now (Settings → Tasks). The schedule (which task is due, its last run and result) lives here; the
//! tasks themselves belong to the crates that own them, and the server runs them through [`Tasks`].
use encore_config::Config;
use encore_db::{now, p, Db, Row};
use serde_json::{json, Value};
use std::future::Future;
use std::time::Duration;

/// (key, name, description)
pub const TASKS: [(&str, &str, &str); 15] = [
    ("scan", "Scan the library", "Pick up files added, changed or removed outside Encore."),
    ("sync", "Sync likes & playlists", "Two-way liked songs and playlist sync with Jellyfin, Plex, Navidrome, ListenBrainz, Spotify…"),
    ("lossy", "Lossless upgrades", "Re-check albums you only have as MP3/YouTube rips for a lossless copy."),
    ("discover", "Auto-discover", "Request the top songs of each user's ListenBrainz Weekly Exploration (count set in Auto-download)."),
    ("lyrics", "Fetch missing lyrics", "Download lyrics (.lrc) from LRCLIB for songs that don't have any yet."),
    ("artwork", "Find missing artwork", "Look up and embed cover art for albums without any."),
    ("retag", "Re-tag albums added to MusicBrainz", "Albums tagged from Deezer/Apple/Discogs get proper MusicBrainz tags once they appear there."),
    ("verify", "Verify audio fingerprints", "Fingerprint unchecked songs with AcoustID and flag wrong audio."),
    ("quality", "Quality scan", "Check library songs for fake lossless (a lossy file saved as FLAC), upsampled hi-res, very quiet or clipped audio, a batch per run at low priority. Suspect songs show under Settings → Blocked with a Replace button; nothing is removed."),
    ("loudness", "Measure loudness", "Measure how loud each song is, for volume normalization in the apps. Songs with ReplayGain tags use them; the others are measured at low priority while nobody is using the app, and each run goes on where the last one stopped."),
    ("types", "Sort albums by type", "Work out which albums are live, compilations, remixes, soundtracks, singles or one-offs: reads the type the files' tags give and, for albums without one, asks MusicBrainz at low priority. Imports record the type themselves."),
    ("cleanup", "Clean up", "Remove finished downloads older than the cleanup age from the list and delete leftover staging files and reported files kept in the trash for 7 days."),
    ("tools", "Update plugins", "Check the installed plugins (yt-dlp, streamrip, Soulseek, Cadenza, the Translator and the ones you added) for new versions and install them for the plugins switched to update automatically under Settings → Plugins – no new Encore image needed. A plugin that is switched on but whose program is missing is installed again."),
    ("backup", "Back up the database", "Copy the database to config/backups, or the music folder's .encore-backups when that disk is full (Settings → General → Database backups says how many are kept)."),
    ("update", "Check for Encore updates", "Look for a new Encore release on GitHub. Settings → Diagnostics shows what changed, with Restart server and, when the Docker socket is mounted, Update now."),
];

/// How often the schedule is looked at.
const TICK: Duration = Duration::from_secs(5);

/// What runs the tasks: the server, which knows every crate's jobs.
pub trait Tasks: Clone + Send + Sync + 'static {
    /// The scheduler the server runs.
    fn scheduler(&self) -> &Scheduler;
    /// Run task `key` to the end: Ok(the result line for Settings → Tasks) or the error.
    fn run_task(&self, key: &str) -> impl Future<Output = anyhow::Result<String>> + Send;
}

/// The schedule of one server: its settings, the task runs in its database and the job table that says
/// which task runs now. Clones share them.
#[derive(Clone)]
pub struct Scheduler {
    /// where task runs are recorded
    db: Db,
    /// the settings that switch tasks on and set their intervals
    cfg: Config,
    /// progress of the running jobs
    jobs: crate::Jobs,
    /// when the server started: the first runs after a start are counted from here
    boot: f64,
}

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
        "quality" => ("task_quality_enabled", "task_quality_minutes", 1),
        "loudness" => ("task_loudness_enabled", "task_loudness_minutes", 1),
        "types" => ("task_types_enabled", "task_types_minutes", 1),
        "update" => ("check_updates", "task_update_minutes", 1),
        _ => ("task_cleanup_enabled", "task_cleanup_minutes", 1),
    }
}

impl Scheduler {
    /// A scheduler over `db`, `cfg` and `jobs`, counting from now.
    pub fn new(db: Db, cfg: Config, jobs: crate::Jobs) -> Scheduler {
        Scheduler { db, cfg, jobs, boot: now() }
    }

    /// Whether task `key` is switched on (scan and sync: when their interval is above 0).
    fn enabled(&self, key: &str) -> bool {
        let (flag, mins, _) = keys(key);
        match key {
            "scan" | "sync" => self.cfg.int(mins) > 0,
            "discover" => self.cfg.int(flag) > 0,
            _ => self.cfg.bool(flag),
        }
    }

    /// Task `key`'s interval in minutes (0 or more).
    fn minutes(&self, key: &str) -> i64 {
        let (_, mins, scale) = keys(key);
        self.cfg.int(mins).max(0) * scale
    }

    /// Switch a task on or off and set how often it runs (minutes, 5 minutes to 30 days).
    pub fn update(&self, key: &str, on: Option<bool>, mins: Option<i64>) {
        let (flag, mk, scale) = keys(key);
        if let Some(m) = mins {
            let m = m.clamp(5, 60 * 24 * 30);
            self.cfg.set_many(&json!({ mk: (m / scale).max(1) }));
        }
        if let Some(on) = on {
            match key {
                "scan" | "sync" => {
                    if !on {
                        self.cfg.set_many(&json!({ mk: 0 }));
                    } else if self.cfg.int(mk) <= 0 {
                        let default = match key { "sync" => 15, _ => 60 };
                        self.cfg.set_many(&json!({ mk: default / scale }));
                    }
                }
                "discover" => {
                    let cur = self.cfg.int(flag);
                    self.cfg.set_many(&json!({ flag: if on { if cur > 0 { cur } else { 3 } } else { 0 } }));
                }
                _ => self.cfg.set_many(&json!({ flag: on })),
            }
        }
    }

    /// The last run of task `key` as recorded in `task_runs` (empty when it never ran).
    fn state(&self, key: &str) -> Row {
        self.db.one("SELECT * FROM task_runs WHERE key=?", p![key]).ok().flatten().unwrap_or_default()
    }

    /// Every task with its switch, interval, last run and result, and when it runs next.
    pub fn listing(&self) -> Vec<Value> {
        TASKS.iter().map(|(key, name, desc)| {
            let s = self.state(key);
            let every = self.minutes(key);
            let on = self.enabled(key);
            let last = s.get("last_run").and_then(|v| v.as_f64());
            let (flag, mk, _) = keys(key);
            let locked = [flag, mk].iter().any(|k| !k.is_empty() && self.cfg.env_locked(k));
            json!({ "key": key, "name": name, "desc": desc, "enabled": on, "minutes": every, "last_run": last, "duration": s.get("duration"),
                "result": s.get("result"), "error": s.get("error"), "running": self.jobs.running(&format!("task:{key}")),
                "next_run": if on { Some(last.unwrap_or(self.boot) + every as f64 * 60.0) } else { None },
                "locked": locked })
        }).collect()
    }

    /// The tasks due now.
    fn due(&self) -> Vec<&'static str> {
        let t = now();
        let mut out = vec![];
        for (key, _, _) in TASKS {
            if !self.enabled(key) {
                continue;
            }
            let every = self.minutes(key).max(1) as f64 * 60.0;
            // after a restart the first runs are staggered instead of all at once
            let first = match key { "scan" => 30.0, "sync" => 120.0, "types" => 180.0, "cleanup" => 900.0, _ => 1800.0 };
            let mut last = self.state(key).get("last_run").and_then(|v| v.as_f64()).unwrap_or(self.boot - every + first);
            if key == "backup" {
                // a copy taken at start (or by hand) counts: no second one half an hour later
                if let Some(b) = encore_db::backup::newest_copy(&self.db.data_dir) {
                    last = last.max(b.time);
                }
            }
            if t - last >= every {
                out.push(key);
            }
        }
        out
    }
}

/// Start a task in the background; false when it's already running.
pub fn run_now<T: Tasks>(tasks: &T, key: &str) -> bool {
    let s = tasks.scheduler();
    let job = format!("task:{key}");
    if !s.jobs.start(&job, json!({})) {
        return false;
    }
    let (tasks, key) = (tasks.clone(), key.to_string());
    tokio::spawn(async move {
        let s = tasks.scheduler();
        let start = now();
        let _ = s.db.execute("INSERT OR REPLACE INTO task_runs (key, last_run, duration, result, error) VALUES (?,?,?,?,?)", p![key, start, None::<f64>, "running…", None::<String>]);
        let (res, error) = match tasks.run_task(&key).await { Ok(r) => (Some(r), None), Err(e) => (None, Some(e.to_string().chars().take(300).collect::<String>())) };
        let _ = s.db.execute("UPDATE task_runs SET duration=?, result=?, error=? WHERE key=?", p![now() - start, res, error, key]);
        s.jobs.set(&job, json!({ "running": false }));
    });
    true
}

/// Start every task that's due.
pub fn tick<T: Tasks>(tasks: &T) {
    for key in tasks.scheduler().due() {
        run_now(tasks, key);
    }
}

/// The scheduler's loop: a look at the schedule every few seconds, as long as the server runs.
pub async fn run<T: Tasks>(tasks: T) {
    loop {
        tick(&tasks);
        tokio::time::sleep(TICK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Fake(Scheduler);

    impl Tasks for Fake {
        fn scheduler(&self) -> &Scheduler {
            &self.0
        }
        async fn run_task(&self, key: &str) -> anyhow::Result<String> {
            if key == "lyrics" { anyhow::bail!("no network here") } else { Ok(format!("{key} done")) }
        }
    }

    /// A task switched on runs once its interval is over (counted from the server's start the first
    /// time), its result or error is kept, and a task that runs is not started twice.
    #[tokio::test]
    async fn due_tasks_run_and_their_results_are_kept() {
        let dir = std::env::temp_dir().join(format!("encore-scheduler-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = Db::open(&dir).unwrap();
        let cfg = Config::load(db.clone()).unwrap();
        let jobs = crate::Jobs::default();
        let mut s = Scheduler::new(db.clone(), cfg.clone(), jobs.clone());
        // every task off but two
        for (key, _, _) in TASKS {
            s.update(key, Some(false), None);
        }
        s.update("cleanup", Some(true), Some(60));
        s.update("lyrics", Some(true), Some(60));
        assert!(s.due().is_empty(), "just started: nothing is due yet");
        s.boot -= 3600.0;
        assert_eq!(s.due(), vec!["lyrics", "cleanup"]);
        let fake = Fake(s.clone());
        assert!(run_now(&fake, "cleanup"));
        for _ in 0..200 {
            if !jobs.running("task:cleanup") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let row = |k: &str| s.listing().into_iter().find(|t| t["key"] == k).unwrap();
        assert_eq!(row("cleanup")["result"], "cleanup done");
        assert!(!s.due().contains(&"cleanup"), "ran just now");
        assert!(run_now(&fake, "lyrics"));
        for _ in 0..200 {
            if !jobs.running("task:lyrics") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(row("lyrics")["error"], "no network here");
        assert!(jobs.start("task:scan", json!({})));
        assert!(!run_now(&fake, "scan"), "already running");
        assert_eq!(row("lyrics")["enabled"], true);
        assert_eq!(row("scan")["enabled"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
