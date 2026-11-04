//! The queue: every active request gets a task; a task runs the request's next step (start it, or look at
//! a download in progress), and requests that were mid-step when the server stopped start again.
use super::*;

/// Run the request's next step (start it, go on after a wait, or look at a download in progress).
pub async fn step<C: Ctx>(st: C, id: i64) {
    let Some(mut r) = get(&st, id) else { return };
    let status = r.s("status");
    if !ACTIVE.contains(&status.as_str()) {
        return;
    }
    let source = r.s("source");
    let reason = r.d["wait"]["reason"].as_str().unwrap_or("").to_string();
    let res = match (status.as_str(), source.as_str()) {
        // waiting for the storage: the import (or the staging folder's source step) again once it's due
        ("waiting", _) if reason == storage::STORAGE_IMPORT || reason == storage::STORAGE_DOWNLOAD => storage::retry(&st, &mut r).await,
        // waiting for a downloads folder: on once the probe finds it writable again
        ("waiting", _) if reason == downloads::REASON => downloads::retry(&st, &mut r).await,
        // waiting for Soulseek: go on where it stopped (the Soulseek step is first in its sources; a picked
        // file starts over from its pick). A request parked by an older version is still `queued`.
        ("waiting", _) | ("queued", "slskd" | "slskd_lossless") if r.d["soulseek_wait"].is_object() && r.d["sources"].is_array() && !r.d["pick"].is_object() => next_source(&st, &mut r, "").await,
        // a picked Soulseek file asked for again
        ("waiting", _) if r.d["soulseek_wait"].is_object() && r.d["pick"].is_object() => start(&st, &mut r).await,
        // waiting for a plugin the upgrade is still installing: a source step goes on where it stopped (it is
        // first in its sources), a link or a pick starts again – each parks again while the install runs
        ("waiting", _) if reason == storage::PLUGIN_INSTALL && r.d["install_wait"]["step"] == "source" && r.d["sources"].is_array() => next_source(&st, &mut r, "").await,
        ("waiting", _) if reason == storage::PLUGIN_INSTALL => start(&st, &mut r).await,
        // never a new download for a waiting request
        ("waiting", _) => Ok(()),
        ("queued", _) => start(&st, &mut r).await,
        ("downloading", "slskd" | "slskd_lossless") => poll_slskd(&st, &mut r).await,
        ("downloading" | "monitoring", "torznab") => poll_torznab(&st, &mut r).await,
        ("monitoring", "album") => poll_album(&st, &mut r).await,
        _ => Ok(()), // searching/importing: handled inside a running task; stale ones are reset on startup
    };
    if let Err(e) = res {
        on_error(&st, id, e).await;
    }
}

/// A step (or Download now's work in the request's place) ended with an error: a storage error waits for the
/// storage, another goes on to the next source, or the request fails.
pub(super) async fn on_error(st: &impl Ctx, id: i64, e: anyhow::Error) {
    if e.is::<Cancelled>() {
        return;
    }
    tracing::warn!("request {id}: {e:#}");
    if let Some(mut r) = get(st, id) {
        if ACTIVE.contains(&r.s("status").as_str()) {
            storage::register_roots(st);
            if let Some(se) = encore_core::fsx::storage(&e) {
                // a storage error that came this far (a download's folder): the step waits, it didn't fail
                let reason = if r.d["kept"].is_object() { storage::STORAGE_IMPORT } else { storage::STORAGE_DOWNLOAD };
                let _ = storage::wait_for_storage(st, &mut r, se, reason);
            } else if !r.s("source").is_empty() && r.d["sources"].is_array() {
                let _ = next_source(st, &mut r, &format!("error: {}", encore_core::fsx::explain(&e))).await;
            } else {
                fail(st, &mut r, &encore_core::fsx::explain(&e)).await;
            }
        }
    }
}

