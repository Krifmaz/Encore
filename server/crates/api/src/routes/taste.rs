//! /api/taste: the recommendations made on this server (encore_library::taste) – smart shuffle, the "For you"
//! mixes, genre mixes and "More like this" from the library – and what feeds them besides plays and Liked Songs:
//! dislikes (a song or a whole artist, never queued again) and skips (Next before a play counted).
//!
//! The model's inputs are read here: the library as a [`Catalog`] (made again when the library changes), the
//! server's co-listening as a [`CoListen`] (made again every few minutes, from every listener's plays) and one
//! listener's [`Signals`] (on each call, a few indexed reads). All of it stays on the server: no outside service
//! is asked anything.
use crate::auth::User;
use crate::enrich::enrich;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use encore_core::error::{err, ApiResult};
use encore_db::{now, p, RowExt};
use encore_library::taste::{self, Catalog, CoListen, Profile, Rng, Scope, Signals, SongIn};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The song columns a recommendation answer has (what the players queue).
const SONG_COLS: &str = "id, title, artist, album, album_id, format, bitrate, duration, genre, year";
/// Co-listening is made again at most this often (it reads every listener's plays of the last year).
const CO_LISTEN_EVERY: Duration = Duration::from_secs(10 * 60);
/// The plays co-listening reads: the last year's, at most this many of the newest.
const CO_LISTEN_PLAYS: i64 = 300_000;
/// The plays of one listener the profile reads: the last 400 days', at most this many of the newest.
const PROFILE_PLAYS: i64 = 20_000;
/// Most songs one recommendation call returns.
const MAX_SONGS: usize = 200;
/// Most songs a call may ask to leave out (the songs a long session already played).
const MAX_EXCLUDE: usize = 20_000;

/// What the recommendations keep between calls (`Web::taste`): the catalog for one state of the library, and the
/// co-listening with the time it was made.
#[derive(Default)]
pub(crate) struct TasteCache {
    /// the catalog and the library version it was made at
    catalog: Option<(u64, Arc<Catalog>)>,
    /// the co-listening, the catalog it was made over and when
    co: Option<(Arc<Catalog>, Instant, Arc<CoListen>)>,
}

/// The taste routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/taste/shuffle", post(shuffle))
        .route("/api/taste/mixes", get(mixes))
        .route("/api/taste/mix", post(mix))
        .route("/api/taste/like-this/{id}", get(like_this))
        .route("/api/taste/genres", get(genres))
        .route("/api/taste/profile", get(profile))
        .route("/api/taste/skip", post(skip))
        .route("/api/taste/listened", post(listened))
        .route("/api/taste/dislikes", get(dislikes))
        .route("/api/taste/dislikes/tracks/{id}", put(dislike_track).delete(undislike_track))
        .route("/api/taste/dislikes/artists/{name}", put(dislike_artist).delete(undislike_artist))
}

// ---- the model's inputs -----------------------------------------------------------------------------

/// The catalog of the library as it is now: made on a blocking thread when the library changed since the last one.
fn catalog(st: &AppState) -> ApiResult<Arc<Catalog>> {
    let version = st.db.library_version();
    if let Some((v, c)) = st.web.taste.lock().unwrap().catalog.as_ref() {
        if *v == version {
            return Ok(c.clone());
        }
    }
    // each song once (not the copies of a song, encore_library::twins), with every artist it credits
    let rows = st.db.query(&format!("SELECT id, artist, artist_credit, album_id, genre, year, duration FROM tracks WHERE {}", encore_library::twins::SHOWN), p![])?;
    let songs = rows.into_iter().map(|r| {
        let mut artists: Vec<String> = serde_json::from_str::<Value>(&r.s("artist_credit")).ok()
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|c| c["name"].as_str().map(str::to_string)).collect()))
            .unwrap_or_default();
        if artists.is_empty() {
            artists.push(r.s("artist"));
        }
        SongIn { id: r.i("id"), artists, album_id: r.s("album_id"), genre: r.s("genre"), year: r.s("year"), duration: r.f("duration") }
    }).collect();
    let c = Arc::new(Catalog::build(songs));
    st.web.taste.lock().unwrap().catalog = Some((version, c.clone()));
    Ok(c)
}

