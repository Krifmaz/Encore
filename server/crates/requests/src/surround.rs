//! Fetching Dolby Atmos versions into the Surround library (0.35): whether Apple Music has one for a stereo album
//! or song (asked through the gamdl plugin, kept in `surround_checks`: a yes for a week, a no for two days), the
//! requests that download it (`class = 'surround'`, locked to the Atmos result like a hand pick), and the
//! automatic fetch after a stereo request when `surround_auto_atmos` is on, and the library sweep that looks for
//! every stereo album's Atmos version ([`sweep`]: "Find Dolby Atmos versions" on the Surround tab and the `atmos`
//! task). docs/contracts/surround.md.
use crate::sources::gamdl;
use crate::Ctx;
use encore_core::error::{err, ApiResult};
use encore_db::{now, p, Row, RowExt};
use serde_json::{json, Value};

/// A yes (Apple Music has Atmos) is believed this long.
const YES_FOR: f64 = 7.0 * 86400.0;
/// A no is believed this long.
const NO_FOR: f64 = 2.0 * 86400.0;
/// The version of [`pick`] a kept yes was found with (`matcher` in `surround_checks.result`): a yes from an
/// older, looser match is asked again.
const MATCHER: i64 = 2;

/// The search a stereo album asks Apple Music (the SDK's request fields), plus what [`pick`] checks the
/// answers against: `year` and `album_kind` (album, single, ep or compilation).
fn album_ask(st: &impl Ctx, album_id: &str) -> Option<Value> {
    let r = st.db().one(
        "SELECT MAX(t.albumartist) albumartist, MAX(t.artist) artist, MAX(t.album) album, COUNT(*) n, MIN(t.year) year,
                MAX(COALESCE(t.tracktotal, 0)) total, MAX(COALESCE(t.compilation, 0)) compilation,
                MAX(a.chosen) chosen, MAX(a.release_type) release_type, MAX(a.release_secondary) secondary, MAX(a.release_tracks) release_tracks
         FROM tracks t LEFT JOIN album_types a ON a.album_id = t.album_id WHERE t.album_id=?",
        p![album_id],
    ).ok()??;
    if r.i("n") == 0 {
        return None;
    }
    let artist = Some(r.s("albumartist")).filter(|a| !a.is_empty()).unwrap_or_else(|| r.s("artist"));
    // the whole release's song count when the tags or MusicBrainz know it: the library may hold part of it
    let count = [r.i("release_tracks"), r.i("total")].into_iter().find(|n| *n > 0).unwrap_or(r.i("n")).max(r.i("n"));
    let kind = album_kind(&r.s("chosen"), &r.s("release_type"), &r.s("secondary"), r.i("compilation") == 1, count);
    Some(json!({ "kind": "album", "artist": artist, "album": r.s("album"), "title": r.s("album"), "track_count": count,
        "year": year_of(&r.s("year")), "album_kind": kind, "limit": 5 }))
}

/// What kind of release a library album is: `single`, `ep`, `compilation` or `album`. The owner's choice
/// (Change type) first, then MusicBrainz's or the tags' type, then the songs' compilation flag, then its size
/// (1–3 songs a single, 4–6 an EP: what the stores call them).
fn album_kind(chosen: &str, release_type: &str, secondary: &str, compilation: bool, songs: i64) -> &'static str {
    match chosen {
        "single" => return "single",
        "ep" => return "ep",
        "album" => return if compilation || secondary.contains("Compilation") { "compilation" } else { "album" },
        _ => {}
    }
    if compilation || secondary.contains("Compilation") {
        return "compilation";
    }
    match release_type {
        "Single" => "single",
        "EP" => "ep",
        "Album" => "album",
        _ if songs <= 3 => "single",
        _ if songs <= 6 => "ep",
        _ => "album",
    }
}

/// The year of a date tag ("2012", "2012-05-01"): "" when it has none.
fn year_of(date: &str) -> String {
    let y: String = date.trim().chars().take(4).collect();
    if y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) { y } else { String::new() }
}

