//! zotify (subprocess): Spotify, searched through the owner's own Spotify developer app (the Web API) and
//! downloaded with zotify (github.com/Googolplexed0/zotify) signed in with the owner's Spotify account. The
//! zotify plugin installs zotify into its venv; this is its code, reached through the registry like a plugin
//! program's source (crate::sources::plugin): its results are the SDK's `result()` rows.
//!
//! Signing in works on a server without a browser: Test answers with a Spotify link; the owner opens it, signs
//! in and pastes the address the browser ends on (`http://127.0.0.1:4381/login?code=…`, a page that does not
//! load) into "Sign-in link", then presses Test again. The first round signs in the account zotify streams
//! with (zotify's own client, its credentials kept as zotify's `credentials.json`), the second the developer
//! app that searches (a PKCE token with its refresh token, zotify's `api_credentials.json`). Both live in the
//! plugin's data folder, readable only by Encore's user.
use super::pytool::{self, Spec};
use crate::Ctx;
use base64::Engine;
use encore_plugins::{Call, Code, Downloaded, Failure, FileEntry, Status};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The plugin and program id.
pub const ID: &str = "zotify";
/// Spotify's sign-in page.
const AUTH: &str = "https://accounts.spotify.com/authorize";
/// Spotify's token endpoint.
const TOKEN: &str = "https://accounts.spotify.com/api/token";
/// Spotify's Web API.
const API: &str = "https://api.spotify.com/v1/";
/// Spotify's web player: result links start with it.
const OPEN: &str = "https://open.spotify.com/";
/// The redirect URI the user's Spotify app must list; the sign-in link lands on it (nothing listens there).
pub const REDIRECT: &str = "http://127.0.0.1:4381/login";
/// the client zotify and librespot sign the account in with (librespot's MercuryRequests.keymaster_client_id)
const KEYMASTER: &str = "65b708073fc0480ea92a077233ca87bd";
/// What the sign-in asks for: playback (for zotify's downloads) and the library and playlists.
const SCOPES: &str = "streaming playlist-read-private playlist-read-collaborative user-follow-read user-read-playback-position \
user-top-read user-read-recently-played user-library-read user-read-email user-read-private";
/// the most a Development Mode app may ask a search for since February 2026
const SEARCH_LIMIT: usize = 10;
/// The Python helper that turns the Web API sign-in into zotify's saved session.
const HELPER: &str = include_str!("assets/zotify_session.py");

/// zotify's saved Spotify session.
const CREDS: &str = "credentials.json";
/// The Web API token of the user's app, in zotify's format.
const API_CREDS: &str = "api_credentials.json";
/// The sign-in under way: its PKCE verifier and client id.
const PENDING: &str = "sign_in.json";
/// The signed-in account's name and plan, as last checked.
const ACCOUNT: &str = "account.json";
/// Hashes of the sign-in codes already used (a code works once).
const USED: &str = "used_codes.json";

/// A file in zotify's data folder.
fn file(st: &impl Ctx, name: &str) -> PathBuf {
    pytool::data(st, ID).join(name)
}

/// The Spotify app's client id from Settings.
fn client_id(st: &impl Ctx) -> String {
    st.cfg().str("zotify_client_id").trim().to_string()
}

/// A sign-in failure with this message.
fn auth(message: impl Into<String>) -> Failure {
    Failure::new(Code::Auth, message)
}

/// The Spotify account is signed in (zotify's credentials are saved).
pub fn signed_in(st: &impl Ctx) -> bool {
    file(st, CREDS).is_file()
}

/// Search can run: a developer app is set and signed in.
pub fn search_ready(st: &impl Ctx) -> bool {
    let id = client_id(st);
    !id.is_empty() && pytool::read_json(&file(st, API_CREDS)).is_some_and(|c| c["client_id"] == id.as_str())
}

// ---- signing in ----------------------------------------------------------------------------------------

/// A PKCE pair: a random verifier and its S256 challenge.
fn pkce_pair() -> (String, String) {
    use rand::Rng;
    let verifier: String = rand::rng().sample_iter(&rand::distr::Alphanumeric).take(96).map(char::from).collect();
    let digest = hex::decode(encore_core::ids::sha256_hex(&verifier)).unwrap_or_default();
    (verifier, base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest))
}

