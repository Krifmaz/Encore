//! Users, sessions, roles and permissions.
//!
//! Roles: owner > admin > user. The owner is the person who runs the server: only they can promote or
//! demote admins, and nobody can change or remove the owner. Admins manage everything else.
use crate::db::{now, Row, RowExt};
use crate::error::{err, ApiError, ApiResult};
use crate::p;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, Method};
use axum_extra::extract::CookieJar;
use rand::Rng;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use subtle::ConstantTimeEq;

pub const COOKIE: &str = "encore_session";

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::rng().fill(&mut b[..]);
    hex::encode(b)
}

pub fn random_token(bytes: usize) -> String {
    use base64::Engine;
    let mut b = vec![0u8; bytes];
    rand::rng().fill(&mut b[..]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

pub fn sha256_hex(s: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(s.as_bytes()))
}

pub fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

// ---- passwords --------------------------------------------------------------------------------

pub fn hash_password(pw: &str) -> String {
    let b = &pw.as_bytes()[..pw.len().min(72)];
    bcrypt::hash(b, 12).unwrap_or_default()
}

pub fn verify_password(pw: &str, hashed: &str) -> bool {
    let b = &pw.as_bytes()[..pw.len().min(72)];
    bcrypt::verify(b, hashed).unwrap_or(false)
}

pub fn validate_username(username: &str) -> ApiResult<()> {
    let ok_name = (3..=32).contains(&username.chars().count())
        && username.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !ok_name {
        return Err(err(400, "Username must be 3-32 characters: letters, numbers, . _ -"));
    }
    Ok(())
}

pub fn validate_credentials(username: &str, password: &str) -> ApiResult<()> {
    validate_username(username)?;
    if password.chars().count() < 8 {
        return Err(err(400, "Password must be at least 8 characters"));
    }
    Ok(())
}

// ---- rate limits ------------------------------------------------------------------------------

#[derive(Default)]
pub struct RateLimiter(Mutex<HashMap<String, VecDeque<f64>>>);

impl RateLimiter {
    /// True when `key` already had `limit` events in the last `window` seconds. Records one unless `record` is false.
    pub fn limited(&self, key: &str, limit: usize, window: f64, record: bool) -> bool {
        let mut m = self.0.lock().unwrap();
        let q = m.entry(key.to_string()).or_default();
        let t = now();
        while q.front().is_some_and(|x| *x < t - window) {
            q.pop_front();
        }
        if q.len() >= limit {
            return true;
        }
        if record {
            q.push_back(t);
        }
        false
    }
    pub fn record(&self, key: &str) {
        self.0.lock().unwrap().entry(key.to_string()).or_default().push_back(now());
    }
}

// ---- roles & permissions ----------------------------------------------------------------------

pub fn is_admin(u: &Row) -> bool {
    matches!(u.s("role").as_str(), "admin" | "owner")
}
pub fn is_owner(u: &Row) -> bool {
    u.s("role") == "owner"
}

pub const PERMS: &[(&str, &str)] = &[
    ("auto_approve", "Requests skip the approval queue"),
    ("auto_approve_playlists", "Playlist downloads skip the approval queue"),
    ("pick", "Pick the exact version to download (their own requests)"),
    ("fix", "Fix their own downloads: retry, re-pick, delete a wrong song they requested"),
    ("storage", "See server storage"),
    ("server_dsp", "Use server-side Cadenza processing for their streams (costs server CPU)"),
    ("translate", "Correct translated titles (the fix shows for everyone reading that language)"),
];

pub fn perms_of(st: &AppState, u: &Row) -> Map<String, Value> {
    let saved: Map<String, Value> = serde_json::from_str(&u.s("perms")).unwrap_or_default();
    let mut out = Map::new();
    out.insert("auto_approve_playlists".into(), json!(false));
    out.insert("pick".into(), json!(st.cfg.str("manual_search") != "admins"));
    out.insert("fix".into(), json!(true));
    out.insert("storage".into(), json!(true));
    out.insert("server_dsp".into(), json!(false));
    out.insert("translate".into(), json!(true));
    for (k, v) in saved {
        if PERMS.iter().any(|(p, _)| *p == k) {
            out.insert(k, json!(v.as_bool().unwrap_or(false)));
        }
    }
    out.insert("auto_approve".into(), json!(u.b("auto_approve")));
    out
}

pub fn can(st: &AppState, u: &Row, perm: &str) -> bool {
    is_admin(u) || perms_of(st, u).get(perm).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// What the browser may see about a user.
pub fn public_user(st: &AppState, u: &Row) -> Value {
    let mut m = u.clone();
    for k in ["password_hash", "totp_secret"] {
        m.remove(k);
    }
    m.insert("totp_enabled".into(), json!(u.b("totp_enabled")));
    m.insert("perms".into(), Value::Object(perms_of(st, u)));
    m.insert("is_admin".into(), json!(is_admin(u)));
    m.insert("prefs".into(), serde_json::from_str(&u.s("prefs")).unwrap_or(json!({})));
    Value::Object(m)
}

// ---- users & sessions -------------------------------------------------------------------------

pub fn create_user(st: &AppState, username: &str, password: &str, role: &str, email: Option<&str>, invite_id: Option<i64>) -> ApiResult<i64> {
    validate_credentials(username, password)?;
    if st.db.one("SELECT id FROM users WHERE username=?", p![username])?.is_some() {
        return Err(err(409, "Username already taken"));
    }
    // what every new member starts with (Settings → Users & permissions → New members start with)
    let c = &st.cfg;
    let auto = if role != "user" || c.bool("new_user_auto_approve") { 1 } else { 0 };
    let perms = json!({
        "auto_approve_playlists": c.bool("new_user_auto_approve_playlists"),
        "pick": c.bool("new_user_pick"), "fix": c.bool("new_user_fix"), "storage": c.bool("new_user_storage"),
    });
    Ok(st.db.execute(
        "INSERT INTO users (username, email, password_hash, role, invite_id, auto_approve, auto_download, perms, created_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![username, email, hash_password(password), role, invite_id, auto, c.bool("new_user_auto_download") as i64, perms.to_string(), now()],
    )?)
}

pub fn create_session(st: &AppState, user_id: i64, ua: &str, ip: &str) -> ApiResult<(String, i64)> {
    let token = random_token(32);
    let max_age = st.cfg.int("session_days").max(1) * 86400;
    let ua: String = ua.chars().take(200).collect();
    st.db.execute(
        "INSERT INTO sessions (token, user_id, created_at, expires_at, user_agent, ip, kind) VALUES (?,?,?,?,?,?,'web')",
        p![token, user_id, now(), now() + max_age as f64, ua, ip],
    )?;
    st.db.execute("UPDATE users SET last_login=? WHERE id=?", p![now(), user_id])?;
    Ok((token, max_age))
}

// ---- app tokens (native app, `Authorization: Bearer`) -------------------------------------------

/// How an app token is stored: only its digest, so a copy of the database can't sign anyone in.
pub fn app_token_key(token: &str) -> String {
    format!("sha256:{}", sha256_hex(token))
}

/// App sessions live this long after their last use.
pub fn app_session_secs(st: &AppState) -> i64 {
    st.cfg.int("app_session_days").max(1) * 86400
}

/// `last_seen_at`/`expires_at` are refreshed at most this often (one write per device per day).
pub const APP_SEEN_EVERY: f64 = 86400.0;

/// Signs a device in to the app: returns the raw token (shown once). A new sign-in from the same
/// `device_id` replaces that device's previous app session.
pub fn create_app_session(st: &AppState, user_id: i64, ua: &str, ip: &str, device_id: &str, device_name: &str) -> ApiResult<String> {
    let device_id: String = device_id.trim().chars().take(100).collect();
    if device_id.is_empty() {
        return Err(err(400, "app.device_id is required"));
    }
    let device_name: String = device_name.trim().chars().take(100).collect();
    let token = random_token(32);
    let ua: String = ua.chars().take(200).collect();
    let t = now();
    st.db.execute("DELETE FROM sessions WHERE user_id=? AND kind='app' AND device_id=?", p![user_id, device_id])?;
    st.db.execute(
        "INSERT INTO sessions (token, user_id, created_at, expires_at, user_agent, ip, kind, device_id, device_name, last_seen_at) VALUES (?,?,?,?,?,?,'app',?,?,?)",
        p![app_token_key(&token), user_id, t, t + app_session_secs(st) as f64, ua, ip, device_id, if device_name.is_empty() { None } else { Some(device_name) }, t],
    )?;
    st.db.execute("UPDATE users SET last_login=? WHERE id=?", p![t, user_id])?;
    Ok(token)
}

/// The bearer token of the request, if it has one (`Authorization: Bearer <token>`).
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = v.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_string())
}