/// The server's co-listening over this catalog: made again when the catalog changed or it is older than
/// [`CO_LISTEN_EVERY`].
fn co_listen(st: &AppState, cat: &Arc<Catalog>) -> ApiResult<Arc<CoListen>> {
    if let Some((c, at, co)) = st.web.taste.lock().unwrap().co.as_ref() {
        if Arc::ptr_eq(c, cat) && at.elapsed() < CO_LISTEN_EVERY {
            return Ok(co.clone());
        }
    }
    let plays = st.db.query("SELECT user_id, track_id, played_at FROM plays WHERE track_id IS NOT NULL AND played_at > ? ORDER BY played_at DESC LIMIT ?",
        p![now() - 365.0 * 86400.0, CO_LISTEN_PLAYS])?;
    let co = Arc::new(CoListen::build(cat, plays.iter().map(|r| (r.i("user_id"), r.i("track_id"), r.f("played_at"))).collect()));
    st.web.taste.lock().unwrap().co = Some((cat.clone(), Instant::now(), co.clone()));
    Ok(co)
}

/// What user `uid` told the server: plays (with how much of each was heard), Liked Songs, the share links they
/// made, their own playlists' songs, skips (with how far into the song) and dislikes.
fn signals(st: &AppState, uid: i64) -> ApiResult<Signals> {
    let t = now();
    let ids = |sql: &str, params: Vec<Value>| -> ApiResult<Vec<i64>> { Ok(st.db.query(sql, params)?.iter().map(|r| r.i("id")).collect()) };
    Ok(Signals {
        plays: st.db.query("SELECT track_id, played_at, listened FROM plays WHERE user_id=? AND track_id IS NOT NULL AND played_at > ? ORDER BY played_at DESC LIMIT ?",
            p![uid, t - 400.0 * 86400.0, PROFILE_PLAYS])?.iter()
            .map(|r| (r.i("track_id"), r.f("played_at"), r.get("listened").and_then(Value::as_f64))).collect(),
        likes: ids("SELECT i.track_id id FROM playlist_items i JOIN playlists l ON l.id=i.playlist_id WHERE l.user_id=? AND l.kind='liked' AND i.track_id IS NOT NULL", p![uid])?,
        // the listener's own playlists (not ones added from someone else's share, not Liked Songs)
        playlisted: ids("SELECT DISTINCT i.track_id id FROM playlist_items i JOIN playlists l ON l.id=i.playlist_id
                         WHERE l.user_id=? AND l.kind='playlist' AND i.track_id IS NOT NULL LIMIT 20000", p![uid])?,
        shares: shares(st, uid)?,
        skips: st.db.query("SELECT track_id, skipped_at, seconds FROM skips WHERE user_id=? AND skipped_at > ?", p![uid, t - taste::SKIP_WINDOW])?
            .iter().map(|r| (r.i("track_id"), r.f("skipped_at"), r.f("seconds"))).collect(),
        disliked_songs: st.db.query("SELECT ref FROM dislikes WHERE user_id=? AND kind='track'", p![uid])?.iter().filter_map(|r| r.s("ref").parse().ok()).collect(),
        disliked_artists: st.db.query("SELECT ref FROM dislikes WHERE user_id=? AND kind='artist'", p![uid])?.iter().map(|r| r.s("ref")).collect(),
    })
}

/// The share links user `uid` made, as [`Signals::shares`]: a song's link is that song with portion 1, an album's
/// link each of the album's songs (each song once, not its copies) with portion 1 / their count. Playlist links
/// are left out: a playlist is already counted as the listener's own (and may hold songs others added).
fn shares(st: &AppState, uid: i64) -> ApiResult<Vec<(i64, f64, f64)>> {
    let mut out: Vec<(i64, f64, f64)> = st.db.query("SELECT track_id, created_at FROM shares WHERE user_id=? AND COALESCE(kind,'track')='track'", p![uid])?
        .iter().map(|r| (r.i("track_id"), r.f("created_at"), 1.0)).collect();
    let albums = st.db.query(&format!(
        "SELECT s.ref, s.created_at, tracks.id FROM shares s JOIN tracks ON tracks.album_id = s.ref WHERE s.user_id=? AND s.kind='album' AND {}",
        encore_library::twins::SHOWN), p![uid])?;
    // each album's songs gathered first, so each gets its part of the one album link
    let mut per_album: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
    for r in &albums {
        per_album.entry(r.s("ref")).or_default().push((r.i("id"), r.f("created_at")));
    }
    for songs in per_album.values() {
        let portion = 1.0 / songs.len() as f64;
        out.extend(songs.iter().map(|(id, at)| (*id, *at, portion)));
    }
    Ok(out)
}

/// Everything one recommendation needs for user `uid`: the catalog, the co-listening and their profile.
fn model(st: &AppState, uid: i64) -> ApiResult<(Arc<Catalog>, Arc<CoListen>, Profile)> {
    let cat = catalog(st)?;
    let co = co_listen(st, &cat)?;
    let profile = Profile::build(&cat, &co, &signals(st, uid)?, now());
    Ok((cat, co, profile))
}

/// A random source seeded from the clock (each call draws differently).
fn rng() -> Rng {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    Rng::new(t.as_nanos() as u64 ^ rand::random::<u64>())
}

/// The songs of these ids as the players queue them (in this order), with their loudness for normalization.
fn songs(st: &AppState, ids: &[i64]) -> ApiResult<Value> {
    let mut v = json!(super::library::rows_by_id(&st.db, SONG_COLS, ids)?);
    encore_library::loudness::stamp(&st.db, &mut v);
    Ok(v)
}

/// Every song user `uid` must never be offered: disliked songs and the songs whose main artist they disliked.
/// The plain shuffle (routes::library) leaves these out too.
pub(super) fn disliked_song_ids(st: &AppState, uid: i64) -> ApiResult<HashSet<i64>> {
    let rows = st.db.query("SELECT ref, kind FROM dislikes WHERE user_id=?", p![uid])?;
    if rows.is_empty() {
        return Ok(HashSet::new());
    }
    let mut out: HashSet<i64> = rows.iter().filter(|r| r.s("kind") == "track").filter_map(|r| r.s("ref").parse().ok()).collect();
    let artists: Vec<String> = rows.iter().filter(|r| r.s("kind") == "artist").map(|r| r.s("ref")).collect();
    if !artists.is_empty() {
        let cat = catalog(st)?;
        for name in artists {
            let Some(a) = cat.artist_index(&name) else { continue };
            out.extend(cat.artist_songs[a as usize].iter().filter(|i| cat.songs[**i].artists[0] == a).map(|i| cat.songs[*i].id));
        }
    }
    Ok(out)
}

/// Takes back user `uid`'s dislike of song `id` (a like does: routes::library).
pub(super) fn forget_dislike(st: &AppState, uid: i64, id: i64) -> ApiResult<()> {
    st.db.execute("DELETE FROM dislikes WHERE user_id=? AND kind='track' AND ref=?", p![uid, id.to_string()])?;
    Ok(())
}

/// The ids of a request's `exclude`, at most [`MAX_EXCLUDE`].
fn exclude_set(ids: &[i64]) -> HashSet<i64> {
    ids.iter().take(MAX_EXCLUDE).copied().collect()
}

// ---- recommendations ----------------------------------------------------------------------------------

/// `POST /api/taste/shuffle` body.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ShuffleBody {
    /// how many songs (default 50, at most 200)
    limit: Option<usize>,
    /// song ids to leave out (what the session already played or queued)
    exclude: Vec<i64>,
    /// only this genre's songs
    genre: Option<String>,
    /// only the songs of these genres (a genre shuffle of several; genres the library doesn't have are skipped)
    genres: Option<Vec<String>>,
    /// only these songs (a playlist or album being shuffled): they come back in a taste-weighted order, without
    /// the disliked ones
    ids: Option<Vec<i64>>,
}

