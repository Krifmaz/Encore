//! Library search and artists through the real router: ranking, merged spellings, collaborations split
//! only when every part is an artist of its own (or an alias), duplicates hidden, artist pages.
use encore_db::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

const PW: &str = "searcher-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: std::path::PathBuf,
    cookie: String,
    next: i64,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-search-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    crate::auth::create_user(&st, "searcher", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let r = call(&app, "GET", "/api/health", "").await;
    assert_eq!(r.0, StatusCode::OK);
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "searcher", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    Env { st, app, dir, cookie, next: 1 }
}

async fn call(app: &Router, method: &str, uri: &str, cookie: &str) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("x-requested-with", "encore");
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    let resp = app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

struct T<'a> {
    title: &'a str,
    artist: &'a str,
    album: &'a str,
    albumartist: &'a str,
    format: &'a str,
    bitrate: i64,
    duration: f64,
    mbid: &'a str,
}

fn t<'a>(title: &'a str, artist: &'a str, album: &'a str, albumartist: &'a str) -> T<'a> {
    T { title, artist, album, albumartist, format: "flac", bitrate: 900, duration: 200.0, mbid: "" }
}

impl Env {
    fn add(&mut self, x: T) -> i64 {
        let id = self.next;
        self.next += 1;
        let aid = encore_library::album_key(x.albumartist, x.album, "");
        self.st.db.execute(
            "INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, bitrate, duration, mb_albumartistid, size, year, disc, track, added_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![id, format!("/m/{id}.{}", x.format), x.title, x.artist, x.album, x.albumartist, aid, x.format, x.bitrate, x.duration,
               if x.mbid.is_empty() { Value::Null } else { json!(x.mbid) }, 1000, "2010", 1, id, id as f64]).unwrap();
        self.st.library.artists.invalidate();
        id
    }
    fn many(&mut self, n: usize, artist: &str, album: &str, mbid: &str) {
        for i in 0..n {
            let title = format!("{album} song {i}");
            self.add(T { mbid, ..t(&title, artist, album, artist) });
        }
    }
    async fn get(&self, uri: &str) -> Value {
        let r = call(&self.app, "GET", uri, &self.cookie).await;
        assert_eq!(r.0, StatusCode::OK, "{uri}: {:?}", r.1);
        r.1
    }
    async fn artist_names(&self, uri: &str) -> Vec<String> {
        self.get(uri).await.as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap_or("").to_string()).collect()
    }
}

const YE: &str = "164f0d73-1234-4e2c-8743-d77bf2191051";

/// The owner's library, roughly: what "ye", "kanye" and "808" used to get wrong.
async fn owners_library(name: &str) -> Env {
    let mut e = setup(name).await;
    e.many(6, "Ye", "808s & Heartbreak", YE);
    e.many(4, "Ye", "Graduation", YE);
    e.many(3, "Black Eyed Peas", "Elephunk", "");
    e.many(2, "The Black Eyed Peas", "Monkey Business", "");
    e.many(2, "Elton John & Years & Years", "It's a Sin", "");
    e.many(2, "Kanye West & XXXTENTACION", "True Love", "");
    e.many(3, "XXXTENTACION", "17", "");
    e.many(2, "TAEYEON", "Purpose", "");
    e.many(2, "Simon & Garfunkel", "Bookends", "");
    e.many(2, "Paul Simon", "Graceland", "");
    e
}

#[tokio::test]
async fn ye_ranks_first_and_spellings_merge() {
    let e = owners_library("ye").await;
    let names = e.artist_names("/api/library/artists?sort=name&q=ye").await;
    assert_eq!(names.first().map(String::as_str), Some("Ye"), "{names:?}");
    // the rest: word starts before contains
    let pos = |n: &str| names.iter().position(|x| x == n);
    assert!(pos("Elton John & Years & Years").unwrap() < pos("TAEYEON").unwrap(), "{names:?}");
    assert!(pos("Black Eyed Peas").is_some(), "{names:?}");
    // one Black Eyed Peas, with the songs of both spellings, named after the bigger one
    let all = e.get("/api/library/artists?sort=name").await;
    let peas: Vec<&Value> = all.as_array().unwrap().iter().filter(|a| a["name"].as_str().unwrap().contains("Black Eyed Peas")).collect();
    assert_eq!(peas.len(), 1, "{all}");
    assert_eq!(peas[0]["name"], "Black Eyed Peas");
    assert_eq!(peas[0]["tracks"], 5);
    assert_eq!(peas[0]["albums"], 2);
    let names = e.artist_names("/api/library/artists?q=the%20black%20eyed%20peas").await;
    assert_eq!(names, vec!["Black Eyed Peas"]);
    // accents don't matter
    let names = e.artist_names("/api/library/artists?q=TA%C3%89YEON").await;
    assert_eq!(names.first().map(String::as_str), Some("TAEYEON"));
    // empty q keeps the sorts
    let by_tracks = e.artist_names("/api/library/artists?sort=tracks").await;
    assert_eq!(by_tracks[0], "Ye");
    let by_name = e.artist_names("/api/library/artists?sort=name").await;
    let mut sorted = by_name.clone();
    sorted.sort_by_key(|s| s.to_lowercase());
    assert_eq!(by_name, sorted);
}

