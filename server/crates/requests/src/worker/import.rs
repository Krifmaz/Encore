//! Importing and finishing: the downloaded files are verified, tagged and moved into the library (a wrong
//! file is rejected), the request is marked done or failed, and the admin's channels are told.
use super::*;

/// Import downloaded files for this request; a wrong-audio rejection becomes Err(Rejected). `hint`: a cover
/// to look at besides the request's own. The audio check is off when a person said to take the download as
/// it is (Keep anyway, Download anyway, a hand pick with "Don't check the audio": [`rejected::skips_check`]);
/// its songs are then confirmed by that person. The quality and release checks stay on: a fake-lossless
/// file or another artist's album is turned away all the same. A title that asks for a nightcore, sped up, slowed …
/// version is not fingerprinted (the request's log says so): only its length and title are checked.
pub(super) async fn import(st: &impl Ctx, r: &mut Req, files: &[PathBuf], root: &Path, hint: Option<&str>) -> anyhow::Result<(Vec<PathBuf>, String)> {
    if r.surround() {
        return import_surround(st, r, files).await;
    }
    // a 5.1 or Atmos file that came with a stereo download goes to the Surround library; the request goes on
    // with the stereo files, or looks further when there are none
    let (surround, stereo) = encore_library::surround::split(files).await;
    if !surround.is_empty() {
        let (filed, _) = encore_library::surround::import(st, &surround, &surround_want(r)).await?;
        let src = r.opt("source").unwrap_or("import".into());
        log_push(r, format!("{src}: {} surround file{} filed in the Surround library", filed.len(), if filed.len() == 1 { "" } else { "s" }));
        if stereo.is_empty() {
            return Err(tagger::Rejected::quality("Only a surround version (filed in the Surround library), no stereo copy".into()).into());
        }
    }
    let files = if surround.is_empty() { files.to_vec() } else { stereo };
    let files = files.as_slice();
    let hint = r.hint().or(hint.map(String::from));
    let check = !rejected::skips_check(&r.d);
    let album = r.s("kind") == "album";
    if check {
        let title = if album { r.album_title() } else { r.s("title") };
        if let (Some(words), Some(f)) = (encore_library::matcher::altered_words(&title), files.first()) {
            if tagger::will_verify(st, f) {
                let src = r.opt("source").unwrap_or("import".into());
                log_push(r, format!("{src}: {}", encore_library::acoustid::skipped_detail(&words)));
            }
        }
    }
    let (new, note) = if album {
        // the album's track list (kept on the request; looked up now when no source did): the songs are filed
        // under the album by it when MusicBrainz can't match the files themselves
        release_info(st, r).await;
        let mbid = r.opt("mbid");
        let new = tagger::import_album_files(st, files, tagger::AlbumImport {
            artist: &r.s("artist"), album: &r.album_title(), rgid: mbid.as_deref(), source_root: root, reject: true, audio: check,
            source_meta: source_meta(r).as_ref(), hint: hint.as_deref(),
        }).await?;
        (new, String::new())
    } else {
        let f = files.first().ok_or_else(|| anyhow::anyhow!("no audio file"))?;
        // the recording the album lookup found (the right version of the song): the import asks MusicBrainz
        // for it alone (one lookup, and one the cache may already hold) instead of searching first
        let mbid = r.opt("mbid").or_else(|| r.d["recording"].as_str().filter(|x| !x.is_empty()).map(String::from));
        let (p, note) = tagger::import_single(st, f, &r.s("artist"), &r.s("title"), mbid.as_deref(), root, r.length(), true, check, hint.as_deref()).await?;
        (vec![p], note)
    };
    let Some(skipped) = rejected::skip_note(&r.d) else { return Ok((new, note)) };
    // the person's word confirms the songs (not a copy that was already there and stayed)
    if !note.starts_with("already in library") {
        rejected::confirm(st, &r.d, &new);
    }
    Ok((new, if note.is_empty() { skipped } else { format!("{note}; {skipped}") }))
}

