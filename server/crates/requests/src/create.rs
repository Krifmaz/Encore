//! New requests: the checks (a valid kind and ids, the weekly limit, already requested) and the row – or,
//! when the same song was asked for before and that request failed or was cancelled, that row again, so
//! the Requests list has one row per song.
use crate::Ctx;
use encore_config::perms::is_admin;
use encore_core::error::{err, ApiResult};
use encore_db::{now, p, Row, RowExt};
use serde::Deserialize;
use serde_json::{json, Value};

/// Discover's sources an "ext" release can come from.
const EXT_SOURCES: [&str; 5] = ["deezer", "apple", "ytm", "bandcamp", "discogs"];

/// What a person asks for (POST /api/requests).
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct NewRequest {
    /// "track", "album", "artist", "playlist" or "url"
    pub kind: String,
    /// the MusicBrainz id (recording, release group or artist)
    pub mbid: Option<String>,
    /// the artist
    pub artist: Option<String>,
    /// the song or album title
    pub title: Option<String>,
    /// the album a song is on
    pub album: Option<String>,
    /// the release year
    pub year: Option<String>,
    /// a cover image URL
    pub cover_url: Option<String>,
    /// the link, for a url request
    pub url: Option<String>,
    /// the song's length, seconds
    pub length: Option<f64>,
    /// the song's ISRC
    pub isrc: Option<String>,
    /// a song request: "song" (only it) or "album" (its whole album)
    pub album_mode: Option<String>,
    /// a release only another service has (Discover's "ext" entries)
    pub ext: Option<Value>,
    /// "Request discography": this request's release types ({"kinds": [...], "skip": [...]}), else the setting's
    pub release_filter: Option<Value>,
    /// "Request discography": exactly these releases – the boxes ticked in the dialog, as MusicBrainz release
    /// group ids and Discover "ext-…" ids – instead of what the release types pick (owned and requested ones
    /// are still left out)
    pub release_ids: Option<Vec<String>>,
}


/// A string that is set and not empty.
fn nz(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|x| !x.is_empty())
}

/// Refuse when the user's weekly request limit would be passed (admins have none).
pub fn quota(st: &impl Ctx, u: &Row, n: i64) -> ApiResult<()> {
    if is_admin(u) {
        return Ok(());
    }
    let limit = u.i("request_limit");
    if limit == 0 {
        return Ok(());
    }
    let used = st.db().one("SELECT COUNT(*) n FROM requests WHERE user_id=? AND created_at > ? AND parent_id IS NULL", p![u.i("id"), now() - 7.0 * 86400.0])?.map(|r| r.i("n")).unwrap_or(0);
    if used + n > limit {
        return Err(err(429, format!("Weekly request limit reached ({limit}). Ask an admin to raise it.")));
    }
    Ok(())
}

/// 409 when the song is already requested; the body carries the existing request's `id` and `status`, so a
/// caller (Download missing, say) can link to it instead of skipping the song.
fn dup(st: &impl Ctx, sql: &str, params: Vec<Value>) -> ApiResult<()> {
    if let Some(d) = st.db().one(sql, params)? {
        return Err(err(409, format!("Already requested (#{}, {})", d.i("id"), d.s("status"))).with(json!({ "id": d.i("id"), "status": d.s("status") })));
    }
    Ok(())
}

/// An album request already there for this album, whichever way it was made: the same release group, the same
/// Deezer album or barcode, or the same artist and title (edition notes aside: "Discovery" and "Discovery
/// (Remastered)"). MusicBrainz's answer for one song and Deezer's for another song of the same album (while
/// MusicBrainz was busy) used to make two album requests, and both downloaded.
pub fn album_request_for(st: &impl Ctx, mbid: Option<&str>, artist: &str, title: &str, ext: &Value) -> anyhow::Result<Option<i64>> {
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
    // (two MusicBrainz release groups of one name are two albums – Weezer's self-titled albums, encore_library::split –
    // so the name counts only when one of the two has no release group)
    let same_name = |row: &Row| {
        let theirs = Some(row.s("album")).filter(|a| !a.is_empty()).unwrap_or_else(|| row.s("title"));
        (mbid.is_none() || row.s("mbid").is_empty())
            && !key.is_empty() && encore_library::catalog::title_key(&theirs) == key && encore_core::text::norm(&row.s("artist")) == encore_core::text::norm(artist)
    };
    let found = rows.iter().find(|row| mbid.is_some_and(|m| row.s("mbid") == m))
        .or_else(|| rows.iter().find(|row| same_ext(row)))
        .or_else(|| rows.iter().find(|row| same_name(row)));
    Ok(found.map(|row| row.i("id")))
}

