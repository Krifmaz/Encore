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
        // the plugin first (a link nothing can download fails at once), then the staging folder the link is
        // saved in: while that can't be written the link waits for it
        if RIP_LINK.is_match(&url) {
            if let Some(why) = wait_for_install(st, r, "streamrip") {
                return why;
            }
            if !streamrip::enabled(st) {
                fail(st, r, storage::LINKS_NEED_STREAMRIP.0).await;
                return Ok(());
            }
            if downloads::hold(st, r, "url", false).await? {
                return Ok(());
            }
            upd(st, r, &[("source", json!("streamrip")), ("status", json!("downloading")), ("message", json!("Downloading with streamrip…"))]);
            return run_streamrip_url(st, r).await;
        }
        if let Some(why) = wait_for_install(st, r, "ytdlp") {
            return why;
        }
        if !ytdlp::plugin_on(st) {
            fail(st, r, storage::LINKS_NEED_YTDLP.0).await;
            return Ok(());
        }
        if downloads::hold(st, r, "url", false).await? {
            return Ok(());
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
    // (boxed: the lookup's future is large, and a step's runs on a worker thread's stack)
    if kind == "track" && Box::pin(try_album_upgrade(st, r)).await? {
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
    // why the song goes on its own (MusicBrainz couldn't say which album it is on), for its details
    if let Some(note) = r.d["album_note"].as_str().map(String::from) {
        log_push(r, format!("album lookup: {note}"));
    }
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
            // (what the sources said: the album lookup's line is not a source's)
            let log: Vec<String> = r.d["log"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).filter(|l| !l.starts_with("album lookup:")).collect()).unwrap_or_default();
            let tried = if log.is_empty() { "no download sources enabled".to_string() } else { log[log.len().saturating_sub(3)..].join("; ") };
            fail(st, r, &format!("All sources exhausted ({tried})")).await;
            return Ok(());
        }
        let src = srcs.remove(0);
        r.d["sources"] = json!(srcs);
        // the folder this source saves in can't be written: the request waits for it, this source first again
        if downloads::hold(st, r, &src, true).await? {
            return Ok(());
        }
        let soulseek = src == "slskd" || src == "slskd_lossless";
        // a plugin the upgrade is still installing: the request waits for it (at most an hour) instead of
        // going on to a lower-quality source
        if let Some(name) = being_installed(st, &src) {
            match install_waited_out(r, &src) {
                Some(why) => {
                    stop_waiting_for_install(r, &src);
                    log_push(r, format!("{src}: {why}"));
                    reason.clear();
                    continue;
                }
                None => return park_for_install(st, r, &src, name, true),
            }
        }
        stop_waiting_for_install(r, &src);
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
            // a plugin program's source (uploaded or installed from a URL): searched and downloaded through the
            // registry
            other => match crate::sources::plugin::find(st, other) {
                Some(p) => start_plugin(st, r, &p, None).await,
                None => Ok(Out::Next(format!("unknown source {other}"))),
            },
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

/// How long a request waits for a source plugin the upgrade is still installing before it goes on to its next
/// source (seconds).
pub(super) const INSTALL_WAIT_MAX: f64 = 3600.0;

/// A source whose plugin the upgrade adopted but whose program Encore is still installing (the first start's
/// install pass, tried again until it works – encore_plugins::Registry::installing_after_upgrade): the
/// plugin's name. None: the source is there, or simply missing.
pub(super) fn being_installed(st: &impl Ctx, src: &str) -> Option<&'static str> {
    match src {
        "streamrip" => (streamrip::plugin_on(st) && st.cfg().bool("streamrip_enabled") && streamrip::binary(st).is_none() && st.plugins().installing_after_upgrade("streamrip")).then_some("streamrip"),
        "ytdlp" => (ytdlp::plugin_on(st) && !ytdlp::program_found(st) && st.plugins().installing_after_upgrade("ytdlp")).then_some("yt-dlp"),
        _ => None,
    }
}

