//! Picking where a request comes from: already in the library, the whole album instead of one song, an
//! artist's discography as one request per release, the sources in the admin's order, or exactly what
//! someone picked by hand.
use super::*;

pub(super) async fn start(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let kind = r.s("kind");
    if kind == "playlist" {
        upd(st, r, &[("status", json!("completed"))]);
        return Ok(());
    }
    if kind == "url" {
        let url = r.s("url");
        if RIP_LINK.is_match(&url) {
            if !streamrip::enabled(st) {
                fail(st, r, "Qobuz/Tidal/Deezer links need streamrip switched on in Settings → Downloads → Qobuz / Tidal / Deezer (streamrip)").await;
                return Ok(());
            }
            upd(st, r, &[("source", json!("streamrip")), ("status", json!("downloading")), ("message", json!("Downloading with streamrip…"))]);
            return run_streamrip_url(st, r).await;
        }
        upd(st, r, &[("source", json!("ytdlp")), ("status", json!("downloading")), ("message", json!("Starting download…"))]);
        return run_url(st, r).await;
    }
    if r.d["kept"].is_object() && !r.d["pick"].is_object() && storage::has_files(&r.d) {
        // asked for again while its downloaded files are kept: they are imported, not downloaded again
        return storage::import_kept(st, r).await;
    }
    if let Some(d) = r.d.as_object_mut() {
        d.remove("kept"); // kept files that are gone: a new download
    }
    if r.d["pick"].is_object() {
        return start_pick(st, r).await;
    }
    let lossless_only = r.d["lossless_only"].as_bool().unwrap_or(false);
    // already there? (lossless upgrade requests replace what's there)
    let replacing = r.d["replace"].is_object(); // a reported copy is being replaced: what's there doesn't count
    if kind == "album" && !lossless_only && !replacing && (encore_library::has_release_group(st.db(), &r.s("mbid")) || encore_library::has_album(st.db(), &r.s("artist"), &r.album_title())) {
        complete(st, r, "Already in library", &[], Value::Null).await;
        return Ok(());
    }
    if kind == "track" && !replacing && track_in_library(st, r).await {
        complete(st, r, "Already in library", &[], Value::Null).await;
        return Ok(());
    }
    if kind == "track" && try_album_upgrade(st, r).await? {
        return Ok(());
    }
    if kind == "artist" {
        // always one request per release, so each one goes through every source
        return expand_artist(st, r).await;
    }
    if kind == "album" && r.s("mbid").is_empty() && r.d["ext"].is_object() {
        release_info(st, r).await; // store track list: used for matching downloads and tagging
    }
    let mut order = source_order(st, r);
    crate::report::avoid_source(&r.d, &mut order); // a replacement tries the reported file's source last
    r.d["sources"] = json!(order);
    r.d["log"] = json!([]);
    next_source(st, r, "").await
}