/// 409 when a running track request is for the same song by other names or ids ([`encore_library::matcher`]:
/// "Song (feat. X)" and "Song", "Don't" and "Dont", a MusicBrainz request and a .txt import line, the album and
/// single recordings of one song) – the same song and version, lengths close when both are known. Other
/// versions (live, remix, sped up …) are other songs.
fn same_song_requested(st: &impl Ctx, b: &NewRequest, artist: Option<&str>) -> ApiResult<()> {
    use encore_library::matcher::{compare, describe};
    let (Some(artist), Some(title)) = (artist, nz(&b.title)) else { return Ok(()) };
    let want = describe(title, artist, b.length, nz(&b.isrc), nz(&b.mbid));
    // only the requests whose title has the song's longest word go through the comparison
    let Some(word) = want.base.split_whitespace().filter(|w| w.chars().all(|c| c.is_alphanumeric())).max_by_key(|w| w.chars().count()) else { return Ok(()) };
    let rows = st.db().query("SELECT id, status, artist, title, mbid, detail FROM requests WHERE kind='track' AND class='stereo'
                              AND status NOT IN ('failed','denied','cancelled') AND lower(title) LIKE ? ORDER BY id LIMIT 500",
                             p![format!("%{}%", word.to_lowercase())])?;
    for r in rows {
        let d: Value = serde_json::from_str(&r.s("detail")).unwrap_or(json!({}));
        let mbid = Some(r.s("mbid")).filter(|m| !m.is_empty()).or_else(|| d["recording"].as_str().map(String::from));
        let theirs = describe(&r.s("title"), &r.s("artist"), d["length"].as_f64(), d["isrc"].as_str(), mbid.as_deref());
        if compare(&want, &theirs, true).matched {
            return Err(err(409, format!("Already requested (#{}, {})", r.i("id"), r.s("status"))).with(json!({ "id": r.i("id"), "status": r.s("status") })));
        }
    }
    Ok(())
}

/// The catalogue ids in a Discover "ext-…" album id ("ext-d302127", "ext-d2-a3": Deezer 2, Apple 3).
pub fn ext_from_id(id: &str) -> Option<Value> {
    let rest = id.strip_prefix("ext-")?;
    let mut ext = serde_json::Map::new();
    for part in rest.split('-') {
        let (k, n) = part.split_at(part.chars().next().map_or(0, |c| c.len_utf8()));
        let key = match k { "d" => "deezer", "a" => "apple", _ => continue };
        if let Ok(n) = n.parse::<i64>() {
            ext.entry(key).or_insert(json!(n));
        }
    }
    (!ext.is_empty()).then_some(Value::Object(ext))
}

/// How long what the album lookup found out holds for a song asked for again (seconds): within a day a new
/// attempt doesn't ask MusicBrainz again which album the song is on (the sync's auto-download asks again for a
/// failed song many times a day, and each time the lookup waited in MusicBrainz's line again).
pub const ALBUM_CHECK_KEEP_SECS: f64 = 86_400.0;

/// How many of the previous attempt's log lines a reused row keeps, and how long each may be.
pub const KEPT_LOG_LINES: usize = 40;
/// The longest a kept log line may be.
pub const KEPT_LOG_CHARS: usize = 500;

