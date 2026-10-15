//! Torrent and Usenet releases: search every indexer through Prowlarr, download with the chosen torrent
//! client (qBittorrent, Transmission, Deluge, rTorrent or a blackhole folder) or Usenet client (SABnzbd,
//! NZBGet or a blackhole folder), then import the finished folder. Replaces Lidarr. The clients are in
//! sources/dl.
pub use super::dl::Poll;
use super::dl::Client;
use crate::state::AppState;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// The chosen torrent client is set up (the name is from when qBittorrent was the only one).
pub fn qbit_ready(st: &AppState) -> bool {
    Client::torrent(st).ready(st)
}
/// The chosen Usenet client is set up.
pub fn sab_ready(st: &AppState) -> bool {
    Client::usenet(st).ready(st)
}
pub fn enabled(st: &AppState) -> bool {
    st.cfg.bool("torznab_enabled") && !st.cfg.str("prowlarr_url").is_empty() && !st.cfg.str("prowlarr_api_key").is_empty() && (qbit_ready(st) || sab_ready(st))
}

fn base(st: &AppState, key: &str) -> String {
    st.cfg.str(key).trim_end_matches('/').to_string()
}

// ---- Prowlarr ------------------------------------------------------------------------------------

/// Audio (3000), MP3 (3010) and lossless (3040): where single-track releases are filed.
pub const SONG_CATS: [&str; 3] = ["3000", "3010", "3040"];

pub async fn search(st: &AppState, query: &str) -> anyhow::Result<Vec<Value>> {
    search_in(st, query, &["3000"]).await
}

/// Search every indexer in these Newznab categories.
pub async fn search_in(st: &AppState, query: &str, cats: &[&str]) -> anyhow::Result<Vec<Value>> {
    let mut q: Vec<(&str, &str)> = vec![("query", query), ("type", "search"), ("limit", "100")];
    q.extend(cats.iter().map(|c| ("categories", *c)));
    let r = st.http.get(format!("{}/api/v1/search", base(st, "prowlarr_url")))
        .query(&q)
        .header("X-Api-Key", st.cfg.str("prowlarr_api_key")).timeout(std::time::Duration::from_secs(90)).send().await
        .map_err(|e| anyhow::anyhow!("Prowlarr unreachable: {e}"))?;
    if !r.status().is_success() {
        anyhow::bail!("Prowlarr answered {}{}", r.status().as_u16(), if r.status().as_u16() == 401 { " (check the API key)" } else { "" });
    }
    Ok(r.json::<Value>().await?.as_array().cloned().unwrap_or_default())
}

pub async fn status(st: &AppState) -> Value {
    let p = st.http.get(format!("{}/api/v1/indexer", base(st, "prowlarr_url"))).header("X-Api-Key", st.cfg.str("prowlarr_api_key")).send().await;
    let indexers = match p {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok().and_then(|v| v.as_array().map(|a| a.iter().filter(|i| i["enable"].as_bool().unwrap_or(true)).count())),
        _ => None,
    };
    let Some(n) = indexers else { return json!({ "ok": false, "message": "Prowlarr not reachable or wrong API key" }) };
    let (mut ok, mut parts) = (n > 0, vec![format!("{n} indexer(s) in Prowlarr")]);
    for c in [Client::torrent(st), Client::usenet(st)] {
        if !c.ready(st) {
            parts.push(format!("no {}", c.name()));
            continue;
        }
        match c.status(st).await {
            Ok(extra) => parts.push(format!("{} ok{extra}", c.name())),
            Err(e) => {
                ok = false;
                parts.push(e.to_string());
            }
        }
    }
    json!({ "ok": ok, "message": parts.join(" · ") })
}

static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
static LOSSLESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(flac|lossless|alac|24[\s-]?bit|16[\s-]?bit|wav|hi-?res)\b").unwrap());
static MP3_320: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(320|v0)\b").unwrap());
static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix(es)?|karaoke|instrumental|8d|nightcore|tribute|cover)\b").unwrap());
static BIG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(discography|collection|anthology|complete works|box ?set)\b").unwrap());

static BRACKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[][^\)\]]*[\)\]]").unwrap());
/// Extras of a catalogue title that file and release names often leave out.
static EXTRA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(feat|ft|featuring|with|remaster(ed)?|version|mono|stereo|edit|bonus|deluxe)\b").unwrap());
static FEAT_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s+(feat\.?|ft\.?|featuring)\s.*$").unwrap());

