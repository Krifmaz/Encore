//! Importing and finishing: the downloaded files are verified, tagged and moved into the library (a wrong
//! file is rejected), the request is marked done or failed, and the admin's channels are told.
use super::*;

/// Import downloaded files for this request; a wrong-audio rejection becomes Err(Rejected).
pub(super) async fn import(st: &impl Ctx, r: &Req, files: &[PathBuf], root: &Path) -> anyhow::Result<(Vec<PathBuf>, String)> {
    let hint = r.hint();
    if r.s("kind") == "album" {
        let mbid = r.opt("mbid");
        let new = tagger::import_album_files(st, files, tagger::AlbumImport {
            artist: &r.s("artist"), album: &r.album_title(), rgid: mbid.as_deref(), source_root: root, reject: true,
            source_meta: source_meta(r).as_ref(), hint: hint.as_deref(),
        }).await?;
        Ok((new, String::new()))
    } else {
        let f = files.first().ok_or_else(|| anyhow::anyhow!("no audio file"))?;
        let mbid = r.opt("mbid");
        let (p, note) = tagger::import_single(st, f, &r.s("artist"), &r.s("title"), mbid.as_deref(), root, r.length(), true, hint.as_deref()).await?;
        Ok((vec![p], note))
    }
}

/// Remember where each new file came from (shown on the album page).
pub(super) fn record_origin(st: &impl Ctx, paths: &[PathBuf], origin: &Value, rid: i64, since: f64) {
    for pth in paths {
        let _ = st.db().execute("UPDATE tracks SET source=?, source_detail=?, source_url=?, request_id=?, downloaded_at=?
                               WHERE path=? AND (downloaded_at IS NULL OR downloaded_at >= ?) AND added_at >= ?",
            p![origin["source"], origin["detail"], origin["url"], rid, now(), pth.to_string_lossy(), since, since - 1.0]);
    }
    crate::report::record_origin(st, paths, origin); // the exact source, for Report a problem
}

pub(super) async fn complete(st: &impl Ctx, r: &mut Req, message: &str, new: &[PathBuf], origin: Value) {
    if !new.is_empty() {
        record_origin(st, new, &origin, r.id, r.row.f("created_at"));
    }
    if r.d["replace"].is_object() {
        // a reported song's replacement: it takes over the reported track's id
        crate::report::finish(st, &r.d, new, r.row.f("created_at"));
    }
    upd(st, r, &[("status", json!("completed")), ("progress", json!(100)), ("message", json!(message))]);
    if !new.is_empty() {
        encore_sync::playlists::rematch(st, None, true); // playlists pick up the new songs
    }
    st.db().log("info", &format!("Request completed: {} ({message})", r.label()));
    notify::send(st, "✅ Now available", &format!("{}\n{message}", r.label()), r.hint().as_deref()).await;
    if !new.is_empty() {
        notify::refresh_media_servers(st).await;
        if st.cfg().bool("lyrics_on_import") {
            let ids: Vec<i64> = new.iter().filter_map(|p| st.db().one("SELECT id FROM tracks WHERE path=?", p![p.to_string_lossy()]).ok().flatten()).map(|r| r.i("id")).collect();
            let st = st.clone();
            tokio::spawn(async move {
                encore_library::lyrics::fetch_missing(&st, Some(&ids), ids.len()).await;
            });
        }
    }
}

pub(super) async fn fail(st: &impl Ctx, r: &mut Req, message: &str) {
    if r.d["replace"].is_object() {
        crate::report::replacement_failed(st, &r.d, message);
    }
    if r.d["lossless_only"].as_bool().unwrap_or(false) {
        upd(st, r, &[("status", json!("failed")), ("message", json!("No lossless copy available yet – will try again later"))]);
        return;
    }
    upd(st, r, &[("status", json!("failed")), ("message", json!(message))]);
    st.db().log("error", &format!("Request failed: {} – {message}", r.label()));
    notify::send(st, "❌ Request failed", &format!("{}\n{message}", r.label()), r.hint().as_deref()).await;
}
