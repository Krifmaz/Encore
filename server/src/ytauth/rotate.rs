//! Light refresh of pasted cookies: what a signed-in browser tab does every few minutes. Google's
//! sign-in cookies (__Secure-1PSIDTS / __Secure-3PSIDTS) are short-lived; the page renews them with a
//! POST to `/RotateCookies` on the accounts host of the site (accounts.youtube.com for YouTube,
//! accounts.google.com for google.com) and gets fresh values back as Set-Cookie. The request body and
//! headers are the ones current cookie-refresh implementations use (gemini-webapi's rotate_1psidts):
//! `Content-Type: application/json`, the accounts host as Origin, the body `[000,"-0000000000000000000"]`.
//! A 401 means the account was signed out.
use super::jar::Jar;

pub const BODY: &str = r#"[000,"-0000000000000000000"]"#;
pub const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub host: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: &'static str,
}

/// The rotation calls for a jar: one per accounts host it has a session for (youtube.com first).
pub fn build(jar: &Jar, now: i64) -> Vec<Request> {
    let mut out = vec![];
    for host in ["accounts.youtube.com", "accounts.google.com"] {
        let cookie = jar.header_for(host, now);
        // only where there is a session to rotate
        let has_session = ["__Secure-1PSID", "__Secure-3PSID", "SID"].iter().any(|n| jar.get(host, n).is_some());
        if cookie.is_empty() || !has_session {
            continue;
        }
        out.push(Request { host, url: format!("https://{host}/RotateCookies"), body: BODY, headers: vec![
            ("Content-Type".into(), "application/json".into()),
            ("Origin".into(), format!("https://{host}")),
            ("Referer".into(), format!("https://{host}/RotateCookiesPage?origin=https://www.youtube.com&yt_pid=1")),
            ("User-Agent".into(), UA.into()),
            ("Cookie".into(), cookie),
        ] });
    }
    out
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// new cookies merged (how many Set-Cookie headers)
    Rotated(usize),
    SignedOut,
    Failed(String),
}

/// Send the rotation calls and merge what comes back into `jar`.
pub async fn run(http: &reqwest::Client, jar: &mut Jar, now: i64) -> Outcome {
    let calls = build(jar, now);
    if calls.is_empty() {
        return Outcome::Failed("these cookies have no Google sign-in session to refresh".into());
    }
    let mut merged = 0;
    let mut last_err = String::new();
    for c in calls {
        let mut rb = http.post(&c.url).body(c.body).timeout(std::time::Duration::from_secs(20));
        for (k, v) in &c.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        match rb.send().await {
            Ok(r) => {
                let code = r.status().as_u16();
                for h in r.headers().get_all("set-cookie").iter().filter_map(|h| h.to_str().ok()) {
                    jar.apply_set_cookie(h, c.host, now);
                    merged += 1;
                }
                if code == 401 {
                    return Outcome::SignedOut;
                }
                if code >= 400 {
                    last_err = format!("{} answered HTTP {code}", c.host);
                }
            }
            Err(e) => last_err = format!("{}: {e}", c.host),
        }
    }
    if merged == 0 && !last_err.is_empty() { Outcome::Failed(last_err) } else { Outcome::Rotated(merged) }
}
