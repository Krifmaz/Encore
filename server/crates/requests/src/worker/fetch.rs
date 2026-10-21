//! Fetching: each source's search, download and the polling of downloads in progress (Soulseek, torrents
//! and Usenet through the download clients, streamrip, yt-dlp).
use super::*;

/// A search query from a request's names: bracketed parts and symbols out, the symbols that stand for
/// letters spelled out first (Ke$ha → Kesha, P!nk → Pink; see `torznab::fold_symbols`), so indexers and
/// Soulseek get "Kesha Cannibal" and not "Ke ha Cannibal". The same words as the picker's first search
/// ([`slskd::query_text`]), so the picker finds what the worker searched.
pub(super) fn q(parts: &[&str]) -> String {
    slskd::query_text(parts)
}

/// A request's message while its step runs, without touching the rest of the row (the worker's search
/// says "Searching Soulseek…" once its turn comes).
fn note_message(st: &impl Ctx, id: i64, msg: &str) {
    let _ = st.db().execute("UPDATE requests SET message=?, updated_at=? WHERE id=? AND status='searching'", p![msg, now(), id]);
    crate::events::emit(st, id);
}

/// One Soulseek search for a request: it waits its turn behind the other searches (one at a time, in the
/// Soulseek line: "In line for Soulseek") and says so while it waits, and while it sits out a pause of the
/// server's.
async fn slskd_search(st: &impl Ctx, id: i64, query: &str, searching: &'static str) -> anyhow::Result<Vec<Value>> {
    if st.requests().slskd.session().searching() {
        note_message(st, id, "Waiting for another Soulseek search…");
    }
    // kept with the request, for the picker opened for it
    slskd::search_kept(st, query, slskd::Who::Worker, Some(id), |pause| note_message(st, id, pause.unwrap_or(searching))).await
}

/// Wait for a slot of `line` for the request (`label`: what the line is for it, "Deezer"). While it waits
/// its row reads `queued`, "In line for Deezer" (the list says where it stands); once the slot comes the row
/// reads `status` and `message` again, and a request Download now moved up says so in its log. A row set to
/// cancelled while it waited (Remove artist writes its rows before it stops their tasks) stays cancelled: the
/// slot is given back and the step ends (Err([`Cancelled`])).
async fn take_line(st: &impl Ctx, r: &mut Req, line: &Arc<crate::line::Line>, label: &str, status: &str, message: &str) -> anyhow::Result<crate::line::Slot> {
    let lines = &st.requests().lines;
    let id = r.id;
    let mut waited = false;
    let slot = line.acquire(id, label, &lines.pressed, || {
        waited = true;
        now::in_line_row(st, id, label);
    }).await;
    if waited {
        served_first(st, r, line.source);
        // only over the row that still waits in the line
        let back = st.db().query("UPDATE requests SET status=?, message=?, detail=?, updated_at=? WHERE id=? AND status='queued' RETURNING id",
            p![status, message, r.d.to_string(), now(), id]).is_ok_and(|rows| !rows.is_empty());
        if back {
            r.row.insert("status".into(), json!(status));
            r.row.insert("message".into(), json!(message));
            crate::events::emit(st, id);
        } else {
            check(st, id)?;
            upd(st, r, &[("status", json!(status)), ("message", json!(message))]);
        }
    }
    Ok(slot)
}

/// A request Download now moved up in line `source` got its slot: one line in its log ("streamrip: started
/// first in line (Download now by carol)").
pub(super) fn served_first(st: &impl Ctx, r: &mut Req, source: &str) {
    if let Some(p) = st.requests().lines.pressed.take_served(r.id, source) {
        let src = r.opt("source").unwrap_or(source.to_string());
        log_push(r, format!("{src}: started first in line (Download now by {})", p.by));
    }
}

/// The end of the reason a Soulseek download failed because its user is offline.
pub(super) const SLSKD_OFFLINE: &str = "is offline on Soulseek";

/// slskd's answer when it couldn't ask `user` for the files, in words: "<user> is offline on Soulseek" for
/// Soulseek's "User x appears to be offline", else its own words.
pub(super) fn enqueue_failure(user: &str, said: &str) -> String {
    if said.to_lowercase().contains("appears to be offline") {
        return format!("{user} {SLSKD_OFFLINE}");
    }
    said.to_string()
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
            served_first(st, r, crate::line::SOULSEEK);
            cands = slskd::rank_album(st, &res, &r.s("artist"), &album, n_tracks(&rel), lossless_only);
            if !cands.is_empty() {
                break;
            }
        }
    } else {
        let res = slskd_search(st, r.id, &q(&[&r.s("artist"), &r.s("title")]), searching).await?;
        served_first(st, r, crate::line::SOULSEEK);
        cands = slskd::rank_track(st, &res, &r.s("artist"), &r.s("title"), r.length(), lossless_only);
    }
    check(st, r.id)?;
    // it searched: a later wait for Soulseek starts its own hour
    if let Some(d) = r.d.as_object_mut() {
        d.remove("soulseek_wait");
        d.remove("wait");
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
pub(super) fn slskd_key(c: &Value) -> String {
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
            o.remove("remote");
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
            reason = enqueue_failure(&user, &e.to_string());
            continue;
        }
        if let Some(d) = r.d.as_object_mut() {
            d.remove("soulseek_wait"); // a later wait for Soulseek starts its own hour
            d.remove("wait");
        }
        let t = now();
        r.d["slskd"]["started"] = json!(t);
        r.d["slskd"]["last_change"] = json!(t);
        r.d["slskd"]["last_bytes"] = json!(0);
        upd(st, r, &[("status", json!("downloading")), ("progress", json!(0)), ("message", json!(slskd_message(&c)))]);
        return Ok(Out::Wait);
    }
}