/// The search a stereo song asks Apple Music.
fn track_ask(st: &impl Ctx, id: i64) -> Option<Value> {
    let r = st.db().one("SELECT artist, title, album, duration, isrc, year FROM tracks WHERE id=?", p![id]).ok()??;
    let mut ask = json!({ "kind": "track", "artist": r.s("artist"), "title": r.s("title"), "album": r.s("album"), "year": year_of(&r.s("year")), "limit": 5 });
    if r.f("duration") > 0.0 {
        ask["length"] = json!(r.f("duration"));
    }
    if !r.s("isrc").is_empty() {
        ask["isrc"] = json!(r.s("isrc"));
    }
    Some(ask)
}

/// Whether Apple Music has a Dolby Atmos version (the result to download), asked once in a while.
/// `key`: "album:<stereo album id>" or "track:<stereo track id>".
pub async fn check(st: &impl Ctx, key: &str, ask: &Value, fresh: bool) -> Result<Option<Value>, encore_plugins::Failure> {
    if !fresh {
        if let Some(c) = st.db().one("SELECT * FROM surround_checks WHERE key=?", p![key]).ok().flatten() {
            let age = now() - c.f("checked_at");
            let yes = c.i("available") == 1;
            let kept: Option<Value> = serde_json::from_str(&c.s("result")).ok();
            // a yes found before the strict match ([`pick`], 0.42) may be another release: asked again
            let current = kept.as_ref().is_some_and(|k| k["matcher"] == MATCHER);
            if (yes && current && age < YES_FOR) || (!yes && age < NO_FOR) {
                return Ok(if yes { kept } else { None });
            }
        }
    }
    let found = pick(ask, &gamdl::atmos_results(st, ask).await?).map(|mut f| {
        f["matcher"] = json!(MATCHER);
        f
    });
    let _ = st.db().execute(
        "INSERT OR REPLACE INTO surround_checks (key, checked_at, available, result) VALUES (?,?,?,?)",
        p![key, now(), found.is_some() as i64, found.as_ref().map(|f| f.to_string())],
    );
    Ok(found)
}

/// A title as the match compares it: lowercase words, without what the stores add to a name (" - Single",
/// " - EP", "(Dolby Atmos)" and the like) and without edition notes in brackets ("(Deluxe Edition)",
/// "[2011 Remaster]", "(10th Anniversary Edition)"). "(Live)", "(Remix)" or "(Taylor's Version)" stay: those
/// are other recordings. → (the words, whether an edition note was taken off)
fn core(title: &str) -> (String, bool) {
    let mut t = title.trim().trim_end_matches(" - Single").trim_end_matches(" - EP").to_string();
    t = encore_library::surround::stereo_name(&t);
    let mut edition = false;
    // edition notes at the end, one bracket group at a time
    loop {
        let trimmed = t.trim_end();
        let Some(close) = trimmed.chars().last().filter(|c| *c == ')' || *c == ']') else { break };
        let open = if close == ')' { '(' } else { '[' };
        let Some(at) = trimmed.rfind(open) else { break };
        let note = trimmed[at..].to_lowercase();
        let is_edition = ["deluxe", "expanded", "remaster", "anniversary", "bonus", "edition", "collector"].iter().any(|w| note.contains(w));
        if !is_edition {
            break;
        }
        edition = true;
        t = trimmed[..at].to_string();
    }
    (encore_core::text::ascii_words(&t.replace('&', " and ")), edition)
}

/// Whether an artist name from Apple is the library's: nearly all of the words of each in the other
/// ("Beyoncé" and "Beyoncé & JAY-Z" match, "Various Artists" and "Daft Punk" don't).
fn same_artist(theirs: &str, ours: &str) -> bool {
    use crate::sources::torznab::share;
    ours.trim().is_empty() || (share(theirs, ours) >= 0.8 && share(ours, theirs) >= 0.5)
}

