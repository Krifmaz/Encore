//! Read playlists / albums / songs from streaming-service links into one shape:
//! {name, description, cover, source, tracks: [{title, artist, album, duration, isrc, mbid, source_id, cover}]}
use crate::state::AppState;
use regex::Regex;
use serde_json::{json, Value};

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36";

#[allow(clippy::too_many_arguments)]
fn t(title: &str, artist: &str, album: Option<&str>, duration: Option<f64>, isrc: Option<&str>, mbid: Option<&str>, source_id: Option<String>, cover: Option<&str>) -> Value {
    let isrc = isrc.map(|i| i.to_uppercase().replace('-', "")).filter(|i| !i.is_empty());
    json!({ "title": title.trim(), "artist": artist.trim(), "album": album, "duration": duration.filter(|d| *d > 0.0), "isrc": isrc,
            "mbid": mbid, "source_id": source_id, "cover": cover })
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

pub fn detect(url: &str) -> Option<&'static str> {
    let rules = [
        ("spotify", r"(?i)open\.spotify\.com|^spotify:|spotify\.link"), ("deezer", r"(?i)deezer\.com|deezer\.page\.link|link\.deezer\.com"),
        ("tidal", r"(?i)tidal\.com"), ("qobuz", r"(?i)qobuz\.com"), ("apple", r"(?i)music\.apple\.com"), ("listenbrainz", r"(?i)listenbrainz\.org/playlist"),
        ("ytdlp", r"(?i)youtube\.com|youtu\.be|soundcloud\.com|bandcamp\.com|mixcloud\.com|audiomack\.com|vimeo\.com"),
    ];
    for (name, pat) in rules {
        if Regex::new(pat).unwrap().is_match(url.trim()) {
            return Some(name);
        }
    }
    url.trim().starts_with("http").then_some("web")
}

