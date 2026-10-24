//! The builtin plugins' adapters: Encore's own code for each catalog plugin, behind the plugin trait
//! (encore_plugins::Plugin) so the Plugins page, the picker and the request worker reach them through the
//! registry like any plugin.
//!
//! | Plugin | Encore's code it runs |
//! |---|---|
//! | ytdlp | encore_requests::sources::ytdlp (YouTube Music, links; the YouTube sign-in) |
//! | streamrip | encore_requests::sources::streamrip (Qobuz, Tidal with its sign-in, Deezer) |
//! | zotify | encore_requests::sources::zotify (Spotify: its two-step sign-in, search, downloads) |
//! | gamdl | encore_requests::sources::gamdl (Apple Music: search, downloads, the wrapper) |
//! | wrapper | encore_requests::wrapper (wrapper-v2: its container through the Docker socket, its Apple sign-in) |
//! | slskd | encore_requests::slskd_managed (the managed slskd) with the session supervisor (slskd_session) |
//! | torznab | encore_requests::sources::torznab (Prowlarr) |
//! | qbittorrent … usenet-blackhole | encore_requests::sources::dl (the download clients) |
//! | cadenza | encore_playback::dsp (server processing; `Engine::of` asks the registry for the program) |
//! | translator | encore_library::translate::llama (llama-server and the model) |
//! | jellyfin, plex, navidrome | the connections (encore_sync) and the media-server refresh |
//!
//! Installing, updating and removing are the registry's (the one installer); an adapter is told afterwards
//! (`installed`, `removing`, `enable`) so it restarts, stops or forgets what it runs. Searches answer in the
//! picker's row shape; zotify and gamdl answer in the SDK's shapes instead, as their picker tabs and request steps
//! go through the registry like a plugin program's (encore_plugins::catalog::THROUGH_REGISTRY). A yt-dlp or streamrip download is one run into the folder given; Soulseek and torrent or
//! Usenet downloads are watched by the request worker over minutes or days and are started there, not here.
//!
//! The adapters hold the server's state weakly: [`register`] gives back the one strong handle, which the
//! server keeps for as long as it runs (a test server for as long as it lives), so nothing keeps a state
//! alive in a loop.
use crate::routes::picker::{self, SearchBody};
use crate::state::AppState;
use encore_plugins::plugin::BoxFuture;
use encore_plugins::{Call, Code, Downloaded, Failure, FileEntry, Manifest, Plugin, Status};
use encore_requests::sources::dl::Client;
use encore_requests::sources::{gamdl, streamrip, ytdlp, zotify};
use encore_requests::wrapper;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Weak};

/// What a builtin plugin is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// yt-dlp
    Ytdlp,
    /// streamrip
    Streamrip,
    /// zotify (Spotify)
    Zotify,
    /// gamdl (Apple Music)
    Gamdl,
    /// the Apple Music wrapper gamdl decrypts through
    Wrapper,
    /// slskd (Soulseek)
    Slskd,
    /// the torrent and usenet indexers (Prowlarr)
    Torznab,
    /// a download client
    Client(Client),
    /// Cadenza, the processing engine
    Cadenza,
    /// the lyrics and title translator
    Translator,
    /// a media server connection (Jellyfin, Plex, Navidrome)
    Connection,
}

impl Kind {
    /// The kind of a catalog plugin id; `None` for one Encore has no adapter for.
    fn of(id: &str) -> Option<Kind> {
        Some(match id {
            "ytdlp" => Kind::Ytdlp,
            "streamrip" => Kind::Streamrip,
            "zotify" => Kind::Zotify,
            "gamdl" => Kind::Gamdl,
            "wrapper" => Kind::Wrapper,
            "slskd" => Kind::Slskd,
            "torznab" => Kind::Torznab,
            "cadenza" => Kind::Cadenza,
            "translator" => Kind::Translator,
            "jellyfin" | "plex" | "navidrome" => Kind::Connection,
            other => Kind::Client(Client::of_plugin(other)?),
        })
    }
}

/// One builtin plugin.
pub struct Builtin {
    /// its manifest
    m: Manifest,
    /// which builtin it is
    kind: Kind,
    /// the server, while it runs
    st: Weak<AppState>,
}

