//! Report a problem with a song or an album (⋯ → Report a problem): the exact source the file came from
//! is blocked (never picked again), the file goes to <config>/trash (kept 7 days), and the same song or
//! album is requested again with that source left out and other users / services tried first. The
//! replacement takes over the reported track's id, so plays, likes, playlists and resume spots stay.
//! The routes (and Settings → Blocked) are the server's; the worker asks here what is blocked.
use crate::create::{create_request, NewRequest};
use crate::Ctx;
use encore_config::perms::is_admin;
use encore_core::error::{err, ApiResult};
use encore_core::text::words;
use encore_db::{now, p, Row, RowExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path as FsPath, PathBuf};

/// (key, label) of every reason, in menu order (the same words in both web UIs and the phone app).
pub const REASONS: [(&str, &str); 7] = [
    ("wrong_song", "Wrong song"),
    ("wrong_version", "Wrong version (live, remix, edit, missing parts)"),
    ("bad_audio", "Bad audio quality"),
    ("loudness", "Too quiet or too loud"),
    ("fake_lossless", "Fake lossless"),
    ("glitches", "Skips or glitches"),
    ("other", "Other"),
];

const TRASH_DAYS: f64 = 7.0;
/// A report still 'filing' after this long was interrupted (server stopped while parking files).
const FILING_STUCK_SECS: f64 = 3600.0;

pub fn trash_dir(st: &impl Ctx) -> PathBuf {
    st.data_dir().join("trash")
}

// ---- where a file came from (track_origin) and what is blocked ----------------------------------

/// The exact source a download came from, as (kind, key, label). Workers put an explicit
/// `origin.block` = {kind, key, label}; otherwise it is read from the link (YouTube, streamrip).
pub fn block_of_origin(origin: &Value) -> Option<(String, String, String)> {
    let b = &origin["block"];
    if let (Some(k), Some(key)) = (b["kind"].as_str(), b["key"].as_str().filter(|k| !k.is_empty())) {
        return Some((k.to_string(), key.to_string(), b["label"].as_str().unwrap_or(key).to_string()));
    }
    let url = origin["url"].as_str().unwrap_or("");
    block_of_url(url)
}

/// A YouTube video / playlist or a Deezer / Tidal / Qobuz item from its link.
pub fn block_of_url(url: &str) -> Option<(String, String, String)> {
    let u = reqwest::Url::parse(url).ok()?;
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").to_string();
    let q = |k: &str| u.query_pairs().find(|(a, _)| a == k).map(|(_, v)| v.to_string());
    if host.contains("youtube.com") || host == "youtu.be" {
        let id = if host == "youtu.be" { u.path().trim_matches('/').to_string() } else { q("v").or_else(|| q("list").map(|l| format!("list:{l}")))? };
        return (!id.is_empty()).then(|| ("youtube".to_string(), id.clone(), format!("YouTube {id}")));
    }
    for svc in ["deezer", "tidal", "qobuz"] {
        if host.contains(svc) {
            let segs: Vec<&str> = u.path().split('/').filter(|s| !s.is_empty()).collect();
            let id = segs.last()?.to_string();
            let what = segs.iter().rev().nth(1).copied().unwrap_or("item");
            return Some(("streamrip".into(), format!("{svc}:{id}"), format!("{} {what} {id}", svc[..1].to_uppercase() + &svc[1..])));
        }
    }
    None
}

/// A torrent / Usenet release: the infohash of a magnet, else the indexer's guid, else the link.
pub fn block_of_release(c: &Value) -> Option<(String, String, String)> {
    let kind = if c["protocol"] == "usenet" { "usenet" } else { "torrent" };
    let url = c["url"].as_str().unwrap_or("");
    let hash = url.split(['?', '&']).find_map(|p| p.strip_prefix("xt=urn:btih:")).map(|h| h.to_lowercase());
    let key = hash.or(c["guid"].as_str().filter(|g| !g.is_empty()).map(String::from)).or(Some(url.to_string()).filter(|u| !u.is_empty()))?;
    Some((kind.to_string(), key, c["title"].as_str().unwrap_or("").to_string()))
}