/// Of Apple Music's Atmos results for an ask ([`album_ask`] or [`track_ask`]), the one that is the same
/// release as the library's, or None when none is sure enough: a wrong Atmos album filed beside the right
/// stereo one is worse than none.
///
/// An album must have the same name (edition notes aside), the same artist, a release year at most 2 years
/// off (Apple dates an Atmos edition by the original release) and about the same number of songs; and a
/// compilation, single or EP is only taken for a library album of that kind. Among the ones left, an exact
/// name, the same year and the same song count win. A song must have the same title and artist, be on the
/// library song's album (not a compilation or a single of it), and last within 5 s of it.
pub fn pick(ask: &Value, results: &[Value]) -> Option<Value> {
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let year = |v: &Value| s(v, "year").parse::<i64>().ok();
    let album = ask["kind"] == "album";
    let want_year = year(ask);
    let want_artist = s(ask, "artist");
    // (rank, result): bigger rank wins
    let mut best: Option<(i64, &Value)> = None;
    for r in results {
        if !same_artist(&s(r, "artist"), &want_artist) {
            continue;
        }
        let years_off = want_year.zip(year(r)).map(|(a, b)| (a - b).abs());
        let mut rank = 0;
        if album {
            let (want, want_ed) = core(&s(ask, "album"));
            let (got, got_ed) = core(&s(r, "album"));
            if want.is_empty() || got != want {
                continue;
            }
            // a compilation, single or EP only stands for one of its own kind
            let (ours, theirs) = (s(ask, "album_kind"), s(r, "album_kind"));
            let small = |k: &str| k == "single" || k == "ep";
            match theirs.as_str() {
                "compilation" if ours != "compilation" => continue,
                t if small(t) && !small(&ours) => continue,
                "album" if small(&ours) => rank -= 5, // the album a single is from: a fallback only
                _ => {}
            }
            if years_off.is_some_and(|d| d > 2) {
                continue;
            }
            if let (Some(want), Some(n)) = (ask["track_count"].as_i64().filter(|n| *n > 0), r["tracks"].as_i64().filter(|n| *n > 0)) {
                // a few songs fewer (Apple lacks some in Atmos) or a bonus disc more is the same album; half of
                // it or twice it is another release
                if n * 2 < want || n > want + (want / 2).max(3) {
                    continue;
                }
                rank += if n == want { 20 } else { 10 - (n - want).abs().min(10) };
            }
            rank += if want_ed == got_ed { 20 } else { 10 };
        } else {
            let (want, _) = core(&s(ask, "title"));
            let (got, _) = core(&s(r, "title"));
            if want.is_empty() || got != want {
                continue;
            }
            // on the song's own album, not a compilation, a single or another album of it
            let want_album = core(&s(ask, "album")).0;
            if !want_album.is_empty() && core(&s(r, "album")).0 != want_album {
                continue;
            }
            if let (Some(want), Some(d)) = (ask["length"].as_f64(), r["duration"].as_f64()) {
                if (d - want).abs() > 5.0 {
                    continue;
                }
            }
            if years_off.is_some_and(|d| d > 2) {
                continue;
            }
            let isrc = |v: &Value| s(v, "isrc").to_uppercase();
            if !isrc(ask).is_empty() && isrc(ask) == isrc(r) {
                rank += 20;
            }
        }
        rank += match years_off { Some(0) => 10, Some(_) => 5, None => 0 };
        if best.is_none_or(|(b, _)| rank > b) {
            best = Some((rank, r));
        }
    }
    best.map(|(_, r)| r.clone())
}

/// A Surround request that is not over yet for this key.
fn open_request(st: &impl Ctx, key: &str) -> Option<Row> {
    st.db().one(
        "SELECT id, status FROM requests WHERE class='surround' AND json_extract(detail, '$.surround_key')=? AND status NOT IN ('failed','denied','cancelled','completed') ORDER BY id DESC LIMIT 1",
        p![key],
    ).ok().flatten()
}