/// Try the remaining sources in order until one downloads or waits.
pub(super) async fn next_source(st: &impl Ctx, r: &mut Req, reason: &str) -> anyhow::Result<()> {
    let mut reason = reason.to_string();
    loop {
        if !reason.is_empty() {
            let src = r.opt("source").unwrap_or("start".into());
            storage::log_failure(st, r, &src, &reason);
            log_push(r, format!("{src}: {reason}"));
        }
        let mut srcs: Vec<String> = r.d["sources"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        if srcs.is_empty() {
            let log: Vec<String> = r.d["log"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let tried = if log.is_empty() { "no download sources enabled".to_string() } else { log[log.len().saturating_sub(3)..].join("; ") };
            fail(st, r, &format!("All sources exhausted ({tried})")).await;
            return Ok(());
        }
        let src = srcs.remove(0);
        r.d["sources"] = json!(srcs);
        let soulseek = src == "slskd" || src == "slskd_lossless";
        if soulseek {
            match soulseek_turn(st, r).await {
                Turn::Go => {}
                Turn::Wait(when) => return park_for_soulseek(st, r, &src, &when),
                Turn::Skip(why) => {
                    log_push(r, format!("{src}: {why}"));
                    reason.clear();
                    continue;
                }
            }
        }
        upd(st, r, &[("source", json!(src)), ("progress", json!(0)), ("status", json!("searching")), ("message", json!(format!("Trying {src}…")))]);
        let res = match src.as_str() {
            "slskd" => start_slskd(st, r, false).await,
            "slskd_lossless" => start_slskd(st, r, true).await,
            "torznab" => start_torznab(st, r, false).await,
            "ytdlp" => start_ytdlp(st, r, None).await,
            "streamrip" => start_streamrip(st, r, None).await,
            other => Ok(Out::Next(format!("unknown source {other}"))),
        };
        match res {
            Ok(Out::Done) | Ok(Out::Wait) => return Ok(()),
            Ok(Out::Next(why)) => reason = why,
            Err(e) if e.is::<Cancelled>() => return Err(e),
            // the staging folder (or a download's own folder) failed: the disk's problem, not the source's –
            // the step waits for the storage instead of going on to the next source
            Err(e) if storage_error(st, &e).is_some() => {
                let se = storage_error(st, &e).unwrap();
                let reason = if r.d["kept"].is_object() { storage::STORAGE_IMPORT } else { storage::STORAGE_DOWNLOAD };
                return storage::wait_for_storage(st, r, se, reason);
            }
            // the session went down between the check and the search: wait for it like above
            Err(e) if soulseek && e.is::<slskd::NotConnected>() => match soulseek_turn(st, r).await {
                Turn::Wait(when) => return park_for_soulseek(st, r, &src, &when),
                Turn::Skip(why) => reason = why,
                Turn::Go => reason = format!("error: {e}"),
            },
            Err(e) => reason = format!("error: {}", encore_core::fsx::explain(&e)),
        }
    }
}

/// The storage error in `e`'s chain (with the server's folders registered first).
pub(super) fn storage_error<'a>(st: &impl Ctx, e: &'a anyhow::Error) -> Option<&'a encore_core::fsx::StepError> {
    storage::register_roots(st);
    encore_core::fsx::storage(e)
}

/// Whether a Soulseek step can search now.
pub(super) enum Turn {
    Go,
    /// wait for the session ("next attempt at 14:20")
    Wait(String),
    /// go on to the next source, for this reason
    Skip(String),
}

/// The session's answer for the worker, with the hour-long limit: a request that has waited 60 minutes
/// for Soulseek goes on down its sources.
pub(super) async fn soulseek_turn(st: &impl Ctx, r: &Req) -> Turn {
    use crate::slskd_session::{ForWorker, WORKER_WAIT_MAX};
    match st.requests().slskd.session().for_worker(st).await {
        ForWorker::Go => Turn::Go,
        ForWorker::Skip(why) => Turn::Skip(why),
        ForWorker::Wait(_) if r.d["soulseek_wait"]["since"].as_f64().is_some_and(|t| now() - t >= WORKER_WAIT_MAX) => {
            Turn::Skip("Soulseek was not connected for 60 minutes – trying the next source".into())
        }
        ForWorker::Wait(when) => Turn::Wait(when),
    }
}

/// Park the request until Soulseek is back: `waiting` (reason "soulseek", `next_try_at` the session's next
/// attempt when it has one), "Waiting for Soulseek – next attempt at 14:20", the Soulseek step first in its
/// sources again, and its attempts untouched. The queue looks at it again every few seconds and goes on
/// where it stopped ([`super::queue`]): a Soulseek step searches again, a picked file is asked for again.
/// Writes only what changed.
pub(super) fn park_for_soulseek(st: &impl Ctx, r: &mut Req, src: &str, when: &str) -> anyhow::Result<()> {
    check(st, r.id)?;
    let mut srcs: Vec<Value> = r.d["sources"].as_array().cloned().unwrap_or_default();
    srcs.insert(0, json!(src));
    r.d["sources"] = json!(srcs);
    let mut changed = false;
    if !r.d["soulseek_wait"].is_object() {
        r.d["soulseek_wait"] = json!({ "since": now() });
        log_push(r, format!("{src}: waiting for Soulseek ({when})"));
        changed = true;
    }
    let next = st.requests().slskd.session().snapshot().next_at.map(crate::slskd_session::unix_at).map(|t| (t * 10.0).round() / 10.0);
    let wait = json!({ "reason": storage::SOULSEEK, "since": r.d["soulseek_wait"]["since"], "next_try_at": next });
    if r.d["wait"] != wait {
        r.d["wait"] = wait;
        changed = true;
    }
    let msg = format!("Waiting for Soulseek – {when}");
    if changed || r.s("status") != "waiting" || r.s("message") != msg || r.s("source") != src {
        upd(st, r, &[("status", json!("waiting")), ("source", json!(src)), ("progress", json!(0)), ("message", json!(msg))]);
    }
    Ok(())
}

