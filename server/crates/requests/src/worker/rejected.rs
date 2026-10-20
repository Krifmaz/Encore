//! Downloads the audio check turned away ([`Rejected`] by [`Check::Audio`]): the fingerprint said the file
//! is another song, and it is sometimes wrong (a re-upload, a version AcoustID files under another song).
//! Such a download is not deleted: its files move to `<staging>/rejected-<id>/<n>/` and the request records
//! it in `detail.rejected` (the newest [`KEEP_MAX`]: the source, the link or user, the file, the check's
//! words, what the fingerprint heard and when). The request goes on to its next candidate or source as
//! before; a hand pick fails at once with the reason instead (it never falls through to other sources).
//!
//! Keep anyway ([`keep_anyway`], `POST /api/requests/{id}/keep-anyway`) imports the kept files without the
//! check through the import of kept files ([`super::storage`]), and marks the songs as confirmed by that
//! person – the library's "This is the right song" (`verify_status` "manual"), which the verify task never
//! looks at again. When the files are gone, Download anyway ([`download_anyway`]) asks for the same source
//! and link again as a hand pick with the check off for this request (`detail.skip_check`, cleared once
//! the download is in). A hand pick can turn the check off itself (the picker's "Don't check the audio").
//!
//! The kept files go when the request completes or is deleted, and 7 days after they were turned away
//! ([`tick`]: the worker's hourly look, on the blocking pool).
use super::*;
use encore_core::fsx::{self, Step};

/// How many rejected downloads a request keeps (the newest; an older one's files go).
pub const KEEP_MAX: usize = 3;
/// How long a rejected download is kept (seconds).
pub const KEEP_SECS: f64 = 7.0 * 86400.0;
/// How often the worker removes rejected downloads past their time (seconds).
const SWEEP_SECS: f64 = 3600.0;

/// `<staging>/rejected-<id>`: beside the request's `request-<id>` folder, which every new download clears
/// (and the clean-up task's `request-*` sweep never touches it). None without a staging folder.
pub fn dir_of(st: &impl Ctx, id: i64) -> Option<PathBuf> {
    let root = st.cfg().str("staging_path");
    (!root.trim().is_empty()).then(|| PathBuf::from(root).join(format!("rejected-{id}")))
}

/// Whether the check is off for this request's import: Keep anyway, Download anyway, or a hand pick with
/// "Don't check the audio" (`detail.skip_check`: {by, why: "keep" | "download" | "pick", at}).
pub(super) fn skips_check(d: &Value) -> bool {
    d["skip_check"].is_object()
}

/// What a check that was off adds to the request's words ("kept anyway by alice").
pub(super) fn skip_note(d: &Value) -> Option<String> {
    let s = &d["skip_check"];
    let by = s["by"].as_str().filter(|b| !b.is_empty()).unwrap_or("someone");
    match s["why"].as_str()? {
        "keep" => Some(format!("kept anyway by {by}")),
        "download" => Some(format!("downloaded anyway by {by}, without the audio check")),
        _ => Some(format!("picked by {by} without the audio check")),
    }
}

/// The songs a person accepted without the check are confirmed as the library's "This is the right song"
/// does (`verify_status` "manual", "Confirmed by alice"): the verify task never flags them.
pub(super) fn confirm(st: &impl Ctx, d: &Value, paths: &[PathBuf]) {
    let Some(by) = d["skip_check"]["by"].as_str().filter(|_| skips_check(d)) else { return };
    for p in paths {
        let _ = st.db().execute("UPDATE tracks SET verify_status='manual', verify_detail=?, verified_at=? WHERE path=?",
            p![format!("Confirmed by {by}"), now(), p.to_string_lossy()]);
    }
}

