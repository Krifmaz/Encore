//! How Encore talks to the internet: the HTTP clients ([`client`]) and a module per external service
//! (MusicBrainz, Cover Art Archive, Deezer, Apple Music, ListenBrainz, Last.fm, LRCLIB, AcoustID,
//! Discogs, Tidal, YouTube Music, GitHub, PyPI). Each module knows its service's addresses, parameters
//! and sign-in headers and hands out the requests; the caller picks the timeout and how it reads the
//! answer (the paced calls of Deezer and Apple wait for their turn in the module). Calls take a [`Ctx`]:
//! the clients, the settings, the service addresses, the request pacing and the small answer cache, which
//! the server's state provides.
use serde_json::Value;
use std::time::Duration;

pub mod acoustid;
pub mod apple;
pub mod client;
pub mod coverartarchive;
pub mod deezer;
pub mod discogs;
pub mod github;
pub mod lastfm;
pub mod listenbrainz;
pub mod lrclib;
pub mod mb;
mod mb_line;
pub mod progress;
pub mod pypi;
pub mod tidal;
pub mod ytmusic;

/// What a call to an external service needs from the server.
pub trait Ctx: Send + Sync {
    /// The shared client (built by [`client::build`]).
    fn http(&self) -> &reqwest::Client;
    /// The client that hands redirects back to the caller (built by [`client::no_redirects`]).
    fn http_no_redirects(&self) -> &reqwest::Client;
    /// Where the services tests stand in for live (the real hosts in use).
    fn urls(&self) -> &Urls;
    fn cfg(&self) -> &encore_config::Config;
    /// MusicBrainz: its rate limit and its hour-long answer cache.
    fn mb(&self) -> &mb::Mb;
    /// When the next call to each rate-limited service may go out.
    fn pace(&self) -> &Pace;
    /// The small in-memory cache for slow lookups.
    fn cache_get(&self, key: &str) -> Option<Value>;
    fn cache_put(&self, key: &str, v: Value, secs: u64);
}

impl<T: Ctx + ?Sized> Ctx for &T {
    fn http(&self) -> &reqwest::Client {
        (**self).http()
    }
    fn http_no_redirects(&self) -> &reqwest::Client {
        (**self).http_no_redirects()
    }
    fn urls(&self) -> &Urls {
        (**self).urls()
    }
    fn cfg(&self) -> &encore_config::Config {
        (**self).cfg()
    }
    fn mb(&self) -> &mb::Mb {
        (**self).mb()
    }
    fn pace(&self) -> &Pace {
        (**self).pace()
    }
    fn cache_get(&self, key: &str) -> Option<Value> {
        (**self).cache_get(key)
    }
    fn cache_put(&self, key: &str, v: Value, secs: u64) {
        (**self).cache_put(key, v, secs)
    }
}

/// The addresses of the services a test can stand in for. The real hosts by default; a test server points
/// them at its stand-ins, never through the process environment.
#[derive(Clone, Debug, PartialEq)]
pub struct Urls {
    /// Tidal's API (openapi v2)
    pub tidal: String,
    /// Qobuz's API, the one streamrip's client talks to
    pub qobuz_api: String,
    /// Qobuz's web player, where the app id comes from
    pub qobuz_player: String,
    /// YouTube Music's web API
    pub ytmusic: String,
    /// DeepL's API; empty: the one the key is for (api-free.deepl.com for a key ending in `:fx`, else
    /// api.deepl.com – encore_library::translate::deepl)
    pub deepl: String,
    /// Deezer's public API
    pub deezer: String,
}

impl Default for Urls {
    fn default() -> Self {
        Urls {
            tidal: tidal::API.into(),
            qobuz_api: "https://www.qobuz.com/api.json/0.2".into(),
            qobuz_player: "https://play.qobuz.com".into(),
            ytmusic: ytmusic::API.into(),
            deepl: String::new(),
            deezer: deezer::API.into(),
        }
    }
}

/// Request pacing for the services that turn away a fast caller, shared by everything that calls them.
#[derive(Clone, Default)]
pub struct Pace {
    /// Deezer: the earliest moment of the next request
    deezer: std::sync::Arc<tokio::sync::Mutex<Option<tokio::time::Instant>>>,
    /// Apple's iTunes Search API: the earliest moment of the next request
    itunes: std::sync::Arc<tokio::sync::Mutex<Option<tokio::time::Instant>>>,
    /// AcoustID: the earliest moment of the next request
    acoustid: std::sync::Arc<tokio::sync::Mutex<Option<tokio::time::Instant>>>,
}

/// Wait for the slot in `next`, then book the following one `gap` later.
async fn take_slot(next: &tokio::sync::Mutex<Option<tokio::time::Instant>>, gap: Duration) {
    let mut next = next.lock().await;
    if let Some(at) = *next {
        if at > tokio::time::Instant::now() {
            tokio::time::sleep_until(at).await;
        }
    }
    *next = Some(tokio::time::Instant::now() + gap);
}

/// Send `req` and read the answer as JSON. An HTTP error status is an error.
pub async fn json(req: reqwest::RequestBuilder) -> anyhow::Result<Value> {
    Ok(req.send().await?.error_for_status()?.json().await?)
}

/// [`json`] that reads the body of any answer, error statuses included (a service that explains an error
/// in JSON gives that JSON back).
pub async fn json_any_status(req: reqwest::RequestBuilder) -> anyhow::Result<Value> {
    Ok(req.send().await?.json().await?)
}

/// GET `url` with the query `q` and read the answer as JSON. An HTTP error status is an error.
pub async fn get_json(st: &impl Ctx, url: &str, q: &[(&str, String)]) -> anyhow::Result<Value> {
    Ok(st.http().get(url).query(q).send().await?.error_for_status()?.json().await?)
}