/// Soulseek: one file (user|path|size) for a song, the whole folder (user|folder) for an album.
pub fn block_of_slskd(c: &Value, whole_folder: bool) -> Option<(String, String, String)> {
    let user = c["username"].as_str().filter(|u| !u.is_empty())?;
    let files = c["files"].as_array().cloned().unwrap_or_default();
    if !whole_folder && files.len() == 1 {
        let f = &files[0];
        let name = f["filename"].as_str().unwrap_or("");
        return Some(("slskd".into(), format!("{user}|{name}|{}", f["size"].as_i64().unwrap_or(0)), format!("{user}: {}", name.replace('\\', "/"))));
    }
    let dir = c["directory"].as_str().unwrap_or("");
    Some(("slskd".into(), format!("{user}|{dir}"), format!("{user}: {}", dir.replace('\\', "/"))))
}

/// After an import: remember the exact source of each new file.
pub fn record_origin(st: &impl Ctx, paths: &[PathBuf], origin: &Value) {
    let Some((kind, key, label)) = block_of_origin(origin) else { return };
    for pth in paths {
        if let Ok(Some(t)) = st.db().one("SELECT id FROM tracks WHERE path=?", p![pth.to_string_lossy()]) {
            let _ = st.db().execute("INSERT OR REPLACE INTO track_origin (track_id, kind, key, label) VALUES (?,?,?,?)", p![t.i("id"), kind, key, label]);
        }
    }
}

/// The source of a library track: track_origin, else what the older columns say.
fn origin_of_track(st: &impl Ctx, t: &Row) -> Option<(String, String, String)> {
    if let Ok(Some(o)) = st.db().one("SELECT kind, key, label FROM track_origin WHERE track_id=?", p![t.i("id")]) {
        return Some((o.s("kind"), o.s("key"), o.s("label")));
    }
    if let Some(b) = block_of_url(&t.s("source_url")) {
        return Some(b);
    }
    let (src, detail) = (t.s("source"), t.s("source_detail"));
    if src == "Soulseek" {
        // "user NAME · folder"
        let rest = detail.strip_prefix("user ")?;
        let (user, dir) = rest.split_once(" · ")?;
        return Some(("slskd".into(), format!("{user}|{}", dir.replace('/', "\\")), format!("{user}: {dir}")));
    }
    if let Some(title) = detail.strip_prefix("torrent: ").or(detail.strip_prefix("Usenet: ")) {
        let kind = if detail.starts_with("Usenet") { "usenet" } else { "torrent" };
        return Some((kind.into(), format!("title:{}", title.to_lowercase()), title.to_string()));
    }
    None
}

pub fn blocked(st: &impl Ctx, kind: &str, key: &str) -> bool {
    st.db().one("SELECT 1 FROM source_block WHERE kind=? AND key=?", p![kind, key]).ok().flatten().is_some()
}

/// A Soulseek candidate (user, folder, files) that holds a blocked file or is a blocked folder.
pub fn slskd_blocked(st: &impl Ctx, c: &Value) -> bool {
    let user = c["username"].as_str().unwrap_or("");
    let keys: Vec<String> = st.db().query("SELECT key FROM source_block WHERE kind='slskd' AND key LIKE ?", p![format!("{}|%", like_escape(user))])
        .unwrap_or_default().iter().map(|r| r.s("key")).filter(|k| k.split('|').next() == Some(user)).collect();
    if keys.is_empty() {
        return false;
    }
    let dir = c["directory"].as_str().unwrap_or("");
    if keys.iter().any(|k| *k == format!("{user}|{dir}")) {
        return true;
    }
    c["files"].as_array().into_iter().flatten().any(|f| {
        let k = format!("{user}|{}|{}", f["filename"].as_str().unwrap_or(""), f["size"].as_i64().unwrap_or(0));
        keys.contains(&k)
    })
}

fn like_escape(s: &str) -> String {
    s.replace(['%', '_'], "")
}

