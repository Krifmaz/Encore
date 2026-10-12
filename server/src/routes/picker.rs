//! /api/picker: search one source by hand, then download exactly the chosen result.
use crate::auth::{self, User};
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::matcher::{compare, describe};
use crate::routes::requests::{create_request, get_owned, quota, NewRequest};
use crate::sources::{slskd, streamrip, torznab, ytmusic};
use crate::state::AppState;
use crate::{p, worker};
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/picker/sources", get(sources))
        .route("/api/picker/search", post(search))
        .route("/api/picker/pick", post(pick))
}

const LOSSLESS: [&str; 7] = ["flac", "alac", "wav", "aiff", "aif", "ape", "wv"];

fn available(st: &AppState) -> Vec<Value> {
    let mut out = vec![];
    if slskd::enabled(st) {
        out.push(json!({ "id": "slskd", "name": "Soulseek" }));
    }
    if torznab::enabled(st) {
        out.push(json!({ "id": "torznab", "name": "Torrents & Usenet" }));
    }
    if streamrip::enabled(st) && streamrip::services(st).iter().any(|s| s == "deezer") {
        out.push(json!({ "id": "deezer", "name": "Deezer" }));
    }
    if st.cfg.bool("ytdlp_enabled") {
        out.push(json!({ "id": "ytmusic", "name": "YouTube Music" }));
    }
    out
}

async fn sources(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "sources": available(&st), "allowed": auth::can(&st, &u, "pick") })))
}

fn quality(fmt: &str, bitrate: i64, depth: i64, rate: i64) -> String {
    let f = fmt.to_lowercase();
    if LOSSLESS.contains(&f.as_str()) {
        let bits = if depth > 0 { format!(" {depth}-bit") } else { String::new() };
        let khz = if rate > 0 && rate != 44100 { format!("/{}kHz", rate as f64 / 1000.0) } else { String::new() };
        return format!("{}{bits}{khz}", f.to_uppercase());
    }
    if bitrate > 0 { format!("{} {bitrate}k", f.to_uppercase()) } else { f.to_uppercase() }
}

fn score(want_t: &str, want_a: &str, t: &str, a: &str, len: Option<f64>, got: Option<f64>) -> f64 {
    let v = compare(&describe(want_t, want_a, "", len, None, None), &describe(t, a, "", got, None, None), true);
    if v.matched { (v.score * 100.0).round() / 100.0 } else { 0.0 }
}

