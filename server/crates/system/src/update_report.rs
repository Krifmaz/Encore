//! The record of the last "Update now", step by step, so Settings → Diagnostics → Updates can say exactly
//! which step failed and with which error. The record and how it is read back from the helper's log are the
//! kit's (`kit_server_selfupdate::report`); Encore keeps it in `sync_state` (connection 0), logs a failure
//! in the activity log and the server log, and shows it as `last_update` in GET /api/system/update.
use crate::selfupdate;
use crate::Ctx;
use encore_db::p;
use kit_server_selfupdate::Docker;
use serde_json::Value;

pub use kit_server_selfupdate::report::{Report, State};

/// Where the record is kept (`sync_state`, connection 0).
const SCOPE: &str = "encore:update:last";

/// The kept record, if any (one kept before the kit carries no app name: it is Encore's).
pub fn load(st: &impl Ctx) -> Option<Report> {
    let row = st.db().one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", p![SCOPE]).ok().flatten()?;
    let mut r: Report = serde_json::from_str(row.get("value")?.as_str()?).ok()?;
    if r.app.is_empty() {
        r.app = selfupdate::app().name;
    }
    Some(r)
}

/// Keep `r` as the record of the last update.
pub fn store(st: &impl Ctx, r: &Report) {
    if let Ok(v) = serde_json::to_string(r) {
        let _ = st.db().execute("INSERT OR REPLACE INTO sync_state (connection_id, scope, value) VALUES (0,?,?)", p![SCOPE, v]);
    }
}

/// Read back what the last update did (the kit's `read_back`: the ended helper's log merged in and the helper
/// removed, then the running version `current` checked), log a failure, and keep the record. Nothing
/// without a record; a record that has ended is given back as it is.
pub async fn collect(st: &impl Ctx, current: &str) -> Option<Report> {
    let mut r = load(st)?;
    if r.state != State::Running {
        return Some(r);
    }
    let d = Docker::new(&st.system().docker_socket);
    kit_server_selfupdate::report::read_back(&selfupdate::app(), &d, &mut r, current).await;
    if r.state == State::Failed {
        st.db().log("error", &format!("Update to {} failed at step {}: {}", r.image, r.failed_step.as_deref().unwrap_or("unknown"), r.error.as_deref().unwrap_or("")));
        tracing::error!("update failed:\n{}", r.text());
    }
    store(st, &r);
    Some(r)
}

/// The record for GET /api/system/update (Null when there is none), with the plain-text form Copy takes.
pub fn json(r: Option<&Report>) -> Value {
    kit_server_selfupdate::report::json(r)
}