/// A new request row – or, when the same song was asked for before and that request failed or was
/// cancelled, that row again (reset like Retry, `detail.attempts` one higher, the old message in its log),
/// so the Requests list has one row per song. Returns the row's id.
pub fn create_request(st: &impl Ctx, u: &Row, b: &NewRequest, parent: Option<i64>, extra: Option<Value>) -> ApiResult<i64> {
    create_or_reuse(st, u, b, parent, extra, None)
}

/// The failed or cancelled top-level request for the same song (same kind and MusicBrainz id, or the same
/// artist and title when there is none), newest first.
fn failed_twin(st: &impl Ctx, b: &NewRequest, artist: Option<&str>, title: Option<&str>) -> ApiResult<Option<Row>> {
    let ended = "status IN ('failed','cancelled') AND parent_id IS NULL ORDER BY id DESC LIMIT 1";
    if let Some(m) = nz(&b.mbid) {
        return Ok(st.db().one(&format!("SELECT * FROM requests WHERE kind=? AND mbid=? AND class='stereo' AND {ended}"), p![b.kind, m])?);
    }
    if !matches!(b.kind.as_str(), "album" | "track") {
        return Ok(None);
    }
    let (Some(a), Some(t)) = (artist, title) else { return Ok(None) };
    Ok(st.db().one(&format!("SELECT * FROM requests WHERE kind=? AND mbid IS NULL AND lower(artist)=lower(?) AND lower(title)=lower(?) AND class='stereo' AND {ended}"), p![b.kind, a, t])?)
}

/// The `detail` of a request that is asked for again: the new request's detail plus the attempt count and
/// the previous attempt's log, ending with how it ended.
///
/// The attempts rule: `attempts` counts the runs at the sources that got a real answer from them – the run
/// before ended `failed` or `completed`. A run that was cancelled or taken over while it ran, a wait for
/// Soulseek or for the storage, and an import that failed with its downloaded files kept (the source did
/// its part) never count. Downloaded files that are kept go along, so asking again imports them instead of
/// downloading them again (unless the new ask is a pick of something else).
pub fn detail_for_retry(old_detail: &Value, old_status: &str, old_message: &str, mut detail: Value) -> Value {
    let attempts = old_detail["attempts"].as_u64().unwrap_or(1);
    let counted = matches!(old_status, "failed" | "completed") && !old_detail["kept"].is_object();
    let mut log: Vec<Value> = old_detail["log"].as_array().cloned().unwrap_or_default().into_iter()
        .map(|l| match l.as_str() { Some(s) if s.chars().count() > KEPT_LOG_CHARS => json!(s.chars().take(KEPT_LOG_CHARS).collect::<String>() + "…"), _ => l }).collect();
    let ended = format!("attempt {attempts} {old_status}: {}", old_message.chars().take(KEPT_LOG_CHARS).collect::<String>()).trim_end_matches([':', ' ']).to_string();
    log.push(json!(ended));
    if log.len() > KEPT_LOG_LINES {
        log.drain(..log.len() - KEPT_LOG_LINES);
    }
    detail["attempts"] = json!(if counted { attempts + 1 } else { attempts });
    detail["log"] = json!(log);
    if old_detail["kept"].is_object() && detail["pick"].is_null() {
        detail["kept"] = old_detail["kept"].clone();
    }
    // the downloads the audio check turned away stay offered (Keep anyway) until they are 7 days old
    for k in ["rejected", "rejected_seq"] {
        if !old_detail[k].is_null() && detail[k].is_null() {
            detail[k] = old_detail[k].clone();
        }
    }
    // what MusicBrainz answered about the song's album (it is on none, or the album couldn't be had) and its
    // recording hold for a day: the new attempt goes on with the song on its own at once
    let checked = old_detail["album_checked_at"].as_f64().filter(|t| now() - t < ALBUM_CHECK_KEEP_SECS);
    if checked.is_some() && old_detail["no_album"].as_bool() == Some(true) {
        for k in ["no_album", "album_checked_at", "recording"] {
            if !old_detail[k].is_null() && detail[k].is_null() {
                detail[k] = old_detail[k].clone();
            }
        }
    }
    detail
}

