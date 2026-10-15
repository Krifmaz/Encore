//! /api/discover: search (words or whole sentences), artist and album pages, new releases.
use crate::auth::User;
use crate::db::RowExt;
use crate::error::{err, ApiError, ApiResult};
use crate::library::has_album;
use crate::state::AppState;
use crate::{mb, nlsearch, p};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/discover/search", get(search))
        .route("/api/discover/artist/{mbid}", get(artist))
        .route("/api/discover/artist/{mbid}/library-tracks", get(artist_library_tracks))
        .route("/api/discover/artist-lookup", get(artist_lookup))
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
        .map(|r| (r.s("mbid"), (r.s("status"), crate::req_events::label(&r.s("status"), &r.s("message")).to_string()))).collect()
}

pub(crate) fn annotate_albums(st: &AppState, items: &mut [Value]) {
    let ids: Vec<String> = items.iter().filter_map(|i| i["id"].as_str().map(String::from)).collect();
    let rs = req_info(st, &ids);
    for i in items.iter_mut() {
        let id = i["id"].as_str().unwrap_or("").to_string();
        let lib = crate::library::album_id_for(&st.db, &id, i["artist"].as_str().unwrap_or(""), i["title"].as_str().unwrap_or(""));
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
    let Ok(r) = st.http.get("https://api.deezer.com/search/track").query(&[("q", q), ("limit", "6")]).send().await else { return vec![] };
    let v: Value = r.json().await.unwrap_or_default();
    v["data"].as_array().cloned().unwrap_or_default().iter()
        .filter_map(|t| Some((t["title"].as_str()?.to_string(), t["artist"]["name"].as_str().unwrap_or("").to_string()))).collect()
}

async fn lyric_guesses(st: &AppState, words: &str) -> Vec<(String, String)> {
    let Ok(r) = st.http.get("https://lrclib.net/api/search").query(&[("q", words)]).send().await else { return vec![] };
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
    let fq = crate::artists::fold(q);
    let words = |s: &str| s.split(' ').filter(|w| !matches!(*w, "" | "by")).map(String::from).collect::<Vec<_>>();
    let dq = if p.title.is_empty() && !p.artist.is_empty() { p.artist.clone() } else { q.to_string() };
    let fp = crate::artists::fold(&p.q);
    let alt = (words(&fp) != words(&fq) && !fp.is_empty() && dq == q).then(|| p.q.clone());
    Some((dq, alt))
}

async fn search(State(st): State<AppState>, User(_): User, Query(sq): Query<SearchQ>) -> ApiResult<Json<Value>> {
    let q = sq.q.trim();
    if q.chars().count() < 2 {
        return Ok(Json(json!({ "artists": [], "albums": [], "tracks": [] })));
    }
    let p = nlsearch::parse(q);
    let t = sq.r#type.as_str();
    let t = if ["all", "artist", "album", "track"].contains(&t) { t } else { "all" };
    if let Some((dq, alt)) = discover_plan(q, &p) {
        // "travis scott 90210", "90210 by travis scott", "the weeknd blinding lights": the split into artist and
        // title, MusicBrainz + Deezer + Apple (see search.rs)
        let mut out = crate::search::discover(&st, &dq, alt.as_deref(), t).await.map_err(|e| err(502, e))?;
        if p.sentence {
            out["understood"] = json!({ "q": p.q, "title": p.title, "artist": p.artist, "kind": p.kind, "lyrics": p.lyrics });
        }
        return Ok(Json(out));
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
        let mut albums = mb::search_albums(&st, &aq, if t == "all" { 18 } else { 40 }).await.map_err(mb_err)?;
        annotate_albums(&st, &mut albums);
        out["albums"] = json!(albums);
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
        let ids: Vec<String> = tracks.iter().filter_map(|t| t["id"].as_str().map(String::from)).collect();
        let rs = req_status(&st, &ids);
        for t in tracks.iter_mut() {
            let id = t["id"].as_str().unwrap_or("").to_string();
            let lib = crate::library::find_track(&st.db, &id, t["artist"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""));
            t["in_library"] = json!(lib.is_some());
            t["library"] = json!(lib);
            t["request_status"] = rs.get(&id).map(|s| json!(s)).unwrap_or(Value::Null);
        }
        out["tracks"] = json!(tracks);
    }
    // the Top result: the best song, else album, else artist
    out["top"] = [("tracks", "song"), ("albums", "album"), ("artists", "artist")].iter()
        .find_map(|(list, kind)| out[*list].as_array().and_then(|a| a.first()).map(|it| json!({ "kind": kind, "item": it })))
        .unwrap_or(Value::Null);
    Ok(Json(out))
}

async fn artist(State(st): State<AppState>, User(_u): User, Path(mbid): Path<String>) -> ApiResult<Json<Value>> {
    let mut a = mb::artist(&st, &mbid).await.map_err(mb_err)?;
    let filter = crate::release_types::ReleaseFilter::from_cfg(&st.cfg);
    if let Some(groups) = a["release_groups"].as_array_mut() {
        annotate_albums(&st, groups);
        mark_skipped(&filter, groups);
    }
    let name = a["name"].as_str().unwrap_or("").to_string();
    let rq = req_info(&st, std::slice::from_ref(&mbid)).remove(&mbid);
    a["request_status"] = rq.as_ref().map(|s| json!(s.0)).unwrap_or(Value::Null);
    a["request_label"] = rq.map(|s| json!(if s.0 == "completed" { "Discography requested".to_string() } else { s.1 })).unwrap_or(Value::Null);
    // what "Request discography" includes, so the page can mark those releases at once
    // (older apps read discography_singles; the release-type boxes replace it)
    a["discography_singles"] = json!(filter.kinds.contains(&"Single"));
    a["release_types"] = filter.options();
    a["catalog_merge"] = json!(st.cfg.bool("discover_catalog_merge"));
    // the library artist: every spelling, alias and collaboration (see artists.rs)
    let (acond, aparams) = crate::artists::album_filter(&st.db, &name);
    let mut ps = p![mbid];
    ps.extend(aparams);
    a["library_albums"] = json!(st.db.query(&format!(
        "SELECT album_id, album, albumartist, year, COUNT(*) tracks, MAX(mb_releasegroupid) rgid FROM tracks
         WHERE mb_albumartistid=? OR {acond} GROUP BY album_id ORDER BY year DESC"), ps)?);
    let (tcond, tparams) = crate::artists::track_filter(&st.db, &name);
    let mut ps = p![mbid, mbid];
    ps.extend(tparams);
    a["library_tracks"] = json!(st.db.one(&format!(
        "SELECT COUNT(*) n FROM tracks WHERE mb_albumartistid=? OR mb_artistid=? OR {tcond}"), ps)?.map(|r| r.i("n")).unwrap_or(0));
    Ok(Json(a))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NameQ {
    name: String,
    limit: Option<usize>,
}

async fn artist_library_tracks(State(st): State<AppState>, User(_): User, Path(mbid): Path<String>, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    // every spelling, alias and collaboration of the library artist too (see artists.rs)
    let (cond, params) = crate::artists::track_filter(&st.db, &q.name);
    let mut ps = p![mbid, mbid];
    ps.extend(params);
    Ok(Json(json!(st.db.query(&format!(
        "SELECT id, title, artist, album, album_id, format, duration FROM tracks
         WHERE mb_albumartistid=? OR mb_artistid=? OR {cond}
         ORDER BY year DESC, lower(album), disc, track LIMIT 5000"), ps)?)))
}

async fn artist_lookup(State(st): State<AppState>, User(_): User, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    if let Some(r) = st.db.one("SELECT MAX(mb_albumartistid) id FROM tracks WHERE lower(albumartist)=lower(?)", p![q.name])? {
        if !r.s("id").is_empty() {
            return Ok(Json(json!({ "mbid": r.s("id") })));
        }
    }
    let hits = mb::search_artists(&st, &q.name, 5).await.map_err(mb_err)?;
    let best = hits.iter().filter(|h| h["name"].as_str().unwrap_or("").to_lowercase() == q.name.to_lowercase())
        .max_by_key(|h| h["score"].as_i64().unwrap_or(0)).ok_or_else(|| err(404, "No MusicBrainz artist with that name"))?;
    Ok(Json(json!({ "mbid": best["id"] })))
}

async fn album(State(st): State<AppState>, User(_): User, Path(rgid): Path<String>) -> ApiResult<Json<Value>> {
    if rgid.starts_with("ext-") {
        return Err(err(404, "This album is only on Deezer / Apple Music, not MusicBrainz – update the app to open it"));
    }
    let mut g = mb::release_group(&st, &rgid).await.map_err(mb_err)?;
    let rid = mb::best_release_for_group(&st, &rgid, None).await.map_err(mb_err)?;
    let mut rel = match rid { Some(r) => Some(mb::release(&st, &r).await.map_err(mb_err)?), None => None };
    annotate_albums(&st, std::slice::from_mut(&mut g));
    let mut owned = 0;
    if let Some(r) = rel.as_mut() {
        if let Some(ts) = r["tracks"].as_array_mut() {
            annotate_album_tracks(&st, ts);
            owned = ts.iter().filter(|t| t["in_library"] == true).count();
        }
    }
    // Play / Shuffle show when any song of the album is in the library (the owned ones play in album order)
    g["owned_tracks"] = json!(owned);
    g["release"] = rel.unwrap_or(Value::Null);
    Ok(Json(g))
}

/// Settings → Release types on a list of releases: `skipped` is why one is left out ("Live", "Single" …)
/// or null. The pages hide the skipped ones behind a "+N hidden" chip, and "Request discography" leaves
/// them out.
pub(crate) fn mark_skipped(filter: &crate::release_types::ReleaseFilter, items: &mut [Value]) {
    for i in items.iter_mut() {
        i["skipped"] = filter.reason(i).map(|r| json!(r)).unwrap_or(Value::Null);
    }
}

/// Each song of an album: its library copy (by recording, ISRC, then title + artist + length) and any request.
pub(crate) fn annotate_album_tracks(st: &AppState, ts: &mut [Value]) {
    let ids: Vec<String> = ts.iter().filter_map(|t| t["recording_id"].as_str().filter(|r| !r.is_empty()).map(String::from)).collect();
    let rs = req_info(st, &ids);
    for t in ts.iter_mut() {
        let (rid, artist, title) = (t["recording_id"].as_str().unwrap_or("").to_string(), t["artist"].as_str().unwrap_or("").to_string(), t["title"].as_str().unwrap_or("").to_string());
        let lib = crate::library::find_track(&st.db, &rid, &artist, &title)
            .or_else(|| crate::routes::catalog::library_track_for(&st.db, t["isrc"].as_str().unwrap_or(""), &artist, &title, t["length"].as_f64()));
        t["in_library"] = json!(lib.is_some());
        t["library"] = json!(lib);
        let rq = rs.get(&rid).cloned().or_else(|| crate::routes::catalog::track_request(st, "", &artist, &title));
        t["request_status"] = rq.as_ref().map(|r| json!(r.0)).unwrap_or(Value::Null);
        t["request_label"] = rq.map(|r| json!(r.1)).unwrap_or(Value::Null);
    }
}

fn norm(s: &str) -> String {
    s.to_lowercase().replace('&', "and").chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Recent releases (Deezer release dates) by artists in your library – nothing from outside it.
async fn new_releases(State(st): State<AppState>, User(_): User, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    let limit = q.limit.unwrap_or(24).min(60);
    let names: Vec<String> = st.db.query(
        "SELECT MAX(albumartist) n FROM tracks WHERE albumartist NOT IN ('', 'Unknown Artist', 'Various Artists')
         GROUP BY lower(albumartist) ORDER BY COUNT(*) DESC LIMIT 60", p![])?.into_iter().map(|r| r.s("n")).collect();
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
    let mut seen = HashSet::new();
    let mut res = vec![];
    let filter = crate::release_types::ReleaseFilter::from_cfg(&st.cfg);
    for (artist, a) in all {
        let title = a["title"].as_str().unwrap_or("").to_string();
        if !seen.insert((norm(&artist), norm(&title))) {
            continue;
        }
        let mut item = json!({ "title": title, "artist": artist, "date": a["date"], "type": a["type"], "cover": a["cover"],
            "followed": false, "in_library": has_album(&st.db, &artist, &title), "mbid": null });
        mark_skipped(&filter, std::slice::from_mut(&mut item));
        res.push(item);
        // the limit counts what is shown with "Hide skipped types" on; the skipped ones ride along
        if res.iter().filter(|x| x["skipped"].is_null()).count() >= limit {
            break;
        }
    }
    Ok(Json(json!({ "releases": res })))
}

fn ymd(secs: i64) -> String {
    // civil date from unix days (Howard Hinnant's algorithm), no extra crate needed
    let z = secs.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}
fn chrono_today() -> String {
    ymd(crate::db::now() as i64)
}
fn days_ago(n: i64) -> String {
    ymd(crate::db::now() as i64 - n * 86400)
}

impl AppState {
    /// An artist's Deezer albums (cached 12 h).
    pub async fn deezer_recent(&self, name: &str) -> Vec<Value> {
        let key = format!("nr:{}", norm(name));
        if let Some(v) = self.cache_get(&key) {
            return v.as_array().cloned().unwrap_or_default();
        }
        let mut out = vec![];
        if let Ok(r) = self.http.get("https://api.deezer.com/search/artist").query(&[("q", name), ("limit", "10")]).send().await {
            let v: Value = r.json().await.unwrap_or_default();
            let best = v["data"].as_array().cloned().unwrap_or_default().into_iter()
                .filter(|a| norm(a["name"].as_str().unwrap_or("")) == norm(name)).max_by_key(|a| a["nb_fan"].as_i64().unwrap_or(0));
            if let Some(id) = best.and_then(|a| a["id"].as_i64()) {
                if let Ok(r) = self.http.get(format!("https://api.deezer.com/artist/{id}/albums")).query(&[("limit", "300")]).send().await {
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
