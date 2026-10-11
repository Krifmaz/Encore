//! Per-user account connections (streaming services + media servers) and their credentials.
//! Stored data has the same shape as Encore 1, so connections keep working when switching servers.
use crate::db::{now, Row, RowExt};
use crate::p;
use crate::state::AppState;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// service → (label, kind). media = can take likes/playlists, stream = import source, scrobble = ListenBrainz/Last.fm
pub const SERVICES: [(&str, &str, &str); 9] = [
    ("jellyfin", "Jellyfin / Finamp", "media"), ("plex", "Plex / Plexamp", "media"), ("navidrome", "Navidrome / Subsonic", "media"),
    ("ytmusic", "YouTube Music", "stream"), ("spotify", "Spotify", "stream"), ("tidal", "Tidal", "stream"), ("deezer", "Deezer", "stream"),
    ("listenbrainz", "ListenBrainz", "scrobble"), ("lastfm", "Last.fm", "scrobble"),
];

pub fn get(st: &AppState, uid: i64, service: &str) -> Option<Row> {
    let mut c = st.db.one("SELECT * FROM connections WHERE user_id=? AND service=?", p![uid, service]).ok()??;
    let d: Value = serde_json::from_str(&c.s("data")).unwrap_or(json!({}));
    c.insert("data".into(), d);
    Some(c)
}

pub fn list_for(st: &AppState, uid: i64) -> Vec<Row> {
    st.db.query("SELECT * FROM connections WHERE user_id=? ORDER BY service", p![uid]).unwrap_or_default().into_iter().map(|mut c| {
        let d: Value = serde_json::from_str(&c.s("data")).unwrap_or(json!({}));
        c.insert("data".into(), d);
        c
    }).collect()
}

pub fn save(st: &AppState, uid: i64, service: &str, account: &str, data: &Value) -> anyhow::Result<i64> {
    if let Some(c) = get(st, uid, service) {
        st.db.execute("UPDATE connections SET account=?, data=?, last_error=NULL WHERE id=?", p![account, data.to_string(), c.i("id")])?;
        return Ok(c.i("id"));
    }
    Ok(st.db.execute("INSERT INTO connections (user_id, service, account, data, created_at) VALUES (?,?,?,?,?)", p![uid, service, account, data.to_string(), now()])?)
}

pub fn update_data(st: &AppState, id: i64, data: &Value) {
    let _ = st.db.execute("UPDATE connections SET data=? WHERE id=?", p![data.to_string(), id]);
}

pub fn public(c: &Row) -> Value {
    let svc = c.s("service");
    let (label, kind) = SERVICES.iter().find(|x| x.0 == svc).map(|x| (x.1, x.2)).unwrap_or((svc.as_str(), ""));
    let mut v = json!({ "label": label, "kind": kind });
    for k in ["id", "service", "account", "sync_likes", "sync_playlists", "last_sync", "last_error", "created_at"] {
        v[k] = c.get(k).cloned().unwrap_or(Value::Null);
    }
    v
}

fn redirect_uri(st: &AppState, service: &str) -> anyhow::Result<String> {
    let base = st.cfg.str("base_url").trim_end_matches('/').to_string();
    if base.is_empty() {
        anyhow::bail!("Set the Public URL in Settings → General first (sign-in needs a callback address)");
    }
    Ok(format!("{base}/api/connections/{service}/callback"))
}

// ---- OAuth (Spotify / Tidal, PKCE) ----------------------------------------------------------------