/// A song title without "(feat. X)", "(Remastered 2011)" or " - 2009 Version" (Deezer and Apple add them,
/// files usually don't). Versions that matter (live, remix …) stay.
fn core_title(song: &str) -> String {
    let keep = |x: &str| !EXTRA.is_match(x) || BAD.is_match(x);
    let out = BRACKET.replace_all(song, |c: &regex::Captures| if keep(&c[0]) { c[0].to_string() } else { String::new() }).into_owned();
    let out = match out.split_once(" - ") {
        Some((l, r)) if !keep(r) => l.to_string(),
        _ => out,
    };
    FEAT_TAIL.replace(&out, "").trim().to_string()
}

/// `contains` of the song title, or of its core when the extras are what's missing.
fn contains_song(hay: &str, song: &str) -> f64 {
    let t = contains(hay, song);
    let core = core_title(song);
    if core.is_empty() || core == song { t } else { t.max(contains(hay, &core)) }
}

fn norm(s: &str) -> String {
    NON_ALNUM.replace_all(&s.to_lowercase().replace('&', " and "), " ").split_whitespace().collect::<Vec<_>>().join(" ")
}
fn contains(hay: &str, needle: &str) -> f64 {
    let n = norm(needle);
    let w: Vec<&str> = n.split_whitespace().collect();
    if w.is_empty() {
        return 1.0;
    }
    let h = format!(" {} ", norm(hay));
    w.iter().filter(|x| h.contains(&format!(" {x} "))).count() as f64 / w.len() as f64
}

