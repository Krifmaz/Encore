use crate::auth::RateLimiter;
use encore_config::Config;
use encore_db::Db;
use encore_library::Library;
use encore_http::mb::Mb;
use encore_core::cache::{json_bytes, ByteCache};
pub(crate) use encore_jobs::Refreshed;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cfg: Config,
    pub http: reqwest::Client,
    /// the client that leaves redirects to the caller (an indexer's .torrent link that redirects to a magnet)
    pub http_no_redirects: reqwest::Client,
    /// where Tidal, Qobuz and YouTube Music are (the real hosts; a test server points them at stand-ins)
    pub urls: Arc<encore_http::Urls>,
    pub limits: Arc<RateLimiter>,
    pub static_dir: PathBuf,
    pub library: Library,
    pub mb: Mb,
    /// request pacing of the rate-limited services (Deezer, iTunes, AcoustID)
    pub pace: encore_http::Pace,
    /// answers of other services (Deezer, Last.fm, ListenBrainz …) and other slow lookups, each with its time
    /// to live, held to `REMOTE_CACHE_BYTES` (least recently used first out)
    pub cache: Arc<ByteCache<Value>>,
    /// the request queue's tasks and events, the download slots, slskd and the YouTube sign-in
    pub requests: encore_requests::Requests,
    /// when somebody last used the app (background passes wait while they do)
    pub busy: encore_jobs::Busy,
    /// progress of the long library jobs (Settings → Library)
    pub jobs: encore_jobs::Jobs,
    /// the scheduled tasks (Settings → Tasks)
    pub scheduler: encore_jobs::Scheduler,
    /// the audio-info answers per file and the quality check's ffmpeg slots
    pub media: encore_media::Media,
    /// processing slots and renders, the devices hub, resume spots, Roon
    pub playback: encore_playback::Playback,
    /// the WireGuard tunnel for Soulseek, its proxy and relay
    pub vpn: encore_vpn::Vpn,
    /// Encore's own updates, the restart, the download tools' versions and updates
    pub system: encore_system::System,
    /// what the API's own routes keep (list answers, slots of slow routes, Soulseek searches)
    pub web: Web,
}

/// Most bytes the answers of other services may take in memory (`AppState::cache`).
pub const REMOTE_CACHE_BYTES: usize = 32 << 20;
/// Most answers of other services kept.
const REMOTE_CACHE_ENTRIES: usize = 5000;
/// Most bytes the whole library list answers may take in memory (`Web::lists`).
pub const LIST_CACHE_BYTES: usize = 96 << 20;
/// Most list answers kept (each sort / filter / page is one).
const LIST_CACHE_ENTRIES: usize = 96;
/// Most bytes the share lists of Soulseek users browsed lately may take (`Web::soulseek_browsed`): a big
/// share is tens of megabytes as JSON, one bigger than this is not kept.
pub const BROWSE_CACHE_BYTES: usize = 32 << 20;
/// Most share lists kept.
const BROWSE_CACHE_ENTRIES: usize = 20;

/// What the API's own routes keep while the server runs. Clones share it.
#[derive(Clone)]
pub struct Web {
    /// whole library list answers by list key, stamped with the library version they were made at
    /// (routes::library::cached_list), held to `LIST_CACHE_BYTES`
    pub(crate) lists: Arc<ByteCache<Arc<Value>>>,
    /// share preview videos made at the same time (each runs ffmpeg for up to a few minutes)
    pub(crate) share_video_slots: Arc<tokio::sync::Semaphore>,
    /// one preview video build per link at a time: later requests for the link wait for it
    pub(crate) share_video_builds: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// silence detection of track edges: never more than two at once (each one decodes a whole file)
    pub(crate) track_edges: Arc<tokio::sync::Semaphore>,
    /// bug report uploads decoded at once: each may hold MAX_PIXEL_BYTES, so two at a time is what a small
    /// server can take
    pub(crate) decodes: Arc<tokio::sync::Semaphore>,
    /// the Soulseek page's searches by id
    pub(crate) soulseek_searches: Arc<Mutex<HashMap<String, crate::routes::soulseek::Session>>>,
    /// share lists of Soulseek users browsed lately (the folders, by user), each for
    /// `routes::soulseek::BROWSE_CACHE_SECS`, held to `BROWSE_CACHE_BYTES`
    pub(crate) soulseek_browsed: Arc<ByteCache<Arc<Vec<Value>>>>,
    /// the Storage page's disk usage and download folder sizes (`df`, folder walks), kept for a minute
    pub(crate) storage: Refreshed<(Option<Value>, u64, u64)>,
    /// what slskd says about itself (sign-in, shares, transfers) for the Soulseek status line, kept a few seconds
    pub(crate) slskd_probe: Refreshed<Value>,
    /// every API call's time, per route (crate::timing, /api/admin/perf)
    pub(crate) perf: crate::timing::Perf,
    /// held while Home's new releases are made with nothing kept yet (60 Deezer look-ups), so callers that
    /// arrive together wait for one answer instead of making one each
    pub(crate) new_releases: Arc<tokio::sync::Mutex<()>>,
}