/// The link of a sign-in step. One waiting for this step is handed out again for 10 minutes, so a Test in
/// between does not spoil the link the owner is signing in with.
fn start_sign_in(st: &impl Ctx, stage: &str, client: &str) -> String {
    let path = file(st, PENDING);
    let now = encore_db::now();
    if let Some(p) = pytool::read_json(&path) {
        if p["stage"] == stage && p["client_id"] == client && now - p["created"].as_f64().unwrap_or(0.0) < 600.0 {
            if let Some(link) = p["link"].as_str() {
                return link.to_string();
            }
        }
    }
    let (verifier, challenge) = pkce_pair();
    let e = |s: &str| urlencoding::encode(s).into_owned();
    let link = format!(
        "{AUTH}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&scope={}",
        e(client), e(REDIRECT), e(&challenge), e(SCOPES)
    );
    let _ = pytool::write_json(&path, &json!({ "stage": stage, "client_id": client, "verifier": verifier, "link": link, "created": now }));
    link
}

/// The code in a pasted sign-in link (or the code itself).
pub fn code_from(link: &str) -> Result<Option<String>, Failure> {
    let link = link.trim();
    if link.is_empty() {
        return Ok(None);
    }
    if let Some((_, query)) = link.split_once('?') {
        let mut code = None;
        for (k, v) in query.split('&').filter_map(|kv| kv.split_once('=')) {
            let v = urlencoding::decode(v).map(|c| c.into_owned()).unwrap_or_else(|_| v.to_string());
            match k {
                "error" => return Err(auth(format!("Spotify refused the sign-in: {v}"))),
                "code" => code = Some(v),
                _ => {}
            }
        }
        return Ok(code);
    }
    Ok((link.len() >= 20 && link.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then(|| link.to_string()))
}

/// Whether this sign-in code was already used.
fn used(st: &impl Ctx, code: &str) -> bool {
    let h = encore_core::ids::sha256_hex(code);
    pytool::read_json(&file(st, USED)).and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().any(|x| *x == h.as_str())
}

/// Remember a sign-in code as used (its hash; the last 20 are kept).
fn mark_used(st: &impl Ctx, code: &str) {
    let mut seen = pytool::read_json(&file(st, USED)).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    seen.push(json!(encore_core::ids::sha256_hex(code)));
    let keep = seen.len().saturating_sub(20);
    let _ = pytool::write_json(&file(st, USED), &Value::Array(seen.split_off(keep)));
}

/// `fields` as an x-www-form-urlencoded body.
fn form(fields: &[(&str, &str)]) -> String {
    fields.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&")
}

/// POST to Spotify's token endpoint; its error in words when it refuses.
async fn token_request(st: &impl Ctx, fields: &[(&str, &str)]) -> Result<Value, Failure> {
    let r = st.http().post(TOKEN).header("Content-Type", "application/x-www-form-urlencoded").header("Accept", "application/json")
        .body(form(fields)).timeout(Duration::from_secs(20)).send().await
        .map_err(|e| Failure::new(Code::Network, format!("accounts.spotify.com can't be reached: {e}")))?;
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if status.is_success() && body["access_token"].is_string() {
        return Ok(body);
    }
    if status.is_client_error() {
        return Err(auth("Spotify did not accept the sign-in (a link works once and expires after a few minutes): press Test for a new one"));
    }
    Err(Failure::new(Code::Network, format!("accounts.spotify.com answered {status}")))
}

/// Exchange a pasted link's code for the step that waits for it.
async fn finish_sign_in(st: &impl Ctx, code: &str) -> Result<&'static str, Failure> {
    let pending = pytool::read_json(&file(st, PENDING)).ok_or_else(|| auth("No sign-in is waiting for that link: press Test for a new one"))?;
    let client = pending["client_id"].as_str().unwrap_or("").to_string();
    let verifier = pending["verifier"].as_str().unwrap_or("").to_string();
    let tok = token_request(st, &[("grant_type", "authorization_code"), ("client_id", &client), ("redirect_uri", REDIRECT), ("code", code), ("code_verifier", &verifier)]).await?;
    mark_used(st, code);
    let _ = std::fs::remove_file(file(st, PENDING));
    if pending["stage"] == "account" {
        let tmp = file(st, ".token");
        pytool::write_private(&tmp, tok["access_token"].as_str().unwrap_or("")).map_err(|e| Failure::internal(e.to_string()))?;
        let acct = session(st, &["login", &tmp.to_string_lossy(), &file(st, CREDS).to_string_lossy()]).await;
        let _ = std::fs::remove_file(&tmp);
        save_account(st, &acct?);
        return Ok("account");
    }
    save_api_token(st, &client, &tok, None);
    Ok("app")
}