/// Best releases for an album, best first.
pub fn rank_album(st: &AppState, results: &[Value], artist: &str, album: &str, lossless_only: bool) -> Vec<Value> {
    let min_seed = st.cfg.int("torznab_min_seeders");
    let mut out = vec![];
    for r in results {
        let title = r["title"].as_str().unwrap_or("");
        let proto = r["protocol"].as_str().unwrap_or("torrent");
        if (proto == "torrent" && !qbit_ready(st)) || (proto == "usenet" && !sab_ready(st)) {
            continue;
        }
        let (a, b) = (contains(title, album), contains(title, artist));
        if a < 0.8 || b < 0.5 {
            continue;
        }
        let size = r["size"].as_f64().unwrap_or(0.0) / 1e6;
        if size > 0.0 && !(15.0..=4000.0).contains(&size) {
            continue;
        }
        if proto == "torrent" && r["seeders"].as_i64().unwrap_or(0) < min_seed {
            continue;
        }
        let lossless = LOSSLESS.is_match(title);
        if lossless_only && !lossless {
            continue;
        }
        let mut score = a * 30.0 + b * 15.0;
        score += if lossless { 30.0 } else if MP3_320.is_match(title) { 12.0 } else { 0.0 };
        if BAD.is_match(title) && !BAD.is_match(album) {
            score -= 20.0;
        }
        if BIG.is_match(title) {
            score -= 25.0;
        }
        score += if proto == "usenet" { 8.0 } else { (r["seeders"].as_f64().unwrap_or(0.0) + 1.0).log2().min(10.0) };
        let url = r["magnetUrl"].as_str().filter(|m| !m.is_empty()).or(r["downloadUrl"].as_str()).unwrap_or("").to_string();
        if url.is_empty() {
            continue;
        }
        out.push(json!({ "title": title, "indexer": r["indexer"], "protocol": proto, "size": r["size"], "seeders": r["seeders"],
            "url": url, "guid": r["guid"], "lossless": lossless, "score": (score * 10.0).round() / 10.0 }));
    }
    out.sort_by(|x, y| y["score"].as_f64().partial_cmp(&x["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(5);
    out
}

// ---- single songs --------------------------------------------------------------------------------

/// How well a release matches one song (higher is better), and whether it looks like a single-track
/// release. None when it isn't that song. `size` in bytes (0 = unknown).
pub fn score_song(title: &str, size: f64, seeders: Option<i64>, proto: &str, artist: &str, song: &str) -> Option<(f64, bool)> {
    let (t, a) = (contains_song(title, song), contains(title, artist));
    if t < 0.8 || a < 0.5 {
        return None;
    }
    // another version than the one asked for (live, remix, karaoke …): never picked
    if crate::matcher::unwanted_version(song, artist, title).is_some() {
        return None;
    }
    let mb = size / 1e6;
    let single = mb > 0.0 && mb <= 250.0 && !BIG.is_match(title);
    let lossless = LOSSLESS.is_match(title);
    let mut score = t * 40.0 + a * 15.0 + if lossless { 30.0 } else if MP3_320.is_match(title) { 12.0 } else { 0.0 };
    if single {
        score += 12.0;
    }
    if BAD.is_match(title) && !BAD.is_match(song) {
        score -= 20.0;
    }
    if BIG.is_match(title) {
        score -= 25.0;
    }
    // extra words beyond artist and title: a longer name is more likely another release that mentions the song
    let extra = norm(title).split_whitespace().filter(|w| !norm(song).split_whitespace().any(|x| x == *w) && !norm(artist).split_whitespace().any(|x| x == *w)).count();
    score -= (extra as f64).min(10.0);
    score += if proto == "usenet" { 8.0 } else { (seeders.unwrap_or(0) as f64 + 1.0).log2().min(10.0) };
    Some(((score * 10.0).round() / 10.0, single))
}

pub fn is_audio(name: &str) -> bool {
    super::slskd::AUDIO_EXT.contains(&super::slskd::ext(name).as_str())
}

static LEAD_NO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:(?:cd|disc)\s*\d+\s*[-_. ]\s*)?(?:\d-)?(\d{1,3})(?:\D|$)").unwrap());

/// The file of `names` (paths inside a release) that is this song: its name holds every word of the
/// title (or its track number and most of the title), with as few other words as possible. Versions the
/// title doesn't ask for (live, remix, instrumental …) lose.
pub fn pick_file(names: &[String], artist: &str, song: &str, track_no: Option<u32>) -> Option<usize> {
    let want = norm(song);
    if want.is_empty() {
        return None;
    }
    let mut best: Option<(f64, usize)> = None;
    for (i, n) in names.iter().enumerate() {
        if !is_audio(n) {
            continue;
        }
        let file = n.replace('\\', "/");
        let file = file.rsplit('/').next().unwrap_or("");
        let stem = file.rsplit_once('.').map(|x| x.0).unwrap_or(file);
        let t = contains_song(stem, song);
        let no = LEAD_NO.captures(stem).and_then(|c| c[1].parse::<u32>().ok());
        let by_no = track_no.is_some() && no == track_no;
        if t < 0.999 && !(by_no && t >= 0.5) {
            continue;
        }
        if crate::matcher::unwanted_version(song, artist, stem).is_some() {
            continue;
        }
        let words = norm(stem);
        let extra = words.split_whitespace().filter(|w| !want.split_whitespace().any(|x| x == *w) && !norm(artist).split_whitespace().any(|x| x == *w) && w.parse::<u32>().is_err()).count();
        let mut sc = t * 10.0 - extra as f64 + if by_no { 5.0 } else { 0.0 };
        if BAD.is_match(stem) && !BAD.is_match(song) {
            sc -= 8.0;
        }
        if best.is_none_or(|(b, _)| sc > b) {
            best = Some((sc, i));
        }
    }
    best.map(|(_, i)| i)
}

// ---- one download, through the chosen client ----------------------------------------------------

/// The client a release of this protocol goes to.
pub fn client_for(st: &AppState, proto: &str) -> Client {
    if proto == "usenet" { Client::usenet(st) } else { Client::torrent(st) }
}

/// "qBittorrent", "Transmission", "SABnzbd" … for messages.
pub fn client_name(st: &AppState, proto: &str) -> &'static str {
    client_for(st, proto).name()
}

/// Hand a release to the matching download client. → the id used for polling.
pub async fn add(st: &AppState, cand: &Value, rid: i64) -> anyhow::Result<String> {
    let url = cand["url"].as_str().unwrap_or("");
    let c = client_for(st, cand["protocol"].as_str().unwrap_or("torrent"));
    c.add(st, url, cand["title"].as_str().unwrap_or(""), rid).await
}

pub async fn poll(st: &AppState, id: &str) -> anyhow::Result<Poll> {
    let (c, own) = Client::for_id(id);
    c.poll(st, own).await
}

pub enum SongFile {
    /// The client doesn't know the files yet (a magnet's metadata is loading): ask again later.
    Pending,
    /// The client downloads whole releases (Usenet, blackholes) or there is nothing to skip.
    Whole,
    /// Only this file downloads now.
    Only(String),
    /// No file in the release is this song.
    Missing,
}

/// A song from a release with several files: tell the torrent client to download only the song's file.
pub async fn select_song(st: &AppState, id: &str, artist: &str, song: &str, track_no: Option<u32>) -> anyhow::Result<SongFile> {
    let (c, own) = Client::for_id(id);
    if !c.can_select() {
        return Ok(SongFile::Whole);
    }
    let Some(names) = c.files(st, own).await? else { return Ok(SongFile::Pending) };
    if names.iter().filter(|n| is_audio(n)).count() <= 1 {
        return Ok(SongFile::Whole);
    }
    let Some(i) = pick_file(&names, artist, song, track_no) else { return Ok(SongFile::Missing) };
    c.select(st, own, &[i]).await?;
    Ok(SongFile::Only(names[i].clone()))
}

/// Remove the download (and what's left of its files) from the client.
pub async fn remove(st: &AppState, id: &str) {
    let (c, own) = Client::for_id(id);
    c.remove(st, own).await
}

#[cfg(test)]
#[path = "torznab_tests.rs"]
mod song_tests;