/// Drop blocked releases; a release whose title was blocked (older downloads without a hash) too.
pub fn release_blocked(st: &impl Ctx, c: &Value) -> bool {
    if let Some((kind, key, _)) = block_of_release(c) {
        if blocked(st, &kind, &key) {
            return true;
        }
    }
    let kind = if c["protocol"] == "usenet" { "usenet" } else { "torrent" };
    blocked(st, kind, &format!("title:{}", c["title"].as_str().unwrap_or("").to_lowercase()))
}

pub fn url_blocked(st: &impl Ctx, url: &str) -> bool {
    block_of_url(url).is_some_and(|(k, key, _)| blocked(st, &k, &key))
}

/// For a replacement: the Soulseek users the reported file came from go last.
pub fn avoid_users(d: &Value, cands: &mut [Value]) {
    let avoid: Vec<&str> = d["avoid"]["users"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    if avoid.is_empty() {
        return;
    }
    cands.sort_by_key(|c| avoid.contains(&c["username"].as_str().unwrap_or("")));
}

/// For a replacement: the source the reported file came from is tried last.
pub fn avoid_source(d: &Value, order: &mut Vec<String>) {
    let Some(src) = d["avoid"]["source"].as_str() else { return };
    let (mut last, keep): (Vec<String>, Vec<String>) = order.drain(..).partition(|s| s == src || (src == "slskd" && s.starts_with("slskd")));
    order.extend(keep);
    order.append(&mut last);
}

// ---- parking the reported file -------------------------------------------------------------------

fn mv(src: &FsPath, dest: &FsPath) -> std::io::Result<()> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d)?;
    }
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dest)?;
    std::fs::remove_file(src)
}

/// Move a track's file (and its lyrics) to the trash and take its row out of the library, keeping it
/// in track_parked until the replacement arrives.
fn park(st: &impl Ctx, report_id: i64, t: &Row) -> anyhow::Result<()> {
    let path = PathBuf::from(t.s("path"));
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let dest = trash_dir(st).join(format!("{}-{}-{name}", now() as i64, t.i("id")));
    if path.is_file() {
        mv(&path, &dest)?;
        for side in ["lrc", "txt"] {
            let s = path.with_extension(side);
            if s.is_file() {
                let _ = mv(&s, &dest.with_extension(format!("{}.{side}", dest.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default())));
            }
        }
    }
    st.db().execute("INSERT OR REPLACE INTO track_parked (id, report_id, row, trash_path, parked_at) VALUES (?,?,?,?,?)",
        p![t.i("id"), report_id, Value::Object(t.clone()).to_string(), dest.to_string_lossy(), now()])?;
    st.db().execute("DELETE FROM tracks WHERE id=?", p![t.i("id")])?;
    st.track_removed(t.i("id"));
    st.library().forget_cover(None);
    Ok(())
}

/// Put a parked track back: its file from the trash, its row with the same id.
fn restore(st: &impl Ctx, id: i64) -> bool {
    let Ok(Some(pk)) = st.db().one("SELECT * FROM track_parked WHERE id=?", p![id]) else { return false };
    let row: serde_json::Map<String, Value> = serde_json::from_str(&pk.s("row")).unwrap_or_default();
    let orig = PathBuf::from(row.s("path"));
    let trash = PathBuf::from(pk.s("trash_path"));
    let mut dest = orig.clone();
    if trash.is_file() {
        let stem = orig.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let ext = orig.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut i = 2;
        while dest.exists() {
            dest = orig.with_file_name(format!("{stem} ({i}){ext}"));
            i += 1;
        }
        if mv(&trash, &dest).is_err() {
            return false;
        }
        for side in ["lrc", "txt"] {
            let s = PathBuf::from(format!("{}.{side}", trash.to_string_lossy()));
            if s.is_file() {
                let _ = mv(&s, &dest.with_extension(side));
            }
        }
    } else if !orig.is_file() {
        // purged from the trash already: nothing to bring back
        let _ = st.db().execute("DELETE FROM track_parked WHERE id=?", p![id]);
        return false;
    }
    let mut row = row;
    row.insert("path".into(), json!(dest.to_string_lossy()));
    if st.db().one("SELECT 1 FROM tracks WHERE id=?", p![id]).ok().flatten().is_none() {
        let cols: Vec<String> = row.keys().filter(|k| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).cloned().collect();
        let sql = format!("INSERT INTO tracks ({}) VALUES ({})", cols.join(", "), vec!["?"; cols.len()].join(","));
        if st.db().execute(&sql, cols.iter().map(|c| row[c].clone()).collect()).is_err() {
            // the path is in the library under another id by now: just index the file
            let _ = encore_library::index_file(st.db(), &dest.to_string_lossy());
        }
    }
    let _ = st.db().execute("DELETE FROM track_parked WHERE id=?", p![id]);
    st.library().forget_cover(None);
    true
}