/// A Surround request locked to the Atmos result (the worker downloads exactly it, as a hand pick).
/// `user_id`: who asked (None for the library sweep's scheduled runs, like the lossless upgrades).
fn create(st: &impl Ctx, user_id: Option<i64>, key: &str, found: &Value, stereo_album: Option<&str>, stereo_tracks: &[i64], parent: Option<i64>) -> ApiResult<i64> {
    let kind = if found["payload"]["kind"] == "album" { "album" } else { "track" };
    let title = found["title"].as_str().unwrap_or("");
    let detail = json!({
        "pick": { "source": "gamdl", "payload": found, "title": title },
        "surround_key": key, "stereo_album_id": stereo_album, "stereo_track_ids": stereo_tracks,
        "length": found["duration"],
    });
    let t = now();
    let album = found["album"].as_str().unwrap_or("");
    let id = st.db().execute(
        "INSERT INTO requests (user_id, kind, artist, title, album, cover_url, status, message, detail, parent_id, class, created_at, updated_at)
         VALUES (?,?,?,?,?,?,'queued','Dolby Atmos version',?,?,'surround',?,?)",
        p![user_id, kind, found["artist"].as_str().unwrap_or(""), title, if kind == "album" { title } else { album }, found["cover"], detail.to_string(), parent, t, t],
    )?;
    st.db().log("info", &format!("Surround request #{id}: the Dolby Atmos version of {} – {title}", found["artist"].as_str().unwrap_or("")));
    crate::events::emit(st, id);
    Ok(id)
}

/// The error when gamdl can't fetch Atmos versions.
fn not_ready() -> encore_core::error::ApiError {
    err(409, "Dolby Atmos versions need the gamdl plugin with a wrapper-v2 server (Settings → Plugins → gamdl)")
}

/// A search failure as the API's error (sign-in 409, rate limit 429, else 502).
fn search_failed(f: encore_plugins::Failure) -> encore_core::error::ApiError {
    match f.code {
        encore_plugins::Code::Auth => err(409, f.message),
        encore_plugins::Code::RateLimited => err(429, f.message),
        _ => err(502, format!("Apple Music: {}", f.message)),
    }
}

/// "Get Dolby Atmos version" on a stereo album. → the request's id
pub async fn request_album(st: &impl Ctx, user_id: i64, album_id: &str) -> ApiResult<i64> {
    if !gamdl::atmos_ready(st) {
        return Err(not_ready());
    }
    let ask = album_ask(st, album_id).ok_or_else(|| err(404, "No such album"))?;
    let key = format!("album:{album_id}");
    if let Some(r) = open_request(st, &key) {
        return Err(err(409, format!("Already requested (#{}, {})", r.i("id"), r.s("status"))).with(json!({ "id": r.i("id"), "status": r.s("status") })));
    }
    let found = check(st, &key, &ask, true).await.map_err(search_failed)?.ok_or_else(|| err(404, "Apple Music has no Dolby Atmos version of this album"))?;
    let tracks: Vec<i64> = vec![];
    create(st, Some(user_id), &key, &found, Some(album_id), &tracks, None)
}

/// "Get Dolby Atmos version" on a stereo song. → the request's id
pub async fn request_track(st: &impl Ctx, user_id: i64, track_id: i64) -> ApiResult<i64> {
    if !gamdl::atmos_ready(st) {
        return Err(not_ready());
    }
    let ask = track_ask(st, track_id).ok_or_else(|| err(404, "No such song"))?;
    let key = format!("track:{track_id}");
    if let Some(r) = open_request(st, &key) {
        return Err(err(409, format!("Already requested (#{}, {})", r.i("id"), r.s("status"))).with(json!({ "id": r.i("id"), "status": r.s("status") })));
    }
    let found = check(st, &key, &ask, true).await.map_err(search_failed)?.ok_or_else(|| err(404, "Apple Music has no Dolby Atmos version of this song"))?;
    let album = st.db().one("SELECT album_id FROM tracks WHERE id=?", p![track_id]).ok().flatten().map(|r| r.s("album_id"));
    create(st, Some(user_id), &key, &found, album.as_deref(), &[track_id], None)
}

/// What the album page shows: the surround albums it has, a Surround request running, and whether Apple has an
/// Atmos version (from the last check: null when never asked).
pub fn album_state(st: &impl Ctx, album_id: &str) -> Value {
    let key = format!("album:{album_id}");
    let req = open_request(st, &key).map(|r| json!({ "id": r.i("id"), "status": r.s("status") }));
    let available = st.db().one("SELECT available FROM surround_checks WHERE key=?", p![key]).ok().flatten().map(|r| r.i("available") == 1);
    json!({ "have": encore_library::surround::for_stereo_album(st.db(), album_id), "request": req, "available": available, "can_fetch": gamdl::atmos_ready(st) })
}

