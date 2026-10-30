//! Amazon Music public links: playlists, albums (and a song on an album), shared user playlists, single tracks,
//! amzn.to short links. Read without an Amazon account, in this order:
//!   1. the server-rendered page (its `<music-*-row>` elements, or schema.org JSON-LD when present),
//!   2. the web player's own JSON API (the anonymous config.json CSRF/device token, then the "Skyfire"
//!      showCatalogPlaylist / showCatalogAlbum / … calls the web app makes).
//!
//! Both give rows of texts that `rows_to_tracks` turns into the import shape of `links.rs`.
//! There's no sign-in: private libraries (My Playlists, Liked songs) need Amazon's private API, which can't be
//! called reliably from a server, so only public links are supported.
use crate::Ctx;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// The desktop browser User-Agent sent with every request, so Amazon serves the full page.
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36";

/// What a link points at.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// "playlist", "album", "user-playlist" or "track"
    pub kind: String,
    /// the album, playlist or track id (an ASIN)
    pub id: String,
    /// the song on an album link (`?trackAsin=`), or the id of a track link
    pub track: Option<String>,
    /// music.amazon.<tld> host
    pub host: String,
}

/// Read a music.amazon.<tld> (or amazon.<tld>/music/player) link into what it points at; `None` for anything else.
pub fn parse_url(url: &str) -> Option<Link> {
    let re = Regex::new(r"(?i)(?:https?://)?(?:www\.)?(music\.amazon\.[a-z.]+|amazon\.[a-z.]+/music/player)/(playlists|albums|user-playlists|tracks)/([A-Za-z0-9]+)").unwrap();
    let c = re.captures(url.trim())?;
    let host = c[1].split('/').next().unwrap_or("").to_lowercase();
    let host = if host.starts_with("music.") { host } else { format!("music.{host}") };
    let id = c[3].to_string();
    let track = Regex::new(r"(?i)[?&]trackAsin=([A-Za-z0-9]+)").unwrap().captures(url).map(|t| t[1].to_string());
    let kind = match c[2].to_lowercase().as_str() {
        "playlists" => "playlist",
        "albums" => if track.is_some() { "track" } else { "album" },
        "user-playlists" => "user-playlist",
        _ => "track",
    };
    let track = if c[2].eq_ignore_ascii_case("tracks") { Some(id.clone()) } else { track };
    Some(Link { kind: kind.into(), id, track, host })
}

/// The Skyfire API region for a storefront ("NA", "EU", "FE" from config.json's siteRegion, else from the domain).
pub fn api_host(host: &str, site_region: &str) -> String {
    let r = match site_region.to_lowercase().as_str() {
        r @ ("na" | "eu" | "fe") => r.to_string(),
        _ => {
            let tld = host.trim_start_matches("music.amazon.");
            if ["co.uk", "de", "fr", "it", "es", "in", "nl", "se", "pl", "com.tr", "ae", "sa"].contains(&tld) {
                "eu".into()
            } else if ["co.jp", "com.au", "sg"].contains(&tld) {
                "fe".into()
            } else {
                "na".into()
            }
        }
    };
    format!("https://{r}.mesk.skill.music.a2z.com/api")
}

/// The Skyfire calls for a link, tried in order.
pub fn api_methods(kind: &str) -> &'static [&'static str] {
    match kind {
        "playlist" => &["showCatalogPlaylist"],
        "album" | "track" => &["showCatalogAlbum"],
        _ => &["showSharedUserPlaylist", "showUserPlaylist"],
    }
}

