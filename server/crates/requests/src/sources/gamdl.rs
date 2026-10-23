//! gamdl (subprocess): Apple Music, searched in the catalog of the owner's storefront and downloaded with gamdl
//! (github.com/glomatico/gamdl) signed in with the owner's media-user-token: AAC 256k on its own, ALAC and
//! Dolby Atmos through a wrapper-v2 server the owner runs. The gamdl plugin installs gamdl from PyPI into its
//! venv; this is its code, reached through the registry like a plugin program's source
//! (crate::sources::plugin): its results are the SDK's `result()` rows.
use super::pytool::{self, Spec};
use crate::Ctx;
use base64::Engine;
use encore_plugins::{Call, Code, Downloaded, Failure, FileEntry, Status};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// The plugin and program id.
pub const ID: &str = "gamdl";
/// Apple Music's web player, the origin the AMP API expects.
const HOME: &str = "https://music.apple.com";
/// Apple's AMP catalog and account API.
const AMP: &str = "https://amp-api.music.apple.com";
/// The cookie domain gamdl's cookies file names.
const COOKIE_DOMAIN: &str = ".music.apple.com";
/// the catalog search's most per type
const SEARCH_LIMIT: usize = 25;
/// the wrapper-v2 API version gamdl 3.9 checks for
const WRAPPER_API: &str = "0.0.2";

/// A sign-in failure with this message.
fn auth(message: impl Into<String>) -> Failure {
    Failure::new(Code::Auth, message)
}

/// A network failure: Apple Music couldn't be reached.
fn net(e: impl std::fmt::Display) -> Failure {
    Failure::new(Code::Network, format!("Apple Music can't be reached: {e}"))
}

/// The codec setting: (gamdl's --song-codec-priority, the format shown, its bitrate, lossless, needs the wrapper).
fn codec(st: &impl Ctx) -> (&'static str, &'static str, i64, bool, bool) {
    codec_for(st, false)
}

/// [`codec`] for a request: one for the Surround library (`surround` in the request) is Dolby Atmos only, never
/// the AAC fallback; a stereo one never takes Atmos (a saved "atmos" Format reads as ALAC since 0.35: Atmos files
/// belong in the Surround library).
fn codec_for(st: &impl Ctx, surround: bool) -> (&'static str, &'static str, i64, bool, bool) {
    if surround {
        return ("atmos", "atmos", 768, false, true);
    }
    match st.cfg().str("gamdl_codec").as_str() {
        "aac-he-web" => ("aac-he-web", "aac", 64, false, false),
        "alac" => ("alac,aac-web", "alac", 0, true, true),
        "atmos" => ("alac,aac-web", "alac", 0, true, true),
        _ => ("aac-web", "aac", 256, false, false),
    }
}

/// The media-user-token setting; a pasted cookies.txt line or `media-user-token=…` works too.
pub fn media_user_token(st: &impl Ctx) -> String {
    token_in(&st.cfg().str("gamdl_media_user_token"))
}

/// The media-user-token from what the user pasted: a bare token, or a cookie line / cookies.txt row holding it.
fn token_in(raw: &str) -> String {
    let raw = raw.trim();
    let after = raw.find("media-user-token").map(|i| &raw[i + "media-user-token".len()..]);
    let v = match after {
        Some(rest) => rest.trim_start_matches(|c: char| c == '=' || c.is_whitespace()).split(|c: char| c.is_whitespace() || c == ';').next().unwrap_or(""),
        None => raw,
    };
    v.trim_matches('"').to_string()
}

/// The wrapper: (base URL, decrypt host, decrypt port).
fn wrapper(st: &impl Ctx) -> Option<(String, String, u16)> {
    let url = st.cfg().str("gamdl_wrapper_url").trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return None;
    }
    let host_of_url = url.split("://").nth(1).unwrap_or(&url).split(['/', ':']).next().unwrap_or("127.0.0.1").to_string();
    let dec = st.cfg().str("gamdl_wrapper_decrypt").trim().to_string();
    let (host, port) = match dec.rsplit_once(':') {
        Some((h, p)) if !dec.is_empty() => (if h.is_empty() { host_of_url } else { h.to_string() }, p.parse().unwrap_or(10020)),
        _ if !dec.is_empty() => (dec, 10020),
        _ => (host_of_url, 10020),
    };
    Some((url, host, port))
}