/// A Soulseek download (a picked file, or a request's Soulseek step) that can't go on while the session is
/// down waits for Soulseek ([`park_for_soulseek`]).
pub(super) fn park_download(st: &impl Ctx, r: &mut Req, when: &str) -> anyhow::Result<()> {
    let src = Some(r.s("source")).filter(|s| s == "slskd_lossless").unwrap_or_else(|| "slskd".into());
    park_for_soulseek(st, r, &src, when)
}

pub(super) fn source_order(st: &impl Ctx, r: &Req) -> Vec<String> {
    let lossless_only = r.d["lossless_only"].as_bool().unwrap_or(false);
    let order = arrange(&st.cfg().str("source_order"), st.cfg().bool("quality_first"), lossless_only, st.cfg().bool("ytdlp_last_resort"));
    let album = r.s("kind") == "album";
    let avail = |s: &str| match s {
        "torznab" => torznab::enabled(st) && album,
        "slskd" | "slskd_lossless" => slskd::enabled(st),
        "ytdlp" => ytdlp::enabled(st) || st.cfg().bool("ytdlp_last_resort"),
        "streamrip" => streamrip::enabled(st),
        _ => false,
    };
    order.into_iter().filter(|s| avail(s)).collect()
}

/// The sources in the order they are tried, before the ones that aren't set up are left out: the
/// admin's `source_order` (qobuz / tidal / deezer read as streamrip). With highest quality first (and for
/// a request for a lossless copy) the lossless options come first: streamrip, FLAC on Soulseek, then the
/// torrent and Usenet indexers – whose best releases are lossless, and which only get a turn when
/// Soulseek has no copy in any format otherwise – and only then the lossy fallbacks (Soulseek in any
/// format, YouTube Music). A lossless-only request keeps the indexers too: their search keeps only
/// lossless releases then.
fn arrange(source_order: &str, quality_first: bool, lossless_only: bool, ytdlp_last_resort: bool) -> Vec<String> {
    let mut order: Vec<String> = source_order.split(',').map(|s| s.trim().to_lowercase())
        // qobuz / tidal / deezer are streamrip services: the first one marks where streamrip runs
        .map(|s| if streamrip::SERVICES.contains(&s.as_str()) { "streamrip".to_string() } else { s })
        .filter(|s| !s.is_empty()).collect();
    if !order.iter().any(|s| s == "torznab") {
        // saved orders that predate the torrent and Usenet source get it before yt-dlp
        let at = order.iter().position(|s| s == "ytdlp").unwrap_or(order.len());
        order.insert(at, "torznab".into());
    }
    if quality_first || lossless_only {
        // every lossless option first, then lossy fallbacks (each part in the admin's order)
        let first = |s: &String| s == "streamrip" || s == "torznab";
        let mut lossless: Vec<String> = order.iter().filter(|s| *s == "streamrip").cloned().collect();
        if order.iter().any(|s| s == "slskd") {
            lossless.push("slskd_lossless".into());
        }
        if order.iter().any(|s| s == "torznab") {
            lossless.push("torznab".into());
        }
        lossless.extend(order.iter().filter(|s| !first(s)).cloned());
        order = lossless;
    }
    if lossless_only {
        order.retain(|s| ["streamrip", "slskd_lossless", "torznab"].contains(&s.as_str()));
    } else if ytdlp_last_resort {
        // downloads should basically never fail: YouTube Music is always the final fallback
        order.retain(|s| s != "ytdlp");
        order.push("ytdlp".into());
    }
    // each once (qobuz, tidal and deezer are all streamrip)
    let mut seen = std::collections::HashSet::new();
    order.retain(|s| seen.insert(s.clone()));
    order
}