/// The body the web player POSTs: the request headers travel JSON-encoded inside the JSON body.
pub fn api_body(cfg: &Value, link: &Link, page_url: &str, now_ms: u128) -> Value {
    let s = |k: &str| cfg[k].as_str().unwrap_or("").to_string();
    let csrf = json!({ "interface": "CSRFInterface.v1_0.CSRFHeaderElement", "token": cfg["csrf"]["token"].as_str().unwrap_or(""),
        "timestamp": cfg["csrf"]["ts"].as_str().map(String::from).unwrap_or_else(|| cfg["csrf"]["ts"].to_string()),
        "rndNonce": cfg["csrf"]["rnd"].as_str().map(String::from).unwrap_or_else(|| cfg["csrf"]["rnd"].to_string()) });
    let auth = json!({ "interface": "ClientAuthenticationInterface.v1_0.ClientTokenElement", "accessToken": s("accessToken") });
    let lang = cfg["displayLanguage"].as_str().unwrap_or("en_US");
    let headers = json!({
        "x-amzn-authentication": auth.to_string(),
        "x-amzn-device-model": "WEBPLAYER",
        "x-amzn-device-width": "1920",
        "x-amzn-device-family": "WebPlayer",
        "x-amzn-device-id": s("deviceId"),
        "x-amzn-user-agent": UA,
        "x-amzn-session-id": s("sessionId"),
        "x-amzn-device-height": "1080",
        "x-amzn-request-id": format!("{:x}-{:x}", now_ms, now_ms.wrapping_mul(2654435761) & 0xffff_ffff),
        "x-amzn-device-language": lang,
        "x-amzn-currency-of-preference": "USD",
        "x-amzn-os-version": "1.0",
        "x-amzn-application-version": s("version"),
        "x-amzn-device-time-zone": "UTC",
        "x-amzn-timestamp": now_ms.to_string(),
        "x-amzn-csrf": csrf.to_string(),
        "x-amzn-music-domain": link.host,
        "x-amzn-referer": link.host,
        "x-amzn-affiliate-tags": "",
        "x-amzn-ref-marker": "",
        "x-amzn-page-url": page_url,
        "x-amzn-weblab-id-overrides": "",
        "x-amzn-video-player-token": "",
        "x-amzn-feature-flags": "hd-supported",
        "x-amzn-has-profile-id": "",
        "x-amzn-age-band": "",
    });
    json!({ "id": link.id, "userHash": json!({ "level": "LIBRARY_MEMBER" }).to_string(), "headers": headers.to_string() })
}

// ---- reading rows -------------------------------------------------------------------------------

/// One song row as the page shows it: its title, the other texts in display order, and its link.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    /// the song's title
    pub title: String,
    /// the row's other texts in display order (artist, album, time …)
    pub texts: Vec<String>,
    /// the row's link, which carries the track ASIN
    pub href: String,
    /// the ISRC, when the API row has one
    pub isrc: Option<String>,
}

/// The page header: name, artist (albums), cover.
#[derive(Debug, Clone, Default)]
pub struct Header {
    /// the playlist or album name
    pub name: String,
    /// the album's artist; empty for playlists
    pub artist: String,
    /// the cover image URL
    pub cover: Option<String>,
}

/// HTML entities in an attribute or text: the named ones pages use and any numeric one (&#233; &#x2019;),
/// in one pass so "&amp;#39;" stays "&#39;".
fn unescape(s: &str) -> String {
    static ENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[a-zA-Z]+);").unwrap());
    ENT.replace_all(s, |c: &regex::Captures| {
        let e = &c[1];
        let ch = if let Some(h) = e.strip_prefix("#x").or(e.strip_prefix("#X")) {
            u32::from_str_radix(h, 16).ok().and_then(char::from_u32)
        } else if let Some(d) = e.strip_prefix('#') {
            d.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            match e {
                "quot" => Some('"'), "apos" => Some('\''), "lt" => Some('<'), "gt" => Some('>'), "amp" => Some('&'), "nbsp" => Some(' '),
                _ => None,
            }
        };
        ch.map(|c| if c == '\u{a0}' { ' '.to_string() } else { c.to_string() }).unwrap_or_else(|| c[0].to_string())
    }).replace('\u{a0}', " ")
}

/// Every `name="value"` attribute of an HTML tag, names lowercased and values unescaped.
fn attrs(tag: &str) -> Vec<(String, String)> {
    Regex::new(r#"([a-zA-Z0-9_-]+)="([^"]*)""#).unwrap().captures_iter(tag).map(|c| (c[1].to_lowercase(), unescape(&c[2]))).collect()
}

/// The value of attribute `k`, or "" when the tag lacks it.
fn attr<'a>(a: &'a [(String, String)], k: &str) -> &'a str {
    a.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("")
}

/// "3:45", "03:45", "1:02:03" → seconds.
pub fn clock(s: &str) -> Option<f64> {
    let c = Regex::new(r"^\s*(?:(\d{1,2}):)?(\d{1,2}):(\d{2})\s*$").unwrap().captures(s)?;
    let n = |i: usize| c.get(i).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0);
    Some(n(1) * 3600.0 + n(2) * 60.0 + n(3))
}

