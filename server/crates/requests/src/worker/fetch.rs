//! Fetching: each source's search, download and the polling of downloads in progress (Soulseek, torrents
//! and Usenet through the download clients, streamrip, yt-dlp).
use super::*;

pub(super) fn q(parts: &[&str]) -> String {
    static BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[].*?[\)\]]").unwrap());
    static NW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\s']").unwrap());
    let s = parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" ");
    let s = BR.replace_all(&s, " ");
    NW.replace_all(&s, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A request's message while its step runs, without touching the rest of the row (the worker's search
/// says "Searching Soulseek…" once its turn comes).
fn note_message(st: &impl Ctx, id: i64, msg: &str) {
    let _ = st.db().execute("UPDATE requests SET message=?, updated_at=? WHERE id=? AND status='searching'", p![msg, now(), id]);
    crate::events::emit(st, id);
}

/// One Soulseek search for a request: it waits its turn behind the other searches (one at a time) and
/// says so while it waits, and while it sits out a pause of the server's.
async fn slskd_search(st: &impl Ctx, id: i64, query: &str, searching: &'static str) -> anyhow::Result<Vec<Value>> {
    if st.requests().slskd.session().searching() {
        note_message(st, id, "Waiting for another Soulseek search…");
    }
    slskd::search_as(st, query, slskd::Who::Worker, |pause| note_message(st, id, pause.unwrap_or(searching))).await
}

pub(super) async fn start_slskd(st: &impl Ctx, r: &mut Req, lossless_only: bool) -> anyhow::Result<Out> {
    let searching = if lossless_only { "Searching Soulseek for lossless…" } else { "Searching Soulseek…" };
    upd(st, r, &[("status", json!("searching")), ("message", json!(searching))]);
    let mut cands = vec![];
    if r.s("kind") == "album" {
        let rel = release_info(st, r).await;
        let album = r.album_title();
        for query in [q(&[&r.s("artist"), &album]), q(&[&album])] {
            let res = slskd_search(st, r.id, &query, searching).await?;
            cands = slskd::rank_album(st, &res, &r.s("artist"), &album, n_tracks(&rel), lossless_only);
            if !cands.is_empty() {
                break;
            }
        }
    } else {
        let res = slskd_search(st, r.id, &q(&[&r.s("artist"), &r.s("title")]), searching).await?;
        cands = slskd::rank_track(st, &res, &r.s("artist"), &r.s("title"), r.length(), lossless_only);
    }
    check(st, r.id)?;
    // it searched: a later wait for Soulseek starts its own hour
    if let Some(d) = r.d.as_object_mut() {
        d.remove("soulseek_wait");
    }
    let tried: Vec<String> = r.d["slskd_tried"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    cands.retain(|c| !tried.contains(&format!("{}|{}", c["username"].as_str().unwrap_or(""), c["directory"].as_str().unwrap_or(""))));
    // reported files are never picked again; a replacement tries other users first
    cands.retain(|c| !crate::report::slskd_blocked(st, c));
    crate::report::avoid_users(&r.d, &mut cands);
    if cands.is_empty() {
        return Ok(Out::Next(if lossless_only { "no lossless results on Soulseek" } else { "no suitable results on Soulseek" }.into()));
    }
    r.d["slskd"] = json!({ "candidates": cands, "idx": -1 });
    slskd_try_next(st, r, "").await
}

pub(super) async fn slskd_cancel_current(st: &impl Ctx, c: &Value) {
    let wanted: Vec<&str> = c["files"].as_array().map(|a| a.iter().filter_map(|f| f["filename"].as_str()).collect()).unwrap_or_default();
    let user = c["username"].as_str().unwrap_or("");
    for f in slskd::user_downloads(st, user).await {
        if wanted.contains(&f["filename"].as_str().unwrap_or("")) && !f["state"].as_str().unwrap_or("").starts_with("Completed") {
            slskd::cancel(st, user, &f["id"].as_str().map(String::from).unwrap_or(f["id"].to_string())).await;
        }
    }
}

/// How long a download slskd is too busy to take is asked for again before the user counts as failed.
const ENQUEUE_BUSY_MAX: f64 = 600.0;

/// The candidate's key in `slskd_tried`: "user|folder".
fn slskd_key(c: &Value) -> String {
    format!("{}|{}", c["username"].as_str().unwrap_or(""), c["directory"].as_str().unwrap_or(""))
}

/// Hand the next candidate's files to slskd (the current one failed for `reason`, or there is none yet).
/// A download slskd didn't take for reasons of its own is not the user's failure: while slskd is busy
/// taking other downloads the same candidate is asked for again at the next look (`pending`), and when the
/// session is down the request waits for Soulseek (Err([`slskd::NotConnected`])) – also the candidate
/// whose transfer just failed is not counted as tried then, since the session ending is what ends the
/// transfers.
pub(super) async fn slskd_try_next(st: &impl Ctx, r: &mut Req, reason: &str) -> anyhow::Result<Out> {
    let mut reason = reason.to_string();
    // the current candidate's download was never taken: ask for it again, it didn't fail
    let mut again = r.d["slskd"]["pending"].is_object();
    // the candidate marked tried by this call because its transfer failed
    let mut marked: Option<String> = None;
    let mut first = true;
    loop {
        let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(-1);
        let cands = r.d["slskd"]["candidates"].as_array().cloned().unwrap_or_default();
        let idx = if again {
            idx.max(0)
        } else {
            if let Some(c) = cands.get(idx.max(0) as usize).filter(|_| idx >= 0) {
                let key = slskd_key(c);
                if !r.d["slskd_tried"].is_array() {
                    r.d["slskd_tried"] = json!([]);
                }
                r.d["slskd_tried"].as_array_mut().unwrap().push(json!(key));
                if first {
                    marked = Some(key);
                }
                slskd_cancel_current(st, c).await;
            }
            idx + 1
        };
        let pending_since = if again { r.d["slskd"]["pending"]["since"].as_f64() } else { None };
        again = false;
        first = false;
        r.d["slskd"]["idx"] = json!(idx);
        if let Some(o) = r.d["slskd"].as_object_mut() {
            o.remove("pending");
        }
        let Some(c) = cands.get(idx as usize).cloned() else {
            return Ok(Out::Next(if reason.is_empty() { "all Soulseek candidates failed".into() } else { reason }));
        };
        let user = c["username"].as_str().unwrap_or("").to_string();
        if let Err(e) = slskd::enqueue(st, &user, c["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[])).await {
            if e.is::<slskd::NotConnected>() || e.is::<crate::slskd_managed::Unreachable>() {
                // the session (or slskd itself, restarting) is gone, not this user: the supervisor looks at
                // once, and the request waits for Soulseek like a search that finds it down
                let down = st.requests().slskd.session().search_found_it_down();
                r.d["slskd"]["pending"] = json!({ "since": pending_since.unwrap_or(now()) });
                if let (Some(k), Some(tried)) = (marked.take(), r.d["slskd_tried"].as_array_mut()) {
                    if let Some(i) = tried.iter().rposition(|t| t.as_str() == Some(k.as_str())) {
                        tried.remove(i);
                    }
                }
                return Err(if e.is::<slskd::NotConnected>() { e } else { down.into() });
            }
            let since = pending_since.unwrap_or(now());
            if e.is::<slskd::EnqueueBusy>() && now() - since < ENQUEUE_BUSY_MAX {
                // slskd is taking other downloads: this one is asked for again at the next look
                r.d["slskd"]["pending"] = json!({ "since": since });
                upd(st, r, &[("status", json!("downloading")), ("progress", json!(0)),
                             ("message", json!(format!("Waiting for Soulseek to take the download from {user} (it is busy with other downloads)")))]);
                return Ok(Out::Wait);
            }
            reason = e.to_string();
            continue;
        }
        if let Some(d) = r.d.as_object_mut() {
            d.remove("soulseek_wait"); // a later wait for Soulseek starts its own hour
        }
        let t = now();
        r.d["slskd"]["started"] = json!(t);
        r.d["slskd"]["last_change"] = json!(t);
        r.d["slskd"]["last_bytes"] = json!(0);
        let br = c["bitrate"].as_i64().unwrap_or(0);
        let msg = format!("Downloading {} file(s) from {user} ({}{})", c["files"].as_array().map(|a| a.len()).unwrap_or(0),
            c["format"].as_str().unwrap_or("").to_uppercase(), if br > 0 { format!(" {br}k") } else { String::new() });
        upd(st, r, &[("status", json!("downloading")), ("progress", json!(0)), ("message", json!(msg))]);
        return Ok(Out::Wait);
    }
}

pub(super) async fn after_slskd(st: &impl Ctx, r: &mut Req, out: Out) -> anyhow::Result<()> {
    if let Out::Next(why) = out {
        next_source(st, r, &why).await?;
    }
    Ok(())
}

/// Look at a Soulseek download in progress. When the session is down – the transfers end with it – the
/// request waits for Soulseek instead of going on to the next user or source ([`slskd_down`]).
pub(super) async fn poll_slskd(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    match poll_slskd_once(st, r).await {
        Err(e) if e.is::<slskd::NotConnected>() => slskd_down(st, r).await,
        other => other,
    }
}

/// A Soulseek step found the session down (a download slskd couldn't take, a transfer that ended with
/// the session): the request waits for Soulseek where it is, as a search that finds it down does (the
/// 60-minute rule applies), or goes on when Soulseek won't be back by itself (off, a rejected login).
pub(super) async fn slskd_down(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    match soulseek_turn(st, r).await {
        Turn::Skip(why) => next_source(st, r, &why).await,
        Turn::Wait(when) => park_download(st, r, &when),
        // the answer said down, the session doesn't know yet (the supervisor looks at once)
        Turn::Go => park_download(st, r, "connecting shortly"),
    }
}

/// The transfer can't go on: is that because the session is down? Then the request waits for Soulseek
/// (true) and the user isn't counted as failed.
async fn parked_for_the_session(st: &impl Ctx, r: &mut Req) -> anyhow::Result<bool> {
    match soulseek_turn(st, r).await {
        Turn::Wait(when) => park_download(st, r, &when).map(|_| true),
        Turn::Go | Turn::Skip(_) => Ok(false),
    }
}

async fn poll_slskd_once(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    if r.d["slskd"]["pending"].is_object() {
        // slskd didn't take the download yet (it was busy): ask again
        let o = slskd_try_next(st, r, "").await?;
        return after_slskd(st, r, o).await;
    }
    let idx = r.d["slskd"]["idx"].as_i64().unwrap_or(-1);
    let Some(c) = r.d["slskd"]["candidates"].get(idx.max(0) as usize).cloned() else {
        return next_source(st, r, "lost track of the Soulseek download").await;
    };
    let user = c["username"].as_str().unwrap_or("").to_string();
    let wanted: Vec<String> = c["files"].as_array().map(|a| a.iter().filter_map(|f| f["filename"].as_str().map(String::from)).collect()).unwrap_or_default();
    // one entry per file: re-queued files show up again, the newest counts
    let mut latest: HashMap<String, Value> = HashMap::new();
    for f in slskd::user_downloads(st, &user).await {
        let n = f["filename"].as_str().unwrap_or("").to_string();
        if wanted.contains(&n) {
            latest.insert(n, f);
        }
    }
    let t = now();
    if latest.is_empty() {
        // the transfer list can be cleared from the Soulseek page: finished files on disk still count
        if !wanted.is_empty() && wanted.iter().all(|n| slskd::local_path(st, n).is_some()) {
            upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Importing…"))]);
            return import_slskd(st, r, &c).await;
        }
        if t - r.d["slskd"]["started"].as_f64().unwrap_or(t) > 120.0 {
            if parked_for_the_session(st, r).await? {
                return Ok(());
            }
            let o = slskd_try_next(st, r, "transfers disappeared").await?;
            return after_slskd(st, r, o).await;
        }
        return Ok(());
    }
    let states: Vec<String> = latest.values().map(|f| f["state"].as_str().unwrap_or("").to_string()).collect();
    if states.iter().any(|s| slskd::DONE_BAD.iter().any(|b| s.contains(b))) {
        if parked_for_the_session(st, r).await? {
            return Ok(());
        }
        st.db().log("warning", &format!("Soulseek transfer from {user} failed for {}; trying next source", r.label()));
        let o = slskd_try_next(st, r, "transfer failed").await?;
        return after_slskd(st, r, o).await;
    }
    let total: f64 = latest.values().map(|f| f["size"].as_f64().unwrap_or(0.0)).sum::<f64>().max(1.0);
    let got: f64 = latest.values().map(|f| f["bytesTransferred"].as_f64().unwrap_or(0.0)).sum();
    if got > r.d["slskd"]["last_bytes"].as_f64().unwrap_or(0.0) {
        r.d["slskd"]["last_bytes"] = json!(got);
        r.d["slskd"]["last_change"] = json!(t);
    }
    let done = states.iter().filter(|s| s.contains(slskd::DONE_OK)).count();
    if done == wanted.len() {
        upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Importing…"))]);
        return import_slskd(st, r, &c).await;
    }
    if t - r.d["slskd"]["last_change"].as_f64().unwrap_or(t) > st.cfg().int("slskd_stall_minutes") as f64 * 60.0 {
        if parked_for_the_session(st, r).await? {
            return Ok(());
        }
        let o = slskd_try_next(st, r, "transfer stalled").await?;
        return after_slskd(st, r, o).await;
    }
    let queued = states.iter().all(|s| s.starts_with("Queued"));
    upd(st, r, &[("progress", json!((got / total * 1000.0).round() / 10.0)),
                 ("message", json!(format!("{} {user} – {done}/{} files", if queued { "Queued by" } else { "Downloading from" }, wanted.len())))]);
    Ok(())
}

pub(super) async fn import_slskd(st: &impl Ctx, r: &mut Req, c: &Value) -> anyhow::Result<()> {
    let names: Vec<String> = c["files"].as_array().map(|a| a.iter().filter_map(|f| f["filename"].as_str().map(String::from)).collect()).unwrap_or_default();
    let mut paths = vec![];
    for _ in 0..6 {
        let found: Vec<Option<PathBuf>> = names.iter().map(|n| slskd::local_path(st, n)).collect();
        let all = found.iter().all(|p| p.is_some());
        paths = found.into_iter().flatten().collect();
        if all {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    let root = crate::slskd_managed::download_dir(st);
    if paths.is_empty() {
        fail(st, r, &format!("Downloaded files not found under {} – check the shared volume mapping", root.display())).await;
        return Ok(());
    }
    let user = c["username"].as_str().unwrap_or("");
    match import(st, r, &paths, &root).await {
        Ok((new, note)) => {
            let mut origin = json!({ "source": "Soulseek", "detail": format!("user {user} · {}", c["directory"].as_str().unwrap_or("").replace('\\', "/")) });
            if let Some((kind, key, label)) = crate::report::block_of_slskd(c, r.s("kind") == "album") {
                origin["block"] = json!({ "kind": kind, "key": key, "label": label });
            }
            complete(st, r, &with_note(&format!("Downloaded from Soulseek ({})", c["format"].as_str().unwrap_or("").to_uppercase()), &note), &new, origin).await;
            Ok(())
        }
        Err(e) if e.is::<Rejected>() => {
            for p in &paths {
                let _ = std::fs::remove_file(p);
                encore_library::prune_empty(p.parent().unwrap(), &root);
            }
            st.db().log("warning", &format!("Rejected download from {user} for {}: {e}", r.label()));
            upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("Rejected {user}'s files: {e}")))]);
            let o = slskd_try_next(st, r, &format!("wrong audio ({e})")).await?;
            after_slskd(st, r, o).await
        }
        Err(e) => Err(e),
    }
}

pub(super) async fn start_torznab(st: &impl Ctx, r: &mut Req, lossless_only: bool) -> anyhow::Result<Out> {
    let lossless_only = lossless_only || r.d["lossless_only"].as_bool().unwrap_or(false);
    upd(st, r, &[("status", json!("searching")), ("message", json!("Searching torrent and Usenet indexers (Prowlarr)…"))]);
    let album = r.album_title();
    let mut cands = vec![];
    for query in [q(&[&r.s("artist"), &album]), q(&[&album])] {
        let res = torznab::search(st, &query).await?;
        cands = torznab::rank_album(st, &res, &r.s("artist"), &album, lossless_only);
        if !cands.is_empty() {
            break;
        }
    }
    check(st, r.id)?;
    cands.retain(|c| !crate::report::release_blocked(st, c)); // reported releases are never picked again
    if cands.is_empty() {
        return Ok(Out::Next("no matching release on your indexers".into()));
    }
    r.d["torznab"] = json!({ "candidates": cands, "idx": -1 });
    torznab_try_next(st, r, "").await
}

pub(super) async fn torznab_try_next(st: &impl Ctx, r: &mut Req, reason: &str) -> anyhow::Result<Out> {
    let mut reason = reason.to_string();
    if let Some(id) = r.d["torznab"]["id"].as_str().map(String::from) {
        torznab::remove(st, &id).await;
        r.d["torznab"]["id"] = Value::Null;
    }
    // the song's file in the last release says nothing about the next one
    if r.d["torznab"].is_object() {
        r.d["torznab"]["selected"] = Value::Null;
    }
    loop {
        let idx = r.d["torznab"]["idx"].as_i64().unwrap_or(-1) + 1;
        r.d["torznab"]["idx"] = json!(idx);
        let Some(c) = r.d["torznab"]["candidates"].get(idx as usize).cloned() else {
            return Ok(Out::Next(if reason.is_empty() { "no indexer release worked".into() } else { reason }));
        };
        match torznab::add(st, &c, r.id).await {
            Ok(id) => {
                let t = now();
                r.d["torznab"]["id"] = json!(id);
                r.d["torznab"]["started"] = json!(t);
                r.d["torznab"]["last_change"] = json!(t);
                r.d["torznab"]["last_progress"] = json!(0);
                let via = torznab::client_name(st, c["protocol"].as_str().unwrap_or("torrent"));
                upd(st, r, &[("status", json!("downloading")), ("progress", json!(0)),
                             ("message", json!(format!("{via}: {} ({})", c["title"].as_str().unwrap_or(""), c["indexer"].as_str().unwrap_or(""))))]);
                return Ok(Out::Wait);
            }
            Err(e) => reason = e.to_string(),
        }
    }
}

pub(super) async fn poll_torznab(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let Some(id) = r.d["torznab"]["id"].as_str().map(String::from) else {
        let o = torznab_try_next(st, r, "lost track of the download").await?;
        return after_slskd(st, r, o).await;
    };
    let t = now();
    // a song from a release with several files: download only the song's file
    if r.s("kind") == "track" && r.d["torznab"]["selected"].is_null() {
        let cand = r.d["torznab"]["candidates"][r.d["torznab"]["idx"].as_u64().unwrap_or(0) as usize].clone();
        let track_no = cand["track_no"].as_u64().map(|n| n as u32);
        match torznab::select_song(st, &id, &r.s("artist"), &r.s("title"), track_no).await {
            Ok(torznab::SongFile::Pending) => {}
            Ok(torznab::SongFile::Whole) => r.d["torznab"]["selected"] = json!(false),
            Ok(torznab::SongFile::Only(name)) => {
                log_push(r, format!("torznab: downloading only {name}"));
                r.d["torznab"]["selected"] = json!(name);
            }
            Ok(torznab::SongFile::Missing) => {
                let o = torznab_try_next(st, r, "the song isn't in this release").await?;
                return after_slskd(st, r, o).await;
            }
            Err(e) => {
                // the whole release downloads; only the song is imported
                log_push(r, format!("torznab: couldn't select the song's file ({e})"));
                r.d["torznab"]["selected"] = json!(false);
            }
        }
    }
    match torznab::poll(st, &id).await? {
        torznab::Poll::Progress(pct, msg) => {
            if pct > r.d["torznab"]["last_progress"].as_f64().unwrap_or(0.0) {
                r.d["torznab"]["last_progress"] = json!(pct);
                r.d["torznab"]["last_change"] = json!(t);
            }
            if t - r.d["torznab"]["last_change"].as_f64().unwrap_or(t) > st.cfg().int("torznab_timeout_minutes") as f64 * 60.0 {
                let o = torznab_try_next(st, r, "download stalled").await?;
                return after_slskd(st, r, o).await;
            }
            upd(st, r, &[("progress", json!(pct)), ("message", json!(msg))]);
            Ok(())
        }
        torznab::Poll::Failed(why) => {
            let o = torznab_try_next(st, r, &why).await?;
            after_slskd(st, r, o).await
        }
        torznab::Poll::Done(path) => {
            upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
            let mut files = if path.is_file() { vec![path.clone()] } else { streamrip::audio_files(&path, &encore_core::audio::DOWNLOAD_EXTS) };
            if files.is_empty() {
                let o = torznab_try_next(st, r, "the download had no audio files").await?;
                return after_slskd(st, r, o).await;
            }
            let root = path.parent().map(|p| p.to_path_buf()).unwrap_or(path.clone());
            let cand = r.d["torznab"]["candidates"][r.d["torznab"]["idx"].as_u64().unwrap_or(0) as usize].clone();
            if r.s("kind") == "track" && files.len() > 1 {
                // only the song is imported; the rest goes with the download when it is removed
                let names: Vec<String> = files.iter().map(|f| f.strip_prefix(&path).unwrap_or(f).to_string_lossy().to_string()).collect();
                match torznab::pick_file(&names, &r.s("artist"), &r.s("title"), cand["track_no"].as_u64().map(|n| n as u32)) {
                    Some(i) => files = vec![files[i].clone()],
                    None => {
                        let o = torznab_try_next(st, r, "the song isn't in this release").await?;
                        return after_slskd(st, r, o).await;
                    }
                }
            }
            match import(st, r, &files, &root).await {
                Ok((new, _)) => {
                    torznab::remove(st, &id).await;
                    let via = if cand["protocol"] == "usenet" { "Usenet" } else { "torrent" };
                    let mut origin = json!({ "source": cand["indexer"].as_str().unwrap_or(via), "detail": format!("{via}: {}", cand["title"].as_str().unwrap_or("")) });
                    if let Some((kind, key, label)) = crate::report::block_of_release(&cand) {
                        origin["block"] = json!({ "kind": kind, "key": key, "label": label });
                    }
                    complete(st, r, &format!("Downloaded via {via} ({})", cand["indexer"].as_str().unwrap_or("Prowlarr")), &new, origin).await;
                    Ok(())
                }
                Err(e) if e.is::<Rejected>() => {
                    st.db().log("warning", &format!("Rejected {} for {}: {e}", cand["title"].as_str().unwrap_or(""), r.label()));
                    let o = torznab_try_next(st, r, &format!("wrong audio ({e})")).await?;
                    after_slskd(st, r, o).await
                }
                Err(e) => Err(e),
            }
        }
    }
}

pub(super) async fn isrc_for(st: &impl Ctx, r: &mut Req) -> Option<String> {
    if let Some(i) = r.d["isrc"].as_str() {
        return Some(i.to_string());
    }
    if r.s("kind") == "track" && !r.s("mbid").is_empty() {
        if let Ok(rec) = mb::recording(st, &r.s("mbid")).await {
            r.d["isrc"] = rec["isrc"].clone();
        }
    }
    r.d["isrc"].as_str().map(String::from)
}

pub(super) async fn start_streamrip(st: &impl Ctx, r: &mut Req, urls: Option<Vec<String>>) -> anyhow::Result<Out> {
    let _slot = st.requests().streamrip.acquire().await?;
    check(st, r.id)?;
    upd(st, r, &[("status", json!("searching")), ("message", json!("Looking for an exact lossless match…"))]);
    let mut urls = urls.unwrap_or_default();
    if urls.is_empty() {
        if let (Some(dz), true) = (r.d["ext"]["deezer"].as_i64().map(|x| x.to_string()).or(r.d["ext"]["deezer"].as_str().map(String::from)), streamrip::services(st).iter().any(|s| s == "deezer")) {
            urls.push(format!("https://www.deezer.com/album/{dz}"));
        }
    }
    if urls.is_empty() {
        if r.s("kind") == "album" {
            let rel = release_info(st, r).await;
            let mut first_isrc = rel.as_ref().and_then(|x| x["tracks"].as_array()?.iter().find_map(|t| t["isrc"].as_str().map(String::from)));
            if first_isrc.is_none() {
                if let Some(id) = rel.as_ref().and_then(|x| x["id"].as_str()) {
                    if let Ok(full) = mb::release(st, id).await {
                        first_isrc = full["tracks"].as_array().and_then(|a| a.iter().find_map(|t| t["isrc"].as_str().map(String::from)));
                    }
                }
            }
            urls = streamrip::resolve_album(st, &r.s("artist"), &r.album_title(), n_tracks(&rel), first_isrc.as_deref()).await;
        } else {
            let isrc = isrc_for(st, r).await;
            urls = streamrip::resolve_track(st, &r.s("artist"), &r.s("title"), isrc.as_deref(), r.length()).await;
        }
    }
    urls.retain(|u| !crate::report::url_blocked(st, u)); // reported items are never picked again
    if urls.is_empty() {
        return Ok(Out::Next("not found on Deezer/Tidal".into()));
    }
    let mut last = String::new();
    for url in urls {
        check(st, r.id)?;
        let out = staging(st, r).join("streamrip");
        rm(&out);
        upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("streamrip: {url}")))]);
        let res: anyhow::Result<(Vec<PathBuf>, String)> = async {
            let files = streamrip::download(st, &url, &out).await?;
            upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
            import(st, r, &files, &out).await
        }.await;
        match res {
            Ok((new, note)) => {
                rm(&staging(st, r));
                let svc = if url.contains("deezer") { "Deezer" } else if url.contains("tidal") { "Tidal" } else { "Qobuz" };
                complete(st, r, &with_note(&format!("Downloaded from {svc} via streamrip"), &note), &new, json!({ "source": svc, "detail": "via streamrip", "url": url })).await;
                return Ok(Out::Done);
            }
            Err(e) if e.is::<Cancelled>() => return Err(e),
            Err(e) => {
                st.db().log("warning", &format!("streamrip failed for {} ({url}): {e}", r.label()));
                // the whole output stays in the request's log (the message shows its first lines)
                if let Some(log) = streamrip::run_log(&e) {
                    log_push(r, format!("streamrip output for {url}:\n{log}"));
                }
                last = e.to_string();
            }
        }
    }
    rm(&staging(st, r));
    Ok(Out::Next(last.chars().take(300).collect::<String>()))
}