/// Register an adapter for every catalog plugin. Keep the answer while the server runs: the adapters reach the
/// server's state through it.
pub fn register(st: &AppState) -> Arc<AppState> {
    let anchor = Arc::new(st.clone());
    for m in encore_plugins::catalog::catalog() {
        if let Some(kind) = Kind::of(m.id()) {
            st.plugins.register(Arc::new(Builtin { m: m.clone(), kind, st: Arc::downgrade(&anchor) }));
        }
    }
    anchor
}

/// An error as a plugin failure: NotFound when something isn't installed, else Internal.
fn fail(e: anyhow::Error) -> Failure {
    let code = if e.to_string().contains("not installed") { Code::NotFound } else { Code::Internal };
    Failure::new(code, encore_core::fsx::explain(&e))
}

/// A section test's answer (`{ok, message}`) as the Test button's.
fn tested(v: Value) -> Result<String, Failure> {
    let m = v["message"].as_str().unwrap_or("").to_string();
    if v["ok"].as_bool().unwrap_or(false) { Ok(m) } else { Err(Failure::internal(if m.is_empty() { "The test failed".into() } else { m })) }
}

/// A status answer.
fn ready(message: impl Into<String>, signed_in: Option<bool>) -> Result<Status, Failure> {
    Ok(Status { message: message.into(), signed_in })
}

impl Builtin {
    /// The server's state; an error once it is stopping.
    fn state(&self) -> Result<Arc<AppState>, Failure> {
        self.st.upgrade().ok_or_else(|| Failure::internal("Encore is stopping"))
    }