#[tokio::test]
async fn collaborations_split_only_when_every_part_exists() {
    let mut e = owners_library("collab").await;
    // "Kanye West" isn't an artist of its own (yet): the collaboration stays whole
    let all = e.artist_names("/api/library/artists?sort=name").await;
    assert!(all.contains(&"Kanye West & XXXTENTACION".to_string()), "{all:?}");
    assert!(all.contains(&"Simon & Garfunkel".to_string()), "{all:?}");
    assert!(all.contains(&"Elton John & Years & Years".to_string()), "{all:?}");
    // Ye is also known as Kanye West (MusicBrainz alias): now it splits into Ye and XXXTENTACION
    encore_library::artists::store_aliases(&e.st.library.artists, &e.st.db, "ye", &["Ye".into(), "Kanye West".into(), "West, Kanye".into()]).unwrap();
    let all = e.get("/api/library/artists?sort=name").await;
    let names: Vec<&str> = all.as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert!(!names.contains(&"Kanye West & XXXTENTACION"), "{names:?}");
    assert!(names.contains(&"Simon & Garfunkel"), "{names:?}");
    let ye = all.as_array().unwrap().iter().find(|a| a["name"] == "Ye").unwrap();
    assert_eq!(ye["tracks"], 12);
    assert_eq!(ye["albums"], 3);
    assert_eq!(ye["aliases"], json!(["Kanye West", "West, Kanye"]));
    let x = all.as_array().unwrap().iter().find(|a| a["name"] == "XXXTENTACION").unwrap();
    assert_eq!(x["tracks"], 5);
    // "kanye" finds Ye through the alias
    let hits = e.get("/api/library/artists?q=kanye").await;
    assert_eq!(hits[0]["name"], "Ye", "{hits}");
    assert_eq!(hits[0]["match"], "alias");
    // a real "Kanye West" album artist joins Ye (same person by alias) rather than being a second artist
    e.many(1, "Kanye West", "The College Dropout", "");
    let names = e.artist_names("/api/library/artists?sort=name").await;
    assert!(!names.contains(&"Kanye West".to_string()), "{names:?}");
    // a collaboration of two artists that both exist on their own splits without any alias
    e.many(1, "Paul Simon & XXXTENTACION", "Odd Pair", "");
    let names = e.artist_names("/api/library/artists?sort=name").await;
    assert!(!names.contains(&"Paul Simon & XXXTENTACION".to_string()), "{names:?}");
}

#[tokio::test]
async fn artists_are_found_through_their_albums_and_songs() {
    let e = owners_library("808").await;
    let hits = e.get("/api/library/artists?q=808").await;
    assert_eq!(hits[0]["name"], "Ye", "{hits}");
    assert_eq!(hits[0]["match"], "album");
    let hits = e.get("/api/library/artists?q=graduation%20song%203").await;
    assert_eq!(hits[0]["name"], "Ye", "{hits}");
    assert_eq!(hits[0]["match"], "song");
    // a name match ranks above an album match
    let hits = e.get("/api/library/artists?q=xxx").await;
    assert_eq!(hits[0]["name"], "XXXTENTACION", "{hits}");
    assert_eq!(hits[0]["match"], "name");
    // albums: exact title first, one row per album
    let r = e.get("/api/library/albums?sort=name&limit=20&q=graceland").await;
    assert_eq!(r["total"], 1);
    assert_eq!(r["items"][0]["album"], "Graceland");
    let r = e.get("/api/library/albums?sort=name&limit=20&q=ye").await;
    let titles: Vec<&str> = r["items"].as_array().unwrap().iter().map(|a| a["album"].as_str().unwrap()).collect();
    assert_eq!(&titles[..2], &["808s & Heartbreak", "Graduation"], "{titles:?}");
}