/// A JWT's `exp` claim (seconds since the epoch); 0 when it can't be read.
fn jwt_exp(token: &str) -> f64 {
    let Some(part) = token.split('.').nth(1) else { return 0.0 };
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(part.trim_end_matches('=')).ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["exp"].as_f64()).unwrap_or(0.0)
}

/// The web player's own token, read from music.apple.com's script as gamdl reads it; kept until it runs out.
async fn developer_token(st: &impl Ctx) -> Result<String, Failure> {
    let cache = pytool::data(st, ID).join("token.json");
    if let Some(t) = pytool::read_json(&cache).and_then(|c| c["token"].as_str().map(String::from)) {
        if jwt_exp(&t) - 3600.0 > encore_db::now() {
            return Ok(t);
        }
    }
    let get = |url: String| async move {
        st.http().get(url).timeout(Duration::from_secs(15)).send().await.map_err(net)?.text().await.map_err(net)
    };
    let page = get(HOME.to_string()).await?;
    let js = find_between(&page, "/assets/index", ".js").map(|mid| format!("{HOME}/assets/index{mid}.js"))
        .ok_or_else(|| Failure::new(Code::Network, "could not find the web player's script on music.apple.com"))?;
    let script = get(js).await?;
    let token = find_jwt(&script).ok_or_else(|| Failure::new(Code::Network, "could not find the web player's token on music.apple.com"))?;
    let _ = pytool::write_json(&cache, &json!({ "token": token }));
    Ok(token)
}

/// The part of `hay` between the first `start` and the next `end` (no quote or slash in it).
fn find_between<'a>(hay: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(i) = hay[from..].find(start) {
        let rest = &hay[from + i + start.len()..];
        if let Some(j) = rest.find(end) {
            let mid = &rest[..j];
            if (mid.starts_with('-') || mid.starts_with('~')) && !mid.contains(['"', '/', '\'']) {
                return Some(mid);
            }
        }
        from += i + start.len();
    }
    None
}

/// The first quoted JWT ("eyJ….eyJ….sig") in a script.
fn find_jwt(script: &str) -> Option<String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    script.match_indices("\"eyJ").find_map(|(i, _)| {
        let rest = &script[i + 1..];
        let end = rest.find('"')?;
        let t = &rest[..end];
        let parts: Vec<&str> = t.split('.').collect();
        (parts.len() == 3 && parts[1].starts_with("eyJ") && parts.iter().all(|p| !p.is_empty() && p.chars().all(ok))).then(|| t.to_string())
    })
}

/// GET `uri` on Apple's AMP API with the developer token and the language; with `user`, the media-user-token too.
async fn amp(st: &impl Ctx, uri: &str, query: &[(&str, String)], user: bool) -> Result<Value, Failure> {
    let token = developer_token(st).await?;
    let lang = Some(st.cfg().str("gamdl_language")).filter(|l| !l.trim().is_empty()).unwrap_or_else(|| "en-US".into());
    let mut q: Vec<(&str, String)> = query.to_vec();
    q.push(("l", lang));
    let mut r = st.http().get(format!("{AMP}{uri}")).query(&q).bearer_auth(&token).header("Origin", HOME).header("Accept", "application/json").timeout(Duration::from_secs(15));
    if user {
        let mut mut_ = media_user_token(st);
        if mut_.is_empty() {
            return Err(auth("Add your Apple Music media-user-token on the gamdl card"));
        }
        mut_ = mut_.trim().to_string();
        r = r.header("Media-User-Token", &mut_).header("Cookie", format!("media-user-token={mut_}"));
    }
    let r = r.send().await.map_err(net)?;
    let status = r.status();
    match status.as_u16() {
        200..=299 => r.json().await.map_err(net),
        401 | 403 if user => Err(auth("Apple Music did not accept the media-user-token: copy a fresh one from music.apple.com")),
        401 | 403 => {
            let _ = std::fs::remove_file(pytool::data(st, ID).join("token.json")); // the cached web token was revoked early
            Err(Failure::new(Code::Network, "Apple Music refused the web player's token: try again"))
        }
        404 => Ok(json!({ "data": [] })),
        429 => Err(Failure { code: Code::RateLimited, message: "Apple Music is rate-limiting this server".into(), retry_after: Some(60.0) }),
        _ => Err(Failure::new(Code::Network, format!("Apple Music answered {status}"))),
    }
}

