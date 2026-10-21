//! /api/discover: search (words or whole sentences), artist and album pages, new releases.
use crate::enrich::{enrich, enrich_now};
use crate::auth::User;
use encore_db::RowExt;
use encore_core::error::{err, ApiError, ApiResult};
use crate::state::AppState;
use encore_http::mb;
use encore_library::nlsearch;
use encore_db::p;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use encore_core::ids::is_mbid;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/discover/search", get(search))
        .route("/api/discover/artist/{mbid}", get(artist))
        .route("/api/discover/artist/{mbid}/library-tracks", get(artist_library_tracks))
        .route("/api/discover/artist-lookup", get(artist_lookup))
        .route("/api/discover/album-lookup", get(album_lookup))
        .route("/api/discover/album/{rgid}", get(album))
        .route("/api/discover/new-releases", get(new_releases))
}

fn mb_err(e: anyhow::Error) -> ApiError {
    err(502, format!("MusicBrainz error: {e}"))
}

pub(crate) fn req_status(st: &AppState, ids: &[String]) -> HashMap<String, String> {
    req_info(st, ids).into_iter().map(|(k, (s, _))| (k, s)).collect()
}

/// The newest request per MusicBrainz id: (status, label as the pages show it).
pub(crate) fn req_info(st: &AppState, ids: &[String]) -> HashMap<String, (String, String)> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let sql = format!("SELECT mbid, status, message FROM requests WHERE mbid IN ({}) AND status NOT IN ('denied','cancelled') ORDER BY id", vec!["?"; ids.len()].join(","));
    st.db.query(&sql, ids.iter().map(|i| json!(i)).collect()).unwrap_or_default().into_iter()
        .map(|r| (r.s("mbid"), (r.s("status"), encore_requests::events::label(&r.s("status"), &r.s("message")).to_string()))).collect()
}

/// Each release group: its library album (by release group, else title and artist) and any request. One
/// query per 500 albums (encore_library::lookup).
pub(crate) fn annotate_albums(st: &AppState, items: &mut [Value]) {
    let ids: Vec<String> = items.iter().filter_map(|i| i["id"].as_str().map(String::from)).collect();
    let rs = req_info(st, &ids);
    let keys: Vec<(String, String, String)> = items.iter()
        .map(|i| (i["id"].as_str().unwrap_or("").to_string(), i["artist"].as_str().unwrap_or("").to_string(), i["title"].as_str().unwrap_or("").to_string())).collect();
    let libs = encore_library::lookup::album_ids_for(&st.db, &keys);
    for (i, lib) in items.iter_mut().zip(libs) {
        let id = i["id"].as_str().unwrap_or("").to_string();
        i["in_library"] = json!(lib.is_some());
        i["library_album_id"] = json!(lib);
        i["request_status"] = rs.get(&id).map(|s| json!(s.0)).unwrap_or(Value::Null);
        i["request_label"] = rs.get(&id).map(|s| json!(s.1)).unwrap_or(Value::Null);
    }
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
    #[serde(default = "all")]
    r#type: String,
}
fn all() -> String {
    "all".into()
}

async fn deezer_guesses(st: &AppState, q: &str) -> Vec<(String, String)> {
    let Ok(r) = encore_http::deezer::search(st, "track", q, 6).send().await else { return vec![] };
    let v: Value = r.json().await.unwrap_or_default();
    v["data"].as_array().cloned().unwrap_or_default().iter()
        .filter_map(|t| Some((t["title"].as_str()?.to_string(), t["artist"]["name"].as_str().unwrap_or("").to_string()))).collect()
}

async fn lyric_guesses(st: &AppState, words: &str) -> Vec<(String, String)> {
    let Ok(r) = encore_http::lrclib::search(st, words).send().await else { return vec![] };
    let v: Value = r.json().await.unwrap_or_default();
    let mut seen = HashSet::new();
    v.as_array().cloned().unwrap_or_default().iter().take(20).filter_map(|t| {
        let (n, a) = (t["trackName"].as_str()?.to_string(), t["artistName"].as_str().unwrap_or("").to_string());
        seen.insert((n.to_lowercase(), a.to_lowercase())).then_some((n, a))
    }).take(5).collect()
}