/// "Downloading 12 file(s) from bob (FLAC)", "… (MP3 320k)": a Soulseek candidate handed to slskd.
pub(super) fn slskd_message(c: &Value) -> String {
    format!("Downloading {} file(s) from {} ({})", c["files"].as_array().map(|a| a.len()).unwrap_or(0), c["username"].as_str().unwrap_or(""), slskd_format(c))
}

/// "FLAC", "MP3 320k": a Soulseek candidate's format.
pub(super) fn slskd_format(c: &Value) -> String {
    let br = c["bitrate"].as_i64().unwrap_or(0);
    format!("{}{}", c["format"].as_str().unwrap_or("").to_uppercase(), if br > 0 { format!(" {br}k") } else { String::new() })
}

/// How often a download waiting in a user's upload queue asks for its place there, when slskd doesn't say.
const REMOTE_ASK_SECS: f64 = 300.0;

/// Every file of a Soulseek download waits in the user's own upload queue ("Queued, Remotely") and no byte
/// arrived: Some(its place there – the smallest `placeInQueue` slskd reports; None when it reports none).
/// None otherwise: a file that downloads, bytes that arrived, slskd's own queue ("Queued, Locally").
pub(super) fn remote_queue<'a>(files: impl IntoIterator<Item = &'a Value>) -> Option<Option<u64>> {
    let mut any = false;
    let mut place: Option<u64> = None;
    for f in files {
        any = true;
        if f["state"].as_str() != Some("Queued, Remotely") || f["bytesTransferred"].as_f64().unwrap_or(0.0) > 0.0 {
            return None;
        }
        if let Some(p) = f["placeInQueue"].as_u64().filter(|p| *p > 0) {
            place = Some(place.map_or(p, |q| q.min(p)));
        }
    }
    any.then_some(place)
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
    // every file waits in the user's own upload queue: where it stands there (Download now can switch to a
    // user with a free upload slot); gone as soon as anything moves
    let remote = remote_queue(latest.values()).filter(|_| got <= 0.0);
    match remote {
        Some(found) => {
            let prev = r.d["slskd"]["remote"].clone();
            let since = prev["since"].as_f64().unwrap_or(t);
            let mut asked_at = prev["asked_at"].as_f64();
            let place = match found {
                Some(p) => Some(p),
                // slskd doesn't say: the user is asked, every 5 minutes at most
                None if asked_at.is_none_or(|a| t - a >= REMOTE_ASK_SECS) => {
                    asked_at = Some(t);
                    let first = wanted.iter().find_map(|n| latest.get(n)).map(|f| f["id"].as_str().map(String::from).unwrap_or(f["id"].to_string()));
                    match first {
                        Some(fid) => slskd::place_in_queue(st, &user, &fid).await,
                        None => None,
                    }
                }
                None => prev["place"].as_u64(),
            };
            r.d["slskd"]["remote"] = json!({ "place": place, "since": since, "asked_at": asked_at });
        }
        None => {
            if let Some(o) = r.d["slskd"].as_object_mut() {
                o.remove("remote");
            }
        }
    }
    let msg = match r.d["slskd"]["remote"]["place"].as_u64().filter(|_| remote.is_some()) {
        Some(p) => format!("Queued by {user} – {} in their queue, {done}/{} files", crate::line::ordinal(p as usize), wanted.len()),
        None => format!("{} {user} – {done}/{} files", if queued { "Queued by" } else { "Downloading from" }, wanted.len()),
    };
    upd(st, r, &[("progress", json!((got / total * 1000.0).round() / 10.0)), ("message", json!(msg))]);
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
    let mut origin = json!({ "source": "Soulseek", "detail": format!("user {user} · {}", c["directory"].as_str().unwrap_or("").replace('\\', "/")) });
    if let Some((kind, key, label)) = crate::report::block_of_slskd(c, r.s("kind") == "album") {
        origin["block"] = json!({ "kind": kind, "key": key, "label": label });
    }
    let src = if r.s("source") == "slskd_lossless" { "slskd_lossless" } else { "slskd" };
    let done = format!("Downloaded from Soulseek ({})", c["format"].as_str().unwrap_or("").to_uppercase());
    // the files stay in Soulseek's folder when the import has to wait: Encore never clears that folder
    match storage::import_checked(st, r, &paths, &root, storage::Spec::new(src, false, origin, done)).await {
        Ok(_) => Ok(()),
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
    let searching = "Searching torrent and Usenet indexers (Prowlarr)…";
    // the album's songs from MusicBrainz before its turn in the line, not while it holds a slot others wait for
    release_info(st, r).await;
    // the requests' indexer searches, two at a time: its turn in the indexers' line (held for its searches)
    let _slot = take_line(st, r, &st.requests().lines.indexers, crate::line::TORRENTS, "searching", searching).await?;
    check(st, r.id)?;
    // a client's folder may have broken while it waited in line
    if downloads::hold(st, r, "torznab", true).await? {
        return Ok(Out::Wait);
    }
    upd(st, r, &[("status", json!("searching")), ("message", json!(searching))]);
    let rel = release_info(st, r).await;
    let album = r.album_title();
    let (artist, year) = (r.s("artist"), r.s("year").get(..4).and_then(|y| y.parse::<i32>().ok()));
    let want = torznab::Want { artist: &artist, album: &album, year, tracks: n_tracks(&rel), lossless_only };
    let mut cands = vec![];
    for query in [q(&[&artist, &album]), q(&[&album])] {
        let res = torznab::search(st, &query).await?;
        // kept with the request, for the picker opened for it
        crate::search_cache::keep_releases(st, &query, &["3000"], Some(r.id), &res).await;
        cands = torznab::rank_album(st, &res, &want);
        // the indexers had releases for "artist album" (none of them fit): the album's name alone finds
        // the same and more of the artist's other releases – not worth another of the indexers' searches
        if !cands.is_empty() || !res.is_empty() {
            break;
        }
    }
    check(st, r.id)?;
    cands.retain(|c| !crate::report::release_blocked(st, c)); // reported releases are never picked again
    torznab_drop_old(st, r).await;
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
    // a release whose client's folder can't be written is not handed over (downloads.rs)
    let mut held: Option<(String, downloads::Probed)> = None;
    loop {
        let idx = r.d["torznab"]["idx"].as_i64().unwrap_or(-1) + 1;
        r.d["torznab"]["idx"] = json!(idx);
        let Some(c) = r.d["torznab"]["candidates"].get(idx as usize).cloned() else {
            // the releases left all go to a client whose folder fails: wait for it
            if let Some((proto, p)) = held {
                downloads::park_release(st, r, &proto, &p)?;
                return Ok(Out::Wait);
            }
            return Ok(Out::Next(if reason.is_empty() { "no indexer release worked".into() } else { reason }));
        };
        let proto = c["protocol"].as_str().unwrap_or("torrent");
        if let Some(p) = downloads::failing_client(st, proto) {
            held.get_or_insert((proto.to_string(), p));
            continue;
        }
        match torznab::add(st, &c, r.id).await {
            Ok(id) => {
                let t = now();
                r.d["torznab"]["id"] = json!(id);
                r.d["torznab"]["started"] = json!(t);
                r.d["torznab"]["last_change"] = json!(t);
                r.d["torznab"]["last_progress"] = json!(0);
                r.d["torznab"]["queued_since"] = Value::Null;
                r.d["torznab"]["busy"] = Value::Null;
                let via = torznab::client_name(st, c["protocol"].as_str().unwrap_or("torrent"));
                upd(st, r, &[("status", json!("downloading")), ("progress", json!(0)),
                             ("message", json!(format!("{via}: {} ({})", c["title"].as_str().unwrap_or(""), c["indexer"].as_str().unwrap_or(""))))]);
                return Ok(Out::Wait);
            }
            Err(e) => reason = e.to_string(),
        }
    }
}

/// How long a download client may take to list a download it was just given (qBittorrent takes a
/// magnet in the background).
pub(super) const CLIENT_TAKES: f64 = 120.0;

/// How many stall times (Settings → Plugins → Torrents & Usenet (Prowlarr), "Try the next release after a
/// stall of") a download may wait in its client's queue before the next release is tried.
const QUEUED_STALLS: f64 = 6.0;

/// "13 hours", "45 minutes": how long something took, in words.
fn span(secs: f64) -> String {
    if secs >= 2.0 * 3600.0 { format!("{} hours", (secs / 3600.0).round()) } else { format!("{} minutes", (secs / 60.0).round().max(1.0)) }
}

/// The client a request's current release went to ("qBittorrent", "SABnzbd" …).
pub(super) fn torznab_client(st: &impl Ctx, r: &Req) -> &'static str {
    let idx = r.d["torznab"]["idx"].as_u64().unwrap_or(0) as usize;
    torznab::client_name(st, r.d["torznab"]["candidates"][idx]["protocol"].as_str().unwrap_or("torrent"))
}