/// The new row `new_id` becomes `old_id`: everything that points at the song keeps pointing at it.
fn rekey(st: &impl Ctx, new_id: i64, old_id: i64) -> bool {
    for t in ["track_quality", "track_origin"] {
        let _ = st.db().execute(&format!("DELETE FROM {t} WHERE track_id=?"), p![old_id]);
    }
    if st.db().execute("UPDATE tracks SET id=? WHERE id=?", p![old_id, new_id]).is_err() {
        return false;
    }
    for t in ["playlist_items", "plays", "track_quality", "track_origin", "shares", "contrib_acoustid", "contrib_isrc", "playback_spot"] {
        let _ = st.db().execute(&format!("UPDATE {t} SET track_id=? WHERE track_id=?"), p![old_id, new_id]);
    }
    st.track_removed(new_id);
    true
}

/// The reported song's references move to a copy that was already in the library.
fn adopt(st: &impl Ctx, existing: i64, old_id: i64) {
    for t in ["playlist_items", "plays", "shares", "playback_spot"] {
        let _ = st.db().execute(&format!("UPDATE {t} SET track_id=? WHERE track_id=?"), p![existing, old_id]);
    }
}

fn set_report(st: &impl Ctx, id: i64, status: &str, message: &str) {
    let _ = st.db().execute("UPDATE reports SET status=?, message=?, updated_at=? WHERE id=?", p![status, message, now(), id]);
}

fn parked_ids(st: &impl Ctx, report_id: i64) -> Vec<i64> {
    st.db().query("SELECT id FROM track_parked WHERE report_id=?", p![report_id]).unwrap_or_default().iter().map(|r| r.i("id")).collect()
}

/// The replacement request finished: each parked song is matched to a new file (same recording, same
/// disc/track number, or same title) and the new row takes the parked id. Parked songs the new download
/// didn't have are put back.
pub fn finish(st: &impl Ctx, d: &Value, new: &[PathBuf], since: f64) {
    let Some(rid) = d["replace"]["report"].as_i64() else { return };
    let parked: Vec<Row> = parked_ids(st, rid).iter().filter_map(|id| st.db().one("SELECT * FROM track_parked WHERE id=?", p![id]).ok().flatten())
        .map(|pk| serde_json::from_str::<serde_json::Map<String, Value>>(&pk.s("row")).unwrap_or_default()).collect();
    let mut fresh: Vec<Row> = new.iter().filter_map(|pth| st.db().one("SELECT * FROM tracks WHERE path=?", p![pth.to_string_lossy()]).ok().flatten()).collect();
    let (mut done, mut kept) = (0, 0);
    for old in &parked {
        let oid = old.i("id");
        let pick = if parked.len() == 1 && fresh.len() == 1 {
            Some(0)
        } else {
            fresh.iter().position(|n| !old.s("mb_trackid").is_empty() && n.s("mb_trackid") == old.s("mb_trackid"))
                .or_else(|| fresh.iter().position(|n| old.i("track") > 0 && n.i("track") == old.i("track") && n.i("disc").max(1) == old.i("disc").max(1)
                    && encore_library::tagger::sim(&n.s("title"), &old.s("title")) >= 0.6))
                .or_else(|| fresh.iter().position(|n| words(&n.s("title")) == words(&old.s("title")) || encore_library::tagger::sim(&n.s("title"), &old.s("title")) >= 0.85))
        };
        let Some(i) = pick else {
            restore(st, oid);
            continue;
        };
        let n = fresh.remove(i);
        if n.f("added_at") < since - 1.0 {
            // the import kept a copy that was already in the library: the song's references go there
            adopt(st, n.i("id"), oid);
            let _ = st.db().execute("DELETE FROM track_parked WHERE id=?", p![oid]);
            kept += 1;
        } else if rekey(st, n.i("id"), oid) {
            let _ = st.db().execute("DELETE FROM track_parked WHERE id=?", p![oid]);
            done += 1;
        } else {
            restore(st, oid);
        }
    }
    let total = parked.len();
    if done + kept == 0 {
        set_report(st, rid, "no_replacement", "The new download didn't have this song – the reported copy is back for now");
    } else {
        let msg = if total == 1 { "Replaced with a copy from another source".to_string() } else { format!("Replaced {} of {total} songs with copies from another source", done + kept) };
        set_report(st, rid, "replaced", &msg);
        st.db().log("info", &format!("Report #{rid}: {msg}"));
    }
}