/// `create_request` with the row to reuse known (a manual pick for an existing request), or found among
/// the failed and cancelled requests.
pub fn create_or_reuse(st: &impl Ctx, u: &Row, b: &NewRequest, parent: Option<i64>, extra: Option<Value>, reuse: Option<i64>) -> ApiResult<i64> {
    // an "ext-…" id is a Deezer / Apple album from Discover search, not a MusicBrainz id (clients that
    // don't know ext albums send it as the mbid): request it as the ext album it is
    let fixed;
    let b = match nz(&b.mbid).filter(|m| m.starts_with("ext-")) {
        Some(m) => {
            let ext = match (b.kind.as_str(), ext_from_id(m)) {
                ("album", Some(e)) => e,
                _ => return Err(err(400, "That isn't a MusicBrainz id")),
            };
            let mut nb = b.clone();
            nb.mbid = None;
            let mut merged = b.ext.clone().filter(|e| e.is_object()).unwrap_or(json!({}));
            for (k, v) in ext.as_object().into_iter().flatten() {
                if merged[k].is_null() {
                    merged[k] = v.clone();
                }
            }
            nb.ext = Some(merged);
            fixed = nb;
            &fixed
        }
        None => b,
    };
    let has_extra = extra.is_some();
    let ext_ok = b.ext.as_ref().is_some_and(|e| EXT_SOURCES.iter().any(|k| !e[k].is_null() && e[k] != ""));
    let (artist, title) = (nz(&b.artist), nz(&b.title).or(nz(&b.album)));
    match b.kind.as_str() {
        "url" => {
            if !b.url.as_deref().is_some_and(|u| u.starts_with("http://") || u.starts_with("https://")) {
                return Err(err(400, "A valid http(s) URL is required"));
            }
        }
        // an album with no MusicBrainz id is fine when another catalogue knows it, or when a manual
        // pick (extra) says exactly what to download – a Soulseek folder, say
        "album" if nz(&b.mbid).is_none() && (ext_ok || has_extra) => {
            if artist.is_none() || title.is_none() {
                return Err(err(400, "Artist and album title are required"));
            }
            if !has_extra {
                dup(st, "SELECT id, status FROM requests WHERE kind='album' AND mbid IS NULL AND lower(artist)=lower(?) AND lower(title)=lower(?)
                         AND class='stereo' AND status NOT IN ('failed','denied','cancelled')", p![artist, title])?;
            }
        }
        "artist" | "album" if nz(&b.mbid).is_none() => return Err(err(400, "A MusicBrainz id is required")),
        "track" if nz(&b.mbid).is_none() && (artist.is_none() || nz(&b.title).is_none()) => return Err(err(400, "Track needs a MusicBrainz id or artist + title")),
        "artist" | "album" | "track" => {}
        _ => return Err(err(400, "Invalid request kind")),
    }
    if b.kind == "track" && nz(&b.mbid).is_none() && !has_extra {
        dup(st, "SELECT id, status FROM requests WHERE kind='track' AND lower(artist)=lower(?) AND lower(title)=lower(?) AND class='stereo' AND status NOT IN ('failed','denied','cancelled')",
            p![artist, b.title])?;
    }
    if let (Some(m), false) = (nz(&b.mbid), has_extra) {
        dup(st, "SELECT id, status FROM requests WHERE mbid=? AND kind=? AND class='stereo' AND status NOT IN ('failed','denied','cancelled')", p![m, b.kind])?;
    }
    // the same song or album asked for another way: by name from a .txt or playlist import and from MusicBrainz,
    // or as a Deezer album and its MusicBrainz release group – one download
    if !has_extra {
        match b.kind.as_str() {
            "track" => same_song_requested(st, b, artist)?,
            "album" => {
                let ext = b.ext.clone().unwrap_or(json!({}));
                if let (Some(a), Some(t)) = (artist, title) {
                    if let Some(id) = album_request_for(st, nz(&b.mbid), a, t, &ext).map_err(|e| err(500, e.to_string()))? {
                        dup(st, "SELECT id, status FROM requests WHERE id=?", p![id])?;
                    }
                }
            }
            _ => {}
        }
    }
    let twin = match reuse {
        Some(id) => st.db().one("SELECT * FROM requests WHERE id=?", p![id])?,
        None if parent.is_none() => failed_twin(st, b, artist, title)?,
        None => None,
    };
    let approved = is_admin(u) || u.b("auto_approve") || st.cfg().bool("auto_approve");
    let mut detail = json!({});
    if let Some(l) = b.length.filter(|l| *l > 0.0) {
        detail["length"] = json!(l);
    }
    if let Some(i) = nz(&b.isrc) {
        detail["isrc"] = json!(i);
    }
    if let Some(m) = b.album_mode.as_deref().filter(|m| *m == "song" || *m == "album") {
        detail["album_mode"] = json!(m);
    }
    if b.kind == "artist" {
        use encore_config::release_types::ReleaseFilter;
        // (the request's own boxes over the artist's own release types, else the setting's)
        if let Some(f) = b.release_filter.as_ref().filter(|f| f.is_object()) {
            detail["release_filter"] = ReleaseFilter::from_value(f, &ReleaseFilter::for_artist(st.cfg(), nz(&b.mbid).unwrap_or(""))).to_value();
        }
    }
    if let (true, Some(ids)) = (b.kind == "artist", b.release_ids.as_ref()) {
        let mut seen: Vec<&str> = vec![];
        for id in ids.iter().map(|i| i.trim()).filter(|i| !i.is_empty()) {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        if seen.is_empty() {
            return Err(err(400, "Tick at least one release"));
        }
        detail["release_ids"] = json!(seen);
    }

    if let Some(e) = &b.ext {
        let mut keep = serde_json::Map::new();
        for k in EXT_SOURCES.iter().chain(["upc"].iter()) {
            if !e[*k].is_null() && e[*k] != "" {
                keep.insert(k.to_string(), e[*k].clone());
            }
        }
        detail["ext"] = Value::Object(keep);
    }
    if let Some(Value::Object(x)) = extra {
        for (k, v) in x {
            detail[k] = v;
        }
    }
    let t = now();
    let what = title.or(artist).or(nz(&b.url)).unwrap_or("");
    if let Some(old) = twin {
        let id = old.i("id");
        let old_detail: Value = serde_json::from_str(&old.s("detail")).unwrap_or(json!({}));
        let detail = detail_for_retry(&old_detail, &old.s("status"), &old.s("message"), detail);
        st.db().execute(
            "UPDATE requests SET user_id=?, status=?, message=?, progress=0, source=NULL, detail=?, artist=COALESCE(?, artist), title=COALESCE(?, title),
             album=COALESCE(?, album), year=COALESCE(?, year), cover_url=COALESCE(?, cover_url), url=COALESCE(?, url), updated_at=? WHERE id=?",
            p![u.i("id"), if approved { "queued" } else { "pending" }, if approved { "Requested again" } else { "Waiting for admin approval" }, detail.to_string(),
               artist, title, nz(&b.album), nz(&b.year), nz(&b.cover_url), nz(&b.url), t, id])?;
        st.db().log("info", &format!("{} requested {} again (#{id}, attempt {}): {what}", u.s("username"), b.kind, detail["attempts"]));
        crate::events::emit(st, id);
        return Ok(id);
    }
    let id = st.db().execute(
        "INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, url, status, message, detail, parent_id, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        p![u.i("id"), b.kind, nz(&b.mbid), artist, title, nz(&b.album), nz(&b.year), nz(&b.cover_url), nz(&b.url),
           if approved { "queued" } else { "pending" }, if approved { "" } else { "Waiting for admin approval" }, detail.to_string(), parent, t, t])?;
    st.db().log("info", &format!("{} requested {}: {what}", u.s("username"), b.kind));
    crate::events::emit(st, id);
    Ok(id)
}