/// The `sessions.token` value of the session making the request (bearer digest, or the cookie).
pub fn session_key(headers: &HeaderMap) -> Option<String> {
    if let Some(t) = bearer(headers) {
        return Some(app_token_key(&t));
    }
    CookieJar::from_headers(headers).get(COOKIE).map(|c| c.value().to_string())
}

/// First start: make sure there is an owner (the first admin becomes the owner on upgrade).
pub fn ensure_owner(st: &AppState) -> anyhow::Result<()> {
    if st.db.one("SELECT id FROM users WHERE role='owner'", p![])?.is_some() {
        return Ok(());
    }
    if let Some(a) = st.db.one("SELECT id, username FROM users WHERE role='admin' ORDER BY created_at, id LIMIT 1", p![])? {
        st.db.execute("UPDATE users SET role='owner' WHERE id=?", p![a.i("id")])?;
        st.db.log("info", &format!("{} is now the server owner", a.s("username")));
        return Ok(());
    }
    let username = std::env::var("ENCORE_ADMIN_USER").unwrap_or_else(|_| "admin".into());
    let (password, generated) = match std::env::var("ENCORE_ADMIN_PASSWORD") {
        Ok(p) if !p.trim().is_empty() => (p, false),
        _ => (random_token(12), true),
    };
    create_user(st, &username, &password, "owner", None, None).map_err(|e| anyhow::anyhow!(e.detail))?;
    if generated {
        let f = st.db.data_dir.join("initial_admin_password.txt");
        std::fs::write(&f, format!("{username}\n{password}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))?;
        }
        println!("{}\nCreated owner account '{username}' with password: {password}\n(change it in Settings → Account)\n{}", "=".repeat(70), "=".repeat(70));
    }
    st.db.log("info", &format!("Created owner account '{username}'"));
    Ok(())
}

pub fn client_ip(parts: &Parts) -> String {
    if let Some(f) = parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        return f.split(',').next().unwrap_or("").trim().to_string();
    }
    parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_default()
}