/// How long the Storage page's folder sizes and disk usage are reused before they are worked out again (in
/// the background, while the last ones are shown).
pub(crate) const STORAGE_EVERY: Duration = Duration::from_secs(60);
/// How long slskd's own status is reused for the Soulseek status line.
pub(crate) const SLSKD_PROBE_EVERY: Duration = Duration::from_secs(5);

impl Default for Web {
    fn default() -> Self {
        Web {
            lists: Arc::new(ByteCache::new(LIST_CACHE_BYTES, LIST_CACHE_ENTRIES)),
            share_video_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            share_video_builds: Default::default(),
            track_edges: Arc::new(tokio::sync::Semaphore::new(2)),
            decodes: Arc::new(tokio::sync::Semaphore::new(2)),
            soulseek_searches: Default::default(),
            soulseek_browsed: Arc::new(ByteCache::new(BROWSE_CACHE_BYTES, BROWSE_CACHE_ENTRIES)),
            storage: Refreshed::new(STORAGE_EVERY),
            slskd_probe: Refreshed::new(SLSKD_PROBE_EVERY),
            perf: Default::default(),
            new_releases: Default::default(),
        }
    }
}

impl AppState {
    /// Opens (and migrates) the database in the config folder `data`, loads the settings (with the process's
    /// environment variables over them), and makes sure there is an owner account. `static_dir`: the web
    /// app's files.
    pub fn open(data: &std::path::Path, static_dir: PathBuf) -> anyhow::Result<AppState> {
        AppState::open_with(data, static_dir, encore_config::Config::load)
    }

    /// [`AppState::open`] with the settings loaded by `load` (a test server's: without the process's
    /// environment).
    pub fn open_with(data: &std::path::Path, static_dir: PathBuf, load: impl FnOnce(Db) -> anyhow::Result<Config>) -> anyhow::Result<AppState> {
        let db = Db::open(data)?;
        let cfg = load(db.clone())?;
        let contact = cfg.str("contact_email");
        let http = encore_http::client::build(&contact)?;
        let http_no_redirects = encore_http::client::no_redirects(&contact)?;
        let busy = encore_jobs::Busy::default();
        let jobs = encore_jobs::Jobs::default();
        let scheduler = encore_jobs::Scheduler::new(db.clone(), cfg.clone(), jobs.clone());
        let st = AppState {
            db, cfg, http, http_no_redirects, urls: Default::default(), limits: Arc::new(RateLimiter::default()), static_dir, library: Library::new(busy.clone()), mb: Default::default(),
            pace: Default::default(), cache: Arc::new(ByteCache::new(REMOTE_CACHE_BYTES, REMOTE_CACHE_ENTRIES)), requests: Default::default(), busy, jobs, scheduler, media: Default::default(),
            playback: Default::default(), vpn: Default::default(), system: Default::default(), web: Default::default(),
        };
        crate::auth::ensure_owner(&st)?;
        Ok(st)
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.db.data_dir
    }

    /// Runs `f` with the state on tokio's blocking pool: for a handler whose work needs more than the
    /// database (the artist index, the covers, cached lists) and reads it statement by statement. Work that
    /// is only SQL goes through `Db::read` / `Db::write` instead. A panic in `f` is a 500.
    pub async fn blocking<T, F>(&self, f: F) -> encore_core::error::ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&AppState) -> encore_core::error::ApiResult<T> + Send + 'static,
    {
        let st = self.clone();
        tokio::task::spawn_blocking(move || f(&st)).await.unwrap_or_else(|e| Err(encore_core::error::err(500, e.to_string())))
    }

    /// Small in-memory cache for slow lookups (Deezer, Last.fm …).
    pub fn cache_get(&self, key: &str) -> Option<Value> {
        self.cache.get(key)
    }
    pub fn cache_put(&self, key: &str, v: Value, secs: u64) {
        let bytes = key.len() + json_bytes(&v);
        self.cache.insert(key, v, bytes, Some(Duration::from_secs(secs)), 0);
    }
}