/// The subscription behind the media-user-token: (active, storefront). Kept for an hour.
async fn account(st: &impl Ctx, fresh: bool) -> Result<(bool, String), Failure> {
    let mut_ = media_user_token(st);
    let cache = pytool::data(st, ID).join("account.json");
    let tail: String = mut_.chars().rev().take(12).collect();
    if let Some(c) = pytool::read_json(&cache).filter(|c| !fresh && c["for"] == tail.as_str() && encore_db::now() - c["checked"].as_f64().unwrap_or(0.0) < 3600.0) {
        return Ok((c["active"] == true, c["storefront"].as_str().unwrap_or("").to_string()));
    }
    let d = amp(st, "/v1/me/account", &[("meta", "subscription".into())], true).await?;
    let sub = &d["meta"]["subscription"];
    let (active, sf) = (sub["active"] == true, sub["storefront"].as_str().unwrap_or("").to_string());
    let _ = pytool::write_json(&cache, &json!({ "for": tail, "active": active, "storefront": sf, "checked": encore_db::now() }));
    Ok((active, sf))
}

/// The storefront to search: the setting, else the signed-in account's, else "us".
async fn storefront(st: &impl Ctx) -> String {
    let sf = st.cfg().str("gamdl_storefront").trim().to_lowercase();
    if !sf.is_empty() {
        return sf;
    }
    if !media_user_token(st).is_empty() {
        if let Ok((_, sf)) = account(st, false).await {
            if !sf.is_empty() {
                return sf;
            }
        }
    }
    "us".into()
}

// ---- search ----------------------------------------------------------------------------------------------

/// An item's artwork URL at 1200×1200 JPEG, or null.
fn cover(a: &Value) -> Value {
    a["artwork"]["url"].as_str().map(|u| json!(u.replace("{w}", "1200").replace("{h}", "1200").replace("{f}", "jpg").replace("{c}", ""))).unwrap_or(Value::Null)
}

/// What gamdl delivers for an item with these audio traits: (format, bitrate, lossless, bit depth).
fn quality_for(st: &impl Ctx, traits: &Value, surround: bool) -> (&'static str, i64, bool, i64) {
    let (_, fmt, rate, lossless, wrapped) = codec_for(st, surround);
    let has = |t: &str| traits.as_array().is_some_and(|a| a.iter().any(|x| x.as_str().is_some_and(|s| s.eq_ignore_ascii_case(t))));
    match fmt {
        "alac" if has("lossless") => ("alac", 0, true, if has("hi-res-lossless") { 24 } else { 16 }),
        "atmos" if has("atmos") => ("atmos", 768, false, 0),
        // the item has no such stream: gamdl falls back to AAC 256k
        _ if wrapped => ("aac", 256, false, 0),
        _ => (fmt, rate, lossless, 0),
    }
}

/// Whether Apple's `audioTraits` list holds `t` (any case).
fn has_trait(traits: &Value, t: &str) -> bool {
    traits.as_array().is_some_and(|a| a.iter().any(|x| x.as_str().is_some_and(|s| s.eq_ignore_ascii_case(t))))
}

/// The quality shown for a format: "ALAC 24-bit", "Dolby Atmos", "HE-AAC 64k" or "AAC 256k".
fn label(fmt: &str, rate: i64, depth: i64) -> String {
    match fmt {
        "alac" if depth > 0 => format!("ALAC {depth}-bit"),
        "alac" => "ALAC".into(),
        "atmos" => "Dolby Atmos".into(),
        "aac" if rate == 64 => "HE-AAC 64k".into(),
        _ => format!("{} {rate}k", fmt.to_uppercase()),
    }
}

/// How much of `needle`'s words `hay` holds (as torznab scores); 1 when there's nothing to match.
fn share(hay: &str, needle: &str) -> f64 {
    if needle.trim().is_empty() { 1.0 } else { super::torznab::share(hay, needle) }
}

