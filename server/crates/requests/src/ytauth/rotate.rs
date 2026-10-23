//! Light refresh of a Google sign-in: what a signed-in browser tab does every few minutes. Google's
//! sign-in cookies (__Secure-1PSIDTS / __Secure-3PSIDTS) are short-lived; the page renews them with a
//! POST to `/RotateCookies` on the accounts host of the site (accounts.youtube.com for YouTube,
//! accounts.google.com for google.com) and gets fresh values back as Set-Cookie. The request body and
//! headers are the ones current cookie-refresh implementations use (gemini-webapi's rotate_1psidts):
//! `Content-Type: application/json`, the accounts host as Origin, the body `[000,"-0000000000000000000"]`.
//! A 401 means the account was signed out. The User-Agent is the browser the session came from
//! (`user_agent`): Google ties a sign-in to its browser, so the refresh presents the same one.
use super::jar::Jar;

/// The body of a rotation call.
pub const BODY: &str = r#"[000,"-0000000000000000000"]"#;
/// The browser Encore presents when it knows nothing better (pasted cookies, a hand-off from an extension
/// older than 0.26 that reported no User-Agent): Chrome on Linux.
pub const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// The User-Agent for a session: the full `navigator.userAgent` the extension reported with the cookies,
/// else one built from the short browser name it reported ("Firefox 143", "Edge 141"), else [UA].
pub fn user_agent(reported: Option<&str>, agent: Option<&str>) -> String {
    if let Some(r) = reported.map(str::trim).filter(|r| r.starts_with("Mozilla/") && r.len() <= 400 && !r.chars().any(|c| c.is_control())) {
        return r.to_string();
    }
    let Some((name, major)) = agent.and_then(|a| a.trim().split_once(' ')).filter(|(_, v)| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit())) else { return UA.into() };
    match name {
        "Firefox" => format!("Mozilla/5.0 (X11; Linux x86_64; rv:{major}.0) Gecko/20100101 Firefox/{major}.0"),
        "Chrome" | "Brave" => format!("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"),
        "Edge" => format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36 Edg/{major}.0.0.0"),
        "Opera" => format!("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36 OPR/{major}.0.0.0"),
        _ => UA.into(),
    }
}

/// One rotation call.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// the accounts host it goes to
    pub host: &'static str,
    /// the RotateCookies URL
    pub url: String,
    /// its headers
    pub headers: Vec<(String, String)>,
    /// its body ([`BODY`])
    pub body: &'static str,
}

/// The rotation calls for a jar: one per accounts host it has a session for (youtube.com first), presented
/// as the browser `ua`.
pub fn build(jar: &Jar, now: i64, ua: &str) -> Vec<Request> {
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
            ("User-Agent".into(), ua.into()),
            ("Cookie".into(), cookie),
        ] });
    }
    out
}

/// How a rotation went.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// new cookies merged (how many Set-Cookie headers; 0: Google answered but renewed nothing)
    Rotated(usize),
    /// Google answered 401: the account is signed out
    SignedOut,
    /// it failed (the network, another answer)
    Failed(String),
}

/// Send the rotation calls as the browser `ua` and merge what comes back into `jar`.
pub async fn run(http: &reqwest::Client, jar: &mut Jar, now: i64, ua: &str) -> Outcome {
    let calls = build(jar, now, ua);
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