/// What a Surround import knows of the request: its names and the stereo album and songs it was asked for.
fn surround_want(r: &Req) -> encore_library::surround::Import {
    encore_library::surround::Import {
        artist: r.s("artist"),
        album: r.album_title(),
        stereo_album_id: r.d["stereo_album_id"].as_str().map(String::from),
        stereo_track_ids: r.d["stereo_track_ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_i64()).collect()).unwrap_or_default(),
    }
}

/// A request for the Surround library: the surround files go to the Surround folder; a download without any
/// (no Atmos stream after all) is turned away.
async fn import_surround(st: &impl Ctx, r: &mut Req, files: &[PathBuf]) -> anyhow::Result<(Vec<PathBuf>, String)> {
    let (new, others) = encore_library::surround::import(st, files, &surround_want(r)).await?;
    if new.is_empty() {
        return Err(tagger::Rejected::quality("The download has no surround audio (no Dolby Atmos stream)".into()).into());
    }
    let note = if others > 0 { format!("{others} without a surround stream left out") } else { String::new() };
    Ok((new, note))
}

/// Who brought a request's songs in (`tracks.added_by`, `added_via`, `requested_at`): its user, how it was asked
/// (a link → `link`, a download from the Soulseek page → `soulseek`, else `request`) and when. None for a request
/// without a user (a lossless upgrade): the new copy keeps who it inherited from the copy it replaced.
pub fn adder_of(row: &Row, d: &Value) -> Option<(i64, &'static str, f64)> {
    let user = row.get("user_id").and_then(|v| v.as_i64()).filter(|u| *u > 0)?;
    let via = if d["via"] == "soulseek" { "soulseek" } else if row.s("kind") == "url" { "link" } else { "request" };
    Some((user, via, row.f("created_at")))
}

/// Remember where each new file came from (shown on the album page) and who asked for it (kept on the song, so
/// Clear finished – which deletes the request – doesn't lose the name).
pub fn record_origin(st: &impl Ctx, paths: &[PathBuf], origin: &Value, rid: i64, since: f64, adder: Option<(i64, &str, f64)>) {
    let (by, via, at) = match adder {
        Some((u, v, t)) => (json!(u), json!(v), json!(t)),
        None => (Value::Null, Value::Null, Value::Null),
    };
    // without a user the three stay as they are (COALESCE keeps them: `via` NULL means "leave it")
    let who = "added_by=CASE WHEN ? IS NULL THEN added_by ELSE ? END, added_via=COALESCE(?, added_via), requested_at=CASE WHEN ? IS NULL THEN requested_at ELSE ? END";
    for pth in paths {
        let _ = st.db().execute(&format!("UPDATE surround_tracks SET source=?, source_detail=?, source_url=?, request_id=?, downloaded_at=?, {who} WHERE path=?"),
            p![origin["source"], origin["detail"], origin["url"], rid, now(), via, by, via, via, at, pth.to_string_lossy()]);
        let _ = st.db().execute(&format!("UPDATE tracks SET source=?, source_detail=?, source_url=?, request_id=?, downloaded_at=?, {who}
                               WHERE path=? AND (downloaded_at IS NULL OR downloaded_at >= ?) AND added_at >= ?"),
            p![origin["source"], origin["detail"], origin["url"], rid, now(), via, by, via, via, at, pth.to_string_lossy(), since, since - 1.0]);
    }
    crate::report::record_origin(st, paths, origin); // the exact source, for Report a problem
}

/// Finish a request as completed: record where its files came from, notify, and start what follows (playlists, Atmos, lyrics).
pub(super) async fn complete(st: &impl Ctx, r: &mut Req, message: &str, new: &[PathBuf], origin: Value) {
    if !new.is_empty() {
        record_origin(st, new, &origin, r.id, r.row.f("created_at"), adder_of(&r.row, &r.d));
    }
    if r.d["replace"].is_object() {
        // a reported song's replacement: it takes over the reported track's id
        crate::report::finish(st, &r.d, new, r.row.f("created_at"));
    }
    upd(st, r, &[("status", json!("completed")), ("progress", json!(100)), ("message", json!(message))]);
    if r.surround() {
        // the Surround library: no playlists or lyrics task (they read the stereo library), the media servers
        // see the new folder
        st.db().log("info", &format!("Surround request completed: {} ({message})", r.label()));
        notify::send(st, "✅ Dolby Atmos version available", &format!("{}\n{message}", r.label()), r.hint().as_deref()).await;
        if !new.is_empty() {
            notify::refresh_media_servers(st).await;
        }
        return;
    }
    if !new.is_empty() {
        encore_sync::playlists::rematch(st, None, true); // playlists pick up the new songs
        crate::surround::after_stereo(st, r, new); // the Atmos version too, when the setting says so
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

/// Finish a request as failed: log it and notify (a lossless-only or automatic request waits to try again instead).
pub(super) async fn fail(st: &impl Ctx, r: &mut Req, message: &str) {
    if r.d["replace"].is_object() {
        crate::report::replacement_failed(st, &r.d, message);
    }
    if r.d["lossless_only"].as_bool().unwrap_or(false) {
        upd(st, r, &[("status", json!("failed")), ("message", json!("No lossless copy available yet – will try again later"))]);
        return;
    }
    if r.d["auto"].as_bool() == Some(true) {
        // the playlist's auto-download asks for it again – not at once, and later after each failed attempt
        let attempts = r.d["attempts"].as_u64().unwrap_or(1);
        let at = now() + crate::missing::auto_retry_after(attempts);
        log_push(r, format!("auto-download: asks for it again after {} (the wait doubles after each failed attempt, at most a day)", crate::slskd_session::clock(at)));
    }
    upd(st, r, &[("status", json!("failed")), ("message", json!(message))]);
    if r.surround() {
        crate::surround::failed(st, &r.d);
    }
    tracing::warn!("request {} failed: {} – {message}", r.id, r.label());
    st.db().log("error", &format!("Request failed: {} – {message}", r.label()));
    notify::send(st, "❌ Request failed", &format!("{}\n{message}", r.label()), r.hint().as_deref()).await;
}
