//! An import that hasn't moved, and the watchdog. An import says what it is at as it goes ([`super::steps`]:
//! "Importing – checking the audio (3 of 12)", and every few seconds while it waits in MusicBrainz's line) and
//! its row is written at each change of state; one that has said nothing and written nothing for
//! [`STALL_SECS`] (15 minutes), or whose task is gone, is stalled. Its row offers Import again
//! (`import_stalled` in the requests list, [`STALLED_TEXT`] beside it), which stops the stuck task and
//! imports the kept files at once in its place ([`import_again`]). The copy into the library keeps saying
//! it moves as the bytes go (`encore_library::tagger::place_all`), so a long copy onto a slow disk never
//! counts as stalled; one stuck on a mount that went away does, and since stopping the task doesn't stop
//! that blocking copy, the import started again waits for it to end before it moves the same files.
//!
//! The watchdog ([`watchdog`], at each look of the queue) starts a search or an import whose task is gone –
//! it ended without writing where it is (a panic, say) – again the way a server restart does, instead of
//! leaving it until the next restart.
use super::*;

/// How long an import may go without a word or a write before it counts as stalled (seconds).
pub const STALL_SECS: f64 = 900.0;
/// What Import again on a stalled import says about it (the tooltip of the row's button).
pub const STALLED_TEXT: &str = "The import hasn't moved for 15 minutes";

/// The sources whose files stay in their download client's folder (Soulseek, torrents and Usenet): an import
/// of theirs looks at the finished download again where it is.
const CLIENTS: [&str; 3] = ["slskd", "slskd_lossless", "torznab"];

/// Whether an import is stalled: the row is `importing`, and its task is gone (`running` false) or nothing
/// moved since `moved_at` for [`STALL_SECS`].
pub fn stalled(status: &str, moved_at: f64, running: bool, now: f64) -> bool {
    status == "importing" && (!running || now - moved_at >= STALL_SECS)
}

/// When request `r`'s import last moved: its row's last write, or a word of its step since then.
pub fn moved_at(st: &impl Ctx, r: &Req) -> f64 {
    let row = r.row.f("updated_at");
    st.requests().steps.heard(r.id).map_or(row, |h| h.max(row))
}

/// Whether Import again has something to import: the files were kept for the import, or they wait in a
/// download client's folder.
fn has_import(r: &Req) -> bool {
    r.d["kept"].is_object() || CLIENTS.contains(&r.s("source").as_str())
}

/// The request's import has not moved for too long and no task runs it.
fn is_stalled(st: &impl Ctx, r: &Req) -> bool {
    stalled(&r.s("status"), moved_at(st, r), st.requests().tasks.taken(r.id), now())
}

/// What the requests list adds to a row: `import_stalled` (an import that hasn't moved, which Import again
/// can take: [`STALLED_TEXT`]) and `import_moved_at` (when an import last moved, unix seconds; null when the
/// request isn't importing).
pub fn row_fields(st: &impl Ctx, r: &Req) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    let importing = r.s("status") == "importing";
    m.insert("import_stalled".into(), json!(importing && has_import(r) && is_stalled(st, r)));
    m.insert("import_moved_at".into(), if importing { json!(moved_at(st, r)) } else { Value::Null });
    m
}

/// Import again on a stalled import (`POST /api/requests/{id}/import-again` on an `importing` request, `who`
/// asked): its task is stopped and its place in the queue held (nothing starts on the old state meanwhile),
/// and in that place the kept files are imported at once; a Soulseek or torrent download whose import had not
/// kept its files yet is looked at again where it is. Nothing is downloaded again and no attempt is counted.
/// Err: (HTTP status, why not) – 409 while the import still moves.
pub async fn import_again<C: Ctx>(st: &C, id: i64, who: &str) -> Result<(), (u16, String)> {
    let Some(r) = get(st, id) else { return Err((404, "Request not found".into())) };
    if r.s("status") != "importing" {
        return Err((400, "Only an import can be imported again here".into()));
    }
    if !has_import(&r) {
        return Err((400, "There are no downloaded files to import".into()));
    }
    let running = st.requests().tasks.taken(id);
    if !is_stalled(st, &r) {
        return Err((409, "It is being imported now".into()));
    }
    let hold = queue::stop_holding(st, id);
    // read again in its held place: the import may have ended just before its task stopped
    let Some(mut r) = get(st, id).filter(|r| r.s("status") == "importing") else {
        return Err((409, "The import ended meanwhile".into()));
    };
    let why = if running { "the import hadn't moved for 15 minutes" } else { "the import had stopped" };
    log_push(&mut r, format!("Import again by {who}: {why} – it was stopped and started again"));
    restart_import(st, &mut r);
    st.db().log("info", &format!("{who} asked to import request #{id} ({}) again: {why}", r.label()));
    let st2 = st.clone();
    drop(queue::take_turn(st, id, hold, async move { queue::step(st2, id).await }));
    Ok(())
}