#[tokio::test]
async fn the_same_song_twice_shows_once_in_its_best_quality() {
    let mut e = owners_library("dupes").await;
    let mp3 = e.add(T { format: "mp3", bitrate: 320, duration: 180.0, ..t("BADSIDE", "Ye", "Vultures", "Ye") });
    let flac = e.add(T { format: "flac", bitrate: 1000, duration: 181.2, ..t("BADSIDE", "Ye", "Vultures", "Ye") });
    // a different recording (much longer) stays
    let live = e.add(T { duration: 260.0, ..t("BADSIDE", "Ye", "Vultures", "Ye") });
    let r = e.get("/api/library/tracks?sort=artist&limit=30&q=badside").await;
    let ids: Vec<i64> = r.as_array().unwrap().iter().map(|x| x["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&flac) && ids.contains(&live) && !ids.contains(&mp3), "{ids:?}");
    assert_eq!(ids.len(), 2);
    // same JSON as before
    assert!(r[0].get("album_id").is_some() && r[0].get("verify_status").is_some());
    // title matches rank above artist / album matches
    let r = e.get("/api/library/tracks?sort=artist&limit=30&q=graduation%20song%201").await;
    assert_eq!(r[0]["title"], "Graduation song 1");
}

#[tokio::test]
async fn artist_pages_include_collaborations_and_spellings() {
    let mut e = owners_library("page").await;
    encore_library::artists::store_aliases(&e.st.library.artists, &e.st.db, "ye", &["Kanye West".into()]).unwrap();
    // a feature on someone else's album, credited to the song only
    let feat = e.add(t("Guest Verse", "Ye feat. XXXTENTACION", "Hits", "Various Artists"));
    let r = e.get("/api/library/artist-tracks?name=Ye").await;
    let titles: Vec<&str> = r.as_array().unwrap().iter().map(|x| x["title"].as_str().unwrap()).collect();
    assert_eq!(titles.len(), 13, "{titles:?}");
    assert!(titles.contains(&"True Love song 0"), "{titles:?}");
    assert!(r.as_array().unwrap().iter().any(|x| x["id"] == feat));
    assert!(r[0].get("album_id").is_some() && r[0].get("format").is_some());
    // any spelling or alias finds the same artist
    let same = e.get("/api/library/artist-tracks?name=kanye%20west").await;
    assert_eq!(same.as_array().unwrap().len(), 13);
    let peas = e.get("/api/library/artist-tracks?name=The%20Black%20Eyed%20Peas").await;
    assert_eq!(peas.as_array().unwrap().len(), 5);
    // albums?artist= has the collaboration album too
    let r = e.get("/api/library/albums?artist=Ye&sort=year").await;
    let titles: Vec<&str> = r["items"].as_array().unwrap().iter().map(|a| a["album"].as_str().unwrap()).collect();
    assert!(titles.contains(&"True Love") && titles.contains(&"Graduation"), "{titles:?}");
    let r = e.get("/api/library/albums?artist=black%20eyed%20peas").await;
    assert_eq!(r["total"], 2);
    // the catalog page's Play / Shuffle (by MusicBrainz id and name)
    let r = e.get(&format!("/api/discover/artist/{YE}/library-tracks?name=Ye")).await;
    assert_eq!(r.as_array().unwrap().len(), 13);
    // unknown names still work like before
    let r = e.get("/api/library/artist-tracks?name=Nobody").await;
    assert_eq!(r, json!([]));
}

#[tokio::test]
async fn removing_an_artist_removes_every_merged_spelling() {
    let mut e = owners_library("delete").await;
    e.st.db.execute("UPDATE users SET role='admin' WHERE username='searcher'", p![]).unwrap();
    let r = call(&e.app, "DELETE", "/api/library/artists?name=The%20Black%20Eyed%20Peas", &e.cookie).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    assert_eq!(r.1["removed"], 5);
    let names = e.artist_names("/api/library/artists?sort=name").await;
    assert!(!names.iter().any(|n| n.contains("Black Eyed Peas")), "{names:?}");
    // a collaboration stays with its other artists
    encore_library::artists::store_aliases(&e.st.library.artists, &e.st.db, "ye", &["Kanye West".into()]).unwrap();
    assert_eq!(e.get("/api/library/artist-tracks?name=Ye").await.as_array().unwrap().len(), 12);
    let r = call(&e.app, "DELETE", "/api/library/artists?name=XXXTENTACION", &e.cookie).await;
    assert_eq!(r.1["removed"], 3);
    let all = e.get("/api/library/artists?sort=name").await;
    let collab = all.as_array().unwrap().iter().find(|a| a["name"] == "Kanye West & XXXTENTACION").expect("the collaboration stays");
    assert_eq!(collab["tracks"], 2);
    // an artist named with symbols only is still an artist
    e.many(2, "!!!", "Thr!!!er", "");
    let all = e.get("/api/library/artists?sort=name").await;
    let chk = all.as_array().unwrap().iter().find(|a| a["name"] == "!!!").expect("!!! listed");
    assert_eq!(chk["tracks"], 2);
    assert_eq!(e.artist_names("/api/library/artists?q=!!!").await, vec!["!!!"]);
}

/// The owner's "travis scott 90210" over the library: any order, typos, accents, a Top result.
async fn travis_library(name: &str) -> Env {
    let mut e = setup(name).await;
    for title in ["Pornography", "Oh My Dis Side", "3500", "Wasted", "90210", "Pray 4 Love", "Antidote"] {
        e.add(t(title, "Travis Scott", "Rodeo", "Travis Scott"));
    }
    e.add(t("travis scott", "user-1776339120", "travis scott", "user-1776339120"));
    e.add(t("Travis Scott Burger", "Meal Jokes", "Fast Food", "Meal Jokes"));
    e.add(t("Scott Travis (Drums)", "Judas Priest", "Painkiller", "Judas Priest"));
    e.add(t("Halo", "Beyoncé", "I Am... Sasha Fierce", "Beyoncé"));
    e.add(t("Café del Mar", "Energy 52", "Café del Mar", "Energy 52"));
    e
}

#[tokio::test]
async fn library_search_reads_artist_and_title() {
    let e = travis_library("split").await;
    for q in ["travis%20scott%2090210", "90210%20travis%20scott", "90210%20by%20travis%20scott", "trvis%20scot%2090210", "TRAVIS%20SCOTT%20-%2090210"] {
        let r = e.get(&format!("/api/library/search?q={q}")).await;
        assert_eq!(r["top"]["kind"], "song", "{q}: {r}");
        assert_eq!(r["top"]["item"]["title"], "90210", "{q}: {r}");
        assert_eq!(r["songs"][0]["title"], "90210", "{q}: {r}");
        // the album it is on, and the artist
        assert_eq!(r["albums"][0]["album"], "Rodeo", "{q}: {r}");
        assert_eq!(r["artists"][0]["name"], "Travis Scott", "{q}: {r}");
        // the list endpoints read it the same way
        let tr = e.get(&format!("/api/library/tracks?q={q}")).await;
        assert_eq!(tr[0]["title"], "90210", "{q}: {tr}");
    }
    // the joke songs are not what "travis scott 90210" finds first
    let r = e.get("/api/library/tracks?q=travis%20scott%2090210").await;
    let titles: Vec<&str> = r.as_array().unwrap().iter().map(|x| x["title"].as_str().unwrap()).collect();
    assert_eq!(titles[0], "90210", "{titles:?}");
    // an artist's name alone: the artist on top
    let r = e.get("/api/library/search?q=travis%20scott").await;
    assert_eq!(r["top"]["kind"], "artist", "{r}");
    assert_eq!(r["top"]["item"]["name"], "Travis Scott");
    // an album by artist + title
    let r = e.get("/api/library/albums?q=rodeo%20travis%20scott").await;
    assert_eq!(r["items"][0]["album"], "Rodeo", "{r}");
    let r = e.get("/api/library/search?q=travis%20scott%20rodeo").await;
    assert_eq!(r["top"]["kind"], "album", "{r}");
    assert_eq!(r["top"]["item"]["album"], "Rodeo");
}

#[tokio::test]
async fn library_search_forgives_typos_and_accents() {
    let e = travis_library("typos").await;
    // a typo in the artist alone
    let names = e.artist_names("/api/library/artists?q=trvis%20scot").await;
    assert_eq!(names.first().map(String::as_str), Some("Travis Scott"), "{names:?}");
    // a typo in a title alone
    let r = e.get("/api/library/search?q=antidoet").await;
    assert_eq!(r["top"]["item"]["title"], "Antidote", "{r}");
    // accents either way
    let r = e.get("/api/library/search?q=beyonce%20halo").await;
    assert_eq!(r["top"]["kind"], "song", "{r}");
    assert_eq!(r["top"]["item"]["title"], "Halo");
    let r = e.get("/api/library/search?q=cafe%20del%20mar").await;
    assert_eq!(r["albums"][0]["album"], "Café del Mar", "{r}");
    let r = e.get("/api/library/search?q=Caf%C3%A9").await;
    assert!(r["songs"].as_array().unwrap().iter().any(|s| s["title"] == "Café del Mar"), "{r}");
    // nothing typed, nothing found
    assert_eq!(e.get("/api/library/search?q=").await["top"], Value::Null);
    let r = e.get("/api/library/search?q=zzzzqqqq").await;
    assert_eq!(r["songs"], json!([]));
}

/// An album search ranks the matches out of the cached albums list in memory; the answer is the one the
/// query with every matching album id in an IN list gave (0.28), for every sort, filter, kind and page.
#[tokio::test]
async fn album_search_in_memory_answers_as_the_in_list_query() {
    use encore_library::lists::{order, ALBUM_SORTS};
    let mut e = owners_library("albums_in_memory").await;
    e.add(T { format: "mp3", ..t("Lossy one", "Ye", "Yeezus", "Ye") });
    let old = |q: &str, sort: &str, dir: &str, filter: &str, kind: &str, artist: &str, offset: usize, limit: usize| -> Value {
        let mut r = encore_library::search::library_album_ranks(&e.st.library.artists.index(&e.st.db), q);
        r.truncate(30000);
        let mut wh = vec![format!("album_id IN ({})", encore_library::artists::marks(r.len()))];
        let mut params: Vec<Value> = r.iter().map(|(id, _)| json!(id)).collect();
        if r.is_empty() {
            params.push(json!(""));
            wh.push("0".into());
        }
        if !artist.is_empty() {
            let (cond, ps) = e.st.library.artists.album_filter(&e.st.db, artist);
            wh.push(cond);
            params.extend(ps);
        }
        let mut having = vec![];
        if filter == "lossless" {
            having.push("SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = COUNT(*)");
        }
        if kind == "singles" {
            having.push("COUNT(*) <= 2");
        }
        let having = if having.is_empty() { String::new() } else { format!("HAVING {}", having.join(" AND ")) };
        let sql = format!(
            "SELECT album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(genre) genre,
                    COUNT(*) tracks, SUM(duration) duration, SUM(size) size, MAX(added_at) added,
                    GROUP_CONCAT(DISTINCT format) formats, MAX(mb_albumid) mb_albumid, MAX(mb_releasegroupid) mb_releasegroupid,
                    SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) untagged
             FROM tracks WHERE {} GROUP BY album_id {having} ORDER BY {}", wh.join(" AND "), order(ALBUM_SORTS, sort, dir, "name"));
        let tier: std::collections::HashMap<String, u8> = r.into_iter().collect();
        let mut items = e.st.db.query(&sql, params).unwrap();
        use encore_db::RowExt;
        items.sort_by_key(|i| tier.get(&i.s("id")).copied().unwrap_or(9));
        let total = items.len();
        json!({ "total": total, "items": items.into_iter().skip(offset).take(limit).collect::<Vec<_>>() })
    };
    let mut checked = 0;
    for q in ["ye", "808", "black", "years", "simon", "song", "zzzz"] {
        for (sort, dir) in [("name", ""), ("year", "asc"), ("added", "desc"), ("size", ""), ("album", "desc")] {
            for (filter, kind, artist) in [("", "", ""), ("lossless", "", ""), ("", "singles", ""), ("", "", "Ye")] {
                for (offset, limit) in [(0, 120), (1, 2)] {
                    let got = e.get(&format!("/api/library/albums?q={q}&sort={sort}&dir={dir}&filter={filter}&kind={kind}&artist={artist}&offset={offset}&limit={limit}")).await;
                    assert_eq!(got, old(q, sort, dir, filter, kind, artist, offset, limit), "{q} {sort} {dir} {filter} {kind} {artist} {offset}");
                    checked += got["items"].as_array().unwrap().len();
                }
            }
        }
    }
    assert!(checked > 100, "the cases found albums: {checked}");
}