/// A song from the catalog search as a search result; `None` when it can't be streamed in this storefront.
fn song_result(st: &impl Ctx, req: &Value, s: &Value, sf: &str, by_isrc: bool) -> Option<Value> {
    let id = s["id"].as_str()?;
    let a = &s["attributes"];
    if a["playParams"].is_null() {
        return None; // not streamable in this storefront
    }
    let (title, artist, album) = (a["name"].as_str().unwrap_or(""), a["artistName"].as_str().unwrap_or(""), a["albumName"].as_str().unwrap_or(""));
    let dur = a["durationInMillis"].as_f64().map(|ms| ms / 1000.0).filter(|d| *d > 0.0);
    let rel = share(title, req["title"].as_str().unwrap_or(""));
    if rel < 0.5 && !by_isrc {
        return None;
    }
    let surround = req["surround"] == true;
    if surround && !has_trait(&a["audioTraits"], "atmos") {
        return None; // a Surround request takes Dolby Atmos only
    }
    let (fmt, rate, lossless, depth) = quality_for(st, &a["audioTraits"], surround);
    if req["lossless_only"] == true && !lossless {
        return None;
    }
    let want_artist = req["artist"].as_str().unwrap_or("");
    let mut score = rel * 50.0 + if want_artist.is_empty() { 10.0 } else { share(artist, want_artist) * 20.0 } + if lossless { 20.0 } else { 8.0 };
    if let (Some(want), Some(d)) = (req["length"].as_f64(), dur) {
        score += if (d - want).abs() < 3.0 { 10.0 } else { -10.0 };
    }
    let isrc = a["isrc"].as_str();
    if by_isrc || isrc.zip(req["isrc"].as_str()).is_some_and(|(x, y)| x.eq_ignore_ascii_case(y)) {
        score += 20.0;
    }
    let info = [a["releaseDate"].as_str().unwrap_or("").chars().take(4).collect::<String>(),
        if a["contentRating"] == "explicit" { "explicit".into() } else { String::new() },
        if a["isAppleDigitalMaster"] == true { "Apple Digital Master".into() } else { String::new() }]
        .into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    let url = a["url"].as_str().map(String::from).unwrap_or_else(|| format!("{HOME}/{sf}/song/{id}"));
    let subtitle = [artist, album].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
    Some(json!({
        "key": format!("am:song:{id}"), "title": title, "artist": artist, "album": album, "subtitle": subtitle,
        "format": if fmt == "aac" { "m4a" } else { fmt }, "bitrate": rate, "bit_depth": depth, "sample_rate": 0, "quality": label(fmt, rate, depth), "lossless": lossless, "tracks": 1,
        "size": dur.filter(|_| rate > 0).map(|d| (rate as f64 * 1000.0 / 8.0 * d) as i64), "duration": dur, "isrc": isrc, "cover": cover(a), "info": info,
        "score": (score.clamp(0.0, 100.0) * 10.0).round() / 10.0, "url": url,
        "payload": { "url": url, "kind": "song", "tracks": 1, "title": title, "artist": artist, "album": album, "track": a["trackNumber"], "disc": a["discNumber"] },
    }))
}

/// An album from the catalog search as a search result; `None` when it can't be streamed in this storefront.
fn album_result(st: &impl Ctx, req: &Value, al: &Value, sf: &str) -> Option<Value> {
    let id = al["id"].as_str()?;
    let a = &al["attributes"];
    if a["playParams"].is_null() {
        return None;
    }
    let (title, artist, n) = (a["name"].as_str().unwrap_or(""), a["artistName"].as_str().unwrap_or(""), a["trackCount"].as_i64());
    let bare = title.trim_end_matches(" - Single").trim_end_matches(" - EP");
    let want = req["album"].as_str().filter(|s| !s.is_empty()).or(req["title"].as_str()).unwrap_or("");
    let rel = share(title, want).max(share(bare, want));
    if rel < 0.5 {
        return None;
    }
    let surround = req["surround"] == true;
    if surround && !has_trait(&a["audioTraits"], "atmos") {
        return None; // a Surround request takes Dolby Atmos only
    }
    let (fmt, rate, lossless, depth) = quality_for(st, &a["audioTraits"], surround);
    if req["lossless_only"] == true && !lossless {
        return None;
    }
    let want_artist = req["artist"].as_str().unwrap_or("");
    let mut score = rel * 50.0 + if want_artist.is_empty() { 10.0 } else { share(artist, want_artist) * 20.0 } + if lossless { 20.0 } else { 8.0 };
    if let (Some(want), Some(n)) = (req["track_count"].as_i64(), n) {
        score += if n == want { 10.0 } else if n < want { -10.0 } else { 0.0 };
    }
    let kind = if a["isSingle"] == true { "single" } else if a["isCompilation"] == true { "compilation" } else { "" };
    let info = [a["releaseDate"].as_str().unwrap_or("").chars().take(4).collect::<String>(), kind.to_string(), n.map(|n| format!("{n} tracks")).unwrap_or_default(),
        if a["contentRating"] == "explicit" { "explicit".into() } else { String::new() }]
        .into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    let url = a["url"].as_str().map(String::from).unwrap_or_else(|| format!("{HOME}/{sf}/album/{id}"));
    Some(json!({
        "key": format!("am:album:{id}"), "title": bare, "artist": artist, "album": bare, "subtitle": artist,
        "format": if fmt == "aac" { "m4a" } else { fmt }, "bitrate": rate, "bit_depth": depth, "sample_rate": 0, "quality": label(fmt, rate, depth), "lossless": lossless, "tracks": n,
        "size": null, "duration": null, "isrc": null, "cover": cover(a), "info": info, "score": (score.clamp(0.0, 100.0) * 10.0).round() / 10.0, "url": url,
        "payload": { "url": url, "kind": "album", "tracks": n.unwrap_or(0), "title": bare, "artist": artist, "album": bare },
    }))
}

