//! A cookie jar in the shapes YouTube cookies arrive and leave in: Netscape cookies.txt (yt-dlp, pasted),
//! Chrome DevTools `Storage.getCookies` (the managed browser) and `chrome.cookies.getAll` (the extension).
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Cookie {
    /// ".youtube.com" for a domain cookie, "music.youtube.com" for a host-only one
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// unix seconds; 0 = a session cookie
    pub expires: i64,
    pub name: String,
    pub value: String,
}

/// The cookies that keep a Google account signed in: their expiry is the jar's expiry.
pub const AUTH_NAMES: [&str; 6] = ["SAPISID", "__Secure-3PAPISID", "__Secure-1PSID", "__Secure-3PSID", "SID", "LOGIN_INFO"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Jar(pub Vec<Cookie>);

fn tf(s: &str) -> bool {
    s.trim().eq_ignore_ascii_case("true")
}

/// Domains Encore keeps (and sends to YouTube): youtube.com and google.com, nothing else from a browser.
pub fn wanted_domain(d: &str) -> bool {
    let d = d.trim_start_matches('.').to_ascii_lowercase();
    ["youtube.com", "google.com"].iter().any(|w| d == *w || d.ends_with(&format!(".{w}")))
}

/// Does a cookie for `domain` go to `host`?
fn matches_host(domain: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    match domain.strip_prefix('.') {
        Some(d) => host == d || host.ends_with(&format!(".{d}")),
        None => host == domain.to_ascii_lowercase(),
    }
}

impl Jar {
    /// Netscape cookies.txt (tab separated; `#HttpOnly_` prefix marks HttpOnly cookies).
    pub fn parse_netscape(text: &str) -> Jar {
        let mut out = Jar::default();
        for line in text.lines() {
            let (line, http_only) = match line.strip_prefix("#HttpOnly_") { Some(l) => (l, true), None => (line, false) };
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                continue;
            }
            let mut domain = f[0].trim().to_string();
            // the include-subdomains flag and a leading dot say the same thing; keep them in step
            if tf(f[1]) && !domain.starts_with('.') {
                domain = format!(".{domain}");
            }
            out.set(Cookie { domain, path: f[2].to_string(), secure: tf(f[3]), http_only, expires: f[4].trim().parse::<f64>().unwrap_or(0.0) as i64,
                name: f[5].to_string(), value: f[6].trim_end_matches(['\r', '\n']).to_string() });
        }
        out
    }

    /// The jar as a Netscape cookies.txt yt-dlp reads.
    pub fn to_netscape(&self) -> String {
        let mut s = String::from("# Netscape HTTP Cookie File\n# Written by Encore – YouTube sign-in\n\n");
        for c in &self.0 {
            let sub = c.domain.starts_with('.');
            s.push_str(&format!("{}{}\t{}\t{}\t{}\t{}\t{}\t{}\n", if c.http_only { "#HttpOnly_" } else { "" }, c.domain,
                if sub { "TRUE" } else { "FALSE" }, c.path, if c.secure { "TRUE" } else { "FALSE" }, c.expires.max(0), c.name, c.value));
        }
        s
    }

    /// Chrome DevTools `Storage.getCookies` / `Network.getAllCookies` → `{cookies: [{name, value, domain, path,
    /// expires (-1 = session), httpOnly, secure, session}]}` (or the array itself).
    pub fn from_cdp(v: &Value) -> Jar {
        let list = v["cookies"].as_array().or(v.as_array()).cloned().unwrap_or_default();
        let mut out = Jar::default();
        for c in list {
            let domain = c["domain"].as_str().unwrap_or("").to_string();
            if !wanted_domain(&domain) {
                continue;
            }
            let session = c["session"].as_bool().unwrap_or(false);
            let exp = c["expires"].as_f64().unwrap_or(-1.0);
            out.set(Cookie { domain, path: c["path"].as_str().unwrap_or("/").to_string(), secure: c["secure"].as_bool().unwrap_or(false),
                http_only: c["httpOnly"].as_bool().unwrap_or(false), expires: if session || exp <= 0.0 { 0 } else { exp as i64 },
                name: c["name"].as_str().unwrap_or("").to_string(), value: c["value"].as_str().unwrap_or("").to_string() });
        }
        out
    }

    /// `chrome.cookies.getAll()` / Firefox `browser.cookies.getAll()` items: {domain, hostOnly, name, value,
    /// path, secure, httpOnly, session, expirationDate}.
    pub fn from_extension(list: &[Value]) -> Jar {
        let mut out = Jar::default();
        for c in list {
            let mut domain = c["domain"].as_str().unwrap_or("").to_string();
            if !wanted_domain(&domain) || c["name"].as_str().unwrap_or("").is_empty() {
                continue;
            }
            let host_only = c["hostOnly"].as_bool().unwrap_or(false);
            if host_only {
                domain = domain.trim_start_matches('.').to_string();
            } else if !domain.starts_with('.') {
                domain = format!(".{domain}");
            }
            let session = c["session"].as_bool().unwrap_or(false);
            let exp = c["expirationDate"].as_f64().unwrap_or(0.0);
            out.set(Cookie { domain, path: c["path"].as_str().unwrap_or("/").to_string(), secure: c["secure"].as_bool().unwrap_or(false),
                http_only: c["httpOnly"].as_bool().unwrap_or(false), expires: if session { 0 } else { exp as i64 },
                name: c["name"].as_str().unwrap_or("").to_string(), value: c["value"].as_str().unwrap_or("").to_string() });
        }
        out
    }

    /// Add or replace (same domain, path and name).
    pub fn set(&mut self, c: Cookie) {
        if let Some(old) = self.0.iter_mut().find(|o| o.domain.eq_ignore_ascii_case(&c.domain) && o.path == c.path && o.name == c.name) {
            *old = c;
        } else {
            self.0.push(c);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, host: &str, name: &str) -> Option<&Cookie> {
        self.0.iter().find(|c| c.name == name && matches_host(&c.domain, host))
    }

    /// `Cookie:` header for a request to `host` (expired cookies left out).
    pub fn header_for(&self, host: &str, now: i64) -> String {
        self.0.iter().filter(|c| matches_host(&c.domain, host) && (c.expires == 0 || c.expires > now))
            .map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; ")
    }

    /// The youtube.com sign-in cookies that are still valid.
    fn auth_cookies(&self, now: i64) -> impl Iterator<Item = &Cookie> {
        self.0.iter().filter(move |c| AUTH_NAMES.contains(&c.name.as_str()) && matches_host(&c.domain, "music.youtube.com") && (c.expires == 0 || c.expires > now))
    }

    /// Signed-in cookies are there and not expired (SAPISID or its secure twin is what YouTube checks).
    pub fn has_auth(&self, now: i64) -> bool {
        self.auth_cookies(now).any(|c| c.name == "SAPISID" || c.name == "__Secure-3PAPISID")
    }

    /// The earliest expiry of the sign-in cookies (None: none of them, or session cookies only).
    pub fn auth_expiry(&self, now: i64) -> Option<i64> {
        self.auth_cookies(now).filter(|c| c.expires > 0).map(|c| c.expires).min()
    }

    /// Fold a `Set-Cookie` header from `host` into the jar (a Max-Age of 0 or a past expiry deletes).
    pub fn apply_set_cookie(&mut self, header: &str, host: &str, now: i64) {
        let mut parts = header.split(';');
        let Some((name, value)) = parts.next().and_then(|kv| kv.split_once('=')) else { return };
        let (name, value) = (name.trim().to_string(), value.trim().to_string());
        if name.is_empty() {
            return;
        }
        let mut c = Cookie { domain: host.to_ascii_lowercase(), path: "/".into(), secure: false, http_only: false, expires: 0, name, value };
        let mut max_age: Option<i64> = None;
        for a in parts {
            let (k, v) = a.split_once('=').map(|(k, v)| (k.trim(), v.trim())).unwrap_or((a.trim(), ""));
            match k.to_ascii_lowercase().as_str() {
                "domain" if !v.is_empty() => c.domain = format!(".{}", v.trim_start_matches('.').to_ascii_lowercase()),
                "path" if !v.is_empty() => c.path = v.to_string(),
                "secure" => c.secure = true,
                "httponly" => c.http_only = true,
                "max-age" => max_age = v.parse().ok(),
                "expires" => {
                    if let Some(t) = parse_http_date(v) {
                        c.expires = t;
                    }
                }
                _ => {}
            }
        }
        if let Some(m) = max_age {
            c.expires = if m <= 0 { -1 } else { now + m };
        }
        if c.expires < 0 || (c.expires > 0 && c.expires <= now) {
            let (d, p, n) = (c.domain.clone(), c.path.clone(), c.name.clone());
            self.0.retain(|o| !(o.domain.eq_ignore_ascii_case(&d) && o.path == p && o.name == n));
            return;
        }
        self.set(c);
    }
}

/// "Wed, 21 Oct 2026 07:28:00 GMT" (also with dashes: "Wed, 21-Oct-2026 07:28:00 GMT") → unix seconds.
pub fn parse_http_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let s = s.split_once(',').map(|(_, r)| r.trim()).unwrap_or(s);
    let f: Vec<&str> = s.split([' ', '-']).filter(|x| !x.is_empty()).collect();
    if f.len() < 4 {
        return None;
    }
    let day: i64 = f[0].parse().ok()?;
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter()
        .position(|m| f[1].to_ascii_lowercase().starts_with(m))? as i64 + 1;
    let mut year: i64 = f[2].parse().ok()?;
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }
    let hms: Vec<i64> = f[3].split(':').map(|x| x.parse().unwrap_or(0)).collect();
    Some(days_from_civil(year, month, day) * 86400 + hms.first().copied().unwrap_or(0) * 3600 + hms.get(1).copied().unwrap_or(0) * 60 + hms.get(2).copied().unwrap_or(0))
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