/// What a song's menu shows.
pub fn track_state(st: &impl Ctx, track_id: i64) -> Value {
    let key = format!("track:{track_id}");
    let have = encore_library::surround::for_stereo_tracks(st.db(), &[track_id]).get(&track_id).copied();
    let req = open_request(st, &key).map(|r| json!({ "id": r.i("id"), "status": r.s("status") }));
    let available = st.db().one("SELECT available FROM surround_checks WHERE key=?", p![key]).ok().flatten().map(|r| r.i("available") == 1);
    json!({ "have": have, "request": req, "available": available, "can_fetch": gamdl::atmos_ready(st) })
}

/// A Surround request failed: the next check asks Apple again instead of offering the same result.
pub fn failed(st: &impl Ctx, d: &Value) {
    if let Some(k) = d["surround_key"].as_str() {
        let _ = st.db().execute("DELETE FROM surround_checks WHERE key=?", p![k]);
    }
}

/// After a stereo request brought songs (`new`): with `surround_auto_atmos` on and gamdl able to fetch Atmos,
/// their Dolby Atmos version is looked for and requested (a child of the stereo request: it counts against no
/// limit). In the background; once per album or song.
pub fn after_stereo(st: &impl Ctx, r: &crate::worker::Req, new: &[std::path::PathBuf]) {
    if !st.cfg().bool("surround_auto_atmos") || !gamdl::atmos_ready(st) || r.d["replace"].is_object() {
        return;
    }
    let kind = r.row.s("kind");
    if kind != "album" && kind != "track" {
        return;
    }
    let paths: Vec<String> = new.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let rows: Vec<Row> = paths.iter().filter_map(|p| st.db().one("SELECT id, album_id FROM tracks WHERE path=?", p![p]).ok().flatten()).collect();
    let Some(first) = rows.first() else { return };
    let (key, ask, album, tracks) = if kind == "album" {
        let a = first.s("album_id");
        let Some(ask) = album_ask(st, &a) else { return };
        (format!("album:{a}"), ask, Some(a), vec![])
    } else {
        let id = first.i("id");
        let Some(ask) = track_ask(st, id) else { return };
        (format!("track:{id}"), ask, Some(first.s("album_id")), vec![id])
    };
    // already there or asked for
    let have = match album.as_deref() {
        Some(a) if kind == "album" => !encore_library::surround::for_stereo_album(st.db(), a).is_empty(),
        _ => tracks.first().is_some_and(|t| encore_library::surround::for_stereo_tracks(st.db(), &[*t]).contains_key(t)),
    };
    if have || open_request(st, &key).is_some() {
        return;
    }
    let (st, user, parent) = (st.clone(), r.row.i("user_id"), r.id);
    tokio::spawn(async move {
        match check(&st, &key, &ask, false).await {
            Ok(Some(found)) => {
                let _ = create(&st, Some(user), &key, &found, album.as_deref(), &tracks, Some(parent));
            }
            Ok(None) => {}
            Err(e) => tracing::info!("surround: no Atmos check for {key}: {}", e.message),
        }
    });
}

/// The job the library sweep reports its progress under (`GET /api/surround/sweep`).
pub const SWEEP_JOB: &str = "surround_sweep";

/// How long the sweep waits between two albums, so a whole library doesn't hit Apple's rate limit.
const SWEEP_PACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// After this many failed checks in a row the sweep stops: the wrapper or the network is down, not one album.
const SWEEP_MAX_ERRORS: usize = 5;

/// What one run of the library sweep did.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct SweepReport {
    /// stereo albums asked about (from the cache or Apple Music)
    pub checked: usize,
    /// albums Apple Music has in Dolby Atmos
    pub found: usize,
    /// Surround requests queued for them
    pub requested: usize,
    /// checks that failed (network, a refused search)
    pub errors: usize,
    /// why the run stopped early: a sign-in or rate limit problem, too many errors, or cancelled
    pub stopped: Option<String>,
}

