//! MusicBrainz web service: rate limited (1 request/second by default, every caller in one line that serves
//! the most urgent first – [`crate::mb_line`]) and cached for an hour; and the calls that always go to
//! musicbrainz.org itself (the account sign-in, ISRC submissions).
use crate::Ctx;
use encore_core::json::s;
use reqwest::RequestBuilder;
use serde_json::{json, Value};
use encore_core::cache::{json_bytes, ByteCache, CacheStats};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub use crate::mb_line::{one_place, one_place_within, priority, with_priority, Priority};
use crate::mb_line::Line;

#[derive(Clone)]
pub struct Mb {
    /// the line every call waits in for its turn
    line: Line,
    /// answers by path and query, for an hour, held to `CACHE_BYTES`
    cache: Arc<ByteCache<Value>>,
}

/// musicbrainz.org itself: signing in to an account and edits go there, never to a mirror (`musicbrainz_url`).
pub const MAIN: &str = "https://musicbrainz.org";

/// The OAuth2 token call (`form`: grant_type with a code or a refresh token, client_id, client_secret …).
pub fn oauth_token(st: &impl Ctx, form: &[(&str, &str)]) -> RequestBuilder {
    st.http().post(format!("{MAIN}/oauth2/token")).form(form)
}

/// Who an access token belongs to (`sub`: the account name).
pub fn userinfo(st: &impl Ctx, token: &str) -> RequestBuilder {
    st.http().get(format!("{MAIN}/oauth2/userinfo")).bearer_auth(token)
}

/// Submit ISRCs (`xml`: a `<metadata>` with one `<recording>` per recording) as the account of `token`;
/// `client` names the program (`encore-<version>`).
pub fn submit_isrcs(st: &impl Ctx, token: &str, client: &str, xml: String) -> RequestBuilder {
    st.http().post(format!("{MAIN}/ws/2/recording")).query(&[("client", client)]).bearer_auth(token)
        .header("Content-Type", "application/xml; charset=utf-8").body(xml)
}

/// MusicBrainz said it was busy (503) every time a call asked: a reason to ask again later, unlike a
/// failure (a refused connection, a 500).
#[derive(Debug)]
pub struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MusicBrainz is rate limiting requests, try again shortly")
    }
}

impl std::error::Error for Busy {}

/// Whether a call failed because MusicBrainz was busy ([`Busy`]).
pub fn is_busy(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<Busy>())
}

/// Whether MusicBrainz said it doesn't have the thing asked for (404, or 400 for an id it can't read).
pub fn is_not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<reqwest::Error>().and_then(|r| r.status()).is_some_and(|s| s.as_u16() == 404 || s.as_u16() == 400))
}

/// The time between two calls to MusicBrainz: `musicbrainz_rate` calls a second for the whole server.
pub fn gap(st: &impl Ctx) -> Duration {
    Duration::from_secs_f64(1.0 / st.cfg().float("musicbrainz_rate").max(0.1))
}

const TTL: Duration = Duration::from_secs(3600);
/// Most bytes the cached MusicBrainz answers may take in memory.
pub const CACHE_BYTES: usize = 16 << 20;

impl Default for Mb {
    fn default() -> Self {
        Mb { line: Line::default(), cache: Arc::new(ByteCache::new(CACHE_BYTES, 3000)) }
    }
}

impl Mb {
    /// What the answer cache holds (`/api/admin/perf`).
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Who waits in the line at each priority (`/api/admin/perf`).
    pub fn line_stats(&self) -> Value {
        self.line.stats()
    }

    /// How many calls wait at `p` or a more urgent priority: what a call at `p` made now has ahead of it.
    pub fn ahead_of(&self, p: Priority) -> usize {
        self.line.ahead_of(p)
    }

    /// The answer kept for this call, without asking MusicBrainz.
    pub fn cached(&self, path: &str, params: &[(&str, String)]) -> Option<Value> {
        self.cache.get(&cache_key(path, params))
    }