/// The external services (encore_http) take the server's client, settings, pacing and cache from here.
impl encore_http::Ctx for AppState {
    fn http(&self) -> &reqwest::Client {
        &self.http
    }
    fn http_no_redirects(&self) -> &reqwest::Client {
        &self.http_no_redirects
    }
    fn urls(&self) -> &encore_http::Urls {
        &self.urls
    }
    fn cfg(&self) -> &Config {
        &self.cfg
    }
    fn mb(&self) -> &Mb {
        &self.mb
    }
    fn pace(&self) -> &encore_http::Pace {
        &self.pace
    }
    fn cache_get(&self, key: &str) -> Option<Value> {
        AppState::cache_get(self, key)
    }
    fn cache_put(&self, key: &str, v: Value, secs: u64) {
        AppState::cache_put(self, key, v, secs)
    }
}

/// Covers, tags, audio info and the quality check (encore_media) take the database and the caches from here.
impl encore_media::Ctx for AppState {
    fn db(&self) -> &Db {
        &self.db
    }
    fn media(&self) -> &encore_media::Media {
        &self.media
    }
    fn covers(&self) -> &encore_media::CoverIndex {
        &self.library.covers
    }
}

/// The library (encore_library) takes its caches and job table from here, and tells the server when a song
/// leaves the library.
impl encore_library::Ctx for AppState {
    fn library(&self) -> &Library {
        &self.library
    }
    fn jobs(&self) -> &encore_jobs::Jobs {
        &self.jobs
    }
    fn track_removed(&self, id: i64) {
        encore_playback::dsp::render::forget_track(&self.db.data_dir, id);
    }
}

/// The sync (encore_sync) hands the server what needs its request queue and its link reader.
impl encore_sync::Ctx for AppState {
    fn request_missing(&self, owner: &encore_db::Row, playlist: i64, auto: bool, album_mode: Option<String>) -> anyhow::Result<encore_sync::MissingReport> {
        encore_requests::missing::request_missing(self, owner, playlist, auto, album_mode)
    }
    fn fetch_link(&self, url: &str, user: Option<i64>) -> impl std::future::Future<Output = anyhow::Result<Value>> + Send {
        encore_requests::sources::links::fetch_as(self, url, user)
    }
    fn spotify_api(&self, token: &str, kind: &str, id: &str) -> impl std::future::Future<Output = anyhow::Result<Value>> + Send {
        encore_requests::sources::links::spotify_api(self, token, kind, id)
    }
}

/// Playback (encore_playback) keeps its slots, renders, devices and Roon link here.
impl encore_playback::Ctx for AppState {
    fn playback(&self) -> &encore_playback::Playback {
        &self.playback
    }
}

/// Encore's updates and the download tools (encore_system) keep their state here; slskd and the translator,
/// installed and run by the request and library crates, reach the tools card through the helper calls.
impl encore_system::Ctx for AppState {
    fn system(&self) -> &encore_system::System {
        &self.system
    }
    fn helper_latest(&self, id: &str) -> impl std::future::Future<Output = anyhow::Result<(String, String)>> + Send {
        let (st, translator) = (self.clone(), id == "translator");
        async move {
            if translator { encore_library::translate::llama::latest(&st).await } else { encore_requests::slskd_managed::latest(&st).await }
        }
    }
    fn helper_install(&self, id: &str, tag: &str, url: &str) -> impl std::future::Future<Output = anyhow::Result<String>> + Send {
        let (st, translator, tag, url) = (self.clone(), id == "translator", tag.to_string(), url.to_string());
        async move {
            if translator {
                encore_library::translate::llama::install(&st, &tag, &url).await
            } else {
                encore_requests::slskd_managed::install(&st, &tag, &url).await.map(|_| tag)
            }
        }
    }
    fn helper_info(&self, id: &str, last: &Value) -> encore_system::ToolInfo {
        if id == "translator" {
            crate::translate::tool_info(self, last)
        } else {
            encore_system::tools::slskd_info(encore_requests::slskd_managed::installed_version(self), last)
        }
    }
    fn helper_reset(&self, id: &str) -> anyhow::Result<()> {
        if id == "translator" {
            encore_library::translate::llama::reset(self)
        } else {
            encore_requests::slskd_managed::reset(self);
            Ok(())
        }
    }
}

/// The request queue (encore_requests) keeps its tasks, events, download slots and helper programs here.
impl encore_requests::Ctx for AppState {
    fn requests(&self) -> &encore_requests::Requests {
        &self.requests
    }
    fn vpn(&self) -> &encore_vpn::Vpn {
        &self.vpn
    }
}