/// Download exactly the chosen result – no fallback to other sources. A picked Soulseek file waits for
/// the session like any Soulseek step (the hour-long limit included), then is asked for again.
pub(super) async fn start_pick(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let pick = r.d["pick"].clone();
    r.d["sources"] = json!([]);
    if !r.d["soulseek_wait"].is_object() {
        r.d["log"] = json!([]); // a pick that waited for Soulseek keeps the line that says so
    }
    let src = pick["source"].as_str().unwrap_or("");
    let url = pick["payload"]["url"].as_str().unwrap_or("").to_string();
    let out = match src {
        "slskd" => {
            upd(st, r, &[("source", json!("slskd"))]);
            match soulseek_turn(st, r).await {
                Turn::Wait(when) => return park_for_soulseek(st, r, "slskd", &when),
                Turn::Skip(why) => Out::Next(why),
                Turn::Go => {
                    r.d["slskd"] = json!({ "candidates": [pick["payload"]], "idx": -1 });
                    match slskd_try_next(st, r, "").await {
                        Err(e) if e.is::<slskd::NotConnected>() => return slskd_down(st, r).await,
                        o => o?,
                    }
                }
            }
        }
        "torznab" => {
            torznab_drop_old(st, r).await;
            r.d["torznab"] = json!({ "candidates": [pick["payload"]], "idx": -1 });
            upd(st, r, &[("source", json!("torznab"))]);
            torznab_try_next(st, r, "").await?
        }
        "deezer" | "tidal" | "qobuz" | "link" if src != "link" || RIP_LINK.is_match(&url) => {
            if !streamrip::enabled(st) {
                fail(st, r, "Qobuz/Tidal/Deezer links need streamrip switched on in Settings → Downloads → Qobuz / Tidal / Deezer (streamrip)").await;
                return Ok(());
            }
            upd(st, r, &[("source", json!("streamrip"))]);
            start_streamrip(st, r, Some(vec![url])).await?
        }
        "ytmusic" | "link" => {
            upd(st, r, &[("source", json!("ytdlp"))]);
            start_ytdlp(st, r, Some(url)).await?
        }
        other => Out::Next(format!("unknown source {other}")),
    };
    if let Out::Next(why) = out {
        // a picked Soulseek file whose user is offline: the request says what to do instead of "All
        // sources exhausted"
        if src == "slskd" && why.ends_with(SLSKD_OFFLINE) {
            storage::log_failure(st, r, "slskd", &why);
            log_push(r, format!("slskd: {why}"));
            fail(st, r, &format!("{why} – open Pick and choose another result, or Search again")).await;
            return Ok(());
        }
        next_source(st, r, &why).await?;
    }
    Ok(())
}

pub(super) async fn expand_artist(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    upd(st, r, &[("status", json!("searching")), ("message", json!("Looking up discography…"))]);
    let art = mb::artist(st, &r.s("mbid")).await?;
    // the request's own boxes (Request discography panel), else Settings → Release types; the releases ticked
    // in the dialog (`release_ids`) take the place of the filter altogether
    let filter = encore_config::release_types::ReleaseFilter::from_value(&r.d["release_filter"], &encore_config::release_types::ReleaseFilter::from_cfg(st.cfg()));
    let only: Option<Vec<String>> = r.d["release_ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());

    // the merged catalog: MusicBrainz plus the Deezer / Apple Music releases MusicBrainz doesn't list
    let ext = if st.cfg().bool("discover_catalog_merge") { encore_library::catalog::merged(st, &art).await } else { vec![] };
    let name = art["name"].as_str().unwrap_or("").to_string();
    let skip = |x: &Value| {
        let title = x["title"].as_str().unwrap_or("");
        match x["id"].as_str().filter(|id| !x["ext"].is_object() && !id.is_empty()) {
            Some(id) => encore_library::has_release_group(st.db(), id)
                || st.db().one("SELECT 1 FROM requests WHERE mbid=? AND status NOT IN ('failed','denied','cancelled')", p![id]).ok().flatten().is_some(),
            None => encore_library::has_album(st.db(), &name, title)
                || st.db().one("SELECT 1 FROM requests WHERE kind='album' AND mbid IS NULL AND lower(artist)=lower(?) AND lower(title)=lower(?)
                              AND status NOT IN ('failed','denied','cancelled')", p![name, title]).ok().flatten().is_some(),
        }
    };
    let plan = encore_library::catalog::discography_plan(art["release_groups"].as_array().map(|a| a.as_slice()).unwrap_or(&[]), &ext, &filter, only.as_deref(), skip);

    let t = now();
    let (mut n, mut n_ext) = (0, 0);
    for g in plan {
        let is_ext = g["ext"].is_object();
        let detail = if is_ext { json!({ "ext": g["ext"] }) } else { json!({}) };
        let mbid = if is_ext { Value::Null } else { g["id"].clone() };
        let year = g["year"].as_str().filter(|y| !y.is_empty()).map(|y| json!(y)).unwrap_or(Value::Null);
        let id = st.db().execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, detail, parent_id, created_at, updated_at)
                       VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![r.row.get("user_id"), "album", mbid, name, g["title"], g["title"], year, g["cover"], "queued", detail.to_string(), r.id, t, t])?;
        crate::events::emit(st, id);
        n += 1;
        if is_ext {
            n_ext += 1;
        }
    }
    let msg = match (n, n_ext) {
        (0, _) if only.is_some() => "Nothing new – you already have (or requested) every ticked release".to_string(),
        (0, _) => "Nothing new – you already have (or requested) every release".to_string(),

        (n, 0) => format!("Queued {n} release(s) from the discography"),
        (n, e) => format!("Queued {n} release(s) from the discography ({e} not in MusicBrainz)"),
    };
    complete(st, r, &msg, &[], Value::Null).await;
    Ok(())
}

