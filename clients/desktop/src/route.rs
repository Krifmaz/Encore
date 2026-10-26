//! Which address the app reaches the server at: the local network address while it answers, the public
//! one otherwise.
//!
//! The server publishes both under Settings → General (Public URL, Local network address) at
//! `GET /api/auth/addresses`, with the `server_id` its `/api/auth/info` also shows. The app keeps
//! every URL in the form of the server it was signed in to (the *home* address: the config's
//! `server`, which also names the kept pages, the offline songs and the group links), and [`net`]
//! turns such a URL into the one actually fetched just before it goes out. [`choose`] decides the
//! address: at sign-in, every minute ([`crate::app`]'s tick) and at once when a call to the chosen
//! address can't connect ([`fall_back`]).
//!
//! An address is used only after its `/api/auth/info` answered with this server's `server_id`, so
//! the session never goes to some other machine that happens to have the home network's address.

use serde_json::Value;
use std::sync::RwLock;
use std::time::Duration;

/// How long [`probe`] waits for an address: a home network answers in milliseconds, and a
/// network that doesn't have the address may only time out.
const PROBE_TIMEOUT: Duration = Duration::from_millis(2500);

/// Which of the server's addresses is in use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Via {
    /// The address the app was signed in to (nothing else known, or nothing else answers).
    #[default]
    Home,
    /// The server's Local network address.
    Local,
    /// The server's Public URL.
    Public,
}

/// What the app knows about the server's addresses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Route {
    /// The address the app was signed in to (no trailing `/`); every URL the app builds starts with it.
    pub home: String,
    /// The server's Local network address ("" when not set).
    pub local: String,
    /// The server's Public URL ("" when not set).
    pub public: String,
    /// The id `/api/auth/info` must show at an address before it is used.
    pub server_id: String,
    /// The address in use.
    pub via: Via,
}

impl Route {
    /// The address requests go to now (no trailing `/`).
    pub fn target(&self) -> &str {
        match self.via {
            Via::Local if !self.local.is_empty() => &self.local,
            Via::Public if !self.public.is_empty() => &self.public,
            _ => &self.home,
        }
    }

    /// `url` with the home address swapped for [`Route::target`]; other URLs as they are.
    pub fn rewrite(&self, url: &str) -> String {
        let target = self.target();
        if self.home.is_empty() || target == self.home {
            return url.to_string();
        }
        match url.strip_prefix(&self.home) {
            // only a whole host:port, never "http://nas:80" inside "http://nas:8080"
            Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') => format!("{target}{rest}"),
            _ => url.to_string(),
        }
    }

    /// The words of the indicator in Settings: which address and what it is.
    pub fn label(&self) -> String {
        let target = self.target();
        if same(target, &self.local) {
            format!("Local network ({target})")
        } else if same(target, &self.public) {
            format!("Public address ({target})")
        } else {
            target.to_string()
        }
    }
}

/// The route of this process (one server at a time, like [`crate::api::Api`]).
static ROUTE: RwLock<Route> = RwLock::new(Route {
    home: String::new(),
    local: String::new(),
    public: String::new(),
    server_id: String::new(),
    via: Via::Home,
});

