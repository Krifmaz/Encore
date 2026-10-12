//! /api/discover extras: ListenBrainz "For you", similar artists/songs, genre pages, artist photo/bio.
use crate::auth::User;
use crate::db::RowExt;
use crate::error::{err, ApiResult};
use crate::state::AppState;
use crate::{mb, p};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;

const LB: &str = "https://api.listenbrainz.org/1";
const LB_LABS: &str = "https://labs.api.listenbrainz.org";
const LB_ALGO: &str = "session_based_days_7500_session_300_contribution_3_threshold_10_limit_100_filter_True_skip_30";
const LASTFM: &str = "https://ws.audioscrobbler.com/2.0/";
const JSPF_PL: &str = "https://musicbrainz.org/doc/jspf#playlist";
const JSPF_TR: &str = "https://musicbrainz.org/doc/jspf#track";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/discover/for-you", get(for_you))
        .route("/api/discover/made-for-you", get(made_for_you))
        .route("/api/discover/artist-picture", get(artist_picture))
        .route("/api/discover/artist/{mbid}/similar", get(artist_similar))
        .route("/api/discover/artist/{mbid}/extras", get(artist_extras))
        .route("/api/discover/similar-tracks", get(similar_tracks))
        .route("/api/discover/tags", get(tags))
        .route("/api/discover/tag/{name}", get(tag))
}

pub fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '_').collect()
}
fn last_seg(s: &str) -> String {
    s.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}
fn st_(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

async fn getj(st: &AppState, url: &str, q: &[(&str, String)]) -> anyhow::Result<Value> {
    if !url.contains("api.deezer.com") {
        return Ok(st.http.get(url).query(q).send().await?.error_for_status()?.json().await?);
    }
    // Deezer allows ~50 requests per 5 s and answers "Quota limit exceeded" (HTTP 200) beyond that:
    // space requests out, and wait + retry when it says so
    for attempt in 0..4 {
        deezer_pace().await;
        let d: Value = st.http.get(url).query(q).send().await?.error_for_status()?.json().await?;
        match d["error"]["code"].as_i64() {
            None => return Ok(d),
            Some(4) | Some(700) if attempt < 3 => tokio::time::sleep(std::time::Duration::from_millis(1500 * (attempt + 1))).await,
            Some(_) => anyhow::bail!("Deezer: {}", d["error"]["message"].as_str().unwrap_or("error")),
        }
    }
    anyhow::bail!("Deezer is busy – try again in a moment")
}

/// At most ~8 Deezer requests a second from this server.
async fn deezer_pace() {
    static NEXT: std::sync::LazyLock<tokio::sync::Mutex<tokio::time::Instant>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(tokio::time::Instant::now()));
    let mut next = NEXT.lock().await;
    let now = tokio::time::Instant::now();
    if *next > now {
        tokio::time::sleep_until(*next).await;
    }
    *next = tokio::time::Instant::now().max(*next) + std::time::Duration::from_millis(125);
}

fn library_artists(st: &AppState) -> HashSet<String> {
    st.db.query("SELECT DISTINCT lower(albumartist) n FROM tracks", p![]).unwrap_or_default().into_iter().map(|r| r.s("n")).collect()
}

/// Cached JSON producer (errors are not cached).
async fn cached<F, Fut>(st: &AppState, key: &str, ttl: u64, f: F) -> anyhow::Result<Value>
where F: FnOnce() -> Fut, Fut: std::future::Future<Output = anyhow::Result<Value>> {
    if let Some(v) = st.cache_get(key) {
        return Ok(v);
    }
    let v = f().await?;
    st.cache_put(key, v.clone(), ttl);
    Ok(v)
}

// ---- ListenBrainz "For you" -------------------------------------------------------------------

fn kind(pl: &Value) -> String {
    let patch = st_(&pl["extension"][JSPF_PL]["additional_metadata"]["algorithm_metadata"]["source_patch"]);
    if !patch.is_empty() {
        return patch;
    }
    let t = st_(&pl["title"]).to_lowercase();
    if t.contains("exploration") { "weekly-exploration" } else if t.contains("daily") { "daily-jams" } else if t.contains("weekly jams") { "weekly-jams" } else { "other" }.into()
}

