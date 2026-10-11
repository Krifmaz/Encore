//! MusicBrainz web service: rate limited (1 request/second by default) and cached for an hour.
use crate::state::AppState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct Mb {
    last: Arc<Mutex<Option<Instant>>>,
    cache: Arc<std::sync::Mutex<HashMap<String, (Instant, Value)>>>,
}

const TTL: Duration = Duration::from_secs(3600);

impl Mb {
    pub async fn get(&self, st: &AppState, path: &str, params: &[(&str, String)]) -> anyhow::Result<Value> {
        let mut ps: Vec<(String, String)> = params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        ps.push(("fmt".into(), "json".into()));
        ps.sort();
        let key = format!("{path}?{}", ps.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
        if let Some((t, v)) = self.cache.lock().unwrap().get(&key) {
            if t.elapsed() < TTL {
                return Ok(v.clone());
            }
        }
        let base = st.cfg.str("musicbrainz_url");
        let base = if base.is_empty() { "https://musicbrainz.org".to_string() } else { base.trim_end_matches('/').to_string() };
        let gap = Duration::from_secs_f64(1.0 / st.cfg.float("musicbrainz_rate").max(0.1));
        let contact = st.cfg.str("contact_email");
        let ua = format!("Encore/{} ( {} )", crate::VERSION, if contact.is_empty() { "https://github.com/Krifmaz/Encore".into() } else { contact });
        for attempt in 0..4 {
            {
                let mut last = self.last.lock().await;
                if let Some(l) = *last {
                    if l.elapsed() < gap {
                        tokio::time::sleep(gap - l.elapsed()).await;
                    }
                }
                *last = Some(Instant::now());
            }
            let r = st.http.get(format!("{base}/ws/2/{path}")).query(&ps).header("User-Agent", &ua).send().await?;
            if r.status().as_u16() == 503 {
                tokio::time::sleep(Duration::from_millis(1500 * (attempt + 1))).await;
                continue;
            }
            let v: Value = r.error_for_status()?.json().await?;
            let mut c = self.cache.lock().unwrap();
            if c.len() > 3000 {
                c.retain(|_, (t, _)| t.elapsed() < TTL);
            }
            c.insert(key, (Instant::now(), v.clone()));
            return Ok(v);
        }
        anyhow::bail!("MusicBrainz is rate limiting requests, try again shortly")
    }
}

pub fn esc(s: &str) -> String {
    s.chars().flat_map(|c| if "+-&|!(){}[]^\"~*?:\\/".contains(c) { vec!['\\', c] } else { vec![c] }).collect()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

pub fn credit(ac: &Value) -> String {
    ac.as_array().map(|a| a.iter().map(|c| {
        let n = c.get("name").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(String::from).unwrap_or_else(|| s(&c["artist"], "name"));
        n + c.get("joinphrase").and_then(|x| x.as_str()).unwrap_or("")
    }).collect()).unwrap_or_default()
}

pub fn credit_id(ac: &Value) -> Value {
    ac.as_array().and_then(|a| a.first()).and_then(|c| c["artist"].get("id")).cloned().unwrap_or(Value::Null)
}

pub fn cover_rg(id: &str, size: u32) -> String {
    format!("https://coverartarchive.org/release-group/{id}/front-{size}")
}

fn top_tags(v: &Value, n: usize) -> Vec<String> {
    let mut t: Vec<(i64, String)> = v.as_array().map(|a| a.iter().map(|x| (x["count"].as_i64().unwrap_or(0), s(x, "name"))).collect()).unwrap_or_default();
    t.sort_by(|a, b| b.0.cmp(&a.0));
    t.into_iter().take(n).map(|x| x.1).collect()
}

pub fn rg(v: &Value) -> Value {
    let id = s(v, "id");
    let date = s(v, "first-release-date");
    json!({
        "id": id, "title": s(v, "title"), "artist": credit(&v["artist-credit"]), "artist_id": credit_id(&v["artist-credit"]),
        "type": s(v, "primary-type"), "secondary": v.get("secondary-types").cloned().unwrap_or(json!([])),
        "year": date.chars().take(4).collect::<String>(), "date": date, "score": v.get("score").cloned().unwrap_or(json!(0)),
        "cover": cover_rg(&id, 250),
    })
}

pub async fn search_artists(st: &AppState, q: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
    let d = st.mb.get(st, "artist", &[("query", q.into()), ("limit", limit.to_string())]).await?;
    Ok(d["artists"].as_array().cloned().unwrap_or_default().iter().map(|a| json!({
        "id": s(a, "id"), "name": s(a, "name"), "disambiguation": s(a, "disambiguation"), "type": s(a, "type"), "country": s(a, "country"),
        "score": a.get("score").cloned().unwrap_or(json!(0)), "begin": s(&a["life-span"], "begin"), "tags": top_tags(&a["tags"], 4),
    })).collect())
}

pub async fn search_albums(st: &AppState, q: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
    let d = st.mb.get(st, "release-group", &[("query", q.into()), ("limit", limit.to_string())]).await?;
    Ok(d["release-groups"].as_array().cloned().unwrap_or_default().iter().map(rg).collect())
}

pub async fn search_recordings(st: &AppState, q: &str, limit: usize, artist: Option<&str>) -> anyhow::Result<Vec<Value>> {
    let query = match artist {
        Some(a) => format!("recording:\"{}\" AND artist:\"{}\"", esc(q), esc(a)),
        None => q.to_string(),
    };
    let d = st.mb.get(st, "recording", &[("query", query), ("limit", limit.to_string())]).await?;
    Ok(d["recordings"].as_array().cloned().unwrap_or_default().iter().map(|r| {
        let rel = r["releases"].as_array().and_then(|a| a.first()).cloned().unwrap_or(json!({}));
        let rgid = s(&rel["release-group"], "id");
        let year: String = { let d = s(&rel, "date"); if d.is_empty() { s(r, "first-release-date") } else { d } }.chars().take(4).collect();
        let (rgv, cover) = if rgid.is_empty() { (Value::Null, Value::Null) } else { (json!(rgid), json!(cover_rg(&rgid, 250))) };
        json!({
            "id": s(r, "id"), "title": s(r, "title"), "artist": credit(&r["artist-credit"]), "artist_id": credit_id(&r["artist-credit"]),
            "length": r["length"].as_f64().unwrap_or(0.0) / 1000.0, "album": s(&rel, "title"), "release_id": rel.get("id").cloned().unwrap_or(Value::Null),
            "release_group_id": rgv, "year": year,
            "score": r.get("score").cloned().unwrap_or(json!(0)), "cover": cover,
        })
    }).collect())
}

pub async fn artist(st: &AppState, mbid: &str) -> anyhow::Result<Value> {
    let a = st.mb.get(st, &format!("artist/{mbid}"), &[("inc", "tags+url-rels".into())]).await?;
    let name = s(&a, "name");
    let mut rgs = vec![];
    let mut offset = 0;
    loop {
        let page = st.mb.get(st, "release-group", &[("artist", mbid.into()), ("limit", "100".into()), ("offset", offset.to_string())]).await?;
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
        "id": s(&a, "id"), "name": name, "disambiguation": s(&a, "disambiguation"), "type": s(&a, "type"), "country": s(&a, "country"),
        "begin": s(&a["life-span"], "begin"), "tags": top_tags(&a["tags"], 8), "links": links, "urls": urls, "release_groups": groups,
    }))
}

