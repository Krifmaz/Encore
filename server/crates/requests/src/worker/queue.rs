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
    let res = match (status.as_str(), source.as_str()) {
        // waiting for Soulseek: go on where it stopped (the Soulseek step is first in its sources; a picked
        // file starts over from its pick)
        ("queued", "slskd" | "slskd_lossless") if r.d["soulseek_wait"].is_object() && r.d["sources"].is_array() && !r.d["pick"].is_object() => next_source(&st, &mut r, "").await,
        ("queued", _) => start(&st, &mut r).await,
        ("downloading", "slskd" | "slskd_lossless") => poll_slskd(&st, &mut r).await,
        ("downloading" | "monitoring", "torznab") => poll_torznab(&st, &mut r).await,
        ("monitoring", "album") => poll_album(&st, &mut r).await,
        _ => Ok(()), // searching/importing: handled inside a running task; stale ones are reset on startup
    };
    if let Err(e) = res {
        if e.is::<Cancelled>() {
            return;
        }
        tracing::warn!("request {id}: {e:#}");
        if let Some(mut r) = get(&st, id) {
            if ACTIVE.contains(&r.s("status").as_str()) {
                if !r.s("source").is_empty() && r.d["sources"].is_array() {
                    let _ = next_source(&st, &mut r, &format!("error: {e}")).await;
                } else {
                    fail(&st, &mut r, &e.to_string()).await;
                }
            }
        }
    }
}

/// Reset requests that were mid-task when the server stopped.
pub(super) fn recover(st: &impl Ctx) {
    for r in st.db().query("SELECT id, status, source, detail FROM requests WHERE status IN ('searching','importing','downloading')", p![]).unwrap_or_default() {
        if r.s("status") == "downloading" && ["slskd", "slskd_lossless", "torznab"].contains(&r.s("source").as_str()) {
            continue;
        }
        // what was asked for stays (length, ISRC, Deezer/Apple ids, the ticked release types), and a
        // reported song's replacement keeps what it replaces and which source to avoid
        let old: Value = serde_json::from_str(&r.s("detail")).unwrap_or(json!({}));
        let mut d = json!({});
        for k in ["replace", "avoid", "album_mode", "release_filter", "release_ids", "length", "isrc", "ext", "attempts", "log"] {

            if !old[k].is_null() {
                d[k] = old[k].clone();
            }
        }
        let _ = st.db().execute("UPDATE requests SET status='queued', message='Restarted after server restart', detail=?, updated_at=? WHERE id=?", p![d.to_string(), now(), r.i("id")]);
    }
}

pub async fn run<C: Ctx>(st: C) {
    recover(&st);
    loop {
        let ids: Vec<i64> = st.db().query(&format!("SELECT id FROM requests WHERE status IN ({}) ORDER BY id", vec!["?"; ACTIVE.len()].join(",")),
            ACTIVE.iter().map(|s| json!(s)).collect()).unwrap_or_default().into_iter().map(|r| r.i("id")).collect();
        {
            let mut tasks = st.requests().tasks.0.lock().unwrap();
            tasks.retain(|_, h| !h.is_finished());
            for id in ids {
                if tasks.contains_key(&id) {
                    continue;
                }
                let h = tokio::spawn(step(st.clone(), id));
                tasks.insert(id, h.abort_handle());
            }
        }
        crate::report::reconcile(&st); // reports whose replacement was cancelled get their song back
        let _ = st.db().execute("DELETE FROM sessions WHERE expires_at < ?", p![now()]);
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Stop work on a request (its downloads are cancelled in slskd / the download client).
pub async fn cancel(st: &impl Ctx, id: i64) {
    let Some(r) = get(st, id) else { return };
    let src = r.s("source");
    if src.starts_with("slskd") {
        let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(-1);
        if let Some(c) = r.d["slskd"]["candidates"].get(idx.max(0) as usize).filter(|_| idx >= 0) {
            slskd_cancel_current(st, c).await;
        }
    }
    if let Some(tid) = r.d["torznab"]["id"].as_str() {
        torznab::remove(st, tid).await;
    }
    if let Some(h) = st.requests().tasks.0.lock().unwrap().remove(&id) {
        h.abort();
    }
}
