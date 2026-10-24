//! Read playlists / albums / songs from streaming-service links into one shape:
//! {name, description, cover, source, tracks: [{title, artist, album, duration, isrc, mbid, source_id, cover}]}
use crate::Ctx;
use encore_core::json::str_ref as s;
use encore_http::{deezer, tidal};
use regex::Regex;
use serde_json::{json, Value};

/// The desktop browser User-Agent sent with page requests, so sites serve the full page.
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36";

/// One track in the import shape: trimmed title and artist, the ISRC uppercased without dashes, and no zero durations.
#[allow(clippy::too_many_arguments)]
fn t(title: &str, artist: &str, album: Option<&str>, duration: Option<f64>, isrc: Option<&str>, mbid: Option<&str>, source_id: Option<String>, cover: Option<&str>) -> Value {
    let isrc = isrc.map(|i| i.to_uppercase().replace('-', "")).filter(|i| !i.is_empty());
    json!({ "title": title.trim(), "artist": artist.trim(), "album": album, "duration": duration.filter(|d| *d > 0.0), "isrc": isrc,
            "mbid": mbid, "source_id": source_id, "cover": cover })
}

/// The service a link belongs to ("spotify", "deezer", "tidal", "qobuz", "apple", "listenbrainz", "amazon" or "ytdlp"), or `None`.
pub fn detect(url: &str) -> Option<&'static str> {
    let rules = [
        ("spotify", r"(?i)open\.spotify\.com|^spotify:|spotify\.link"), ("deezer", r"(?i)deezer\.com|deezer\.page\.link|link\.deezer\.com"),
        ("tidal", r"(?i)tidal\.com"), ("qobuz", r"(?i)qobuz\.com"), ("apple", r"(?i)music\.apple\.com"), ("listenbrainz", r"(?i)listenbrainz\.org/playlist"),
        ("amazon", r"(?i)music\.amazon\.[a-z.]+/|amazon\.[a-z.]+/music/player/|amzn\.to/"),
        ("ytdlp", r"(?i)youtube\.com|youtu\.be|soundcloud\.com|bandcamp\.com|mixcloud\.com|audiomack\.com|vimeo\.com"),
    ];
    for (name, pat) in rules {
        if Regex::new(pat).unwrap().is_match(url.trim()) {
            return Some(name);
        }
    }
    url.trim().starts_with("http").then_some("web")
}

/// The tracks behind a playlist link, with the signed-in user's own service connections when `uid` is given
/// (private playlists, full length).
pub async fn fetch_as(st: &impl Ctx, url: &str, uid: Option<i64>) -> anyhow::Result<Value> {
    let kind = detect(url).ok_or_else(|| anyhow::anyhow!("Unsupported link"))?;
    let mut data = match kind {
        "spotify" => spotify(st, url, uid).await?,
        "deezer" => deezer(st, url).await?,
        "tidal" => tidal(st, url).await?,
        "apple" => apple(st, url).await?,
        "amazon" => super::amazon::fetch(st, url).await?,
        "listenbrainz" => listenbrainz(st, url).await?,
        "ytdlp" => ytdlp_playlist(st, url).await?,
        _ => web_jsonld(st, url).await?,
    };
    data["source"] = json!(kind);
    let tracks: Vec<Value> = data["tracks"].as_array().cloned().unwrap_or_default().into_iter().filter(|x| !s(&x["title"]).is_empty()).collect();
    if tracks.is_empty() {
        anyhow::bail!("No tracks found at that link (is the playlist public?)");
    }
    data["tracks"] = json!(tracks);
    Ok(data)
}

