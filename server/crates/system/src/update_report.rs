//! The record of the last "Update now", step by step, so Settings → Diagnostics → Updates can say exactly
//! which step failed and with which error, instead of only "the update did not go through".
//!
//! An update runs in two places. The server checks, pulls the new image and starts the helper container
//! (update.rs: steps `check`, `pull`, `helper`); the helper, a container of the NEW image, swaps the server's
//! container (selfupdate.rs: `inspect`, `schema`, `stop`, `rename`, `create`, `start`, `verify`, `cleanup`)
//! while the server is down. The server writes its part into the database as it goes. The helper has no
//! database: it prints one line per step, and its container is left behind when it ends (no `AutoRemove`) so
//! the server that comes back – the new one, or the old one after a roll-back – reads its log, merges it in
//! and removes it ([`collect`]). Last, the server that comes back checks its own version against the one the
//! update was for (step `verify`).
//!
//! Helper lines look like `encore self-update: <step>: <text>`; the last one of a failed swap is
//! `encore self-update: failed: <step>: <error>` (helpers before this record wrote the error without its step:
//! [`parse_helper_log`] reads both).
use crate::docker::{self, Docker};
use crate::selfupdate::HELPER_LABEL;
use crate::Ctx;
use encore_db::{self as db, p};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the record is kept (`sync_state`, connection 0).
const SCOPE: &str = "encore:update:last";
/// What every helper line starts with (selfupdate.rs `main_if_helper`).
pub const LINE_PREFIX: &str = "encore self-update: ";
/// The start of a helper's last line when the swap failed.
pub const FAILED: &str = "failed: ";
/// How many lines of a helper's log are read back (a swap prints about fifteen; a failure adds the new
/// container's last lines).
const HELPER_LOG_LINES: usize = 400;
/// A record still `running` this long after it started, with no helper left to read, failed without a word.
const LOST_AFTER_SECS: f64 = 15.0 * 60.0;
/// The step names a failure can be filed under; anything else is the helper's swap as a whole.
const STEPS: &[&str] = &["check", "pull", "helper", "inspect", "schema", "stop", "rename", "create", "start", "verify", "cleanup"];

/// Where an update is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// on its way (the pull, or the helper swapping the container)
    #[default]
    Running,
    /// the server came back on the new version
    Ok,
    /// a step failed; `failed_step` and `error` say which and why
    Failed,
}

/// The last update, as the Diagnostics page shows it (`last_update` in GET /api/system/update).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// the image it was for (`ghcr.io/krifmaz/encore:0.42.18`)
    pub image: String,
    /// the version it was for (`0.42.18`)
    pub to: String,
    /// the version that started it
    pub from: String,
    /// who pressed Update now
    pub by: String,
    /// when it started (Unix seconds)
    pub started_at: f64,
    /// when it ended, either way (Unix seconds)
    pub finished_at: Option<f64>,
    /// where it is
    pub state: State,
    /// the step that failed (one of the step names in the module docs)
    pub failed_step: Option<String>,
    /// that step's error, in Docker's or Encore's own words
    pub error: Option<String>,
    /// every step with its output, server and helper lines in order (`<step>: <text>`)
    pub lines: Vec<String>,
}

impl Report {
    /// A new record for an update from `from` to `image` (`IMAGE:to`) asked for by `by`.
    pub fn start(image: &str, to: &str, from: &str, by: &str, now: f64) -> Report {
        Report { image: image.into(), to: to.into(), from: from.into(), by: by.into(), started_at: now, ..Default::default() }
    }

    /// One step's output.
    pub fn line(&mut self, step: &str, text: &str) {
        self.lines.push(format!("{step}: {text}"));
    }

    /// `step` failed with `error`: the record ends here.
    pub fn fail(&mut self, step: &str, error: &str, now: f64) {
        self.line(step, &format!("{FAILED}{error}"));
        self.state = State::Failed;
        self.failed_step = Some(step.into());
        self.error = Some(error.into());
        self.finished_at = Some(now);
    }

    /// The whole record as plain text, for the Copy button and the server log.
    pub fn text(&self) -> String {
        let mut out = format!("Encore update {} → {} ({}), started by {}\n", self.from, self.to, self.image, self.by);
        match self.state {
            State::Running => out.push_str("State: running\n"),
            State::Ok => out.push_str("State: done\n"),
            State::Failed => out.push_str(&format!("Failed at step: {}\nError: {}\n",
                self.failed_step.as_deref().unwrap_or("unknown"), self.error.as_deref().unwrap_or(""))),
        }
        out.push_str("Steps:\n");
        for l in &self.lines {
            out.push_str("  ");
            out.push_str(l);
            out.push('\n');
        }
        out
    }
}

/// The kept record, if any.
pub fn load(st: &impl Ctx) -> Option<Report> {
    let row = st.db().one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", p![SCOPE]).ok().flatten()?;
    serde_json::from_str(row.get("value")?.as_str()?).ok()
}

/// Keep `r` as the record of the last update.
pub fn store(st: &impl Ctx, r: &Report) {
    if let Ok(v) = serde_json::to_string(r) {
        let _ = st.db().execute("INSERT OR REPLACE INTO sync_state (connection_id, scope, value) VALUES (0,?,?)", p![SCOPE, v]);
    }
}

