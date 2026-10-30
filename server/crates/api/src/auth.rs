//! Users, sessions, roles and permissions.
//!
//! Roles: owner > admin > user. The owner is the person who runs the server: only they can promote or
//! demote admins, and nobody can change or remove the owner. Admins manage everything else.
use encore_db::{now, Row, RowExt};
use encore_core::error::{err, ApiError, ApiResult};
use encore_db::p;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, Method};
use axum_extra::extract::CookieJar;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use subtle::ConstantTimeEq;

/// The web session cookie's name.
pub const COOKIE: &str = "encore_session";

use encore_core::ids::random_token;

/// Compare two secrets in constant time.
pub fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

// ---- passwords --------------------------------------------------------------------------------

/// A bcrypt hash (cost 12) of the password's first 72 bytes.
pub fn hash_password(pw: &str) -> String {
    let b = &pw.as_bytes()[..pw.len().min(72)];
    bcrypt::hash(b, 12).unwrap_or_default()
}

/// Whether the password matches a bcrypt hash.
pub fn verify_password(pw: &str, hashed: &str) -> bool {
    let b = &pw.as_bytes()[..pw.len().min(72)];
    bcrypt::verify(b, hashed).unwrap_or(false)
}

/// [`hash_password`] on tokio's blocking pool, for handlers: bcrypt at cost 12 is about a quarter of a second
/// of CPU (more on a small NAS), which would hold one of the threads that answer requests.
pub async fn hash_password_off(pw: String) -> String {
    tokio::task::spawn_blocking(move || hash_password(&pw)).await.unwrap_or_default()
}

/// [`verify_password`] on tokio's blocking pool (see [`hash_password_off`]).
pub async fn verify_password_off(pw: String, hashed: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&pw, &hashed)).await.unwrap_or(false)
}

/// A username of 3 to 32 letters, digits, `_`, `-` and `.`; 400 otherwise.
pub fn validate_username(username: &str) -> ApiResult<()> {
    let ok_name = (3..=32).contains(&username.chars().count())
        && username.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !ok_name {
        return Err(err(400, "Username must be 3-32 characters: letters, numbers, . _ -"));
    }
    Ok(())
}

/// A valid username and a password of at least 8 characters; 400 otherwise.
pub fn validate_credentials(username: &str, password: &str) -> ApiResult<()> {
    validate_username(username)?;
    if password.chars().count() < 8 {
        return Err(err(400, "Password must be at least 8 characters"));
    }
    Ok(())
}

// ---- rate limits ------------------------------------------------------------------------------

/// The longest window any limit counts over (sign-ups and bug reports: an hour). An address whose last event
/// is older than this can't be limited by anything any more, so the sweep drops it.
const LIMIT_WINDOW_MAX: f64 = 3600.0;
/// Calls between two sweeps of the whole table.
const SWEEP_EVERY: u64 = 256;

/// Recent events per key (an action and a client address or a user id). Keys with nothing recent left are
/// dropped every [`SWEEP_EVERY`] calls, so the table holds only the addresses of the last hour.
#[derive(Default)]
pub struct RateLimiter(Mutex<Limits>);

/// The events behind [`RateLimiter`]'s lock.
#[derive(Default)]
struct Limits {
    /// the times of each key's recent events
    keys: HashMap<String, VecDeque<f64>>,
    /// calls since the last sweep
    calls: u64,
}

impl Limits {
    /// Counts a call; every [`SWEEP_EVERY`] calls the keys with no event in the last hour go.
    fn tick(&mut self, t: f64) {
        self.calls += 1;
        if self.calls.is_multiple_of(SWEEP_EVERY) {
            self.keys.retain(|_, q| q.back().is_some_and(|last| *last >= t - LIMIT_WINDOW_MAX));
        }
    }
}

impl RateLimiter {
    /// True when `key` already had `limit` events in the last `window` seconds (at most an hour). Records one
    /// unless `record` is false.
    pub fn limited(&self, key: &str, limit: usize, window: f64, record: bool) -> bool {
        debug_assert!(window <= LIMIT_WINDOW_MAX, "the sweep keeps events for an hour at most");
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        m.tick(t);
        let Some(q) = m.keys.get_mut(key) else {
            if record && limit > 0 {
                m.keys.insert(key.to_string(), VecDeque::from([t]));
            }
            return limit == 0;
        };
        while q.front().is_some_and(|x| *x < t - window) {
            q.pop_front();
        }
        if q.len() >= limit {
            return true;
        }
        if record {
            q.push_back(t);
        } else if q.is_empty() {
            m.keys.remove(key);
        }
        false
    }