/// "Artist - Title" (or "Title / Artist", "Title | Artist", "Title by Artist", tab- or comma-separated) per
/// line. Every other line with text is still a song: "- Title" or "Title -" (no artist), "Artist -「Title」"
/// (a dash spaced on one side only) or a bare title. Lines of only invisible characters are left out.
pub fn parse_text(text: &str) -> anyhow::Result<Value> {
    // "1. " / "12) " numbering; a name like "10.000 Maniacs" keeps its digits
    let num = Regex::new(r"^\d+[\.\)]\s+").unwrap();
    let dash = Regex::new(r"\s[-–—‐−˗]\s").unwrap();
    let loose_dash = Regex::new(r"\s[-–—‐−˗]|[-–—‐−˗]\s").unwrap();
    // "Title / Artist" and "Title | Artist", the way video titles name their singer
    let slash = Regex::new(r"\s[/|｜／]\s").unwrap();
    let loose_slash = Regex::new(r"\s[/|｜／]|[/|｜／]\s").unwrap();
    let double_slash = Regex::new(r"\s//\s").unwrap();
    let by = Regex::new(r"(?i)\sby\s").unwrap();
    let edge_dash: &[char] = &['-', '–', '—', '‐', '−', '˗'];
    let invisible = |c: char| c.is_whitespace() || matches!(c, '\u{feff}' | '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{00ad}');
    let split2 = |re: &Regex, line: &str| {
        let mut it = re.splitn(line, 2);
        (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
    };
    let mut tracks = vec![];
    for line in text.lines() {
        let line = line.trim_matches(invisible);
        // "# a comment" and m3u "#EXTINF" lines; a song may start with "#" ("#電音部 - DIVE")
        if line.is_empty() || line == "#" || line.starts_with("# ") || line.starts_with("#EXT") {
            continue;
        }
        let line = num.replace(line, "").to_string();
        let (artist, title) = if dash.is_match(&line) {
            split2(&dash, &line)
        } else if double_slash.is_match(&line) {
            split2(&double_slash, &line)
        } else if line.starts_with(edge_dash) || line.ends_with(edge_dash) {
            (String::new(), line.trim_matches(edge_dash).to_string())
        } else if slash.is_match(&line) {
            let (title, artist) = split2(&slash, &line);
            (artist, title)
        } else if by.is_match(&line) {
            let (title, artist) = split2(&by, &line);
            (artist, title)
        } else if let Some((a, b)) = line.split_once('\t') {
            (a.to_string(), b.to_string())
        } else if loose_slash.is_match(&line) {
            let (title, artist) = split2(&loose_slash, &line);
            (artist, title)
        } else if loose_dash.is_match(&line) {
            split2(&loose_dash, &line)
        } else if let Some((a, b)) = split_outside_brackets(&line, ',') {
            (a.trim().trim_matches('"').to_string(), b.trim().trim_matches('"').to_string())
        } else {
            (String::new(), line.clone())
        };
        let (artist, title) = (artist.trim_matches(invisible).to_string(), title.trim_matches(invisible).to_string());
        // "Artist - " with nothing after it: the one name there is the song's
        let (artist, title) = if title.is_empty() { (String::new(), artist) } else { (artist, title) };
        if title.is_empty() {
            continue;
        }
        tracks.push(t(&title, &artist, None, None, None, None, None, None));
    }
    if tracks.is_empty() {
        anyhow::bail!("Couldn't read any 'Artist - Title' lines");
    }
    Ok(json!({ "name": "Imported list", "description": "", "cover": null, "source": "text", "tracks": tracks }))
}

/// `line` split at the first `sep` that isn't inside brackets: "Song (feat. A, B)" has none.
fn split_outside_brackets(line: &str, sep: char) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    for (i, c) in line.char_indices() {
        match c {
            '(' | '[' | '{' | '（' | '【' | '「' | '『' => depth += 1,
            ')' | ']' | '}' | '）' | '】' | '」' | '』' => depth -= 1,
            _ if c == sep && depth <= 0 => return Some((&line[..i], &line[i + c.len_utf8()..])),
            _ => {}
        }
    }
    None
}

/// Follow a spotify.link / Deezer short link to the page it redirects to; other links are returned as they are.
async fn resolve_short(st: &impl Ctx, url: &str) -> String {
    if Regex::new(r"spotify\.link|deezer\.page\.link|link\.deezer\.com").unwrap().is_match(url) {
        if let Ok(r) = st.http().get(url).header("User-Agent", UA).send().await {
            return r.url().to_string();
        }
    }
    url.to_string()
}

/// The first value in a JSON tree (depth first, the node itself included) that `pred` accepts.
fn walk_find<'a>(v: &'a Value, pred: &dyn Fn(&Value) -> bool) -> Option<&'a Value> {
    if pred(v) {
        return Some(v);
    }
    match v {
        Value::Object(m) => m.values().find_map(|x| walk_find(x, pred)),
        Value::Array(a) => a.iter().find_map(|x| walk_find(x, pred)),
        _ => None,
    }
}

/// Collect every value in a JSON tree that `pred` accepts, including matches nested inside matches.
fn walk_all<'a>(v: &'a Value, pred: &dyn Fn(&Value) -> bool, out: &mut Vec<&'a Value>) {
    if pred(v) {
        out.push(v);
    }
    match v {
        Value::Object(m) => m.values().for_each(|x| walk_all(x, pred, out)),
        Value::Array(a) => a.iter().for_each(|x| walk_all(x, pred, out)),
        _ => {}
    }
}