/// An import starts again: from its kept files (a wait for its import that is due now), or – a Soulseek or
/// torrent download whose import had not kept its files yet – by looking at the finished download again.
fn restart_import(st: &impl Ctx, r: &mut Req) {
    if r.d["kept"].is_object() {
        let t = now();
        let since = r.d["wait"]["since"].as_f64().unwrap_or(t);
        r.d["wait"] = json!({ "reason": storage::STORAGE_IMPORT, "since": since, "next_try_at": t, "error": r.d["wait"]["error"].clone(), "dir": r.d["wait"]["dir"].clone() });
        r.d["kept"]["reason"] = json!("storage");
        upd(st, r, &[("status", json!("waiting")), ("progress", json!(100)), ("message", json!("Waiting to import the downloaded files"))]);
    } else {
        upd(st, r, &[("status", json!("downloading")), ("message", json!("Importing again"))]);
    }
}

/// The watchdog, one look: every request that reads `searching` or `importing` while no task works on it
/// starts again – an import from its kept files (or its download client's folder), anything else from the
/// start the way a server restart does ([`queue::restart_detail`]: what was asked for and what MusicBrainz
/// said stay). Each one's place in the queue is held while it is written. → how many started again
pub fn watchdog(st: &impl Ctx) -> usize {
    let mut rows: Vec<i64> = st.db().query("SELECT id FROM requests WHERE status IN ('searching','importing') ORDER BY id", p![]).unwrap_or_default()
        .into_iter().map(|r| r.i("id")).collect();
    if rows.is_empty() {
        return 0;
    }
    let orphans: Vec<(i64, queue::Hold)> = {
        let mut tasks = st.requests().tasks.0.lock().unwrap_or_else(|p| p.into_inner());
        // keep the ids no live task works on, in place (the lookup borrows `tasks`, which hold_place then changes)
        rows.retain(|id| !tasks.get(id).is_some_and(|h| !h.is_finished()));
        rows.into_iter().map(|id| (id, queue::hold_place(&mut tasks, id))).collect()
    };
    let mut n = 0;
    for (id, _hold) in orphans {
        // read again in its held place: its task may have written where it is just before it ended
        let Some(mut r) = get(st, id) else { continue };
        let status = r.s("status");
        let what = match status.as_str() {
            "searching" => "search",
            "importing" => "import",
            _ => continue,
        };
        if status == "importing" && has_import(&r) {
            restart_import(st, &mut r);
        } else {
            r.d = queue::restart_detail(&r.d);
            upd(st, &mut r, &[("status", json!("queued")), ("message", json!(format!("Started again – the {what} had stopped")))]);
        }
        let line = format!("Request #{id} ({}): its {what} had stopped without saying where it was – started again", r.label());
        tracing::warn!("{line}");
        st.db().log("warning", &line);
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An import is stalled once it said and wrote nothing for 15 minutes, or at once when its task is gone;
    /// anything but an import never is.
    #[test]
    fn an_import_stalls_after_15_minutes_or_when_its_task_is_gone() {
        let t = 1_000_000.0;
        assert!(!stalled("importing", t - 60.0, true, t));
        assert!(!stalled("importing", t - STALL_SECS + 1.0, true, t));
        assert!(stalled("importing", t - STALL_SECS, true, t));
        assert!(stalled("importing", t - 3600.0, true, t));
        assert!(stalled("importing", t, false, t), "its task is gone");
        for s in ["searching", "downloading", "waiting", "queued", "completed", "failed"] {
            assert!(!stalled(s, t - 3600.0, false, t), "{s}");
        }
        assert_eq!(STALLED_TEXT, "The import hasn't moved for 15 minutes");
    }
}
