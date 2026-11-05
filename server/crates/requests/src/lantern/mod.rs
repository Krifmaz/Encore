//! Lantern: the owner's own grabber (torrents, Usenet and Soulseek in one service) as the place Encore's
//! requests are searched and downloaded, instead of Encore's own Prowlarr, download clients and Soulseek
//! engine. Encore still imports, tags and files what Lantern finishes.
//!
//! Two steps, so nothing changes for a server that doesn't run Lantern:
//!
//! 1. **Connected**: Lantern's address and API key are saved (Settings → Connections → Lantern, the
//!    `lantern_url` and `lantern_api_key` settings). Nothing is routed yet; the card can test the address and
//!    show what the move would carry over ([`migrate::plan`], the dry run).
//! 2. **Taken over**: the one-time move of the download settings into Lantern is done
//!    ([`migrate::apply`], which sets `lantern_took_over`). From then on the request worker asks Lantern
//!    where it asked Soulseek and the indexers ([`route`]), and the native cards are hidden
//!    ([`hides_plugin`]). Disconnecting ([`disconnect`]) undoes both: the native sources and their
//!    settings, which were never deleted, are back as they were.
//!
//! Lantern's API is not final until Lantern 1.0, so the parts that talk to it (search, grab, the live
//! progress of its event stream, writing the moved settings) wait for it: [`NOT_YET`] says so wherever one
//! is asked for. Everything that doesn't depend on that API (the connection, the routing, the dry run, the
//! hiding) is here and tested.
use encore_http::Ctx;
use serde_json::{json, Value};
use std::time::Duration;

pub mod migrate;
#[cfg(test)]
mod tests;

/// The source name Lantern has in a request's source list (`detail.sources`, `source`).
pub const SOURCE: &str = "lantern";

/// The native sources Lantern stands in for once it has taken over: Soulseek (in any format and FLAC
/// only) and the torrent and Usenet indexers.
pub const COVERED_SOURCES: [&str; 3] = ["slskd", "slskd_lossless", "torznab"];

/// The Settings → Plugins cards Lantern takes over (hidden while it has): the Torrents & Usenet and
/// Soulseek cards and every download client the Torrents & Usenet card hands releases to.
pub const COVERED_PLUGINS: [&str; 10] = [
    "torznab", "slskd", "qbittorrent", "transmission", "deluge", "rtorrent", "torrent-blackhole", "sabnzbd", "nzbget",
    "usenet-blackhole",
];

/// What every part that needs Lantern's API answers until that API is final.
pub const NOT_YET: &str = "Lantern 1.0 isn't out yet: Encore searches, downloads and moves settings through Lantern once its API is final";

/// How long the connection test waits for Lantern to answer.
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A saved Lantern connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// Lantern's base address, without a trailing slash ([`base`])
    pub url: String,
    /// its API key
    pub api_key: String,
}

impl Connection {
    /// The saved connection, when both the address and the API key are set.
    pub fn of(st: &impl Ctx) -> Option<Connection> {
        let url = base(&st.cfg().str("lantern_url"));
        let api_key = st.cfg().str("lantern_api_key").trim().to_string();
        (!url.is_empty() && !api_key.is_empty()).then_some(Connection { url, api_key })
    }
}

/// Whether Lantern's address and API key are both saved.
pub fn connected(st: &impl Ctx) -> bool {
    Connection::of(st).is_some()
}

/// Whether Lantern has taken over the torrent, Usenet and Soulseek downloads: connected, and the
/// one-time move of the settings done. A connection whose address or key was removed (by hand, or an
/// environment variable that went away) has not: the native sources work again at once.
pub fn took_over(st: &impl Ctx) -> bool {
    connected(st) && st.cfg().bool("lantern_took_over")
}

/// Whether the Settings → Plugins card `plugin` is hidden because Lantern has taken over what it sets up.
pub fn hides_plugin(st: &impl Ctx, plugin: &str) -> bool {
    COVERED_PLUGINS.contains(&plugin) && took_over(st)
}