/// Keep the account's name and plan for the card.
fn save_account(st: &impl Ctx, acct: &Value) {
    let _ = pytool::write_json(&file(st, ACCOUNT), &json!({ "username": acct["username"], "product": acct["product"], "checked": encore_db::now() }));
}

/// zotify's own format for the app's token (it refreshes it itself while it runs).
fn save_api_token(st: &impl Ctx, client: &str, tok: &Value, old: Option<&Value>) -> Value {
    let refresh = tok["refresh_token"].as_str().or(old.and_then(|o| o["refresh_token"].as_str())).unwrap_or("");
    let creds = json!({ "client_id": client, "access_token": tok["access_token"], "expires_at": encore_db::now() + tok["expires_in"].as_f64().unwrap_or(3600.0),
        "refresh_token": refresh, "type": "OAUTH_PKCE_TOKEN" });
    let _ = pytool::write_json(&file(st, API_CREDS), &creds);
    creds
}

/// librespot's sign-in, through the helper in the venv: a token in, zotify's credentials out (`login`), or the
/// saved credentials checked (`check`). → {username, product}
async fn session(st: &impl Ctx, args: &[&str]) -> Result<Value, Failure> {
    let py = pytool::python(st, ID).ok_or_else(|| pytool::not_installed("zotify"))?;
    let helper = file(st, "zotify_session.py");
    pytool::write_private(&helper, HELPER).map_err(|e| Failure::internal(e.to_string()))?;
    let mut all = vec![helper.to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let spawn = encore_plugins::sandbox::Spawn::new(&encore_plugins::sandbox::Sandbox::limits_only(), &py)
        .args(all).env(encore_plugins::sandbox::program_env()).cwd(&pytool::data(st, ID));
    let mut cmd = spawn.tokio_command();
    cmd.stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(Duration::from_secs(90), cmd.output()).await
        .map_err(|_| Failure::new(Code::Network, "Spotify's access point did not answer in 90 s"))?
        .map_err(|e| Failure::internal(format!("could not start zotify's Python: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let answer: Value = stdout.lines().rev().find(|l| l.starts_with('{')).and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| json!({ "ok": false, "error": String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("no answer").to_string() }));
    if answer["ok"].as_bool() == Some(true) {
        return Ok(answer);
    }
    let err = answer["error"].as_str().unwrap_or("no answer").to_string();
    let low = err.to_lowercase();
    let code = if ["connection", "timed out", "resolve"].iter().any(|w| low.contains(w)) { Code::Network } else { Code::Auth };
    Err(Failure::new(code, format!("Spotify sign-in failed: {err}")))
}

/// The developer app's token, refreshed when it is about to run out.
async fn api_token(st: &impl Ctx) -> Result<String, Failure> {
    let client = client_id(st);
    if client.is_empty() {
        return Err(auth("Search needs a Spotify developer app: add its Client ID on the zotify card, then Test"));
    }
    let creds = pytool::read_json(&file(st, API_CREDS)).filter(|c| c["client_id"] == client.as_str())
        .ok_or_else(|| auth("The developer app is not signed in yet: press Test on the zotify card"))?;
    if creds["expires_at"].as_f64().unwrap_or(0.0) - 60.0 > encore_db::now() {
        return Ok(creds["access_token"].as_str().unwrap_or("").to_string());
    }
    let refresh = creds["refresh_token"].as_str().filter(|r| !r.is_empty())
        .ok_or_else(|| auth("The developer app's sign-in ran out: press Test to sign in again"))?.to_string();
    let tok = token_request(st, &[("grant_type", "refresh_token"), ("client_id", &client), ("refresh_token", &refresh)]).await
        .map_err(|f| if f.code == Code::Auth { auth("Spotify no longer accepts the developer app's sign-in: press Test to sign in again") } else { f })?;
    Ok(save_api_token(st, &client, &tok, Some(&creds))["access_token"].as_str().unwrap_or("").to_string())
}

/// GET `endpoint` on the Web API with the app's token and the language setting.
async fn spotify(st: &impl Ctx, endpoint: &str, query: &[(&str, String)]) -> Result<Value, Failure> {
    let token = api_token(st).await?;
    let lang = Some(st.cfg().str("zotify_language")).filter(|l| !l.trim().is_empty()).unwrap_or_else(|| "en".into());
    let r = st.http().get(format!("{API}{endpoint}")).query(query).bearer_auth(token).header("Accept-Language", lang)
        .timeout(Duration::from_secs(15)).send().await.map_err(|e| Failure::new(Code::Network, format!("api.spotify.com can't be reached: {e}")))?;
    let status = r.status();
    match status.as_u16() {
        200..=299 => r.json().await.map_err(|e| Failure::new(Code::Network, format!("api.spotify.com: {e}"))),
        401 | 403 => Err(auth("Spotify refused the developer app's token: press Test on the zotify card")),
        429 => {
            let wait = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(30.0);
            Err(Failure { code: Code::RateLimited, message: "Spotify is rate-limiting the developer app".into(), retry_after: Some(wait) })
        }
        _ => Err(Failure::new(Code::Network, format!("api.spotify.com answered {status}"))),
    }
}

// ---- search ----------------------------------------------------------------------------------------------

/// Field filters break on quotes and colons; plain words match best.
fn clean(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '"' | '\'' | ':') { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The searches a request makes: (type, q). An ISRC first when the request has one.
pub fn queries(req: &Value) -> Vec<(&'static str, String)> {
    let s = |k: &str| clean(req[k].as_str().unwrap_or(""));
    let (artist, title, album, typed) = (s("artist"), s("title"), s("album"), s("query"));
    let isrc: String = req["isrc"].as_str().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    let part = |k: &str, v: &str| if v.is_empty() { String::new() } else { format!("{k}:{v}") };
    let join = |parts: Vec<String>| parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
    if req["kind"] == "album" {
        if !typed.is_empty() {
            return vec![("album", typed)];
        }
        let name = if album.is_empty() { &title } else { &album };
        let q = join(vec![part("album", name), part("artist", &artist)]);
        return if q.is_empty() { vec![] } else { vec![("album", q)] };
    }
    let mut out = vec![];
    if !isrc.is_empty() {
        out.push(("track", format!("isrc:{isrc}")));
    }
    if !typed.is_empty() {
        out.push(("track", typed));
    } else {
        let q = join(vec![part("track", &title), part("artist", &artist), if title.is_empty() { part("album", &album) } else { String::new() }]);
        if !q.is_empty() {
            out.push(("track", q));
        }
    }
    out
}

/// The format and bitrate zotify writes: Ogg Vorbis as streamed, or what it transcodes to.
fn quality_of(st: &impl Ctx) -> (&'static str, i64) {
    let premium = pytool::read_json(&file(st, ACCOUNT)).is_some_and(|a| a["product"].as_str().unwrap_or("").eq_ignore_ascii_case("premium"));
    let rate = match st.cfg().str("zotify_quality").as_str() {
        "very_high" => 320,
        "high" => 160,
        "normal" => 96,
        _ if premium => 320,
        _ => 160,
    };
    let rate = if premium { rate } else { rate.min(160) };
    let fmt = match st.cfg().str("zotify_format").as_str() {
        "mp3" => "mp3",
        "aac" => "m4a",
        "opus" => "opus",
        _ => "ogg",
    };
    (fmt, rate)
}

/// The quality shown for a format: "AAC 160k", "OGG 320k".
fn label(fmt: &str, rate: i64) -> String {
    let name = if fmt == "m4a" { "AAC" } else { fmt };
    format!("{} {rate}k", name.to_uppercase())
}

/// The artists' names, comma separated.
fn names(artists: &Value) -> String {
    artists.as_array().map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

/// The largest image's URL, or null.
fn cover(images: &Value) -> Value {
    images.as_array().and_then(|a| a.iter().max_by_key(|i| i["width"].as_i64().unwrap_or(0))).map(|i| i["url"].clone()).unwrap_or(Value::Null)
}

/// How much of `needle`'s words `hay` holds (as torznab scores); 1 when there's nothing to match.
fn share(hay: &str, needle: &str) -> f64 {
    if needle.trim().is_empty() { 1.0 } else { super::torznab::share(hay, needle) }
}

/// A Web API track as a search result; `None` when it can't be played.
pub fn track_result(req: &Value, t: &Value, fmt: &str, rate: i64, by_isrc: bool) -> Option<Value> {
    let id = t["id"].as_str()?;
    if t["is_playable"] == false {
        return None;
    }
    let (title, artist, album) = (t["name"].as_str().unwrap_or(""), names(&t["artists"]), t["album"]["name"].as_str().unwrap_or(""));
    let dur = t["duration_ms"].as_f64().map(|ms| ms / 1000.0).filter(|d| *d > 0.0);
    let isrc = t["external_ids"]["isrc"].as_str();
    let rel = share(title, req["title"].as_str().unwrap_or(""));
    if rel < 0.5 && !by_isrc {
        return None;
    }
    let want_artist = req["artist"].as_str().unwrap_or("");
    let mut score = rel * 50.0 + if want_artist.is_empty() { 10.0 } else { share(&artist, want_artist) * 20.0 } + if rate >= 320 { 10.0 } else { 5.0 };
    if let (Some(want), Some(d)) = (req["length"].as_f64(), dur) {
        score += if (d - want).abs() < 3.0 { 10.0 } else { -10.0 };
    }
    if by_isrc || isrc.zip(req["isrc"].as_str()).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)) {
        score += 20.0;
    }
    let info = [t["album"]["release_date"].as_str().unwrap_or("").chars().take(4).collect::<String>(), if t["explicit"] == true { "explicit".into() } else { String::new() }]
        .into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    let url = format!("{OPEN}track/{id}");
    let subtitle = [artist.as_str(), album].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
    Some(json!({
        "key": format!("spotify:track:{id}"), "title": title, "artist": artist, "album": album, "subtitle": subtitle,
        "format": fmt, "bitrate": rate, "bit_depth": 0, "sample_rate": 0, "quality": label(fmt, rate), "lossless": false, "tracks": 1,
        "size": dur.map(|d| (rate as f64 * 1000.0 / 8.0 * d) as i64), "duration": dur, "isrc": isrc, "cover": cover(&t["album"]["images"]), "info": info,
        "score": (score.clamp(0.0, 100.0) * 10.0).round() / 10.0, "url": t["external_urls"]["spotify"].as_str().unwrap_or(&url),
        "payload": { "url": url, "kind": "track", "tracks": 1, "title": title, "artist": artist, "album": album },
    }))
}

/// A Web API album as a search result.
pub fn album_result(req: &Value, a: &Value, fmt: &str, rate: i64) -> Option<Value> {
    let id = a["id"].as_str()?;
    let (title, artist) = (a["name"].as_str().unwrap_or(""), names(&a["artists"]));
    let want = req["album"].as_str().filter(|s| !s.is_empty()).or(req["title"].as_str()).unwrap_or("");
    let rel = share(title, want);
    if rel < 0.5 {
        return None;
    }
    // a live album ("Rodeo (Live)") of an album not asked for live: it shares every word of the title, so it
    // would pass as the album itself
    if encore_library::matcher::live_take(want, req["artist"].as_str().unwrap_or(""), title) {
        return None;
    }
    let n = a["total_tracks"].as_i64();
    let want_artist = req["artist"].as_str().unwrap_or("");
    let mut score = rel * 50.0 + if want_artist.is_empty() { 10.0 } else { share(&artist, want_artist) * 20.0 } + if rate >= 320 { 10.0 } else { 5.0 };
    if let (Some(want), Some(n)) = (req["track_count"].as_i64(), n) {
        score += if n == want { 10.0 } else if n < want { -10.0 } else { 0.0 };
    }
    let kind = a["album_type"].as_str().filter(|k| *k != "album").unwrap_or("");
    let info = [a["release_date"].as_str().unwrap_or("").chars().take(4).collect::<String>(), kind.to_string(), n.map(|n| format!("{n} tracks")).unwrap_or_default()]
        .into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    let url = format!("{OPEN}album/{id}");
    Some(json!({
        "key": format!("spotify:album:{id}"), "title": title, "artist": artist, "album": title, "subtitle": artist,
        "format": fmt, "bitrate": rate, "bit_depth": 0, "sample_rate": 0, "quality": label(fmt, rate), "lossless": false, "tracks": n,
        "size": null, "duration": null, "isrc": null, "cover": cover(&a["images"]), "info": info,
        "score": (score.clamp(0.0, 100.0) * 10.0).round() / 10.0, "url": a["external_urls"]["spotify"].as_str().unwrap_or(&url),
        "payload": { "url": url, "kind": "album", "tracks": n.unwrap_or(0), "title": title, "artist": artist, "album": title },
    }))
}

/// A request's search (the SDK's request fields) → results, best first.
pub async fn search(st: &impl Ctx, req: &Value) -> Result<Vec<Value>, Failure> {
    if req["lossless_only"] == true {
        return Ok(vec![]); // Spotify streams are lossy
    }
    let (fmt, rate) = quality_of(st);
    let mut out: Vec<Value> = vec![];
    for (typ, q) in queries(req) {
        let data = spotify(st, "search", &[("q", q.clone()), ("type", typ.to_string()), ("limit", SEARCH_LIMIT.to_string()), ("market", "from_token".into())]).await?;
        for it in data[format!("{typ}s")]["items"].as_array().cloned().unwrap_or_default() {
            let r = if typ == "track" { track_result(req, &it, fmt, rate, q.starts_with("isrc:")) } else { album_result(req, &it, fmt, rate) };
            let Some(r) = r else { continue };
            match out.iter_mut().find(|x| x["key"] == r["key"]) {
                Some(x) if x["score"].as_f64() < r["score"].as_f64() => *x = r,
                Some(_) => {}
                None => out.push(r),
            }
        }
    }
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(req["limit"].as_u64().unwrap_or(25) as usize);
    Ok(out)
}

// ---- download --------------------------------------------------------------------------------------------

/// zotify's command line: its saved credentials, `folder` as the root, the output layout and the settings.
fn args(st: &impl Ctx, folder: &Path) -> Vec<String> {
    let cfg = |k: &str, d: &str| Some(st.cfg().str(k)).filter(|v| !v.trim().is_empty()).unwrap_or_else(|| d.to_string());
    let p = |n: &str| file(st, n).to_string_lossy().into_owned();
    let conf = pytool::data(st, ID).join("zotify");
    let _ = std::fs::create_dir_all(&conf);
    let mut a: Vec<String> = [
        "--config-location", &conf.to_string_lossy(), "--credentials-location", &p(CREDS), "--api-credentials-location", &p(API_CREDS),
        "--save-credentials", "True", "--root-path", &folder.to_string_lossy(), "--root-podcast-path", &folder.join("Podcasts").to_string_lossy(),
        "--output-single", "{artist}/{album}/{track_number} {song_name}", "--output-album", "{album_artist}/{album}/{disc_number}-{track_number} {song_name}",
        "--codec", &cfg("zotify_format", "copy"), "--download-quality", &cfg("zotify_quality", "auto"),
        "--lyrics-to-file", if st.cfg().bool("zotify_lyrics_file") { "True" } else { "False" }, "--lyrics-to-metadata", "True",
        "--bulk-wait-time", &st.cfg().float("zotify_bulk_wait").max(0.0).to_string(), "--download-rate-limiter", &st.cfg().float("zotify_real_time").max(0.0).to_string(),
        "--song-archive-location", &pytool::data(st, ID).to_string_lossy(), "--disable-directory-archives", "True", "--skip-previously-downloaded", "False",
        "--language", &cfg("zotify_language", "en"), "--retry-attempts", "3",
        // never wait for a sign-in in a browser: the saved credentials or nothing
        "--redirect-timeout", "5",
        "--print-splash", "False", "--print-url-progress", "False", "--print-album-progress", "False", "--print-playlist-progress", "False", "--print-artist-progress", "False",
    ].iter().map(|s| s.to_string()).collect();
    let client = client_id(st);
    if !client.is_empty() {
        a.extend(["--client-id".to_string(), client]);
    }
    a
}

/// A tqdm progress bar redraw: its percent.
fn bar_percent(line: &str) -> Option<f64> {
    let i = line.find("%|")?;
    let digits: String = line[..i].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<String>().chars().rev().collect();
    digits.parse().ok()
}

/// Download a result's Spotify link with zotify into `folder`.
pub async fn download(st: &impl Ctx, call: &Call, req: &Value, folder: &Path) -> Result<Downloaded, Failure> {
    let payload = &req["payload"];
    let url = payload["url"].as_str().or(req["url"].as_str()).filter(|u| u.starts_with(OPEN)).ok_or_else(|| Failure::new(Code::NotFound, "Nothing to download: the result has no Spotify link"))?.to_string();
    if !signed_in(st) {
        return Err(auth("Spotify is not signed in: press Test on the zotify card"));
    }
    let program = pytool::program(st, ID).ok_or_else(|| pytool::not_installed("zotify"))?;
    let codec = st.cfg().str("zotify_format");
    if !matches!(codec.as_str(), "" | "copy" | "ogg" | "vorbis") && encore_core::which::which("ffmpeg").is_none() {
        return Err(Failure::unsupported("Converting needs ffmpeg on the server's PATH"));
    }
    encore_core::fsx::mkdir_all(folder, encore_core::fsx::Step::Stage).map_err(|e| Failure::internal(e.to_string()))?;
    let expected = payload["tracks"].as_u64().unwrap_or(1).max(1) as usize;
    let hook = move |line: &str| match bar_percent(line) {
        // per-track bars drive the bar only for a single song, and never go to the log
        Some(p) => ((expected == 1).then_some(p), false),
        None => (None, true),
    };
    let mut a = args(st, folder);
    a.push(url.clone());
    let secrets: Vec<String> = pytool::read_json(&file(st, API_CREDS)).and_then(|c| c["access_token"].as_str().map(String::from)).into_iter().collect();
    let run = pytool::run(st, Spec { id: ID, program, args: a, folder, expected, hook: Some(&hook), secrets, call }).await?;
    if run.files.is_empty() {
        let text = run.tail.iter().rev().take(6).cloned().collect::<Vec<_>>().join(" ").to_uppercase();
        if !run.success && (text.contains("LOGIN") || text.contains("NO SESSION")) {
            return Err(auth("zotify could not sign in with the saved credentials: press Test on the zotify card"));
        }
        if text.contains("AUDIO KEY") || text.contains("RATE LIMIT") || text.contains("429") {
            return Err(Failure { code: Code::RateLimited, message: "Spotify is rate-limiting this account: raise \"Wait between tracks\" and try again later".into(), retry_after: Some(600.0) });
        }
        let why = pytool::last_message(&run.tail);
        return Err(if run.success { Failure::new(Code::NotFound, format!("zotify finished without a file: {why}")) } else { Failure::internal(format!("zotify stopped (exit code {}): {why}", run.code.unwrap_or(-1))) });
    }
    let single = expected == 1 && run.files.len() == 1;
    let s = |k: &str| payload[k].as_str().map(String::from);
    let files = run.files.iter().map(|p| FileEntry {
        path: p.clone(), size: std::fs::metadata(p).ok().map(|m| m.len()), title: if single { s("title") } else { None },
        artist: if single { s("artist") } else { None }, album: s("album"), track: None, disc: None, duration: None, format: None, isrc: None,
    }).collect::<Vec<_>>();
    let missing = expected.saturating_sub(files.len());
    let n = files.len();
    let message = format!("Downloaded {n} file{} with zotify{}", if n == 1 { "" } else { "s" }, if expected > 1 && missing > 0 { format!(" ({missing} skipped, see the log)") } else { String::new() });
    Ok(Downloaded { files, source_url: Some(url), message: Some(message), cover_url: None })
}

// ---- status and test -------------------------------------------------------------------------------------

/// The zotify version installed.
pub async fn version(st: &impl Ctx) -> Option<String> {
    st.plugins().program(ID).map(|(v, _)| v)
}

/// The card's status line: the zotify version and the account and its plan.
pub async fn status(st: &impl Ctx) -> Result<Status, Failure> {
    let Some(v) = version(st).await.filter(|_| pytool::program(st, ID).is_some()) else {
        return Err(pytool::not_installed("zotify"));
    };
    let acct = pytool::read_json(&file(st, ACCOUNT)).unwrap_or(Value::Null);
    if !signed_in(st) || acct.is_null() {
        return Ok(Status { message: format!("zotify {v} · not signed in · press Test"), signed_in: Some(false) });
    }
    let product = acct["product"].as_str().unwrap_or("?");
    let who = format!("signed in as {} ({}{})", acct["username"].as_str().unwrap_or("?"), product.get(..1).unwrap_or("").to_uppercase(), product.get(1..).unwrap_or(""));
    let app = if search_ready(st) { "search ready" } else { "search needs the developer app: press Test" };
    Ok(Status { message: format!("zotify {v} · {who} · {app}"), signed_in: Some(search_ready(st)) })
}

/// The card's test: finish a pasted sign-in link, else check the saved sign-in (or give the link to sign in with).
pub async fn test(st: &impl Ctx) -> Result<String, Failure> {
    let v = version(st).await.filter(|_| pytool::program(st, ID).is_some()).ok_or_else(|| pytool::not_installed("zotify"))?;
    if let Some(code) = code_from(&st.cfg().str("zotify_sign_in_link"))? {
        if !used(st, &code) {
            finish_sign_in(st, &code).await?;
        }
    }
    if !signed_in(st) {
        let link = start_sign_in(st, "account", KEYMASTER);
        return Err(auth(format!(
            "Step 1 of 2, sign in to Spotify: open {link} , sign in, then paste the address your browser ends on \
             (http://127.0.0.1:4381/login?code=…, the page itself does not load) into \"Sign-in link\" and press Test again."
        )));
    }
    let acct = session(st, &["check", &file(st, CREDS).to_string_lossy()]).await?;
    save_account(st, &acct);
    let product = acct["product"].as_str().unwrap_or("unknown");
    let who = format!("Signed in as {} ({}{})", acct["username"].as_str().unwrap_or("?"), product.get(..1).unwrap_or("").to_uppercase(), product.get(1..).unwrap_or(""));
    let client = client_id(st);
    if client.is_empty() {
        return Ok(format!("{who} · zotify {v} · downloads work; search needs a Spotify developer app Client ID"));
    }
    if !search_ready(st) {
        let link = start_sign_in(st, "app", &client);
        return Err(auth(format!(
            "{who}. Step 2 of 2, sign in your developer app: open {link} , allow it, then paste the address your browser ends on \
             into \"Sign-in link\" and press Test again. The app's Redirect URI must be {REDIRECT}."
        )));
    }
    let me = spotify(st, "me", &[]).await?;
    let name = me["display_name"].as_str().or(me["id"].as_str()).unwrap_or("?");
    Ok(format!("{who} · zotify {v} · search through your developer app as {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_link_gives_its_code() {
        assert_eq!(code_from("http://127.0.0.1:4381/login?code=AQBabc_def-123456789012").unwrap(), Some("AQBabc_def-123456789012".into()));
        assert_eq!(code_from("  AQBabc_def-123456789012  ").unwrap(), Some("AQBabc_def-123456789012".into()));
        assert_eq!(code_from("").unwrap(), None);
        assert_eq!(code_from("short").unwrap(), None);
        assert_eq!(code_from("http://127.0.0.1:4381/login?error=access_denied").unwrap_err().code, Code::Auth);
    }

    #[test]
    fn pkce_challenge_is_the_verifier_hashed() {
        let (v, c) = pkce_pair();
        assert_eq!(v.len(), 96);
        let digest = hex::decode(encore_core::ids::sha256_hex(&v)).unwrap();
        assert_eq!(c, base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest));
        assert!(!c.contains('=') && !c.contains('+') && !c.contains('/'));
    }

    #[test]
    fn requests_become_spotify_queries() {
        let q = queries(&json!({ "kind": "track", "artist": "Radiohead", "title": "Weird Fishes: Arpeggi", "isrc": "GB-STK-07-00012" }));
        assert_eq!(q, [("track", "isrc:GBSTK0700012".to_string()), ("track", "track:Weird Fishes Arpeggi artist:Radiohead".to_string())]);
        assert_eq!(queries(&json!({ "kind": "album", "artist": "Radiohead", "album": "In Rainbows" })), [("album", "album:In Rainbows artist:Radiohead".to_string())]);
        assert_eq!(queries(&json!({ "kind": "track", "query": "typed words" })), [("track", "typed words".to_string())]);
        assert!(queries(&json!({ "kind": "album" })).is_empty());
    }

    #[test]
    fn results_have_the_sdk_shape() {
        let req = json!({ "kind": "track", "artist": "Radiohead", "title": "Weird Fishes", "isrc": "GBSTK0700012", "length": 318 });
        let t = json!({ "id": "4wajJ1o7jWIg62YqpkHC7S", "name": "Weird Fishes/Arpeggi", "artists": [{ "name": "Radiohead" }], "duration_ms": 318000,
            "album": { "name": "In Rainbows", "release_date": "2007-12-28", "images": [{ "url": "s", "width": 64 }, { "url": "L", "width": 640 }] },
            "external_ids": { "isrc": "GBSTK0700012" } });
        let r = track_result(&req, &t, "ogg", 320, false).unwrap();
        assert_eq!((r["quality"].as_str(), r["cover"].as_str(), r["score"].as_f64()), (Some("OGG 320k"), Some("L"), Some(100.0)));
        assert_eq!(r["payload"]["url"], "https://open.spotify.com/track/4wajJ1o7jWIg62YqpkHC7S");
        assert_eq!(r["size"], 12720000);
        let other = json!({ "id": "x", "name": "Another Song", "artists": [{ "name": "Radiohead" }] });
        assert!(track_result(&req, &other, "ogg", 320, false).is_none(), "a different title is left out");
        let a = json!({ "id": "5vkq", "name": "In Rainbows", "artists": [{ "name": "Radiohead" }], "total_tracks": 10, "album_type": "album", "release_date": "2007-12-28" });
        let r = album_result(&json!({ "kind": "album", "artist": "Radiohead", "album": "In Rainbows", "track_count": 10 }), &a, "m4a", 160).unwrap();
        assert_eq!((r["quality"].as_str(), r["tracks"].as_i64(), r["info"].as_str()), (Some("AAC 160k"), Some(10), Some("2007 · 10 tracks")));
    }

    #[test]
    fn progress_bars_are_read() {
        assert_eq!(bar_percent(" 'Song' 45%|####      | 1.2M/2.6M"), Some(45.0));
        assert_eq!(bar_percent("###   DOWNLOADED   ###"), None);
    }
}