impl SweepReport {
    /// The one line Settings → Tasks and the Surround tab show.
    pub fn line(&self) -> String {
        let mut s = format!("{} album(s) checked, {} in Dolby Atmos, {} requested", self.checked, self.found, self.requested);
        if self.errors > 0 {
            s.push_str(&format!(", {} failed", self.errors));
        }
        if let Some(why) = &self.stopped {
            s.push_str(&format!(" (stopped: {why})"));
        }
        s
    }
}

/// The stereo albums the sweep asks about next, at most `limit`: those with no surround copy and no Surround
/// request (only a failed, denied or cancelled one, which may be tried again), and whose last "no" from Apple
/// Music is older than `recheck_days`. Never-asked albums come first, then the longest-asked ones; newest added
/// first among equals, so what was just downloaded is looked at soon.
pub fn sweep_candidates(db: &encore_db::Db, recheck_days: i64, limit: usize) -> Vec<String> {
    let cutoff = now() - recheck_days.max(1) as f64 * 86400.0;
    let sql = "SELECT t.album_id, MAX(t.added_at) added, c.checked_at checked
        FROM tracks t LEFT JOIN surround_checks c ON c.key = 'album:' || t.album_id
        WHERE t.album_id IS NOT NULL AND t.album_id != ''
          -- a surround copy is here already
          AND NOT EXISTS (SELECT 1 FROM surround_tracks s WHERE s.stereo_album_id = t.album_id)
          -- a Surround request is running or done (a failed one is tried again)
          AND NOT EXISTS (SELECT 1 FROM requests r WHERE r.class = 'surround'
                AND json_extract(r.detail, '$.surround_key') = 'album:' || t.album_id
                AND r.status NOT IN ('failed', 'denied', 'cancelled'))
          -- Apple said no not long ago
          AND (c.key IS NULL OR c.available = 1 OR c.checked_at < ?)
        GROUP BY t.album_id
        ORDER BY COALESCE(c.checked_at, 0), added DESC
        LIMIT ?";
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    db.query(sql, p![cutoff, limit]).unwrap_or_default().iter().map(|r| r.s("album_id")).collect()
}