/// Reset requests that were mid-task when the server stopped. Downloaded files are never thrown away for
/// it: an import that was running goes back to waiting for its import ([`storage::recover`], which also
/// rescues failed requests whose files are still there), and a Soulseek or torrent download that was being
/// imported is looked at again where it is (its files are in the client's folder).
pub fn recover(st: &impl Ctx) {
    storage::register_roots(st);
    storage::recover(st);
    // (a request that waited in a source's line reads `queued` with `detail.in_line`: it starts over too)
    for r in st.db().query("SELECT id, status, source, detail FROM requests WHERE status IN ('searching','importing','downloading')
                            OR (status='queued' AND json_extract(detail, '$.in_line') IS NOT NULL)", p![]).unwrap_or_default() {
        let client = ["slskd", "slskd_lossless", "torznab"].contains(&r.s("source").as_str());
        if r.s("status") == "downloading" && client {
            continue;
        }
        if r.s("status") == "importing" && client {
            let _ = st.db().execute("UPDATE requests SET status='downloading', message='Importing again after a server restart', updated_at=? WHERE id=?", p![now(), r.i("id")]);
            continue;
        }
        let old: Value = serde_json::from_str(&r.s("detail")).unwrap_or_else(|_| json!({}));
        let _ = st.db().execute("UPDATE requests SET status='queued', message='Restarted after server restart', detail=?, updated_at=? WHERE id=?",
            p![restart_detail(&old).to_string(), now(), r.i("id")]);
    }
}

/// What a request starting over keeps of its `detail`: what was asked for stays (length, ISRC, Deezer/Apple
/// ids, the ticked release types), and a reported song's replacement keeps what it replaces and which source
/// to avoid. Downloaded files kept for their import stay too (the import runs again, nothing is downloaded),
/// so do the downloads the audio check turned away and a check a person turned off. What MusicBrainz already
/// said stays as well – which album the song is on (or that it is on none), its recording, the album's track
/// list – so a restart doesn't ask it all again at once (a few hundred lookups in the line after an
/// upgrade); a lossless upgrade stays one (without `lossless_only` it found the lossy copy in the library and
/// ended "Already in library"), a hi-res upgrade stays one (`hires_only`), and an auto-download stays one.
pub(super) fn restart_detail(old: &Value) -> Value {
    let mut d = json!({});
    for k in ["replace", "avoid", "album_mode", "release_filter", "release_ids", "length", "isrc", "ext", "attempts", "log", "pick", "kept",
              "rejected", "rejected_seq", "skip_check", "no_album", "recording", "album_checked_at", "album_lookup", "album_note", "release", "lossless_only", "hires_only", "auto", "type"] {
        if !old[k].is_null() {
            d[k] = old[k].clone();
        }
    }
    d
}

/// The request worker: picks up what was running at start, then runs the queue forever.
pub async fn run<C: Ctx>(st: C) {
    recover(&st);
    loop {
        let _ = storage::tick(&st); // a paused import's health check every 10 minutes, started and not waited for
        let _ = rejected::tick(&st); // rejected downloads past their 7 days go, once an hour, not waited for
        // the watchdog: a search or an import whose task is gone starts again now, not at the next restart
        stall::watchdog(&st);
        let ids: Vec<i64> = st.db().query(&format!("SELECT id FROM requests WHERE status IN ({}) ORDER BY id", vec!["?"; ACTIVE.len()].join(",")),
            ACTIVE.iter().map(|s| json!(s)).collect()).unwrap_or_default().into_iter().map(|r| r.i("id")).collect();
        // the download folders' probe: every minute while requests are active, started and not waited for
        let _ = downloads::tick(&st, !ids.is_empty());
        // finished downloads in Encore's category that Encore didn't start: every two minutes, not waited for
        outside::tick(&st);
        // Download now's presses last while their request is active
        st.requests().lines.retain(&ids.iter().copied().collect());
        {
            let mut tasks = st.requests().tasks.0.lock().unwrap();
            tasks.retain(|_, h| !h.is_finished());
            for id in ids {
                if tasks.contains_key(&id) {
                    continue;
                }
                // a request's MusicBrainz lookups wait behind pages and imports (encore_http::mb)
                let h = tokio::spawn(mb::with_priority(mb::Priority::Request, step(st.clone(), id)));
                tasks.insert(id, h.abort_handle());
            }
        }
        crate::report::reconcile(&st); // reports whose replacement was cancelled get their song back
        let _ = st.db().execute("DELETE FROM sessions WHERE expires_at < ?", p![now()]);
        // the next look in 5 s, or at once when Download now asks for it
        let wake = st.requests().wake.clone();
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            _ = wake.notified() => {}
        }
    }
}