pub async fn release_group(st: &AppState, id: &str) -> anyhow::Result<Value> {
    let g = st.mb.get(st, &format!("release-group/{id}"), &[("inc", "artist-credits+releases+tags".into())]).await?;
    let mut out = rg(&g);
    out["tags"] = json!(top_tags(&g["tags"], 5));
    Ok(out)
}

pub async fn best_release_for_group(st: &AppState, rgid: &str, track_count: Option<i64>) -> anyhow::Result<Option<String>> {
    let d = st.mb.get(st, "release", &[("release-group", rgid.into()), ("inc", "media".into()), ("limit", "100".into())]).await?;
    let rels = d["releases"].as_array().cloned().unwrap_or_default();
    let score = |r: &Value| -> f64 {
        let media = r["media"].as_array().cloned().unwrap_or_default();
        let tc: i64 = media.iter().map(|m| m["track-count"].as_i64().unwrap_or(0)).sum();
        let mut sc = 0.0;
        if let Some(n) = track_count {
            sc -= (tc - n).abs() as f64 * 10.0;
        }
        if s(r, "status") == "Official" { sc += 5.0; }
        let fmts: Vec<String> = media.iter().map(|m| s(m, "format")).collect();
        if fmts.iter().any(|f| f == "Digital Media") { sc += 3.0; }
        if fmts.iter().any(|f| f == "CD") { sc += 2.0; }
        if matches!(s(r, "country").as_str(), "XW" | "US" | "GB") { sc += 1.0; }
        let year: f64 = s(r, "date").chars().take(4).collect::<String>().parse().unwrap_or(9999.0);
        sc - (year - 1900.0) / 1000.0
    };
    Ok(rels.iter().max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal)).map(|r| s(r, "id")))
}