/// POST /api/taste/shuffle: a smart shuffle – random songs weighted by the listener's taste (favourites come up
/// more often, just-played songs wait, artists are spread out), never a disliked song or artist. Over the whole
/// library, one genre (`genre`), several (`genres`: a genre shuffle) or a given list (`ids`). Answers the songs,
/// with their loudness.
async fn shuffle(State(st): State<AppState>, User(u): User, Json(b): Json<ShuffleBody>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let (cat, _, profile) = model(st, u.i("id"))?;
        let scope = if let Some(ids) = &b.ids {
            Scope::Songs(ids.iter().filter_map(|i| cat.index(*i)).collect())
        } else if let Some(gs) = b.genres.as_ref().filter(|v| !v.is_empty()) {
            let found: Vec<u16> = gs.iter().filter_map(|g| cat.genre_index(g)).collect();
            if found.is_empty() {
                return Err(err(404, "No songs of those genres"));
            }
            Scope::Genres(found)
        } else if let Some(g) = b.genre.as_deref().filter(|g| !g.trim().is_empty()) {
            Scope::Genre(cat.genre_index(g).ok_or_else(|| err(404, "No songs of that genre"))?)
        } else {
            Scope::All
        };
        // a list being shuffled comes back whole (less the disliked songs); a library shuffle in pages
        let n = match &scope { Scope::Songs(v) => v.len().min(5000), _ => b.limit.unwrap_or(50).clamp(1, MAX_SONGS) };
        let ids = profile.shuffle(&cat, &scope, n, &exclude_set(&b.exclude), &mut rng());
        if matches!(scope, Scope::Songs(_)) {
            // a long list: the ids alone (the player has the songs already)
            return Ok(Json(json!(ids)));
        }
        Ok(Json(songs(st, &ids)?))
    }).await
}