pub fn oauth_start(st: &AppState, uid: i64, service: &str) -> anyhow::Result<String> {
    let verifier: String = crate::auth::random_token(64).chars().take(96).collect();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = crate::auth::random_token(24);
    st.cache_put(&format!("oauth:{state}"), json!({ "user_id": uid, "service": service, "verifier": verifier }), 900);
    let (base, cid, scope) = match service {
        "spotify" => ("https://accounts.spotify.com/authorize", st.cfg.str("spotify_client_id"), "playlist-read-private playlist-read-collaborative user-library-read"),
        "tidal" => ("https://login.tidal.com/authorize", st.cfg.str("tidal_client_id"), "user.read collection.read playlists.read"),
        _ => anyhow::bail!("Unsupported sign-in service"),
    };
    if cid.is_empty() {
        anyhow::bail!("The admin needs to add a {} Client ID in Settings → Integrations", if service == "spotify" { "Spotify" } else { "Tidal" });
    }
    let q = [("client_id", cid), ("response_type", "code".into()), ("redirect_uri", redirect_uri(st, service)?), ("code_challenge_method", "S256".into()),
             ("code_challenge", challenge), ("state", state), ("scope", scope.into())];
    Ok(format!("{base}?{}", q.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&")))
}

pub async fn oauth_finish(st: &AppState, service: &str, code: &str, state: &str, uid: i64) -> anyhow::Result<()> {
    let key = format!("oauth:{state}");
    let pend = st.cache_get(&key).filter(|p| p["service"] == service && p["user_id"].as_i64() == Some(uid))
        .ok_or_else(|| anyhow::anyhow!("Sign-in expired or was started in another session – try again"))?;
    st.cache_put(&key, Value::Null, 0);
    let (url, cid) = if service == "spotify" { ("https://accounts.spotify.com/api/token", st.cfg.str("spotify_client_id")) } else { ("https://auth.tidal.com/v1/oauth2/token", st.cfg.str("tidal_client_id")) };
    let tok: Value = st.http.post(url).form(&[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &redirect_uri(st, service)?),
        ("client_id", &cid), ("code_verifier", pend["verifier"].as_str().unwrap_or(""))]).send().await?.json().await?;
    let Some(access) = tok["access_token"].as_str() else {
        anyhow::bail!("{service}: {}", tok["error_description"].as_str().map(String::from).unwrap_or(tok.to_string()));
    };
    let expires = now() + tok["expires_in"].as_f64().unwrap_or(3600.0);
    if service == "spotify" {
        let me: Value = st.http.get("https://api.spotify.com/v1/me").bearer_auth(access).send().await?.json().await?;
        let name = me["display_name"].as_str().or(me["id"].as_str()).unwrap_or("Spotify");
        save(st, uid, "spotify", name, &json!({ "access_token": access, "refresh_token": tok["refresh_token"], "expires": expires, "user_id": me["id"] }))?;
    } else {
        let tuid = tok["user_id"].as_i64().map(|x| x.to_string()).or(tok["user"]["userId"].as_i64().map(|x| x.to_string())).unwrap_or_default();
        let name = tok["user"]["username"].as_str().map(String::from).unwrap_or(if tuid.is_empty() { "Tidal".into() } else { tuid.clone() });
        save(st, uid, "tidal", &name, &json!({ "access_token": access, "refresh_token": tok["refresh_token"], "expires": expires, "user_id": tuid,
            "country": tok["user"]["countryCode"].as_str().unwrap_or("US") }))?;
    }
    Ok(())
}

async fn refresh(st: &AppState, c: &Row, url: &str, client_id: &str) -> Option<String> {
    let mut d = c["data"].clone();
    if d["expires"].as_f64().unwrap_or(0.0) > now() + 60.0 {
        return d["access_token"].as_str().map(String::from);
    }
    let rt = d["refresh_token"].as_str()?.to_string();
    let tok: Value = st.http.post(url).form(&[("grant_type", "refresh_token"), ("refresh_token", &rt), ("client_id", client_id)]).send().await.ok()?.json().await.ok()?;
    let Some(access) = tok["access_token"].as_str() else {
        let _ = st.db.execute("UPDATE connections SET last_error=? WHERE id=?", p!["Login expired – reconnect", c.i("id")]);
        return None;
    };
    d["access_token"] = json!(access);
    d["expires"] = json!(now() + tok["expires_in"].as_f64().unwrap_or(3600.0));
    if tok["refresh_token"].is_string() {
        d["refresh_token"] = tok["refresh_token"].clone();
    }
    update_data(st, c.i("id"), &d);
    Some(access.to_string())
}

pub async fn spotify_token(st: &AppState, uid: i64) -> Option<String> {
    let c = get(st, uid, "spotify")?;
    refresh(st, &c, "https://accounts.spotify.com/api/token", &st.cfg.str("spotify_client_id")).await
}

pub async fn tidal_token(st: &AppState, uid: i64) -> Option<String> {
    let c = get(st, uid, "tidal")?;
    refresh(st, &c, "https://auth.tidal.com/v1/oauth2/token", &st.cfg.str("tidal_client_id")).await
}

// ---- Jellyfin -----------------------------------------------------------------------------------

pub fn jellyfin_headers(token: Option<&str>, device: &str) -> Vec<(String, String)> {
    let mut v = format!("MediaBrowser Client=\"Encore\", Device=\"Encore\", DeviceId=\"{device}\", Version=\"1.0\"");
    if let Some(t) = token {
        v += &format!(", Token=\"{t}\"");
    }
    let mut h = vec![("Authorization".into(), v.clone()), ("X-Emby-Authorization".into(), v)];
    if let Some(t) = token {
        h.push(("X-Emby-Token".into(), t.into()));
    }
    h
}

pub async fn connect_jellyfin(st: &AppState, uid: i64, user: &str, password: &str) -> anyhow::Result<Value> {
    let url = st.cfg.str("jellyfin_url").trim_end_matches('/').to_string();
    if url.is_empty() {
        anyhow::bail!("The admin hasn't set a Jellyfin URL in Settings → Media servers");
    }
    let mut rb = st.http.post(format!("{url}/Users/AuthenticateByName")).json(&json!({ "Username": user, "Pw": password }));
    for (k, v) in jellyfin_headers(None, &format!("encore-{uid}")) {
        rb = rb.header(k, v);
    }
    let r = rb.send().await?;
    if r.status().as_u16() != 200 {
        anyhow::bail!("Jellyfin rejected that username/password");
    }
    let d: Value = r.json().await?;
    let name = d["User"]["Name"].as_str().unwrap_or(user).to_string();
    save(st, uid, "jellyfin", &name, &json!({ "token": d["AccessToken"], "user_id": d["User"]["Id"] }))?;
    Ok(json!({ "account": name }))
}

// ---- Plex (PIN sign-in) --------------------------------------------------------------------------

pub fn plex_client_id(st: &AppState) -> String {
    let cid = st.cfg.str("plex_client_id");
    if !cid.is_empty() {
        return cid;
    }
    let b: [u8; 16] = rand::random();
    let h = hex::encode(b);
    let cid = format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]);
    st.cfg.set_many(&json!({ "plex_client_id": cid }));
    cid
}