/// A copy of the route now.
pub fn current() -> Route {
    ROUTE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Replaces the route.
fn set(r: Route) {
    *ROUTE.write().unwrap_or_else(|e| e.into_inner()) = r;
}

/// `url` as it is fetched now: a URL on the home address goes to the address in use.
pub fn net(url: &str) -> String {
    ROUTE.read().unwrap_or_else(|e| e.into_inner()).rewrite(url)
}

/// A new home address (sign-in, Change server): what was known about the old server is forgotten.
pub fn reset(home: &str) {
    set(Route { home: home.trim_end_matches('/').to_string(), ..Route::default() });
}

/// A call to the address in use could not connect: back to the home address until [`choose`]
/// finds a better one. True when the address changed (the call is worth one more try).
pub fn fall_back() -> bool {
    let mut r = ROUTE.write().unwrap_or_else(|e| e.into_inner());
    if r.via == Via::Home {
        return false;
    }
    r.via = Via::Home;
    true
}

/// The same address, ignoring case and a trailing `/` ("" never matches).
fn same(a: &str, b: &str) -> bool {
    !a.is_empty() && a.trim_end_matches('/').eq_ignore_ascii_case(b.trim_end_matches('/'))
}

/// `addresses.json` in the app's config folder: the last addresses each server gave, by home address, so a
/// computer signed in to the local address finds the public one while away, when the server can't be asked.
fn saved_path() -> std::path::PathBuf {
    crate::config::dir().join("addresses.json")
}

/// The addresses kept for `home` (local, public, server id); empty when none.
fn load(home: &str) -> (String, String, String) {
    let v: Value = std::fs::read_to_string(saved_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let e = &v[home];
    let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    (s("local"), s("public"), s("server_id"))
}

/// Keeps the addresses of `r.home` for the next start (best effort: a failed write only costs the
/// fallback while away).
fn save(r: &Route) {
    let path = saved_path();
    let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    if !v.is_object() {
        v = serde_json::json!({});
    }
    v[r.home.as_str()] = serde_json::json!({ "local": r.local, "public": r.public, "server_id": r.server_id });
    let _ = std::fs::create_dir_all(crate::config::dir());
    let _ = std::fs::write(&path, v.to_string());
}

/// An address from the server's answer: trimmed, no trailing `/`, "" unless it is http(s).
fn clean(v: &Value, key: &str) -> String {
    let s = v.get(key).and_then(Value::as_str).unwrap_or("").trim().trim_end_matches('/');
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        String::new()
    }
}

/// Whether `base` answers `/api/auth/info` as the server `server_id` (an empty id: any Encore
/// server, which only an old server without ids gives).
pub async fn probe(client: &reqwest::Client, base: &str, server_id: &str) -> bool {
    if base.is_empty() {
        return false;
    }
    let sent = client.get(format!("{base}/api/auth/info")).timeout(PROBE_TIMEOUT).send().await;
    let Ok(res) = sent else { return false };
    if !res.status().is_success() {
        return false;
    }
    let Ok(v) = res.json::<Value>().await else { return false };
    let id = v.get("server_id").and_then(Value::as_str).unwrap_or("");
    // an old server without the id can't be told apart from another one, so it is never switched to
    !server_id.is_empty() && id == server_id
}

/// Takes the server's `/api/auth/addresses` answer and picks the address: the local one when it
/// answers as this server, else the public one when it does, else the home address. Returns the
/// route chosen; `answer` None keeps what was known (the server didn't answer the question), which
/// after a restart is what [`save`] kept.
pub async fn choose(client: &reqwest::Client, home: &str, answer: Option<Value>) -> Route {
    let mut r = current();
    let home = home.trim_end_matches('/').to_string();
    if r.home != home {
        // a server not asked in this run yet: what it said last time
        let (local, public, server_id) = load(&home);
        r = Route { home, local, public, server_id, via: Via::Home };
    }
    if let Some(v) = answer {
        let (local, public) = (clean(&v, "local"), clean(&v, "public"));
        let server_id = v.get("server_id").and_then(Value::as_str).unwrap_or("").to_string();
        if (&local, &public, &server_id) != (&r.local, &r.public, &r.server_id) {
            (r.local, r.public, r.server_id) = (local, public, server_id);
            save(&r);
        }
    }
    // the local address is asked even when it is the home address: away from home it doesn't answer, and
    // then the public one is the way in
    r.via = if !r.local.is_empty() && probe(client, &r.local, &r.server_id).await {
        if same(&r.local, &r.home) { Via::Home } else { Via::Local }
    } else if !r.public.is_empty() && !same(&r.public, &r.home) && probe(client, &r.public, &r.server_id).await {
        Via::Public
    } else {
        Via::Home
    };
    // the server may have changed while the probes ran (Change server): keep the newer one
    if current().home == r.home {
        set(r.clone());
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(via: Via) -> Route {
        Route {
            home: "https://encore.example.uk".into(),
            local: "http://10.0.1.7:8080".into(),
            public: "https://encore.example.uk".into(),
            server_id: "abc".into(),
            via,
        }
    }

    #[test]
    fn rewrites_only_the_home_address() {
        let r = route(Via::Local);
        assert_eq!(r.rewrite("https://encore.example.uk/api/library/recent"), "http://10.0.1.7:8080/api/library/recent");
        assert_eq!(r.rewrite("https://encore.example.uk"), "http://10.0.1.7:8080");
        // another host, or a longer host that starts the same way, stays as it is
        assert_eq!(r.rewrite("https://cdn.example.com/a.jpg"), "https://cdn.example.com/a.jpg");
        assert_eq!(r.rewrite("https://encore.example.uk.evil/x"), "https://encore.example.uk.evil/x");
        assert_eq!(route(Via::Home).rewrite("https://encore.example.uk/api/x"), "https://encore.example.uk/api/x");
    }

    #[test]
    fn signed_in_locally_falls_back_to_the_public_address() {
        let mut r = route(Via::Public);
        r.home = "http://10.0.1.7:8080".into();
        assert_eq!(r.rewrite("http://10.0.1.7:8080/api/x"), "https://encore.example.uk/api/x");
        assert_eq!(r.label(), "Public address (https://encore.example.uk)");
        r.via = Via::Home;
        assert_eq!(r.label(), "Local network (http://10.0.1.7:8080)");
    }

    #[test]
    fn addresses_from_the_answer_are_cleaned() {
        let v = serde_json::json!({ "local": " http://10.0.1.7:8080/ ", "public": "encore.example.uk" });
        assert_eq!(clean(&v, "local"), "http://10.0.1.7:8080");
        assert_eq!(clean(&v, "public"), "", "not an http(s) address");
        assert!(same("HTTP://10.0.1.7:8080/", "http://10.0.1.7:8080"));
        assert!(!same("", ""));
    }
}
