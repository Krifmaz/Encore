//! The catalogue merge: Deezer, Apple Music, Bandcamp and Discogs discographies merged onto MusicBrainz,
//! and the helpers it shares with the search (title normalising, release kinds, Harmony links,
//! durations in the services' formats).
//!
//! Matching, strongest first: barcode (UPC ↔ MusicBrainz release barcode) → ISRC of the first track
//! (→ MusicBrainz recording → release group) → normalized title + year. Releases that match nothing are
//! shown as "Not in MusicBrainz", with a Harmony link so they can be added there.
use crate::Ctx;
use encore_config::release_types::ReleaseFilter;
use encore_core::json::s;
use encore_db::{p, Db, Row, RowExt};
use encore_http::{apple, deezer, discogs, json as getj, mb};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::LazyLock;

/// " - Single" / " - EP" at the end of a title (lower case).
static RE_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+-\s+(single|ep)$").unwrap());
/// A featured-artist note: "(feat. X)", "[with X]", or " feat. X" to the end.
static RE_FEAT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[]\s*(feat\.?|ft\.?|featuring|with)\s[^\)\]]*[\)\]]|\s+(feat\.|ft\.|featuring)\s.*$").unwrap());
/// An edition note in brackets: "(Deluxe Edition)", "[Remastered 2011]" …
static RE_EDITION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[][^\)\]]*(deluxe|edition|remaster|version|expanded|anniversary|explicit|clean|bonus)[^\)\]]*[\)\]]").unwrap());

/// The word "live" in an edition note.
static RE_LIVE_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\blive\b").unwrap());

/// The key a release title is compared by: without " - Single" / " - EP", featured artists ("(feat. X)") and
/// edition notes ("(Deluxe Edition)", "\[Remastered\]" …, but not "(Live Version)"), accents dropped, "&" read as "and", then letters,
/// digits and underscores.
pub fn title_key(title: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let t: String = title.to_lowercase().nfkd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).collect();
    let t = RE_SUFFIX.replace(&t, "");
    let t = RE_FEAT.replace_all(&t, "");
    // an edition note that says live ("(Live Version)") is no edition: the live album is another album
    let t = RE_EDITION.replace_all(&t, |c: &regex::Captures| if RE_LIVE_WORD.is_match(&c[0]) { c[0].to_string() } else { String::new() }).replace('&', "and");
    encore_core::text::norm_keep_underscore(&t)
}