pub fn plex_headers(st: &AppState, token: Option<&str>) -> Vec<(String, String)> {
    let mut h = vec![("X-Plex-Product".to_string(), "Encore".to_string()), ("X-Plex-Client-Identifier".into(), plex_client_id(st)),
                     ("X-Plex-Version".into(), "1.0".into()), ("Accept".into(), "application/json".into())];
    if let Some(t) = token {
        h.push(("X-Plex-Token".into(), t.into()));
    }
    h
}

pub fn with_headers(mut rb: reqwest::RequestBuilder, h: &[(String, String)]) -> reqwest::RequestBuilder {
    for (k, v) in h {
        rb = rb.header(k, v);
    }
    rb
}

pub async fn plex_pin_start(st: &AppState) -> anyhow::Result<Value> {
    let d: Value = with_headers(st.http.post("https://plex.tv/api/v2/pins").query(&[("strong", "true")]), &plex_headers(st, None)).send().await?.json().await?;
    let mut q = vec![("clientID", plex_client_id(st)), ("code", d["code"].as_str().unwrap_or("").to_string()), ("context[device][product]", "Encore".into())];
    let base = st.cfg.str("base_url");
    if !base.is_empty() {
        q.push(("forwardUrl", format!("{}/#/connections", base.trim_end_matches('/'))));
    }
    let qs = q.iter().map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v))).collect::<Vec<_>>().join("&");
    Ok(json!({ "pin_id": d["id"], "url": format!("https://app.plex.tv/auth#?{qs}") }))
}