    /// The settings section a builtin's Test runs (routes::system::run_test).
    fn section(&self) -> &'static str {
        match self.kind {
            Kind::Ytdlp => "ytdlp",
            Kind::Streamrip => "streamrip",
            Kind::Slskd => "slskd",
            Kind::Torznab => "torznab",
            Kind::Cadenza => "cadenza",
            _ => "",
        }
    }

    /// The status line of this builtin's card.
    async fn status_of(&self, st: &AppState) -> Result<Status, Failure> {
        match self.kind {
            Kind::Zotify => zotify::status(st).await,
            Kind::Gamdl => gamdl::status(st).await,
            Kind::Wrapper => wrapper::status(st).await,
            Kind::Ytdlp | Kind::Streamrip => {
                let id = self.m.id();
                let st2 = st.clone();
                let id2 = id.to_string();
                let found = tokio::task::spawn_blocking(move || {
                    let custom = (id2 == "streamrip").then(|| streamrip::binary(&st2)).flatten();
                    match custom.filter(|_| !st2.cfg.str("streamrip_bin").trim().is_empty()) {
                        Some(p) => Some((p, None, "your own copy")),
                        None => encore_system::tools::resolve(&st2, &id2).map(|(p, v, from)| {
                            let at = match from {
                                encore_system::tools::CopyFrom::Plugin => "installed by the plugin",
                                encore_system::tools::CopyFrom::Adopted => "kept from before plugins (config/tools/venv)",
                                encore_system::tools::CopyFrom::Bundled => "from the image",
                                encore_system::tools::CopyFrom::System => "the system's",
                            };
                            (p.to_string_lossy().into_owned(), v, at)
                        }),
                    }
                })
                .await
                .map_err(|e| Failure::internal(e.to_string()))?;
                let Some((_, v, at)) = found else {
                    return Err(Failure::new(Code::NotFound, format!("{} is not installed here – Update installs it", self.m.plugin.name)));
                };
                let line = format!("{} {} ({at})", self.m.plugin.name, v.unwrap_or_default()).replace("  ", " ");
                if self.kind == Kind::Ytdlp {
                    return ready(line, None);
                }
                let signed = streamrip::deezer_arl_set(st) || streamrip::tidal_signed_in(st) || streamrip::qobuz_login(st).is_some();
                let accounts: Vec<&str> = [("Deezer", streamrip::deezer_arl_set(st)), ("Tidal", streamrip::tidal_signed_in(st)), ("Qobuz", streamrip::qobuz_login(st).is_some())]
                    .iter().filter(|(_, on)| *on).map(|(n, _)| *n).collect();
                let who = if accounts.is_empty() { "no account signed in".to_string() } else { format!("signed in: {}", accounts.join(", ")) };
                ready(format!("{line} · {who}"), Some(signed))
            }
            Kind::Slskd => {
                let s = st.requests.slskd.status();
                let configured = encore_requests::slskd_managed::configured(st);
                let state = s["state"].as_str().unwrap_or("off");
                let ver = s["version"].as_str().map(|v| format!(" {v}")).unwrap_or_default();
                let line = match state {
                    "running" => format!("slskd{ver} runs"),
                    "starting" => format!("slskd{ver} is starting"),
                    "not_installed" => return Err(Failure::new(Code::NotFound, encore_requests::slskd_managed::NOT_INSTALLED)),
                    _ if !configured => "Switched on here; Soulseek needs a username and password (and to be on) to run".to_string(),
                    other => s["error"].as_str().map(String::from).unwrap_or_else(|| format!("slskd is {other}")),
                };
                ready(line, Some(configured))
            }
            Kind::Torznab => {
                if st.cfg.str("prowlarr_url").is_empty() || st.cfg.str("prowlarr_api_key").is_empty() {
                    return ready("Prowlarr needs its address and API key", Some(false));
                }
                let clients: Vec<&str> = [Client::torrent(st), Client::usenet(st)].into_iter().filter(|c| st.plugins.enabled(c.plugin_id()) && c.ready(st)).map(|c| c.name()).collect();
                let line = if clients.is_empty() { "Prowlarr is set up; no download client is ready".to_string() } else { format!("Prowlarr with {}", clients.join(" and ")) };
                ready(line, Some(true))
            }
            Kind::Client(c) => {
                let chosen = Client::torrent(st) == c || Client::usenet(st) == c;
                let line = match (chosen, c.ready(st)) {
                    (true, true) => format!("{} is the chosen client", c.name()),
                    (true, false) => format!("{} is chosen but not set up yet", c.name()),
                    (false, _) => format!("{} is not the chosen client (Torrents & Usenet)", c.name()),
                };
                ready(line, None)
            }
            Kind::Cadenza => {
                let st2 = st.clone();
                let e = tokio::task::spawn_blocking(move || st2.playback.cadenza.external_of(&st2.plugins)).await.map_err(|e| Failure::internal(e.to_string()))?;
                match e {
                    Some(p) => ready(format!("Cadenza {} (downloaded) runs server processing", p.version), None),
                    None => ready(format!("The engine built into Encore ({}) runs server processing", encore_playback::cadenza::builtin_cadenza_version()), None),
                }
            }
            Kind::Translator => {
                use encore_library::translate::llama;
                let m = llama::model(st);
                match llama::installed(st) {
                    None => Err(Failure::new(Code::NotFound, "llama.cpp is not installed here – Update installs it")),
                    Some((_, tag)) if llama::model_path(st, &m).is_file() => ready(format!("llama.cpp {tag} with the {} model", m.id), None),
                    Some((_, tag)) => ready(format!("llama.cpp {tag}; the {} model is not downloaded yet – Update downloads it", m.id), None),
                }
            }
            Kind::Connection => {
                let url = st.cfg.str(&format!("{}_url", self.m.id()));
                if url.trim().is_empty() {
                    ready(format!("{} needs its address (Settings → Integrations)", self.m.plugin.name), Some(false))
                } else {
                    ready(format!("{} at {}", self.m.plugin.name, url.trim()), Some(true))
                }
            }
        }
    }

    /// This builtin's Test: what it checked, in words.
    async fn test_of(&self, st: &AppState) -> Result<String, Failure> {
        match self.kind {
            Kind::Zotify => zotify::test(st).await,
            Kind::Gamdl => gamdl::test(st).await,
            Kind::Wrapper => wrapper::test(st).await,
            Kind::Client(c) => c.status(st).await.map_err(fail),
            Kind::Translator => {
                let m = self.m.clone();
                let (reg, st2) = (st.plugins.clone(), st.clone());
                tokio::task::spawn_blocking(move || {
                    let (bin, tag) = encore_library::translate::llama::installed(&st2).ok_or_else(|| Failure::new(Code::NotFound, "llama.cpp is not installed here"))?;
                    let p = encore_plugins::ProgramAt { version: tag.clone(), path: bin, adopted: false };
                    reg.program_runs(&m, &p).map_err(Failure::internal)?;
                    Ok(format!("llama.cpp {tag} runs"))
                })
                .await
                .map_err(|e| Failure::internal(e.to_string()))?
            }
            Kind::Connection => {
                let (id, url) = (self.m.id(), st.cfg.str(&format!("{}_url", self.m.id())).trim().trim_end_matches('/').to_string());
                if url.is_empty() {
                    return Err(Failure::new(Code::Auth, format!("{} needs its address (Settings → Integrations)", self.m.plugin.name)));
                }
                let req = match id {
                    "jellyfin" => st.http.get(format!("{url}/System/Info/Public")),
                    "plex" => st.http.get(format!("{url}/identity")).header("Accept", "application/json").header("X-Plex-Token", st.cfg.str("plex_token")),
                    _ => st.http.get(format!("{url}/rest/ping")).query(&[("v", "1.16.1"), ("c", "encore"), ("f", "json")]),
                };
                match req.timeout(std::time::Duration::from_secs(15)).send().await {
                    Ok(r) if r.status().is_success() => Ok(format!("{} answers at {url}", self.m.plugin.name)),
                    Ok(r) => Err(Failure::internal(format!("{} answered {}", self.m.plugin.name, r.status()))),
                    Err(e) => Err(Failure::new(Code::Network, format!("{} can't be reached: {e}", self.m.plugin.name))),
                }
            }
            _ => crate::routes::system::run_test(st, self.section(), None).await.map_err(fail).and_then(tested),
        }
    }

    /// A search in the picker's terms: `source` names the picker tab when a plugin has several (streamrip's
    /// qobuz, tidal, deezer).
    async fn search_of(&self, st: &AppState, request: &Value) -> Result<Vec<Value>, Failure> {
        // the SDK's request and results: the registry's path (sources::plugin) asks these two
        match self.kind {
            Kind::Zotify => return zotify::search(st, request).await,
            Kind::Gamdl => return gamdl::search(st, request).await,
            _ => {}
        }
        let source = match (self.kind, request["source"].as_str()) {
            (Kind::Ytdlp, _) => "ytmusic",
            (Kind::Slskd, _) => "slskd",
            (Kind::Torznab, _) => "torznab",
            (Kind::Streamrip, Some(s @ ("qobuz" | "tidal" | "deezer"))) => s,
            (Kind::Streamrip, _) => streamrip::services(st).first().map(String::as_str).and_then(|s| ["qobuz", "tidal", "deezer"].into_iter().find(|x| *x == s)).unwrap_or("deezer"),
            _ => return Err(Failure::unsupported(format!("{} does not search", self.m.plugin.name))),
        };
        let s = |k: &str| request[k].as_str().unwrap_or("").to_string();
        let b = SearchBody {
            source: source.into(),
            kind: if s("kind") == "album" { "album".into() } else { "track".into() },
            artist: s("artist"),
            title: if s("kind") == "album" && s("title").is_empty() { s("album") } else { s("title") },
            query: request["query"].as_str().map(String::from),
            album: request["album"].as_str().map(String::from),
            track_count: request["track_count"].as_u64().map(|n| n as usize),
            length: request["length"].as_f64(),
            ..Default::default()
        };
        let limit = request["limit"].as_u64().unwrap_or(50).clamp(1, 200) as usize;
        match picker::run_search(st, &b).await {
            Ok(a) => Ok(a.items.into_iter().take(limit).collect()),
            Err(e) => {
                let code = match e.status.as_u16() {
                    409 => Code::Auth,
                    429 => Code::RateLimited,
                    400 => Code::Unsupported,
                    _ => Code::Network,
                };
                Err(Failure::new(code, e.detail))
            }
        }
    }

    /// Download a request's link with this builtin into `folder`.
    async fn download_of(&self, st: &AppState, call: &Call, request: &Value, folder: &Path) -> Result<Downloaded, Failure> {
        let url = request["payload"]["url"].as_str().or(request["url"].as_str()).unwrap_or("").to_string();
        let entries = |files: Vec<std::path::PathBuf>| -> Vec<FileEntry> {
            files.into_iter().map(|path| FileEntry { size: std::fs::metadata(&path).ok().map(|m| m.len()), path, title: None, artist: None, album: None, track: None, disc: None, duration: None, format: None, isrc: None }).collect()
        };
        match self.kind {
            Kind::Zotify => zotify::download(st, call, request, folder).await,
            Kind::Gamdl => gamdl::download(st, call, request, folder).await,
            Kind::Ytdlp => {
                if !url.starts_with("http") {
                    return Err(Failure::new(Code::NotFound, "Nothing to download: the result has no link"));
                }
                let progress = call.progress.clone();
                let cb = move |pct: f64| {
                    if let Some(p) = &progress {
                        p(pct, None);
                    }
                };
                let playlist = request["kind"] == "album" || url.contains("playlist?list=");
                let dl = tokio::select! {
                    r = ytdlp::download(st, &url, folder, playlist, Some(&cb)) => r.map_err(fail)?,
                    _ = call.cancel.cancelled() => return Err(Failure::cancelled()),
                };
                Ok(Downloaded { files: entries(dl.files), source_url: Some(url), ..Default::default() })
            }
            Kind::Streamrip => {
                if !url.starts_with("http") {
                    return Err(Failure::new(Code::NotFound, "Nothing to download: the result has no link"));
                }
                let run = tokio::select! {
                    r = streamrip::download_run(st, &url, folder) => r.map_err(fail)?,
                    _ = call.cancel.cancelled() => return Err(Failure::cancelled()),
                };
                Ok(Downloaded { files: entries(run.files), source_url: Some(url), message: None, cover_url: None })
            }
            Kind::Slskd | Kind::Torznab => Err(Failure::unsupported(format!(
                "{} downloads run in the request queue, which watches them until the files arrive – pick the result there",
                self.m.plugin.name
            ))),
            _ => Err(Failure::unsupported(format!("{} does not download", self.m.plugin.name))),
        }
    }

    /// After an install, an update, a switch or before a removal: what the running code keeps is forgotten
    /// and the programs it runs are restarted or stopped.
    async fn refresh(&self, st: &AppState, removing: bool, on: bool) {
        st.system.tools.forget_versions();
        match self.kind {
            Kind::Cadenza => st.playback.cadenza.forget(),
            Kind::Slskd => {
                if removing || !on {
                    encore_requests::slskd_managed::stop(st).await;
                }
                st.requests.slskd.poke();
            }
            // removed: its container goes; switched off: it stops (Start brings it back)
            Kind::Wrapper if removing => wrapper::removing(st).await,
            Kind::Wrapper if !on && wrapper::mode(st) == wrapper::Mode::Managed => {
                if let Err(e) = wrapper::stop(st).await {
                    tracing::debug!("wrapper: {e}");
                }
            }
            Kind::Translator => {
                if removing || !on {
                    encore_library::translate::llama::stop(st).await;
                } else {
                    encore_library::translate::wake(st);
                }
            }
            _ => {}
        }
    }
}