/// The request has waited an hour for `src`'s plugin: why it goes on without it.
fn install_waited_out(r: &Req, src: &str) -> Option<String> {
    let w = &r.d["install_wait"];
    let long = w["source"].as_str() == Some(src) && w["since"].as_f64().is_some_and(|t| now() - t >= INSTALL_WAIT_MAX);
    long.then(|| format!("the {src} plugin was still being installed after 60 minutes – trying the next source"))
}

/// A link or a pick that needs `src` (streamrip, ytdlp) whose plugin is still being installed: parked
/// (`Some(Ok)`), or `Some(fail…)` after an hour; None: go on.
fn wait_for_install(st: &impl Ctx, r: &mut Req, src: &str) -> Option<anyhow::Result<()>> {
    let Some(name) = being_installed(st, src) else {
        stop_waiting_for_install(r, src);
        return None;
    };
    if install_waited_out(r, src).is_some() {
        stop_waiting_for_install(r, src);
        return None; // the step runs and says what is missing
    }
    Some(park_for_install(st, r, src, name, false))
}

/// The request goes on with `src`: it no longer waits for a plugin's install.
fn stop_waiting_for_install(r: &mut Req, src: &str) {
    if let Some(d) = r.d.as_object_mut() {
        if d.get("wait").is_some_and(|w| w["reason"] == storage::PLUGIN_INSTALL) {
            d.remove("wait");
        }
        if d.get("install_wait").is_some_and(|w| w["source"] == src) {
            d.remove("install_wait");
        }
    }
}