/// Wait up to `max` for request `id`'s running step to end (a step of a waiting or downloading request only
/// looks and returns), then hold its place in the queue so no step starts on it while the caller writes
/// (Download now). None: its step was still running after `max`.
pub async fn hold_idle(st: &impl Ctx, id: i64, max: std::time::Duration) -> Option<Hold> {
    let until = tokio::time::Instant::now() + max;
    loop {
        {
            let mut tasks = st.requests().tasks.0.lock().unwrap_or_else(|p| p.into_inner());
            if !tasks.get(&id).is_some_and(|h| !h.is_finished()) {
                let h = tokio::spawn(std::future::pending::<()>()).abort_handle();
                tasks.insert(id, h.clone());
                return Some(Hold(Some(h)));
            }
        }
        if tokio::time::Instant::now() >= until {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Run `work` (the request's step, or what Download now does) as request `id`'s task now, in the place
/// `hold` kept: a cancel stops it like any step, and the queue starts no other step for it meanwhile.
pub fn take_turn<F>(st: &impl Ctx, id: i64, hold: Hold, work: F) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let mut tasks = st.requests().tasks.0.lock().unwrap_or_else(|p| p.into_inner());
    let h = tokio::spawn(mb::with_priority(mb::Priority::Request, work));
    tasks.insert(id, h.abort_handle());
    drop(tasks);
    // the held place's own task ends; the map holds the new one
    drop(hold);
    h
}

/// Stop work on a request: a download in progress is cancelled in slskd / the download client, and
/// Encore's own staging folder of the request goes. Finished files that wait in a download client's folder
/// for their import are never deleted (the client's folder is the client's).
pub async fn cancel(st: &impl Ctx, id: i64) {
    drop(stop(st, id, false).await);
}

/// A request's place in the queue, held by a task that never ends ([`cancel_holding`]): the queue starts
/// no step for the request while it is there. The place is given back when this goes – after the new state
/// is written, and also when the caller returns early or its connection closes.
pub struct Hold(Option<tokio::task::AbortHandle>);

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(h) = self.0.take() {
            h.abort();
        }
    }
}

/// Stop request `id`'s task and hold its place in the queue until the returned [`Hold`] goes, touching
/// nothing else: no download is cancelled and no folder removed (Import again on an import that hasn't
/// moved: its kept files are imported again in that place).
pub fn stop_holding(st: &impl Ctx, id: i64) -> Hold {
    let mut tasks = st.requests().tasks.0.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(h) = tasks.remove(&id) {
        h.abort();
    }
    hold_place(&mut tasks, id)
}

/// Hold request `id`'s place in the running tasks (`tasks`, locked by the caller) with a task that never ends.
pub(super) fn hold_place(tasks: &mut HashMap<i64, tokio::task::AbortHandle>, id: i64) -> Hold {
    let h = tokio::spawn(std::future::pending::<()>()).abort_handle();
    tasks.insert(id, h.clone());
    Hold(Some(h))
}

/// [`cancel`] for whoever writes the request's new state next (Keep anyway, Download anyway, a pick, a
/// cancel): its place in the queue stays taken from the moment its task stops until the returned [`Hold`]
/// goes. Without it the queue could start a step on the old state in between, which would find the
/// download stopped here and start the next one (another user's files, another release) that nothing then
/// looks after.
pub async fn cancel_holding(st: &impl Ctx, id: i64) -> Hold {
    stop(st, id, true).await
}

/// Stop the request's task and its download (`hold`: keep its place taken; see [`cancel_holding`]).
async fn stop(st: &impl Ctx, id: i64, hold: bool) -> Hold {
    let Some(r) = get(st, id) else { return Hold(None) };
    let held = {
        let mut tasks = st.requests().tasks.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(h) = tasks.remove(&id) {
            h.abort();
        }
        hold.then(|| {
            let h = tokio::spawn(std::future::pending::<()>()).abort_handle();
            tasks.insert(id, h.clone());
            h
        })
    };
    let kept = r.d["kept"].is_object();
    let src = r.s("source");
    if src.starts_with("slskd") && !kept {
        let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(-1);
        if let Some(c) = r.d["slskd"]["candidates"].get(idx.max(0) as usize).filter(|_| idx >= 0) {
            slskd_cancel_current(st, c).await;
        }
    }
    if let (Some(tid), false) = (r.d["torznab"]["id"].as_str(), kept) {
        torznab::remove(st, tid).await;
    }
    storage::remove_staging(st, id);
    Hold(held)
}