pub(super) fn progress_cb(st: &impl Ctx, id: i64) -> impl Fn(f64) + Send + Sync {
    let (db, events) = (st.db().clone(), st.requests().events.clone());
    move |pct| {
        let _ = db.execute("UPDATE requests SET progress=?, updated_at=? WHERE id=?", p![pct, now(), id]);
        events.emit(&db, id);
    }
}

pub(super) async fn start_ytdlp(st: &impl Ctx, r: &mut Req, pick_url: Option<String>) -> anyhow::Result<Out> {
    let _slot = st.requests().ytdlp.acquire().await?;
    check(st, r.id)?;
    let out = staging(st, r);
    let cb = progress_cb(st, r.id);
    if r.s("kind") == "track" {
        upd(st, r, &[("status", json!("searching")), ("message", json!("Searching YouTube Music…"))]);
        let (url, from_ytm, cover) = match &pick_url {
            Some(u) => (Some(u.clone()), u.contains("music.youtube.com"), None),
            None => match encore_library::ytmusic::find_song(st, &r.s("artist"), &r.s("title"), r.length()).await
                .filter(|hit| !crate::report::blocked(st, "youtube", hit["videoId"].as_str().unwrap_or(""))) {
                Some(hit) => (Some(format!("https://music.youtube.com/watch?v={}", hit["videoId"].as_str().unwrap_or(""))), true, hit["cover"].as_str().map(String::from)),
                None => (ytdlp::find_best(st, &r.s("artist"), &r.s("title"), r.length()).await, false, None),
            },
        };
        // a reported video is never picked again
        let url = url.filter(|u| pick_url.is_some() || !crate::report::url_blocked(st, u));
        let Some(url) = url else { return Ok(Out::Next("no match on YouTube".into())) };
        upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("Downloading {url}")))]);
        let res = ytdlp::download(st, &url, &out, false, Some(&cb)).await?;
        let Some(f) = res.files.first() else { return Ok(Out::Next("yt-dlp produced no audio".into())) };
        let hint = r.hint().or(cover);
        let mbid = r.opt("mbid");
        match tagger::import_single(st, f, &r.s("artist"), &r.s("title"), mbid.as_deref(), &out, r.length(), true, hint.as_deref()).await {
            Ok((new, note)) => {
                rm(&out);
                let src = if from_ytm { "YouTube Music" } else { "YouTube" };
                complete(st, r, &with_note(&format!("Downloaded from {src}"), &note), &[new], json!({ "source": src, "url": url })).await;
                Ok(Out::Done)
            }
            Err(e) if e.is::<Rejected>() => {
                rm(&out);
                Ok(Out::Next(format!("YouTube match was the wrong audio ({e})")))
            }
            Err(e) => Err(e),
        }
    } else {
        let rel = release_info(st, r).await;
        let album = r.album_title();
        upd(st, r, &[("status", json!("searching")), ("message", json!("Searching YouTube Music for the album…"))]);
        let ext = r.d["ext"].clone();
        let yt_album = if let Some(u) = &pick_url {
            Some(json!({ "title": r.d["pick"]["title"].as_str().map(String::from).unwrap_or(album.clone()), "playlist_url": u }))
        } else if ext["ytm"].is_string() || ext["bandcamp"].is_string() {
            // release MusicBrainz lacks: download exactly the YouTube Music / Bandcamp release it was found on
            let mut a = json!({ "title": album, "playlist_url": ext["bandcamp"] });
            if let Some(b) = ext["ytm"].as_str() {
                match encore_http::ytmusic::album(st, b).await {
                    Ok((_, Some(pl), _)) => a["playlist_url"] = json!(format!("https://music.youtube.com/playlist?list={pl}")),
                    Ok(_) => {}
                    Err(e) => st.db().log("warning", &format!("YouTube Music album lookup failed for {}: {e}", r.label())),
                }
            }
            Some(a)
        } else {
            encore_library::ytmusic::find_album(st, &r.s("artist"), &album, n_tracks(&rel)).await
        };
        if let Some(pl) = yt_album.as_ref().and_then(|a| a["playlist_url"].as_str().map(String::from)).filter(|u| pick_url.is_some() || !crate::report::url_blocked(st, u)) {
            let title = yt_album.as_ref().unwrap()["title"].as_str().unwrap_or("").to_string();
            upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("YouTube Music album: {title}")))]);
            let res: anyhow::Result<Option<Vec<PathBuf>>> = async {
                let dl = ytdlp::download(st, &pl, &out.join("album"), true, Some(&cb)).await?;
                let n = n_tracks(&rel);
                let enough = pick_url.is_some() || n.is_none_or(|n| dl.files.len() + (n / 5).max(1) >= n);
                if dl.files.is_empty() || !enough {
                    return Ok(None);
                }
                upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
                Ok(Some(import(st, r, &dl.files, &out).await?.0))
            }.await;
            match res {
                Ok(Some(new)) => {
                    rm(&out);
                    let via = if pl.contains("bandcamp.com") { "Bandcamp" } else { "YouTube Music" };
                    complete(st, r, &format!("Downloaded from {via}"), &new, json!({ "source": via, "detail": format!("album {title}"), "url": pl })).await;
                    return Ok(Out::Done);
                }
                Ok(None) => {}
                Err(e) if e.is::<Cancelled>() => return Err(e),
                Err(e) => st.db().log("warning", &format!("YouTube Music album for {} failed: {e}", r.label())),
            }
            rm(&out);
            if pick_url.is_some() {
                return Ok(Out::Next("the chosen YouTube album could not be downloaded or didn't match".into()));
            }
        }
        let Some(rel) = rel else { return Ok(Out::Next("no MusicBrainz tracklist for this album".into())) };
        let tracks = rel["tracks"].as_array().cloned().unwrap_or_default();
        let n = tracks.len();
        let (mut files, mut missing) = (vec![], vec![]);
        for (i, t) in tracks.iter().enumerate() {
            check(st, r.id)?;
            let title = t["title"].as_str().unwrap_or("").to_string();
            let artist = t["artist"].as_str().filter(|a| !a.is_empty()).map(String::from).unwrap_or(r.s("artist"));
            upd(st, r, &[("status", json!("downloading")), ("progress", json!((i as f64 / n as f64 * 1000.0).round() / 10.0)),
                         ("message", json!(format!("YouTube: track {}/{n} – {title}", i + 1)))]);
            let len = t["length"].as_f64().filter(|l| *l > 0.0);
            let url = match encore_library::ytmusic::find_song(st, &artist, &title, len).await {
                Some(h) => Some(format!("https://music.youtube.com/watch?v={}", h["videoId"].as_str().unwrap_or(""))),
                None => ytdlp::find_best(st, &artist, &title, len).await,
            };
            let url = url.filter(|u| !crate::report::url_blocked(st, u));
            let got = match url { Some(u) => ytdlp::download(st, &u, &out.join(format!("{:02}", i + 1)), false, None).await.map(|d| d.files).unwrap_or_default(), None => vec![] };
            if got.is_empty() {
                missing.push(title);
            }
            files.extend(got);
        }
        if missing.len() > (n / 5).max(1) {
            rm(&out);
            return Ok(Out::Next(format!("YouTube missing {}/{n} tracks", missing.len())));
        }
        upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
        match import(st, r, &files, &out).await {
            Ok((new, _)) => {
                rm(&out);
                let m = if missing.is_empty() { "Downloaded from YouTube".to_string() } else { format!("Downloaded from YouTube ({} track(s) missing)", missing.len()) };
                complete(st, r, &m, &new, json!({ "source": "YouTube Music", "detail": "matched track by track" })).await;
                Ok(Out::Done)
            }
            Err(e) if e.is::<Rejected>() => {
                rm(&out);
                Ok(Out::Next(format!("YouTube matches were wrong ({e})")))
            }
            Err(e) => Err(e),
        }
    }
}