pub async fn release(st: &AppState, rid: &str) -> anyhow::Result<Value> {
    let r = st.mb.get(st, &format!("release/{rid}"), &[("inc", "recordings+artist-credits+release-groups+labels+genres+isrcs".into())]).await?;
    let g = &r["release-group"];
    let media = r["media"].as_array().cloned().unwrap_or_default();
    let mut tracks = vec![];
    for m in &media {
        for t in m["tracks"].as_array().cloned().unwrap_or_default() {
            let rec = &t["recording"];
            let ac = if t["artist-credit"].is_array() { &t["artist-credit"] } else { &rec["artist-credit"] };
            let pos = t["position"].as_i64().or_else(|| s(&t, "number").parse().ok()).unwrap_or(0);
            let title = { let x = s(&t, "title"); if x.is_empty() { s(rec, "title") } else { x } };
            tracks.push(json!({
                "disc": m["position"].as_i64().unwrap_or(1), "track": pos, "title": title,
                "artist": credit(ac), "artist_id": credit_id(ac),
                "length": t["length"].as_f64().or(rec["length"].as_f64()).unwrap_or(0.0) / 1000.0,
                "recording_id": rec.get("id").cloned().unwrap_or(Value::Null), "track_id": t.get("id").cloned().unwrap_or(Value::Null),
                "isrc": rec["isrcs"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null),
            }));
        }
    }
    let mut genres: Vec<(i64, String)> = [r["genres"].as_array(), g["genres"].as_array()].into_iter().flatten().flatten()
        .map(|x| (x["count"].as_i64().unwrap_or(0), s(x, "name"))).collect();
    genres.sort_by(|a, b| b.0.cmp(&a.0));
    let labels: Vec<String> = r["label-info"].as_array().cloned().unwrap_or_default().iter().filter_map(|li| li["label"]["name"].as_str().map(String::from)).collect();
    let date = { let d = s(&r, "date"); if d.is_empty() { s(g, "first-release-date") } else { d } };
    let year: String = { let d = s(g, "first-release-date"); if d.is_empty() { s(&r, "date") } else { d } }.chars().take(4).collect();
    let genre = genres.first().map(|x| { let mut c = x.1.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }).unwrap_or_default();
    Ok(json!({
        "id": s(&r, "id"), "title": s(&r, "title"), "artist": credit(&r["artist-credit"]), "artist_id": credit_id(&r["artist-credit"]),
        "date": date, "year": year, "country": s(&r, "country"), "status": s(&r, "status"), "release_group_id": g.get("id").cloned().unwrap_or(Value::Null),
        "type": s(g, "primary-type"), "label": labels.join(", "), "barcode": s(&r, "barcode"), "disc_count": media.len(),
        "genre": genre,
        "tracks": tracks, "cover": format!("https://coverartarchive.org/release/{}/front-500", s(&r, "id")),
    }))
}