    /// One call: the hour-long cache, else a turn in the line at the caller's [`priority`] (inside [`one_place`]
    /// at the place of the work's first call) and the call. A 503 (MusicBrainz says it is busy) waits 1.5, 3,
    /// 4.5 s and joins the line again at the place it had; the fourth one gives up with [`Busy`].
    pub async fn get(&self, st: &impl Ctx, path: &str, params: &[(&str, String)]) -> anyhow::Result<Value> {
        let key = cache_key(path, params);
        if let Some(v) = self.cache.get(&key) {
            return Ok(v);
        }
        let mut ps: Vec<(String, String)> = params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        ps.push(("fmt".into(), "json".into()));
        let base = st.cfg().str("musicbrainz_url");
        let base = if base.is_empty() { "https://musicbrainz.org".to_string() } else { base.trim_end_matches('/').to_string() };
        let gap = gap(st);
        let contact = st.cfg().str("contact_email");
        let ua = format!("Encore/{} ( {} )", encore_core::VERSION, if contact.is_empty() { "https://github.com/Krifmaz/Encore".into() } else { contact });
        let class = priority();
        let mut place = None;
        for attempt in 0..4 {
            place = Some(self.line.turn(class, gap, place).await);
            let r = st.http().get(format!("{base}/ws/2/{path}")).query(&ps).header("User-Agent", &ua).send().await?;
            if r.status().as_u16() == 503 {
                tokio::time::sleep(Duration::from_millis(1500 * (attempt + 1))).await;
                continue;
            }
            let v: Value = r.error_for_status()?.json().await?;
            let bytes = key.len() + json_bytes(&v);
            self.cache.insert(key, v.clone(), bytes, Some(TTL), 0);
            return Ok(v);
        }
        Err(Busy.into())
    }
}