/// A request's search (the SDK's request fields) → results, best first.
pub async fn search(st: &impl Ctx, req: &Value) -> Result<Vec<Value>, Failure> {
    let sf = storefront(st).await;
    let s = |k: &str| req[k].as_str().unwrap_or("").trim().to_string();
    let album = req["kind"] == "album";
    let isrc: String = s("isrc").chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    let mut out: Vec<Value> = vec![];
    let mut keep = |r: Option<Value>| {
        let Some(r) = r else { return };
        match out.iter_mut().find(|x| x["key"] == r["key"]) {
            Some(x) if x["score"].as_f64() < r["score"].as_f64() => *x = r,
            Some(_) => {}
            None => out.push(r),
        }
    };
    if !album && !isrc.is_empty() {
        let d = amp(st, &format!("/v1/catalog/{sf}/songs"), &[("filter[isrc]", isrc.clone())], false).await?;
        for x in d["data"].as_array().cloned().unwrap_or_default() {
            keep(song_result(st, req, &x, &sf, true));
        }
    }
    let typ = if album { "albums" } else { "songs" };
    let name = if album && !s("album").is_empty() { s("album") } else { s("title") };
    let term = Some(s("query")).filter(|q| !q.is_empty()).unwrap_or_else(|| [s("artist"), name].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" "));
    if !term.is_empty() {
        let d = amp(st, &format!("/v1/catalog/{sf}/search"), &[("term", term), ("types", typ.into()), ("limit", SEARCH_LIMIT.to_string())], false).await?;
        for x in d["results"][typ]["data"].as_array().cloned().unwrap_or_default() {
            keep(if album { album_result(st, req, &x, &sf) } else { song_result(st, req, &x, &sf, false) });
        }
    }
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(req["limit"].as_u64().unwrap_or(25) as usize);
    Ok(out)
}

// ---- download --------------------------------------------------------------------------------------------

/// gamdl reads a Netscape cookies.txt; only media-user-token matters, so that is all it gets.
fn write_cookies(st: &impl Ctx) -> Result<String, Failure> {
    let mut_ = media_user_token(st);
    if mut_.is_empty() {
        return Err(auth("Add your Apple Music media-user-token on the gamdl card"));
    }
    let p = pytool::data(st, ID).join("cookies.txt");
    let expires = encore_db::now() as i64 + 180 * 86400;
    let text = format!("# Netscape HTTP Cookie File\n{COOKIE_DOMAIN}\tTRUE\t/\tTRUE\t{expires}\tmedia-user-token\t{mut_}\n");
    pytool::write_private(&p, &text).map_err(|e| Failure::internal(e.to_string()))?;
    Ok(p.to_string_lossy().into_owned())
}