    /// Record an event for `key`.
    pub fn record(&self, key: &str) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        m.tick(t);
        m.keys.entry(key.to_string()).or_default().push_back(t);
    }

    /// How many keys are kept (for the tests).
    #[cfg(test)]
    fn kept(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).keys.len()
    }
}

#[cfg(test)]
mod limiter_tests {
    use super::*;

    #[test]
    fn x_forwarded_for_counts_only_from_a_trusted_proxy_and_from_the_right() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().ok();
        // straight from the internet: the header is the client's own words
        assert_eq!(client_ip_from(ip("203.0.113.9"), Some("1.1.1.1"), ""), "203.0.113.9");
        assert_eq!(client_ip_from(ip("203.0.113.9"), None, ""), "203.0.113.9");
        // a proxy in another container: the address it appended, not what the client wrote in front of it
        assert_eq!(client_ip_from(ip("172.18.0.2"), Some("1.1.1.1, 203.0.113.9"), ""), "203.0.113.9");
        assert_eq!(client_ip_from(ip("127.0.0.1"), Some(" 203.0.113.9 "), ""), "203.0.113.9");
        assert_eq!(client_ip_from(ip("::ffff:10.0.0.3"), Some("203.0.113.9"), ""), "203.0.113.9", "IPv4-mapped");
        // a chain of trusted proxies is passed over; all of them trusted: the left-most (a LAN client)
        assert_eq!(client_ip_from(ip("10.0.0.3"), Some("203.0.113.9, 192.168.1.2"), ""), "203.0.113.9");
        assert_eq!(client_ip_from(ip("10.0.0.3"), Some("192.168.1.5"), ""), "192.168.1.5");
        assert_eq!(client_ip_from(ip("10.0.0.3"), Some("[2001:db8::1]:443"), ""), "2001:db8::1");
        // the setting replaces the private networks: a public proxy (Cloudflare) is trusted, the LAN is not
        assert_eq!(client_ip_from(ip("198.51.100.7"), Some("1.1.1.1, 203.0.113.9"), "198.51.100.0/24"), "203.0.113.9");
        assert_eq!(client_ip_from(ip("10.0.0.3"), Some("203.0.113.9"), "198.51.100.0/24"), "10.0.0.3");
        assert_eq!(client_ip_from(ip("2001:db8::5"), Some("203.0.113.9"), "2001:db8::/32, 198.51.100.7"), "203.0.113.9");
        // no connection address (not served over a socket): nothing to trust
        assert_eq!(client_ip_from(None, Some("1.1.1.1"), ""), "");
    }

    #[test]
    fn limits_count_per_key_and_old_keys_are_swept() {
        let l = RateLimiter::default();
        for _ in 0..3 {
            assert!(!l.limited("login:1.2.3.4", 3, 300.0, true));
        }
        assert!(l.limited("login:1.2.3.4", 3, 300.0, true));
        assert!(!l.limited("login:5.6.7.8", 3, 300.0, true), "another address has its own count");
        // a look that records nothing keeps nothing
        assert!(!l.limited("login:9.9.9.9", 3, 300.0, false));
        assert_eq!(l.kept(), 2);
        l.record("login:9.9.9.9");
        assert_eq!(l.kept(), 3);
        // events older than the longest window: the next sweep drops their keys
        {
            let mut m = l.0.lock().unwrap();
            for q in m.keys.values_mut() {
                for t in q.iter_mut() {
                    *t -= LIMIT_WINDOW_MAX + 1.0;
                }
            }
        }
        for _ in 0..SWEEP_EVERY {
            l.limited("login:fresh", 3, 300.0, false);
        }
        assert_eq!(l.kept(), 0, "old addresses are gone");
        assert!(!l.limited("login:1.2.3.4", 3, 300.0, true), "and their counts with them");
    }
}

// ---- roles & permissions ----------------------------------------------------------------------

use encore_config::perms::{is_admin, is_owner};

/// The user's permissions, by name.
pub fn perms_of(st: &AppState, u: &Row) -> Map<String, Value> {
    encore_config::perms::perms_of(&st.cfg, u)
}

/// Whether the user has permission `perm`.
pub fn can(st: &AppState, u: &Row, perm: &str) -> bool {
    encore_config::perms::can(&st.cfg, u, perm)
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
    m.insert("prefs".into(), serde_json::from_str(&u.s("prefs")).unwrap_or_else(|_| json!({})));
    Value::Object(m)
}

// ---- users & sessions -------------------------------------------------------------------------

/// Create an account (the credentials checked, the username free); its id.
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

/// Create a web session for the user: its token and how long it lasts (seconds).
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
    format!("sha256:{}", encore_core::ids::sha256_hex(token))
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