/// How a request signed in. Only cookies need the CSRF header: a browser never adds the other two on its own.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Via {
    ApiKey,
    Bearer,
    Cookie,
}

fn user_from_parts(st: &AppState, parts: &Parts) -> ApiResult<Option<(Row, Via)>> {
    if let Some(key) = parts.headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let want = st.cfg.str("api_key");
        if !want.is_empty() && ct_eq(key, &want) {
            let u = st.db.one("SELECT * FROM users WHERE role IN ('owner','admin') AND disabled=0 ORDER BY role='owner' DESC, id LIMIT 1", p![])?;
            return Ok(u.map(|u| (u, Via::ApiKey)));
        }
    }
    // a bearer token is used on its own: a wrong or expired one is a 401, never a fall back to the cookie
    if let Some(token) = bearer(&parts.headers) {
        let key = app_token_key(&token);
        let Some(s) = st.db.one("SELECT * FROM sessions WHERE token=? AND kind='app'", p![key])? else { return Ok(None) };
        let t = now();
        if s.f("expires_at") < t {
            return Ok(None);
        }
        let Some(u) = st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![s.i("user_id")])? else { return Ok(None) };
        // sliding expiry, written at most once a day per device
        if t - s.f("last_seen_at") > APP_SEEN_EVERY {
            let ua: String = parts.headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").chars().take(200).collect();
            st.db.execute(
                "UPDATE sessions SET last_seen_at=?, expires_at=?, ip=?, user_agent=CASE WHEN ?='' THEN user_agent ELSE ? END WHERE token=?",
                p![t, t + app_session_secs(st) as f64, client_ip(parts), ua, ua, key],
            )?;
        }
        return Ok(Some((u, Via::Bearer)));
    }
    let jar = CookieJar::from_headers(&parts.headers);
    let Some(token) = jar.get(COOKIE).map(|c| c.value().to_string()) else { return Ok(None) };
    let Some(s) = st.db.one("SELECT * FROM sessions WHERE token=? AND kind='web'", p![token])? else { return Ok(None) };
    if s.f("expires_at") < now() {
        return Ok(None);
    }
    Ok(st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![s.i("user_id")])?.map(|u| (u, Via::Cookie)))
}