pub async fn fetch(st: &AppState, url: &str) -> anyhow::Result<Value> {
    let kind = detect(url).ok_or_else(|| anyhow::anyhow!("Unsupported link"))?;
    let mut data = match kind {
        "spotify" => spotify(st, url).await?,
        "deezer" => deezer(st, url).await?,
        "tidal" => tidal(st, url).await?,
        "apple" => apple(st, url).await?,
        "listenbrainz" => listenbrainz(st, url).await?,
        "ytdlp" => ytdlp_playlist(url).await?,
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

/// "Artist - Title" (or "Title by Artist", or CSV artist,title) per line.
pub fn parse_text(text: &str) -> anyhow::Result<Value> {
    let num = Regex::new(r"^\d+[\.\)]\s*").unwrap();
    let dash = Regex::new(r"\s[-–—]\s").unwrap();
    let by = Regex::new(r"(?i)\sby\s").unwrap();
    let mut tracks = vec![];
    for line in text.lines() {
        let line = line.trim().trim_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = num.replace(line, "").to_string();
        let (artist, title) = if dash.is_match(&line) {
            let mut it = dash.splitn(&line, 2);
            (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
        } else if by.is_match(&line) {
            let mut it = by.splitn(&line, 2);
            let title = it.next().unwrap_or("").to_string();
            (it.next().unwrap_or("").to_string(), title)
        } else if let Some((a, b)) = line.split_once(',') {
            (a.trim().trim_matches('"').to_string(), b.trim().trim_matches('"').to_string())
        } else if let Some((a, b)) = line.split_once('\t') {
            (a.to_string(), b.to_string())
        } else {
            continue;
        };
        tracks.push(t(&title, &artist, None, None, None, None, None, None));
    }
    if tracks.is_empty() {
        anyhow::bail!("Couldn't read any 'Artist - Title' lines");
    }
    Ok(json!({ "name": "Imported list", "description": "", "cover": null, "source": "text", "tracks": tracks }))
}

async fn resolve_short(st: &AppState, url: &str) -> String {
    if Regex::new(r"spotify\.link|deezer\.page\.link|link\.deezer\.com").unwrap().is_match(url) {
        if let Ok(r) = st.http.get(url).header("User-Agent", UA).send().await {
            return r.url().to_string();
        }
    }
    url.to_string()
}

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

fn sp_track(x: &Value, album: Option<&Value>) -> Value {
    let alb = album.unwrap_or(&x["album"]);
    let artists = x["artists"].as_array().map(|a| a.iter().map(|a| s(&a["name"])).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let cover = alb["images"][0]["url"].as_str();
    t(s(&x["name"]), &artists, alb["name"].as_str(), x["duration_ms"].as_f64().map(|d| d / 1000.0), x["external_ids"]["isrc"].as_str(), None,
      x["id"].as_str().map(String::from), cover)
}

async fn spotify_app_token(st: &AppState) -> Option<String> {
    let (cid, sec) = (st.cfg.str("spotify_client_id"), st.cfg.str("spotify_client_secret"));
    if cid.is_empty() || sec.is_empty() {
        return None;
    }
    let r = st.http.post("https://accounts.spotify.com/api/token").basic_auth(cid, Some(sec)).form(&[("grant_type", "client_credentials")]).send().await.ok()?;
    r.json::<Value>().await.ok()?["access_token"].as_str().map(String::from)
}

pub async fn spotify_api(st: &AppState, token: &str, kind: &str, id: &str) -> anyhow::Result<Value> {
    let base = "https://api.spotify.com/v1";
    let get = |u: String| async move {
        let mut r = st.http.get(&u).bearer_auth(token).send().await?;
        if r.status().as_u16() == 429 {
            let wait = r.headers().get("retry-after").and_then(|h| h.to_str().ok()).and_then(|s| s.parse().ok()).unwrap_or(2);
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            r = st.http.get(&u).bearer_auth(token).send().await?;
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

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#x27;", "'").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

/// No-credentials fallback: Spotify's public embed player ships the track list as JSON.
async fn spotify_embed(st: &AppState, kind: &str, id: &str) -> anyhow::Result<Value> {
    let r = st.http.get(format!("https://open.spotify.com/embed/{kind}/{id}")).header("User-Agent", UA).send().await?;
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

async fn spotify(st: &AppState, url: &str) -> anyhow::Result<Value> {
    let url = resolve_short(st, url).await;
    let re = Regex::new(r"(?:open\.spotify\.com/(?:intl-[\w-]+/)?(?:user/[^/]+/)?|spotify:)(playlist|album|track|artist|collection)[/:]?([A-Za-z0-9]*)").unwrap();
    let c = re.captures(&url).ok_or_else(|| anyhow::anyhow!("Not a Spotify playlist/album/track link"))?;
    let (kind, id) = (c[1].to_string(), c[2].to_string());
    let mut errors = vec![];
    if kind != "collection" {
        if let Some(tok) = spotify_app_token(st).await {
            match spotify_api(st, &tok, &kind, &id).await {
                Ok(v) => return Ok(v),
                Err(e) => errors.push(e.to_string()),
            }
        }
    } else {
        anyhow::bail!("Liked Songs need your Spotify account connected (Connections page)");
    }
    match spotify_embed(st, &kind, &id).await {
        Ok(mut v) => {
            v["description"] = json!("Imported without Spotify login – connect Spotify for ISRCs and >100-track playlists");
            Ok(v)
        }
        Err(e) => {
            errors.push(e.to_string());
            anyhow::bail!("{}", errors.join("; "))
        }
    }
}

// ---- Deezer -------------------------------------------------------------------------------------

async fn deezer(st: &AppState, url: &str) -> anyhow::Result<Value> {
    let url = resolve_short(st, url).await;
    let re = Regex::new(r"deezer\.com/(?:[a-z]{2}/)?(playlist|album|track|profile)/(\d+)").unwrap();
    let c = re.captures(&url).ok_or_else(|| anyhow::anyhow!("Not a Deezer playlist/album/track link"))?;
    let (kind, id) = (c[1].to_string(), c[2].to_string());
    let api = "https://api.deezer.com";
    let get = |u: String| async move {
        let d: Value = st.http.get(&u).send().await?.json().await?;
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
        let x = get(format!("{api}/track/{id}")).await?;
        return Ok(json!({ "name": x["title"], "description": "", "cover": null, "tracks": [dz(&x, None)] }));
    }
    let (meta, mut next, name, cover) = if kind == "profile" {
        let m = get(format!("{api}/user/{id}")).await?;
        let n = format!("{}'s Deezer favourites", s(&m["name"]));
        let c = m["picture_xl"].clone();
        (m, Some(format!("{api}/user/{id}/tracks?limit=100")), n, c)
    } else {
        let m = get(format!("{api}/{kind}/{id}")).await?;
        let n = s(&m["title"]).to_string();
        let c = if m["picture_xl"].is_string() { m["picture_xl"].clone() } else { m["cover_xl"].clone() };
        (m, Some(format!("{api}/{kind}/{id}/tracks?limit=100")), n, c)
    };
    let album = (kind == "album").then_some(&meta);
    let mut tracks = vec![];
    while let Some(u) = next.filter(|_| tracks.len() < 10000) {
        let page = get(u).await?;
        tracks.extend(page["data"].as_array().cloned().unwrap_or_default().iter().map(|x| dz(x, album)));
        next = page["next"].as_str().map(String::from);
    }
    Ok(json!({ "name": name, "description": s(&meta["description"]), "cover": cover, "tracks": tracks }))
}

// ---- Tidal (developer API, client credentials) ---------------------------------------------------

fn iso_dur(v: &str) -> Option<f64> {
    let c = Regex::new(r"^P(?:T)?(?:(\d+)H)?(?:(\d+)M)?(?:([\d.]+)S)?").unwrap().captures(v)?;
    let g = |i| c.get(i).and_then(|m: regex::Match| m.as_str().parse::<f64>().ok()).unwrap_or(0.0);
    Some(g(1) * 3600.0 + g(2) * 60.0 + g(3))
}

async fn tidal(st: &AppState, url: &str) -> anyhow::Result<Value> {
    let c = Regex::new(r"tidal\.com/(?:browse/)?(playlist|album|track|mix)/([\w-]+)").unwrap().captures(url)
        .ok_or_else(|| anyhow::anyhow!("Not a Tidal playlist/album/track link"))?;
    let (kind, id) = (c[1].to_string(), c[2].to_string());
    if kind == "mix" {
        anyhow::bail!("Tidal mixes can't be read through the public API – save it as a playlist first");
    }
    let token = super::streamrip::tidal_token(st).await?;
    let base = "https://openapi.tidal.com/v2";
    let get = |path: String, q: Vec<(String, String)>| {
        let token = token.clone();
        async move {
            let u = if path.starts_with("http") { path } else { format!("{base}{path}") };
            let r = st.http.get(&u).query(&q).bearer_auth(&token).header("Accept", "application/vnd.api+json").send().await?;
            if r.status().as_u16() >= 400 {
                let c = r.status().as_u16();
                anyhow::bail!("Tidal API {c}: {}", r.text().await.unwrap_or_default().chars().take(200).collect::<String>());
            }
            Ok::<Value, anyhow::Error>(r.json().await?)
        }
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
            next = page["links"]["next"].as_str().map(|n| if n.starts_with("http") || !n.starts_with('/') { n.to_string() } else { format!("{base}{n}") });
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
            tracks.push(t(&title, &names("artists", "artists", "name").join(", "), albums.first().map(|x| x.as_str()), a["duration"].as_str().and_then(iso_dur),
                          a["isrc"].as_str(), None, Some(tid.clone()), None));
        }
    }
    let name = attrs["name"].as_str().or(attrs["title"].as_str()).unwrap_or("Tidal import");
    Ok(json!({ "name": name, "description": s(&attrs["description"]), "cover": null, "tracks": tracks }))
}

// ---- Apple Music / Qobuz / any page with schema.org data ------------------------------------------

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
        t(s(&x["name"]), &artist, x["inAlbum"]["name"].as_str(), x["duration"].as_str().and_then(iso_dur), x["isrcCode"].as_str(), None, None, None)
    }).collect()
}

async fn page(st: &AppState, url: &str, what: &str) -> anyhow::Result<String> {
    let r = st.http.get(url).header("User-Agent", UA).header("Accept-Language", "en-US,en;q=0.9").send().await?;
    if r.status().as_u16() >= 400 {
        anyhow::bail!("{what} returned {}", r.status().as_u16());
    }
    Ok(r.text().await?)
}

async fn web_jsonld(st: &AppState, url: &str) -> anyhow::Result<Value> {
    let html = page(st, url, "Page").await?;
    let ld = jsonld(&html).ok_or_else(|| anyhow::anyhow!("No track list found on that page"))?;
    let name = ld["name"].as_str().unwrap_or("Imported playlist");
    Ok(json!({ "name": name, "description": s(&ld["description"]), "cover": ld["image"].as_str(), "tracks": ld_tracks(&ld) }))
}

async fn apple(st: &AppState, url: &str) -> anyhow::Result<Value> {
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

async fn listenbrainz(st: &AppState, url: &str) -> anyhow::Result<Value> {
    let c = Regex::new(r"playlist/([0-9a-f-]{36})").unwrap().captures(url).ok_or_else(|| anyhow::anyhow!("Not a ListenBrainz playlist link"))?;
    let r = st.http.get(format!("https://api.listenbrainz.org/1/playlist/{}", &c[1])).send().await?;
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

async fn ytdlp_playlist(url: &str) -> anyhow::Result<Value> {
    let info = super::ytdlp::playlist(url).await.map_err(|e| anyhow::anyhow!("Couldn't read that link: {}", e.to_string().chars().take(200).collect::<String>()))?;
    let entries = info["entries"].as_array().cloned().unwrap_or_else(|| vec![info.clone()]);
    let junk = Regex::new(r"(?i)\s*[\(\[](official|lyric|audio|music video|visuali[sz]er|hd|hq|4k)[^\)\]]*[\)\]]").unwrap();
    let tracks: Vec<Value> = entries.iter().filter(|e| e.is_object()).map(|e| {
        let mut title = e["track"].as_str().or(e["title"].as_str()).unwrap_or("").to_string();
        let mut artist = e["artist"].as_str().or(e["creator"].as_str()).unwrap_or("").to_string();
        let ch = e["channel"].as_str().or(e["uploader"].as_str()).unwrap_or("");
        if artist.is_empty() {
            if title.contains(" - ") && !ch.ends_with(" - Topic") {
                let (a, b) = title.split_once(" - ").unwrap();
                (artist, title) = (a.to_string(), b.to_string());
            } else {
                artist = ch.replace(" - Topic", "");
            }
        }
        let title = junk.replace_all(&title, "").to_string();
        let sid = e["url"].as_str().or(e["webpage_url"].as_str()).or(e["id"].as_str()).map(String::from);
        t(&title, &artist, e["album"].as_str(), e["duration"].as_f64(), None, None, sid, None)
    }).collect();
    let name = info["title"].as_str().unwrap_or("Imported playlist");
    Ok(json!({ "name": name, "description": s(&info["description"]), "cover": info["thumbnail"], "tracks": tracks }))
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
}