/// The server-rendered page: `<music-detail-header>` + `<music-image-row>` / `<music-text-row>` elements.
pub fn html_rows(html: &str) -> (Header, Vec<Cell>) {
    let mut h = Header::default();
    if let Some(m) = Regex::new(r"(?s)<music-detail-header\b([^>]*)>").unwrap().captures(html) {
        let a = attrs(&m[1]);
        h.name = attr(&a, "primary-text").trim().to_string();
        h.artist = attr(&a, "secondary-text").trim().to_string();
        h.cover = Some(attr(&a, "image-src").to_string()).filter(|c| c.starts_with("http"));
    }
    if h.name.is_empty() {
        if let Some(m) = Regex::new(r#"<meta\s+(?:property|name)="og:title"\s+content="([^"]*)""#).unwrap().captures(html) {
            h.name = unescape(&m[1]).trim().to_string();
        }
    }
    if h.cover.is_none() {
        h.cover = Regex::new(r#"<meta\s+(?:property|name)="og:image"\s+content="([^"]*)""#).unwrap().captures(html).map(|m| unescape(&m[1]));
    }
    let mut cells = vec![];
    for m in Regex::new(r"(?s)<music-(?:image|text|container)-row\b([^>]*)>").unwrap().captures_iter(html) {
        let a = attrs(&m[1]);
        let title = attr(&a, "primary-text").trim().to_string();
        if title.is_empty() {
            continue;
        }
        let mut texts = vec![];
        for k in ["secondary-text-1", "secondary-text-2", "secondary-text-3", "secondary-text", "tertiary-text", "duration"] {
            let v = attr(&a, k).trim();
            if !v.is_empty() {
                texts.push(v.to_string());
            }
        }
        let href = [attr(&a, "primary-href"), attr(&a, "href")].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string();
        cells.push(Cell { title, texts, href, isrc: Some(attr(&a, "isrc").to_string()).filter(|i| !i.is_empty()) });
    }
    // other rows (artists, "more like this") don't link to a song: keep only song rows when the page marks them
    if cells.iter().any(|c| asin_of(&c.href).is_some()) {
        cells.retain(|c| asin_of(&c.href).is_some());
    }
    (h, cells)
}

/// schema.org MusicPlaylist / MusicAlbum, when the page carries it.
pub fn jsonld_rows(html: &str) -> Option<(Header, Vec<Cell>)> {
    let re = Regex::new(r#"(?s)<script[^>]+type="application/ld\+json"[^>]*>(.*?)</script>"#).unwrap();
    for c in re.captures_iter(html) {
        let Ok(d) = serde_json::from_str::<Value>(c[1].trim()) else { continue };
        let Some(ld) = find(&d, &|x| (x["@type"] == "MusicPlaylist" || x["@type"] == "MusicAlbum") && !x["track"].is_null()) else { continue };
        let by = |v: &Value| match v { Value::Array(a) => a.iter().map(|b| txt(&b["name"])).collect::<Vec<_>>().join(", "), b => txt(&b["name"]) };
        let h = Header { name: txt(&ld["name"]), artist: by(&ld["byArtist"]), cover: ld["image"].as_str().map(String::from) };
        let list = match &ld["track"] {
            Value::Object(o) => o.get("itemListElement").and_then(|x| x.as_array()).cloned().unwrap_or_else(|| vec![ld["track"].clone()]),
            Value::Array(a) => a.clone(),
            _ => vec![],
        };
        let cells = list.iter().map(|x| {
            let x = if x["item"].is_object() { &x["item"] } else { x };
            let mut texts = vec![by(&x["byArtist"]), txt(&x["inAlbum"]["name"])];
            if let Some(d) = x["duration"].as_str().and_then(encore_core::text::iso_duration) {
                texts.push(format!("{}:{:02}", (d as u64) / 60, (d as u64) % 60));
            }
            Cell { title: txt(&x["name"]), texts: texts.into_iter().filter(|t| !t.is_empty()).collect(), href: txt(&x["url"]), isrc: x["isrcCode"].as_str().map(String::from) }
        }).collect();
        return Some((h, cells));
    }
    None
}

/// A Skyfire text: a plain string, or {text: …}.
fn txt(v: &Value) -> String {
    match v {
        Value::String(s) => unescape(s).trim().to_string(),
        Value::Object(o) => o.get("text").map(txt).unwrap_or_default(),
        _ => String::new(),
    }
}

/// The first value in a JSON tree (depth first, the node itself included) that `pred` accepts.
fn find<'a>(v: &'a Value, pred: &dyn Fn(&Value) -> bool) -> Option<&'a Value> {
    if pred(v) {
        return Some(v);
    }
    match v {
        Value::Object(m) => m.values().find_map(|x| find(x, pred)),
        Value::Array(a) => a.iter().find_map(|x| find(x, pred)),
        _ => None,
    }
}