/// Is the requested song (this version of it) in the library? The library's index of every song is kept
/// while the songs are unchanged and built on the blocking pool when they changed (encore_library's `Songs`).
pub(super) async fn track_in_library(st: &impl Ctx, r: &Req) -> bool {
    let mbid = r.opt("mbid").or(r.d["recording"].as_str().map(String::from));
    let want = encore_library::matcher::describe(&r.s("title"), &r.s("artist"), r.length(), r.d["isrc"].as_str(), mbid.as_deref());
    st.library().songs.load(st.db()).await.find(&want).is_some()
}

/// If the song is on an official album, fetch the whole album instead (best quality available).
pub(super) async fn try_album_upgrade(st: &impl Ctx, r: &mut Req) -> anyhow::Result<bool> {
    let mode = match r.d["album_mode"].as_str() { Some("song") => "off".to_string(), Some("album") => "album_ep".to_string(), _ => st.cfg().str("album_upgrade") };
    if mode == "off" || r.d["no_album"].as_bool().unwrap_or(false) {
        return Ok(false);
    }
    upd(st, r, &[("status", json!("searching")), ("message", json!("Looking up which album this song is on…"))]);
    let (mut rid, mut alb) = (None, None);
    let found: anyhow::Result<()> = async {
        rid = match r.opt("mbid") { Some(m) => Some(m), None => encore_library::matcher::resolve_recording(st, &r.s("artist"), &r.s("title"), r.length(), r.d["isrc"].as_str()).await? };
        if let Some(id) = &rid {
            alb = mb::album_for_recording(st, id, mode == "album_ep").await?;
        }
        Ok(())
    }.await;
    if let Err(e) = found {
        st.db().log("warning", &format!("Album lookup failed for {}: {e}", r.label()));
    }
    r.d["recording"] = json!(rid);
    let Some(alb) = alb.filter(|a| !encore_library::has_release_group(st.db(), a["id"].as_str().unwrap_or(""))) else {
        r.d["no_album"] = json!(true);
        upd(st, r, &[]);
        return Ok(false);
    };
    let typ = alb["type"].as_str().unwrap_or("album").to_lowercase();
    let aid = match st.db().one("SELECT id FROM requests WHERE kind='album' AND mbid=? AND status NOT IN ('failed','denied','cancelled')", p![alb["id"]])? {
        Some(e) => e.i("id"),
        None => {
            let t = now();
            let artist = if alb["artist"].as_str().unwrap_or("").is_empty() { json!(r.s("artist")) } else { alb["artist"].clone() };
            let id = st.db().execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, message, detail, created_at, updated_at)
                                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                p![r.row.get("user_id"), "album", alb["id"], artist, alb["title"], alb["title"], alb["year"], alb["cover"], "queued",
                   format!("Full album for “{}”", r.s("title")), "{}", t, t])?;
            st.db().log("info", &format!("“{}” is on {typ} “{}” – downloading the whole {typ}", r.s("title"), alb["title"].as_str().unwrap_or("")));
            id
        }
    };
    r.d["album_request"] = json!(aid);
    let cover = r.opt("cover_url").map(|c| json!(c)).unwrap_or(alb["cover"].clone());
    upd(st, r, &[("status", json!("monitoring")), ("source", json!("album")), ("cover_url", cover),
                 ("message", json!(format!("Getting the whole {typ} “{}”", alb["title"].as_str().unwrap_or(""))))]);
    Ok(true)
}