/// The networks trusted as proxies while `trusted_proxies` is empty: loopback, the private networks (a proxy
/// in another container or on the same LAN), shared address space (Tailscale) and link-local.
const DEFAULT_PROXIES: &[&str] = &["127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10", "169.254.0.0/16", "::1/128", "fc00::/7", "fe80::/10"];

/// An address, also with a port ("1.2.3.4:80", "\[::1\]:80"); IPv4-mapped IPv6 as IPv4.
fn parse_ip(s: &str) -> Option<std::net::IpAddr> {
    let s = s.trim();
    s.parse::<std::net::IpAddr>().ok().or_else(|| s.parse::<std::net::SocketAddr>().ok().map(|a| a.ip())).map(|ip| ip.to_canonical())
}

/// Whether `ip` is in `net` ("10.0.0.0/8", "2001:db8::/32", or one address).
fn in_net(ip: std::net::IpAddr, net: &str) -> bool {
    use std::net::IpAddr;
    let (addr, bits) = match net.trim().split_once('/') {
        Some((a, b)) => (a, b.trim().parse::<u32>().ok()),
        None => (net.trim(), None),
    };
    let Some(base) = parse_ip(addr) else { return false };
    match (ip, base) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let n = bits.unwrap_or(32).min(32);
            let mask = if n == 0 { 0 } else { u32::MAX << (32 - n) };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let n = bits.unwrap_or(128).min(128);
            let mask = if n == 0 { 0 } else { u128::MAX << (128 - n) };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

/// The client's address from the connection's address (`peer`, None when unknown), the `X-Forwarded-For`
/// header and the trusted proxies (networks, comma-separated; empty: [`DEFAULT_PROXIES`]). The header counts
/// only when the connection comes from a trusted proxy; then the right-most entry that is not itself a
/// trusted proxy is the client (a proxy appends the address it saw, so what a client wrote at the front of the
/// header can't name another address), and when every entry is a trusted proxy, the left-most. Without a
/// connection address there is nothing to trust: "".
pub fn client_ip_from(peer: Option<std::net::IpAddr>, forwarded: Option<&str>, trusted: &str) -> String {
    let nets: Vec<&str> = trusted.split(',').map(str::trim).filter(|n| !n.is_empty()).collect();
    let nets: &[&str] = if nets.is_empty() { DEFAULT_PROXIES } else { &nets };
    let is_proxy = |ip: std::net::IpAddr| nets.iter().any(|n| in_net(ip, n));
    let Some(peer) = peer.map(|p| p.to_canonical()) else { return String::new() };
    let Some(f) = forwarded.filter(|_| is_proxy(peer)) else { return peer.to_string() };
    let entries: Vec<&str> = f.split(',').map(str::trim).filter(|e| !e.is_empty()).collect();
    for e in entries.iter().rev() {
        match parse_ip(e) {
            Some(ip) if is_proxy(ip) => continue,
            Some(ip) => return ip.to_string(),
            // not an address ("unknown"): what the proxy wrote, as it is
            None => return e.to_string(),
        }
    }
    entries.first().and_then(|e| parse_ip(e)).map(|ip| ip.to_string()).unwrap_or_else(|| peer.to_string())
}

/// The address a request came from (see [`client_ip_from`]; `trusted_proxies` names the proxies).
pub fn client_ip(st: &AppState, parts: &Parts) -> String {
    let peer = parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0.ip());
    let forwarded = parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    client_ip_from(peer, forwarded, &st.cfg.str("trusted_proxies"))
}

/// How a request signed in. Only cookies need the CSRF header: a browser never adds the other two on its own.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Via {
    /// the server's API key (`x-api-key`), as the first admin
    ApiKey,
    /// an app token (`Authorization: Bearer`)
    Bearer,
    /// the web session cookie
    Cookie,
}

/// The signed-in user of a request, and how it signed in; `None` when it isn't signed in.
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
                p![t, t + app_session_secs(st) as f64, client_ip(st, parts), ua, ua, key],
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
/// A signed-in admin (or the owner); 403 otherwise.
pub struct Admin(pub Row);
/// The signed-in owner; 403 otherwise.
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
    /// the client's address (forwarded by a trusted proxy)
    pub ip: String,
    /// its User-Agent
    pub ua: String,
    /// whether the request came over HTTPS
    pub https: bool,
    /// the address the client reached the server at ("https://music.example.com")
    pub base_url: String,
}

impl FromRequestParts<AppState> for ClientInfo {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let h = |k: &str| parts.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let https = h("x-forwarded-proto") == "https" || parts.uri.scheme_str() == Some("https");
        let host = if !h("x-forwarded-host").is_empty() { h("x-forwarded-host") } else { h("host") };
        Ok(ClientInfo { ip: client_ip(st, parts), ua: h("user-agent"), https, base_url: format!("{}://{}", if https { "https" } else { "http" }, host) })
    }
}