/// What a helper's log says: its step lines (without the prefix), and the failed step and its error when the
/// last line is a failure. An error without a step name (a helper before this record) is filed under `swap`.
pub fn parse_helper_log(text: &str) -> (Vec<String>, Option<(String, String)>) {
    let lines: Vec<String> = text.lines().filter_map(|l| l.trim_end().split_once(LINE_PREFIX).map(|(_, rest)| rest.to_string())).collect();
    let failure = lines.iter().rev().find_map(|l| l.strip_prefix(FAILED)).map(|e| match e.split_once(": ") {
        Some((step, why)) if STEPS.contains(&step) => (step.to_string(), why.to_string()),
        _ => ("swap".to_string(), e.to_string()),
    });
    (lines, failure)
}

/// Merge an ended helper's log (`text`, its exit code `exit`) into `r`.
pub fn merge_helper(r: &mut Report, text: &str, exit: i64, now: f64) {
    let (lines, failure) = parse_helper_log(text);
    r.lines.extend(lines.into_iter().map(|l| format!("helper: {l}")));
    match failure {
        Some((step, error)) => {
            r.state = State::Failed;
            r.failed_step = Some(step);
            r.error = Some(error);
            r.finished_at = Some(now);
        }
        None if exit != 0 => r.fail("helper", &format!("the update helper ended with exit code {exit} without saying why"), now),
        // the swap went through: the version check below has the last word
        None => {}
    }
}

/// The server that came back checks its own version: `current` is what it runs, `helper_done` whether the
/// helper's log was read (or there is none to wait for). A record still running ends here: done when the
/// version is the new one, failed (step `verify`) when it is still the old one after the helper ended or after
/// a long while.
pub fn verify(r: &mut Report, current: &str, helper_done: bool, now: f64) {
    if r.state != State::Running {
        return;
    }
    if current == r.to {
        r.line("verify", &format!("the server answers as Encore {current}"));
        r.state = State::Ok;
        r.finished_at = Some(now);
    } else if helper_done || now - r.started_at > LOST_AFTER_SECS {
        r.fail("verify", &format!("the server came back on Encore {current}, not {}; the update helper's log is gone (it was removed before it could be read)", r.to), now);
    }
}

/// The helper containers Docker still has (running or ended): `[(id, running, exit code)]`.
pub async fn helpers(d: &Docker) -> anyhow::Result<Vec<(String, bool, i64)>> {
    let filters = urlencoding::encode(&format!(r#"{{"label":["{HELPER_LABEL}=self-update-helper"]}}"#)).into_owned();
    let (code, list) = d.get(&format!("/containers/json?all=true&filters={filters}")).await?;
    if code != 200 {
        anyhow::bail!("Docker answered {code}: {}", docker::message(&list));
    }
    let mut out = vec![];
    for c in list.as_array().into_iter().flatten() {
        let Some(id) = c["Id"].as_str() else { continue };
        let running = c["State"].as_str() == Some("running");
        // the list has no exit code: the inspect has
        let exit = if running { 0 } else {
            d.inspect(id).await.ok().flatten().and_then(|v| v["State"]["ExitCode"].as_i64()).unwrap_or(0)
        };
        out.push((id.to_string(), running, exit));
    }
    Ok(out)
}

/// Read back what the last update did: every ended helper's log is merged into the record and the helper
/// container removed, then the running version is checked. Nothing without a record, or when it has ended.
pub async fn collect(st: &impl Ctx, current: &str) -> Option<Report> {
    let mut r = load(st)?;
    if r.state != State::Running {
        return Some(r);
    }
    let now = db::now();
    let d = Docker::new(&st.system().docker_socket);
    // the helper of this update is the newest; older ones (an earlier try) are only cleaned up
    let mut helper_done = false;
    let mut still_running = false;
    match helpers(&d).await {
        Ok(list) => {
            for (id, running, exit) in list {
                if running {
                    still_running = true;
                    continue;
                }
                if !helper_done {
                    match d.logs(&id, HELPER_LOG_LINES).await {
                        Ok(text) => merge_helper(&mut r, &text, exit, now),
                        Err(e) => r.line("helper", &format!("its log could not be read: {e}")),
                    }
                    helper_done = true;
                }
                if let Err(e) = d.delete(&format!("/containers/{id}?force=true")).await {
                    tracing::warn!("update: the ended helper container {id} stays: {e}");
                }
            }
        }
        Err(e) => r.line("helper", &format!("the helper containers could not be listed: {e}")),
    }
    if !still_running {
        verify(&mut r, current, helper_done, now);
    }
    if r.state == State::Failed {
        st.db().log("error", &format!("Update to {} failed at step {}: {}", r.image, r.failed_step.as_deref().unwrap_or("unknown"), r.error.as_deref().unwrap_or("")));
        tracing::error!("update failed:\n{}", r.text());
    }
    store(st, &r);
    Some(r)
}

/// The record for GET /api/system/update (Null when there is none).
/// The plain-text form (`text`, what Copy takes) rides along with the fields.
pub fn json(r: Option<&Report>) -> Value {
    let Some(r) = r else { return Value::Null };
    let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
    if v.is_object() {
        v["text"] = Value::String(r.text());
    }
    v
}

#[cfg(test)]
#[path = "update_report_tests.rs"]
mod tests;