/// How alike two release titles are by their [`title_key`]s, 0..1.
pub fn similar(a: &str, b: &str) -> f64 {
    let (a, b) = (title_key(a), title_key(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    crate::matcher::sim(&a, &b)
}

/// A Harmony link that looks these releases up (`url=` each, and the barcode as `gtin=`).
pub fn harmony_url(urls: &[String], upc: Option<&str>) -> String {
    let mut q: Vec<String> = urls.iter().map(|u| format!("url={}", urlencoding::encode(u))).collect();
    if let Some(u) = upc.filter(|u| !u.is_empty()) {
        q.push(format!("gtin={u}"));
    }
    q.push("category=preferred".into());
    format!("https://harmony.pulsewidth.org.uk/release?{}", q.join("&"))
}

/// "EP", "Single", "Compilation" or "Album" from a service's type and its title's suffix.
pub fn kind_of(kind: &str, title: &str) -> &'static str {
    let k = kind.to_lowercase();
    if k == "ep" || title.ends_with(" - EP") {
        "EP"
    } else if k == "single" || title.ends_with(" - Single") {
        "Single"
    } else if k == "compile" || k == "compilation" {
        "Compilation"
    } else {
        "Album"
    }
}

/// A length in seconds from a number, an ISO 8601 duration ("PT3M25S") or "m:ss" / "h:mm:ss"; null when none.
pub fn dur(v: &Value) -> Value {
    if let Some(n) = v.as_f64() {
        return json!(n);
    }
    let t = v.as_str().unwrap_or("").replace("P00H", "PT0H");
    if t.is_empty() {
        return Value::Null;
    }
    let iso = Regex::new(r"^P?T?(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?$").unwrap();
    if let Some(c) = iso.captures(&t) {
        if c.iter().skip(1).any(|m| m.is_some()) {
            let g = |i| c.get(i).and_then(|m: regex::Match| m.as_str().parse::<f64>().ok()).unwrap_or(0.0);
            return json!(g(1) * 3600.0 + g(2) * 60.0 + g(3));
        }
    }
    let parts: Vec<f64> = t.split(':').filter_map(|p| p.parse().ok()).collect();
    if parts.is_empty() { Value::Null } else { json!(parts.iter().fold(0.0, |acc, p| acc * 60.0 + p)) }
}

// ---- the catalogue merge: Deezer, Apple Music, Bandcamp and Discogs onto MusicBrainz ---------------

/// Deezer album lookups (UPC and ISRC) per artist.
const MAX_DEEP: i32 = 25; // Deezer album lookups (UPC + ISRC) per artist
/// MusicBrainz ISRC lookups per artist.
const MAX_ISRC: i32 = 8; // MusicBrainz ISRC lookups per artist



// ---- finding the artist on each service ------------------------------------------------------

/// The first capture of `re` in any of the URLs.
fn from_urls(urls: &[String], re: &str) -> Option<String> {
    let re = Regex::new(re).unwrap();
    urls.iter().find_map(|u| re.captures(u).map(|c| c[1].to_string()))
}

/// The artist's Deezer id: from MusicBrainz's links, else a search by name.
async fn deezer_artist(st: &impl Ctx, name: &str, urls: &[String]) -> anyhow::Result<Option<String>> {
    if let Some(id) = from_urls(urls, r"deezer\.com/(?:\w+/)?artist/(\d+)") {
        return Ok(Some(id));
    }
    let d = getj(deezer::search(st, "artist", name, 10)).await?;
    Ok(d["data"].as_array().cloned().unwrap_or_default().into_iter().filter(|a| title_key(&s(&a["name"])) == title_key(name))
        .max_by_key(|a| a["nb_fan"].as_i64().unwrap_or(0)).map(|a| a["id"].to_string()))
}

/// The artist's Apple Music id: from MusicBrainz's links, else a search by name.
async fn apple_artist(st: &impl Ctx, name: &str, urls: &[String]) -> anyhow::Result<Option<String>> {
    if let Some(id) = from_urls(urls, r"(?:music|itunes)\.apple\.com/.*?/artist/(?:[^/]+/)?(?:id)?(\d+)") {
        return Ok(Some(id));
    }
    let d = getj(apple::search(st, name, "musicArtist", 10)).await?;
    Ok(d["results"].as_array().cloned().unwrap_or_default().into_iter().find(|a| title_key(&s(&a["artistName"])) == title_key(name)).map(|a| a["artistId"].to_string()))
}

/// The artist's Discogs id: from MusicBrainz's links, else a search by name (with a Discogs token).
async fn discogs_artist(st: &impl Ctx, name: &str, urls: &[String]) -> anyhow::Result<Option<String>> {
    if let Some(id) = from_urls(urls, r"discogs\.com/(?:\w+/)?artist/(\d+)") {
        return Ok(Some(id));
    }
    if st.cfg().str("discogs_token").is_empty() {
        return Ok(None); // search needs a token; MusicBrainz links work without
    }
    let d: Value = discogs::search(st, &[("q", name), ("type", "artist"), ("per_page", "10")]).send().await?.json().await?;
    let num = Regex::new(r"\s*\(\d+\)$").unwrap();
    Ok(d["results"].as_array().cloned().unwrap_or_default().into_iter()
        .find(|a| title_key(&num.replace(&s(&a["title"]), "")) == title_key(name)).map(|a| a["id"].to_string()))
}

/// The artist's Bandcamp site: from MusicBrainz's links, else a search by name.
async fn bandcamp_artist(st: &impl Ctx, name: &str, urls: &[String]) -> anyhow::Result<Option<String>> {
    if let Some(b) = from_urls(urls, r"^(https?://[\w-]+\.bandcamp\.com)") {
        return Ok(Some(b));
    }
    let html = st.http().get("https://bandcamp.com/search").query(&[("q", name), ("item_type", "b")]).send().await?.text().await?;
    let re = Regex::new(r#"<div class="heading">\s*<a href="(https://[\w-]+\.bandcamp\.com)[^"]*">\s*([^<]+?)\s*</a>"#).unwrap();
    let found = re.captures_iter(&html).find(|c| title_key(&c[2]) == title_key(name)).map(|c| c[1].to_string());
    Ok(found)
}

// ---- their discographies --------------------------------------------------------------------

/// A `sources` list of one: the service, its id and the release's page.
fn src(name: &str, id: Value, url: String) -> Value {
    json!([{ "name": name, "id": id, "url": url }])
}

/// The artist's albums on Deezer, as catalogue entries.
async fn deezer_albums(st: &impl Ctx, aid: &str) -> anyhow::Result<Vec<Value>> {
    let d = getj(deezer::artist_albums(st, aid, 300)).await?;
    Ok(d["data"].as_array().cloned().unwrap_or_default().iter().map(|a| {
        let title = s(&a["title"]);
        json!({ "title": title, "type": kind_of(&s(&a["record_type"]), &title), "date": s(&a["release_date"]),
                "cover": a.get("cover_xl").filter(|c| !c.is_null()).or(a.get("cover_medium")), "tracks": a["nb_tracks"], "explicit": a["explicit_lyrics"],
                "sources": src("Deezer", a["id"].clone(), format!("https://www.deezer.com/album/{}", a["id"])) })
    }).collect())
}

/// The artist's albums on Apple Music, as catalogue entries.
async fn apple_albums(st: &impl Ctx, aid: &str) -> anyhow::Result<Vec<Value>> {
    let d = getj(apple::lookup(st, aid, "album", Some(200))).await?;
    Ok(d["results"].as_array().cloned().unwrap_or_default().iter().filter(|a| a["wrapperType"] == "collection").map(|a| {
        let title = s(&a["collectionName"]);
        let cover = s(&a["artworkUrl100"]).replace("100x100bb", "600x600bb");
        let id = a["collectionId"].clone();
        let url = a["collectionViewUrl"].as_str().map(String::from).unwrap_or(format!("https://music.apple.com/album/{id}"));
        json!({ "title": RE_SUFFIX_CASE.replace(&title, "").to_string(), "type": kind_of("", &title), "date": s(&a["releaseDate"]).chars().take(10).collect::<String>(),
                "cover": if cover.is_empty() { Value::Null } else { json!(cover) }, "tracks": a["trackCount"],
                "explicit": a["collectionExplicitness"] == "explicit", "sources": src("Apple Music", id, url) })
    }).collect())
}
/// " - Single" / " - EP" at the end of a title, as Apple writes it.
static RE_SUFFIX_CASE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+-\s+(Single|EP)$").unwrap());

/// A Discogs format's type: "Single", "EP", "Compilation" or "Album".
fn discogs_type(fmt: &str) -> &'static str {
    let f = fmt.to_lowercase();
    if f.contains("single") {
        "Single"
    } else if f.split(|c: char| c == ',' || c.is_whitespace()).any(|w| w == "ep") {
        "EP"
    } else if f.contains("comp") {
        "Compilation"
    } else {
        "Album"
    }
}

/// The artist's releases on Discogs (up to three pages), as catalogue entries.
async fn discogs_albums(st: &impl Ctx, aid: &str) -> anyhow::Result<Vec<Value>> {
    let mut out = vec![];
    for page in 1..=3 {
        let d: Value = discogs::get(st, &format!("artists/{aid}/releases"))
            .query(&[("sort", "year"), ("per_page", "100"), ("page", &page.to_string())]).send().await?.json().await?;
        for x in d["releases"].as_array().cloned().unwrap_or_default() {
            let typ = s(&x["type"]);
            if x["role"].as_str().unwrap_or("Main") != "Main" || !(typ == "master" || typ == "release") {
                continue;
            }
            let (pre, path) = if typ == "master" { ("m", "master") } else { ("r", "release") };
            let date = x["year"].as_i64().filter(|y| *y > 0).map(|y| y.to_string()).unwrap_or_default();
            out.push(json!({ "title": s(&x["title"]), "type": discogs_type(&s(&x["format"])), "date": date,
                "cover": x["thumb"].as_str().filter(|t| !t.is_empty()), "tracks": null, "explicit": false,
                "sources": src("Discogs", json!(format!("{pre}{}", x["id"])), format!("https://www.discogs.com/{path}/{}", x["id"])) }));
        }
        if page >= d["pagination"]["pages"].as_i64().unwrap_or(1) {
            break;
        }
    }
    Ok(out)
}

/// The first JSON-LD block of a page.
fn ld(html: &str) -> Value {
    let re = Regex::new(r#"(?s)<script type="application/ld\+json"[^>]*>(.*?)</script>"#).unwrap();
    for c in re.captures_iter(html) {
        if let Ok(d) = serde_json::from_str::<Value>(&c[1]) {
            if d["@type"] == "MusicAlbum" || d["@type"] == "MusicRecording" {
                return d;
            }
        }
    }
    json!({})
}

/// Month names as Bandcamp writes them.
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A Bandcamp date as YYYY-MM-DD.
fn bc_date(d: &str) -> String {
    // "05 Mar 2021 00:00:00 GMT" or "2021-03-05"
    let p: Vec<&str> = d.split_whitespace().collect();
    if p.len() >= 3 {
        if let (Ok(day), Some(m)) = (p[0].parse::<u32>(), MONTHS.iter().position(|m| *m == p[1])) {
            return format!("{}-{:02}-{day:02}", p[2], m + 1);
        }
    }
    if d.len() >= 10 && d.as_bytes()[4] == b'-' { d[..10].to_string() } else { String::new() }
}

/// A Bandcamp release as a catalogue entry, from its page's JSON-LD.
fn bc_album(url: &str, ld: &Value, fallback_title: &str, cover: Option<String>) -> Value {
    let n = ld["numTracks"].as_i64()
        .or_else(|| ld["track"]["itemListElement"].as_array().map(|a| a.len() as i64).filter(|n| *n > 0))
        .or(if ld["@type"] == "MusicRecording" { Some(1) } else { None });
    let title = ld["name"].as_str().map(String::from).unwrap_or(fallback_title.into());
    let img = match &ld["image"] { Value::Array(a) => a.first().and_then(|v| v.as_str()).map(String::from), Value::String(s) => Some(s.clone()), _ => None };
    let typ = if url.contains("/track/") || n == Some(1) { "Single" } else if n.is_some_and(|n| n <= 6) { "EP" } else { "Album" };
    json!({ "title": title, "type": typ, "date": bc_date(ld["datePublished"].as_str().unwrap_or("")), "cover": img.or(cover),
            "tracks": n, "explicit": false, "sources": src("Bandcamp", json!(url), url.into()) })
}

/// The releases on a Bandcamp site's /music page, as catalogue entries.
async fn bandcamp_albums(st: &impl Ctx, base: &str) -> anyhow::Result<Vec<Value>> {
    let r = st.http().get(format!("{base}/music")).send().await?;
    let final_url = r.url().to_string();
    let html = r.text().await?;
    if final_url.contains("/album/") || final_url.contains("/track/") {
        return Ok(vec![bc_album(&final_url, &ld(&html), "", None)]); // single-release artists land on the release itself
    }
    let a = Regex::new(r#"(?s)<a href="((?:https://[\w-]+\.bandcamp\.com)?/(?:album|track)/[^"?]+)"[^>]*>(.*?)</a>"#).unwrap();
    let t = Regex::new(r#"(?s)<p class="title">\s*(.*?)\s*(?:<br|</p>)"#).unwrap();
    let im = Regex::new(r#"<img[^>]+(?:data-original|src)="([^"]+)""#).unwrap();
    let ws = Regex::new(r"\s+").unwrap();
    let mut items: Vec<(String, String, Option<String>)> = vec![];
    for c in a.captures_iter(&html) {
        let url = if c[1].starts_with("http") { c[1].to_string() } else { format!("{base}{}", &c[1]) };
        if let Some(tt) = t.captures(&c[2]) {
            if !items.iter().any(|i| i.0 == url) {
                items.push((url, ws.replace_all(&tt[1], " ").trim().to_string(), im.captures(&c[2]).map(|m| m[1].to_string())));
            }
        }
    }
    use futures_util::StreamExt;
    Ok(futures_util::stream::iter(items.into_iter().take(30).map(|(url, title, img)| async move {
        let page = match st.http().get(&url).send().await { Ok(r) => r.text().await.unwrap_or_default(), Err(_) => String::new() };
        bc_album(&url, &ld(&page), &title, img)
    })).buffered(4).collect().await)
}

// ---- tracklists (requesting a release MusicBrainz doesn't have) --------------------------------

/// The tracks of a release MusicBrainz doesn't have, from the service the entry came from.
pub async fn tracklist(st: &impl Ctx, ext: &Value) -> anyhow::Result<Vec<Value>> {
    let t = |title: Value, artist: Value, length: Value, isrc: Value, n: Value, disc: Value| json!({
        "title": title, "artist": artist, "length": length, "isrc": isrc, "track": n, "disc": disc, "recording_id": null });
    if !ext["deezer"].is_null() {
        let id = ext["deezer"].to_string().trim_matches('"').to_string();
        let d = getj(deezer::album_tracks(st, &id, 500)).await?;
        return Ok(d["data"].as_array().cloned().unwrap_or_default().iter().enumerate().map(|(i, x)| t(json!(s(&x["title"])), json!(s(&x["artist"]["name"])),
            x["duration"].clone(), x["isrc"].clone(), x.get("track_position").filter(|v| !v.is_null()).cloned().unwrap_or(json!(i + 1)),
            x.get("disk_number").filter(|v| !v.is_null()).cloned().unwrap_or(json!(1)))).collect());
    }
    if !ext["apple"].is_null() {
        let id = ext["apple"].to_string().trim_matches('"').to_string();
        let d = getj(apple::lookup(st, &id, "song", Some(300))).await?;
        return Ok(d["results"].as_array().cloned().unwrap_or_default().iter().filter(|x| x["wrapperType"] == "track").map(|x| {
            let ms = x["trackTimeMillis"].as_f64().unwrap_or(0.0);
            t(json!(s(&x["trackName"])), json!(s(&x["artistName"])), if ms > 0.0 { json!(ms / 1000.0) } else { Value::Null }, Value::Null,
              x["trackNumber"].clone(), x.get("discNumber").filter(|v| !v.is_null()).cloned().unwrap_or(json!(1)))
        }).collect());
    }
    if let Some(url) = ext["bandcamp"].as_str() {
        let d = ld(&st.http().get(url).send().await?.text().await?);
        let items = d["track"]["itemListElement"].as_array().cloned()
            .unwrap_or_else(|| if d["@type"] == "MusicRecording" { vec![json!({ "position": 1, "item": d })] } else { vec![] });
        return Ok(items.iter().enumerate().map(|(i, x)| t(json!(s(&x["item"]["name"])), json!(""), dur(&x["item"]["duration"]), Value::Null,
            x.get("position").filter(|v| !v.is_null()).cloned().unwrap_or(json!(i + 1)), json!(1))).collect());
    }
    if let Some(id) = ext["discogs"].as_str() {
        let kind = if id.starts_with('m') { "masters" } else { "releases" };
        let d: Value = discogs::get(st, &format!("{kind}/{}", &id[1..])).send().await?.json().await?;
        return Ok(d["tracklist"].as_array().cloned().unwrap_or_default().iter().filter(|x| x["type_"].as_str().unwrap_or("track") == "track").enumerate()
            .map(|(i, x)| t(json!(s(&x["title"])), json!(x["artists"].as_array().map(|a| a.iter().map(|a| s(&a["name"])).collect::<Vec<_>>().join(", ")).unwrap_or_default()),
                dur(&x["duration"]), Value::Null, json!(i + 1), json!(1))).collect());
    }
    Ok(vec![])
}

/// The library copy of a song from another catalogue: same ISRC, else same title and artist (and, when
/// both lengths are known, within 3 seconds). Best quality first.
pub fn library_track_for(db: &Db, isrc: &str, artist: &str, title: &str, length: Option<f64>) -> Option<Row> {
    const COLS: &str = "SELECT id, title, artist, album, album_id, format, duration FROM tracks";
    const ORDER: &str = " ORDER BY CASE WHEN format IN ('flac','alac','wav','aiff') THEN 0 ELSE 1 END";
    let isrc = isrc.to_uppercase().replace('-', "");
    if !isrc.is_empty() {
        if let Ok(Some(r)) = db.one(&format!("{COLS} WHERE upper(replace(isrc,'-',''))=?{ORDER} LIMIT 1"), p![isrc]) {
            return Some(r);
        }
    }
    if title.is_empty() {
        return None;
    }
    let rows = db.query(&format!("{COLS} WHERE lower(title)=lower(?) AND (lower(artist)=lower(?) OR lower(albumartist)=lower(?)){ORDER} LIMIT 20"), p![title, artist, artist]).ok()?;
    match length.filter(|l| *l > 0.0) {
        Some(l) => rows.into_iter().find(|r| { let d = r.f("duration"); d <= 0.0 || (d - l).abs() <= 3.0 }),
        None => rows.into_iter().next(),
    }
}

/// What "Request discography" asks for: the MusicBrainz release groups the release-type filter wants
/// (kinds, minus the ticked extras: live, compilation …) and the Deezer / Apple Music albums MusicBrainz is
/// missing (their types read from record type and title), minus what `skip` says is owned or already
/// requested. An ext album that is a MusicBrainz release under the same normalized title (released within
/// a year of it, skipped type or not) is left out, as is a second copy of the same ext album.
///
/// With `only` – the releases ticked in the dialog, by id – the type filter is not consulted: exactly those,
/// minus the owned and requested ones, and minus an ext copy of a MusicBrainz release that is planned too.
pub fn discography_plan(groups: &[Value], ext: &[Value], filter: &ReleaseFilter, only: Option<&[String]>, skip: impl Fn(&Value) -> bool) -> Vec<Value> {
    let year = |v: &Value| s(&v["year"]).chars().take(4).collect::<String>().parse::<i32>().ok()
        .or_else(|| s(&v["date"]).chars().take(4).collect::<String>().parse::<i32>().ok());
    // as in `fuzzy`: a title-track single isn't the album of the same name
    let same = |a: &Value, b: &Value| title_key(&s(&a["title"])) == title_key(&s(&b["title"])) && !title_key(&s(&a["title"])).is_empty()
        && { let (ta, tb) = (s(&a["type"]), s(&b["type"])); ta == tb || (ta != "Single" && tb != "Single") }
        && match (year(a), year(b)) { (Some(x), Some(y)) => (x - y).abs() <= 1, _ => true };
    let wanted = |v: &Value| match only {
        Some(ids) => ids.iter().any(|i| *i == s(&v["id"])),
        None => filter.wants(v),
    };
    let mut out: Vec<Value> = vec![];
    for g in groups {
        if !wanted(g) || skip(g) {
            continue;
        }
        out.push(g.clone());
    }
    for e in ext {
        if !e["ext"].is_object() || !wanted(e) {
            continue;
        }
        // MusicBrainz has it: with the settings' filter any group counts (owned or skipped too); with ticked
        // ids only a group that is being requested, so a tick on the ext copy alone is honoured
        let in_mb = match only {
            Some(_) => out.iter().any(|o| !o["ext"].is_object() && same(o, e)),
            None => groups.iter().any(|g| same(g, e)),
        };
        if in_mb || out.iter().any(|o| o["ext"].is_object() && same(o, e)) || skip(e) {
            continue;
        }
        out.push(e.clone());
    }
    out
}

/// The clients' Request discography dialog: `items` (MusicBrainz release groups, or the Deezer / Apple
/// albums MusicBrainz is missing) grouped by canonical kind in the clients' order, each release carrying
/// `kind` and `allowed` (whether the Release types settings want it). → `[{kind, label, releases}]`.
pub fn grouped(filter: &ReleaseFilter, items: &[Value]) -> Vec<Value> {
    let mut groups: Vec<(&'static str, Vec<Value>)> = vec![];
    for it in items {
        let kind = encore_config::release_types::group_kind(it);
        let mut r = it.clone();
        r["kind"] = json!(kind);
        r["allowed"] = json!(filter.wants(it));
        match groups.iter_mut().find(|(k, _)| *k == kind) {
            Some(g) => g.1.push(r),
            None => groups.push((kind, vec![r])),
        }
    }
    groups.sort_by_key(|(k, _)| encore_config::release_types::group_rank(k));
    groups.into_iter().map(|(k, releases)| json!({ "kind": k, "label": encore_config::release_types::label(k), "releases": releases })).collect()
}


// ---- merging --------------------------------------------------------------------------------

/// The release group a service's release is: same type (EPs and singles may meet), and a title that matches (a close one in the same year).
fn fuzzy<'a>(ext: &Value, rgs: &'a [Value]) -> Option<&'a Value> {
    let y: String = s(&ext["date"]).chars().take(4).collect();
    let et = s(&ext["type"]);
    rgs.iter().find(|rg| {
        let rt = s(&rg["type"]);
        // a title-track single isn't the album (Deezer and Apple call many EPs singles: those two may meet)
        let small = |t: &str| t == "Single" || t == "EP";
        if (et == "Single" || rt == "Single") && et != rt && !(small(&et) && small(&rt)) {
            return false;
        }
        let sim = similar(&s(&ext["title"]), &s(&rg["title"]));
        sim >= 0.92 || (sim >= 0.8 && !y.is_empty() && s(&rg["year"]) == y)
    })
}

/// What the deep matching may still ask per artist.
struct Budget {
    /// Deezer album lookups left
    deep: i32,
    /// MusicBrainz ISRC lookups left
    isrc: i32,
}

/// Match a Deezer release by its barcode, else by its first ISRC's release groups. True when it is one MusicBrainz has.
async fn deep_match(st: &impl Ctx, ext: &mut Value, barcodes: &HashMap<String, String>, rgs_by_id: &HashMap<String, Value>, b: &mut Budget) -> anyhow::Result<bool> {
    let Some(dz) = ext["sources"].as_array().and_then(|a| a.iter().find(|s| s["name"] == "Deezer")).map(|s| s["id"].to_string()) else { return Ok(false) };
    if b.deep <= 0 {
        return Ok(false);
    }
    b.deep -= 1;
    let d = getj(deezer::album(st, &dz)).await?;
    let upc = s(&d["upc"]);
    ext["upc"] = d["upc"].clone();
    if barcodes.contains_key(upc.trim_start_matches('0')) && !upc.is_empty() {
        return Ok(true);
    }
    let tracks = d["tracks"]["data"].as_array().cloned().unwrap_or_default();
    let mut isrc = tracks.iter().find_map(|t| t["isrc"].as_str().map(String::from));
    if isrc.is_none() {
        if let Some(first) = tracks.first() {
            isrc = getj(deezer::track(st, &first["id"])).await?["isrc"].as_str().map(String::from);
        }
    }
    if let Some(isrc) = isrc {
        if b.isrc > 0 {
            b.isrc -= 1;
            for g in mb::release_groups_for_isrc(st, &isrc).await {
                if let Some(rg) = rgs_by_id.get(&g) {
                    if similar(&s(&ext["title"]), &s(&rg["title"])) >= 0.6 {
                        return Ok(true); // same song on a single vs. album isn't the same release
                    }
                }
            }
        }
    }
    Ok(false)
}

/// The services merged onto MusicBrainz: (their name in `sources`, their key).
const EXT_KEYS: [(&str, &str); 4] = [("Deezer", "deezer"), ("Apple Music", "apple"), ("Bandcamp", "bandcamp"), ("Discogs", "discogs")];

/// The artist's catalogue: MusicBrainz's release groups plus the releases only Deezer, Apple Music, Bandcamp or Discogs have (cached).
pub async fn merged(st: &impl Ctx, artist: &Value) -> Vec<Value> {
    let aid = s(&artist["id"]);
    let key = format!("catalog:{aid}");
    if let Some(v) = st.cache_get(&key) {
        return v.as_array().cloned().unwrap_or_default();
    }
    let rgs = artist["release_groups"].as_array().cloned().unwrap_or_default();
    let urls: Vec<String> = artist["urls"].as_array().cloned().unwrap_or_default().iter().map(s).collect();
    let name = s(&artist["name"]);
    let (dz, ap, bc, dc) = tokio::join!(deezer_artist(st, &name, &urls), apple_artist(st, &name, &urls), bandcamp_artist(st, &name, &urls), discogs_artist(st, &name, &urls));
    let ids = [dz.ok().flatten(), ap.ok().flatten(), bc.ok().flatten(), dc.ok().flatten()];
    async fn opt<F: std::future::Future<Output = anyhow::Result<Vec<Value>>>>(f: Option<F>) -> Vec<Value> {
        match f { Some(f) => f.await.unwrap_or_default(), None => vec![] }
    }
    let (l0, l1, l2, l3, barcodes) = tokio::join!(
        opt(ids[0].as_deref().map(|i| deezer_albums(st, i))),
        opt(ids[1].as_deref().map(|i| apple_albums(st, i))),
        opt(ids[2].as_deref().map(|i| bandcamp_albums(st, i))),
        opt(ids[3].as_deref().map(|i| discogs_albums(st, i))),
        mb::artist_barcodes(st, &aid),
    );
    let barcodes = barcodes.unwrap_or_default();
    let mut lists = [l0, l1, l2, l3];
    // a name-only artist match must share at least one release with MusicBrainz, or it's someone else
    for (domain, lst) in ["deezer.com", "apple.com", "bandcamp.com", "discogs.com"].iter().zip(lists.iter_mut()) {
        if !lst.is_empty() && !rgs.is_empty() && !urls.iter().any(|u| u.contains(domain)) && !lst.iter().any(|x| fuzzy(x, &rgs).is_some()) {
            lst.clear();
        }
    }
    let rgs_by_id: HashMap<String, Value> = rgs.iter().map(|g| (s(&g["id"]), g.clone())).collect();
    let mut budget = Budget { deep: MAX_DEEP, isrc: MAX_ISRC };
    let mut missing: Vec<Value> = vec![];
    for mut ext in lists.into_iter().flatten() {
        if fuzzy(&ext, &rgs).is_some() {
            continue;
        }
        // same release from another service → one entry with both sources
        let names = |v: &Value| v["sources"].as_array().cloned().unwrap_or_default().iter().map(|x| s(&x["name"])).collect::<Vec<_>>();
        let twin = missing.iter_mut().find(|m| similar(&s(&m["title"]), &s(&ext["title"])) >= 0.92
            && (m["type"] == ext["type"] || names(m).iter().chain(names(&ext).iter()).any(|n| n == "Discogs")));
        if let Some(t) = twin {
            let first = ext["sources"][0].clone();
            if !names(t).contains(&s(&first["name"])) {
                t["sources"].as_array_mut().unwrap().push(first);
            }
            if t["cover"].is_null() { t["cover"] = ext["cover"].clone(); }
            if t["tracks"].is_null() { t["tracks"] = ext["tracks"].clone(); }
            if s(&t["date"]).len() < s(&ext["date"]).len() { t["date"] = ext["date"].clone(); }
            continue;
        }
        if deep_match(st, &mut ext, &barcodes, &rgs_by_id, &mut budget).await.unwrap_or(false) {
            continue;
        }
        missing.push(ext);
    }
    for m in missing.iter_mut() {
        let srcs = m["sources"].as_array().cloned().unwrap_or_default();
        let id_of = |x: &Value| x["id"].as_str().map(String::from).unwrap_or(x["id"].to_string());
        m["id"] = json!(format!("ext-{}", srcs.iter().map(|x| format!("{}{}", s(&x["name"]).chars().next().unwrap_or('x').to_lowercase(), id_of(x))).collect::<Vec<_>>().join("-")));
        m["year"] = json!(s(&m["date"]).chars().take(4).collect::<String>());
        m["artist"] = json!(name);
        let mut hurls: Vec<String> = srcs.iter().filter(|x| x["name"] != "Discogs").map(|x| s(&x["url"])).collect();
        if hurls.is_empty() {
            hurls.push(s(&srcs[0]["url"]));
        }
        m["harmony"] = json!(harmony_url(&hurls, m["upc"].as_str()));
        let mut ext = serde_json::Map::new();
        for x in &srcs {
            if let Some((_, k)) = EXT_KEYS.iter().find(|(n, _)| *n == s(&x["name"])) {
                ext.insert(k.to_string(), x["id"].clone());
            }
        }
        if let Some(u) = m["upc"].as_str().filter(|u| !u.is_empty()) {
            ext.insert("upc".into(), json!(u));
        }
        m["ext"] = Value::Object(ext);
    }
    missing.sort_by(|a, b| s(&b["date"]).cmp(&s(&a["date"])));
    st.cache_put(&key, json!(missing), 86400);
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn titles() {
        assert_eq!(title_key("Folklore (Deluxe Edition)"), "folklore");
        assert_eq!(title_key("Hello - Single"), "hello");
        assert_eq!(title_key("Café (feat. Someone) [Remastered]"), "cafe");
        assert_eq!(title_key("Song ft. Guest"), "song");
        assert_eq!(title_key("With You"), "withyou");
        // a live edition is another album
        assert_ne!(title_key("Rodeo (Live Version)"), title_key("Rodeo"));
        assert!(similar("Rock & Roll", "rock and roll") == 1.0);
        let r = similar("abcd", "bcde");
        assert!((r - 0.75).abs() < 1e-9, "{r}");
        assert_eq!(bc_date("05 Mar 2021 00:00:00 GMT"), "2021-03-05");
        assert_eq!(dur(&json!("PT3M45S")), json!(225.0));
        assert_eq!(dur(&json!("3:45")), json!(225.0));
    }

    #[test]
    fn discography_includes_ext_albums_and_skips_owned() {
        let groups = vec![
            json!({ "id": "rg-rodeo", "title": "Rodeo", "type": "Album", "year": "2015", "secondary": [] }),
            json!({ "id": "rg-owned", "title": "Birds in the Trap", "type": "Album", "year": "2016", "secondary": [] }),
            json!({ "id": "rg-live", "title": "Live at Rolling Loud", "type": "Album", "year": "2019", "secondary": ["Live"] }),
            json!({ "id": "rg-single", "title": "90210", "type": "Single", "year": "2015", "secondary": [] }),
        ];
        let ext = vec![
            // the same album on Deezer (a year off): MusicBrainz has it
            json!({ "id": "ext-d1", "title": "Rodeo (Deluxe)", "type": "Album", "year": "2016", "ext": { "deezer": 1 } }),
            // only on Deezer and Apple
            json!({ "id": "ext-d2-a3", "title": "Days Before Rodeo", "type": "Album", "year": "2014", "ext": { "deezer": 2, "apple": 3 } }),
            json!({ "id": "ext-a4", "title": "Days Before Rodeo", "type": "Album", "year": "2014", "ext": { "apple": 4 } }),
            // owned already
            json!({ "id": "ext-d5", "title": "Owned Mixtape", "type": "EP", "year": "2012", "ext": { "deezer": 5 } }),
            json!({ "id": "ext-d6", "title": "A Compilation", "type": "Compilation", "year": "2012", "ext": { "deezer": 6 } }),
            json!({ "id": "ext-d7", "title": "Bonus Single", "type": "Single", "year": "2020", "ext": { "deezer": 7 } }),
        ];
        let owned = |v: &Value| ["rg-owned", "ext-d5"].contains(&s(&v["id"]).as_str());
        let ids = |p: Vec<Value>| p.iter().map(|v| s(&v["id"])).collect::<Vec<_>>();
        let plan = ids(discography_plan(&groups, &ext, &kinds("Album,EP"), None, owned));
        assert_eq!(plan, vec!["rg-rodeo", "ext-d2-a3"]);
        let plan = ids(discography_plan(&groups, &ext, &kinds("Album,EP,Single"), None, owned));
        assert_eq!(plan, vec!["rg-rodeo", "rg-single", "ext-d2-a3", "ext-d7"]);
        // live albums asked for this time: MusicBrainz's live album comes along, compilations still don't
        let f = ReleaseFilter::from_value(&json!({ "skip": ["Compilation"] }), &kinds("Album,EP"));
        assert_eq!(ids(discography_plan(&groups, &ext, &f, None, owned)), vec!["rg-rodeo", "rg-live", "ext-d2-a3"]);
    }

    #[test]
    fn ticked_release_ids_are_requested_as_they_are() {
        let groups = vec![
            json!({ "id": "rg-rodeo", "title": "Rodeo", "type": "Album", "year": "2015", "secondary": [] }),
            json!({ "id": "rg-owned", "title": "Birds in the Trap", "type": "Album", "year": "2016", "secondary": [] }),
            json!({ "id": "rg-live", "title": "Live at Rolling Loud", "type": "Album", "year": "2019", "secondary": ["Live"] }),
            json!({ "id": "rg-single", "title": "90210", "type": "Single", "year": "2015", "secondary": [] }),
        ];
        let ext = vec![
            json!({ "id": "ext-d1", "title": "Rodeo (Deluxe)", "type": "Album", "year": "2016", "ext": { "deezer": 1 } }),
            json!({ "id": "ext-d2-a3", "title": "Days Before Rodeo", "type": "Album", "year": "2014", "ext": { "deezer": 2, "apple": 3 } }),
            json!({ "id": "ext-a4", "title": "Days Before Rodeo", "type": "Album", "year": "2014", "ext": { "apple": 4 } }),
        ];
        let owned = |v: &Value| s(&v["id"]) == "rg-owned";
        let ids = |p: Vec<Value>| p.iter().map(|v| s(&v["id"])).collect::<Vec<_>>();
        let only = |l: &[&str]| l.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        // the ticked live album and single come along although the settings leave them out; the owned one
        // doesn't, and the second copy of the same Deezer / Apple album is one request
        let t = only(&["rg-live", "rg-single", "rg-owned", "ext-d2-a3", "ext-a4"]);
        assert_eq!(ids(discography_plan(&groups, &ext, &kinds("Album,EP"), Some(&t), owned)), vec!["rg-live", "rg-single", "ext-d2-a3"]);
        // the Deezer copy of a MusicBrainz album: one request when both are ticked, itself when only it is
        let t = only(&["rg-rodeo", "ext-d1"]);
        assert_eq!(ids(discography_plan(&groups, &ext, &kinds("Album,EP"), Some(&t), owned)), vec!["rg-rodeo"]);
        let t = only(&["ext-d1"]);
        assert_eq!(ids(discography_plan(&groups, &ext, &kinds("Album,EP"), Some(&t), owned)), vec!["ext-d1"]);
        // nothing ticked that is new (or an id the page never had): nothing
        assert!(discography_plan(&groups, &ext, &kinds("Album,EP"), Some(&only(&["rg-owned", "nope"])), owned).is_empty());
    }

    #[test]
    fn the_dialog_groups_releases_by_kind_with_their_allowed_flag() {
        let items = vec![
            json!({ "id": "rg-single", "title": "90210", "type": "Single", "year": "2015", "secondary": [] }),
            json!({ "id": "rg-rodeo", "title": "Rodeo", "type": "Album", "year": "2015", "secondary": [] }),
            json!({ "id": "rg-live", "title": "Live at Rolling Loud", "type": "Album", "year": "2019", "secondary": ["Live"] }),
            json!({ "id": "rg-ep", "title": "Owl Pharaoh", "type": "EP", "year": "2013", "secondary": [] }),
            json!({ "id": "ext-d1", "title": "Hits", "type": "Compilation", "year": "2020", "ext": { "deezer": 1 } }),
            json!({ "id": "ext-d2", "title": "Days Before Rodeo", "type": "Album", "year": "2014", "ext": { "deezer": 2 } }),
        ];
        let g = grouped(&kinds("Album,EP"), &items);
        assert_eq!(g.iter().map(|x| s(&x["kind"])).collect::<Vec<_>>(), vec!["Album", "EP", "Single", "Compilation", "Live"]);
        let of = |k: &str| g.iter().find(|x| x["kind"] == k).unwrap()["releases"].as_array().unwrap().iter()
            .map(|r| (s(&r["id"]), r["allowed"].as_bool().unwrap(), s(&r["kind"]))).collect::<Vec<_>>();
        assert_eq!(of("Album"), vec![("rg-rodeo".into(), true, "Album".into()), ("ext-d2".into(), true, "Album".into())]);
        assert_eq!(of("EP"), vec![("rg-ep".into(), true, "EP".into())]);
        assert_eq!(of("Single"), vec![("rg-single".into(), false, "Single".into())]);
        assert_eq!(of("Compilation"), vec![("ext-d1".into(), false, "Compilation".into())]);
        assert_eq!(of("Live"), vec![("rg-live".into(), false, "Live".into())]);
        assert_eq!(g[3]["label"], "Compilation");
        // singles on and live albums not skipped: the flags follow the settings
        let f = ReleaseFilter::from_csv("Album,EP,Single", "Compilation");
        let g = grouped(&f, &items);
        let allowed = |k: &str| g.iter().find(|x| x["kind"] == k).unwrap()["releases"][0]["allowed"].as_bool().unwrap();
        assert!(allowed("Single") && allowed("Live") && !allowed("Compilation"));
    }


    #[test]
    fn a_title_track_single_is_not_the_album() {
        // MusicBrainz only has the single "Utopia"; Deezer has the album "Utopia" too, and its own single
        let groups = vec![json!({ "id": "rg-single", "title": "Utopia", "type": "Single", "year": "2020", "secondary": [] })];
        let ext = vec![
            json!({ "id": "ext-d1", "title": "Utopia", "type": "Album", "year": "2020", "ext": { "deezer": 1 } }),
            json!({ "id": "ext-d2", "title": "Utopia", "type": "Single", "year": "2020", "ext": { "deezer": 2 } }),
        ];
        let ids = |p: Vec<Value>| p.iter().map(|v| s(&v["id"])).collect::<Vec<_>>();
        assert_eq!(ids(discography_plan(&groups, &ext, &kinds("Album,EP"), None, |_| false)), vec!["ext-d1"]);
        assert_eq!(ids(discography_plan(&groups, &ext, &kinds("Album,EP,Single"), None, |_| false)), vec!["rg-single", "ext-d1"]);

    }

    fn kinds(k: &str) -> ReleaseFilter {
        ReleaseFilter::from_csv(k, &encore_config::release_types::default_skip())
    }

    #[test]
    fn discography_skips_ext_albums_by_their_title_words() {
        let groups = vec![json!({ "id": "rg-rodeo", "title": "Rodeo", "type": "Album", "year": "2015", "secondary": [] })];
        let ext = vec![
            json!({ "id": "ext-d1", "title": "Rodeo (Live at Coachella)", "type": "Album", "year": "2017", "ext": { "deezer": 1 } }),
            json!({ "id": "ext-d2", "title": "Astroworld (The Remixes)", "type": "Album", "year": "2019", "ext": { "deezer": 2 } }),
            json!({ "id": "ext-d3", "title": "Utopia (Instrumentals)", "type": "Album", "year": "2023", "ext": { "deezer": 3 } }),
            json!({ "id": "ext-d4", "title": "Astroworld (Remastered)", "type": "Album", "year": "2018", "ext": { "deezer": 4 } }),
            json!({ "id": "ext-d5", "title": "Hits", "type": "Compilation", "year": "2020", "ext": { "deezer": 5 } }),
        ];
        let ids = |p: Vec<Value>| p.iter().map(|v| s(&v["id"])).collect::<Vec<_>>();
        assert_eq!(ids(discography_plan(&groups, &ext, &kinds("Album,EP"), None, |_| false)), vec!["rg-rodeo", "ext-d4"]);
        let f = ReleaseFilter::from_csv("Album", "Compilation");
        assert_eq!(ids(discography_plan(&groups, &ext, &f, None, |_| false)), vec!["rg-rodeo", "ext-d1", "ext-d2", "ext-d3", "ext-d4"]);

    }
}