/// The files of a rejected download moved into `dir` (a rename, or a copy when it is another disk). A
/// copied file's original goes only once every file is in, so nothing is lost when one can't be moved (a
/// full disk halfway through an album): Err, with the renamed files renamed back and the copies removed,
/// and the download is then recorded without files. `dir` goes only when everything was put back.
fn move_files(files: &[PathBuf], dir: &Path) -> Result<Vec<PathBuf>, fsx::StepError> {
    fsx::mkdir_all(dir, Step::Stage)?;
    let mut out: Vec<PathBuf> = vec![];
    // (the original, where it went, whether it was copied: its original is still there)
    let mut done: Vec<(PathBuf, PathBuf, bool)> = vec![];
    for f in files {
        let name = f.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "file".into());
        let mut to = dir.join(&name);
        let mut i = 2;
        while to.exists() {
            to = dir.join(format!("{i} {}", name.to_string_lossy()));
            i += 1;
        }
        let res = fsx::rename(f, &to, Step::Stage).map(|_| false).or_else(|_| fsx::copy(f, &to, Step::Stage).map(|_| true));
        match res {
            Ok(copied) => {
                done.push((f.clone(), to.clone(), copied));
                out.push(to);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&to);
                let mut all_back = true;
                for (from, to, copied) in done {
                    let back = if copied { std::fs::remove_file(&to) } else { std::fs::rename(&to, &from) };
                    all_back &= back.is_ok();
                }
                if all_back {
                    let _ = std::fs::remove_dir_all(dir);
                }
                return Err(e);
            }
        }
    }
    for (from, _, copied) in &done {
        if *copied {
            let _ = std::fs::remove_file(from);
        }
    }
    Ok(out)
}