pub async fn plex_pin_check(st: &AppState, uid: i64, pin: i64) -> anyhow::Result<Value> {
    let d: Value = with_headers(st.http.get(format!("https://plex.tv/api/v2/pins/{pin}")), &plex_headers(st, None)).send().await?.json().await?;
    let Some(tok) = d["authToken"].as_str().filter(|t| !t.is_empty()) else { return Ok(json!({ "done": false })) };
    let me: Value = with_headers(st.http.get("https://plex.tv/api/v2/user"), &plex_headers(st, Some(tok))).send().await?.json().await?;
    let mut server_token = tok.to_string();
    let (url, admin_tok) = (st.cfg.str("plex_url").trim_end_matches('/').to_string(), st.cfg.str("plex_token"));
    if !url.is_empty() && !admin_tok.is_empty() {
        let ident: anyhow::Result<Value> = async { Ok(with_headers(st.http.get(format!("{url}/identity")), &plex_headers(st, Some(&admin_tok))).send().await?.json().await?) }.await;
        if let Ok(ident) = ident {
            let mid = ident["MediaContainer"]["machineIdentifier"].as_str().unwrap_or("").to_string();
            let res: Value = with_headers(st.http.get("https://plex.tv/api/v2/resources").query(&[("includeHttps", "1")]), &plex_headers(st, Some(tok))).send().await?.json().await?;
            match res.as_array().and_then(|a| a.iter().find(|d| d["clientIdentifier"] == mid.as_str() && d["accessToken"].is_string())) {
                Some(dev) => server_token = dev["accessToken"].as_str().unwrap().to_string(),
                None => anyhow::bail!("Your Plex account doesn't have access to this server – ask the admin to share the library with you"),
            }
        }
    }
    let name = me["username"].as_str().or(me["title"].as_str()).unwrap_or("Plex").to_string();
    save(st, uid, "plex", &name, &json!({ "token": server_token, "account_token": tok }))?;
    Ok(json!({ "done": true, "account": name }))
}

// ---- Navidrome / Subsonic ------------------------------------------------------------------------

pub fn subsonic_params(user: &str, password: &str) -> Vec<(&'static str, String)> {
    let salt = hex::encode(rand::random::<[u8; 6]>());
    vec![("u", user.into()), ("t", format!("{:x}", md5::compute(format!("{password}{salt}")))), ("s", salt), ("v", "1.16.1".into()), ("c", "encore".into()), ("f", "json".into())]
}

pub async fn connect_navidrome(st: &AppState, uid: i64, user: &str, password: &str) -> anyhow::Result<Value> {
    let url = st.cfg.str("navidrome_url").trim_end_matches('/').to_string();
    if url.is_empty() {
        anyhow::bail!("The admin hasn't set a Navidrome URL in Settings → Media servers");
    }
    let d: Value = st.http.get(format!("{url}/rest/ping")).query(&subsonic_params(user, password)).send().await?.json().await?;
    if d["subsonic-response"]["status"] != "ok" {
        anyhow::bail!("Navidrome rejected that username/password");
    }
    save(st, uid, "navidrome", user, &json!({ "user": user, "password": password }))?;
    Ok(json!({ "account": user }))
}

// ---- ListenBrainz / Last.fm / Deezer -------------------------------------------------------------

pub async fn connect_listenbrainz(st: &AppState, uid: i64, token: &str) -> anyhow::Result<Value> {
    let d: Value = st.http.get("https://api.listenbrainz.org/1/validate-token").header("Authorization", format!("Token {token}")).send().await?.json().await?;
    if !d["valid"].as_bool().unwrap_or(false) {
        anyhow::bail!("ListenBrainz token is not valid");
    }
    let name = d["user_name"].as_str().unwrap_or("").to_string();
    save(st, uid, "listenbrainz", &name, &json!({ "token": token, "user": name }))?;
    Ok(json!({ "account": name }))
}

pub async fn connect_lastfm(st: &AppState, uid: i64, user: &str) -> anyhow::Result<Value> {
    let key = st.cfg.str("lastfm_api_key");
    if key.is_empty() {
        anyhow::bail!("The admin needs to add a Last.fm API key in Settings → Integrations");
    }
    let d: Value = st.http.get("https://ws.audioscrobbler.com/2.0/").query(&[("method", "user.getinfo"), ("user", user), ("api_key", &key), ("format", "json")]).send().await?.json().await?;
    let Some(name) = d["user"]["name"].as_str() else { anyhow::bail!("Last.fm user not found") };
    save(st, uid, "lastfm", name, &json!({ "user": name }))?;
    Ok(json!({ "account": name }))
}