fn contains(hay: &str, needle: &str) -> f64 {
    let norm = |s: &str| s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect::<String>();
    let n = norm(needle);
    let w: Vec<&str> = n.split_whitespace().collect();
    if w.is_empty() {
        return 0.0;
    }
    let h = format!(" {} ", norm(hay).split_whitespace().collect::<Vec<_>>().join(" "));
    w.iter().filter(|x| h.contains(&format!(" {x} "))).count() as f64 / w.len() as f64
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchBody {
    source: String,
    kind: String,
    artist: String,
    title: String,
    query: Option<String>,
    track_count: Option<usize>,
    length: Option<f64>,
}

async fn search_slskd(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    let q: String = q.chars().map(|c| if c.is_alphanumeric() || c.is_whitespace() || c == '\'' { c } else { ' ' }).collect();
    let mut out = vec![];
    for resp in slskd::search(st, &q).await? {
        let user = resp["username"].as_str().unwrap_or("").to_string();
        let (free, queue, speed) = (resp["hasFreeUploadSlot"].as_bool().unwrap_or(false), resp["queueLength"].as_f64().unwrap_or(0.0), resp["uploadSpeed"].as_f64().unwrap_or(0.0));
        let audio: Vec<Value> = resp["files"].as_array().cloned().unwrap_or_default().into_iter().filter(|f| slskd::AUDIO_EXT.contains(&slskd::ext(f["filename"].as_str().unwrap_or("")).as_str())).collect();
        let mut groups: Vec<(String, Vec<Value>)> = vec![];
        for f in audio {
            let name = f["filename"].as_str().unwrap_or("").replace('\\', "/");
            let key = if b.kind == "album" { name.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default() } else { name };
            match groups.iter_mut().find(|(k, _)| *k == key) { Some((_, v)) => v.push(f), None => groups.push((key, vec![f])) }
        }
        for (d, mut files) in groups {
            let exts: std::collections::HashSet<String> = files.iter().map(|f| slskd::ext(f["filename"].as_str().unwrap_or(""))).collect();
            let fmt = if exts.len() == 1 { exts.into_iter().next().unwrap() } else { "mixed".into() };
            let parts: Vec<&str> = d.split('/').collect();
            let name = if b.kind == "track" { parts.last().unwrap_or(&"").to_string() } else { parts[parts.len().saturating_sub(2)..].join("/") };
            let rel = contains(&name, &b.title);
            if rel < 0.4 {
                continue;
            }
            let br = files.iter().map(|f| f["bitRate"].as_i64().unwrap_or(0)).min().unwrap_or(0);
            let depth = files.iter().map(|f| f["bitDepth"].as_i64().unwrap_or(0)).min().unwrap_or(0);
            let rate = files.iter().map(|f| f["sampleRate"].as_i64().unwrap_or(0)).max().unwrap_or(0);
            let lossless = LOSSLESS.contains(&fmt.as_str());
            let mut sc = rel * 40.0 + contains(&d, &b.artist) * 10.0 + if lossless { 30.0 } else if br >= 320 { 15.0 } else { 5.0 };
            sc += if depth >= 24 { 6.0 } else { 0.0 } + if free { 8.0 } else { 0.0 } - queue.min(50.0) / 5.0;
            if let (true, Some(tc)) = (b.kind == "album", b.track_count) {
                sc += if files.len() == tc { 15.0 } else if files.len() < tc { -10.0 } else { 0.0 };
            }
            if let (true, Some(l), Some(fl)) = (b.kind == "track", b.length, files[0]["length"].as_f64()) {
                sc += if (fl - l).abs() < 5.0 { 10.0 } else { -10.0 };
            }
            files.sort_by_key(|f| f["filename"].as_str().unwrap_or("").to_string());
            let info = [user.clone(), if free { "free slot".into() } else { format!("queue {queue}") }, if speed > 0.0 { format!("{:.1} MB/s", speed / 1e6) } else { String::new() }]
                .into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            let dir = if b.kind == "track" { d.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default() } else { d.clone() };
            out.push(json!({ "source": "slskd", "key": format!("{user}|{d}"), "title": name, "subtitle": d.replace('/', " / "),
                "quality": quality(&fmt, br, depth, rate), "lossless": lossless, "tracks": files.len(),
                "size": files.iter().map(|f| f["size"].as_i64().unwrap_or(0)).sum::<i64>(),
                "duration": if b.kind == "track" { files[0]["length"].clone() } else { Value::Null }, "info": info, "score": (sc * 10.0).round() / 10.0,
                "payload": { "username": user, "directory": dir, "format": fmt, "bitrate": br,
                    "files": files.iter().map(|f| json!({ "filename": f["filename"], "size": f["size"] })).collect::<Vec<_>>() } }));
        }
    }
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(150);
    Ok(out)
}

async fn search_torznab(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    if b.kind != "album" {
        anyhow::bail!("Torrent and Usenet searches find whole albums only");
    }
    let mut out = vec![];
    for r in torznab::search(st, q).await? {
        let proto = r["protocol"].as_str().unwrap_or("torrent").to_string();
        let url = r["magnetUrl"].as_str().filter(|m| !m.is_empty()).or(r["downloadUrl"].as_str()).unwrap_or("").to_string();
        if url.is_empty() || (proto == "torrent" && !torznab::qbit_ready(st)) || (proto == "usenet" && !torznab::sab_ready(st)) {
            continue;
        }
        let title = r["title"].as_str().unwrap_or("").to_string();
        let lossless = regex::Regex::new(r"(?i)\b(flac|lossless|alac|24[\s-]?bit|hi-?res)\b").unwrap().is_match(&title);
        let seeds = r["seeders"].as_i64();
        let age = r["age"].as_i64().map(|d| format!("{d} days old")).unwrap_or_default();
        let info = [proto.clone(), seeds.map(|s| format!("{s} seeders")).unwrap_or_default(), age].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        let sc = contains(&title, &b.title) * 40.0 + contains(&title, &b.artist) * 15.0 + if lossless { 30.0 } else { 0.0 } + (seeds.unwrap_or(0) as f64 + 1.0).log2().min(10.0);
        out.push(json!({ "source": "torznab", "key": format!("torznab:{}", r["guid"].as_str().unwrap_or(&title)), "title": title,
            "subtitle": r["indexer"], "quality": if lossless { "FLAC" } else { "" }, "lossless": lossless, "tracks": null, "size": r["size"], "info": info,
            "score": (sc * 10.0).round() / 10.0,
            "payload": { "title": title, "indexer": r["indexer"], "protocol": proto, "size": r["size"], "seeders": r["seeders"], "url": url, "lossless": lossless } }));
    }
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

async fn search_deezer(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    let kind = if b.kind == "album" { "album" } else { "track" };
    let d: Value = st.http.get(format!("https://api.deezer.com/search/{kind}")).query(&[("q", q), ("limit", "25")]).send().await?.json().await?;
    let mut out: Vec<Value> = d["data"].as_array().cloned().unwrap_or_default().iter().map(|x| {
        let a = x["artist"]["name"].as_str().unwrap_or("");
        let explicit = if x["explicit_lyrics"].as_bool().unwrap_or(false) { "explicit" } else { "" };
        if kind == "album" {
            let rt = x["record_type"].as_str().unwrap_or("");
            let rt = rt.chars().next().map(|c| c.to_uppercase().collect::<String>() + &rt[1..]).unwrap_or_default();
            let info = [rt, explicit.to_string()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            json!({ "source": "deezer", "key": format!("deezer:album:{}", x["id"]), "title": x["title"], "subtitle": a, "quality": "FLAC (Deezer HiFi)",
                "lossless": true, "tracks": x["nb_tracks"], "size": null, "info": info,
                "cover": x["cover_medium"], "score": score(&b.title, &b.artist, x["title"].as_str().unwrap_or(""), a, None, None),
                "payload": { "url": format!("https://www.deezer.com/album/{}", x["id"]) } })
        } else {
            let mut t = x["title"].as_str().unwrap_or("").to_string();
            if let Some(v) = x["title_version"].as_str().filter(|v| !v.is_empty() && !t.to_lowercase().contains(&v.to_lowercase())) {
                t = format!("{t} {v}");
            }
            json!({ "source": "deezer", "key": format!("deezer:track:{}", x["id"]), "title": t, "subtitle": format!("{a} · {}", x["album"]["title"].as_str().unwrap_or("")),
                "quality": "FLAC (Deezer HiFi)", "lossless": true, "tracks": 1, "size": null, "duration": x["duration"], "info": explicit, "cover": x["album"]["cover_medium"],
                "score": score(&b.title, &b.artist, &t, a, b.length, x["duration"].as_f64()), "payload": { "url": format!("https://www.deezer.com/track/{}", x["id"]) } })
        }
    }).collect();
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

async fn search_ytmusic(st: &AppState, b: &SearchBody, q: &str) -> anyhow::Result<Vec<Value>> {
    let album = b.kind == "album";
    Ok(ytmusic::search(st, q, !album).await?.into_iter().take(20).filter_map(|x| {
        let (t, a) = (x["title"].as_str().unwrap_or("").to_string(), x["artist"].as_str().unwrap_or("").to_string());
        if album {
            let bid = x["browseId"].as_str().filter(|b| b.starts_with("MPRE"))?;
            Some(json!({ "source": "ytmusic", "key": format!("ytm:{bid}"), "title": t, "subtitle": a, "quality": "Opus ~160k / AAC 256k", "lossless": false,
                "tracks": null, "size": null, "info": "", "cover": x["cover"], "score": score(&b.title, &b.artist, &t, &a, None, None), "payload": { "browseId": bid } }))
        } else {
            let vid = x["videoId"].as_str()?;
            let sub = format!("{a} · {}", x["album"].as_str().unwrap_or("")).trim_end_matches(" · ").to_string();
            Some(json!({ "source": "ytmusic", "key": format!("ytm:{vid}"), "title": t, "subtitle": sub, "quality": "Opus ~160k / AAC 256k", "lossless": false,
                "tracks": 1, "size": null, "duration": x["duration"], "info": "", "cover": x["cover"],
                "score": score(&b.title, &b.artist, &t, &a, b.length, x["duration"].as_f64()), "payload": { "url": format!("https://music.youtube.com/watch?v={vid}") } }))
        }
    }).collect())
}

async fn search(State(st): State<AppState>, User(u): User, Json(b): Json<SearchBody>) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "pick") {
        return Err(err(403, "Ask an admin for permission to pick downloads"));
    }
    if b.kind != "album" && b.kind != "track" {
        return Err(err(400, "Manual search works for albums and songs"));
    }
    let q = b.query.clone().map(|q| q.trim().to_string()).filter(|q| !q.is_empty())
        .unwrap_or_else(|| [b.artist.as_str(), b.title.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" "));
    let res = match b.source.as_str() {
        "slskd" => search_slskd(&st, &b, &q).await,
        "torznab" => search_torznab(&st, &b, &q).await,
        "deezer" => search_deezer(&st, &b, &q).await,
        "ytmusic" => search_ytmusic(&st, &b, &q).await,
        other => return Err(err(400, format!("Unknown source {other}"))),
    };
    res.map(|v| Json(json!(v))).map_err(|e| err(502, format!("{} search failed: {}", b.source, e.to_string().chars().take(160).collect::<String>())))
}

#[derive(Deserialize)]
struct PickBody {
    source: String,
    payload: Value,
    title: Option<String>,
    request_id: Option<i64>,
    request: Option<NewRequest>,
}

/// Create a request locked to the chosen result. An existing request is cancelled and replaced.
async fn pick(State(st): State<AppState>, User(u): User, Json(mut b): Json<PickBody>) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "pick") {
        return Err(err(403, "Ask an admin for permission to pick downloads"));
    }
    if b.source == "link" {
        let url = b.payload["url"].as_str().unwrap_or("").trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(err(400, "Paste a full http(s) link"));
        }
        b.payload = json!({ "url": url });
    }
    // YouTube Music albums → their playlist link
    if b.source == "ytmusic" {
        if let Some(bid) = b.payload["browseId"].as_str().map(String::from) {
            let (title, pl, _) = ytmusic::album(&st, &bid).await.map_err(|e| err(502, e.to_string()))?;
            let pl = pl.ok_or_else(|| err(502, "YouTube Music has no playable playlist for this album"))?;
            b.payload = json!({ "url": format!("https://music.youtube.com/playlist?list={pl}"), "title": title });
        }
    }
    let req = if let Some(rid) = b.request_id {
        let old = get_owned(&st, rid, &u)?;
        let s = |k: &str| old[k].as_str().map(String::from);
        let req = NewRequest { kind: s("kind").unwrap_or_default(), mbid: s("mbid"), artist: s("artist"), title: s("title"), album: s("album"), year: s("year"),
            cover_url: s("cover_url"), length: old["detail"]["length"].as_f64(), isrc: old["detail"]["isrc"].as_str().map(String::from), ..Default::default() };
        let status = old["status"].as_str().unwrap_or("");
        if status == "pending" || worker::ACTIVE.contains(&status) {
            worker::cancel(&st, rid).await;
            st.db.execute("UPDATE requests SET status='cancelled', message='Replaced by a manual pick', updated_at=? WHERE id=?", p![now(), rid])?;
        }
        req
    } else if let Some(r) = b.request.clone() {
        quota(&st, &u, 1)?;
        r
    } else {
        return Err(err(400, "Nothing to download"));
    };
    let label = b.title.clone().or(b.payload["title"].as_str().map(String::from)).unwrap_or_default();
    let (id, status) = create_pick(&st, &u, &req, &b.source, b.payload, &label)?;
    Ok(Json(json!({ "id": id, "status": status })))
}

/// The request behind a manual pick: the worker downloads exactly `payload` from `source`, then tags
/// and imports it like any other request. Returns (id, status).
pub fn create_pick(st: &AppState, u: &crate::db::Row, req: &NewRequest, source: &str, payload: Value, label: &str) -> ApiResult<(i64, String)> {
    if req.kind != "album" && req.kind != "track" {
        return Err(err(400, "Manual picks work for albums and songs"));
    }
    let id = create_request(st, u, req, None, Some(json!({ "pick": { "source": source, "payload": payload, "title": label } })))?;
    let status = st.db.one("SELECT status FROM requests WHERE id=?", p![id])?.map(|r| r.s("status")).unwrap_or_default();
    if status == "queued" {
        let msg: String = if label.is_empty() { "Manual pick".into() } else { format!("Manual pick: {label}").chars().take(200).collect() };
        st.db.execute("UPDATE requests SET message=? WHERE id=?", p![msg, id])?;
    }
    st.db.log("info", &format!("{} manually picked {source} result for {}: {label}", u.s("username"), req.title.clone().or(req.album.clone()).unwrap_or_default()));
    Ok((id, status))
}