/// How a query is searched: Some((query, other reading)) through search::discover (every query but lyrics),
/// None for the lyrics path below. The other reading is the sentence without its filler and kind words
/// ("radiohead album ok computer" → "radiohead ok computer"), weighed as extra splits; "play some music by
/// daft punk" searches the artist.
fn discover_plan(q: &str, p: &nlsearch::Parsed) -> Option<(String, Option<String>)> {
    if p.lyrics.is_some() {
        return None;
    }
    let fq = encore_library::artists::fold(q);
    let words = |s: &str| s.split(' ').filter(|w| !matches!(*w, "" | "by")).map(String::from).collect::<Vec<_>>();
    let dq = if p.title.is_empty() && !p.artist.is_empty() { p.artist.clone() } else { q.to_string() };
    let fp = encore_library::artists::fold(&p.q);
    let alt = (words(&fp) != words(&fq) && !fp.is_empty() && dq == q).then(|| p.q.clone());
    Some((dq, alt))
}

async fn search(State(st): State<AppState>, User(viewer): User, Query(sq): Query<SearchQ>) -> ApiResult<Json<Value>> {
    let mut v = search_answer(State(st.clone()), User(viewer.clone()), Query(sq)).await?;
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

async fn search_answer(State(st): State<AppState>, User(_): User, Query(sq): Query<SearchQ>) -> ApiResult<Value> {
    let q = sq.q.trim();
    if q.chars().count() < 2 {
        return Ok(json!({ "artists": [], "albums": [], "tracks": [] }));
    }
    let p = nlsearch::parse(q);
    let t = sq.r#type.as_str();
    let t = if ["all", "artist", "album", "track"].contains(&t) { t } else { "all" };
    if let Some((dq, alt)) = discover_plan(q, &p) {
        // "travis scott 90210", "90210 by travis scott", "the weeknd blinding lights": the split into artist and
        // title, MusicBrainz + Deezer + Apple (see encore_library::search)
        let mut out = crate::search::discover(&st, &dq, alt.as_deref(), t).await.map_err(|e| err(502, e))?;
        if p.sentence {
            out["understood"] = json!({ "q": p.q, "title": p.title, "artist": p.artist, "kind": p.kind, "lyrics": p.lyrics });
        }
        return Ok(out);
    }
    let words = p.q.clone();
    let mut out = json!({ "artists": [], "albums": [], "tracks": [], "understood": if p.sentence {
        json!({ "q": p.q, "title": p.title, "artist": p.artist, "kind": p.kind, "lyrics": p.lyrics }) } else { Value::Null } });
    if t == "all" || t == "artist" {
        let mut arts = mb::search_artists(&st, if p.artist.is_empty() { &words } else { &p.artist }, if t == "all" { 12 } else { 30 }).await.map_err(mb_err)?;
        let have: HashSet<String> = st.db.query("SELECT DISTINCT lower(albumartist) n FROM tracks", p![])?.into_iter().map(|r| r.s("n")).collect();
        for a in arts.iter_mut() {
            a["in_library"] = json!(have.contains(&a["name"].as_str().unwrap_or("").to_lowercase()));
        }
        out["artists"] = json!(arts);
    }
    if t == "all" || t == "album" {
        let aq = if !p.title.is_empty() && !p.artist.is_empty() {
            format!("releasegroup:\"{}\" AND artist:\"{}\"", mb::esc(&p.title), mb::esc(&p.artist))
        } else {
            words.clone()
        };
        let albums = mb::search_albums(&st, &aq, if t == "all" { 18 } else { 40 }).await.map_err(mb_err)?;
        out["albums"] = st.blocking(move |st| {
            let mut albums = albums;
            annotate_albums(st, &mut albums);
            Ok(json!(albums))
        }).await?;
    }
    if t == "all" || t == "track" {
        let n = if t == "all" { 15 } else { 40 };
        let mut tracks = if !p.title.is_empty() && !p.artist.is_empty() {
            mb::search_recordings(&st, &p.title, n, Some(&p.artist)).await.map_err(mb_err)?
        } else if p.lyrics.is_some() {
            vec![]
        } else {
            mb::search_recordings(&st, &words, n, None).await.map_err(mb_err)?
        };
        if p.sentence && (p.lyrics.is_some() || tracks.len() < 3) {
            // loose wording or lyrics: Deezer / LRCLIB guess the song, MusicBrainz finds it
            let guesses = match &p.lyrics { Some(l) => lyric_guesses(&st, l).await, None => deezer_guesses(&st, q).await };
            let mut seen: HashSet<String> = tracks.iter().filter_map(|t| t["id"].as_str().map(String::from)).collect();
            let mut extra = vec![];
            for (title, artist) in guesses.into_iter().take(4) {
                if let Ok(found) = mb::search_recordings(&st, &title, 2, Some(&artist)).await {
                    if let Some(f) = found.into_iter().next() {
                        if seen.insert(f["id"].as_str().unwrap_or("").to_string()) {
                            extra.push(f);
                        }
                    }
                }
            }
            tracks = if p.lyrics.is_some() { extra.into_iter().chain(tracks).collect() } else { tracks.into_iter().chain(extra).collect() };
        }
        out["tracks"] = st.blocking(move |st| {
            let ids: Vec<String> = tracks.iter().filter_map(|t| t["id"].as_str().map(String::from)).collect();
            let rs = req_status(st, &ids);
            let keys: Vec<(String, String, String)> = tracks.iter()
                .map(|t| (t["id"].as_str().unwrap_or("").to_string(), t["artist"].as_str().unwrap_or("").to_string(), t["title"].as_str().unwrap_or("").to_string())).collect();
            let libs = encore_library::lookup::find_tracks(&st.db, &keys);
            for (t, lib) in tracks.iter_mut().zip(libs) {
                let id = t["id"].as_str().unwrap_or("").to_string();
                t["in_library"] = json!(lib.is_some());
                t["library"] = json!(lib);
                t["request_status"] = rs.get(&id).map(|s| json!(s)).unwrap_or(Value::Null);
            }
            Ok(json!(tracks))
        }).await?;
    }
    // the Top result: the best song, else album, else artist
    out["top"] = [("tracks", "song"), ("albums", "album"), ("artists", "artist")].iter()
        .find_map(|(list, kind)| out[*list].as_array().and_then(|a| a.first()).map(|it| json!({ "kind": kind, "item": it })))
        .unwrap_or(Value::Null);
    Ok(out)
}

/// An artist page: MusicBrainz's artist and releases, then (on the blocking pool) what the library and the
/// requests know about them.
async fn artist(State(st): State<AppState>, User(u): User, Path(mbid): Path<String>) -> ApiResult<Json<Value>> {
    let mut a = mb::artist(&st, &mbid).await.map_err(mb_err)?;
    st.blocking(move |st| {
        let filter = encore_config::release_types::ReleaseFilter::from_cfg(&st.cfg);
        if let Some(groups) = a["release_groups"].as_array_mut() {
            annotate_albums(st, groups);
            mark_skipped(&filter, groups);
        }
        let name = a["name"].as_str().unwrap_or("").to_string();
        let rq = req_info(st, std::slice::from_ref(&mbid)).remove(&mbid);
        a["request_status"] = rq.as_ref().map(|s| json!(s.0)).unwrap_or(Value::Null);
        a["request_label"] = rq.map(|s| json!(if s.0 == "completed" { "Discography requested".to_string() } else { s.1 })).unwrap_or(Value::Null);
        // what "Request discography" includes, so the page can mark those releases at once
        // (older apps read discography_singles; the release-type boxes replace it)
        a["discography_singles"] = json!(filter.kinds.contains(&"Single"));
        a["release_types"] = filter.options();
        a["catalog_merge"] = json!(st.cfg.bool("discover_catalog_merge"));
        // the library artist: every spelling, alias and collaboration (see encore_library::artists)
        let (acond, aparams) = st.library.artists.album_filter(&st.db, &name);
        let mut ps = p![mbid];
        ps.extend(aparams);
        // One-offs (a song or two of a compilation …) are on the page's One-offs shelf instead
        let types = st.library.types.get(&st.db);
        let albums: Vec<_> = st.db.query(&format!(
            "SELECT album_id, album, albumartist, year, COUNT(*) tracks, MAX(mb_releasegroupid) rgid FROM tracks
             WHERE mb_albumartistid=? OR {acond} GROUP BY album_id ORDER BY year DESC"), ps)?
            .into_iter().filter(|r| types.get(r.get("album_id").and_then(|v| v.as_str()).unwrap_or("")).is_none_or(|c| c.format != "one_off")).collect();
        a["library_albums"] = json!(albums);
        let (tcond, tparams) = st.library.artists.track_filter(&st.db, &name);
        let mut ps = p![mbid, mbid];
        ps.extend(tparams);
        a["library_tracks"] = json!(st.db.one(&format!(
            "SELECT COUNT(*) n FROM tracks WHERE mb_albumartistid=? OR mb_artistid=? OR {tcond}"), ps)?.map(|r| r.i("n")).unwrap_or(0));
        enrich_now(st, Some(&u), &mut a);
        Ok(Json(a))
    }).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NameQ {
    name: String,
    limit: Option<usize>,
}

/// The library's songs of an artist page (Play / Shuffle), on the blocking pool (the artist index, the query).
async fn artist_library_tracks(State(st): State<AppState>, User(u): User, Path(mbid): Path<String>, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        // every spelling, alias and collaboration of the library artist too (see encore_library::artists)
        let (cond, params) = st.library.artists.track_filter(&st.db, &q.name);
        let mut ps = p![mbid, mbid];
        ps.extend(params);
        let mut v = json!(st.db.query(&format!(
            "SELECT id, title, artist, album, album_id, format, duration FROM tracks
             WHERE mb_albumartistid=? OR mb_artistid=? OR {cond}
             ORDER BY year DESC, lower(album), disc, track LIMIT 5000"), ps)?);
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

async fn artist_lookup(State(st): State<AppState>, User(_): User, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    let name = q.name.trim();
    if name.is_empty() {
        return Err(err(400, "No artist name"));
    }
    // the library first (the artist index and a few look-ups, on the blocking pool)
    let n = name.to_string();
    if let Some(id) = st.blocking(move |st| Ok(library_artist_mbid(&st.library.artists, &st.db, &n))).await? {
        return Ok(Json(json!({ "mbid": id })));
    }
    // MusicBrainz's artist search matches names, sort names and aliases; its raw rows carry the aliases
    let d = st.mb.get(&st, "artist", &[("query", name.to_string()), ("limit", "10".to_string())]).await.map_err(mb_err)?;
    let hits = d["artists"].as_array().cloned().unwrap_or_default();
    let id = pick_artist(&hits, name).ok_or_else(|| err(404, "No MusicBrainz artist with that name"))?;
    Ok(Json(json!({ "mbid": id })))
}

#[derive(serde::Deserialize)]
struct AlbumQ {
    artist: String,
    album: String,
}

/// "Get the rest of the album" from a library album with no MusicBrainz tag: the release group that matches
/// the artist and title best, so the catalog page can show the missing songs with Request.
async fn album_lookup(State(st): State<AppState>, User(viewer): User, Query(q): Query<AlbumQ>) -> ApiResult<Json<Value>> {
    let (artist, album) = (q.artist.trim(), q.album.trim());
    if album.is_empty() {
        return Err(err(400, "No album title"));
    }
    let query = if artist.is_empty() { format!("releasegroup:\"{}\"", encore_http::mb::esc(album)) } else { format!("releasegroup:\"{}\" AND artist:\"{}\"", encore_http::mb::esc(album), encore_http::mb::esc(artist)) };
    let hits = encore_http::mb::search_albums(&st, &query, 10).await.map_err(mb_err)?;
    let key = encore_library::catalog::title_key;
    let (want_t, want_a) = (key(album), key(artist));
    // an exact title (and artist when given) first, else MusicBrainz's best score
    let hit = hits.iter().find(|h| key(h["title"].as_str().unwrap_or("")) == want_t && (want_a.is_empty() || key(h["artist"].as_str().unwrap_or("")) == want_a))
        .or_else(|| hits.first())
        .ok_or_else(|| err(404, "MusicBrainz doesn't know this album – look it up in the Downloader instead"))?;
    let mut v = json!({ "rgid": hit["id"], "title": hit["title"], "artist": hit["artist"] });
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// The MusicBrainz id the library knows for an artist name: the library artist (any spelling or alias),
/// else the album artist id of its albums, else the artist id of songs credited to that name alone.
pub(crate) fn library_artist_mbid(artists: &encore_library::artists::Artists, db: &encore_db::Db, name: &str) -> Option<String> {
    let idx = artists.index(db);
    let mut names: Vec<String> = vec![name.to_string()];
    if let Some(a) = idx.find(name) {
        if is_mbid(&a.mbid) {
            return Some(a.mbid.clone());
        }
        names.extend(a.spellings.iter().cloned());
    }
    drop(idx);
    for n in &names {
        for sql in ["SELECT mb_albumartistid id, COUNT(*) n FROM tracks WHERE lower(albumartist)=lower(?) AND length(mb_albumartistid)=36 GROUP BY mb_albumartistid ORDER BY n DESC LIMIT 1",
                    "SELECT mb_artistid id, COUNT(*) n FROM tracks WHERE lower(artist)=lower(?) AND length(mb_artistid)=36 GROUP BY mb_artistid ORDER BY n DESC LIMIT 1"] {
            if let Ok(Some(r)) = db.one(sql, p![n]) {
                if is_mbid(&r.s("id")) {
                    return Some(r.s("id"));
                }
            }
        }
    }
    None
}

/// The best MusicBrainz artist search row for a name: its name, sort name or one of its aliases is the
/// name (ignoring case, accents, "the", "&"/"and" and punctuation); the highest score wins.
pub(crate) fn pick_artist(hits: &[Value], name: &str) -> Option<String> {
    let want = encore_library::artists::key(name);
    let want_n = name_key(name);
    if want.is_empty() && want_n.is_empty() {
        return None;
    }
    let same = |s: &str| !s.trim().is_empty() && (encore_library::artists::key(s) == want || (!want_n.is_empty() && name_key(s) == want_n));
    let score = |h: &Value| h["score"].as_i64().or_else(|| h["score"].as_str().and_then(|x| x.parse().ok())).unwrap_or(0);
    let named = |h: &Value| same(h["name"].as_str().unwrap_or(""));
    let aliased = |h: &Value| same(h["sort-name"].as_str().unwrap_or(""))
        || h["aliases"].as_array().into_iter().flatten().any(|a| same(a["name"].as_str().unwrap_or("")) || same(a["sort-name"].as_str().unwrap_or("")));
    // the artist's own name beats an alias another artist also carries
    let ok = |h: &&Value| is_mbid(h["id"].as_str().unwrap_or(""));
    hits.iter().filter(ok).filter(|h| named(h)).max_by_key(|h| score(h))
        .or_else(|| hits.iter().filter(ok).filter(|h| aliased(h)).max_by_key(|h| score(h)))
        .map(|h| h["id"].as_str().unwrap_or("").to_string())
}

async fn album(State(st): State<AppState>, User(viewer): User, Path(rgid): Path<String>) -> ApiResult<Json<Value>> {
    if rgid.starts_with("ext-") {
        return Err(err(404, "This album is only on Deezer / Apple Music, not MusicBrainz – update the app to open it"));
    }
    let mut g = mb::release_group(&st, &rgid).await.map_err(mb_err)?;
    let rid = mb::best_release_for_group(&st, &rgid, None).await.map_err(mb_err)?;
    let rel = match rid { Some(r) => Some(mb::release(&st, &r).await.map_err(mb_err)?), None => None };
    // the library and the requests, on the blocking pool
    let (mut g, rel, owned) = st.blocking(move |st| {
        let (mut rel, mut owned) = (rel, 0);
        annotate_albums(st, std::slice::from_mut(&mut g));
        if let Some(r) = rel.as_mut() {
            if let Some(ts) = r["tracks"].as_array_mut() {
                annotate_album_tracks(st, ts);
                owned = ts.iter().filter(|t| t["in_library"] == true).count();
            }
        }
        Ok((g, rel, owned))
    }).await?;
    // Play / Shuffle show when any song of the album is in the library (the owned ones play in album order)
    g["owned_tracks"] = json!(owned);
    g["release"] = rel.unwrap_or(Value::Null);
    enrich(&st, Some(&viewer), &mut g).await;
    Ok(Json(g))
}

/// Settings → Release types on a list of releases: `skipped` is why one is left out ("Live", "Single" …)
/// or null, `allowed` the same as a flag, and `kind` the canonical kind the clients group it under. The
/// pages hide the skipped ones behind a "+N hidden" chip, and the Request discography dialog starts with
/// the allowed ones ticked.
pub(crate) fn mark_skipped(filter: &encore_config::release_types::ReleaseFilter, items: &mut [Value]) {
    for i in items.iter_mut() {
        let reason = filter.reason(i);
        i["skipped"] = reason.map(|r| json!(r)).unwrap_or(Value::Null);
        i["allowed"] = json!(reason.is_none());
        i["kind"] = json!(encore_config::release_types::group_kind(i));
    }
}


/// Each song of an album: its library copy (by recording, ISRC, then title + artist + length) and any request.
/// One query per 500 songs for each step (encore_library::lookup).
pub(crate) fn annotate_album_tracks(st: &AppState, ts: &mut [Value]) {
    use encore_library::lookup;
    let ids: Vec<String> = ts.iter().filter_map(|t| t["recording_id"].as_str().filter(|r| !r.is_empty()).map(String::from)).collect();
    let rs = req_info(st, &ids);
    let keys: Vec<(String, String, String)> = ts.iter()
        .map(|t| (t["recording_id"].as_str().unwrap_or("").to_string(), t["artist"].as_str().unwrap_or("").to_string(), t["title"].as_str().unwrap_or("").to_string())).collect();
    let mut libs = lookup::find_tracks(&st.db, &keys);
    // not found by recording or name: by ISRC, then name and length
    let rest: Vec<usize> = (0..ts.len()).filter(|i| libs[*i].is_none()).collect();
    let ext: Vec<(String, String, String, Option<f64>)> = rest.iter()
        .map(|i| (ts[*i]["isrc"].as_str().unwrap_or("").to_string(), keys[*i].1.clone(), keys[*i].2.clone(), ts[*i]["length"].as_f64())).collect();
    for (i, lib) in rest.into_iter().zip(lookup::library_tracks_for(&st.db, &ext)) {
        libs[i] = lib;
    }
    // songs without a request by recording: the newest by artist and title
    let pairs: Vec<(String, String)> = keys.iter().filter(|k| !rs.contains_key(&k.0)).map(|k| (k.1.clone(), k.2.clone())).collect();
    let by_name = lookup::Requests::read(&st.db, "track", &pairs, &[]);
    for ((t, lib), (rid, artist, title)) in ts.iter_mut().zip(libs).zip(keys) {
        t["in_library"] = json!(lib.is_some());
        t["library"] = json!(lib);
        let rq = rs.get(&rid).cloned().or_else(|| {
            by_name.by_name(&artist, &title, true, false).map(|(status, message)| (status.clone(), encore_requests::events::label(&status, &message).to_string()))
        });
        t["request_status"] = rq.as_ref().map(|r| json!(r.0)).unwrap_or(Value::Null);
        t["request_label"] = rq.map(|r| json!(r.1)).unwrap_or(Value::Null);
    }
}

/// Names compared by letters and digits ([`encore_core::text::norm`]), with "&" read as "and".
fn name_key(s: &str) -> String {
    encore_core::text::norm(&s.replace('&', "and"))
}

/// Recent releases (Deezer release dates) by artists in your library – nothing from outside it.
/// New releases are reused for this long; after that the last answer is still shown at once while a
/// fresh one is made in the background (60 Deezer lookups take seconds on a cold cache).
const NEW_RELEASES_FRESH: u64 = 15 * 60;

async fn new_releases(State(st): State<AppState>, User(viewer): User, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    let limit = q.limit.unwrap_or(24).min(60);
    let mut v = new_releases_answer(&st, limit).await?;
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// How many releases the apps' home pages ask for (web, desktop): the answer made at start.
const NEW_RELEASES_HOME: usize = 12;

/// Makes the home page's new releases right after a start, in the background, so the first home page
/// doesn't wait for 60 Deezer look-ups (a first look while this runs waits for it instead of making its own).
pub async fn warm_new_releases(st: &AppState) {
    if let Err(e) = new_releases_answer(st, NEW_RELEASES_HOME).await {
        tracing::debug!("new releases at start: {}", e.detail);
    }
}

async fn new_releases_answer(st: &AppState, limit: usize) -> ApiResult<Value> {
    let (fresh_key, keep_key) = (format!("new-releases:fresh:{limit}"), format!("new-releases:{limit}"));
    if let Some(v) = st.cache_get(&keep_key) {
        if st.cache_get(&fresh_key).is_none() {
            // mark it fresh first, so only one refresh runs
            st.cache_put(&fresh_key, json!(true), NEW_RELEASES_FRESH);
            let st = st.clone();
            tokio::spawn(async move {
                if let Ok(v) = compute_new_releases(&st, limit).await {
                    st.cache_put(&format!("new-releases:{limit}"), v, 7 * 86400);
                }
            });
        }
        return Ok(v);
    }
    // nothing kept yet (the first look after a start): one answer is made for everyone who asks meanwhile
    let _one = st.web.new_releases.lock().await;
    if let Some(v) = st.cache_get(&keep_key) {
        return Ok(v);
    }
    let v = compute_new_releases(st, limit).await?;
    st.cache_put(&fresh_key, json!(true), NEW_RELEASES_FRESH);
    st.cache_put(&keep_key, v.clone(), 7 * 86400);
    Ok(v)
}

async fn compute_new_releases(st: &AppState, limit: usize) -> ApiResult<Value> {
    // the library's 60 biggest artists: a GROUP BY over every song, on a read connection
    let names: Vec<String> = st.db.read(|c| c.query(
        "SELECT MAX(albumartist) n FROM tracks WHERE albumartist NOT IN ('', 'Unknown Artist', 'Various Artists')
         GROUP BY lower(albumartist) ORDER BY COUNT(*) DESC LIMIT 60", p![])).await?.into_iter().map(|r| r.s("n")).collect();
    let today = chrono_today();
    let cutoff = days_ago(120);
    let mut tasks = tokio::task::JoinSet::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(5));
    for name in names {
        let (st, sem) = (st.clone(), sem.clone());
        tasks.spawn(async move {
            let _p = sem.acquire().await;
            st.deezer_recent(&name).await.into_iter().map(|a| (name.clone(), a)).collect::<Vec<_>>()
        });
    }
    let mut all = vec![];
    while let Some(r) = tasks.join_next().await {
        all.extend(r.unwrap_or_default());
    }
    all.retain(|(_, a)| { let d = a["date"].as_str().unwrap_or(""); !d.is_empty() && d >= cutoff.as_str() && d <= today.as_str() });
    all.sort_by(|x, y| y.1["date"].as_str().cmp(&x.1["date"].as_str()));
    // which of them the library has: one query per 500 titles on the blocking pool (encore_library::lookup,
    // the same test as `has_album`)
    let keys: Vec<(String, String, String)> = all.iter().map(|(artist, a)| (String::new(), artist.clone(), a["title"].as_str().unwrap_or("").to_string())).collect();
    let owned: Vec<bool> = st.blocking(move |st| Ok(encore_library::lookup::album_ids_for(&st.db, &keys).into_iter().map(|a| a.is_some()).collect())).await?;
    let mut seen = HashSet::new();
    let mut res = vec![];
    let filter = encore_config::release_types::ReleaseFilter::from_cfg(&st.cfg);
    for ((artist, a), in_library) in all.into_iter().zip(owned) {
        let title = a["title"].as_str().unwrap_or("").to_string();
        if !seen.insert((name_key(&artist), name_key(&title))) {
            continue;
        }
        let mut item = json!({ "title": title, "artist": artist, "date": a["date"], "type": a["type"], "cover": a["cover"],
            "followed": false, "in_library": in_library, "mbid": null });
        mark_skipped(&filter, std::slice::from_mut(&mut item));
        res.push(item);
        // the limit counts what is shown with "Hide skipped types" on; the skipped ones ride along
        if res.iter().filter(|x| x["skipped"].is_null()).count() >= limit {
            break;
        }
    }
    Ok(json!({ "releases": res }))
}

fn chrono_today() -> String {
    encore_core::time::ymd(encore_db::now())
}
fn days_ago(n: i64) -> String {
    encore_core::time::ymd(encore_db::now() - (n * 86400) as f64)
}

impl AppState {
    /// An artist's Deezer albums (cached 12 h).
    pub async fn deezer_recent(&self, name: &str) -> Vec<Value> {
        let key = format!("nr:{}", name_key(name));
        if let Some(v) = self.cache_get(&key) {
            return v.as_array().cloned().unwrap_or_default();
        }
        let mut out = vec![];
        if let Ok(r) = encore_http::deezer::search(self, "artist", name, 10).send().await {
            let v: Value = r.json().await.unwrap_or_default();
            let best = v["data"].as_array().cloned().unwrap_or_default().into_iter()
                .filter(|a| name_key(a["name"].as_str().unwrap_or("")) == name_key(name)).max_by_key(|a| a["nb_fan"].as_i64().unwrap_or(0));
            if let Some(id) = best.and_then(|a| a["id"].as_i64()) {
                if let Ok(r) = encore_http::deezer::artist_albums(self, id, 300).send().await {
                    let v: Value = r.json().await.unwrap_or_default();
                    for a in v["data"].as_array().cloned().unwrap_or_default() {
                        let title = a["title"].as_str().unwrap_or("").to_string();
                        let rt = a["record_type"].as_str().unwrap_or("").to_lowercase();
                        let typ = if rt == "ep" { "EP" } else if rt == "single" { "Single" } else if rt.starts_with("compil") { "Compilation" } else { "Album" };
                        out.push(json!({ "title": title, "type": typ, "date": a["release_date"], "cover": a.get("cover_xl").or(a.get("cover_medium")) }));
                    }
                }
            }
        }
        self.cache_put(&key, json!(out), 12 * 3600);
        out
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    fn plan(q: &str) -> Option<(String, Option<String>)> {
        discover_plan(q, &nlsearch::parse(q))
    }

    #[test]
    fn every_query_but_lyrics_goes_through_discover() {
        for q in ["travis scott 90210", "90210 travis scott", "90210 by travis scott", "travis scott - 90210", "kendrick lamar swimming pools",
            "the weeknd blinding lights", "linkin park one step closer", "beyonce single ladies"] {
            let (dq, _) = plan(q).unwrap_or_else(|| panic!("{q} took the old path"));
            assert_eq!(dq, q);
        }
        // the sentence's own reading is weighed too
        assert_eq!(plan("radiohead album ok computer").unwrap().1.as_deref(), Some("radiohead ok computer"));
        assert_eq!(plan("travis scott 90210").unwrap().1, None);
        assert_eq!(plan("play some music by daft punk").unwrap().0, "daft punk");
        assert!(plan("that song that goes is this the real life").is_none());
    }
}

#[cfg(test)]
mod artist_lookup_tests {
    use super::*;
    use encore_db::Db;

    const A: &str = "aaaaaaaa-1111-2222-3333-444444444444";
    const B: &str = "bbbbbbbb-1111-2222-3333-444444444444";

    #[test]
    fn picks_by_name_then_alias() {
        let hits = vec![
            json!({ "id": B, "name": "Somebody Else", "score": 100, "aliases": [{ "name": "Beyonce Knowles" }] }),
            json!({ "id": A, "name": "Beyoncé", "sort-name": "Knowles, Beyoncé", "score": 90, "aliases": [{ "name": "Beyonce Knowles" }] }),
        ];
        assert_eq!(pick_artist(&hits, "beyonce").as_deref(), Some(A), "accents and case don't matter");
        assert_eq!(pick_artist(&hits, "Beyonce Knowles").as_deref(), Some(B), "an alias, highest score");
        assert_eq!(pick_artist(&hits, "Knowles, Beyoncé").as_deref(), Some(A), "the sort name");
        assert_eq!(pick_artist(&hits, "Nobody"), None);
        let hits = vec![json!({ "id": A, "name": "The Weeknd", "score": "100" })];
        assert_eq!(pick_artist(&hits, "Weeknd").as_deref(), Some(A), "\"the\" doesn't matter");
        let hits = vec![json!({ "id": "not-an-id", "name": "X" })];
        assert_eq!(pick_artist(&hits, "X"), None);
    }

    #[test]
    fn library_ids_from_album_artist_or_solo_songs() {
        let d = std::env::temp_dir().join(format!("encore-artist-lookup-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
        let _ = std::fs::remove_dir_all(&d);
        let db = Db::open(&d).unwrap();
        let add = |path: &str, artist: &str, aa: &str, aid: &str, aaid: &str| {
            db.execute("INSERT INTO tracks (path, album_id, mtime, size, title, artist, album, albumartist, mb_artistid, mb_albumartistid, added_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                p![path, "al1", 0, 0, path, artist, "Album", aa, aid, aaid, 0]).unwrap();
        };
        add("/m/1.flac", "Band", "Band", "", A);
        add("/m/2.flac", "Guest", "Someone", B, "");
        add("/m/3.flac", "Guest feat. Band", "Someone", &format!("{B}/{A}"), "");
        let artists = encore_library::artists::Artists::default();
        assert_eq!(library_artist_mbid(&artists, &db, "band").as_deref(), Some(A));
        assert_eq!(library_artist_mbid(&artists, &db, "Guest").as_deref(), Some(B), "a song artist with no albums of its own");
        assert_eq!(library_artist_mbid(&artists, &db, "Nobody"), None);
        let _ = std::fs::remove_dir_all(d);
    }
}