/// The cache key of a call: its path and its parameters in order (`fmt=json` among them).
fn cache_key(path: &str, params: &[(&str, String)]) -> String {
    let mut ps: Vec<(String, String)> = params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    ps.push(("fmt".into(), "json".into()));
    ps.sort();
    format!("{path}?{}", ps.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"))
}

pub fn esc(s: &str) -> String {
    s.chars().flat_map(|c| if "+-&|!(){}[]^\"~*?:\\/".contains(c) { vec!['\\', c] } else { vec![c] }).collect()
}

pub fn credit(ac: &Value) -> String {
    ac.as_array().map(|a| a.iter().map(|c| {
        let n = c.get("name").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(String::from).unwrap_or_else(|| s(&c["artist"]["name"]));
        n + c.get("joinphrase").and_then(|x| x.as_str()).unwrap_or("")
    }).collect()).unwrap_or_default()
}

/// Every artist of a credit, as credited, with its MusicBrainz id: `[{name, mbid}]` (what Encore writes into the
/// ARTISTS tags and keeps in `mb_credits`).
pub fn credit_list(ac: &Value) -> Value {
    Value::Array(ac.as_array().map(|a| a.iter().filter_map(|c| {
        let n = c.get("name").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(String::from).unwrap_or_else(|| s(&c["artist"]["name"]));
        (!n.trim().is_empty()).then(|| json!({ "name": n.trim(), "mbid": s(&c["artist"]["id"]) }))
    }).collect()).unwrap_or_default())
}

pub fn credit_id(ac: &Value) -> Value {
    ac.as_array().and_then(|a| a.first()).and_then(|c| c["artist"].get("id")).cloned().unwrap_or(Value::Null)
}

pub use crate::coverartarchive::{cover_release, cover_rg};

fn top_tags(v: &Value, n: usize) -> Vec<String> {
    let mut t: Vec<(i64, String)> = v.as_array().map(|a| a.iter().map(|x| (x["count"].as_i64().unwrap_or(0), s(&x["name"]))).collect()).unwrap_or_default();
    t.sort_by_key(|x| std::cmp::Reverse(x.0));
    t.into_iter().take(n).map(|x| x.1).collect()
}

pub fn rg(v: &Value) -> Value {
    let id = s(&v["id"]);
    let date = s(&v["first-release-date"]);
    json!({
        "id": id, "title": s(&v["title"]), "artist": credit(&v["artist-credit"]), "artist_id": credit_id(&v["artist-credit"]),
        "type": s(&v["primary-type"]), "secondary": v.get("secondary-types").cloned().unwrap_or(json!([])),
        "disambiguation": s(&v["disambiguation"]),
        "year": date.chars().take(4).collect::<String>(), "date": date, "score": v.get("score").cloned().unwrap_or(json!(0)),
        "cover": cover_rg(&id, 250),
    })
}

pub async fn search_artists(st: &impl Ctx, q: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
    let d = st.mb().get(st, "artist", &[("query", q.into()), ("limit", limit.to_string())]).await?;
    Ok(d["artists"].as_array().cloned().unwrap_or_default().iter().map(|a| json!({
        "id": s(&a["id"]), "name": s(&a["name"]), "disambiguation": s(&a["disambiguation"]), "type": s(&a["type"]), "country": s(&a["country"]),
        "score": a.get("score").cloned().unwrap_or(json!(0)), "begin": s(&a["life-span"]["begin"]), "tags": top_tags(&a["tags"], 4),
    })).collect())
}

pub async fn search_albums(st: &impl Ctx, q: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
    let d = st.mb().get(st, "release-group", &[("query", q.into()), ("limit", limit.to_string())]).await?;
    Ok(d["release-groups"].as_array().cloned().unwrap_or_default().iter().map(rg).collect())
}

pub async fn search_recordings(st: &impl Ctx, q: &str, limit: usize, artist: Option<&str>) -> anyhow::Result<Vec<Value>> {
    let query = match artist {
        Some(a) => format!("recording:\"{}\" AND artist:\"{}\"", esc(q), esc(a)),
        None => q.to_string(),
    };
    let d = st.mb().get(st, "recording", &[("query", query), ("limit", limit.to_string())]).await?;
    Ok(d["recordings"].as_array().cloned().unwrap_or_default().iter().map(|r| {
        let rel = r["releases"].as_array().and_then(|a| a.first()).cloned().unwrap_or(json!({}));
        let rgid = s(&rel["release-group"]["id"]);
        let year: String = { let d = s(&rel["date"]); if d.is_empty() { s(&r["first-release-date"]) } else { d } }.chars().take(4).collect();
        let (rgv, cover) = if rgid.is_empty() { (Value::Null, Value::Null) } else { (json!(rgid), json!(cover_rg(&rgid, 250))) };
        json!({
            "id": s(&r["id"]), "title": s(&r["title"]), "artist": credit(&r["artist-credit"]), "artist_id": credit_id(&r["artist-credit"]),
            "length": r["length"].as_f64().unwrap_or(0.0) / 1000.0, "album": s(&rel["title"]), "release_id": rel.get("id").cloned().unwrap_or(Value::Null),
            "release_group_id": rgv, "year": year,
            "score": r.get("score").cloned().unwrap_or(json!(0)), "cover": cover,
        })
    }).collect())
}

pub async fn artist(st: &impl Ctx, mbid: &str) -> anyhow::Result<Value> {
    let a = st.mb().get(st, &format!("artist/{mbid}"), &[("inc", "tags+url-rels".into())]).await?;
    let name = s(&a["name"]);
    let mut rgs = vec![];
    let mut offset = 0;
    loop {
        let page = st.mb().get(st, "release-group", &[("artist", mbid.into()), ("limit", "100".into()), ("offset", offset.to_string())]).await?;
        rgs.extend(page["release-groups"].as_array().cloned().unwrap_or_default());
        offset += 100;
        if offset >= page["release-group-count"].as_i64().unwrap_or(0) || offset >= 2000 {
            break;
        }
    }
    let mut groups: Vec<Value> = rgs.into_iter().map(|mut g| {
        g["artist-credit"] = json!([{ "name": name, "artist": { "id": mbid } }]);
        rg(&g)
    }).collect();
    groups.sort_by(|x, y| {
        let k = |v: &Value| { let y = v["year"].as_str().unwrap_or(""); if y.is_empty() { "9999".to_string() } else { y.to_string() } };
        k(y).cmp(&k(x))
    });
    let mut links = serde_json::Map::new();
    let mut urls = vec![];
    for rel in a["relations"].as_array().cloned().unwrap_or_default() {
        if let Some(u) = rel["url"]["resource"].as_str() {
            links.insert(rel["type"].as_str().unwrap_or("link").to_string(), json!(u));
            urls.push(json!(u));
        }
    }
    Ok(json!({
        "id": s(&a["id"]), "name": name, "disambiguation": s(&a["disambiguation"]), "type": s(&a["type"]), "country": s(&a["country"]),
        "begin": s(&a["life-span"]["begin"]), "tags": top_tags(&a["tags"], 8), "links": links, "urls": urls, "release_groups": groups,
    }))
}

pub async fn release_group(st: &impl Ctx, id: &str) -> anyhow::Result<Value> {
    let g = st.mb().get(st, &format!("release-group/{id}"), &[("inc", "artist-credits+releases+tags".into())]).await?;
    let mut out = rg(&g);
    out["tags"] = json!(top_tags(&g["tags"], 5));
    Ok(out)
}

pub async fn best_release_for_group(st: &impl Ctx, rgid: &str, track_count: Option<i64>) -> anyhow::Result<Option<String>> {
    let d = st.mb().get(st, "release", &[("release-group", rgid.into()), ("inc", "media".into()), ("limit", "100".into())]).await?;
    let rels = d["releases"].as_array().cloned().unwrap_or_default();
    let score = |r: &Value| -> f64 {
        let media = r["media"].as_array().cloned().unwrap_or_default();
        let tc: i64 = media.iter().map(|m| m["track-count"].as_i64().unwrap_or(0)).sum();
        let mut sc = 0.0;
        if let Some(n) = track_count {
            sc -= (tc - n).abs() as f64 * 10.0;
        }
        if s(&r["status"]) == "Official" { sc += 5.0; }
        let fmts: Vec<String> = media.iter().map(|m| s(&m["format"])).collect();
        if fmts.iter().any(|f| f == "Digital Media") { sc += 3.0; }
        if fmts.iter().any(|f| f == "CD") { sc += 2.0; }
        if matches!(s(&r["country"]).as_str(), "XW" | "US" | "GB") { sc += 1.0; }
        let year: f64 = s(&r["date"]).chars().take(4).collect::<String>().parse().unwrap_or(9999.0);
        sc - (year - 1900.0) / 1000.0
    };
    Ok(rels.iter().max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal)).map(|r| s(&r["id"])))
}