pub(super) async fn poll_album(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let a = get(st, r.d["album_request"].as_i64().unwrap_or(0));
    let reason = match &a {
        Some(a) if a.s("status") == "completed" => {
            if track_in_library(st, r).await {
                complete(st, r, &format!("Downloaded with the album “{}”", a.s("title")), &[], Value::Null).await;
                return Ok(());
            }
            "the album didn't contain this exact version"
        }
        None => "the album couldn't be downloaded",
        Some(a) if ["failed", "denied", "cancelled"].contains(&a.s("status").as_str()) => "the album couldn't be downloaded",
        Some(a) => {
            let msg = a.opt("message").unwrap_or(a.s("status"));
            if a.row.f("progress") != r.row.f("progress") || r.d["last_msg"].as_str() != Some(&msg) {
                r.d["last_msg"] = json!(msg);
                upd(st, r, &[("progress", json!(a.row.f("progress"))), ("message", json!(format!("Album “{}”: {msg}", a.s("title"))))]);
            }
            return Ok(());
        }
    };
    r.d["no_album"] = json!(true);
    upd(st, r, &[("status", json!("queued")), ("source", Value::Null), ("message", json!(format!("Getting the song on its own ({reason})")))]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::arrange;

    fn order(setting: &str, quality_first: bool, lossless_only: bool) -> Vec<String> {
        arrange(setting, quality_first, lossless_only, true)
    }

    /// The README's order with highest quality first: Qobuz/Tidal/Deezer → FLAC on Soulseek →
    /// torrents/Usenet → Soulseek in any format → YouTube Music. Torrents and Usenet used to come after
    /// Soulseek in any format, so they only ran when nobody on Soulseek had the album at all.
    #[test]
    fn quality_first_tries_the_indexers_before_soulseek_in_any_format() {
        assert_eq!(order("tidal,deezer,slskd,torznab,ytdlp", true, false), ["streamrip", "slskd_lossless", "torznab", "slskd", "ytdlp"]);
        // wherever the admin put the indexers
        assert_eq!(order("qobuz,tidal,deezer,torznab,slskd,ytdlp", true, false), ["streamrip", "slskd_lossless", "torznab", "slskd", "ytdlp"]);
        assert_eq!(order("slskd,ytdlp,torznab,deezer", true, false), ["streamrip", "slskd_lossless", "torznab", "slskd", "ytdlp"]);
        // an order saved before the indexers existed gets them before YouTube Music
        assert_eq!(order("deezer,slskd,ytdlp", true, false), ["streamrip", "slskd_lossless", "torznab", "slskd", "ytdlp"]);
        // no Soulseek in the order: no Soulseek steps
        assert_eq!(order("deezer,torznab,ytdlp", true, false), ["streamrip", "torznab", "ytdlp"]);
        // without highest quality first the admin's order stands
        assert_eq!(order("tidal,slskd,torznab,ytdlp", false, false), ["streamrip", "slskd", "torznab", "ytdlp"]);
        assert_eq!(arrange("ytdlp,slskd", false, false, false), ["torznab", "ytdlp", "slskd"]);
    }

    /// A request for a lossless copy (an upgrade) asks the indexers too: their search keeps only lossless
    /// releases then. They used to be dropped, so upgrades never reached torrents or Usenet.
    #[test]
    fn a_lossless_copy_is_looked_for_on_the_indexers_too() {
        assert_eq!(order("tidal,deezer,slskd,torznab,ytdlp", true, true), ["streamrip", "slskd_lossless", "torznab"]);
        assert_eq!(order("tidal,deezer,slskd,torznab,ytdlp", false, true), ["streamrip", "slskd_lossless", "torznab"]);
    }
}