/// `GET /api/taste/mixes` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct MixesQ {
    /// songs per mix (default 30, at most 100)
    limit: Option<usize>,
}

/// GET /api/taste/mixes: the "For you" mixes – Your mix, Fresh finds, Rediscover, a mix for each favourite genre
/// and a radio for each favourite artist – `{cold, mixes: [{id, title, subtitle, covers: [album ids], tracks}]}`.
/// `cold` says the listener hasn't played, liked or disliked anything yet (the mixes are plain shuffles).
async fn mixes(State(st): State<AppState>, User(u): User, Query(q): Query<MixesQ>) -> ApiResult<Json<Value>> {
    let (cold, list) = st.blocking(move |st| {
        let (cat, co, profile) = model(st, u.i("id"))?;
        let list = profile.mixes(&cat, &co, q.limit.unwrap_or(30).clamp(5, 100), &mut rng());
        let out: Vec<Value> = list.into_iter().map(|m| {
            let tracks = songs(st, &m.songs)?;
            // the first four different albums, for a cover grid
            let mut covers: Vec<String> = vec![];
            for t in tracks.as_array().into_iter().flatten() {
                let a = t["album_id"].as_str().unwrap_or("").to_string();
                if !a.is_empty() && !covers.contains(&a) && covers.len() < 4 {
                    covers.push(a);
                }
            }
            Ok(json!({ "id": m.id, "title": m.title, "subtitle": m.subtitle, "covers": covers, "tracks": tracks }))
        }).collect::<ApiResult<_>>()?;
        Ok((profile.cold, out))
    }).await?;
    let mut v = json!(list);
    enrich(&st, None, &mut v).await;
    Ok(Json(json!({ "cold": cold, "mixes": v })))
}

/// `POST /api/taste/mix` body.
#[derive(Deserialize, Default)]
#[serde(default)]
struct MixBody {
    /// the mix (`you`, `fresh`, `rediscover`, `genre:<name>`, `artist:<name>`)
    id: String,
    /// how many songs (default 50, at most 200)
    limit: Option<usize>,
    /// song ids to leave out (the ones queued already, so a mix can go on)
    exclude: Vec<i64>,
}

/// POST /api/taste/mix: one mix made fresh (to play it, or to keep it going when the queue runs out). 404 for a
/// genre or artist the library doesn't have.
async fn mix(State(st): State<AppState>, User(u): User, Json(b): Json<MixBody>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let (cat, co, profile) = model(st, u.i("id"))?;
        let ids = profile.mix(&cat, &co, &b.id, b.limit.unwrap_or(50).clamp(1, MAX_SONGS), &exclude_set(&b.exclude), &mut rng())
            .ok_or_else(|| err(404, "No such mix"))?;
        Ok(Json(songs(st, &ids)?))
    }).await
}

/// `GET /api/taste/like-this/{id}` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct LikeThisQ {
    /// how many songs (default 25, at most 200)
    limit: Option<usize>,
    /// song ids to leave out, comma separated
    exclude: String,
}

/// GET /api/taste/like-this/{id}: songs of the library like this one – its artist and the artists heard with it,
/// shared genres, a close era – leaning to the listener's taste, never a disliked one.
async fn like_this(State(st): State<AppState>, User(u): User, Path(id): Path<i64>, Query(q): Query<LikeThisQ>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let (cat, co, profile) = model(st, u.i("id"))?;
        let seed = cat.index(id).ok_or_else(|| err(404, "Track not found"))?;
        let ex: Vec<i64> = q.exclude.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        let ids = profile.like_this(&cat, &co, seed, q.limit.unwrap_or(25).clamp(1, MAX_SONGS), &exclude_set(&ex), &mut rng());
        Ok(Json(songs(st, &ids)?))
    }).await
}