/// Park the request until `src`'s plugin is installed: `waiting` (reason "plugin_install"), "Waiting for the
/// streamrip plugin to finish installing". `in_sources`: a request's source step (it goes first in its
/// sources again, and the queue goes on from there); else a link or a pick, started again. The queue looks at
/// it again every few seconds ([`super::queue`]). Writes only what changed.
pub(super) fn park_for_install(st: &impl Ctx, r: &mut Req, src: &str, name: &str, in_sources: bool) -> anyhow::Result<()> {
    check(st, r.id)?;
    if in_sources {
        let mut srcs: Vec<Value> = r.d["sources"].as_array().cloned().unwrap_or_default();
        srcs.insert(0, json!(src));
        r.d["sources"] = json!(srcs);
    }
    let mut changed = false;
    if r.d["install_wait"]["source"].as_str() != Some(src) {
        r.d["install_wait"] = json!({ "source": src, "since": now(), "step": if in_sources { "source" } else { "start" } });
        log_push(r, format!("{src}: waiting for the {name} plugin to finish installing (Encore installs the programs that were in use before the upgrade)"));
        changed = true;
    }
    let wait = json!({ "reason": storage::PLUGIN_INSTALL, "since": r.d["install_wait"]["since"] });
    if r.d["wait"] != wait {
        r.d["wait"] = wait;
        changed = true;
    }
    let msg = format!("Waiting for the {name} plugin to finish installing");
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
    // the sources of plugin programs: the lossless ones count with the lossless options
    let plugins = st.plugins().plugin_sources();
    let lossless: Vec<String> = plugins.iter().filter(|p| p.lossless).map(|p| p.id.clone()).collect();
    let order = arrange(&st.cfg().str("source_order"), st.cfg().bool("quality_first"), lossless_only, st.cfg().bool("ytdlp_last_resort"), &lossless);
    let kind = r.s("kind");
    let album = kind == "album";
    // every source through its plugin: installed and switched on (the registry), then set up
    let avail = |s: &str| match s {
        "torznab" => torznab::enabled(st) && album,
        "slskd" | "slskd_lossless" => slskd::enabled(st),
        "ytdlp" => ytdlp::plugin_on(st) && (ytdlp::enabled(st) || st.cfg().bool("ytdlp_last_resort")),
        // still being installed after the upgrade: kept in its place, the request waits for it there
        "streamrip" => streamrip::enabled(st) || being_installed(st, "streamrip").is_some(),
        // through the registry like a plugin program's source, once signed in
        "zotify" => plugins.iter().any(|p| p.id == "zotify" && p.enabled && p.kinds.contains(&kind)) && crate::sources::zotify::search_ready(st),
        "gamdl" => plugins.iter().any(|p| p.id == "gamdl" && p.enabled && p.kinds.contains(&kind)) && crate::sources::gamdl::signed_in(st),
        other => plugins.iter().any(|p| p.id == other && p.enabled && p.kinds.contains(&kind)),
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
fn arrange(source_order: &str, quality_first: bool, lossless_only: bool, ytdlp_last_resort: bool, lossless_plugins: &[String]) -> Vec<String> {
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
        let plugin = |s: &String| lossless_plugins.contains(s);
        let first = |s: &String| s == "streamrip" || s == "torznab" || plugin(s);
        let mut lossless: Vec<String> = order.iter().filter(|s| *s == "streamrip").cloned().collect();
        if order.iter().any(|s| s == "slskd") {
            lossless.push("slskd_lossless".into());
        }
        if order.iter().any(|s| s == "torznab") {
            lossless.push("torznab".into());
        }
        // a plugin program that delivers lossless files: after the builtin lossless options, in the admin's order
        lossless.extend(order.iter().filter(|s| plugin(s)).cloned());
        lossless.extend(order.iter().filter(|s| !first(s)).cloned());
        order = lossless;
    }
    if lossless_only {
        order.retain(|s| ["streamrip", "slskd_lossless", "torznab"].contains(&s.as_str()) || lossless_plugins.contains(s));
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
    if !r.d["soulseek_wait"].is_object() && !r.d["install_wait"].is_object() {
        r.d["log"] = json!([]); // a pick that waited for Soulseek (or a plugin's install) keeps the line that says so
    }
    let src = pick["source"].as_str().unwrap_or("");
    let url = pick["payload"]["url"].as_str().unwrap_or("").to_string();
    let rip = matches!(src, "deezer" | "tidal" | "qobuz") || (src == "link" && RIP_LINK.is_match(&url));
    // the plugin first: a pick nothing can download fails at once (or waits for its plugin's install) instead
    // of waiting for the folder and failing after
    if rip {
        if let Some(why) = wait_for_install(st, r, "streamrip") {
            return why;
        }
        if !streamrip::enabled(st) {
            fail(st, r, storage::LINKS_NEED_STREAMRIP.0).await;
            return Ok(());
        }
    } else if matches!(src, "ytmusic" | "link") {
        if let Some(why) = wait_for_install(st, r, "ytdlp") {
            return why;
        }
        if !ytdlp::plugin_on(st) {
            fail(st, r, storage::PICKS_NEED_YTDLP.0).await;
            return Ok(());
        }
    }
    // the folder the pick is saved in can't be written: the pick waits for it
    if downloads::hold(st, r, downloads::pick_source(src, &url), false).await? {
        return Ok(());
    }
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
        // the plugin was checked above
        "deezer" | "tidal" | "qobuz" | "link" if rip => {
            upd(st, r, &[("source", json!("streamrip"))]);
            start_streamrip(st, r, Some(vec![url])).await?
        }
        "ytmusic" | "link" => {
            upd(st, r, &[("source", json!("ytdlp"))]);
            start_ytdlp(st, r, Some(url)).await?
        }
        // a plugin program's result: downloaded through the registry
        other => match crate::sources::plugin::find(st, other) {
            Some(p) => {
                upd(st, r, &[("source", json!(p.id))]);
                start_plugin(st, r, &p, Some(pick["payload"].clone())).await?
            }
            None => Out::Next(format!("unknown source {other}")),
        },
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
    const LOOKING: &str = "Looking up discography…";
    upd(st, r, &[("status", json!("searching")), ("message", json!(LOOKING))]);
    // (while MusicBrainz's line is long the message says so: "Looking up discography – waiting for MusicBrainz …")
    let mbid = r.s("mbid");
    let art = with_steps(st, r.id, "searching", "Looking up discography", LOOKING, mb::artist(st, &mbid)).await?;
    // the request's own boxes (Request discography panel), else Settings → Release types; the releases ticked
    // in the dialog (`release_ids`) take the place of the filter altogether
    let filter = encore_config::release_types::ReleaseFilter::from_value(&r.d["release_filter"], &encore_config::release_types::ReleaseFilter::from_cfg(st.cfg()));
    let only: Option<Vec<String>> = r.d["release_ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());

    // the merged catalog: MusicBrainz plus the Deezer / Apple Music releases MusicBrainz doesn't list
    let ext = if st.cfg().bool("discover_catalog_merge") {
        with_steps(st, r.id, "searching", "Looking up discography", LOOKING, encore_library::catalog::merged(st, &art)).await
    } else {
        vec![]
    };
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

/// The album lookup's words while it runs (and its base for what it waits for: "Looking up which album this
/// song is on – waiting for MusicBrainz (about 2 minutes, 120 lookups ahead)").
pub(super) const LOOKING_UP_ALBUM: &str = "Looking up which album this song is on…";
const LOOKING_UP_ALBUM_BASE: &str = "Looking up which album this song is on";
/// How long one try of the album lookup waits for its first turn in MusicBrainz's line before it stops
/// waiting, and how long the rest of it may take after that turn (its next calls keep the first one's place).
pub const ALBUM_WAIT: std::time::Duration = std::time::Duration::from_secs(300);
/// The tries before the song goes on without its album, and the time between two of them (seconds).
pub const ALBUM_TRIES: u64 = 3;
pub const ALBUM_RETRY_SECS: f64 = 300.0;
/// A try that gave up in the line looks this often whether the line has got short, so its next try can start
/// sooner (seconds; a little more for some requests, so they don't all come back at once).
const ALBUM_LOOK_SECS: f64 = 30.0;

/// What one try of the album lookup came to.
enum Lookup {
    /// MusicBrainz answered: the recording it found and the album it is on (either may be none)
    Answer(Option<String>, Option<Value>),
    /// MusicBrainz is busy: why, in words, and whether it was the line here (no turn in time) rather than
    /// MusicBrainz saying so
    Busy(String, bool),
    /// MusicBrainz failed (down, out of reach, an error of its own): why, in words
    Failed(String),
}

fn status_of(e: &anyhow::Error) -> Option<reqwest::StatusCode> {
    e.chain().find_map(|c| c.downcast_ref::<reqwest::Error>().and_then(|r| r.status()))
}

/// Whether MusicBrainz said no for good (a 4xx other than 429: an id it doesn't know, say) rather than "not now".
fn refused_for_good(e: &anyhow::Error) -> bool {
    status_of(e).is_some_and(|s| s.is_client_error() && s.as_u16() != 429)
}

/// Whether MusicBrainz said it is busy: still a 503 after its back-off, or a 429.
fn said_busy(e: &anyhow::Error) -> bool {
    mb::is_busy(e) || status_of(e).is_some_and(|s| s.as_u16() == 429)
}

/// "MusicBrainz is busy (1 lookup ahead)", "MusicBrainz is busy (412 lookups ahead)".
fn busy_words(ahead: usize) -> String {
    match ahead {
        0 => "MusicBrainz is busy".into(),
        1 => "MusicBrainz is busy (1 lookup ahead)".into(),
        n => format!("MusicBrainz is busy ({n} lookups ahead)"),
    }
}

/// One try: the recording (its MusicBrainz id, else the ISRC, else the names) and the album it is on. Its
/// calls keep one place in MusicBrainz's line: it waits at most [`ALBUM_WAIT`] for its first turn, and its
/// next call comes right after that turn instead of at the back of the line. While it waits the request's
/// message says how long and behind how many lookups.
async fn album_on_musicbrainz(st: &impl Ctx, r: &Req, allow_ep: bool) -> Lookup {
    let (mbid, artist, title, length, isrc) = (r.opt("mbid"), r.s("artist"), r.s("title"), r.length(), r.d["isrc"].as_str().map(String::from));
    let lookup = async {
        let rid = match mbid {
            Some(m) => Some(m),
            None => encore_library::matcher::resolve_recording(st, &artist, &title, length, isrc.as_deref()).await?,
        };
        let alb = match &rid {
            Some(id) => mb::album_for_recording(st, id, allow_ep).await?,
            None => None,
        };
        anyhow::Ok((rid, alb))
    };
    // (boxed: a step's future stays small on the worker thread's stack)
    let tried = mb::one_place_within(ALBUM_WAIT, ALBUM_WAIT, Box::pin(lookup));
    match with_steps(st, r.id, "searching", LOOKING_UP_ALBUM_BASE, LOOKING_UP_ALBUM, tried).await {
        Some(Ok((rid, alb))) => Lookup::Answer(rid, alb),
        Some(Err(e)) => {
            st.db().log("warning", &format!("Album lookup failed for {}: {e}", r.label()));
            if refused_for_good(&e) {
                Lookup::Answer(None, None)
            } else if said_busy(&e) {
                Lookup::Busy(busy_words(0), false)
            } else {
                Lookup::Failed("MusicBrainz didn't answer".into())
            }
        }
        None => Lookup::Busy(busy_words(st.mb().ahead_of(mb::Priority::Request)), true),
    }
}

/// Words in a Deezer album title that mark a best-of or another collection, besides the extras the catalog
/// merge reads from titles ([`encore_config::release_types::marks`]: live, remix, soundtrack, "greatest hits" …).
static COLLECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bbest of\b|\bvery best\b|\bgreatest\b|\bhits\b|\bcollection\b|\banthology\b|\bessentials?\b|\bthe singles\b|\bretrospective\b").unwrap()
});

/// The album the song is on as Deezer names it, by the song's ISRC: an album (or an EP when EPs count) of the
/// song's own artist – never a single, a compilation or best-of, a live album, a soundtrack or a remix album
/// (Deezer often names a later release of the song: those are left out as MusicBrainz's lookup leaves them
/// out). → (title, artist, "Album"/"EP", year, cover, the catalogue ids for the album request's `ext`)
async fn deezer_album(st: &impl Ctx, isrc: &str, artist: &str, allow_ep: bool) -> Option<Value> {
    use encore_http::deezer;
    let t = std::time::Duration::from_secs(15);
    let track = deezer::get(st, deezer::track_by_isrc(st, isrc).timeout(t)).await.ok()?;
    let id = track["album"]["id"].as_i64()?;
    let a = deezer::get(st, deezer::album(st, id).timeout(t)).await.ok()?;
    let typ = match a["record_type"].as_str()? {
        "album" => "Album",
        "ep" if allow_ep => "EP",
        _ => return None,
    };
    let name = a["artist"]["name"].as_str().filter(|n| !n.is_empty())?;
    if tagger::sim(name, artist) < 0.75 {
        return None; // another artist's album (a various-artists release, say)
    }
    let title = a["title"].as_str().filter(|x| !x.is_empty())?;
    let mut ext = json!({ "deezer": id });
    if !encore_config::release_types::marks(&json!({ "title": title, "type": typ, "ext": ext })).is_empty() || COLLECTION.is_match(title) {
        return None;
    }
    let year: String = a["release_date"].as_str().unwrap_or("").chars().take(4).collect();
    let cover = ["cover_xl", "cover_big", "cover"].iter().find_map(|k| a[*k].as_str().filter(|c| !c.is_empty())).map(String::from);
    if let Some(upc) = a["upc"].as_str().filter(|u| !u.is_empty()) {
        ext["upc"] = json!(upc);
    }
    Some(json!({ "title": title, "artist": name, "type": typ, "year": year, "cover": cover, "ext": ext }))
}

/// An album request already there for this album, whichever way it was made: the same release group, the same
/// Deezer album or barcode, or the same artist and title (edition notes aside: "Discovery" and "Discovery
/// (Remastered)"). MusicBrainz's answer for one song and Deezer's for another song of the same album (while
/// MusicBrainz was busy) used to make two album requests, and both downloaded.
fn album_request_for(st: &impl Ctx, mbid: Option<&str>, artist: &str, title: &str, ext: &Value) -> anyhow::Result<Option<i64>> {
    let deezer = ext["deezer"].as_i64().map(|d| d.to_string()).or_else(|| ext["deezer"].as_str().map(String::from)).filter(|d| !d.is_empty());
    let upc = ext["upc"].as_str().filter(|u| !u.is_empty());
    let rows = st.db().query("SELECT id, mbid, artist, title, album, detail FROM requests WHERE kind='album' AND status NOT IN ('failed','denied','cancelled')
                              AND (mbid=? OR lower(artist)=lower(?) OR CAST(json_extract(detail, '$.ext.deezer') AS TEXT)=? OR json_extract(detail, '$.ext.upc')=?)
                              ORDER BY id", p![mbid, artist, deezer, upc])?;
    let same_ext = |row: &Row| {
        let d: Value = serde_json::from_str(&row.s("detail")).unwrap_or(json!({}));
        let theirs = d["ext"]["deezer"].as_i64().map(|x| x.to_string()).or_else(|| d["ext"]["deezer"].as_str().map(String::from));
        (deezer.is_some() && theirs == deezer) || (upc.is_some() && d["ext"]["upc"].as_str() == upc)
    };
    let key = encore_library::catalog::title_key(title);
    let same_name = |row: &Row| {
        let theirs = Some(row.s("album")).filter(|a| !a.is_empty()).unwrap_or_else(|| row.s("title"));
        !key.is_empty() && encore_library::catalog::title_key(&theirs) == key && encore_core::text::norm(&row.s("artist")) == encore_core::text::norm(artist)
    };
    let found = rows.iter().find(|row| mbid.is_some_and(|m| row.s("mbid") == m))
        .or_else(|| rows.iter().find(|row| same_ext(row)))
        .or_else(|| rows.iter().find(|row| same_name(row)));
    Ok(found.map(|row| row.i("id")))
}

/// The whole album instead of the song: the album request (one that is already there, or a new one) and the
/// song following it. `alb`: {title, artist, type, year, cover} with `id` (a MusicBrainz release group) or
/// `ext` (the album only Deezer named). None: the album is in the library already (the song isn't on it in
/// this version), so the song goes on its own.
fn follow_album(st: &impl Ctx, r: &mut Req, alb: &Value) -> anyhow::Result<Option<i64>> {
    let title = alb["title"].as_str().unwrap_or("").to_string();
    let artist = alb["artist"].as_str().filter(|a| !a.is_empty()).map(String::from).unwrap_or_else(|| r.s("artist"));
    let mbid = alb["id"].as_str().filter(|x| !x.is_empty());
    let there = match mbid {
        Some(id) => encore_library::has_release_group(st.db(), id),
        None => encore_library::has_album(st.db(), &artist, &title),
    };
    if there {
        return Ok(None);
    }
    let typ = alb["type"].as_str().unwrap_or("album").to_lowercase();
    let aid = match album_request_for(st, mbid, &artist, &title, &alb["ext"])? {
        Some(e) => e,
        None => {
            let t = now();
            let detail = if mbid.is_none() && alb["ext"].is_object() { json!({ "ext": alb["ext"] }) } else { json!({}) };
            let year = alb["year"].as_str().filter(|y| !y.is_empty()).map(|y| json!(y)).unwrap_or(Value::Null);
            let id = st.db().execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, message, detail, created_at, updated_at)
                                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                p![r.row.get("user_id"), "album", mbid, artist, title, title, year, alb["cover"], "queued",
                   format!("Full album for “{}”", r.s("title")), detail.to_string(), t, t])?;
            st.db().log("info", &format!("“{}” is on {typ} “{title}” – downloading the whole {typ}", r.s("title")));
            crate::events::emit(st, id);
            id
        }
    };
    r.d["album_request"] = json!(aid);
    let cover = r.opt("cover_url").map(|c| json!(c)).unwrap_or(alb["cover"].clone());
    upd(st, r, &[("status", json!("monitoring")), ("source", json!("album")), ("cover_url", cover),
                 ("message", json!(format!("Getting the whole {typ} “{title}”")))]);
    Ok(Some(aid))
}

/// The lookup is over without an album: the song goes on its own. `note`: why, for the request's log – the
/// lookup gave up (MusicBrainz couldn't answer), and the next attempt of the request looks again. Without a
/// note MusicBrainz answered, and a request asked for again within a day doesn't ask it again
/// ([`crate::create::detail_for_retry`]).
fn no_album(r: &mut Req, note: Option<String>) {
    r.d["no_album"] = json!(true);
    if let Some(d) = r.d.as_object_mut() {
        d.remove("album_lookup");
    }
    match note {
        Some(n) => r.d["album_note"] = json!(n),
        None => r.d["album_checked_at"] = json!(now()),
    }
}

/// Whether MusicBrainz's line is short enough now for a lookup's first turn to come well within [`ALBUM_WAIT`].
fn line_is_short(st: &impl Ctx) -> bool {
    st.mb().ahead_of(mb::Priority::Request) as f64 * mb::gap(st).as_secs_f64() < ALBUM_WAIT.as_secs_f64() / 2.0
}

/// Wait for the next try, due at `at`. A try that gave up in the line here (`in_line`) starts sooner once the
/// line is short – MusicBrainz would sit idle meanwhile; a try MusicBrainz said it was too busy for waits it out.
async fn until_next_try(st: &impl Ctx, id: i64, at: f64, in_line: bool) {
    let look = ALBUM_LOOK_SECS + id.rem_euclid(15) as f64;
    loop {
        let left = at - now();
        if left <= 0.0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs_f64(if in_line { left.min(look) } else { left })).await;
        if in_line && line_is_short(st) {
            return;
        }
    }
}