/// gamdl's command line: the codec, the wrapper when the codec needs it, cookies, ffmpeg and the output layout.
fn args(st: &impl Ctx, folder: &Path, ffmpeg: &str, surround: bool) -> Result<Vec<String>, Failure> {
    let (prio, _, _, _, wrapped) = codec_for(st, surround);
    let w = wrapper(st);
    if surround && w.is_none() {
        return Err(Failure::unsupported("Dolby Atmos needs a wrapper-v2 server: set \"Wrapper URL\" on the gamdl card"));
    }
    if wrapped && w.is_none() {
        return Err(Failure::unsupported("ALAC and Dolby Atmos need a wrapper-v2 server: set \"Wrapper URL\" on the gamdl card, or choose AAC"));
    }
    let tmp = pytool::data(st, ID).join("tmp");
    let _ = std::fs::create_dir_all(&tmp);
    let lang = Some(st.cfg().str("gamdl_language")).filter(|l| !l.trim().is_empty()).unwrap_or_else(|| "en-US".into());
    let mut a: Vec<String> = ["--no-config-file", "--log-level", "INFO", "--no-exceptions", "--output-path", &folder.to_string_lossy(), "--temp-path", &tmp.to_string_lossy(),
        "--song-codec-priority", prio, "--language", &lang, "--ffmpeg-path", ffmpeg].iter().map(|s| s.to_string()).collect();
    match st.cfg().str("gamdl_lyrics").as_str() {
        "none" => a.push("--no-synced-lyrics".into()),
        f @ ("srt" | "ttml") => a.extend(["--synced-lyrics-format".into(), f.into()]),
        _ => a.extend(["--synced-lyrics-format".into(), "lrc".into()]),
    }
    if let Some((url, host, port)) = &w {
        a.extend(["--use-wrapper".into(), "--wrapper-url".into(), url.clone(), "--wrapper-decrypt-host".into(), host.clone(), "--wrapper-decrypt-port".into(), port.to_string()]);
    }
    if !media_user_token(st).is_empty() || w.is_none() {
        a.extend(["--cookies-path".into(), write_cookies(st)?]);
    }
    Ok(a)
}

/// gamdl's `[Track  3/12 ] Downloading "…"` lines: the song it starts.
fn track_line(line: &str) -> Option<(f64, f64)> {
    if !line.contains("Downloading") {
        return None;
    }
    let rest = &line[line.find("Track")? + 5..];
    let (n, total) = rest.trim_start().split_once('/')?;
    let total: String = total.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    Some((n.trim().parse().ok()?, total.parse().ok()?))
}

/// Why a run produced no file, from what it printed.
fn classify(success: bool, code: Option<i32>, tail: &[String]) -> Failure {
    let text = tail.iter().rev().take(12).cloned().collect::<Vec<_>>().join("\n").to_lowercase();
    let last = tail.iter().rev().find(|l| { let l = l.to_lowercase(); l.contains("error") || l.contains("skipping") }).map(|l| l.chars().take(300).collect::<String>())
        .unwrap_or_else(|| pytool::last_message(tail));
    if text.contains("media-user-token") || text.contains("subscription") || text.contains("401") {
        return auth("Apple Music did not accept the media-user-token: copy a fresh one from music.apple.com");
    }
    if text.contains("wrapper") && (text.contains("logged_out") || text.contains("not authenticated") || text.contains("connect")) {
        return auth(format!("The wrapper is not reachable or not signed in: {last}"));
    }
    if text.contains("decryption") || (text.contains("format") && text.contains("not available")) {
        return Failure::unsupported(format!("That format can't be fetched for this item: {last}"));
    }
    if text.contains("not streamable") || text.contains("404") {
        return Failure::new(Code::NotFound, format!("Apple Music has no stream for this in your storefront: {last}"));
    }
    if text.contains("error fetching") || text.contains("connecterror") || text.contains("timed out") {
        return Failure::new(Code::Network, format!("gamdl could not reach Apple Music: {last}"));
    }
    if text.contains("429") || text.contains("too many") {
        return Failure { code: Code::RateLimited, message: "Apple Music is rate-limiting this account".into(), retry_after: Some(300.0) };
    }
    Failure::internal(format!("gamdl produced no file ({}): {last}", if success { "it finished".to_string() } else { format!("exit code {}", code.unwrap_or(-1)) }))
}