fn lb_track(t: &Value) -> Value {
    let ids: Vec<String> = match &t["identifier"] { Value::Array(a) => a.iter().map(st_).collect(), Value::String(s) => vec![s.clone()], _ => vec![] };
    let rec = ids.iter().find(|i| i.contains("musicbrainz.org/recording/")).map(|i| last_seg(i));
    let ext = &t["extension"][JSPF_TR];
    let add = &ext["additional_metadata"];
    let rel = Some(last_seg(&st_(&ext["release_identifier"]))).filter(|s| !s.is_empty());
    let cover = if add["caa_release_mbid"].is_string() && !add["caa_id"].is_null() {
        Some(format!("https://coverartarchive.org/release/{}/{}-250.jpg", st_(&add["caa_release_mbid"]), add["caa_id"].to_string().trim_matches('"')))
    } else {
        rel.as_ref().map(|r| format!("https://coverartarchive.org/release/{r}/front-250"))
    };
    let artist_mbid = ext["artist_identifiers"].as_array().and_then(|a| a.first()).map(|a| last_seg(&st_(a)));
    let dur = t["duration"].as_f64().unwrap_or(0.0) / 1000.0;
    json!({ "title": st_(&t["title"]), "artist": st_(&t["creator"]), "album": st_(&t["album"]), "mbid": rec, "artist_mbid": artist_mbid,
            "release_mbid": rel, "length": if dur > 0.0 { json!(dur) } else { Value::Null }, "cover": cover })
}