/// An address as typed, made a base address: spaces around it and trailing slashes go.
pub fn base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// Why `url` can't be Lantern's address, if it can't: it must be an http(s) address with a host.
pub fn url_problem(url: &str) -> Option<&'static str> {
    let u = base(url);
    let rest = u.strip_prefix("http://").or_else(|| u.strip_prefix("https://"));
    match rest {
        None => Some("Lantern's address starts with http:// or https://"),
        Some(r) if r.is_empty() || r.starts_with('/') => Some("Lantern's address needs a host, e.g. http://lantern:8080"),
        Some(_) => None,
    }
}

/// The sources a request tries, with Lantern standing in for Soulseek and the indexers: the covered
/// sources go, and Lantern takes the place of the first of them, so it keeps the admin's order against
/// streamrip, yt-dlp and the plugins. An order without a covered source is left as it is (Lantern is
/// only asked where one of the sources it replaces would have been).
pub fn route(order: Vec<String>) -> Vec<String> {
    let Some(at) = order.iter().position(|s| COVERED_SOURCES.contains(&s.as_str())) else { return order };
    let mut out: Vec<String> = Vec::with_capacity(order.len());
    for (i, s) in order.into_iter().enumerate() {
        if i == at {
            out.push(SOURCE.to_string());
        } else if !COVERED_SOURCES.contains(&s.as_str()) {
            out.push(s);
        }
    }
    out
}

/// The connection as the Connections card shows it (GET /api/admin/lantern): the address, whether a key is
/// saved (never the key), whether either is locked by an environment variable, the two steps' state and
/// what waits for Lantern 1.0.
pub fn state(st: &impl Ctx) -> Value {
    let cfg = st.cfg();
    json!({
        "url": cfg.str("lantern_url"),
        "key_set": !cfg.str("lantern_api_key").trim().is_empty(),
        "env_locked": cfg.env_locked("lantern_url") || cfg.env_locked("lantern_api_key"),
        "connected": connected(st),
        "took_over": took_over(st),
        "api_ready": false,
        "waiting": NOT_YET,
    })
}

/// Save the connection (PUT /api/admin/lantern). `api_key` is `None` (or the masked form the card sends
/// back) to keep the saved key. Err with what's wrong for an address that can't be one.
pub fn save(st: &impl Ctx, url: &str, api_key: Option<&str>) -> Result<(), &'static str> {
    if let Some(why) = url_problem(url) {
        return Err(why);
    }
    let mut values = json!({ "lantern_url": base(url) });
    if let Some(k) = api_key.map(str::trim).filter(|k| !encore_config::Config::is_masked(&json!(k))) {
        values["lantern_api_key"] = json!(k);
    }
    st.cfg().set_many(&values);
    Ok(())
}

/// Forget the connection (DELETE /api/admin/lantern): the address, the key and the takeover are cleared,
/// so requests go through Encore's own sources and their cards show again. The native settings were never
/// removed, so they are exactly as they were.
pub fn disconnect(st: &impl Ctx) {
    st.cfg().set_many(&json!({ "lantern_url": "", "lantern_api_key": "", "lantern_took_over": false }));
}

/// The connection test (POST /api/system/test/lantern): `{ok, message}`. Lantern's address must answer
/// over HTTP; the API key can only be checked once Lantern 1.0's API is final, and the message says so.
pub async fn test(st: &impl Ctx) -> Value {
    let Some(c) = Connection::of(st) else {
        return json!({ "ok": false, "message": "Add Lantern's address and API key first" });
    };
    match st.http().get(&c.url).timeout(TEST_TIMEOUT).send().await {
        Ok(r) => json!({
            "ok": true,
            "message": format!("Lantern answers at {} (HTTP {}). The API key is checked once Lantern 1.0 is out.", c.url, r.status().as_u16()),
        }),
        Err(e) => json!({ "ok": false, "message": format!("Lantern isn't reachable at {}: {}", c.url, e.without_url()) }),
    }
}