/// If the song is on an official album, fetch the whole album instead (best quality available).
///
/// The lookup asks MusicBrainz, behind pages and imports in its line (it says how long it waits). When
/// MusicBrainz can't answer now the album Deezer names for the song's ISRC is taken. Without one:
/// - MusicBrainz busy ([`ALBUM_WAIT`] passed without a turn in the line, or MusicBrainz said so): the lookup
///   tries again [`ALBUM_RETRY_SECS`] later – sooner once the line is short – its message saying why and when
///   ("… – MusicBrainz is busy (412 lookups ahead). Next try by 14:20"), and after [`ALBUM_TRIES`] tries the
///   song goes on its own, the request's log saying why;
/// - MusicBrainz failing (down, out of reach, an error): the song goes on its own at once, as it did before the
///   tries, the request's log saying why – waiting would make every request wait while it is down.
pub(super) async fn try_album_upgrade(st: &impl Ctx, r: &mut Req) -> anyhow::Result<bool> {
    let mode = match r.d["album_mode"].as_str() { Some("song") => "off".to_string(), Some("album") => "album_ep".to_string(), _ => st.cfg().str("album_upgrade") };
    if mode == "off" || r.d["no_album"].as_bool().unwrap_or(false) {
        return Ok(false);
    }
    let allow_ep = mode == "album_ep";
    loop {
        // a try that is due later (the server restarted between two tries): its wait first
        if let Some(at) = r.d["album_lookup"]["next_try_at"].as_f64().filter(|t| *t > now()) {
            let why = r.d["album_lookup"]["why"].as_str().unwrap_or("MusicBrainz is busy").to_string();
            let in_line = r.d["album_lookup"]["in_line"].as_bool().unwrap_or(false);
            // (a try that gave up in the line may come sooner: "by")
            let when = format!("{} {}", if in_line { "by" } else { "at" }, crate::slskd_session::clock(at));
            upd(st, r, &[("status", json!("searching")), ("message", json!(format!("{LOOKING_UP_ALBUM_BASE} – {why}. Next try {when}")))]);
            until_next_try(st, r.id, at, in_line).await;
            check(st, r.id)?;
        }
        upd(st, r, &[("status", json!("searching")), ("message", json!(LOOKING_UP_ALBUM))]);
        let (why, in_line) = match Box::pin(album_on_musicbrainz(st, r, allow_ep)).await {
            Lookup::Answer(rid, alb) => {
                check(st, r.id)?;
                if rid.is_some() {
                    r.d["recording"] = json!(rid);
                }
                if let Some(alb) = alb {
                    r.d["album_checked_at"] = json!(now());
                    if let Some(d) = r.d.as_object_mut() {
                        d.remove("album_lookup");
                    }
                    if follow_album(st, r, &alb)?.is_some() {
                        return Ok(true);
                    }
                }
                // on no official album (or on one the library has, without this version of the song)
                no_album(r, None);
                upd(st, r, &[]);
                return Ok(false);
            }
            Lookup::Busy(why, in_line) => (why, Some(in_line)),
            Lookup::Failed(why) => (why, None),
        };
        check(st, r.id)?;
        // the album the streaming service names for the song, while MusicBrainz can't say
        if let Some(isrc) = r.d["isrc"].as_str().filter(|i| !i.is_empty()).map(String::from) {
            if let Some(alb) = Box::pin(deezer_album(st, &isrc, &r.s("artist"), allow_ep)).await {
                check(st, r.id)?;
                let note = format!("{why} – Deezer says the song is on the {} “{}”", alb["type"].as_str().unwrap_or("Album").to_lowercase(), alb["title"].as_str().unwrap_or(""));
                if follow_album(st, r, &alb)?.is_some() {
                    st.db().log("info", &format!("Album lookup for {}: {note}", r.label()));
                    r.d["album_checked_at"] = json!(now());
                    r.d["album_note"] = json!(note);
                    if let Some(d) = r.d.as_object_mut() {
                        d.remove("album_lookup");
                    }
                    upd(st, r, &[]);
                    return Ok(true);
                }
            }
        }
        let Some(in_line) = in_line else {
            let note = format!("{why} – the song is downloaded on its own");
            st.db().log("warning", &format!("Album lookup for {}: {note}", r.label()));
            no_album(r, Some(note));
            upd(st, r, &[]);
            return Ok(false);
        };
        let tries = r.d["album_lookup"]["tries"].as_u64().unwrap_or(0) + 1;
        if tries >= ALBUM_TRIES {
            let note = format!("{why} – after {tries} tries the song is downloaded on its own");
            st.db().log("warning", &format!("Album lookup for {}: {note}", r.label()));
            no_album(r, Some(note));
            upd(st, r, &[]);
            return Ok(false);
        }
        let at = now() + ALBUM_RETRY_SECS;
        r.d["album_lookup"] = json!({ "tries": tries, "next_try_at": at, "why": why, "in_line": in_line });
        upd(st, r, &[]);
    }
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
    r.d["album_checked_at"] = json!(now()); // asked for again within a day, the song goes on its own at once
    upd(st, r, &[("status", json!("queued")), ("source", Value::Null), ("message", json!(format!("Getting the song on its own ({reason})")))]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::arrange;

    fn order(setting: &str, quality_first: bool, lossless_only: bool) -> Vec<String> {
        arrange(setting, quality_first, lossless_only, true, &[])
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
        assert_eq!(arrange("ytdlp,slskd", false, false, false, &[]), ["torznab", "ytdlp", "slskd"]);
    }

    /// A plugin program's source keeps its place in the admin's order; a lossless one joins the lossless
    /// options (after the builtin ones) with highest quality first and for a lossless copy, a lossy one does not.
    #[test]
    fn plugin_sources_take_their_place() {
        let lossless = vec!["tone".to_string()];
        assert_eq!(arrange("tone,deezer,slskd,lossy,ytdlp", false, false, true, &lossless), ["tone", "streamrip", "slskd", "lossy", "torznab", "ytdlp"]);
        assert_eq!(arrange("deezer,slskd,lossy,tone,ytdlp", true, false, true, &lossless), ["streamrip", "slskd_lossless", "torznab", "tone", "slskd", "lossy", "ytdlp"]);
        assert_eq!(arrange("deezer,slskd,lossy,tone,ytdlp", true, true, true, &lossless), ["streamrip", "slskd_lossless", "torznab", "tone"]);
    }

    /// A request for a lossless copy (an upgrade) asks the indexers too: their search keeps only lossless
    /// releases then. They used to be dropped, so upgrades never reached torrents or Usenet.
    #[test]
    fn a_lossless_copy_is_looked_for_on_the_indexers_too() {
        assert_eq!(order("tidal,deezer,slskd,torznab,ytdlp", true, true), ["streamrip", "slskd_lossless", "torznab"]);
        assert_eq!(order("tidal,deezer,slskd,torznab,ytdlp", false, true), ["streamrip", "slskd_lossless", "torznab"]);
    }
}