pub async fn release(st: &impl Ctx, rid: &str) -> anyhow::Result<Value> {
    let r = st.mb().get(st, &format!("release/{rid}"), &[("inc", "recordings+artist-credits+release-groups+labels+genres+isrcs".into())]).await?;
    let g = &r["release-group"];
    let media = r["media"].as_array().cloned().unwrap_or_default();
    let mut tracks = vec![];
    for m in &media {
        for t in m["tracks"].as_array().cloned().unwrap_or_default() {
            let rec = &t["recording"];
            let ac = if t["artist-credit"].is_array() { &t["artist-credit"] } else { &rec["artist-credit"] };
            let pos = t["position"].as_i64().or_else(|| s(&t["number"]).parse().ok()).unwrap_or(0);
            let title = { let x = s(&t["title"]); if x.is_empty() { s(&rec["title"]) } else { x } };
            tracks.push(json!({
                "disc": m["position"].as_i64().unwrap_or(1), "track": pos, "title": title,
                "artist": credit(ac), "artist_id": credit_id(ac), "artists": credit_list(ac),
                "length": t["length"].as_f64().or(rec["length"].as_f64()).unwrap_or(0.0) / 1000.0,
                "recording_id": rec.get("id").cloned().unwrap_or(Value::Null), "track_id": t.get("id").cloned().unwrap_or(Value::Null),
                "isrc": rec["isrcs"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null),
            }));
        }
    }
    let mut genres: Vec<(i64, String)> = [r["genres"].as_array(), g["genres"].as_array()].into_iter().flatten().flatten()
        .map(|x| (x["count"].as_i64().unwrap_or(0), s(&x["name"]))).collect();
    genres.sort_by_key(|x| std::cmp::Reverse(x.0));
    let labels: Vec<String> = r["label-info"].as_array().cloned().unwrap_or_default().iter().filter_map(|li| li["label"]["name"].as_str().map(String::from)).collect();
    let date = { let d = s(&r["date"]); if d.is_empty() { s(&g["first-release-date"]) } else { d } };
    let year: String = { let d = s(&g["first-release-date"]); if d.is_empty() { s(&r["date"]) } else { d } }.chars().take(4).collect();
    let genre = genres.first().map(|x| { let mut c = x.1.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }).unwrap_or_default();
    Ok(json!({
        "id": s(&r["id"]), "title": s(&r["title"]), "artist": credit(&r["artist-credit"]), "artist_id": credit_id(&r["artist-credit"]),
        "artists": credit_list(&r["artist-credit"]), "date": date, "year": year, "country": s(&r["country"]), "status": s(&r["status"]), "release_group_id": g.get("id").cloned().unwrap_or(Value::Null),
        "type": s(&g["primary-type"]), "label": labels.join(", "), "barcode": s(&r["barcode"]), "disc_count": media.len(),
        // the release group's secondary types (["Live"], ["Compilation", "Remix"]) and its songs over all media:
        // what the library's album types are told at import
        "secondary": g.get("secondary-types").cloned().unwrap_or(json!([])), "track_count": tracks.len(),
        "genre": genre,
        "tracks": tracks, "cover": cover_release(&s(&r["id"]), 500),
    }))
}

