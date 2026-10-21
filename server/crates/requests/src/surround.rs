//! Fetching Dolby Atmos versions into the Surround library (0.35): whether Apple Music has one for a stereo album
//! or song (asked through the gamdl plugin, kept in `surround_checks`: a yes for a week, a no for two days), the
//! requests that download it (`class = 'surround'`, locked to the Atmos result like a hand pick), and the
//! automatic fetch after a stereo request when `surround_auto_atmos` is on. docs/contracts/surround.md.
use crate::sources::gamdl;
use crate::Ctx;
use encore_core::error::{err, ApiResult};
use encore_db::{now, p, Row, RowExt};
use serde_json::{json, Value};

const YES_FOR: f64 = 7.0 * 86400.0;
const NO_FOR: f64 = 2.0 * 86400.0;

/// The search a stereo album asks Apple Music (the SDK's request fields).
fn album_ask(st: &impl Ctx, album_id: &str) -> Option<Value> {
    let r = st.db().one(
        "SELECT MAX(albumartist) albumartist, MAX(artist) artist, MAX(album) album, COUNT(*) n FROM tracks WHERE album_id=?",
        p![album_id],
    ).ok()??;
    if r.i("n") == 0 {
        return None;
    }
    let artist = Some(r.s("albumartist")).filter(|a| !a.is_empty()).unwrap_or_else(|| r.s("artist"));
    Some(json!({ "kind": "album", "artist": artist, "album": r.s("album"), "title": r.s("album"), "track_count": r.i("n"), "limit": 5 }))
}

/// The search a stereo song asks Apple Music.
fn track_ask(st: &impl Ctx, id: i64) -> Option<Value> {
    let r = st.db().one("SELECT artist, title, album, duration, isrc FROM tracks WHERE id=?", p![id]).ok()??;
    let mut ask = json!({ "kind": "track", "artist": r.s("artist"), "title": r.s("title"), "album": r.s("album"), "limit": 5 });
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
            if (yes && age < YES_FOR) || (!yes && age < NO_FOR) {
                return Ok(if yes { serde_json::from_str(&c.s("result")).ok() } else { None });
            }
        }
    }
    let found = gamdl::atmos_for(st, ask).await?;
    let _ = st.db().execute(
        "INSERT OR REPLACE INTO surround_checks (key, checked_at, available, result) VALUES (?,?,?,?)",
        p![key, now(), found.is_some() as i64, found.as_ref().map(|f| f.to_string())],
    );
    Ok(found)
}

/// A Surround request that is not over yet for this key.
fn open_request(st: &impl Ctx, key: &str) -> Option<Row> {
    st.db().one(
        "SELECT id, status FROM requests WHERE class='surround' AND json_extract(detail, '$.surround_key')=? AND status NOT IN ('failed','denied','cancelled','completed') ORDER BY id DESC LIMIT 1",
        p![key],
    ).ok().flatten()
}

/// A Surround request locked to the Atmos result (the worker downloads exactly it, as a hand pick).
fn create(st: &impl Ctx, user_id: i64, key: &str, found: &Value, stereo_album: Option<&str>, stereo_tracks: &[i64], parent: Option<i64>) -> ApiResult<i64> {
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

fn not_ready() -> encore_core::error::ApiError {
    err(409, "Dolby Atmos versions need the gamdl plugin with a wrapper-v2 server (Settings → Plugins → gamdl)")
}

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
    create(st, user_id, &key, &found, Some(album_id), &tracks, None)
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
    create(st, user_id, &key, &found, album.as_deref(), &[track_id], None)
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
                let _ = create(&st, user, &key, &found, album.as_deref(), &tracks, Some(parent));
            }
            Ok(None) => {}
            Err(e) => tracing::info!("surround: no Atmos check for {key}: {}", e.message),
        }
    });
}