// ---- Spotify ------------------------------------------------------------------------------------

/// A Spotify API track as an import track; `album` stands in for the track's own album (album tracks have none).
fn sp_track(x: &Value, album: Option<&Value>) -> Value {
    let alb = album.unwrap_or(&x["album"]);
    let artists = x["artists"].as_array().map(|a| a.iter().map(|a| s(&a["name"])).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let cover = alb["images"][0]["url"].as_str();
    t(s(&x["name"]), &artists, alb["name"].as_str(), x["duration_ms"].as_f64().map(|d| d / 1000.0), x["external_ids"]["isrc"].as_str(), None,
      x["id"].as_str().map(String::from), cover)
}

/// A Spotify client credentials token from the configured client id and secret; `None` when they're not set or Spotify refuses.
async fn spotify_app_token(st: &impl Ctx) -> Option<String> {
    let (cid, sec) = (st.cfg().str("spotify_client_id"), st.cfg().str("spotify_client_secret"));
    if cid.is_empty() || sec.is_empty() {
        return None;
    }
    let r = st.http().post("https://accounts.spotify.com/api/token").basic_auth(cid, Some(sec)).form(&[("grant_type", "client_credentials")]).send().await.ok()?;
    r.json::<Value>().await.ok()?["access_token"].as_str().map(String::from)
}

/// Every track of a Spotify playlist, album, track or the user's Liked Songs (`collection`) through the Web API,
/// following pages up to 10 000 tracks and waiting out 429 Retry-After answers.
pub async fn spotify_api(st: &impl Ctx, token: &str, kind: &str, id: &str) -> anyhow::Result<Value> {
    let base = "https://api.spotify.com/v1";
    let get = |u: String| async move {
        let mut r = st.http().get(&u).bearer_auth(token).send().await?;
        if r.status().as_u16() == 429 {
            let wait = r.headers().get("retry-after").and_then(|h| h.to_str().ok()).and_then(|s| s.parse().ok()).unwrap_or(2);
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            r = st.http().get(&u).bearer_auth(token).send().await?;
        }
        if r.status().as_u16() >= 400 {
            let c = r.status().as_u16();
            anyhow::bail!("Spotify API {c}: {}", r.text().await.unwrap_or_default().chars().take(200).collect::<String>());
        }
        Ok::<Value, anyhow::Error>(r.json().await?)
    };
    if kind == "track" {
        let x = get(format!("{base}/tracks/{id}")).await?;
        return Ok(json!({ "name": x["name"], "description": "", "cover": null, "tracks": [sp_track(&x, None)] }));
    }
    let mut items = vec![];
    if kind == "collection" {
        let mut next = Some(format!("{base}/me/tracks?limit=50"));
        while let Some(u) = next.filter(|_| items.len() < 10000) {
            let page = get(u).await?;
            items.extend(page["items"].as_array().cloned().unwrap_or_default().iter().filter(|i| i["track"].is_object()).map(|i| sp_track(&i["track"], None)));
            next = page["next"].as_str().map(String::from);
        }
        return Ok(json!({ "name": "Spotify Liked Songs", "description": "", "cover": null, "tracks": items }));
    }
    let meta = get(format!("{base}/{kind}s/{id}")).await?;
    let mut next;
    if kind == "album" {
        items.extend(meta["tracks"]["items"].as_array().cloned().unwrap_or_default().iter().map(|x| sp_track(x, Some(&meta))));
        next = meta["tracks"]["next"].as_str().map(String::from);
    } else {
        // the 2026 API renamed /tracks → /items; older apps still have /tracks
        next = Some(format!("{base}/playlists/{id}/items?limit=100"));
        if get(next.clone().unwrap()).await.is_err() {
            next = Some(format!("{base}/playlists/{id}/tracks?limit=100"));
        }
    }
    while let Some(u) = next.filter(|_| items.len() < 10000) {
        let page = get(u).await?;
        for it in page["items"].as_array().cloned().unwrap_or_default() {
            let x = if kind == "playlist" { if it["item"].is_object() { it["item"].clone() } else { it["track"].clone() } } else { it };
            if x["name"].is_string() && x["type"].as_str().unwrap_or("track") == "track" {
                items.push(if kind == "playlist" { sp_track(&x, None) } else { sp_track(&x, Some(&meta)) });
            }
        }
        next = page["next"].as_str().map(String::from);
    }
    Ok(json!({ "name": meta["name"], "description": html_unescape(s(&meta["description"])), "cover": meta["images"][0]["url"], "tracks": items }))
}

/// Undo the HTML entities pages put in embedded JSON (`&amp;`, `&quot;`, `&#39;`, `&lt;`, `&gt;`).
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#x27;", "'").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

/// No-credentials fallback: Spotify's public embed player ships the track list as JSON.
async fn spotify_embed(st: &impl Ctx, kind: &str, id: &str) -> anyhow::Result<Value> {
    let r = st.http().get(format!("https://open.spotify.com/embed/{kind}/{id}")).header("User-Agent", UA).send().await?;
    if !r.status().is_success() {
        anyhow::bail!("Spotify embed page returned {}", r.status().as_u16());
    }
    let html = r.text().await?;
    let m = Regex::new(r#"(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#).unwrap().captures(&html)
        .ok_or_else(|| anyhow::anyhow!("Spotify embed page format changed"))?;
    let data: Value = serde_json::from_str(&m[1])?;
    let entity = walk_find(&data, &|d| d.get("trackList").is_some()).cloned().unwrap_or_default();
    let mut tracks: Vec<Value> = entity["trackList"].as_array().cloned().unwrap_or_default().iter().map(|x| {
        t(s(&x["title"]), &s(&x["subtitle"]).replace('\u{a0}', " "), None, x["duration"].as_f64().map(|d| d / 1000.0), None, None,
          x["uri"].as_str().and_then(|u| u.rsplit(':').next()).map(String::from), None)
    }).collect();
    if tracks.is_empty() && entity["title"].is_string() {
        let artists = entity["artists"].as_array().map(|a| a.iter().map(|a| s(&a["name"])).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        tracks.push(t(s(&entity["title"]), &artists, None, entity["duration"].as_f64().map(|d| d / 1000.0), None, None, Some(id.into()), None));
    }
    let name = entity["name"].as_str().or(entity["title"].as_str()).unwrap_or("Spotify import");
    Ok(json!({ "name": name, "description": "", "cover": entity["coverArt"]["sources"][0]["url"], "tracks": tracks }))
}

/// Import a Spotify link: through the Web API (the user's token, then the app token), else the public embed player.
async fn spotify(st: &impl Ctx, url: &str, uid: Option<i64>) -> anyhow::Result<Value> {
    let url = resolve_short(st, url).await;
    let re = Regex::new(r"(?:open\.spotify\.com/(?:intl-[\w-]+/)?(?:user/[^/]+/)?|spotify:)(playlist|album|track|artist|collection)[/:]?([A-Za-z0-9]*)").unwrap();
    let c = re.captures(&url).ok_or_else(|| anyhow::anyhow!("Not a Spotify playlist/album/track link"))?;
    let (kind, id) = (c[1].to_string(), c[2].to_string());
    let mut errors = vec![];
    // the user's own sign-in sees private playlists and every track; the app's credentials see public ones;
    // the public embed page is the last resort and stops at 100 tracks
    let mut tokens = vec![];
    if let Some(uid) = uid {
        if let Some(t) = encore_sync::connections::spotify_token(st, uid).await {
            tokens.push(("your Spotify sign-in", t));
        }
    }
    if kind == "collection" && tokens.is_empty() {
        anyhow::bail!("Liked Songs need your Spotify account connected (Connections page)");
    }
    if let Some(t) = spotify_app_token(st).await {
        tokens.push(("the Spotify app credentials", t));
    }
    for (via, tok) in &tokens {
        match spotify_api(st, tok, &kind, &id).await {
            Ok(v) => return Ok(v),
            Err(e) => errors.push(format!("{via}: {e}")),
        }
    }
    match spotify_embed(st, &kind, &id).await {
        Ok(mut v) => {
            // what was missed goes in `warning` (the import shows it); the description stays the playlist's
            v["warning"] = json!(if errors.is_empty() {
                "Read from Spotify's public page, which stops at 100 tracks and has no ISRCs. Add the Spotify app credentials in Settings and connect your account on the Connections page for the whole playlist.".to_string()
            } else {
                format!("Spotify's API refused ({}), so only the public page could be read – it stops at 100 tracks. Connect your Spotify account on the Connections page (your Spotify app is in development mode: add yourself under its User Management).", errors.join("; "))
            });
            Ok(v)
        }
        Err(e) => {
            errors.push(e.to_string());
            anyhow::bail!("{}", errors.join("; "))
        }
    }
}

// ---- Deezer -------------------------------------------------------------------------------------

/// Import a Deezer playlist, album, track or profile's favourites through Deezer's public API.
async fn deezer(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    let url = resolve_short(st, url).await;
    let re = Regex::new(r"deezer\.com/(?:[a-z]{2}/)?(playlist|album|track|profile)/(\d+)").unwrap();
    let c = re.captures(&url).ok_or_else(|| anyhow::anyhow!("Not a Deezer playlist/album/track link"))?;
    let (kind, id) = (c[1].to_string(), c[2].to_string());
    let get = |r: reqwest::RequestBuilder| async move {
        let d: Value = r.send().await?.json().await?;
        if d["error"].is_object() {
            anyhow::bail!("Deezer: {}", s(&d["error"]["message"]));
        }
        Ok::<Value, anyhow::Error>(d)
    };
    let dz = |x: &Value, album: Option<&Value>| {
        let alb = album.unwrap_or(&x["album"]);
        let mut title = s(&x["title"]).to_string();
        let ver = s(&x["title_version"]);
        if !ver.is_empty() && !title.to_lowercase().contains(&ver.to_lowercase()) {
            title = format!("{title} {ver}");
        }
        t(&title, s(&x["artist"]["name"]), alb["title"].as_str(), x["duration"].as_f64(), x["isrc"].as_str(), None,
          Some(x["id"].to_string()), alb["cover_xl"].as_str().or(alb["cover_big"].as_str()))
    };
    if kind == "track" {
        let x = get(deezer::track(st, &id)).await?;
        return Ok(json!({ "name": x["title"], "description": "", "cover": null, "tracks": [dz(&x, None)] }));
    }
    let (meta, mut next, name, cover) = if kind == "profile" {
        let m = get(deezer::user(st, &id)).await?;
        let n = format!("{}'s Deezer favourites", s(&m["name"]));
        let c = m["picture_xl"].clone();
        (m, Some(deezer::path(st, &format!("user/{id}/tracks")).query(&[("limit", "100")])), n, c)
    } else {
        let m = get(deezer::path(st, &format!("{kind}/{id}"))).await?;
        let n = s(&m["title"]).to_string();
        let c = if m["picture_xl"].is_string() { m["picture_xl"].clone() } else { m["cover_xl"].clone() };
        (m, Some(deezer::path(st, &format!("{kind}/{id}/tracks")).query(&[("limit", "100")])), n, c)
    };
    let album = (kind == "album").then_some(&meta);
    let mut tracks = vec![];
    while let Some(r) = next.filter(|_| tracks.len() < 10000) {
        let page = get(r).await?;
        tracks.extend(page["data"].as_array().cloned().unwrap_or_default().iter().map(|x| dz(x, album)));
        // the next page's address as Deezer gives it
        next = page["next"].as_str().map(|u| st.http().get(u));
    }
    Ok(json!({ "name": name, "description": s(&meta["description"]), "cover": cover, "tracks": tracks }))
}

// ---- Tidal (developer API, client credentials) ---------------------------------------------------

/// Import a Tidal playlist, album or track through Tidal's developer API with client credentials.
async fn tidal(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    let c = Regex::new(r"tidal\.com/(?:browse/)?(playlist|album|track|mix)/([\w-]+)").unwrap().captures(url)
        .ok_or_else(|| anyhow::anyhow!("Not a Tidal playlist/album/track link"))?;
    let (kind, id) = (c[1].to_string(), c[2].to_string());
    if kind == "mix" {
        anyhow::bail!("Tidal mixes can't be read through the public API – save it as a playlist first");
    }
    let token = super::streamrip::tidal_token(st).await?;
    let get = |path: String, q: Vec<(String, String)>| {
        let token = token.clone();
        async move { tidal::read(tidal::get(st, &token, &path).query(&q).send().await?).await }
    };
    let cc = vec![("countryCode".to_string(), "US".to_string())];
    let meta = get(format!("/{kind}s/{id}"), cc.clone()).await?;
    let attrs = meta["data"]["attributes"].clone();
    let mut ids = vec![];
    if kind == "track" {
        ids.push(id.clone());
    } else {
        let mut next = Some(format!("/{kind}s/{id}/relationships/items"));
        let mut q = cc.clone();
        while let Some(n) = next.filter(|_| ids.len() < 10000) {
            let page = get(n, q).await?;
            ids.extend(page["data"].as_array().cloned().unwrap_or_default().iter().filter(|d| d["type"] == "tracks").filter_map(|d| d["id"].as_str().map(String::from)));
            next = page["links"]["next"].as_str().map(String::from);
            q = vec![];
        }
    }
    let mut tracks = vec![];
    for chunk in ids.chunks(20) {
        let d = get("/tracks".into(), vec![("countryCode".into(), "US".into()), ("filter[id]".into(), chunk.join(",")), ("include".into(), "artists,albums".into())]).await?;
        let inc: std::collections::HashMap<(String, String), Value> = d["included"].as_array().cloned().unwrap_or_default().into_iter()
            .map(|x| ((s(&x["type"]).to_string(), s(&x["id"]).to_string()), x["attributes"].clone())).collect();
        for tid in chunk {
            let Some(x) = d["data"].as_array().and_then(|a| a.iter().find(|x| s(&x["id"]) == tid)) else { continue };
            let a = &x["attributes"];
            let names = |rel: &str, typ: &str, field: &str| -> Vec<String> {
                x["relationships"][rel]["data"].as_array().cloned().unwrap_or_default().iter()
                    .filter_map(|r| inc.get(&(typ.to_string(), s(&r["id"]).to_string())).and_then(|v| v[field].as_str().map(String::from))).collect()
            };
            let mut title = s(&a["title"]).to_string();
            let ver = s(&a["version"]);
            if !ver.is_empty() && !title.to_lowercase().contains(&ver.to_lowercase()) {
                title = format!("{title} ({ver})");
            }
            let albums = names("albums", "albums", "title");
            tracks.push(t(&title, &names("artists", "artists", "name").join(", "), albums.first().map(|x| x.as_str()), a["duration"].as_str().and_then(encore_core::text::iso_duration),
                          a["isrc"].as_str(), None, Some(tid.clone()), None));
        }
    }
    let name = attrs["name"].as_str().or(attrs["title"].as_str()).unwrap_or("Tidal import");
    Ok(json!({ "name": name, "description": s(&attrs["description"]), "cover": null, "tracks": tracks }))
}

// ---- Apple Music / Qobuz / any page with schema.org data ------------------------------------------

/// The first schema.org MusicPlaylist / MusicAlbum with tracks in a page's ld+json scripts.
fn jsonld(html: &str) -> Option<Value> {
    let re = Regex::new(r#"(?s)<script[^>]+type="application/ld\+json"[^>]*>(.*?)</script>"#).unwrap();
    for c in re.captures_iter(html) {
        let Ok(d) = serde_json::from_str::<Value>(html_unescape(&c[1]).trim()) else { continue };
        if let Some(h) = walk_find(&d, &|x| (x["@type"] == "MusicPlaylist" || x["@type"] == "MusicAlbum") && !x["track"].is_null()) {
            return Some(h.clone());
        }
    }
    None
}

/// The tracks of a schema.org playlist or album, whether `track` is an ItemList or a plain array.
fn ld_tracks(ld: &Value) -> Vec<Value> {
    let tr = match &ld["track"] {
        Value::Object(_) => ld["track"]["itemListElement"].as_array().cloned().unwrap_or_else(|| vec![ld["track"].clone()]),
        Value::Array(a) => a.clone(),
        _ => vec![],
    };
    tr.iter().map(|x| {
        let x = if x["item"].is_object() { &x["item"] } else { x };
        let by = if x["byArtist"].is_null() { &ld["byArtist"] } else { &x["byArtist"] };
        let artist = match by { Value::Array(a) => a.iter().map(|b| s(&b["name"])).collect::<Vec<_>>().join(", "), b => s(&b["name"]).to_string() };
        t(s(&x["name"]), &artist, x["inAlbum"]["name"].as_str(), x["duration"].as_str().and_then(encore_core::text::iso_duration), x["isrcCode"].as_str(), None, None, None)
    }).collect()
}

/// GET a page as a browser would; `what` names the site in the error when it answers 4xx/5xx.
async fn page(st: &impl Ctx, url: &str, what: &str) -> anyhow::Result<String> {
    let r = st.http().get(url).header("User-Agent", UA).header("Accept-Language", "en-US,en;q=0.9").send().await?;
    if r.status().as_u16() >= 400 {
        anyhow::bail!("{what} returned {}", r.status().as_u16());
    }
    Ok(r.text().await?)
}

/// Import any page that carries the playlist as schema.org data (Qobuz and others).
async fn web_jsonld(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    let html = page(st, url, "Page").await?;
    let ld = jsonld(&html).ok_or_else(|| anyhow::anyhow!("No track list found on that page"))?;
    let name = ld["name"].as_str().unwrap_or("Imported playlist");
    Ok(json!({ "name": name, "description": s(&ld["description"]), "cover": ld["image"].as_str(), "tracks": ld_tracks(&ld) }))
}

/// Import an Apple Music page: its schema.org data, else the track list in its embedded state.
async fn apple(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    let html = page(st, url, "Apple Music").await?;
    if let Some(ld) = jsonld(&html) {
        let tr = ld_tracks(&ld);
        if !tr.is_empty() {
            return Ok(json!({ "name": ld["name"], "description": s(&ld["description"]), "cover": null, "tracks": tr }));
        }
    }
    let c = Regex::new(r#"(?s)<script[^>]+id="serialized-server-data"[^>]*>(.*?)</script>"#).unwrap().captures(&html)
        .ok_or_else(|| anyhow::anyhow!("Couldn't read the Apple Music page (is the playlist public?)"))?;
    let data: Value = serde_json::from_str(&c[1])?;
    let mut rows = vec![];
    walk_all(&data, &|d| d.get("artistName").is_some() && d.get("title").is_some() && (d.get("duration").is_some() || d.get("contentDescriptor").is_some()), &mut rows);
    let tracks: Vec<Value> = rows.iter().map(|x| t(s(&x["title"]), s(&x["artistName"]), x["tertiaryLinks"][0]["title"].as_str(), x["duration"].as_f64().map(|d| d / 1000.0), None, None, None, None)).collect();
    let title = walk_find(&data, &|d| d["title"].is_string() && d.get("artwork").is_some() && d.get("contentDescriptor").is_some());
    Ok(json!({ "name": title.and_then(|x| x["title"].as_str()).unwrap_or("Apple Music import"), "description": "", "cover": null, "tracks": tracks }))
}

// ---- ListenBrainz -------------------------------------------------------------------------------

/// Import a ListenBrainz playlist (JSPF) by its MBID.
async fn listenbrainz(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    let c = Regex::new(r"playlist/([0-9a-f-]{36})").unwrap().captures(url).ok_or_else(|| anyhow::anyhow!("Not a ListenBrainz playlist link"))?;
    let r = encore_http::listenbrainz::playlist(st, &c[1], None).send().await?;
    if r.status().as_u16() >= 400 {
        anyhow::bail!("ListenBrainz returned {}", r.status().as_u16());
    }
    let pl = r.json::<Value>().await?["playlist"].clone();
    let tracks: Vec<Value> = pl["track"].as_array().cloned().unwrap_or_default().iter().map(|x| {
        let ids: Vec<String> = match &x["identifier"] { Value::Array(a) => a.iter().map(|i| s(i).to_string()).collect(), Value::String(v) => vec![v.clone()], _ => vec![] };
        let mbid = ids.iter().find(|i| i.contains("musicbrainz.org/recording/")).and_then(|i| i.rsplit('/').next()).map(String::from);
        t(s(&x["title"]), s(&x["creator"]), x["album"].as_str(), x["duration"].as_f64().map(|d| d / 1000.0), None, mbid.as_deref(), None, None)
    }).collect();
    Ok(json!({ "name": pl["title"], "description": s(&pl["annotation"]), "cover": null, "tracks": tracks }))
}

// ---- YouTube / SoundCloud / Bandcamp / … via yt-dlp ------------------------------------------------

/// A YouTube / SoundCloud / Bandcamp … playlist (or one video) read by yt-dlp. A video names its uploader,
/// which is often not the song's artist: the artist and title come from the tags and the video title first
/// ([`encore_library::video_title::parse`]). Each track also carries `artist_guessed` (only the uploader
/// named the artist: the playlist matches it by title and length too) and `source_title` (the video's own
/// title, shown next to what it matched).
async fn ytdlp_playlist(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    let info = super::ytdlp::playlist(st, url).await.map_err(|e| anyhow::anyhow!("Couldn't read that link: {}", e.to_string().chars().take(200).collect::<String>()))?;
    let entries = info["entries"].as_array().cloned().unwrap_or_else(|| vec![info.clone()]);
    let tracks: Vec<Value> = entries.iter().filter(|e| e.is_object()).map(ytdlp_track).collect();
    let name = info["title"].as_str().unwrap_or("Imported playlist");
    Ok(json!({ "name": name, "description": s(&info["description"]), "cover": info["thumbnail"], "tracks": tracks }))
}

/// One yt-dlp playlist entry → a track ([`t`]) with `artist_guessed` and `source_title`.
fn ytdlp_track(e: &Value) -> Value {
    let video_title = s(&e["title"]);
    let channel = e["channel"].as_str().or(e["uploader"].as_str()).unwrap_or("");
    let artist_tag = e["artist"].as_str().or(e["creator"].as_str()).unwrap_or("");
    let song = encore_library::video_title::parse(video_title, channel, artist_tag, s(&e["track"]));
    let sid = e["url"].as_str().or(e["webpage_url"].as_str()).or(e["id"].as_str()).map(String::from);
    let mut tr = t(&song.title, &song.artist, e["album"].as_str(), e["duration"].as_f64(), None, None, sid, None);
    tr["artist_guessed"] = json!(song.artist_guessed as i64);
    tr["source_title"] = json!(Some(video_title.trim()).filter(|v| !v.is_empty()));
    tr
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_lists() {
        let v = super::parse_text("1. Daft Punk - One More Time\nHello by Adele\nMuse,Uprising\n# comment").unwrap();
        let t = v["tracks"].as_array().unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[1]["artist"], "Adele");
        assert_eq!(t[2]["title"], "Uprising");
        assert_eq!(super::detect("https://open.spotify.com/playlist/x"), Some("spotify"));
    }

    #[test]
    fn text_lists_keep_every_song_line() {
        let v = super::parse_text("\u{feff}Aesop Rock - 39 Thieves\r\n - No Artist Song\r\nLonely Title -\r\nArtist -Glued\r\nJust A Title\r\n10.000 Maniacs - Trouble Me\r\n\r\n米津玄師 - Lemon\r\nTab\tSeparated\r\n\u{200d}\r\nNO EULOGIES / JAMIE PAIGE + UNBEATABLE\r\nど～ぱみん -「アポカリゴットタレント」feat.初音ミク\r\nNEWLY HUMAN FEELING | Moe Shop\r\nミクだし/ 初音ミク\r\nAC/DC\r\nMac DeMarco // On the Level\r\ndialing+darling (feat. A (CV:B), C &...\r\n#電音部 - DIVE\r\n# a comment").unwrap();
        let t = v["tracks"].as_array().unwrap();
        let got: Vec<(&str, &str)> = t.iter().map(|x| (x["artist"].as_str().unwrap(), x["title"].as_str().unwrap())).collect();
        assert_eq!(got, vec![("Aesop Rock", "39 Thieves"), ("", "No Artist Song"), ("", "Lonely Title"), ("Artist", "Glued"), ("", "Just A Title"),
                             ("10.000 Maniacs", "Trouble Me"), ("米津玄師", "Lemon"), ("Tab", "Separated"),
                             ("JAMIE PAIGE + UNBEATABLE", "NO EULOGIES"), ("ど～ぱみん", "「アポカリゴットタレント」feat.初音ミク"), ("Moe Shop", "NEWLY HUMAN FEELING"),
                             ("初音ミク", "ミクだし"), ("", "AC/DC"), ("Mac DeMarco", "On the Level"),
                             ("", "dialing+darling (feat. A (CV:B), C &..."), ("#電音部", "DIVE")]);
    }

    /// A video's uploader isn't taken for the artist when the title names one; when only the uploader
    /// does, the track says the artist is a guess. The video's own title is kept.
    #[test]
    fn youtube_entries() {
        use serde_json::json;
        let a = super::ytdlp_track(&json!({ "title": "Avicii - Levels (Lyrics)", "channel": "7clouds", "duration": 200.0, "id": "abc" }));
        assert_eq!((a["artist"].as_str(), a["title"].as_str(), a["artist_guessed"].as_i64()), (Some("Avicii"), Some("Levels"), Some(0)));
        assert_eq!(a["source_title"], "Avicii - Levels (Lyrics)");
        let b = super::ytdlp_track(&json!({ "title": "Levels (Lyrics)", "channel": "7clouds", "id": "abd" }));
        assert_eq!((b["artist"].as_str(), b["title"].as_str(), b["artist_guessed"].as_i64()), (Some("7clouds"), Some("Levels"), Some(1)));
    }
}