/// barcode (UPC/EAN, leading zeros dropped) → release-group id for every release by the artist.
pub async fn artist_barcodes(st: &impl Ctx, mbid: &str) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let mut offset = 0;
    while offset < 400 {
        let page = st.mb().get(st, "release", &[("artist", mbid.into()), ("inc", "release-groups".into()), ("limit", "100".into()), ("offset", offset.to_string())]).await?;
        for rel in page["releases"].as_array().cloned().unwrap_or_default() {
            let bc = rel["barcode"].as_str().unwrap_or("").trim_start_matches('0').to_string();
            if let (false, Some(g)) = (bc.is_empty(), rel["release-group"]["id"].as_str()) {
                out.insert(bc, g.to_string());
            }
        }
        offset += 100;
        if offset >= page["release-count"].as_i64().unwrap_or(0) {
            break;
        }
    }
    Ok(out)
}

pub async fn release_groups_for_isrc(st: &impl Ctx, isrc: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Ok(d) = st.mb().get(st, &format!("isrc/{isrc}"), &[]).await else { return out };
    for r in d["recordings"].as_array().cloned().unwrap_or_default().iter().take(2) {
        let Some(rid) = r["id"].as_str() else { continue };
        if let Ok(d) = st.mb().get(st, &format!("recording/{rid}"), &[("inc", "releases+release-groups".into())]).await {
            for rel in d["releases"].as_array().cloned().unwrap_or_default() {
                if let Some(g) = rel["release-group"]["id"].as_str() {
                    out.insert(g.to_string());
                }
            }
        }
    }
    out
}

pub async fn search_releases(st: &impl Ctx, artist: &str, album: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
    let mut q = format!("release:\"{}\"", esc(album));
    if !artist.is_empty() {
        q += &format!(" AND artist:\"{}\"", esc(artist));
    }
    let d = st.mb().get(st, "release", &[("query", q), ("limit", limit.to_string())]).await?;
    Ok(d["releases"].as_array().cloned().unwrap_or_default().iter().map(|r| {
        let id = s(&r["id"]);
        json!({ "id": id, "title": s(&r["title"]), "artist": credit(&r["artist-credit"]), "date": s(&r["date"]), "country": s(&r["country"]),
            "status": s(&r["status"]), "track_count": r["track-count"].as_i64().unwrap_or(0), "release_group_id": r["release-group"]["id"],
            "type": s(&r["release-group"]["primary-type"]), "score": r["score"].as_i64().unwrap_or(0), "cover": cover_release(&id, 250) })
    }).collect())
}

/// What a recording is asked for with (by [`recording`] and [`album_for_recording`] alike: one cache entry).
const RECORDING_INC: &str = "artist-credits+releases+release-groups+isrcs";