pub async fn connect_deezer(st: &AppState, uid: i64, profile: &str) -> anyhow::Result<Value> {
    let id = regex::Regex::new(r"(\d{3,})").unwrap().captures(profile).map(|c| c[1].to_string())
        .ok_or_else(|| anyhow::anyhow!("Paste your Deezer profile link or user id (your favourites must be public)"))?;
    let d: Value = st.http.get(format!("https://api.deezer.com/user/{id}")).send().await?.json().await?;
    if !d["error"].is_null() {
        anyhow::bail!("Deezer profile not found");
    }
    let name = d["name"].as_str().unwrap_or(&id).to_string();
    save(st, uid, "deezer", &name, &json!({ "user_id": id }))?;
    Ok(json!({ "account": name }))
}

// ---- YouTube Music (Google device sign-in, or pasted browser headers) -----------------------------

pub async fn ytmusic_oauth_start(st: &AppState) -> anyhow::Result<Value> {
    let cid = st.cfg.str("ytmusic_client_id");
    if cid.is_empty() || st.cfg.str("ytmusic_client_secret").is_empty() {
        anyhow::bail!("The admin needs to add a YouTube Data API OAuth client (type “TVs and Limited Input devices”) in Settings → Integrations");
    }
    let d: Value = st.http.post("https://oauth2.googleapis.com/device/code").form(&[("client_id", cid.as_str()), ("scope", "https://www.googleapis.com/auth/youtube")]).send().await?.json().await?;
    Ok(json!({ "device_code": d["device_code"], "user_code": d["user_code"], "url": d["verification_url"].as_str().unwrap_or("https://www.google.com/device"),
               "interval": d["interval"].as_i64().unwrap_or(5) }))
}

pub async fn ytmusic_oauth_check(st: &AppState, uid: i64, device_code: &str) -> anyhow::Result<Value> {
    let d: Value = st.http.post("https://oauth2.googleapis.com/token").form(&[("client_id", st.cfg.str("ytmusic_client_id")), ("client_secret", st.cfg.str("ytmusic_client_secret")),
        ("device_code", device_code.into()), ("grant_type", "urn:ietf:params:oauth:grant-type:device_code".into())]).send().await?.json().await?;
    if d["access_token"].is_null() {
        let e = d["error"].as_str().unwrap_or("");
        if e == "authorization_pending" || e == "slow_down" {
            return Ok(json!({ "done": false }));
        }
        anyhow::bail!("Google sign-in failed: {}", d["error_description"].as_str().unwrap_or(e));
    }
    let mut token = d.clone();
    token["expires_at"] = json!(now() as i64 + d["expires_in"].as_i64().unwrap_or(3600));
    save(st, uid, "ytmusic", "YouTube Music", &json!({ "mode": "oauth", "token": token }))?;
    Ok(json!({ "done": true, "account": "YouTube Music" }))
}

/// Alternative to Google sign-in: the request headers of a music.youtube.com request (devtools → copy headers).
pub async fn ytmusic_connect_browser(st: &AppState, uid: i64, raw: &str) -> anyhow::Result<Value> {
    let mut headers = serde_json::Map::new();
    let lines: Vec<&str> = raw.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let mut i = 0;
    while i < lines.len() {
        // "Name: value" or Chrome's alternating "name" / "value" lines
        if let Some((k, v)) = lines[i].split_once(": ").filter(|(k, _)| !k.starts_with(':')) {
            headers.insert(k.to_lowercase(), json!(v));
        } else if lines[i].ends_with(':') && i + 1 < lines.len() {
            headers.insert(lines[i].trim_end_matches(':').to_lowercase(), json!(lines[i + 1]));
            i += 1;
        }
        i += 1;
    }
    let cookie = headers.get("cookie").and_then(|c| c.as_str()).unwrap_or("");
    if !cookie.contains("SAPISID") {
        anyhow::bail!("Couldn't find the YouTube Music cookies in those headers – copy them from a music.youtube.com request while signed in");
    }
    let keep: serde_json::Map<String, Value> = headers.into_iter().filter(|(k, _)| ["cookie", "x-goog-authuser", "user-agent"].contains(&k.as_str())).collect();
    let data = json!({ "mode": "browser", "headers": keep });
    crate::sources::ytmusic::authed(st, &data, "browse", json!({ "browseId": "FEmusic_liked_videos" })).await
        .map_err(|e| anyhow::anyhow!("YouTube Music rejected those headers (copy them again from a fresh request): {e}"))?;
    save(st, uid, "ytmusic", "YouTube Music", &data)?;
    Ok(json!({ "account": "YouTube Music" }))
}