/// Download a result's Apple Music link with gamdl into `folder`; with surround, only the Dolby Atmos files are kept.
pub async fn download(st: &impl Ctx, call: &Call, req: &Value, folder: &Path) -> Result<Downloaded, Failure> {
    let payload = &req["payload"];
    let url = payload["url"].as_str().or(req["url"].as_str()).filter(|u| u.starts_with(HOME)).ok_or_else(|| Failure::new(Code::NotFound, "Nothing to download: the result has no Apple Music link"))?.to_string();
    let program = pytool::program(st, ID).ok_or_else(|| pytool::not_installed("gamdl"))?;
    let ffmpeg = encore_core::which::which("ffmpeg").ok_or_else(|| Failure::unsupported("gamdl needs ffmpeg on the server's PATH"))?;
    encore_core::fsx::mkdir_all(folder, encore_core::fsx::Step::Stage).map_err(|e| Failure::internal(e.to_string()))?;
    let expected = payload["tracks"].as_u64().unwrap_or(1).max(1) as usize;
    let hook = |line: &str| (track_line(line).map(|(n, total)| (n - 1.0) / total.max(1.0) * 100.0), true);
    let surround = req["surround"] == true;
    let mut a = args(st, folder, &ffmpeg.to_string_lossy(), surround)?;
    a.push(url.clone());
    let secrets = vec![media_user_token(st)];
    let run = pytool::run(st, Spec { id: ID, program, args: a, folder, expected, hook: Some(&hook), secrets, call }).await?;
    let mut files: Vec<_> = run.files.iter().filter(|p| !matches!(p.extension().and_then(|x| x.to_str()), Some("lrc" | "srt" | "ttml"))).cloned().collect();
    let mut not_atmos = 0;
    if surround {
        // only what really holds surround audio (should gamdl ever fall back to stereo, that copy goes)
        let (keep, stereo) = encore_library::surround::split(&files).await;
        not_atmos = stereo.len();
        for f in &stereo {
            let _ = std::fs::remove_file(f);
        }
        files = keep;
        if files.is_empty() && not_atmos > 0 {
            return Err(Failure::new(Code::NotFound, "Apple Music sent no Dolby Atmos stream for this"));
        }
    }
    if files.is_empty() {
        return Err(classify(run.success, run.code, &run.tail));
    }
    let single = expected == 1 && files.len() == 1;
    let s = |k: &str| payload[k].as_str().map(String::from);
    let entries = files.iter().map(|p| FileEntry {
        path: p.clone(), size: std::fs::metadata(p).ok().map(|m| m.len()), title: if single { s("title") } else { None }, artist: if single { s("artist") } else { None },
        album: s("album"), track: if single { payload["track"].as_i64() } else { None }, disc: if single { payload["disc"].as_i64() } else { None },
        duration: None, format: None, isrc: None,
    }).collect::<Vec<_>>();
    let (n, missing) = (entries.len(), expected.saturating_sub(entries.len()));
    let message = format!("Downloaded {n} file{} with gamdl{}{}", if n == 1 { "" } else { "s" },
        if surround { " in Dolby Atmos" } else { "" },
        if expected > 1 && missing > 0 { format!(" ({missing} without Dolby Atmos or skipped, see the log)") } else if not_atmos > 0 { format!(" ({not_atmos} without Dolby Atmos left out)") } else { String::new() });
    Ok(Downloaded { files: entries, source_url: Some(url), message: Some(message), cover_url: None })
}

// ---- status and test -------------------------------------------------------------------------------------

/// The quality the codec setting gives, as the card shows it.
fn quality_label(st: &impl Ctx) -> &'static str {
    match st.cfg().str("gamdl_codec").as_str() {
        "aac-he-web" => "HE-AAC 64k",
        "alac" => "ALAC through the wrapper",
        "atmos" => "Dolby Atmos through the wrapper",
        _ => "AAC 256k",
    }
}

/// The gamdl plugin can fetch Dolby Atmos: installed and on, signed in, with a wrapper.
pub fn atmos_ready(st: &impl Ctx) -> bool {
    st.plugins().enabled(ID) && wrapper(st).is_some() && pytool::program(st, ID).is_some()
}

/// The Dolby Atmos version of a song or album (the SDK's request fields): the best matching Atmos result, if
/// Apple has one.
pub async fn atmos_for(st: &impl Ctx, ask: &Value) -> Result<Option<Value>, Failure> {
    let mut req = ask.clone();
    req["surround"] = json!(true);
    let results = search(st, &req).await?;
    Ok(results.into_iter().next())
}

/// Whether gamdl can download as a subscriber: a media-user-token is set, or a wrapper server is.
pub fn signed_in(st: &impl Ctx) -> bool {
    !media_user_token(st).is_empty() || wrapper(st).is_some()
}

/// The card's status line: the gamdl version, whether the subscription is active, and the quality.
pub async fn status(st: &impl Ctx) -> Result<Status, Failure> {
    let v = st.plugins().program(ID).filter(|_| pytool::program(st, ID).is_some()).map(|(v, _)| v).ok_or_else(|| pytool::not_installed("gamdl"))?;
    if !signed_in(st) {
        return Ok(Status { message: format!("gamdl {v} · not signed in · add your media-user-token"), signed_in: Some(false) });
    }
    let acct = pytool::read_json(&pytool::data(st, ID).join("account.json"));
    let sub = match acct.as_ref().map(|a| a["active"] == true) {
        Some(true) => "subscription active",
        Some(false) => "no active subscription",
        None => "not checked yet, press Test",
    };
    Ok(Status { message: format!("gamdl {v} · {sub} · {}", quality_label(st)), signed_in: Some(acct.is_none_or(|a| a["active"] == true)) })
}