/// The replacement request failed or was cancelled: the reported files come back (the block stays, so
/// a later try still never picks that source).
pub fn replacement_failed(st: &impl Ctx, d: &Value, why: &str) {
    let Some(rid) = d["replace"]["report"].as_i64() else { return };
    for id in parked_ids(st, rid) {
        restore(st, id);
    }
    set_report(st, rid, "no_replacement", &format!("No other copy found ({}) – the reported one is back for now", why.chars().take(160).collect::<String>()));
}

/// Reports whose replacement request ended without the worker noticing (cancelled, removed, denied).
/// A report still being filed ('filing': parking files, creating the request) is left alone unless it
/// got stuck there (the server stopped mid-way).
pub fn reconcile(st: &impl Ctx) {
    let rows = st.db().query("SELECT r.id, r.request_id, q.status rs, q.detail FROM reports r LEFT JOIN requests q ON q.id=r.request_id
                            WHERE r.status='replacing' OR (r.status='filing' AND r.updated_at < ?)", p![now() - FILING_STUCK_SECS]).unwrap_or_default();
    for r in rows {
        let ended = match r.get("rs").and_then(|v| v.as_str()) {
            None => true,
            Some(s) => ["cancelled", "denied"].contains(&s),
        };
        if ended {
            replacement_failed(st, &json!({ "replace": { "report": r.i("id") } }), if r.get("rs").is_some_and(|v| !v.is_null()) { "the download was cancelled" } else { "the download was removed" });
        }
    }
}

/// Files in the trash older than 7 days are deleted (scheduler: Clean up).
pub fn purge_trash(st: &impl Ctx) -> usize {
    let cutoff = now() - TRASH_DAYS * 86400.0;
    // files of reports still waiting for their replacement stay, however long that takes
    let waiting: Vec<String> = st.db().query("SELECT p.trash_path FROM track_parked p JOIN reports r ON r.id=p.report_id WHERE r.status IN ('replacing','filing')", p![])
        .unwrap_or_default().iter().map(|r| r.s("trash_path")).filter(|t| !t.is_empty()).collect();
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(trash_dir(st)) {
        for e in rd.flatten() {
            let full = e.path().to_string_lossy().to_string();
            if waiting.iter().any(|w| full == *w || full.starts_with(&format!("{w}."))) {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let t = name.split('-').next().and_then(|x| x.parse::<f64>().ok())
                .or_else(|| e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()))
                .unwrap_or(now());
            if t < cutoff && std::fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    // parked rows whose file is gone and whose report is settled are dropped
    let _ = st.db().execute("DELETE FROM track_parked WHERE parked_at < ? AND report_id NOT IN (SELECT id FROM reports WHERE status IN ('replacing','filing'))", p![cutoff]);
    n
}

// ---- the report itself ---------------------------------------------------------------------------

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct ReportBody {
    pub kind: String,
    pub track_id: Option<i64>,
    pub album_id: Option<String>,
    pub reason: String,
    pub note: Option<String>,
}

/// May this user report these tracks? Admins anything; others what they played or requested.
fn allowed(st: &impl Ctx, u: &Row, tracks: &[Row]) -> bool {
    if is_admin(u) {
        return true;
    }
    tracks.iter().any(|t| {
        st.db().one("SELECT 1 FROM plays WHERE user_id=? AND track_id=? LIMIT 1", p![u.i("id"), t.i("id")]).ok().flatten().is_some()
            || st.db().one("SELECT 1 FROM requests WHERE id=? AND user_id=?", p![t.i("request_id"), u.i("id")]).ok().flatten().is_some()
    })
}

/// File the report, block the source, park the files and ask for a replacement. → the report row
pub fn file_report(st: &impl Ctx, u: &Row, b: &ReportBody) -> ApiResult<Value> {
    if !REASONS.iter().any(|(k, _)| *k == b.reason) {
        return Err(err(400, "Pick what is wrong"));
    }
    let note = b.note.as_deref().map(|n| n.trim().chars().take(500).collect::<String>()).filter(|n| !n.is_empty());
    if b.reason == "other" && note.is_none() {
        return Err(err(400, "Say what is wrong"));
    }
    let tracks: Vec<Row> = match b.kind.as_str() {
        "track" => st.db().query("SELECT * FROM tracks WHERE id=?", p![b.track_id.unwrap_or(0)])?,
        "album" => st.db().query("SELECT * FROM tracks WHERE album_id=? ORDER BY disc, track, path", p![b.album_id.clone().unwrap_or_default()])?,
        _ => return Err(err(400, "Report a song or an album")),
    };
    if tracks.is_empty() {
        return Err(err(404, if b.kind == "album" { "Album not found" } else { "Song not found" }));
    }
    if !allowed(st, u, &tracks) {
        return Err(err(403, "You can report songs you have played or requested"));
    }
    if !is_admin(u) {
        let today = st.db().one("SELECT COUNT(*) n FROM reports WHERE user_id=? AND created_at > ?", p![u.i("id"), now() - 86400.0])?.map(|r| r.i("n")).unwrap_or(0);
        if today >= 20 {
            return Err(err(429, "That's a lot of reports today – an admin can look at the rest"));
        }
    }
    if tracks.iter().any(|t| st.db().one("SELECT 1 FROM track_parked WHERE id=?", p![t.i("id")]).ok().flatten().is_some()) {
        return Err(err(409, "Already reported – a replacement is on its way"));
    }
    let first = &tracks[0];
    let t = now();
    let origins: Vec<(String, String, String)> = tracks.iter().filter_map(|t| origin_of_track(st, t)).collect();
    let source = first.s("source");
    let rid = st.db().execute("INSERT INTO reports (user_id, kind, track_id, album_id, reason, note, artist, title, album, source, status, created_at, updated_at)
                             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        p![u.i("id"), b.kind, if b.kind == "track" { json!(first.i("id")) } else { Value::Null }, first.s("album_id"), b.reason, note,
           if b.kind == "album" { first.s("albumartist") } else { first.s("artist") }, if b.kind == "track" { first.s("title") } else { first.s("album") }, first.s("album"),
           source, "filing", t, t])?;
    // 1. never that exact source again
    let mut seen = std::collections::HashSet::new();
    for (kind, key, label) in &origins {
        if seen.insert((kind.clone(), key.clone())) {
            let _ = st.db().execute("INSERT OR IGNORE INTO source_block (kind, key, label, report_id, user_id, created_at) VALUES (?,?,?,?,?,?)",
                p![kind, key, label, rid, u.i("id"), t]);
        }
    }
    // 2. the bad files go to the trash (kept 7 days); their rows wait for the replacement
    for tr in &tracks {
        if let Err(e) = park(st, rid, tr) {
            st.db().log("warning", &format!("Report #{rid}: couldn't move {} to the trash: {e}", tr.s("path")));
        }
    }
    // 3. the same song / album again, from somewhere else
    let request = request_replacement(st, u, rid, &b.kind, &tracks, &origins);
    let label = REASONS.iter().find(|(k, _)| *k == b.reason).map(|(_, l)| *l).unwrap_or("");
    let what = if b.kind == "album" { format!("{} – {}", first.s("albumartist"), first.s("album")) } else { format!("{} – {}", first.s("artist"), first.s("title")) };
    st.db().log("info", &format!("{} reported {what}: {label}{}", u.s("username"), note.as_deref().map(|n| format!(" ({n})")).unwrap_or_default()));
    match request {
        Ok(req) => {
            let _ = st.db().execute("UPDATE reports SET status='replacing', request_id=?, message=?, updated_at=? WHERE id=?", p![req, "Looking for a replacement from another source", now(), rid]);
        }
        Err(e) => {
            let msg = e.detail.clone();
            replacement_failed(st, &json!({ "replace": { "report": rid } }), &msg);
        }
    }
    report_json(st, rid)
}

fn request_replacement(st: &impl Ctx, u: &Row, rid: i64, kind: &str, tracks: &[Row], origins: &[(String, String, String)]) -> ApiResult<i64> {
    let first = &tracks[0];
    let src_kind = origins.first().map(|o| o.0.as_str()).unwrap_or("");
    let avoid_source = match src_kind { "slskd" => "slskd", "torrent" | "usenet" => "torznab", "streamrip" => "streamrip", "youtube" => "ytdlp", _ => "" };
    let users: Vec<String> = origins.iter().filter(|o| o.0 == "slskd").filter_map(|o| o.1.split('|').next().map(String::from)).collect();
    let mut extra = json!({ "replace": { "report": rid, "tracks": tracks.iter().map(|t| t.i("id")).collect::<Vec<_>>() }, "avoid": { "users": users } });
    if !avoid_source.is_empty() {
        extra["avoid"]["source"] = json!(avoid_source);
    }
    // what was asked for originally, when the request is still around
    let orig = st.db().one("SELECT * FROM requests WHERE id=?", p![first.i("request_id")]).ok().flatten();
    let od: Value = orig.as_ref().and_then(|o| serde_json::from_str(&o.s("detail")).ok()).unwrap_or(json!({}));
    let nz = |s: String| Some(s).filter(|x| !x.is_empty());
    let b = if kind == "album" {
        let rg = nz(first.s("mb_releasegroupid")).or_else(|| orig.as_ref().filter(|o| o.s("kind") == "album").and_then(|o| nz(o.s("mbid"))));
        NewRequest { kind: "album".into(), mbid: rg, artist: nz(first.s("albumartist")).or(nz(first.s("artist"))), title: nz(first.s("album")), album: nz(first.s("album")),
            year: nz(first.s("year")), ext: od.get("ext").filter(|e| e.is_object()).cloned(), ..Default::default() }
    } else {
        extra["album_mode"] = json!("song");
        let from_req = orig.as_ref().filter(|o| o.s("kind") == "track");
        let length = od["length"].as_f64().or(Some(first.f("duration")).filter(|d| *d > 0.0));
        NewRequest { kind: "track".into(), mbid: from_req.and_then(|o| nz(o.s("mbid"))).or(nz(first.s("mb_trackid"))),
            artist: from_req.and_then(|o| nz(o.s("artist"))).or(nz(first.s("artist"))), title: from_req.and_then(|o| nz(o.s("title"))).or(nz(first.s("title"))),
            album: nz(first.s("album")), length, isrc: nz(first.s("isrc")), album_mode: Some("song".into()), ..Default::default() }
    };
    // a replacement is not a new request: no approval queue, no weekly limit
    let id = create_request(st, u, &b, None, Some(extra))?;
    let _ = st.db().execute("UPDATE requests SET status='queued', message=? WHERE id=? AND status='pending'", p![format!("Replacement for report #{rid}"), id]);
    Ok(id)
}

pub fn report_json(st: &impl Ctx, id: i64) -> ApiResult<Value> {
    let r = st.db().one("SELECT r.*, u.username, q.status request_status, q.message request_message FROM reports r LEFT JOIN users u ON u.id=r.user_id
                       LEFT JOIN requests q ON q.id=r.request_id WHERE r.id=?", p![id])?.ok_or_else(|| err(404, "Report not found"))?;
    let mut v = Value::Object(r);
    v["reason_label"] = json!(REASONS.iter().find(|(k, _)| *k == v["reason"]).map(|(_, l)| *l).unwrap_or(""));
    Ok(v)
}

/// Try again (report row `r`): the songs that came back are parked again and requested once more
/// (blocks stay). → the report
pub fn retry_report(st: &impl Ctx, u: &Row, r: &Row) -> ApiResult<Value> {
    let id = r.i("id");
    if r.s("status") == "replacing" || r.s("status") == "filing" {
        return Err(err(409, "A replacement is already on its way"));
    }
    let tracks: Vec<Row> = if r.s("kind") == "album" {
        st.db().query("SELECT * FROM tracks WHERE album_id=? ORDER BY disc, track, path", p![r.s("album_id")])?
    } else {
        st.db().query("SELECT * FROM tracks WHERE id=?", p![r.i("track_id")])?
    };
    if tracks.is_empty() {
        return Err(err(404, "The song is no longer in the library"));
    }
    // 'filing' until the new request exists, so reconcile doesn't settle it in between
    let _ = st.db().execute("UPDATE reports SET status='filing', request_id=NULL, message=?, updated_at=? WHERE id=?", p!["Looking for a replacement from another source", now(), id]);
    // the copies there now (a replacement the user rejects, or the original) are blocked too
    let t = now();
    for tr in &tracks {
        if let Some((kind, key, label)) = origin_of_track(st, tr) {
            let _ = st.db().execute("INSERT OR IGNORE INTO source_block (kind, key, label, report_id, user_id, created_at) VALUES (?,?,?,?,?,?)",
                p![kind, key, label, id, u.i("id"), t]);
        }
    }
    for tr in &tracks {
        if let Err(e) = park(st, id, tr) {
            st.db().log("warning", &format!("Report #{id}: couldn't move {} to the trash: {e}", tr.s("path")));
        }
    }
    let origins: Vec<(String, String, String)> = st.db().query("SELECT kind, key, label FROM source_block WHERE report_id=? ORDER BY id DESC", p![id])?.iter().map(|b| (b.s("kind"), b.s("key"), b.s("label"))).collect();
    match request_replacement(st, u, id, &r.s("kind"), &tracks, &origins) {
        Ok(req) => { let _ = st.db().execute("UPDATE reports SET status='replacing', request_id=?, updated_at=? WHERE id=?", p![req, now(), id]); }
        Err(e) => replacement_failed(st, &json!({ "replace": { "report": id } }), &e.detail),
    }
    report_json(st, id)
}

/// Dismiss report `id`: a replacement on its way is cancelled and the parked songs come back.
pub async fn dismiss_report(st: &impl Ctx, id: i64) -> ApiResult<()> {
    let r = st.db().one("SELECT * FROM reports WHERE id=?", p![id])?.ok_or_else(|| err(404, "Report not found"))?;
    if r.s("status") == "replacing" || r.s("status") == "filing" {
        if let Some(req) = r.get("request_id").and_then(|v| v.as_i64()) {
            crate::worker::cancel(st, req).await;
            let _ = st.db().execute("UPDATE requests SET status='cancelled', message='Report dismissed', updated_at=? WHERE id=? AND status IN ('pending','queued','searching','downloading','monitoring','importing','waiting')", p![now(), req]);
        }
        for t in parked_ids(st, id) {
            restore(st, t);
        }
    }
    set_report(st, id, "dismissed", "Dismissed");
    Ok(())
}