/// Why a finished download with no audio files is no use when it holds archives (RAR/ZIP/7z), which
/// Encore doesn't unpack: a Usenet client is asked to unpack Encore's downloads (so it failed to, or
/// the archive held no audio), a torrent client never unpacks. None without archives.
fn packed(path: &Path, via: &str, usenet: bool) -> Option<String> {
    let archives = walkdir::WalkDir::new(path).max_depth(3).into_iter().flatten().any(|e| {
        let n = e.file_name().to_string_lossy().to_lowercase();
        [".rar", ".zip", ".7z", ".r00", ".001"].iter().any(|x| n.ends_with(x))
    });
    archives.then(|| if usenet {
        format!("the release is still packed (RAR/ZIP/7z) – {via} didn't unpack it")
    } else {
        "the release is packed (RAR/ZIP/7z), which Encore doesn't unpack".to_string()
    })
}

pub(super) async fn poll_torznab(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let Some(id) = r.d["torznab"]["id"].as_str().map(String::from) else {
        let o = torznab_try_next(st, r, "lost track of the download").await?;
        return after_slskd(st, r, o).await;
    };
    let t = now();
    let via = torznab_client(st, r);
    let limit = st.cfg().int("torznab_timeout_minutes").max(1) as f64 * 60.0;
    let polled = match torznab::poll(st, &id).await {
        Ok(p) => {
            if let Some(o) = r.d["torznab"].as_object_mut().filter(|o| o.contains_key("err_since")) {
                // back: the time it didn't answer was no stall of the download
                o.remove("err_since");
                o.insert("last_change".into(), json!(t));
                log_push(r, format!("torznab: {via} answers again"));
            }
            p
        }
        Err(e) => {
            // the client restarting, or slow to answer once, is no reason to drop the download: it is
            // asked again at the next look, and given up after as long as a stalled download
            let why = encore_core::fsx::explain(&e);
            let since = r.d["torznab"]["err_since"].as_f64().unwrap_or(t);
            if t - since > limit {
                let o = torznab_try_next(st, r, &format!("{via} didn't answer for {} minutes ({why})", (limit / 60.0).round())).await?;
                return after_slskd(st, r, o).await;
            }
            if r.d["torznab"]["err_since"].is_null() {
                r.d["torznab"]["err_since"] = json!(t);
                log_push(r, format!("torznab: {via} not answering ({why}) – asking again"));
                st.db().log("warning", &format!("{via} not answering for {}: {why} – asking again", r.label()));
            }
            upd(st, r, &[("message", json!(format!("{via} not answering – asking again ({why})")))]);
            return Ok(());
        }
    };
    // a song from a release with several files: download only the song's file (once the client answers)
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
    let busy = matches!(polled, torznab::Poll::Busy(..));
    match polled {
        torznab::Poll::Progress(pct, msg) | torznab::Poll::Busy(pct, msg) => {
            if let Some(o) = r.d["torznab"].as_object_mut() {
                o.remove("queued_since");
                // the client works on the files it has (checks, repairs, unpacks or moves them): the stall time
                // runs as before, but Download now never gives it up (`now::stalled_since`)
                if busy {
                    o.insert("busy".into(), json!(t));
                } else {
                    o.remove("busy");
                }
            }
            if pct > r.d["torznab"]["last_progress"].as_f64().unwrap_or(0.0) {
                r.d["torznab"]["last_progress"] = json!(pct);
                r.d["torznab"]["last_change"] = json!(t);
            }
            if t - r.d["torznab"]["last_change"].as_f64().unwrap_or(t) > limit {
                let o = torznab_try_next(st, r, "download stalled").await?;
                return after_slskd(st, r, o).await;
            }
            upd(st, r, &[("progress", json!(pct)), ("message", json!(msg))]);
            Ok(())
        }
        torznab::Poll::Queued(pct, msg) => {
            // waiting for the client's turn is no stall: the stall time starts once it downloads. A queue
            // that doesn't move for six stall times (its limit filled by stalled downloads of other apps) is
            // given up as a stall is
            r.d["torznab"]["last_change"] = json!(t);
            let since = r.d["torznab"]["queued_since"].as_f64().unwrap_or(t);
            r.d["torznab"]["queued_since"] = json!(since);
            if t - since > QUEUED_STALLS * limit {
                let o = torznab_try_next(st, r, &format!("{via} kept it queued for {}", span(t - since))).await?;
                return after_slskd(st, r, o).await;
            }
            upd(st, r, &[("progress", json!(pct)), ("message", json!(msg))]);
            Ok(())
        }
        torznab::Poll::Missing(_) if t - r.d["torznab"]["started"].as_f64().unwrap_or(0.0) < CLIENT_TAKES => {
            // just added: the client may still be taking it in (a download without its start time is no
            // longer new)
            upd(st, r, &[("message", json!(format!("{via}: adding the download…")))]);
            Ok(())
        }
        torznab::Poll::Missing(why) | torznab::Poll::Failed(why) => {
            let o = torznab_try_next(st, r, &why).await?;
            after_slskd(st, r, o).await
        }
        torznab::Poll::Done(path) => {
            storage::register_roots(st);
            // only a path that isn't there is "can't see it": a mount that doesn't answer (EIO, ENOTCONN,
            // ESTALE) waits for the storage, and a folder Encore may not open is "can't read it"
            let is_file = match encore_core::fsx::metadata(&path, encore_core::fsx::Step::Import) {
                Ok(m) => m.is_file(),
                Err(e) if e.source.kind() == std::io::ErrorKind::NotFound => {
                    // finished where Encore can't look: the client keeps it (nothing is deleted), and the other
                    // releases would land in the same place
                    let why = format!("{via} finished the download, but Encore can't see it at {} – mount the client's download folder in Encore's container at the \
                        same path (e.g. /downloads), or correct the download folder on the {via} card under Settings → Plugins", path.display());
                    return torznab_left_in_client(st, r, why).await;
                }
                Err(e) if e.is_storage() => return Err(e.into()),
                Err(e) => return torznab_unreadable(st, r, via, &e).await,
            };
            upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
            let listed = if is_file { Ok(vec![path.clone()]) } else { streamrip::audio_files_checked(&path, &encore_core::audio::DOWNLOAD_EXTS) };
            let mut files = match listed {
                Ok(f) => f,
                // the disk's problem: the request waits for the storage, and the download stays in the client
                Err(e) if e.is_storage() => return Err(e.into()),
                Err(e) => return torznab_unreadable(st, r, via, &e).await,
            };
            if files.is_empty() {
                let usenet = r.d["torznab"]["candidates"][r.d["torznab"]["idx"].as_u64().unwrap_or(0) as usize]["protocol"] == "usenet";
                let why = packed(&path, via, usenet).unwrap_or_else(|| "the download had no audio files".to_string());
                let o = torznab_try_next(st, r, &why).await?;
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
            let via = if cand["protocol"] == "usenet" { "Usenet" } else { "torrent" };
            let mut origin = json!({ "source": cand["indexer"].as_str().unwrap_or(via), "detail": format!("{via}: {}", cand["title"].as_str().unwrap_or("")) });
            if let Some((kind, key, label)) = crate::report::block_of_release(&cand) {
                origin["block"] = json!({ "kind": kind, "key": key, "label": label });
            }
            // the download leaves its client once its files are in the library – not before: while the import
            // waits for the storage the files stay in the client's folder
            let mut spec = storage::Spec::new("torznab", false, origin, format!("Downloaded via {via} ({})", cand["indexer"].as_str().unwrap_or("Prowlarr")));
            spec.torznab_id = Some(id.clone());
            match storage::import_checked(st, r, &files, &root, spec).await {
                Ok(_) => Ok(()),
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

/// A finished torrent or Usenet download Encore can't import where it is: it stays in its client (nothing
/// is deleted, and the request no longer looks after it), and the other releases are not tried – they
/// would land in the same place. The request goes on to its next source.
async fn torznab_left_in_client(st: &impl Ctx, r: &mut Req, why: String) -> anyhow::Result<()> {
    st.db().log("warning", &format!("{}: {why}", r.label()));
    r.d["torznab"]["id"] = Value::Null;
    let n = r.d["torznab"]["candidates"].as_array().map(|a| a.len()).unwrap_or(0);
    r.d["torznab"]["idx"] = json!(n);
    after_slskd(st, r, Out::Next(why)).await
}

/// [`torznab_left_in_client`] for a download Encore may not read (the client writes as another user):
/// never deleted as "no audio files".
async fn torznab_unreadable(st: &impl Ctx, r: &mut Req, via: &str, e: &encore_core::fsx::StepError) -> anyhow::Result<()> {
    let hint = e.hint().map(|h| format!(" – {h}")).unwrap_or_default();
    torznab_left_in_client(st, r, format!("{via} finished the download, but Encore can't read it: {}{hint}", e.what())).await
}

/// A request's torrent or Usenet download that a new search or pick replaces (`detail.torznab.id`, still
/// set): it leaves its client first, so that nothing is left downloading or finished there that no request
/// looks after.
pub(super) async fn torznab_drop_old(st: &impl Ctx, r: &mut Req) {
    if let Some(id) = r.d["torznab"]["id"].as_str().map(String::from) {
        torznab::remove(st, &id).await;
        r.d["torznab"]["id"] = Value::Null;
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
    // the lookups one at a time; each download then waits for its service's line below
    let looking = "Looking for an exact lossless match…";
    // what it asks MusicBrainz for (the album's songs, the song's ISRC), asked before the lookup lock: the lock then
    // goes to searches, not to waits for MusicBrainz (under the lock they are read from the request)
    let by_deezer = streamrip::services(st).iter().any(|s| s == "deezer") && (r.d["ext"]["deezer"].is_i64() || r.d["ext"]["deezer"].is_string());
    if urls.is_none() && !by_deezer {
        if r.s("kind") == "album" {
            release_info(st, r).await;
        } else {
            isrc_for(st, r).await;
        }
    }
    let search = st.requests().lines.streamrip_search.clone();
    let lookup = search.lock().await;
    check(st, r.id)?;
    if downloads::hold(st, r, "streamrip", !r.d["pick"].is_object()).await? {
        return Ok(Out::Wait);
    }
    upd(st, r, &[("status", json!("searching")), ("message", json!(looking))]);
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
    drop(lookup);
    if urls.is_empty() {
        return Ok(Out::Next("not found on Deezer/Tidal".into()));
    }
    let mut last = String::new();
    for url in urls {
        // one release at a time per service: its turn in that service's line ("In line for Deezer" meanwhile);
        // Deezer, Qobuz and Tidal are separate accounts, so one of each runs at once
        let service = crate::line::service_of(&url).unwrap_or("deezer");
        let label = crate::line::service_name(service);
        let _slot = take_line(st, r, &st.requests().lines.streamrip(service), &label, "downloading", &format!("streamrip: {url}")).await?;
        check(st, r.id)?;
        // the staging folder may have broken while it waited in line
        if downloads::hold(st, r, "streamrip", !r.d["pick"].is_object()).await? {
            return Ok(Out::Wait);
        }
        let out = staging(st, r).join("streamrip");
        rm(&out);
        upd(st, r, &[("status", json!("downloading")), ("message", json!(format!("streamrip: {url}")))]);
        let files = match streamrip::download_run(st, &url, &out).await {
            Ok(run) => {
                // streamrip's own words stay with the request on every outcome (its details show them)
                storage::keep_output(r, "streamrip", &url, &run.log, true);
                run.files
            }
            Err(e) if e.is::<Cancelled>() => return Err(e),
            Err(e) => {
                // the whole output stays with the request, in its details (the message shows streamrip's words)
                if let Some(log) = streamrip::run_log(&e) {
                    log_push(r, format!("streamrip: {url} failed – its output is in the details"));
                    storage::keep_output(r, "streamrip", &url, log, false);
                }
                // the staging folder failed, not streamrip: the step waits for the storage
                if let Some(se) = storage_error(st, &e) {
                    storage::wait_for_storage(st, r, se, storage::STORAGE_DOWNLOAD)?;
                    return Ok(Out::Wait);
                }
                last = encore_core::fsx::explain(&e);
                storage::log_failure(st, r, "streamrip", &format!("{url}: {last}"));
                continue;
            }
        };
        upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
        let svc = if url.contains("deezer") { "Deezer" } else if url.contains("tidal") { "Tidal" } else { "Qobuz" };
        let spec = storage::Spec::new("streamrip", true, json!({ "source": svc, "detail": "via streamrip", "url": url }), format!("Downloaded from {svc} via streamrip"));
        match storage::import_checked(st, r, &files, &out, spec).await {
            // done, or waiting for the storage with the files kept, or stopped with them kept: no other link
            // or source is tried for a storage problem
            Ok(_) => return Ok(Out::Done),
            Err(e) if e.is::<Cancelled>() => return Err(e),
            Err(e) => {
                last = encore_core::fsx::explain(&e);
                storage::log_failure(st, r, "streamrip", &format!("{url}: {last}"));
            }
        }
    }
    rm(&staging(st, r));
    Ok(Out::Next(last))
}

pub(super) fn progress_cb(st: &impl Ctx, id: i64) -> impl Fn(f64) + Send + Sync {
    let (db, events) = (st.db().clone(), st.requests().events.clone());
    move |pct| {
        let _ = db.execute("UPDATE requests SET progress=?, updated_at=? WHERE id=?", p![pct, now(), id]);
        events.emit(&db, id);
    }
}

pub(super) async fn start_ytdlp(st: &impl Ctx, r: &mut Req, pick_url: Option<String>) -> anyhow::Result<Out> {
    // two at a time: its turn in the yt-dlp line ("In line for YouTube" meanwhile)
    let label = match (r.d["pick"]["source"].as_str(), pick_url.as_deref()) {
        (Some("link"), Some(u)) => crate::line::link_site(u),
        _ => crate::line::YOUTUBE.to_string(),
    };
    let searching = if r.s("kind") == "track" { "Searching YouTube Music…" } else { "Searching YouTube Music for the album…" };
    // the album's songs from MusicBrainz before its turn in the line, not while it holds a slot others wait for
    if r.s("kind") != "track" {
        release_info(st, r).await;
    }
    let _slot = take_line(st, r, &st.requests().lines.ytdlp, &label, "searching", searching).await?;
    check(st, r.id)?;
    // the staging folder may have broken while it waited in line
    if downloads::hold(st, r, "ytdlp", !r.d["pick"].is_object()).await? {
        return Ok(Out::Wait);
    }
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
        let src = if from_ytm { "YouTube Music" } else { "YouTube" };
        let mut spec = storage::Spec::new("ytdlp", true, json!({ "source": src, "url": url }), format!("Downloaded from {src}"));
        spec.hint = cover;
        match storage::import_checked(st, r, std::slice::from_ref(f), &out, spec).await {
            Ok(_) => Ok(Out::Done),
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
                Ok(Some(dl.files))
            }.await;
            match res {
                Ok(Some(files)) => {
                    let via = if pl.contains("bandcamp.com") { "Bandcamp" } else { "YouTube Music" };
                    let spec = storage::Spec::new("ytdlp", true, json!({ "source": via, "detail": format!("album {title}"), "url": pl }), format!("Downloaded from {via}"));
                    match storage::import_checked(st, r, &files, &out, spec).await {
                        Ok(_) => return Ok(Out::Done),
                        Err(e) if e.is::<Cancelled>() => return Err(e),
                        Err(e) => st.db().log("warning", &format!("YouTube Music album for {} failed: {}", r.label(), encore_core::fsx::explain(&e))),
                    }
                }
                Ok(None) => {}
                Err(e) if e.is::<Cancelled>() => return Err(e),
                // the staging folder failed: the step waits for the storage
                Err(e) if storage_error(st, &e).is_some() => return Err(e),
                Err(e) => st.db().log("warning", &format!("YouTube Music album for {} failed: {}", r.label(), encore_core::fsx::explain(&e))),
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
            let got = match url {
                Some(u) => match ytdlp::download(st, &u, &out.join(format!("{:02}", i + 1)), false, None).await {
                    Ok(d) => d.files,
                    // the staging folder failed: the step waits for the storage (the tracks got so far stay)
                    Err(e) if storage_error(st, &e).is_some() => return Err(e),
                    Err(_) => vec![],
                },
                None => vec![],
            };
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
        let m = if missing.is_empty() { "Downloaded from YouTube".to_string() } else { format!("Downloaded from YouTube ({} track(s) missing)", missing.len()) };
        let spec = storage::Spec::new("ytdlp", true, json!({ "source": "YouTube Music", "detail": "matched track by track" }), m);
        match storage::import_checked(st, r, &files, &out, spec).await {
            Ok(_) => Ok(Out::Done),
            Err(e) if e.is::<Rejected>() => {
                rm(&out);
                Ok(Out::Next(format!("YouTube matches were wrong ({e})")))
            }
            Err(e) => Err(e),
        }
    }
}

/// A plugin program's source (protocol 1, through the registry: in the sandbox, its settings on stdin): the
/// best matching result of its search – or the result picked by hand (`picked`: the picker row's payload) –
/// downloaded into the request's staging folder, then checked, tagged and imported like any other download. A
/// search or download the plugin could not do goes on to the next source with the plugin's own words. One
/// request at a time per plugin: its search and download hold the plugin's line ("In line for Internet
/// Archive" meanwhile, Download now moves a request up in it).
pub(super) async fn start_plugin(st: &impl Ctx, r: &mut Req, source: &crate::sources::plugin::PluginSource, picked: Option<Value>) -> anyhow::Result<Out> {
    use crate::sources::plugin::{self as p, Ask};
    check(st, r.id)?;
    let name = source.name.clone();
    let line = st.requests().lines.plugin(&source.plugin);
    let _slot = match picked {
        Some(_) => take_line(st, r, &line, &name, "downloading", &format!("Downloading from {name}…")).await?,
        None => take_line(st, r, &line, &name, "searching", &format!("Searching {name}…")).await?,
    };
    check(st, r.id)?;
    // the staging folder may have broken while it waited in line
    if downloads::hold(st, r, &source.id, picked.is_none()).await? {
        return Ok(Out::Wait);
    }
    let kind = r.s("kind");
    let ask = Ask {
        kind: kind.clone(),
        artist: r.s("artist"),
        title: if kind == "album" { r.album_title() } else { r.s("title") },
        album: r.opt("album").unwrap_or_default(),
        query: None,
        length: r.length(),
        track_count: None,
        lossless_only: r.d["lossless_only"].as_bool().unwrap_or(false),
        isrc: r.d["isrc"].as_str().map(String::from),
        limit: 25,
    };
    let chosen = match picked {
        Some(c) => c,
        None => {
            upd(st, r, &[("status", json!("searching")), ("message", json!(format!("Searching {name}…")))]);
            let results = match p::search(st, source, &ask, None).await {
                Ok(r) => r,
                Err(f) => return Ok(Out::Next(format!("{name}: {}", f.message))),
            };
            let blocked = |x: &Value| x["url"].as_str().is_some_and(|u| crate::report::url_blocked(st, u));
            match p::best(&ask, &results, blocked) {
                Some(b) => b.clone(),
                None if results.is_empty() => return Ok(Out::Next(format!("nothing found on {name}"))),
                None => return Ok(Out::Next(format!("{name} found nothing that matches"))),
            }
        }
    };
    check(st, r.id)?;
    let out = staging(st, r).join(format!("plugin-{}", source.id));
    rm(&out);
    encore_core::fsx::mkdir_all(&out, encore_core::fsx::Step::Stage)?;
    let title = chosen["title"].as_str().unwrap_or("").to_string();
    upd(st, r, &[("status", json!("downloading")), ("progress", json!(0)), ("message", json!(format!("{name}: {title}").trim_end_matches(": ").to_string()))]);
    let cb = progress_cb(st, r.id);
    let call = encore_plugins::Call { progress: Some(std::sync::Arc::new(move |pct: f64, _: Option<String>| cb(pct))), ..Default::default() };
    let mut req = p::download_request(&source.id, &ask, &chosen);
    if r.surround() {
        req["surround"] = json!(true); // the Surround library's download: surround audio only (gamdl: Dolby Atmos, no fallback)
    }
    let dl = match st.plugins().download(&source.plugin, &req, &out, Some(call)).await {
        Ok(d) => d,
        Err(f) => {
            rm(&out);
            return Ok(Out::Next(format!("{name}: {}", f.message)));
        }
    };
    check(st, r.id)?;
    let files: Vec<PathBuf> = dl.files.iter().map(|f| f.path.clone()).collect();
    let url = dl.source_url.clone().or_else(|| chosen["url"].as_str().map(String::from));
    let lossless = chosen["lossless"].as_bool().unwrap_or(source.lossless);
    if let Some(m) = dl.message.as_deref().filter(|m| !m.is_empty()) {
        log_push(r, format!("{}: {m}", source.id));
    }
    let mut spec = storage::Spec::new("plugin", true, json!({ "source": name, "detail": "plugin", "plugin": source.plugin, "url": url, "lossless": lossless }),
        format!("Downloaded from {name}"));
    spec.hint = dl.cover_url.clone().or_else(|| chosen["cover"].as_str().map(String::from));
    match storage::import_checked(st, r, &files, &out, spec).await {
        Ok(_) => Ok(Out::Done),
        Err(e) if e.is::<Rejected>() => {
            rm(&out);
            Ok(Out::Next(format!("{name} gave the wrong audio ({e})")))
        }
        Err(e) => Err(e),
    }
}

/// Any link: downloaded as-is with yt-dlp, then tagged from what the files say. A link whose files could
/// not be imported is a failure with its reason, never "Downloaded 0 file(s)"; one whose songs were all in
/// the library already says so.
pub(super) async fn run_url(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    // two at a time, with the requests' YouTube downloads: "In line for bandcamp.com" meanwhile
    let label = crate::line::link_site(&r.s("url"));
    let _slot = take_line(st, r, &st.requests().lines.ytdlp, &label, "downloading", "Starting download…").await?;
    check(st, r.id)?;
    // the staging folder may have broken while it waited in line
    if downloads::hold(st, r, "url", false).await? {
        return Ok(());
    }
    let out = staging(st, r);
    let cb = progress_cb(st, r.id);
    let res = ytdlp::download(st, &r.s("url"), &out, true, Some(&cb)).await?;
    check(st, r.id)?;
    if res.files.is_empty() {
        fail(st, r, "yt-dlp produced no audio files").await;
        return Ok(());
    }
    // one video: the row shows its title and artist instead of the link
    if res.entries.len() == 1 && r.opt("title").is_none() {
        let e = &res.entries[0];
        let mut set = vec![];
        if let Some(t) = e["title"].as_str().filter(|t| !t.is_empty()) {
            set.push(("title", json!(t)));
        }
        if let (Some(a), None) = (e["artist"].as_str().filter(|a| !a.is_empty()), r.opt("artist")) {
            set.push(("artist", json!(a)));
        }
        upd(st, r, &set);
    }
    upd(st, r, &[("status", json!("importing")), ("message", json!("Tagging and importing…"))]);
    let hint = r.hint().or_else(|| (res.entries.len() == 1).then(|| res.entries[0]["thumbnail"].as_str().map(String::from)).flatten());
    let url = r.s("url");
    let host = reqwest::Url::parse(&url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or("link".into());
    let src = if host.contains("music.youtube") { "YouTube Music".to_string() } else { host.trim_start_matches("www.").to_string() };
    let mut spec = storage::Spec::new("ytdlp", true, json!({ "source": src, "detail": "direct link", "url": url }), "");
    spec.how = "url";
    spec.hint = hint;
    spec.entries = res.entries.iter().map(|e| json!({ "album": e["album"], "artist": e["artist"], "title": e["title"] })).collect();
    storage::import_checked(st, r, &res.files, &out, spec).await.map(|_| ())
}

/// A Qobuz, Tidal or Deezer link: downloaded with streamrip, each folder it made imported by its tags.
pub(super) async fn run_streamrip_url(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let url = r.s("url");
    // one release at a time per service, with the requests' streamrip downloads: "In line for Tidal" meanwhile
    let label = crate::line::streamrip_label(Some(&url), &streamrip::services(st));
    let line = st.requests().lines.streamrip(crate::line::service_of(&url).unwrap_or("deezer"));
    let _slot = take_line(st, r, &line, &label, "downloading", "Downloading with streamrip…").await?;
    check(st, r.id)?;
    // the staging folder may have broken while it waited in line
    if downloads::hold(st, r, "url", false).await? {
        return Ok(());
    }
    let out = staging(st, r);
    let files = match streamrip::download_run(st, &url, &out).await {
        Ok(run) => {
            storage::keep_output(r, "streamrip", &url, &run.log, true);
            run.files
        }
        Err(e) => {
            // the whole output stays with the request, in its details, before the failure (or the wait) is
            // recorded
            if let Some(log) = streamrip::run_log(&e) {
                log_push(r, format!("streamrip: {url} failed – its output is in the details"));
                storage::keep_output(r, "streamrip", &url, log, false);
                upd(st, r, &[]);
            }
            return Err(e);
        }
    };
    upd(st, r, &[("status", json!("importing")), ("message", json!("Tagging and importing…"))]);
    let svc = if url.contains("deezer") { "Deezer" } else if url.contains("tidal") { "Tidal" } else { "Qobuz" };
    let mut spec = storage::Spec::new("streamrip", true, json!({ "source": svc, "detail": "via streamrip", "url": url }), "");
    spec.how = "rip_url";
    storage::import_checked(st, r, &files, &out, spec).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{enqueue_failure, q, remote_queue, SLSKD_OFFLINE};
    use serde_json::json;

    /// A download waits in the user's own upload queue only while every file is "Queued, Remotely" and nothing
    /// arrived; its place there is the smallest slskd reports (Download now: switch to a free slot).
    #[test]
    fn the_remote_queue_place_is_read() {
        let f = |state: &str, place: Option<u64>, bytes: f64| json!({ "state": state, "placeInQueue": place, "bytesTransferred": bytes });
        assert_eq!(remote_queue(&[f("Queued, Remotely", Some(12), 0.0), f("Queued, Remotely", Some(15), 0.0)]), Some(Some(12)));
        assert_eq!(remote_queue(&[f("Queued, Remotely", None, 0.0), f("Queued, Remotely", None, 0.0)]), Some(None), "the place unknown");
        assert_eq!(remote_queue(&[f("Queued, Remotely", Some(12), 0.0), f("InProgress", None, 0.0)]), None, "one downloads");
        assert_eq!(remote_queue(&[f("Queued, Remotely", Some(12), 0.0), f("Queued, Remotely", Some(15), 100.0)]), None, "bytes arrived");
        assert_eq!(remote_queue(&[f("Queued, Locally", None, 0.0)]), None, "slskd's own limit");
        assert_eq!(remote_queue(&[] as &[serde_json::Value]), None);
    }

    /// A finished download that holds archives and no audio says so, in the words for its kind of client
    /// (diagnosis item 31): a torrent client has no unpacking to turn on.
    #[test]
    fn packed_releases_are_named_as_such() {
        let dir = std::env::temp_dir().join(format!("encore-packed-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("CD1")).unwrap();
        assert_eq!(super::packed(&dir, "qBittorrent", false), None, "no archives: no audio files, as before");
        std::fs::write(dir.join("CD1/album.part01.rar"), b"x").unwrap();
        // a torrent client never unpacks: no setting to point at
        assert_eq!(super::packed(&dir, "qBittorrent", false).as_deref(), Some("the release is packed (RAR/ZIP/7z), which Encore doesn't unpack"));
        // SABnzbd and NZBGet are asked to unpack Encore's downloads
        assert_eq!(super::packed(&dir, "SABnzbd", true).as_deref(), Some("the release is still packed (RAR/ZIP/7z) – SABnzbd didn't unpack it"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// slskd's 500 for a user who is offline reads as such; a pick of that user's files then says what
    /// to do (pick.rs) instead of "All sources exhausted (… 500 …)".
    #[test]
    fn an_offline_user_is_named() {
        let said = "Soulseek engine returned 500 Internal Server Error: User dave appears to be offline";
        assert_eq!(enqueue_failure("dave", said), "dave is offline on Soulseek");
        assert!(enqueue_failure("dave", said).ends_with(SLSKD_OFFLINE));
        assert_eq!(enqueue_failure("dave", "Soulseek engine returned 404 Not Found: x"), "Soulseek engine returned 404 Not Found: x");
    }

    #[test]
    fn queries_spell_out_the_symbols_that_stand_for_letters() {
        assert_eq!(q(&["Ke$ha", "Cannibal"]), "Kesha Cannibal");
        assert_eq!(q(&["P!nk", "Funhouse"]), "Pink Funhouse");
        assert_eq!(q(&["A$AP Rocky", "Testing (Deluxe)"]), "AsAP Rocky Testing");
        // other symbols still go, and empty parts are left out
        assert_eq!(q(&["AC/DC", "", "Back in Black [Remastered]"]), "AC DC Back in Black");
        assert_eq!(q(&["Panic! at the Disco", "Pray for the Wicked"]), "Panic at the Disco Pray for the Wicked");
    }
}