/// Collect every value in a JSON tree that `pred` accepts; a match isn't searched further down.
fn all<'a>(v: &'a Value, pred: &dyn Fn(&Value) -> bool, out: &mut Vec<&'a Value>) {
    if pred(v) {
        out.push(v);
        return;
    }
    match v {
        Value::Object(m) => m.values().for_each(|x| all(x, pred, out)),
        Value::Array(a) => a.iter().for_each(|x| all(x, pred, out)),
        _ => {}
    }
}

/// A row's link: the first deeplink / url / href under primaryLink, primaryTextLink or link; "" when none.
fn link_of(x: &Value) -> String {
    for k in ["primaryLink", "primaryTextLink", "link"] {
        let l = &x[k];
        for f in ["deeplink", "url", "href"] {
            if let Some(s) = l[f].as_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// A Skyfire answer: {methods: [{template: {headerText, headerPrimaryText, headerImage, widgets: [{items: [row…]}]}}]}.
/// Song rows are the items with a primaryText whose link names a track (…?trackAsin= or /tracks/…).
pub fn api_rows(d: &Value) -> (Header, Vec<Cell>) {
    let tpl = find(d, &|x| x.get("headerText").is_some() || x.get("widgets").is_some()).cloned().unwrap_or_else(|| json!({}));
    let h = Header {
        name: txt(&tpl["headerText"]),
        artist: [txt(&tpl["headerPrimaryText"]), txt(&tpl["headerSecondaryText"])].into_iter().find(|s| !s.is_empty() && clock(s).is_none()).unwrap_or_default(),
        cover: tpl["headerImage"].as_str().or(tpl["backgroundImage"].as_str()).map(String::from),
    };
    let mut rows = vec![];
    let is_song = |x: &Value| x.get("primaryText").is_some() && { let l = link_of(x); l.contains("trackAsin=") || l.contains("/tracks/") };
    all(d, &is_song, &mut rows);
    let cells = rows.iter().map(|x| {
        let mut texts = vec![];
        for k in ["secondaryText1", "secondaryText2", "secondaryText3", "secondaryText", "tertiaryText", "duration"] {
            let t = txt(&x[k]);
            if !t.is_empty() {
                texts.push(t);
            }
        }
        Cell { title: txt(&x["primaryText"]), texts, href: link_of(x), isrc: x["isrc"].as_str().map(String::from) }
    }).collect();
    (h, cells)
}

/// The track ASIN in a link (`trackAsin=…` or `/tracks/…`).
fn asin_of(href: &str) -> Option<String> {
    Regex::new(r"(?:trackAsin=|/tracks/)([A-Za-z0-9]+)").unwrap().captures(href).map(|c| c[1].to_string())
}

/// Rows → the import shape of `links.rs`. Playlist rows read title · artist · album · time; album rows read
/// title · time (the album's artist comes from the header).
pub fn rows_to_tracks(link: &Link, h: &Header, cells: &[Cell]) -> Vec<Value> {
    let album_page = link.kind == "album" || link.kind == "track";
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    for c in cells {
        let asin = asin_of(&c.href);
        if let Some(a) = &asin {
            if !seen.insert(a.clone()) {
                continue;
            }
        }
        if link.kind == "track" && link.track.is_some() && asin.is_some() && asin != link.track {
            continue;
        }
        let duration = c.texts.iter().find_map(|t| clock(t));
        let words: Vec<&String> = c.texts.iter().filter(|t| clock(t).is_none() && !t.eq_ignore_ascii_case("explicit")).collect();
        let (artist, album) = if album_page {
            // a row's own artist (compilations, features) wins over the album's
            (words.first().map(|s| (*s).clone()).unwrap_or_else(|| h.artist.clone()), Some(h.name.clone()).filter(|n| !n.is_empty()))
        } else {
            (words.first().map(|s| (*s).clone()).unwrap_or_default(), words.get(1).map(|s| (*s).clone()))
        };
        let isrc = c.isrc.as_ref().map(|i| i.to_uppercase().replace('-', "")).filter(|i| !i.is_empty());
        out.push(json!({ "title": c.title.trim(), "artist": artist.trim(), "album": album, "duration": duration.filter(|d| *d > 0.0), "isrc": isrc,
                         "mbid": null, "source_id": asin, "cover": if album_page { h.cover.clone() } else { None } }));
    }
    out
}

/// The import answer: name (the album's artist appended for albums), cover and tracks.
fn result(h: &Header, tracks: Vec<Value>, link: &Link) -> Value {
    let name = if h.name.is_empty() { "Amazon Music import".to_string() } else { h.name.clone() };
    let name = if link.kind == "album" && !h.artist.is_empty() && !name.contains(&h.artist) { format!("{name} – {}", h.artist) } else { name };
    json!({ "name": name, "description": "", "cover": h.cover, "tracks": tracks })
}

// ---- fetching -----------------------------------------------------------------------------------

/// Follow an amzn.to short link to the page it redirects to; other links are returned as they are.
async fn resolve_short(st: &impl Ctx, url: &str) -> String {
    if Regex::new(r"(?i)amzn\.to/").unwrap().is_match(url) {
        if let Ok(r) = st.http().get(url.trim()).header("User-Agent", UA).send().await {
            return r.url().to_string();
        }
    }
    url.trim().to_string()
}

/// Read the song list from the public web page: its HTML rows first, then its schema.org data.
async fn from_page(st: &impl Ctx, url: &str, link: &Link) -> anyhow::Result<Value> {
    let r = st.http().get(url).header("User-Agent", UA).header("Accept-Language", "en-US,en;q=0.9").send().await?;
    if r.status().as_u16() >= 400 {
        anyhow::bail!("the page returned {}", r.status().as_u16());
    }
    let html = r.text().await?;
    let (h, cells) = html_rows(&html);
    let mut tracks = rows_to_tracks(link, &h, &cells);
    let mut head = h;
    if tracks.is_empty() {
        if let Some((h2, c2)) = jsonld_rows(&html) {
            tracks = rows_to_tracks(link, &h2, &c2);
            head = h2;
        }
    }
    if tracks.is_empty() {
        anyhow::bail!("no song list on the page");
    }
    Ok(result(&head, tracks, link))
}

/// Read the song list through the web player's API, using the token from the site's config.json.
async fn from_api(st: &impl Ctx, url: &str, link: &Link) -> anyhow::Result<Value> {
    let cfg: Value = st.http().get(format!("https://{}/config.json", link.host)).header("User-Agent", UA).send().await?.json().await?;
    if cfg["csrf"]["token"].as_str().unwrap_or("").is_empty() {
        anyhow::bail!("Amazon Music gave no web player token");
    }
    let base = api_host(&link.host, cfg["siteRegion"].as_str().unwrap_or(""));
    let mut last = String::new();
    for m in api_methods(&link.kind) {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let body = api_body(&cfg, link, url, now_ms);
        let r = st.http().post(format!("{base}/{m}")).header("User-Agent", UA).header("Content-Type", "text/plain;charset=UTF-8")
            .header("Origin", format!("https://{}", link.host)).header("Referer", format!("https://{}/", link.host)).body(body.to_string()).send().await?;
        if r.status().as_u16() >= 400 {
            last = format!("{m} returned {}", r.status().as_u16());
            continue;
        }
        let d: Value = r.json().await?;
        let (h, cells) = api_rows(&d);
        let tracks = rows_to_tracks(link, &h, &cells);
        if !tracks.is_empty() {
            return Ok(result(&h, tracks, link));
        }
        last = format!("{m} listed no songs");
    }
    anyhow::bail!("{last}")
}

/// Read a public Amazon Music link.
pub async fn fetch(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    let url = resolve_short(st, url).await;
    let link = parse_url(&url).ok_or_else(|| anyhow::anyhow!("Not an Amazon Music playlist, album or song link"))?;
    let page_err = match from_page(st, &url, &link).await {
        Ok(v) => return Ok(v),
        Err(e) => e.to_string(),
    };
    match from_api(st, &url, &link).await {
        Ok(v) => Ok(v),
        Err(e) => anyhow::bail!("Couldn't read that Amazon Music link (is it public? only shared public playlists and albums can be imported, there's no Amazon sign-in): {page_err}; {e}"),
    }
}

#[cfg(test)]
#[path = "amazon_tests.rs"]
mod tests;