/// GET /api/taste/genres: the library's genres (from the songs' tags, untagged songs counted under their artist's
/// usual genres) with how many songs each has and how much the listener likes it (-1..1), favourites first, then
/// the biggest: `[{genre, title, songs, affinity}]`. Genres with fewer than 3 songs are left out.
async fn genres(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let (cat, _, profile) = model(st, u.i("id"))?;
        let mut v: Vec<(usize, f64, usize)> = (0..cat.genres.len()).map(|g| (g, profile.genre.get(&(g as u16)).copied().unwrap_or(0.0), cat.genre_songs[g].len()))
            .filter(|(_, _, n)| *n >= 3).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)));
        Ok(Json(json!(v.into_iter().map(|(g, a, n)| json!({
            "genre": cat.genres[g], "title": taste::genre_title(&cat.genres[g]), "songs": n, "affinity": (a * 100.0).round() / 100.0,
        })).collect::<Vec<_>>())))
    }).await
}

/// GET /api/taste/profile: what the recommendations think the listener likes – `{cold, artists, genres, decades}`,
/// each `[{name, affinity}]` best first (the top 15) – so a page can say why a mix is what it is.
async fn profile(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let (cat, _, p) = model(st, u.i("id"))?;
        let r = |x: f64| (x * 100.0).round() / 100.0;
        let artists: Vec<Value> = p.top_artists(15).into_iter().map(|(a, x)| json!({ "name": cat.artists[a as usize], "affinity": r(x) })).collect();
        let genres: Vec<Value> = p.top_genres(&cat, 15, 1).into_iter().filter(|(_, x)| *x > 0.0).map(|(g, x)| json!({ "name": taste::genre_title(&cat.genres[g as usize]), "affinity": r(x) })).collect();
        let mut decades: Vec<(u16, f64)> = p.decade.iter().filter(|(_, x)| **x > 0.0).map(|(d, x)| (*d, *x)).collect();
        decades.sort_by(|a, b| b.1.total_cmp(&a.1));
        let decades: Vec<Value> = decades.into_iter().take(15).map(|(d, x)| json!({ "name": format!("{d}s"), "affinity": r(x) })).collect();
        Ok(Json(json!({ "cold": p.cold, "artists": artists, "genres": genres, "decades": decades })))
    }).await
}

// ---- feedback -----------------------------------------------------------------------------------------

/// `POST /api/taste/skip` and `POST /api/taste/listened` body.
#[derive(Deserialize)]
struct SkipBody {
    /// the song skipped
    track_id: i64,
    /// how far into it (seconds)
    #[serde(default)]
    seconds: f64,
}

/// POST /api/taste/skip: the listener pressed Next before the song's play counted. A skip in the first second or
/// two is not counted (the listener was flicking through), and neither is one of a song the library doesn't have.
async fn skip(State(st): State<AppState>, User(u): User, Json(b): Json<SkipBody>) -> ApiResult<Json<Value>> {
    let secs = if b.seconds.is_finite() { b.seconds.clamp(0.0, 7200.0) } else { 0.0 };
    if secs < 2.0 || st.db.one("SELECT 1 FROM tracks WHERE id=?", p![b.track_id])?.is_none() {
        return Ok(Json(json!({ "ok": true, "counted": false })));
    }
    st.db.execute("INSERT INTO skips (user_id, track_id, seconds, skipped_at) VALUES (?,?,?,?)", p![u.i("id"), b.track_id, secs, now()])?;
    // skips older than the window count for nothing: the table keeps only those
    st.db.execute("DELETE FROM skips WHERE user_id=? AND skipped_at < ?", p![u.i("id"), now() - taste::SKIP_WINDOW])?;
    Ok(Json(json!({ "ok": true, "counted": true })))
}

/// How long after a play was saved the player may still say how much of it was heard (seconds): a long song
/// paused for a while still gets its answer, a play from yesterday is left as it is.
const LISTENED_WITHIN: f64 = 12.0 * 3600.0;