/// The library sweep: looks for the Dolby Atmos version of up to `limit` stereo albums ([`sweep_candidates`])
/// and queues a Surround request for each one Apple Music has (as `user_id`; None for the scheduled task). One
/// sweep runs at a time ([`SWEEP_JOB`], with `total`, `checked`, `found`, `requested` and `current` while it runs,
/// and `result` after); a second one is refused. It stops at a sign-in or rate-limit failure, after
/// [`SWEEP_MAX_ERRORS`] failed checks in a row, or when the job's `cancel` is set.
pub async fn sweep(st: &impl Ctx, user_id: Option<i64>, limit: usize) -> anyhow::Result<SweepReport> {
    if !gamdl::atmos_ready(st) {
        anyhow::bail!("needs the gamdl plugin with a wrapper-v2 server (Settings → Plugins → gamdl)");
    }
    let recheck = st.cfg().int("surround_sweep_recheck_days");
    let albums = sweep_candidates(st.db(), recheck, limit);
    let jobs = st.jobs();
    if !jobs.start(SWEEP_JOB, json!({ "total": albums.len(), "checked": 0, "found": 0, "requested": 0, "started_at": now() })) {
        anyhow::bail!("a Dolby Atmos sweep is running already");
    }
    let mut rep = SweepReport::default();
    let mut in_a_row = 0;
    for (i, album) in albums.iter().enumerate() {
        if jobs.get(SWEEP_JOB)["cancel"] == true {
            rep.stopped = Some("cancelled".into());
            break;
        }
        let Some(ask) = album_ask(st, album) else { continue };
        jobs.set(SWEEP_JOB, json!({ "current": ask["album"] }));
        let key = format!("album:{album}");
        // a request made meanwhile (the album page's button, the automatic fetch) counts
        if open_request(st, &key).is_some() {
            continue;
        }
        if i > 0 {
            tokio::time::sleep(SWEEP_PACE).await;
        }
        rep.checked += 1;
        match check(st, &key, &ask, false).await {
            Ok(Some(found)) => {
                in_a_row = 0;
                rep.found += 1;
                match create(st, user_id, &key, &found, Some(album), &[], None) {
                    Ok(_) => rep.requested += 1,
                    Err(e) => tracing::info!("surround sweep: no request for {key}: {}", e.detail),
                }
            }
            Ok(None) => in_a_row = 0,
            Err(f) => {
                rep.errors += 1;
                in_a_row += 1;
                // these won't get better by asking about the next album
                let fatal = match f.code {
                    encore_plugins::Code::Auth => Some(format!("Apple Music sign-in: {}", f.message)),
                    encore_plugins::Code::RateLimited => Some("Apple Music rate limit, the next run goes on".to_string()),
                    _ if in_a_row >= SWEEP_MAX_ERRORS => Some(format!("{in_a_row} failed checks in a row: {}", f.message)),
                    _ => None,
                };
                if fatal.is_some() {
                    rep.stopped = fatal;
                    break;
                }
            }
        }
        jobs.set(SWEEP_JOB, json!({ "checked": rep.checked, "found": rep.found, "requested": rep.requested }));
    }
    if rep.requested > 0 {
        st.db().log("info", &format!("Dolby Atmos sweep: {}", rep.line()));
    }
    jobs.set(SWEEP_JOB, json!({ "running": false, "cancel": false, "current": null, "result": rep.line(), "finished_at": now(),
        "checked": rep.checked, "found": rep.found, "requested": rep.requested }));
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database in a fresh temporary folder (removed by the caller).
    fn db(name: &str) -> (encore_db::Db, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("encore-sweep-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (encore_db::Db::open(&dir).unwrap(), dir)
    }

    /// One stereo song of `album`, added at `added`.
    fn song(db: &encore_db::Db, album: &str, added: f64) {
        db.execute("INSERT INTO tracks (path, title, album, album_id, added_at) VALUES (?,?,?,?,?)", p![format!("/music/{album}/{added}.flac"), "T", album, album, added]).unwrap();
    }

    #[test]
    fn the_sweep_skips_albums_it_has_or_asked_about_lately() {
        let (db, dir) = db("candidates");
        for (a, added) in [("a1", 10.0), ("a2", 20.0), ("a3", 30.0), ("a4", 40.0), ("a5", 50.0), ("a6", 60.0)] {
            song(&db, a, added);
        }
        song(&db, "a1", 11.0); // two songs: still one album
        // a2 has a surround copy
        db.execute("INSERT INTO surround_tracks (path, title, stereo_album_id) VALUES ('/music/Surround/x.m4a', 'X', 'a2')", p![]).unwrap();
        // a3 is being fetched; a4's fetch failed, so it is asked again
        db.execute("INSERT INTO requests (kind, status, class, detail, created_at, updated_at) VALUES ('album', 'queued', 'surround', '{\"surround_key\":\"album:a3\"}', 1, 1)", p![]).unwrap();
        db.execute("INSERT INTO requests (kind, status, class, detail, created_at, updated_at) VALUES ('album', 'failed', 'surround', '{\"surround_key\":\"album:a4\"}', 1, 1)", p![]).unwrap();
        // Apple said no to a5 just now and to a6 long ago
        db.execute("INSERT INTO surround_checks (key, checked_at, available) VALUES ('album:a5', ?, 0)", p![now()]).unwrap();
        db.execute("INSERT INTO surround_checks (key, checked_at, available) VALUES ('album:a6', 1, 0)", p![]).unwrap();
        // never asked first (newest added first), then the oldest answer
        assert_eq!(sweep_candidates(&db, 30, 10), vec!["a4", "a1", "a6"]);
        assert_eq!(sweep_candidates(&db, 30, 2), vec!["a4", "a1"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// An Atmos album result as gamdl's search gives it.
    fn am_album(key: &str, title: &str, artist: &str, year: &str, kind: &str, tracks: i64) -> Value {
        json!({ "key": key, "title": title, "album": title, "artist": artist, "year": year, "album_kind": kind, "tracks": tracks })
    }

    /// An Atmos song result.
    fn am_song(key: &str, title: &str, artist: &str, album: &str, year: &str, duration: f64) -> Value {
        json!({ "key": key, "title": title, "artist": artist, "album": album, "year": year, "duration": duration })
    }

    #[test]
    fn an_album_takes_its_own_release_not_a_compilation_or_single() {
        let ask = json!({ "kind": "album", "artist": "Daft Punk", "album": "Random Access Memories", "track_count": 13, "year": "2013", "album_kind": "album" });
        let results = vec![
            am_album("hits", "Random Access Memories Hits", "Daft Punk", "2013", "album", 13),
            am_album("single", "Get Lucky (feat. Pharrell Williams) - Single", "Daft Punk", "2013", "single", 1),
            am_album("now", "Random Access Memories", "Various Artists", "2013", "compilation", 13),
            am_album("deluxe", "Random Access Memories (10th Anniversary Edition)", "Daft Punk", "2023", "album", 22),
            am_album("own", "Random Access Memories", "Daft Punk", "2013", "album", 13),
        ];
        assert_eq!(pick(&ask, &results).unwrap()["key"], "own");
        // without it: the 10-years-later edition is another release (year and size), so nothing
        assert_eq!(pick(&ask, &results[..4]), None);
        // a few songs short in Atmos is still the album
        assert_eq!(pick(&ask, &[am_album("short", "Random Access Memories (Dolby Atmos)", "Daft Punk", "2013", "album", 11)]).unwrap()["key"], "short");
    }

    #[test]
    fn a_single_takes_a_single_and_a_compilation_a_compilation() {
        let single = json!({ "kind": "album", "artist": "Dua Lipa", "album": "Houdini", "track_count": 1, "year": "2023", "album_kind": "single" });
        let results = vec![am_album("album", "Houdini", "Dua Lipa", "2024", "album", 11), am_album("s", "Houdini - Single", "Dua Lipa", "2023", "single", 1)];
        assert_eq!(pick(&single, &results).unwrap()["key"], "s");
        let best_of = json!({ "kind": "album", "artist": "Queen", "album": "Greatest Hits", "track_count": 17, "year": "1981", "album_kind": "compilation" });
        assert_eq!(pick(&best_of, &[am_album("gh", "Greatest Hits", "Queen", "1981", "compilation", 17)]).unwrap()["key"], "gh");
    }

    #[test]
    fn a_song_takes_the_one_on_its_own_album() {
        let ask = json!({ "kind": "track", "artist": "The Weeknd", "title": "Blinding Lights", "album": "After Hours", "length": 200.0, "year": "2020" });
        let results = vec![
            am_song("hits", "Blinding Lights", "The Weeknd", "The Highlights", "2021", 200.0),
            am_song("remix", "Blinding Lights (Remix)", "The Weeknd", "After Hours (Deluxe)", "2020", 220.0),
            am_song("cover", "Blinding Lights", "Some Band", "After Hours", "2020", 201.0),
            am_song("own", "Blinding Lights", "The Weeknd", "After Hours (Deluxe)", "2020", 201.5),
        ];
        assert_eq!(pick(&ask, &results).unwrap()["key"], "own");
        assert_eq!(pick(&ask, &results[..3]), None, "only other albums, a remix and a cover: nothing");
    }

    #[test]
    fn edition_notes_are_left_out_but_other_versions_are_not() {
        assert_eq!(core("Rumours (Super Deluxe) [2013 Remaster]"), ("rumours".into(), true));
        assert_eq!(core("Fearless (Taylor's Version)"), ("fearless taylor s version".into(), false));
        assert_eq!(core("Abbey Road (Dolby Atmos)"), ("abbey road".into(), false));
        assert_eq!(album_kind("", "", "", false, 2), "single");
        assert_eq!(album_kind("", "Album", "[\"Compilation\"]", false, 18), "compilation");
        assert_eq!(album_kind("album", "Single", "", false, 2), "album");
    }

    #[test]
    fn the_report_reads_as_one_line() {
        let rep = SweepReport { checked: 12, found: 3, requested: 2, errors: 1, stopped: Some("cancelled".into()) };
        assert_eq!(rep.line(), "12 album(s) checked, 3 in Dolby Atmos, 2 requested, 1 failed (stopped: cancelled)");
    }
}