/// Who these headers still sign in as (read only: no sliding expiry). Long-lived streams call it from time to
/// time, so a revoked session, a disabled user or a changed role takes effect without a reconnect.
pub fn current_user(st: &AppState, headers: &HeaderMap) -> Option<Row> {
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let want = st.cfg.str("api_key");
        if !want.is_empty() && ct_eq(key, &want) {
            return st.db.one("SELECT * FROM users WHERE role IN ('owner','admin') AND disabled=0 ORDER BY role='owner' DESC, id LIMIT 1", p![]).ok().flatten();
        }
    }
    let (kind, token) = match bearer(headers) {
        Some(t) => ("app", app_token_key(&t)),
        None => ("web", CookieJar::from_headers(headers).get(COOKIE)?.value().to_string()),
    };
    let s = st.db.one("SELECT user_id, expires_at FROM sessions WHERE token=? AND kind=?", p![token, kind]).ok().flatten()?;
    if s.f("expires_at") < now() {
        return None;
    }
    st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![s.i("user_id")]).ok().flatten()
}

/// The signed-in user. Mutating cookie requests must carry `X-Requested-With: encore` (CSRF); API-key and
/// bearer requests are exempt.
pub struct User(pub Row);
pub struct Admin(pub Row);
pub struct Owner(pub Row);
/// Signed in or not.
pub struct MaybeUser(pub Option<Row>);

impl FromRequestParts<AppState> for User {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let Some((mut u, via)) = user_from_parts(st, parts)? else { return Err(err(401, "Not authenticated")) };
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) && via == Via::Cookie {
            let ok = parts.headers.get("x-requested-with").and_then(|v| v.to_str().ok()) == Some("encore");
            if !ok {
                return Err(err(403, "Missing CSRF header"));
            }
        }
        u.remove("password_hash");
        Ok(User(u))
    }
}

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let User(u) = User::from_request_parts(parts, st).await?;
        if !is_admin(&u) {
            return Err(err(403, "Admin only"));
        }
        Ok(Admin(u))
    }
}

impl FromRequestParts<AppState> for Owner {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let User(u) = User::from_request_parts(parts, st).await?;
        if !is_owner(&u) {
            return Err(err(403, "Only the server owner can do this"));
        }
        Ok(Owner(u))
    }
}

impl FromRequestParts<AppState> for MaybeUser {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(user_from_parts(st, parts)?.map(|(u, _)| u)))
    }
}

/// Client address + user agent, for sessions and rate limits.
pub struct ClientInfo {
    pub ip: String,
    pub ua: String,
    pub https: bool,
    pub base_url: String,
}

impl FromRequestParts<AppState> for ClientInfo {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _st: &AppState) -> Result<Self, Self::Rejection> {
        let h = |k: &str| parts.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let https = h("x-forwarded-proto") == "https" || parts.uri.scheme_str() == Some("https");
        let host = if !h("x-forwarded-host").is_empty() { h("x-forwarded-host") } else { h("host") };
        Ok(ClientInfo { ip: client_ip(parts), ua: h("user-agent"), https, base_url: format!("{}://{}", if https { "https" } else { "http" }, host) })
    }
}