/// The release a song belongs to, of the releases a recording is on: an official one of the song's own artist,
/// a plain studio album before an EP before a single, and a compilation ("Greatest Hits"), live album,
/// soundtrack or remix album only when the song is on nothing else; the first released of those. MusicBrainz
/// lists the releases in no useful order, and its first was often a best-of or the single, so a song imported
/// on its own went into "Greatest Hits" instead of the album it is from.
pub fn album_release(r: &Value) -> Value {
    let ids = |ac: &Value| -> std::collections::HashSet<String> {
        ac.as_array().map(|a| a.iter().filter_map(|c| c["artist"]["id"].as_str().map(String::from)).collect()).unwrap_or_default()
    };
    let artist_ids = ids(&r["artist-credit"]);
    let rels = r["releases"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let key = |rel: &Value| {
        let g = &rel["release-group"];
        let official = rel["status"].is_null() || rel["status"] == "Official";
        let ra = ids(&rel["artist-credit"]);
        let own = ra.is_empty() || artist_ids.is_empty() || !ra.is_disjoint(&artist_ids);
        let plain = g["secondary-types"].as_array().is_none_or(|a| a.is_empty());
        let kind = match s(&g["primary-type"]).as_str() { "Album" => 0, "EP" => 1, "Single" => 2, _ => 3 };
        let date = [s(&g["first-release-date"]), s(&rel["date"])].into_iter().find(|d| !d.is_empty()).unwrap_or_else(|| "9999".into());
        (!official, !own, !plain, kind, date)
    };
    rels.iter().min_by_key(|rel| key(rel)).cloned().unwrap_or_default()
}

pub async fn recording(st: &impl Ctx, rid: &str) -> anyhow::Result<Value> {
    let r = st.mb().get(st, &format!("recording/{rid}"), &[("inc", RECORDING_INC.into())]).await?;
    let rel = album_release(&r);
    let year: String = { let d = s(&r["first-release-date"]); if d.is_empty() { s(&rel["date"]) } else { d } }.chars().take(4).collect();
    let g = &rel["release-group"];
    Ok(json!({ "id": s(&r["id"]), "title": s(&r["title"]), "artist": credit(&r["artist-credit"]), "artist_id": credit_id(&r["artist-credit"]),
        "artists": credit_list(&r["artist-credit"]),
        "length": r["length"].as_f64().unwrap_or(0.0) / 1000.0, "album": s(&rel["title"]), "release_id": rel.get("id").cloned().unwrap_or(Value::Null),
        "release_group_id": g.get("id").cloned().unwrap_or(Value::Null), "year": year,
        // what the release it picked is: its group's types and its songs (0 when MusicBrainz didn't say)
        "release_type": s(&g["primary-type"]), "release_secondary": g.get("secondary-types").cloned().unwrap_or(json!([])),
        "release_track_count": release_track_count(&rel),
        "isrc": r["isrcs"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null) }))
}

/// Songs on a release as a recording's answer lists it: the sum of its media's `track-count` (0 when not given).
fn release_track_count(rel: &Value) -> i64 {
    rel["media"].as_array().map(|m| m.iter().map(|x| x["track-count"].as_i64().unwrap_or(0)).sum()).unwrap_or(0)
}

/// Every release a recording is on (the answer [`recording`] asks for, so its cache entry): `{title,
/// release_group_id, type, secondary, track_count}` each.
pub async fn recording_releases(st: &impl Ctx, rid: &str) -> anyhow::Result<Vec<Value>> {
    let r = st.mb().get(st, &format!("recording/{rid}"), &[("inc", RECORDING_INC.into())]).await?;
    Ok(r["releases"].as_array().map(|a| a.as_slice()).unwrap_or(&[]).iter().map(|rel| {
        let g = &rel["release-group"];
        json!({ "title": s(&rel["title"]), "release_group_id": s(&g["id"]), "type": s(&g["primary-type"]),
            "secondary": g.get("secondary-types").cloned().unwrap_or(json!([])), "track_count": release_track_count(rel) })
    }).collect())
}

pub async fn recordings_for_isrc(st: &impl Ctx, isrc: &str) -> Vec<String> {
    let Ok(d) = st.mb().get(st, &format!("isrc/{isrc}"), &[]).await else { return vec![] };
    d["recordings"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["id"].as_str().map(String::from)).collect()
}

/// The official studio album (or EP) a recording first appeared on. Compilations, live albums, soundtracks,
/// remix albums and other artists' releases are skipped. (It asks for what [`recording`] asks for, so the
/// import of the song that follows finds the answer in the cache instead of waiting for MusicBrainz again.)
pub async fn album_for_recording(st: &impl Ctx, rid: &str, allow_ep: bool) -> anyhow::Result<Option<Value>> {
    let d = st.mb().get(st, &format!("recording/{rid}"), &[("inc", RECORDING_INC.into())]).await?;
    let ids = |ac: &Value| -> std::collections::HashSet<String> {
        ac.as_array().map(|a| a.iter().filter_map(|c| c["artist"]["id"].as_str().map(String::from)).collect()).unwrap_or_default()
    };
    let artist_ids = ids(&d["artist-credit"]);
    let mut best: Option<((bool, String), Value)> = None;
    for rel in d["releases"].as_array().cloned().unwrap_or_default() {
        let g = &rel["release-group"];
        if !(rel["status"].is_null() || rel["status"] == "Official") {
            continue;
        }
        let pt = s(&g["primary-type"]);
        if !(pt == "Album" || (allow_ep && pt == "EP")) || g["secondary-types"].as_array().is_some_and(|a| !a.is_empty()) {
            continue;
        }
        let ra = ids(&rel["artist-credit"]);
        let ra = if ra.is_empty() { artist_ids.clone() } else { ra };
        if !artist_ids.is_empty() && ra.is_disjoint(&artist_ids) {
            continue;
        }
        let date = { let x = s(&g["first-release-date"]); if x.is_empty() { let y = s(&rel["date"]); if y.is_empty() { "9999".into() } else { y } } else { x } };
        let key = (pt != "Album", date);
        if best.as_ref().is_none_or(|(k, _)| key < *k) {
            best = Some((key, g.clone()));
        }
    }
    Ok(best.map(|(_, g)| {
        let id = s(&g["id"]);
        json!({ "id": id, "title": s(&g["title"]), "type": s(&g["primary-type"]), "artist": credit(&d["artist-credit"]),
            "year": s(&g["first-release-date"]).chars().take(4).collect::<String>(), "cover": cover_rg(&id, 250) })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(id: &str, status: &str, typ: &str, secondary: &[&str], date: &str, artist: &str) -> Value {
        json!({ "id": id, "status": status, "date": date, "artist-credit": [{ "artist": { "id": artist } }],
                "release-group": { "id": format!("rg-{id}"), "primary-type": typ, "secondary-types": secondary, "first-release-date": date } })
    }

    /// The album the song is from, not the best-of MusicBrainz lists first, nor the single, nor another
    /// artist's compilation; a best-of only when the song is on nothing else.
    #[test]
    fn a_song_goes_into_the_album_it_is_from() {
        let r = json!({ "artist-credit": [{ "artist": { "id": "fm" } }], "releases": [
            rel("hits", "Official", "Album", &["Compilation"], "1988-11-21", "fm"),
            rel("single", "Official", "Single", &[], "1977-03-24", "fm"),
            rel("various", "Official", "Album", &[], "1970-01-01", "va"),
            rel("bootleg", "Bootleg", "Album", &[], "1976-01-01", "fm"),
            rel("deluxe", "Official", "Album", &[], "2004-03-22", "fm"),
            rel("lp", "Official", "Album", &[], "1977-02-04", "fm"),
            rel("live", "Official", "Album", &["Live"], "1980-12-08", "fm"),
        ]});
        assert_eq!(album_release(&r)["id"], "lp");
        let only = json!({ "artist-credit": [{ "artist": { "id": "fm" } }], "releases": [
            rel("hits", "Official", "Album", &["Compilation"], "1988-11-21", "fm"), rel("single", "Official", "Single", &[], "1977-03-24", "fm"),
        ]});
        assert_eq!(album_release(&only)["id"], "single");
        let best_of = json!({ "releases": [rel("hits", "Official", "Album", &["Compilation"], "1988-11-21", "fm")] });
        assert_eq!(album_release(&best_of)["id"], "hits");
        assert_eq!(album_release(&json!({})), json!(null));
    }
}