/// The card's test: gamdl and ffmpeg are installed, and the token (or the wrapper) is accepted.
pub async fn test(st: &impl Ctx) -> Result<String, Failure> {
    let v = st.plugins().program(ID).filter(|_| pytool::program(st, ID).is_some()).map(|(v, _)| v).ok_or_else(|| pytool::not_installed("gamdl"))?;
    if encore_core::which::which("ffmpeg").is_none() {
        return Err(Failure::unsupported("gamdl needs ffmpeg on the server's PATH"));
    }
    let (_, _, _, _, wrapped) = codec(st);
    let mut parts = vec![];
    match wrapper(st) {
        Some((url, ..)) => {
            let me: Value = st.http().get(format!("{url}/me")).timeout(Duration::from_secs(15)).send().await
                .map_err(|e| Failure::new(Code::Network, format!("The wrapper at {url} did not answer: {e}")))?.json().await.unwrap_or(Value::Null);
            if me["auth"]["state"] == "logged_out" {
                return Err(auth("The wrapper runs but is not signed in to Apple Music: sign it in first"));
            }
            let ver = me["version"].as_str().unwrap_or("?");
            parts.push(if ver == WRAPPER_API { format!("wrapper {ver} signed in") } else { format!("wrapper {ver} signed in (gamdl {v} expects {WRAPPER_API})") });
        }
        None if wrapped => return Err(Failure::unsupported("ALAC and Dolby Atmos need a wrapper-v2 server: set \"Wrapper URL\" on the gamdl card, or choose AAC")),
        None => {}
    }
    if !media_user_token(st).is_empty() {
        let (active, sf) = account(st, true).await?;
        if !active {
            return Err(auth("Apple Music accepted the token, but this account has no active subscription"));
        }
        parts.insert(0, format!("Signed in · Apple Music subscription active · storefront {}", if sf.is_empty() { "?" } else { &sf }));
    } else if wrapper(st).is_none() {
        return Err(auth("Add your Apple Music media-user-token on the gamdl card (or a wrapper URL)"));
    }
    parts.extend([format!("gamdl {v}"), quality_label(st).to_string()]);
    Ok(parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_read_from_what_was_pasted() {
        assert_eq!(token_in("abcdefghijklmnopqrstuvwxyz0123"), "abcdefghijklmnopqrstuvwxyz0123");
        assert_eq!(token_in("media-user-token=AbCdEfGhIjKlMnOpQrStUv12; other=1"), "AbCdEfGhIjKlMnOpQrStUv12");
        assert_eq!(token_in(".music.apple.com\tTRUE\t/\tTRUE\t1790000000\tmedia-user-token\tAbCd==\n"), "AbCd==");
        assert_eq!(token_in("  \"quoted\"  "), "quoted");
    }

    #[test]
    fn the_web_token_is_found_in_the_page() {
        let page = r#"<script src="/assets/index~abc123.js"></script><link href="/assets/index-legacy.css">"#;
        assert_eq!(find_between(page, "/assets/index", ".js"), Some("~abc123"));
        let script = r#"var x="eyJhbGciOiJFUzI1NiJ9.eyJleHAiOjE5MDAwMDAwMDB9.c2ln",y="eyJnot a token""#;
        let t = find_jwt(script).unwrap();
        assert_eq!(t, "eyJhbGciOiJFUzI1NiJ9.eyJleHAiOjE5MDAwMDAwMDB9.c2ln");
        assert_eq!(jwt_exp(&t), 1_900_000_000.0);
    }

    #[test]
    fn gamdl_track_lines_are_read() {
        assert_eq!(track_line(r#"12:00:00 [INFO] [Track   3/12  ] Downloading "Song""#), Some((3.0, 12.0)));
        assert_eq!(track_line("Track 3/12 skipped"), None);
    }

    #[test]
    fn failures_are_told_apart() {
        let t = |l: &str| classify(true, Some(0), &[l.to_string()]).code;
        assert_eq!(t("GamdlApiResponseError: Error fetching Apple Music homepage"), Code::Network);
        assert_eq!(t("\"media-user-token\" cookie not found in cookies"), Code::Auth);
        assert_eq!(t("Skipping \"Song\": GamdlInterfaceDecryptionNotAvailableError"), Code::Unsupported);
        assert_eq!(t("something else"), Code::Internal);
    }
}