/// barcode (UPC/EAN, leading zeros dropped) → release-group id for every release by the artist.
pub async fn artist_barcodes(st: &AppState, mbid: &str) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let mut offset = 0;
    while offset < 400 {
        let page = st.mb.get(st, "release", &[("artist", mbid.into()), ("inc", "release-groups".into()), ("limit", "100".into()), ("offset", offset.to_string())]).await?;
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

pub async fn release_groups_for_isrc(st: &AppState, isrc: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Ok(d) = st.mb.get(st, &format!("isrc/{isrc}"), &[]).await else { return out };
    for r in d["recordings"].as_array().cloned().unwrap_or_default().iter().take(2) {
        let Some(rid) = r["id"].as_str() else { continue };
        if let Ok(d) = st.mb.get(st, &format!("recording/{rid}"), &[("inc", "releases+release-groups".into())]).await {
            for rel in d["releases"].as_array().cloned().unwrap_or_default() {
                if let Some(g) = rel["release-group"]["id"].as_str() {
                    out.insert(g.to_string());
                }
            }
        }
    }
    out
}

pub fn cover_release(id: &str, size: u32) -> String {
    format!("https://coverartarchive.org/release/{id}/front-{size}")
}

pub async fn search_releases(st: &AppState, artist: &str, album: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
    let mut q = format!("release:\"{}\"", esc(album));
    if !artist.is_empty() {
        q += &format!(" AND artist:\"{}\"", esc(artist));
    }
    let d = st.mb.get(st, "release", &[("query", q), ("limit", limit.to_string())]).await?;
    Ok(d["releases"].as_array().cloned().unwrap_or_default().iter().map(|r| {
        let id = s(r, "id");
        json!({ "id": id, "title": s(r, "title"), "artist": credit(&r["artist-credit"]), "date": s(r, "date"), "country": s(r, "country"),
            "status": s(r, "status"), "track_count": r["track-count"].as_i64().unwrap_or(0), "release_group_id": r["release-group"]["id"],
            "type": s(&r["release-group"], "primary-type"), "score": r["score"].as_i64().unwrap_or(0), "cover": cover_release(&id, 250) })
    }).collect())
}

pub async fn recording(st: &AppState, rid: &str) -> anyhow::Result<Value> {
    let r = st.mb.get(st, &format!("recording/{rid}"), &[("inc", "artist-credits+releases+release-groups+isrcs".into())]).await?;
    let rel = r["releases"].as_array().and_then(|a| a.first()).cloned().unwrap_or_default();
    let year: String = { let d = s(&r, "first-release-date"); if d.is_empty() { s(&rel, "date") } else { d } }.chars().take(4).collect();
    Ok(json!({ "id": s(&r, "id"), "title": s(&r, "title"), "artist": credit(&r["artist-credit"]), "artist_id": credit_id(&r["artist-credit"]),
        "length": r["length"].as_f64().unwrap_or(0.0) / 1000.0, "album": s(&rel, "title"), "release_id": rel.get("id").cloned().unwrap_or(Value::Null),
        "release_group_id": rel["release-group"].get("id").cloned().unwrap_or(Value::Null), "year": year,
        "isrc": r["isrcs"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null) }))
}

pub async fn recordings_for_isrc(st: &AppState, isrc: &str) -> Vec<String> {
    let Ok(d) = st.mb.get(st, &format!("isrc/{isrc}"), &[]).await else { return vec![] };
    d["recordings"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["id"].as_str().map(String::from)).collect()
}

/// The official studio album (or EP) a recording first appeared on. Compilations, live albums, soundtracks,
/// remix albums and other artists' releases are skipped.
pub async fn album_for_recording(st: &AppState, rid: &str, allow_ep: bool) -> anyhow::Result<Option<Value>> {
    let d = st.mb.get(st, &format!("recording/{rid}"), &[("inc", "releases+release-groups+artist-credits".into())]).await?;
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
        let pt = s(g, "primary-type");
        if !(pt == "Album" || (allow_ep && pt == "EP")) || g["secondary-types"].as_array().is_some_and(|a| !a.is_empty()) {
            continue;
        }
        let ra = ids(&rel["artist-credit"]);
        let ra = if ra.is_empty() { artist_ids.clone() } else { ra };
        if !artist_ids.is_empty() && ra.is_disjoint(&artist_ids) {
            continue;
        }
        let date = { let x = s(g, "first-release-date"); if x.is_empty() { let y = s(&rel, "date"); if y.is_empty() { "9999".into() } else { y } } else { x } };
        let key = (pt != "Album", date);
        if best.as_ref().is_none_or(|(k, _)| key < *k) {
            best = Some((key, g.clone()));
        }
    }
    Ok(best.map(|(_, g)| {
        let id = s(&g, "id");
        json!({ "id": id, "title": s(&g, "title"), "type": s(&g, "primary-type"), "artist": credit(&d["artist-credit"]),
            "year": s(&g, "first-release-date").chars().take(4).collect::<String>(), "cover": cover_rg(&id, 250) })
    }))
}

/// The MusicBrainz recording for a song, version-aware (a remix won't resolve to the original).
pub async fn resolve_recording(st: &AppState, artist: &str, title: &str, length: Option<f64>, isrc: Option<&str>) -> anyhow::Result<Option<String>> {
    if let Some(i) = isrc.filter(|i| !i.is_empty()) {
        if let Some(id) = recordings_for_isrc(st, i).await.into_iter().next() {
            return Ok(Some(id));
        }
    }
    let want = crate::matcher::describe(title, artist, "", length, None, None);
    for r in search_recordings(st, title, 10, Some(artist)).await? {
        let l = r["length"].as_f64().filter(|l| *l > 0.0);
        if crate::matcher::compare(&want, &crate::matcher::describe(r["title"].as_str().unwrap_or(""), r["artist"].as_str().unwrap_or(""), "", l, None, None), true).matched {
            return Ok(r["id"].as_str().map(String::from));
        }
    }
    Ok(None)
}