/// What Download anyway asks for again: the hand pick itself, else the source's link or candidate in the
/// picker's terms (null when there is none: YouTube matched song by song).
fn redo_pick(r: &Req, kept: &Value) -> Value {
    if r.d["pick"].is_object() {
        return r.d["pick"].clone();
    }
    let title = if r.s("kind") == "album" { r.album_title() } else { r.s("title") };
    let url = kept["origin"]["url"].as_str().filter(|u| !u.is_empty());
    let current = |key: &str| {
        let idx = r.d[key]["idx"].as_i64().unwrap_or(-1);
        r.d[key]["candidates"].get(idx.max(0) as usize).filter(|_| idx >= 0).cloned()
    };
    match (kept["source"].as_str().unwrap_or(""), url) {
        ("ytdlp", Some(u)) => json!({ "source": "ytmusic", "payload": { "url": u }, "title": title }),
        ("streamrip", Some(u)) => json!({ "source": "link", "payload": { "url": u }, "title": title }),
        ("slskd" | "slskd_lossless", _) => current("slskd").map(|c| json!({ "source": "slskd", "payload": c, "title": title })).unwrap_or(Value::Null),
        ("torznab", _) => current("torznab").map(|c| json!({ "source": "torznab", "payload": c, "title": title })).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// A download the audio check turned away: its files move to the request's rejected folder and the
/// request records it (`detail.rejected`, the newest [`KEEP_MAX`]; an older one's files go). `kept`: the
/// import's record of the download (`detail.kept`: its source, origin, words, cover). Nothing here stops
/// the request: a file that can't be moved stays where it was (its source removes it as before) and the
/// record is kept without files, for Download anyway.
pub(super) async fn keep(st: &impl Ctx, r: &mut Req, files: &[PathBuf], kept: &Value, rej: &Rejected) {
    let n = r.d["rejected"].as_array().into_iter().flatten().filter_map(|e| e["id"].as_i64()).max().unwrap_or(0)
        .max(r.d["rejected_seq"].as_i64().unwrap_or(0)) + 1;
    r.d["rejected_seq"] = json!(n);
    let src = kept["source"].as_str().filter(|s| !s.is_empty()).map(String::from).or(r.opt("source")).unwrap_or("import".into());
    let mut moved: Vec<PathBuf> = vec![];
    let mut dir = Value::Null;
    if let Some(base) = dir_of(st, r.id) {
        let d = base.join(n.to_string());
        let (fs, to) = (files.to_vec(), d.clone());
        match tokio::task::spawn_blocking(move || move_files(&fs, &to)).await {
            Ok(Ok(m)) => {
                moved = m;
                dir = json!(d);
                // the folders the download came in, when nothing else is left in them
                if let Some(root) = kept["root"].as_str().filter(|x| !x.is_empty()) {
                    for f in files {
                        if let Some(parent) = f.parent() {
                            encore_library::prune_empty(parent, Path::new(root));
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                let line = format!("Request #{} ({}): the rejected download could not be kept – {e}", r.id, r.label());
                tracing::warn!("{line}");
                st.db().log("warning", &line);
            }
            Err(_) => {}
        }
    }
    let origin = kept["origin"].clone();
    let entry = json!({
        "id": n, "source": src, "via": origin["source"], "detail": origin["detail"], "url": origin["url"],
        "file": files.first().and_then(|f| f.file_name()).map(|f| f.to_string_lossy().to_string()), "count": files.len(),
        "files": moved, "dir": dir, "check": rej.check.as_str(), "reason": rej.why, "heard": rej.heard, "text": rej.text, "at": now(),
        "pick": redo_pick(r, kept), "origin": origin, "done": kept["done"], "hint": kept["hint"],
    });
    let mut list: Vec<Value> = r.d["rejected"].as_array().cloned().unwrap_or_default();
    list.push(entry);
    while list.len() > KEEP_MAX {
        let old = list.remove(0);
        if let Some(d) = old["dir"].as_str().filter(|d| !d.is_empty()) {
            let _ = std::fs::remove_dir_all(d);
        }
    }
    r.d["rejected"] = json!(list);
}

/// A hand pick the audio check turned away fails at once with the reason (the row offers Keep anyway).
pub(super) async fn fail_pick(st: &impl Ctx, r: &mut Req, rej: &Rejected) {
    let src = r.opt("source").unwrap_or("import".into());
    log_push(r, format!("{src}: {}", rej.why));
    storage::log_failure(st, r, &src, &rej.why);
    fail(st, r, &rej.text).await;
}

/// A recorded rejected download that Keep anyway imported and the quality or release check turned away
/// (`refused`: the words): its files stay until the sweep, and neither Keep anyway nor Download anyway is
/// offered for it again (it would bring the same files).
fn refused(e: &Value) -> Option<&str> {
    e["refused"].as_str().filter(|x| !x.is_empty())
}

/// A recorded rejected download's files, when every one of them is still there.
fn present(e: &Value) -> Vec<PathBuf> {
    let files: Vec<PathBuf> = e["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()).map(PathBuf::from).collect();
    if !files.is_empty() && files.iter().all(|f| f.is_file()) { files } else { vec![] }
}

/// What the requests list adds to a row from its `detail`: `rejected` (the downloads the audio check turned
/// away, oldest first: {id, source, via, detail, url, file, count, check, reason, heard, text, at, has_file,
/// path}; `path`, the folder they are kept in, only for admins) and `anyway` (what the row offers:
/// {action: "keep" | "download", id, text}, or null).
pub fn row_fields(status: &str, d: &Value, admin: bool) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    let list: Vec<Value> = d["rejected"].as_array().cloned().unwrap_or_default();
    let mut rows = vec![];
    let mut keep: Option<&Value> = None;
    let mut download: Option<&Value> = None;
    for e in list.iter().rev() {
        let has = !present(e).is_empty();
        let offered = refused(e).is_none();
        if offered && has && keep.is_none() {
            keep = Some(e);
        }
        if offered && !has && e["pick"].is_object() && download.is_none() {
            download = Some(e);
        }
        rows.push(json!({ "id": e["id"], "source": e["source"], "via": e["via"], "detail": e["detail"], "url": e["url"], "file": e["file"], "count": e["count"],
                          "check": e["check"], "reason": e["reason"], "heard": e["heard"], "text": e["text"], "at": e["at"], "has_file": has,
                          "path": if admin && has { e["dir"].clone() } else { Value::Null } }));
    }
    rows.reverse();
    let anyway = match (open(status, d), keep, download) {
        (Ok(()), Some(e), _) => json!({ "action": "keep", "id": e["id"], "text": e["text"] }),
        (Ok(()), None, Some(e)) => json!({ "action": "download", "id": e["id"], "text": e["text"] }),
        _ => Value::Null,
    };
    m.insert("rejected".into(), json!(rows));
    m.insert("anyway".into(), anyway);
    m
}

/// Whether a request can take Keep anyway or Download anyway now: a failed request, or one still looking
/// (moved on to its next candidate or source, or waiting for Soulseek). Err: why not (an HTTP status and
/// the words).
fn open(status: &str, d: &Value) -> Result<(), (u16, String)> {
    let storage_wait = status == "waiting" && matches!(d["wait"]["reason"].as_str(), Some(storage::STORAGE_IMPORT | storage::STORAGE_DOWNLOAD));
    match status {
        "importing" => Err((409, "It is being imported now".into())),
        "completed" => Err((409, "The request is already done".into())),
        _ if storage_wait || (d["kept"].is_object() && storage::has_files(d)) => {
            Err((409, "This request has downloaded files waiting to be imported – Import again or cancel it first".into()))
        }
        _ if skips_check(d) && status != "failed" => Err((409, "It is being downloaded without the audio check now".into())),
        "failed" | "queued" | "searching" | "downloading" | "monitoring" | "waiting" => Ok(()),
        _ => Err((409, "Only a failed or running request can take a rejected download".into())),
    }
}

/// The recorded rejected download `which` (its id), else the newest one `want` accepts.
fn entry(d: &Value, which: Option<i64>, want: impl Fn(&Value) -> bool) -> Option<Value> {
    let list = d["rejected"].as_array()?;
    match which {
        Some(id) => list.iter().find(|e| e["id"].as_i64() == Some(id)).cloned(),
        None => list.iter().rev().find(|e| want(e)).cloned(),
    }
}

/// Stop what a running request does now (its task, and the download a client has of its current
/// candidate), for Keep anyway or Download anyway. Its place in the queue stays taken until the returned
/// hold goes, after [`take_over`] wrote the new state: a step started in between would act on the download
/// stopped here.
async fn stop(st: &impl Ctx, id: i64, status: &str) -> Option<super::Hold> {
    if ACTIVE.contains(&status) {
        Some(super::cancel_holding(st, id).await)
    } else {
        None
    }
}

/// The rejected download Keep anyway imports: `which` (else the newest whose files are still there) and
/// its files. Err: (HTTP status, why not).
fn keep_choice(r: &Req, which: Option<i64>) -> Result<(Value, Vec<PathBuf>), (u16, String)> {
    open(&r.s("status"), &r.d)?;
    if !r.d["rejected"].as_array().is_some_and(|a| !a.is_empty()) {
        return Err((400, "There is no rejected download to keep".into()));
    }
    let gone = || (400, "The rejected download is gone – Download anyway asks for it again".to_string());
    let e = entry(&r.d, which, |e| refused(e).is_none() && !present(e).is_empty()).ok_or_else(gone)?;
    if let Some(why) = refused(&e) {
        return Err((409, format!("{why} – pick another download")));
    }
    let files = present(&e);
    if files.is_empty() {
        return Err(gone());
    }
    Ok((e, files))
}

/// The rejected download Download anyway asks for again: `which` (else the newest that can be asked for
/// again and whose files are gone). Err: (HTTP status, why not).
fn download_choice(r: &Req, which: Option<i64>) -> Result<Value, (u16, String)> {
    open(&r.s("status"), &r.d)?;
    if !r.d["rejected"].as_array().is_some_and(|a| !a.is_empty()) {
        return Err((400, "There is no rejected download to ask for again".into()));
    }
    let cant = || (400, "This download can't be asked for again – pick one by hand".to_string());
    let e = entry(&r.d, which, |e| e["pick"].is_object() && refused(e).is_none() && present(e).is_empty()).ok_or_else(cant)?;
    if let Some(why) = refused(&e) {
        return Err((409, format!("{why} – pick another download")));
    }
    if !present(&e).is_empty() {
        return Err((409, "The download is still kept – Keep anyway imports it".into()));
    }
    if !e["pick"].is_object() {
        return Err(cant());
    }
    Ok(e)
}

/// A request's new row: status, progress, message, source.
type NewRow<'a> = (&'a str, i64, &'a str, Value);

/// Write the request's new state, only while it still has the status that was looked at, with no task of
/// the queue running for it (one started meanwhile is stopped; the queue starts none while this writes).
fn take_over(st: &impl Ctx, id: i64, old_status: &str, (status, progress, message, source): NewRow<'_>, d: &Value) -> Result<(), (u16, String)> {
    let tasks = st.requests().tasks.0.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(h) = tasks.get(&id) {
        h.abort();
    }
    let n = st.db().execute("UPDATE requests SET status=?, progress=?, message=?, source=?, detail=?, updated_at=? WHERE id=? AND status=?",
        p![status, progress, message, source, d.to_string(), now(), id, old_status]).map_err(|e| (500, e.to_string()))?;
    drop(tasks);
    if n == 0 {
        return Err((409, "The request changed meanwhile – try again".into()));
    }
    crate::events::emit(st, id);
    Ok(())
}

/// Keep anyway: the rejected download `which` (else the newest whose files are still there) is imported
/// without the audio check at the queue's next look, through the import of kept files (waiting,
/// `storage_import`, its time now: a storage problem waits as any import does), and its songs are marked
/// as confirmed by `who`. A running request stops what it does first. Err: (HTTP status, why not).
pub async fn keep_anyway(st: &impl Ctx, id: i64, which: Option<i64>, who: &str) -> Result<(), (u16, String)> {
    let not_found = || (404, "Request not found".to_string());
    let r = get(st, id).ok_or_else(not_found)?;
    let (e, _) = keep_choice(&r, which)?;
    let _hold = stop(st, id, &r.s("status")).await;
    // the request as its stopped task left it
    let r = get(st, id).ok_or_else(not_found)?;
    let (e, files) = keep_choice(&r, e["id"].as_i64())?;
    let status = r.s("status");
    let t = now();
    let mut d = r.d.clone();
    let done = e["done"].as_str().filter(|x| !x.is_empty()).unwrap_or("Downloaded").to_string();
    d["kept"] = json!({ "files": files, "root": e["dir"], "source": e["source"], "how": "request", "own": true, "origin": e["origin"], "done": done,
                        "hint": e["hint"], "reason": "storage" });
    d["skip_check"] = json!({ "by": who, "why": "keep", "at": t, "rejected": e["id"] });
    d["sources"] = json!([]);
    d["wait"] = json!({ "reason": storage::STORAGE_IMPORT, "since": t, "next_try_at": t });
    if let Some(o) = d.as_object_mut() {
        o.remove("soulseek_wait");
    }
    let via = e["via"].as_str().or(e["source"].as_str()).unwrap_or("its source").to_string();
    let mut tmp = Req { id, row: r.row.clone(), d };
    log_push(&mut tmp, format!("{who} kept the rejected download from {via} anyway"));
    take_over(st, id, &status, ("waiting", 100, "Waiting to import the kept download", e["source"].clone()), &tmp.d)?;
    let line = format!("{who} kept the rejected download of request #{id} ({}) anyway – it is imported without the audio check", r.label());
    tracing::info!("{line}");
    st.db().log("info", &line);
    Ok(())
}

/// Keep anyway turns off the audio check only: a kept download the quality check (fake lossless,
/// upsampled, the wrong length) or the release check turns away ends the request with that reason. The
/// check is on again for whatever comes next. The files a person chose to keep are not deleted for a
/// check's verdict: they stay where they are kept (the sweep removes them after 7 days; an admin sees the
/// folder), and the download is marked `refused`, so neither Keep anyway nor Download anyway offers it
/// again (asking for it again would bring the same files).
pub(super) async fn kept_turned_away(st: &impl Ctx, r: &mut Req, e: &anyhow::Error) -> anyhow::Result<()> {
    let which = r.d["skip_check"]["rejected"].as_i64();
    let check = e.downcast_ref::<Rejected>().map(|x| x.check).unwrap_or(Check::Quality);
    let what = match check {
        Check::Release => "the release check",
        _ => "the quality check",
    };
    if let Some(list) = r.d["rejected"].as_array_mut() {
        for x in list.iter_mut().filter(|x| x["id"].as_i64() == which) {
            x["refused"] = json!(format!("Kept anyway, but {what} turned it away: {e}"));
        }
    }
    if let Some(d) = r.d.as_object_mut() {
        d.remove("skip_check");
        d.remove("kept");
        d.remove("wait");
    }
    let src = r.opt("source").unwrap_or("import".into());
    log_push(r, format!("{src}: kept anyway, but turned away by {what}: {e}"));
    fail(st, r, &format!("Kept anyway, but {what} turned it away: {e}")).await;
    Ok(())
}

/// The keys a request keeps when Download anyway starts it over (what was asked for, a reported song's
/// replacement, the rejected downloads).
const CARRIED: [&str; 10] = ["length", "isrc", "ext", "album_mode", "replace", "avoid", "release", "recording", "rejected", "rejected_seq"];

/// Download anyway: the rejected download `which` (else the newest that can be asked for again) is
/// downloaded again from the same source and link as a hand pick, with the audio check off for this request
/// (`detail.skip_check`, cleared once the download is in); its songs are marked as confirmed by `who`. A
/// running request stops what it does first; a failed one counts its attempt as Retry does. Err: (HTTP
/// status, why not).
pub async fn download_anyway(st: &impl Ctx, id: i64, which: Option<i64>, who: &str) -> Result<(), (u16, String)> {
    let not_found = || (404, "Request not found".to_string());
    let r = get(st, id).ok_or_else(not_found)?;
    let e = download_choice(&r, which)?;
    let _hold = stop(st, id, &r.s("status")).await;
    // the request as its stopped task left it
    let r = get(st, id).ok_or_else(not_found)?;
    let e = download_choice(&r, e["id"].as_i64())?;
    let status = r.s("status");
    let t = now();
    let mut base = json!({});
    for k in CARRIED {
        if !r.d[k].is_null() {
            base[k] = r.d[k].clone();
        }
    }
    base["pick"] = e["pick"].clone();
    let mut d = if status == "failed" {
        crate::create::detail_for_retry(&r.d, &status, &r.s("message"), base)
    } else {
        base["attempts"] = r.d["attempts"].clone();
        base["log"] = r.d["log"].clone();
        base
    };
    d["skip_check"] = json!({ "by": who, "why": "download", "at": t, "rejected": e["id"] });
    let via = e["via"].as_str().or(e["source"].as_str()).unwrap_or("its source").to_string();
    let mut tmp = Req { id, row: r.row.clone(), d };
    log_push(&mut tmp, format!("{who} asked for the rejected download from {via} again, without the audio check"));
    take_over(st, id, &status, ("queued", 0, "Downloading it again without the audio check", Value::Null), &tmp.d)?;
    let line = format!("{who} asked for the rejected download of request #{id} ({}) again – it is imported without the audio check", r.label());
    tracing::info!("{line}");
    st.db().log("info", &line);
    Ok(())
}

/// Remove a request's rejected downloads (it completed, or it was deleted).
pub fn remove(st: &impl Ctx, id: i64) -> bool {
    dir_of(st, id).is_some_and(|d| d.is_dir() && !kept_in_library(st, &d) && std::fs::remove_dir_all(&d).is_ok())
}

/// A folder that is (or holds) an extra library folder (kept where it is by the file manager's import) is
/// never removed by the clean-up: its songs are the library's.
fn kept_in_library(st: &impl Ctx, dir: &Path) -> bool {
    let kept = encore_library::folders::touches_extra(st.cfg(), dir);
    if kept {
        tracing::info!("{} holds an extra library folder: the clean-up leaves it", dir.display());
    }
    kept
}

/// How long a folder no request names yet is left alone by [`sweep`] (seconds): [`keep`] fills it (a copy
/// of an album from another disk takes a while) before the request records it.
const FILLING_SECS: u64 = 3600;

/// Remove the rejected downloads past their 7 days, and every one of a request that completed or was
/// deleted (a cancelled request can be retried or asked for again, and its rejected downloads come along:
/// they keep their 7 days); a download Keep anyway is importing (`detail.kept`) stays, and so does a folder
/// that is still being filled. A request the database can't be asked about now is left for the next
/// sweep. → how many folders went
pub fn sweep(st: &impl Ctx) -> usize {
    let root = st.cfg().str("staging_path");
    if root.trim().is_empty() {
        return 0;
    }
    let Ok(rd) = std::fs::read_dir(&root) else { return 0 };
    let t = now();
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_prefix("rejected-").and_then(|x| x.parse::<i64>().ok()) else { continue };
        let dir = e.path();
        // only a request that isn't there is deleted: a database that doesn't answer now (busy, a restore
        // swapping it) says nothing about the request
        let row = match st.db().one("SELECT status, detail FROM requests WHERE id=?", p![id]) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Rejected downloads of request #{id} left for the next sweep: the database didn't answer ({e})");
                continue;
            }
        };
        if kept_in_library(st, &dir) {
            continue;
        }
        let gone = row.as_ref().is_none_or(|r| r.s("status") == "completed");
        if gone {
            if std::fs::remove_dir_all(&dir).is_ok() {
                n += 1;
            }
            continue;
        }
        let d: Value = row.map(|r| serde_json::from_str(&r.s("detail")).unwrap_or(json!({}))).unwrap_or(json!({}));
        let entries: Vec<&Value> = d["rejected"].as_array().into_iter().flatten().collect();
        let mut keep: Vec<PathBuf> = entries.iter().filter(|x| x["at"].as_f64().is_some_and(|at| t - at < KEEP_SECS))
            .filter_map(|x| x["dir"].as_str().map(PathBuf::from)).collect();
        if let Some(k) = d["kept"]["root"].as_str() {
            keep.push(PathBuf::from(k));
        }
        let named: Vec<PathBuf> = entries.iter().filter_map(|x| x["dir"].as_str().map(PathBuf::from)).collect();
        if let Ok(subs) = std::fs::read_dir(&dir) {
            for s in subs.flatten() {
                let p = s.path();
                if keep.contains(&p) || (!named.contains(&p) && filling(&s)) {
                    continue;
                }
                if std::fs::remove_dir_all(&p).or_else(|_| std::fs::remove_file(&p)).is_ok() {
                    n += 1;
                }
            }
        }
        let _ = std::fs::remove_dir(&dir); // only when nothing is left
    }
    if n > 0 {
        let line = format!("Removed {n} rejected download(s) kept for Keep anyway");
        tracing::info!("{line}");
        st.db().log("info", &line);
    }
    n
}

/// A folder changed within the last [`FILLING_SECS`] (or whose time can't be read): [`keep`] may still be
/// filling it.
fn filling(e: &std::fs::DirEntry) -> bool {
    e.metadata().and_then(|m| m.modified()).ok()
        .and_then(|m| std::time::SystemTime::now().duration_since(m).ok())
        .is_none_or(|age| age.as_secs() < FILLING_SECS)
}

/// The worker's look every few seconds: once an hour (per staging folder) [`sweep`] runs on the blocking
/// pool; the worker's loop goes on without waiting for it. → the sweep started now, if any
pub fn tick<C: Ctx>(st: &C) -> Option<tokio::task::JoinHandle<usize>> {
    static LAST: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);
    let key = st.cfg().str("staging_path");
    let t = now();
    {
        let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
        if last.get(&key).is_some_and(|at| t - at < SWEEP_SECS) {
            return None;
        }
        last.insert(key, t);
    }
    let st = st.clone();
    Some(tokio::task::spawn_blocking(move || sweep(&st)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_offers_keep_while_the_files_are_there_else_download() {
        let dir = std::env::temp_dir().join(format!("encore-rejected-row-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.opus");
        std::fs::write(&f, b"x").unwrap();
        let entry = |id: i64, files: Vec<&Path>, pick: Value| json!({ "id": id, "source": "ytdlp", "files": files, "dir": dir, "text": format!("t{id}"), "pick": pick });
        let pick = json!({ "source": "ytmusic", "payload": { "url": "https://music.youtube.com/watch?v=1" } });
        let d = json!({ "rejected": [entry(1, vec![&f], Value::Null), entry(2, vec![], pick.clone())] });
        let m = row_fields("failed", &d, true);
        assert_eq!(m["anyway"], json!({ "action": "keep", "id": 1, "text": "t1" }));
        assert_eq!(m["rejected"][0]["has_file"], true);
        assert_eq!(m["rejected"][0]["path"], json!(dir));
        assert_eq!(row_fields("failed", &d, false)["rejected"][0]["path"], Value::Null, "no server folders for other users");
        // the files gone: Download anyway
        std::fs::remove_file(&f).unwrap();
        assert_eq!(row_fields("failed", &d, true)["anyway"], json!({ "action": "download", "id": 2, "text": "t2" }));
        // running requests too; finished, importing, or already without the check: nothing
        assert_eq!(row_fields("downloading", &d, true)["anyway"]["action"], "download");
        for s in ["completed", "importing", "cancelled", "pending"] {
            assert_eq!(row_fields(s, &d, true)["anyway"], Value::Null, "{s}");
        }
        let mut busy = d.clone();
        busy["skip_check"] = json!({ "by": "a", "why": "download" });
        assert_eq!(row_fields("downloading", &busy, true)["anyway"], Value::Null);
        assert_eq!(row_fields("failed", &json!({}), true)["anyway"], Value::Null);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that can't be moved halfway through a download: the files already moved go back where they
    /// were (one copied from another disk is removed, its original never left), so nothing of the download
    /// is lost; once every file can be moved, a copied file's original goes.
    #[test]
    fn a_move_that_fails_halfway_puts_everything_back() {
        let tag = format!("encore-rejected-move-{}", std::process::id());
        let base = std::env::temp_dir().join(&tag);
        let (src, dir) = (base.join("src"), base.join("kept"));
        std::fs::create_dir_all(&src).unwrap();
        let a = src.join("01.flac");
        std::fs::write(&a, b"a").unwrap();
        let mut files = vec![a];
        // a file on another disk (tmpfs) is copied, not renamed
        let other = Path::new("/dev/shm").is_dir().then(|| Path::new("/dev/shm").join(&tag));
        if let Some(o) = &other {
            std::fs::create_dir_all(o).unwrap();
            std::fs::write(o.join("02.flac"), b"b").unwrap();
            files.push(o.join("02.flac"));
        }
        let all = files.clone();
        files.push(src.join("03.flac")); // not there: it can't be moved
        assert!(move_files(&files, &dir).is_err());
        for f in &all {
            assert!(f.is_file(), "{} is back", f.display());
        }
        assert!(!dir.exists(), "the folder goes when everything went back");
        let moved = move_files(&all, &dir).unwrap();
        assert_eq!(moved.len(), all.len());
        assert!(moved.iter().all(|m| m.starts_with(&dir) && m.is_file()), "{moved:?}");
        assert!(all.iter().all(|f| !f.exists()), "the originals went, the copied one's too");
        let _ = std::fs::remove_dir_all(&base);
        if let Some(o) = other {
            let _ = std::fs::remove_dir_all(o);
        }
    }

    #[test]
    fn the_note_names_who_turned_the_check_off() {
        assert_eq!(skip_note(&json!({ "skip_check": { "by": "alice", "why": "keep" } })).as_deref(), Some("kept anyway by alice"));
        assert_eq!(skip_note(&json!({ "skip_check": { "by": "bob", "why": "pick" } })).as_deref(), Some("picked by bob without the audio check"));
        assert_eq!(skip_note(&json!({})), None);
    }
}