impl Plugin for Builtin {
    fn manifest(&self) -> &Manifest {
        &self.m
    }

    fn status<'a>(&'a self, _call: &'a Call) -> BoxFuture<'a, Result<Status, Failure>> {
        Box::pin(async move {
            let st = self.state()?;
            self.status_of(&st).await
        })
    }

    fn test<'a>(&'a self, _call: &'a Call) -> BoxFuture<'a, Result<String, Failure>> {
        Box::pin(async move {
            let st = self.state()?;
            self.test_of(&st).await
        })
    }

    fn search<'a>(&'a self, _call: &'a Call, request: &'a Value) -> BoxFuture<'a, Result<Vec<Value>, Failure>> {
        Box::pin(async move {
            let st = self.state()?;
            self.search_of(&st, request).await
        })
    }

    fn download<'a>(&'a self, call: &'a Call, request: &'a Value, folder: &'a Path) -> BoxFuture<'a, Result<Downloaded, Failure>> {
        Box::pin(async move {
            let st = self.state()?;
            self.download_of(&st, call, request, folder).await
        })
    }

    fn installed<'a>(&'a self, _call: &'a Call, _version: Option<&'a str>) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            let st = self.state()?;
            self.refresh(&st, false, true).await;
            if self.kind == Kind::Translator {
                // a model chosen after the plugin's install files were fetched
                encore_library::translate::llama::ensure_model(&*st).await.map_err(fail)?;
            }
            Ok(())
        })
    }

    fn removing<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Ok(st) = self.state() {
                self.refresh(&st, true, false).await;
            }
        })
    }

    fn enable<'a>(&'a self, on: bool) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            let st = self.state()?;
            self.refresh(&st, false, on).await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_plugin_has_an_adapter() {
        for m in encore_plugins::catalog::catalog() {
            assert!(Kind::of(m.id()).is_some(), "{} has no adapter", m.id());
        }
        assert_eq!(Kind::of("qbittorrent"), Some(Kind::Client(Client::Qbit)));
        assert_eq!(Kind::of("usenet-blackhole"), Some(Kind::Client(Client::NzbHole)));
        assert_eq!(Kind::of("example-tone"), None);
    }
}