/// POST /api/taste/listened `{track_id, seconds}`: a song whose play already counted ended, or the listener moved
/// on from it, `seconds` in. The listener's latest play of the song (saved in the last [`LISTENED_WITHIN`]) keeps
/// the furthest point it was told, and the recommendations weigh the play by it (encore_library::taste::play_weight).
/// `{ok: true, counted}`: `counted` is false when there was no such play.
async fn listened(State(st): State<AppState>, User(u): User, Json(b): Json<SkipBody>) -> ApiResult<Json<Value>> {
    if !b.seconds.is_finite() || b.seconds <= 0.0 {
        return Ok(Json(json!({ "ok": true, "counted": false })));
    }
    let secs = b.seconds.min(7200.0);
    // MAX: a second report (another tab, a retry) never moves it back; RETURNING says whether a play was there
    let changed = st.db.query(
        "UPDATE plays SET listened = MAX(COALESCE(listened, 0), ?) WHERE id = (SELECT id FROM plays WHERE user_id=? AND track_id=? AND played_at > ? ORDER BY played_at DESC LIMIT 1) RETURNING id",
        p![secs, u.i("id"), b.track_id, now() - LISTENED_WITHIN],
    )?;
    Ok(Json(json!({ "ok": true, "counted": !changed.is_empty() })))
}

/// GET /api/taste/dislikes: what the listener disliked, newest first – `{tracks: [{id, name, created_at}],
/// artists: [{name, created_at}]}` – for the players' dislike buttons and the page that takes dislikes back.
async fn dislikes(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let rows = st.db.query("SELECT kind, ref, name, created_at FROM dislikes WHERE user_id=? ORDER BY created_at DESC", p![u.i("id")])?;
    let tracks: Vec<Value> = rows.iter().filter(|r| r.s("kind") == "track")
        .map(|r| json!({ "id": r.s("ref").parse::<i64>().unwrap_or(0), "name": r.s("name"), "created_at": r.f("created_at") })).collect();
    let artists: Vec<Value> = rows.iter().filter(|r| r.s("kind") == "artist").map(|r| json!({ "name": r.s("name"), "created_at": r.f("created_at") })).collect();
    Ok(Json(json!({ "tracks": tracks, "artists": artists })))
}

/// PUT /api/taste/dislikes/tracks/{id}: dislike a song – it is never queued by a shuffle, a mix or "More like
/// this" again, what sounds like it comes up less, and it leaves Liked Songs (a dislike is the opposite of a like).
async fn dislike_track(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT artist, title FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    let uid = u.i("id");
    st.db.execute("INSERT OR REPLACE INTO dislikes (user_id, kind, ref, name, created_at) VALUES (?, 'track', ?, ?, ?)",
        p![uid, id.to_string(), format!("{} – {}", t.s("artist"), t.s("title")), now()])?;
    super::library::remove_like(&st, uid, id).await?;
    Ok(Json(json!({ "disliked": true })))
}

/// DELETE /api/taste/dislikes/tracks/{id}: take a song's dislike back.
async fn undislike_track(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    forget_dislike(&st, u.i("id"), id)?;
    Ok(Json(json!({ "disliked": false })))
}

/// PUT /api/taste/dislikes/artists/{name}: "Don't play this artist" – none of the artist's own songs is queued by
/// a shuffle or a mix again (songs it only features on just come up less).
async fn dislike_artist(State(st): State<AppState>, User(u): User, Path(name): Path<String>) -> ApiResult<Json<Value>> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 300 {
        return Err(err(422, "Which artist?"));
    }
    st.db.execute("INSERT OR REPLACE INTO dislikes (user_id, kind, ref, name, created_at) VALUES (?, 'artist', ?, ?, ?)",
        p![u.i("id"), taste::artist_key(&name), name, now()])?;
    Ok(Json(json!({ "disliked": true })))
}

/// DELETE /api/taste/dislikes/artists/{name}: play an artist again.
async fn undislike_artist(State(st): State<AppState>, User(u): User, Path(name): Path<String>) -> ApiResult<Json<Value>> {
    st.db.execute("DELETE FROM dislikes WHERE user_id=? AND kind='artist' AND ref=?", p![u.i("id"), taste::artist_key(&name)])?;
    Ok(Json(json!({ "disliked": false })))
}

/// The lock type of `Web::taste`.
pub(crate) type TasteLock = Arc<Mutex<TasteCache>>;

#[cfg(test)]
#[path = "taste_tests.rs"]
mod taste_tests;