/// Any link: downloaded as-is with yt-dlp, then tagged from what the files say.
pub(super) async fn run_url(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let _slot = st.requests().ytdlp.acquire().await?;
    let out = staging(st, r);
    let cb = progress_cb(st, r.id);
    let res = ytdlp::download(st, &r.s("url"), &out, true, Some(&cb)).await?;
    check(st, r.id)?;
    if res.files.is_empty() {
        fail(st, r, "yt-dlp produced no audio files").await;
        return Ok(());
    }
    upd(st, r, &[("status", json!("importing")), ("message", json!("Tagging and importing…"))]);
    let hint = r.hint().or_else(|| (res.entries.len() == 1).then(|| res.entries[0]["thumbnail"].as_str().map(String::from)).flatten());
    let albums: std::collections::HashSet<String> = res.entries.iter().filter_map(|e| e["album"].as_str().filter(|a| !a.is_empty()).map(String::from)).collect();
    let mut new = vec![];
    if res.files.len() > 1 && albums.len() == 1 {
        let artist = res.entries[0]["artist"].as_str().map(String::from).or(r.opt("artist")).unwrap_or("Unknown Artist".into());
        new = tagger::import_album_files(st, &res.files, tagger::AlbumImport {
            artist: &artist, album: albums.iter().next().unwrap(), rgid: None, source_root: &out, reject: false, source_meta: None, hint: hint.as_deref(),
        }).await?;
    } else {
        for f in &res.files {
            let t = encore_media::tags::read_tags(&f.to_string_lossy());
            let (mut artist, mut title) = (t.s("artist"), t.s("title"));
            let base = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if (artist.is_empty() || artist == "Unknown Artist") && base.contains(" - ") {
                let (a, b) = base.split_once(" - ").unwrap();
                (artist, title) = (a.to_string(), b.to_string());
            }
            let prefix = format!("{} - ", artist.to_lowercase());
            if !artist.is_empty() && title.to_lowercase().starts_with(&prefix) {
                title = title[prefix.len()..].to_string();
            }
            let artist = if artist.is_empty() { "Unknown Artist".to_string() } else { artist };
            match tagger::import_single(st, f, &artist, &title, None, &out, None, false, hint.as_deref()).await {
                Ok((p, _)) => new.push(p),
                Err(e) => st.db().log("warning", &format!("Skipped {base}: {e}")),
            }
        }
    }
    rm(&out);
    let url = r.s("url");
    let host = reqwest::Url::parse(&url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or("link".into());
    let src = if host.contains("music.youtube") { "YouTube Music".to_string() } else { host.trim_start_matches("www.").to_string() };
    complete(st, r, &format!("Downloaded {} file(s)", new.len()), &new, json!({ "source": src, "detail": "direct link", "url": url })).await;
    Ok(())
}

pub(super) async fn run_streamrip_url(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let _slot = st.requests().streamrip.acquire().await?;
    let out = staging(st, r);
    let url = r.s("url");
    let files = match streamrip::download(st, &url, &out).await {
        Ok(f) => f,
        Err(e) => {
            // the whole output stays in the request's log before the failure is recorded
            if let Some(log) = streamrip::run_log(&e) {
                log_push(r, format!("streamrip output for {url}:\n{log}"));
                upd(st, r, &[]);
            }
            return Err(e);
        }
    };
    upd(st, r, &[("status", json!("importing")), ("message", json!("Tagging and importing…"))]);
    let mut groups: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
    for f in files {
        groups.entry(f.parent().unwrap().to_path_buf()).or_default().push(f);
    }
    let mut new = vec![];
    for fs in groups.values() {
        let t = encore_media::tags::read_tags(&fs[0].to_string_lossy());
        if fs.len() > 1 {
            new.extend(tagger::import_album_files(st, fs, tagger::AlbumImport {
                artist: &t.s("albumartist"), album: &t.s("album"), rgid: None, source_root: &out, reject: false, source_meta: None, hint: None,
            }).await?);
        } else {
            new.push(tagger::import_single(st, &fs[0], &t.s("artist"), &t.s("title"), None, &out, None, false, None).await?.0);
        }
    }
    rm(&out);
    let svc = if url.contains("deezer") { "Deezer" } else if url.contains("tidal") { "Tidal" } else { "Qobuz" };
    complete(st, r, &format!("Downloaded {} file(s) via streamrip", new.len()), &new, json!({ "source": svc, "detail": "via streamrip", "url": url })).await;
    Ok(())
}