pub async fn lb_playlists(st: &AppState, user: &str) -> anyhow::Result<Vec<Value>> {
    let v = cached(st, &format!("lbpl:{user}"), 3600, || async {
        let r = st.http.get(format!("{LB}/user/{user}/playlists/createdfor")).query(&[("count", "25")]).send().await?;
        if r.status().as_u16() == 404 {
            anyhow::bail!("ListenBrainz user {user} not found");
        }
        let d: Value = r.error_for_status()?.json().await?;
        let mut out = vec![];
        let mut seen = HashSet::new();
        let tag = regex::Regex::new(r"<[^>]+>").unwrap();
        for item in d["playlists"].as_array().cloned().unwrap_or_default() {
            let pl = if item["playlist"].is_object() { item["playlist"].clone() } else { item };
            let k = kind(&pl);
            if !seen.insert(k.clone()) {
                continue; // newest of each kind only
            }
            let ann: String = tag.replace_all(&st_(&pl["annotation"]), "").chars().take(400).collect();
            out.push(json!({ "id": last_seg(&st_(&pl["identifier"])), "title": st_(&pl["title"]), "kind": k, "date": st_(&pl["date"]), "annotation": ann }));
        }
        Ok(json!(out))
    }).await?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

pub async fn lb_playlist_tracks(st: &AppState, pid: &str) -> anyhow::Result<Vec<Value>> {
    let v = cached(st, &format!("lbtr:{pid}"), 6 * 3600, || async {
        let d = getj(st, &format!("{LB}/playlist/{pid}"), &[]).await?;
        Ok(json!(d["playlist"]["track"].as_array().cloned().unwrap_or_default().iter().map(lb_track).collect::<Vec<_>>()))
    }).await?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// Adds `library` (the matching library track) and `request_status` to each song.
pub fn annotate_tracks(st: &AppState, tracks: Vec<Value>) -> Vec<Value> {
    const COLS: &str = "SELECT id, title, artist, album, album_id, format, duration FROM tracks";
    tracks.into_iter().map(|mut t| {
        let (mbid, title, artist) = (st_(&t["mbid"]), st_(&t["title"]), st_(&t["artist"]));
        let mut lt = if mbid.is_empty() { None } else { st.db.one(&format!("{COLS} WHERE mb_trackid=? LIMIT 1"), p![mbid]).ok().flatten() };
        if lt.is_none() && !title.is_empty() {
            lt = st.db.one(&format!("{COLS} WHERE lower(title)=lower(?) AND (lower(artist)=lower(?) OR lower(albumartist)=lower(?)) LIMIT 1"),
                p![title, artist, artist]).ok().flatten();
        }
        let mut req = Value::Null;
        if lt.is_none() {
            req = st.db.one("SELECT status FROM requests WHERE kind='track' AND ((mbid IS NOT NULL AND mbid=?) OR (lower(artist)=lower(?) AND lower(title)=lower(?)))
                             AND status NOT IN ('failed','denied','cancelled') ORDER BY id DESC", p![if mbid.is_empty() { Value::Null } else { json!(mbid) }, artist, title])
                .ok().flatten().map(|r| json!(r.s("status"))).unwrap_or(Value::Null);
        }
        t["library"] = lt.map(|r| json!(r)).unwrap_or(Value::Null);
        t["request_status"] = req;
        t
    }).collect()
}

pub fn lb_user(st: &AppState, user_id: i64) -> Option<String> {
    let c = st.db.one("SELECT data FROM connections WHERE user_id=? AND service='listenbrainz'", p![user_id]).ok()??;
    let d: Value = serde_json::from_str(&c.s("data")).ok()?;
    d["user"].as_str().filter(|s| !s.is_empty()).map(String::from)
}

fn lastfm_user(st: &AppState, user_id: i64) -> Option<String> {
    let c = st.db.one("SELECT data FROM connections WHERE user_id=? AND service='lastfm'", p![user_id]).ok()??;
    let d: Value = serde_json::from_str(&c.s("data")).ok()?;
    d["user"].as_str().filter(|s| !s.is_empty()).map(String::from)
}

/// Last.fm has no personal recommendations API, so: your top artists/songs of the last months →
/// their similar artists/songs, weighted by how much you play them, minus what you already know.
async fn lastfm_for_you(st: &AppState, user: &str, uid: i64) -> anyhow::Result<Value> {
    cached(st, &format!("lffy:{}", user.to_lowercase()), 6 * 3600, || async {
        let top = lastfm(st, "user.gettopartists", &[("user", user.into()), ("period", "3month".into()), ("limit", "30".into())]).await?;
        let top: Vec<String> = top["topartists"]["artist"].as_array().cloned().unwrap_or_default().iter().map(|a| st_(&a["name"])).filter(|n| !n.is_empty()).collect();
        let known: HashSet<String> = top.iter().map(|n| norm(n)).collect();
        let lib = library_artists(st);
        let seeds: Vec<String> = top.iter().take(8).cloned().collect();
        let sims = futures_util::future::join_all(seeds.iter().map(|a| async move { lastfm(st, "artist.getsimilar", &[("artist", a.clone()), ("limit", "25".into()), ("autocorrect", "1".into())]).await })).await;
        let mut score: std::collections::HashMap<String, (f64, Value)> = Default::default();
        for (rank, d) in sims.into_iter().enumerate() {
            let Ok(d) = d else { continue };
            let weight = 1.0 / (1.0 + rank as f64 * 0.25);
            for a in d["similarartists"]["artist"].as_array().cloned().unwrap_or_default() {
                let name = st_(&a["name"]);
                if name.is_empty() || known.contains(&norm(&name)) {
                    continue;
                }
                let m = a["match"].as_str().and_then(|s| s.parse::<f64>().ok()).or(a["match"].as_f64()).unwrap_or(0.0);
                let e = score.entry(norm(&name)).or_insert((0.0, json!({ "name": name, "mbid": a["mbid"].as_str().filter(|s| !s.is_empty()),
                    "in_library": lib.contains(&name.to_lowercase()) })));
                e.0 += m * weight;
            }
        }
        // discovery only: artists you already have are not recommendations
        let mut ranked: Vec<(f64, Value)> = score.into_values().filter(|x| x.1["in_library"] != json!(true)).collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let artists = with_pictures(st, ranked.into_iter().take(18).map(|x| x.1).collect()).await;

        // albums: the best-known album of each recommended artist you don't have yet
        let picks: Vec<String> = artists.iter().filter(|a| a["in_library"] != json!(true)).take(12).map(|a| st_(&a["name"])).collect();
        let tops = futures_util::future::join_all(picks.iter().map(|n| async move {
            lastfm(st, "artist.gettopalbums", &[("artist", n.clone()), ("limit", "3".into()), ("autocorrect", "1".into())]).await
        })).await;
        let mut albums = vec![];
        for d in tops.into_iter().flatten() {
            let best = d["topalbums"]["album"].as_array().cloned().unwrap_or_default().into_iter()
                .find(|al| { let t = st_(&al["name"]); !t.is_empty() && t != "(null)" });
            if let Some(al) = best {
                let (title, artist) = (st_(&al["name"]), st_(&al["artist"]["name"]));
                let cover = al["image"].as_array().and_then(|i| i.iter().rev().find_map(|x| x["#text"].as_str().filter(|u| !u.is_empty()).map(String::from)));
                let owned = st.db.one("SELECT 1 x FROM tracks WHERE lower(albumartist)=lower(?) AND lower(album)=lower(?) LIMIT 1", p![artist, title]).ok().flatten().is_some();
                if !owned {
                    albums.push(json!({ "title": title, "artist": artist, "cover": cover, "in_library": false }));
                }
            }
        }

        // songs: similar to your most played tracks – last month on Last.fm, else longer, else what you
        // played in Encore, else the top songs of the artists you play most (a fresh account has no scrobbles yet)
        let mut seeds: Vec<(String, String)> = vec![];
        for period in ["1month", "3month", "overall"] {
            let tt = lastfm(st, "user.gettoptracks", &[("user", user.into()), ("period", period.into()), ("limit", "10".into())]).await.unwrap_or(json!({}));
            seeds = tt["toptracks"]["track"].as_array().cloned().unwrap_or_default().iter()
                .map(|t| (st_(&t["artist"]["name"]), st_(&t["name"]))).filter(|(a, t)| !a.is_empty() && !t.is_empty()).take(6).collect();
            if !seeds.is_empty() { break; }
        }
        if seeds.is_empty() {
            seeds = st.db.query("SELECT artist, title, COUNT(*) n FROM plays WHERE user_id=? AND artist<>'' AND title<>'' AND played_at > ? GROUP BY lower(artist), lower(title) ORDER BY n DESC LIMIT 6",
                p![uid, crate::db::now() - 90.0 * 86400.0]).unwrap_or_default().into_iter().map(|r| (r.s("artist"), r.s("title"))).collect();
        }
        if seeds.is_empty() {
            for n in top.iter().take(3).cloned() {
                if let Ok(d) = lastfm(st, "artist.gettoptracks", &[("artist", n.clone()), ("limit", "2".into()), ("autocorrect", "1".into())]).await {
                    seeds.extend(d["toptracks"]["track"].as_array().cloned().unwrap_or_default().iter().map(|t| (n.clone(), st_(&t["name"]))).filter(|(_, t)| !t.is_empty()));
                }
            }
        }
        let seen: HashSet<String> = seeds.iter().map(|(a, t)| format!("{}|{}", norm(a), norm(t))).collect();
        let sims = futures_util::future::join_all(seeds.iter().map(|(a, t)| async move { lastfm(st, "track.getsimilar", &[("artist", a.clone()), ("track", t.clone()), ("limit", "12".into()), ("autocorrect", "1".into())]).await })).await;
        let mut tracks = vec![];
        let mut have = seen.clone();
        // take turns between seeds so one song doesn't fill the list
        let lists: Vec<Vec<Value>> = sims.into_iter().map(|d| d.ok().and_then(|d| d["similartracks"]["track"].as_array().cloned()).unwrap_or_default()).collect();
        for i in 0..12 {
            for l in &lists {
                if let Some(t) = l.get(i) {
                    let (artist, title) = (st_(&t["artist"]["name"]), st_(&t["name"]));
                    if title.is_empty() || !have.insert(format!("{}|{}", norm(&artist), norm(&title))) {
                        continue;
                    }
                    // discovery only: skip what the library already has
                    if st.db.one("SELECT 1 x FROM tracks WHERE lower(artist)=lower(?) AND lower(title)=lower(?) LIMIT 1", p![artist, title]).ok().flatten().is_some() {
                        continue;
                    }
                    tracks.push(json!({ "title": title, "artist": artist, "album": "", "mbid": t["mbid"].as_str().filter(|s| !s.is_empty()),
                        "length": t["duration"].as_f64().filter(|d| *d > 0.0) }));
                }
            }
        }
        tracks.truncate(30);
        Ok(json!({ "user": user, "artists": artists, "albums": albums, "tracks": tracks }))
    }).await
}

/// ListenBrainz's own playlists for this user (Daily Jams, Weekly Jams, Weekly Exploration), for the home page.
async fn made_for_you(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let Some(name) = lb_user(&st, u.i("id")) else { return Ok(Json(json!({ "connected": false, "playlists": [] }))) };
    let mut out = vec![];
    let mut error = Value::Null;
    match lb_playlists(&st, &name).await {
        Ok(mut pls) => {
            let order = |k: &str| match k { "daily-jams" => 0, "weekly-jams" => 1, "weekly-exploration" => 2, _ => 9 };
            pls.sort_by_key(|p| order(p["kind"].as_str().unwrap_or("")));
            let results = futures_util::future::join_all(pls.iter().map(|p| lb_playlist_tracks(&st, p["id"].as_str().unwrap_or("")))).await;
            for (mut p, tr) in pls.into_iter().zip(results) {
                let Ok(tr) = tr else { continue };
                let tr = annotate_tracks(&st, tr);
                let owned = tr.iter().filter(|t| !t["library"].is_null()).count();
                // up to four distinct covers for the tile – our own for owned songs, Cover Art Archive otherwise
                let mut covers: Vec<String> = vec![];
                for t in &tr {
                    let own = st_(&t["library"]["album_id"]);
                    let c = if !own.is_empty() { format!("/api/library/albums/{own}/cover") } else { st_(&t["cover"]) };
                    if !c.is_empty() && !covers.contains(&c) {
                        covers.push(c);
                    }
                    if covers.len() == 4 { break; }
                }
                p["owned"] = json!(owned);
                p["total"] = json!(tr.len());
                p["covers"] = json!(covers);
                p["tracks"] = json!(tr);
                out.push(p);
            }
        }
        Err(e) => error = json!(format!("ListenBrainz: {e}")),
    }
    Ok(Json(json!({ "connected": true, "user": name, "playlists": out, "error": error })))
}

async fn for_you(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let lb = lb_user(&st, u.i("id"));
    let lf = lastfm_user(&st, u.i("id")).filter(|_| !st.cfg.str("lastfm_api_key").is_empty());
    if lb.is_none() && lf.is_none() {
        return Ok(Json(json!({ "connected": false, "playlists": [] })));
    }
    let mut out = vec![];
    let mut errors = vec![];
    if let Some(name) = &lb {
        match lb_playlists(&st, name).await {
            Ok(mut pls) => {
                // For you is discovery: the Jams lists are songs you already love, so they stay on ListenBrainz
                pls.retain(|p| !matches!(p["kind"].as_str().unwrap_or(""), "daily-jams" | "weekly-jams"));
                let results = futures_util::future::join_all(pls.iter().map(|p| lb_playlist_tracks(&st, p["id"].as_str().unwrap_or("")))).await;
                for (mut p, tr) in pls.into_iter().zip(results) {
                    if let Ok(tr) = tr {
                        let tr: Vec<Value> = annotate_tracks(&st, tr).into_iter().filter(|t| t["library"].is_null()).collect();
                        if !tr.is_empty() {
                            p["tracks"] = json!(tr);
                            out.push(p);
                        }
                    }
                }
            }
            Err(e) => errors.push(format!("ListenBrainz: {e}")),
        }
    }
    let mut lastfm = Value::Null;
    if let Some(name) = &lf {
        match lastfm_for_you(&st, name, u.i("id")).await {
            Ok(mut v) => {
                let tr: Vec<Value> = v["tracks"].as_array().cloned().unwrap_or_default();
                v["tracks"] = json!(annotate_tracks(&st, tr).into_iter().filter(|t| t["library"].is_null()).collect::<Vec<_>>());
                lastfm = v;
            }
            Err(e) => errors.push(format!("Last.fm: {e}")),
        }
    }
    Ok(Json(json!({ "connected": true, "user": lb, "playlists": out, "lastfm": lastfm, "errors": errors })))
}

// ---- similar artists / songs ------------------------------------------------------------------

async fn lastfm(st: &AppState, method: &str, params: &[(&str, String)]) -> anyhow::Result<Value> {
    let key = st.cfg.str("lastfm_api_key");
    if key.is_empty() {
        anyhow::bail!("no Last.fm key");
    }
    let mut q: Vec<(&str, String)> = vec![("method", method.into()), ("api_key", key), ("format", "json".into())];
    q.extend(params.iter().cloned());
    let d: Value = st.http.get(LASTFM).query(&q).send().await?.json().await?;
    if !d["error"].is_null() {
        anyhow::bail!("{}", d["message"].as_str().map(String::from).unwrap_or(format!("Last.fm error {}", d["error"])));
    }
    Ok(d)
}

/// The Deezer artist with exactly this name and the most fans.
async fn deezer_artist(st: &AppState, name: &str) -> anyhow::Result<Option<Value>> {
    let d = getj(st, "https://api.deezer.com/search/artist", &[("q", name.into()), ("limit", "5".into())]).await?;
    Ok(d["data"].as_array().cloned().unwrap_or_default().into_iter()
        .filter(|a| norm(a["name"].as_str().unwrap_or("")) == norm(name)).max_by_key(|a| a["nb_fan"].as_i64().unwrap_or(0)))
}

/// An artist's photo from Deezer. Found: kept a week; not on Deezer: asked again tomorrow;
/// Deezer busy or down: not remembered at all, so the next view tries again.
async fn deezer_picture(st: &AppState, name: &str) -> Value {
    let key = format!("dzpic:{}", norm(name));
    if let Some(v) = st.cache_get(&key) {
        return v;
    }
    match deezer_artist(st, name).await {
        Ok(a) => {
            let pic = a.and_then(|a| a["picture_medium"].as_str().map(String::from)).filter(|p| !p.contains("/artist//"));
            let v = pic.map(|p| json!(p)).unwrap_or(Value::Null);
            st.cache_put(&key, v.clone(), if v.is_null() { 86400 } else { 7 * 86400 });
            v
        }
        Err(_) => Value::Null,
    }
}

/// Fill in artist photos without making the page wait: known ones directly, the rest as a link the
/// browser loads on its own (/api/discover/artist-picture), so cards appear at once and photos follow.
async fn with_pictures(st: &AppState, items: Vec<Value>) -> Vec<Value> {
    items.into_iter().map(|mut a| {
        if a["image"].as_str().unwrap_or("").is_empty() {
            let name = a["name"].as_str().unwrap_or("").to_string();
            a["image"] = match st.cache_get(&format!("dzpic:{}", norm(&name))) {
                Some(v) => v,
                None => json!(format!("/api/discover/artist-picture?name={}", urlencoding::encode(&name))),
            };
        }
        a
    }).collect()
}

#[derive(Deserialize)]
struct PictureQ {
    name: String,
}

/// The photo for one artist (a redirect to Deezer's image), or 404 – the page then shows the initial.
async fn artist_picture(State(st): State<AppState>, User(_): User, Query(q): Query<PictureQ>) -> axum::response::Response {
    use axum::response::IntoResponse;
    match deezer_picture(&st, &q.name).await.as_str() {
        Some(url) => ([(axum::http::header::CACHE_CONTROL, "private, max-age=86400")], axum::response::Redirect::temporary(url)).into_response(),
        None => (axum::http::StatusCode::NOT_FOUND, [(axum::http::header::CACHE_CONTROL, "no-store")]).into_response(),
    }
}

fn mark_library(st: &AppState, mut res: Value) -> Value {
    let have = library_artists(st);
    if let Some(arr) = res["artists"].as_array_mut() {
        for a in arr.iter_mut() {
            a["in_library"] = json!(have.contains(&a["name"].as_str().unwrap_or("").to_lowercase()));
        }
    }
    res
}

pub async fn similar_artists(st: &AppState, mbid: &str, name: &str, limit: usize) -> Value {
    let res = cached(st, &format!("sim:{}", if mbid.is_empty() { norm(name) } else { mbid.into() }), 86400, || async {
        let lim = limit.to_string();
        // Last.fm
        if !st.cfg.str("lastfm_api_key").is_empty() {
            let mut d = if mbid.is_empty() { Err(anyhow::anyhow!("")) } else {
                lastfm(st, "artist.getsimilar", &[("mbid", mbid.into()), ("limit", lim.clone()), ("autocorrect", "1".into())]).await
            };
            if d.is_err() {
                d = lastfm(st, "artist.getsimilar", &[("artist", name.into()), ("limit", lim.clone()), ("autocorrect", "1".into())]).await;
            }
            if let Ok(d) = d {
                let items: Vec<Value> = d["similarartists"]["artist"].as_array().cloned().unwrap_or_default().iter().map(|a| json!({
                    "name": st_(&a["name"]), "mbid": a["mbid"].as_str().filter(|s| !s.is_empty()),
                    "score": a["match"].as_str().and_then(|s| s.parse::<f64>().ok()).or(a["match"].as_f64()).unwrap_or(0.0) })).collect();
                if !items.is_empty() {
                    return Ok(json!({ "source": "Last.fm", "artists": with_pictures(st, items.into_iter().take(limit).collect()).await }));
                }
            }
        }
        // ListenBrainz
        if !mbid.is_empty() {
            if let Ok(d) = getj(st, &format!("{LB_LABS}/similar-artists/json"), &[("algorithm", LB_ALGO.into()), ("artist_mbids", mbid.into())]).await {
                let mut rows = vec![];
                let mut stack = vec![d];
                while let Some(x) = stack.pop() {
                    match x {
                        Value::Array(a) => stack.extend(a.into_iter().rev()),
                        Value::Object(_) if x["artist_mbid"].is_string() && x["name"].is_string() => rows.push(x),
                        _ => {}
                    }
                }
                let items: Vec<Value> = rows.into_iter().filter(|x| x["artist_mbid"] != mbid).take(limit)
                    .map(|x| json!({ "name": x["name"], "mbid": x["artist_mbid"], "score": x.get("score").cloned().unwrap_or(json!(0)) })).collect();
                if !items.is_empty() {
                    return Ok(json!({ "source": "ListenBrainz", "artists": with_pictures(st, items).await }));
                }
            }
        }
        // Deezer
        if let Ok(Some(a)) = deezer_artist(st, name).await {
            let id = a["id"].as_i64().unwrap_or(0);
            if let Ok(d) = getj(st, &format!("https://api.deezer.com/artist/{id}/related"), &[("limit", lim.clone())]).await {
                let items: Vec<Value> = d["data"].as_array().cloned().unwrap_or_default().iter().take(limit).map(|a| json!({
                    "name": st_(&a["name"]), "mbid": null, "image": a["picture_medium"], "score": a["nb_fan"].as_i64().unwrap_or(0) })).collect();
                if !items.is_empty() {
                    return Ok(json!({ "source": "Deezer", "artists": with_pictures(st, items).await }));
                }
            }
        }
        Ok(json!({ "source": null, "artists": [] }))
    }).await.unwrap_or(json!({ "source": null, "artists": [] }));
    mark_library(st, res)
}

async fn artist_similar(State(st): State<AppState>, User(_): User, Path(mbid): Path<String>) -> ApiResult<Json<Value>> {
    let a = mb::artist(&st, &mbid).await.map_err(|e| err(502, format!("MusicBrainz error: {e}")))?;
    Ok(Json(similar_artists(&st, &mbid, a["name"].as_str().unwrap_or(""), 18).await))
}

#[derive(Deserialize)]
struct SimTrQ {
    artist: String,
    title: String,
    mbid: Option<String>,
}

/// "More like this" for a song: Last.fm track.getSimilar, else a Deezer artist radio.
async fn similar_tracks(State(st): State<AppState>, User(_): User, Query(q): Query<SimTrQ>) -> ApiResult<Json<Value>> {
    let mbid = q.mbid.clone().filter(|m| !m.is_empty());
    let key = format!("simtr:{}", mbid.clone().unwrap_or_else(|| norm(&format!("{}{}", q.artist, q.title))));
    let (artist, title) = (q.artist.clone(), q.title.clone());
    let res = cached(&st, &key, 86400, || async {
        if !st.cfg.str("lastfm_api_key").is_empty() {
            let mut d = match &mbid {
                Some(m) => lastfm(&st, "track.getsimilar", &[("mbid", m.clone()), ("limit", "25".into()), ("autocorrect", "1".into())]).await,
                None => Err(anyhow::anyhow!("")),
            };
            if d.is_err() {
                d = lastfm(&st, "track.getsimilar", &[("artist", artist.clone()), ("track", title.clone()), ("limit", "25".into()), ("autocorrect", "1".into())]).await;
            }
            if let Ok(d) = d {
                let items: Vec<Value> = d["similartracks"]["track"].as_array().cloned().unwrap_or_default().iter().map(|t| json!({
                    "title": st_(&t["name"]), "artist": st_(&t["artist"]["name"]), "mbid": t["mbid"].as_str().filter(|s| !s.is_empty()),
                    "length": t["duration"].as_i64().filter(|d| *d > 0), "album": "", "cover": null })).collect();
                if !items.is_empty() {
                    return Ok(json!({ "source": "Last.fm", "tracks": items }));
                }
            }
        }
        let Some(a) = deezer_artist(&st, &artist).await? else { return Ok(json!({ "source": null, "tracks": [] })) };
        let d = getj(&st, &format!("https://api.deezer.com/artist/{}/radio", a["id"]), &[("limit", "25".into())]).await?;
        let tracks: Vec<Value> = d["data"].as_array().cloned().unwrap_or_default().iter().map(|t| json!({
            "title": t["title_short"].as_str().or(t["title"].as_str()).unwrap_or(""), "artist": st_(&t["artist"]["name"]), "mbid": null,
            "length": t["duration"], "album": st_(&t["album"]["title"]), "cover": t["album"]["cover_medium"], "isrc": t["isrc"] })).collect();
        Ok(json!({ "source": "Deezer radio", "tracks": tracks }))
    }).await.map_err(|e| err(502, e.to_string()))?;
    let tracks = annotate_tracks(&st, res["tracks"].as_array().cloned().unwrap_or_default());
    Ok(Json(json!({ "source": res["source"], "tracks": tracks })))
}

// ---- genres / tags ----------------------------------------------------------------------------

const FALLBACK_TAGS: [&str; 22] = ["rock", "pop", "electronic", "hip hop", "jazz", "metal", "indie", "folk", "punk", "soul", "r&b", "ambient",
    "house", "techno", "classical", "country", "blues", "reggae", "experimental", "shoegaze", "drum and bass", "dubstep"];

async fn tags(State(st): State<AppState>, User(_): User) -> ApiResult<Json<Value>> {
    let top = cached(&st, "toptags", 7 * 86400, || async {
        if let Ok(d) = lastfm(&st, "tag.getTopTags", &[]).await {
            let t: Vec<String> = d["toptags"]["tag"].as_array().cloned().unwrap_or_default().iter().map(|t| st_(&t["name"])).take(40).collect();
            if !t.is_empty() {
                return Ok(json!(t));
            }
        }
        Ok(json!(FALLBACK_TAGS))
    }).await.unwrap_or(json!(FALLBACK_TAGS));
    let lib: Vec<String> = st.db.query("SELECT lower(genre) g, COUNT(*) n FROM tracks WHERE genre IS NOT NULL AND genre!='' GROUP BY lower(genre) ORDER BY n DESC LIMIT 30", p![])?
        .into_iter().map(|r| r.s("g")).collect();
    Ok(Json(json!({ "top": top, "library": lib })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PageQ {
    page: Option<i64>,
}

async fn tag(State(st): State<AppState>, User(_): User, Path(name): Path<String>, Query(q): Query<PageQ>) -> ApiResult<Json<Value>> {
    let page = q.page.unwrap_or(1).max(1);
    let res = cached(&st, &format!("tag:{}:{page}", name.to_lowercase()), 86400, || async {
        if !st.cfg.str("lastfm_api_key").is_empty() {
            if let Ok(d) = lastfm(&st, "tag.gettopartists", &[("tag", name.clone()), ("limit", "48".into()), ("page", page.to_string())]).await {
                let arts: Vec<Value> = d["topartists"]["artist"].as_array().cloned().unwrap_or_default().iter()
                    .map(|a| json!({ "name": st_(&a["name"]), "mbid": a["mbid"].as_str().filter(|s| !s.is_empty()) })).collect();
                if !arts.is_empty() {
                    let summary = lastfm(&st, "tag.getinfo", &[("tag", name.clone())]).await.ok()
                        .map(|i| st_(&i["tag"]["wiki"]["summary"])).unwrap_or_default();
                    let summary = regex::Regex::new(r"(?s)<a [^>]*>.*?</a>\.?").unwrap().replace_all(&summary, "").trim().to_string();
                    return Ok(json!({ "source": "Last.fm", "summary": summary, "artists": with_pictures(&st, arts).await }));
                }
            }
        }
        let d = st.mb.get(&st, "artist", &[("query", format!("tag:\"{}\"", mb::esc(&name))), ("limit", "48".into()), ("offset", ((page - 1) * 48).to_string())]).await?;
        let arts: Vec<Value> = d["artists"].as_array().cloned().unwrap_or_default().iter().map(|a| json!({ "name": a["name"], "mbid": a["id"] })).collect();
        Ok(json!({ "source": "MusicBrainz", "summary": "", "artists": with_pictures(&st, arts).await }))
    }).await.map_err(|e| err(502, e.to_string()))?;
    let mut res = mark_library(&st, res);
    res["tag"] = json!(name);
    Ok(Json(res))
}

// ---- artist page extras: photo, Wikipedia bio, popular tracks ------------------------------------

async fn deezer_extras(st: &AppState, name: &str) -> anyhow::Result<Value> {
    let mut out = json!({});
    let Some(a) = deezer_artist(st, name).await? else { return Ok(out) };
    if let Some(p) = a["picture_xl"].as_str().filter(|p| !p.contains("/artist//")) {
        out["image"] = json!(p);
    }
    out["fans"] = a["nb_fan"].clone();
    let top = getj(st, &format!("https://api.deezer.com/artist/{}/top", a["id"]), &[("limit", "10".into())]).await?;
    out["top"] = json!(top["data"].as_array().cloned().unwrap_or_default().iter().map(|t| json!({
        "title": t["title_short"].as_str().or(t["title"].as_str()).unwrap_or(""), "version": st_(&t["title_version"]),
        "album": st_(&t["album"]["title"]), "cover": t["album"]["cover_medium"], "duration": t["duration"], "isrc": t["isrc"], "rank": t["rank"] })).collect::<Vec<_>>());
    Ok(out)
}

async fn wikipedia(st: &AppState, links: &Value) -> anyhow::Result<Value> {
    let wp = st_(&links["wikipedia"]);
    let mut title = None;
    if let Some((_, t)) = wp.split_once("en.wikipedia.org/wiki/") {
        title = Some(urlencoding::decode(t)?.into_owned());
    } else if let Some(wd) = links["wikidata"].as_str() {
        let qid = last_seg(wd);
        let d = getj(st, &format!("https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"), &[]).await?;
        title = d["entities"][&qid]["sitelinks"]["enwiki"]["title"].as_str().map(String::from);
    }
    let Some(title) = title else { return Ok(json!({})) };
    let s = getj(st, &format!("https://en.wikipedia.org/api/rest_v1/page/summary/{}", urlencoding::encode(&title.replace(' ', "_"))), &[]).await?;
    let mut out = json!({ "bio": st_(&s["extract"]), "bio_url": s["content_urls"]["desktop"]["page"] });
    if let Some(src) = s["originalimage"]["source"].as_str().or(s["thumbnail"]["source"].as_str()) {
        out["wiki_image"] = json!(src);
    }
    Ok(out)
}

async fn artist_extras(State(st): State<AppState>, User(_): User, Path(mbid): Path<String>) -> ApiResult<Json<Value>> {
    let a = mb::artist(&st, &mbid).await.map_err(|e| err(502, format!("MusicBrainz error: {e}")))?;
    let name = st_(&a["name"]);
    let data = cached(&st, &format!("extras:{mbid}"), 86400, || async {
        let mut d = json!({ "image": null, "bio": "", "bio_url": null, "top": [] });
        for part in [deezer_extras(&st, &name).await, wikipedia(&st, &a["links"]).await].into_iter().flatten() {
            if let Value::Object(m) = part {
                for (k, v) in m {
                    d[k] = v;
                }
            }
        }
        if d["image"].is_null() {
            d["image"] = d["wiki_image"].clone();
        }
        d.as_object_mut().unwrap().remove("wiki_image");
        Ok(d)
    }).await.unwrap_or_default();
    let mut data = data;
    if let Some(top) = data["top"].as_array_mut() {
        for t in top.iter_mut() {
            t["library"] = st.db.one("SELECT id, title, artist, album, album_id, format, duration FROM tracks
                WHERE lower(title)=lower(?) AND (lower(artist)=lower(?) OR lower(albumartist)=lower(?))
                ORDER BY CASE WHEN format IN ('flac','alac','wav','aiff') THEN 0 ELSE 1 END LIMIT 1", p![st_(&t["title"]), name, name])?
                .map(|r| json!(r)).unwrap_or(Value::Null);
        }
    }
    Ok(Json(data))
}
