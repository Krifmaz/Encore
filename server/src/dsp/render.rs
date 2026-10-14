//! Cadenza pre-rendering: each processed song is rendered once, to a complete FLAC file in
//! `<data dir>/cadenza-cache/renders/`, and then served like any other file (Range requests, native
//! seeking, no processing slot). While a render runs, listeners follow the growing `.part` file.
//!
//! * Key = sha256 of (track id, file mtime + size, output rate, filter, dither, taps limit, engine:
//!   built in or downloaded + its version + the admin's engine settings), so an engine update or a
//!   settings change renders again instead of serving old renders.
//! * A render is independent of the request that started it: it runs to completion unless the cache
//!   is turned off, cleared, or the user has more than [`USER_CAP`] renders waiting or running.
//! * Renders take a processing slot like live streams (`cadenza_max_streams`).
//! * `cadenza_cache_mb` caps the folder; least recently served songs go first.
use super::{spawn_chain, EngineRun, Job as LiveJob, Output, Slot, Slots};
use crate::error::{err, ApiResult};
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use cadenza_dsp::{CoefficientCache, DitherKind, FilterKind, InputFormat};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

/// Renders one user may have waiting or running (the current song and the next one).
pub const USER_CAP: usize = 2;

/// The rendered-song folder (beside the filter coefficient cache, so clearing one keeps the other).
pub fn renders_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cadenza-cache").join("renders")
}

/// The cache key for one rendering of one file.
/// `engine`: [`super::Engine::cache_tag`] (engine, version and settings).
#[allow(clippy::too_many_arguments)]
pub fn cache_key(track_id: i64, mtime_ns: i128, size: u64, rate: u32, filter: &str, dither: &str, max_taps: usize, engine: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!("encore-cadenza-render-2|{track_id}|{mtime_ns}|{size}|{rate}|{filter}|{dither}|{max_taps}|{engine}"));
    hex::encode(&h.finalize()[..16])
}

/// What to render: one library file with the settings of one request.
#[derive(Clone)]
pub struct Spec {
    pub track_id: i64,
    pub path: String,
    pub mtime_ns: i128,
    pub size: u64,
    pub input: InputFormat,
    pub rate: u32,
    pub filter: FilterKind,
    pub dither: DitherKind,
    pub max_taps: usize,
    /// the engine and its settings
    pub engine: EngineRun,
    /// [`super::Engine::cache_tag`]
    pub engine_tag: String,
}

impl Spec {
    /// The request's settings for a library file (rate / filter / dither ids as the player sends them).
    pub async fn new(st: &AppState, track_id: i64, path: String, rate: u32, filter: &str, dither: &str) -> Spec {
        let lim = super::Limits::of(st);
        let p2 = path.clone();
        let st2 = st.clone();
        let (info, meta, engine) = tokio::task::spawn_blocking(move || (crate::routes::library::audio_info(&p2), std::fs::metadata(&p2).ok(), super::Engine::of(&st2)))
            .await
            .unwrap_or_else(|_| (json!({}), None, super::Engine::from_parts(None, Default::default(), lim.max_taps)));
        let input = super::input_format(&info);
        let (mtime_ns, size) = meta
            .map(|m| (m.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i128).unwrap_or(0), m.len()))
            .unwrap_or((0, 0));
        Spec {
            track_id,
            path,
            mtime_ns,
            size,
            rate: super::target_rate(input.rate, rate, lim.max_rate),
            filter: super::choose_filter_or(filter, &engine.value("filter").unwrap_or_default(), lim.max_taps),
            dither: super::choose_dither_or(dither, &engine.value("dither").unwrap_or_default()),
            max_taps: lim.max_taps,
            input,
            engine: engine.run(),
            engine_tag: engine.cache_tag(),
        }
    }

    pub fn key(&self) -> String {
        cache_key(self.track_id, self.mtime_ns, self.size, self.rate, &self.filter.id(), self.dither.id(), self.max_taps, &self.engine_tag)
    }

    /// The same settings as a live stream starting at `seek` seconds.
    pub fn live(&self, seek: f64) -> LiveJob {
        LiveJob { path: self.path.clone(), input: self.input.clone(), seek, rate: self.rate, filter: self.filter, dither: self.dither, engine: self.engine.clone() }
    }
}

/// Where renders go and what limits them.
#[derive(Clone)]
pub struct Ctx {
    /// the renders folder
    pub dir: PathBuf,
    pub coeff: CoefficientCache,
    pub slots: &'static Slots,
    pub max_streams: usize,
    pub max_bytes: u64,
    /// false once the cache (or server processing) was turned off: running renders stop
    pub keep_going: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Ctx {
    /// None when the render cache is off (`cadenza_cache_mb` = 0): processed streams are live only.
    pub fn of(st: &AppState) -> Option<Ctx> {
        let mb = max_mb(st);
        if mb == 0 {
            return None;
        }
        let lim = super::Limits::of(st);
        let st2 = st.clone();
        Some(Ctx {
            dir: renders_dir(st.data_dir()),
            coeff: super::cache(st),
            slots: &super::SLOTS,
            max_streams: lim.max_streams,
            max_bytes: mb.saturating_mul(1024 * 1024),
            keep_going: Arc::new(move || st2.cfg.bool("cadenza_server_enabled") && st2.cfg.int("cadenza_cache_mb") > 0),
        })
    }

    fn part(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.part"))
    }
    fn flac(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.flac"))
    }
    fn meta(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// A slot within this context's limit, waiting up to 1.5 s like [`super::acquire_with_retry`].
    async fn slot_with_retry(&self) -> Option<Slot> {
        let mut slot = self.slots.try_acquire(self.max_streams);
        if self.max_streams > 0 {
            for _ in 0..30 {
                if slot.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                slot = self.slots.try_acquire(self.max_streams);
            }
        }
        slot
    }

    /// The limit for renders nobody is waiting on (ahead of time, after a seek, queued): one slot
    /// below the server's, so a playing request (a seek, a skip) always finds one free.
    fn background_max(&self) -> usize {
        if self.max_streams >= 2 {
            self.max_streams - 1
        } else {
            self.max_streams
        }
    }

    /// A slot for a render nobody is waiting on, when one is free below [`Ctx::background_max`].
    fn background_slot(&self) -> Option<Slot> {
        self.slots.try_acquire(self.background_max())
    }
}

pub fn max_mb(st: &AppState) -> u64 {
    st.cfg.int("cadenza_cache_mb").max(0) as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// waiting for a processing slot
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub phase: Phase,
    /// seconds of output audio handed to the encoder so far
    pub secs: f64,
    /// the signal path label once the pipeline is built
    pub label: String,
}

/// One render in progress (or waiting for a slot).
pub struct Render {
    pub key: String,
    pub user: i64,
    seq: u64,
    tx: watch::Sender<Status>,
    cancel: AtomicBool,
    followers: AtomicUsize,
    /// a slot handed over by a request that needs this render now
    gift: Mutex<Option<Slot>>,
}

impl Render {
    pub fn status(&self) -> Status {
        self.tx.borrow().clone()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    fn update(&self, f: impl FnOnce(&mut Status)) {
        self.tx.send_modify(f);
    }
}

/// Renders waiting or running, by key: one per key, shared by every request for it.
static JOBS: LazyLock<Mutex<HashMap<String, Arc<Render>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SEQ: AtomicU64 = AtomicU64::new(0);
/// Seconds of audio per wall-clock second of the last finished render.
static LAST_FACTOR: Mutex<Option<f64>> = Mutex::new(None);

/// What there is for a key right now.
pub enum Found {
    Cached(PathBuf),
    Rendering(Arc<Render>),
    Nothing,
}

fn find(ctx: &Ctx, jobs: &HashMap<String, Arc<Render>>, key: &str) -> Found {
    // a cancelled render is about to fail: never follow it (get_or_start replaces it)
    if let Some(j) = jobs.get(key) {
        if !j.cancel.load(Ordering::SeqCst) {
            return Found::Rendering(j.clone());
        }
    }
    let f = ctx.flac(key);
    if f.is_file() {
        return Found::Cached(f);
    }
    Found::Nothing
}

pub fn lookup(ctx: &Ctx, key: &str) -> Found {
    let jobs = JOBS.lock().unwrap();
    find(ctx, &jobs, key)
}

/// The render for `spec`: the cached file, the one already running, or a new one started with
/// `slot` (None: it waits for a slot). A slot that isn't needed is dropped.
pub fn get_or_start(ctx: &Ctx, spec: &Spec, user: i64, slot: Option<Slot>) -> (Found, bool) {
    let key = spec.key();
    let job = {
        let mut jobs = JOBS.lock().unwrap();
        match find(ctx, &jobs, &key) {
            Found::Nothing => {}
            other => return (other, false),
        }
        // a leftover from an interrupted (or cancelled) render must not be followed; a cancelled
        // job still in the table is replaced, and won't remove its successor's entry or file
        let _ = std::fs::remove_file(ctx.part(&key));
        let phase = if slot.is_some() { Phase::Running } else { Phase::Queued };
        let (tx, _) = watch::channel(Status { phase, secs: 0.0, label: String::new() });
        let job = Arc::new(Render {
            key: key.clone(),
            user,
            seq: SEQ.fetch_add(1, Ordering::SeqCst),
            tx,
            cancel: AtomicBool::new(false),
            followers: AtomicUsize::new(0),
            gift: Mutex::new(slot),
        });
        jobs.insert(key, job.clone());
        job
    };
    tokio::spawn(run(ctx.clone(), job.clone(), spec.clone()));
    enforce_user_cap(user);
    (Found::Rendering(job), true)
}

/// More than [`USER_CAP`] renders waiting or running for one user: stop the oldest ones nobody is
/// listening to (waiting ones first).
fn enforce_user_cap(user: i64) {
    let jobs = JOBS.lock().unwrap();
    let mut mine: Vec<&Arc<Render>> =
        jobs.values().filter(|j| j.user == user && !j.cancel.load(Ordering::SeqCst) && matches!(j.status().phase, Phase::Queued | Phase::Running)).collect();
    if mine.len() <= USER_CAP {
        return;
    }
    let newest = mine.iter().map(|j| j.seq).max().unwrap_or(0);
    mine.sort_by_key(|j| (j.status().phase != Phase::Queued, j.seq));
    let mut extra = mine.len() - USER_CAP;
    for j in mine {
        if extra == 0 {
            break;
        }
        if j.seq != newest && j.followers.load(Ordering::SeqCst) == 0 {
            tracing::info!("cadenza: stopping render {} (user {user} has more than {USER_CAP})", j.key);
            j.cancel();
            extra -= 1;
        }
    }
}

/// Run one render to completion: wait for a slot, decode → Cadenza → FLAC file, add a seek table,
/// rename `.part` → `.flac`, then trim the cache.
async fn run(ctx: Ctx, job: Arc<Render>, spec: Spec) {
    let key = job.key.clone();
    let ok = render(&ctx, &job, &spec).await;
    {
        let mut jobs = JOBS.lock().unwrap();
        // a cancelled job may have been replaced by a new render of the same key: leave that one alone
        let current = jobs.get(&key).is_some_and(|j| Arc::ptr_eq(j, &job));
        if !ok {
            if current {
                let _ = std::fs::remove_file(ctx.part(&key));
            }
            job.update(|s| s.phase = Phase::Failed);
        }
        if current {
            jobs.remove(&key);
        }
    }
    if ok {
        evict(&ctx.dir, ctx.max_bytes);
    }
}

async fn render(ctx: &Ctx, job: &Arc<Render>, spec: &Spec) -> bool {
    // wait for a slot (a request that needs this song now may hand one over); the phase changes
    // under the gift lock, so a gift can't arrive after the render took its own slot
    let slot = loop {
        if job.cancel.load(Ordering::SeqCst) || !(ctx.keep_going)() {
            return false;
        }
        {
            let mut gift = job.gift.lock().unwrap();
            if let Some(s) = gift.take().or_else(|| ctx.background_slot()) {
                job.update(|s| s.phase = Phase::Running);
                break s;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    if std::fs::create_dir_all(&ctx.dir).is_err() {
        return false;
    }
    let part = ctx.part(&job.key);
    let rate = spec.rate as f64;
    let started = Instant::now();
    let progress: super::Progress = {
        let job = job.clone();
        let keep = ctx.keep_going.clone();
        let mut last_check = Instant::now();
        let mut alive = true;
        Box::new(move |frames| {
            job.update(|s| s.secs = frames as f64 / rate);
            #[cfg(test)]
            std::thread::sleep(Duration::from_millis(tests::PACE_MS.load(Ordering::SeqCst)));
            if last_check.elapsed() > Duration::from_millis(500) {
                last_check = Instant::now();
                alive = keep();
            }
            alive && !job.cancel.load(Ordering::SeqCst)
        })
    };
    let mut chain = match spawn_chain(ctx.coeff.clone(), spec.live(0.0), Output::File(part.clone()), Some(progress)) {
        Ok(c) => c,
        Err(_) => return false,
    };
    match (&mut chain.ready).await {
        Ok(Ok(label)) => job.update(|s| s.label = label),
        _ => {
            tracing::warn!("cadenza: render {} could not start", job.key);
            return false;
        }
    }
    // the decoder (and a downloaded engine) end with the chain's task
    let complete = (&mut chain.done).await.unwrap_or(false);
    if !complete {
        let _ = chain.encoder.kill().await;
        return false;
    }
    // the encoder finishes the file (STREAMINFO) when its input ends
    let enc_ok = matches!(chain.encoder.wait().await, Ok(s) if s.success());
    drop(slot);
    if !enc_ok || job.cancel.load(Ordering::SeqCst) {
        return false;
    }
    let wall = started.elapsed().as_secs_f64().max(0.001);
    let secs = job.status().secs;
    let factor = secs / wall;
    let p2 = part.clone();
    let _ = tokio::task::spawn_blocking(move || add_seektable(&p2)).await;
    let meta = json!({
        "track_id": spec.track_id, "rate": spec.rate, "filter": spec.filter.id(), "dither": spec.dither.id(), "engine": spec.engine_tag,
        "label": job.status().label, "secs": (secs * 100.0).round() / 100.0, "factor": (factor * 100.0).round() / 100.0,
        "rendered_at": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    });
    // the downloaded engine failed and the built-in one rendered: its output isn't what this key
    // (the downloaded engine's version and settings) promises, so listeners get it but the cache
    // doesn't; the next play tries the downloaded engine again
    if chain.fell_back.load(Ordering::SeqCst) {
        let mut jobs = JOBS.lock().unwrap();
        if job.cancel.load(Ordering::SeqCst) {
            return false;
        }
        // followers keep reading their open handle of the unlinked file
        let _ = std::fs::remove_file(&part);
        job.update(|s| s.phase = Phase::Done);
        if jobs.get(&job.key).is_some_and(|j| Arc::ptr_eq(j, job)) {
            jobs.remove(&job.key);
        }
        tracing::info!("cadenza: track {} rendered by the built-in engine (the downloaded one failed) – not cached", spec.track_id);
        return true;
    }
    let _ = std::fs::write(ctx.meta(&job.key), meta.to_string());
    {
        // rename and leave the job table together, so a request never sees neither
        let _jobs = JOBS.lock().unwrap();
        if job.cancel.load(Ordering::SeqCst) || std::fs::rename(&part, ctx.flac(&job.key)).is_err() {
            return false;
        }
        job.update(|s| s.phase = Phase::Done);
    }
    *LAST_FACTOR.lock().unwrap() = Some(factor);
    tracing::info!("cadenza: rendered track {} ({:.0} s of audio at {} Hz) in {:.1} s – {:.1}× real time", spec.track_id, secs, spec.rate, wall, factor);
    true
}

// ---------------------------------------------------------------------------------------------
// serving

/// Keeps a render's follower count while a response reads it.
struct Follower(Arc<Render>);
impl Follower {
    fn new(job: Arc<Render>) -> Follower {
        job.followers.fetch_add(1, Ordering::SeqCst);
        Follower(job)
    }
}
impl Drop for Follower {
    fn drop(&mut self) {
        self.0.followers.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The growing `.part` file from byte 0: what's there, then more as the render writes it, until it
/// finishes (ends with an error when the render fails, so the player doesn't take a cut-off song as complete).
pub async fn follow(ctx: &Ctx, job: Arc<Render>) -> std::io::Result<impl futures_util::Stream<Item = std::io::Result<Bytes>> + Send + 'static> {
    let mut rx = job.tx.subscribe();
    let guard = Follower::new(job);
    // a render waiting for its slot has no file yet
    let deadline = Instant::now() + Duration::from_secs(60);
    let f = loop {
        let phase = rx.borrow_and_update().phase;
        if phase == Phase::Failed {
            return Err(std::io::Error::other("render stopped"));
        }
        if phase != Phase::Queued {
            if let Ok(f) = std::fs::File::open(ctx.part(&guard.0.key)).or_else(|_| std::fs::File::open(ctx.flac(&guard.0.key))) {
                break f;
            }
        }
        if Instant::now() > deadline {
            return Err(std::io::Error::other("render did not start"));
        }
        let _ = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
    };
    let file = tokio::fs::File::from_std(f);
    let state = (file, rx, guard, vec![0u8; 256 * 1024], false);
    Ok(futures_util::stream::unfold(state, |(mut file, mut rx, guard, mut buf, finished)| async move {
        loop {
            match file.read(&mut buf).await {
                Ok(0) => {}
                Ok(n) => return Some((Ok(Bytes::copy_from_slice(&buf[..n])), (file, rx, guard, buf, finished))),
                Err(e) => return Some((Err(e), (file, rx, guard, buf, true))),
            }
            if finished {
                return None;
            }
            let phase = rx.borrow_and_update().phase;
            match phase {
                // one more read picks up the last bytes
                Phase::Done => return follow_again(file, rx, guard, buf).await,
                Phase::Failed => {
                    return Some((Err(std::io::Error::other("render stopped")), (file, rx, guard, buf, true)));
                }
                _ => {
                    let _ = tokio::time::timeout(Duration::from_millis(250), rx.changed()).await;
                }
            }
        }
    }))
}

type FollowState = (tokio::fs::File, watch::Receiver<Status>, Follower, Vec<u8>, bool);

async fn follow_again(mut file: tokio::fs::File, rx: watch::Receiver<Status>, guard: Follower, mut buf: Vec<u8>) -> Option<(std::io::Result<Bytes>, FollowState)> {
    match file.read(&mut buf).await {
        Ok(0) => None,
        Ok(n) => Some((Ok(Bytes::copy_from_slice(&buf[..n])), (file, rx, guard, buf, false))),
        Err(e) => Some((Err(e), (file, rx, guard, buf, true))),
    }
}

fn no_cache(r: &mut Response) {
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
}

/// Serve a finished render as a plain file (Range / 206 / Content-Length), and mark it used.
async fn serve_cached(path: PathBuf, key: &str, ctx: &Ctx, spec: &Spec, req: Request) -> ApiResult<Response> {
    touch(&path);
    let label = std::fs::read_to_string(ctx.meta(key))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["label"].as_str().map(str::to_string))
        .unwrap_or_default();
    let mut r = crate::routes::library::serve_file(&path.to_string_lossy(), req, Some("audio/flac"), None).await?;
    super::stream_headers(r.headers_mut(), spec.rate, &label, "cached", 0.0);
    no_cache(&mut r);
    Ok(r)
}

/// A render in progress, from the start (`t` = 0) or from `t` seconds when the render is already
/// past it (decoded and re-encoded losslessly so the stream starts there, like a live seek).
async fn serve_following(ctx: &Ctx, job: Arc<Render>, spec: &Spec, t: f64) -> ApiResult<Response> {
    let status = job.status();
    let stream = follow(ctx, job).await.map_err(|e| err(500, format!("render: {e}")))?;
    let mut r = if t > 0.0 {
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.args(["-v", "error", "-nostdin", "-f", "flac", "-i", "pipe:0", "-ss", &format!("{t:.2}"), "-map", "0:a:0"]);
        cmd.args(["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24", "-compression_level", "0", "-f", "flac", "-flush_packets", "1", "pipe:1"]);
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| err(500, format!("ffmpeg: {e}")))?;
        let mut stdin = child.stdin.take().ok_or_else(|| err(500, "ffmpeg took no input"))?;
        let out = child.stdout.take().ok_or_else(|| err(500, "ffmpeg gave no output"))?;
        tokio::spawn(async move {
            let mut s = Box::pin(stream);
            while let Some(Ok(chunk)) = futures_util::StreamExt::next(&mut s).await {
                if stdin.write_all(&chunk).await.is_err() {
                    break;
                }
            }
        });
        let body = futures_util::StreamExt::map(tokio_util::io::ReaderStream::new(out), move |c| {
            let _keep = &child;
            c
        });
        ([(header::CONTENT_TYPE, "audio/flac")], Body::from_stream(body)).into_response()
    } else {
        ([(header::CONTENT_TYPE, "audio/flac")], Body::from_stream(stream)).into_response()
    };
    super::stream_headers(r.headers_mut(), spec.rate, &status.label, "rendering", t);
    no_cache(&mut r);
    Ok(r)
}

/// Whether a running render can serve a request starting at `t` (it has rendered past it).
fn reached(job: &Render, t: f64) -> bool {
    let s = job.status();
    t <= 0.0 || s.phase == Phase::Done || (s.phase == Phase::Running && s.secs >= t + 2.0)
}

/// A processed stream with the render cache on. `None`: play this request live (a render in
/// progress hasn't reached `t` yet); the caller then runs the live path.
pub async fn serve(ctx: &Ctx, spec: &Spec, user: i64, t: f64, req: Request) -> ApiResult<Option<Response>> {
    let key = spec.key();
    match lookup(ctx, &key) {
        Found::Cached(p) => return serve_cached(p, &key, ctx, spec, req).await.map(Some),
        Found::Rendering(job) => return serve_job(ctx, job, spec, t).await,
        Found::Nothing => {}
    }
    if t > 0.0 {
        // a seek into a song nobody rendered yet: the caller plays this request live (taking its
        // slot first) and then starts the render with [`start_background`]
        return Ok(None);
    }
    let slot = ctx.slot_with_retry().await.ok_or_else(|| err(503, "Server processing is busy – playing the original"))?;
    match get_or_start(ctx, spec, user, Some(slot)) {
        (Found::Cached(p), _) => serve_cached(p, &key, ctx, spec, req).await.map(Some),
        (Found::Rendering(job), _) => serve_job(ctx, job, spec, t).await,
        (Found::Nothing, _) => Err(err(500, "render did not start")),
    }
}

async fn serve_job(ctx: &Ctx, job: Arc<Render>, spec: &Spec, t: f64) -> ApiResult<Option<Response>> {
    if !reached(&job, t) {
        return Ok(None);
    }
    if job.status().phase == Phase::Queued && job.gift.lock().unwrap().is_none() {
        // waiting for a slot, but someone wants it now: hand it one
        let slot = ctx.slot_with_retry().await.ok_or_else(|| err(503, "Server processing is busy – playing the original"))?;
        let mut gift = job.gift.lock().unwrap();
        if job.status().phase == Phase::Queued && gift.is_none() {
            *gift = Some(slot);
        }
    }
    serve_following(ctx, job, spec, t).await.map(Some)
}

/// After a live seek took its slot: render the song in the background (with a slot below
/// [`Ctx::background_max`] if one is free, otherwise once one is), unless it is already rendered or rendering.
pub fn start_background(ctx: &Ctx, spec: &Spec, user: i64) {
    if matches!(lookup(ctx, &spec.key()), Found::Nothing) {
        get_or_start(ctx, spec, user, ctx.background_slot());
    }
}

/// No slot for a live seek: when a render of this song is running (faster than real time), wait
/// up to 15 s for it to reach `t` and follow it from there instead of answering 503.
pub async fn wait_for_render(ctx: &Ctx, spec: &Spec, t: f64) -> ApiResult<Option<Response>> {
    let Found::Rendering(job) = lookup(ctx, &spec.key()) else { return Ok(None) };
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut rx = job.tx.subscribe();
    loop {
        if job.cancel.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let s = rx.borrow_and_update().clone();
        match s.phase {
            Phase::Failed => return Ok(None),
            // a queued render has no slot either: it won't get anywhere in time
            Phase::Queued => return Ok(None),
            _ if reached(&job, t) => return serve_following(ctx, job, spec, t).await.map(Some),
            _ => {}
        }
        if Instant::now() > deadline {
            return Ok(None);
        }
        let _ = tokio::time::timeout(Duration::from_millis(250), rx.changed()).await;
    }
}

/// `POST /api/playback/dsp/prepare`: render ahead. Returns cached | rendering | queued | busy.
pub fn prepare(ctx: &Ctx, spec: &Spec, user: i64) -> Value {
    // ahead of time: never the last free slot, which a seek or skip in the playing song may need
    let state = match get_or_start(ctx, spec, user, ctx.background_slot()) {
        (Found::Cached(p), _) => {
            touch(&p);
            return json!({"state": "cached"});
        }
        (Found::Rendering(job), _) => job,
        (Found::Nothing, _) => return json!({"state": "busy"}),
    };
    let s = state.status();
    if state.cancel.load(Ordering::SeqCst) {
        return json!({"state": "busy"});
    }
    match s.phase {
        Phase::Queued => json!({"state": "queued"}),
        Phase::Running => json!({"state": "rendering", "rendered": (s.secs * 10.0).round() / 10.0}),
        Phase::Done => json!({"state": "cached"}),
        Phase::Failed => json!({"state": "busy"}),
    }
}

// ---------------------------------------------------------------------------------------------
// the folder

/// Mark a file as just used (LRU by modification time).
fn touch(p: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(p) {
        let _ = f.set_modified(SystemTime::now());
    }
}

struct Entry {
    key: String,
    size: u64,
    used: SystemTime,
    done: bool,
}

fn entries(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_str()?.to_string();
            if ext != "flac" && ext != "part" {
                return None;
            }
            let m = e.metadata().ok()?;
            Some(Entry { key: p.file_stem()?.to_string_lossy().into_owned(), size: m.len(), used: m.modified().unwrap_or(SystemTime::UNIX_EPOCH), done: ext == "flac" })
        })
        .collect()
}

fn remove_key(dir: &Path, key: &str) {
    let _ = std::fs::remove_file(dir.join(format!("{key}.flac")));
    let _ = std::fs::remove_file(dir.join(format!("{key}.json")));
}

/// Delete the least recently used finished renders until the folder fits `max_bytes`.
/// Returns how many were removed.
pub fn evict(dir: &Path, max_bytes: u64) -> usize {
    let mut all = entries(dir);
    let mut total: u64 = all.iter().map(|e| e.size).sum();
    all.retain(|e| e.done);
    all.sort_by_key(|e| e.used);
    let mut n = 0;
    for e in all {
        if total <= max_bytes {
            break;
        }
        remove_key(dir, &e.key);
        total = total.saturating_sub(e.size);
        n += 1;
        tracing::info!("cadenza: render cache full – removed {}", e.key);
    }
    n
}

/// Stop every render and empty the folder. Returns (files removed, MB freed).
pub fn clear(dir: &Path) -> (usize, f64) {
    for j in JOBS.lock().unwrap().values() {
        j.cancel();
    }
    let all = entries(dir);
    let bytes: u64 = all.iter().map(|e| e.size).sum();
    let files = all.iter().filter(|e| e.done).count();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    (files, mb(bytes))
}

/// A deleted or replaced track's renders.
pub fn forget_track(data_dir: &Path, track_id: i64) {
    let dir = renders_dir(data_dir);
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json") {
            let tid = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v["track_id"].as_i64());
            if tid == Some(track_id) {
                if let Some(k) = p.file_stem() {
                    remove_key(&dir, &k.to_string_lossy());
                }
            }
        }
    }
}

fn mb(bytes: u64) -> f64 {
    (bytes as f64 / 1_048_576.0 * 10.0).round() / 10.0
}

/// Seconds of audio per wall second of the last finished render (since start, else the newest on disk).
pub fn last_factor(dir: &Path) -> Option<f64> {
    if let Some(f) = *LAST_FACTOR.lock().unwrap() {
        return Some(f);
    }
    let newest = entries(dir).into_iter().filter(|e| e.done).max_by_key(|e| e.used)?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{}.json", newest.key))).ok()?).ok()?;
    v["factor"].as_f64()
}

/// `cache` in GET /api/playback/dsp: {used_mb, max_mb, files, rendering, last_factor}.
pub fn usage(st: &AppState) -> Value {
    let dir = renders_dir(st.data_dir());
    let all = entries(&dir);
    let rendering = JOBS.lock().unwrap().values().filter(|j| j.status().phase == Phase::Running).count();
    json!({
        "used_mb": mb(all.iter().map(|e| e.size).sum()),
        "max_mb": max_mb(st),
        "files": all.iter().filter(|e| e.done).count(),
        "rendering": rendering,
        "last_factor": last_factor(&dir).map(|f| (f * 10.0).round() / 10.0),
    })
}

/// For the admin Test: "render cache 812 MB of 4096 MB · 9 songs · renders at 6.2× real time".
pub fn usage_text(st: &AppState) -> String {
    let u = usage(st);
    if max_mb(st) == 0 {
        return "render cache off (live processing only)".to_string();
    }
    let mut s = format!("render cache {} MB of {} MB · {} song(s)", u["used_mb"].as_f64().unwrap_or(0.0).round(), u["max_mb"], u["files"]);
    if let Some(f) = u["last_factor"].as_f64() {
        s.push_str(&format!(" · renders at {f:.1}× real time"));
    }
    s
}

// ---------------------------------------------------------------------------------------------
// FLAC seek table: ffmpeg's FLAC muxer leaves an 8 KiB PADDING block but writes no SEEKTABLE;
// fill the padding with one (same size, so no audio moves) so browsers seek straight to a frame.

fn crc8(data: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in data {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

/// A frame header at the start of `b`: (frame or sample number, bytes 2–3 of the header).
fn frame_header(b: &[u8]) -> Option<(u64, [u8; 2])> {
    if b.len() < 16 || b[0] != 0xFF || b[1] & 0xFE != 0xF8 {
        return None;
    }
    let (bs, sr, ch, ss) = (b[2] >> 4, b[2] & 0x0F, b[3] >> 4, (b[3] >> 1) & 7);
    if bs == 0 || sr == 15 || ch > 10 || ss == 3 || b[3] & 1 != 0 {
        return None;
    }
    let lead = b[4].leading_ones() as usize;
    let len = match lead {
        0 => 1,
        2..=7 => lead,
        _ => return None,
    };
    let mut n: u64 = if len == 1 { b[4] as u64 } else { (b[4] & (0x7F >> lead)) as u64 };
    for i in 1..len {
        let c = b[4 + i];
        if c & 0xC0 != 0x80 {
            return None;
        }
        n = (n << 6) | (c & 0x3F) as u64;
    }
    let mut end = 4 + len;
    end += match bs {
        6 => 1,
        7 => 2,
        _ => 0,
    };
    end += match sr {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    };
    if end >= b.len() || crc8(&b[..end]) != b[end] {
        return None;
    }
    Some((n, [b[2], b[3]]))
}

fn channels_of(code: u8) -> u8 {
    if code <= 7 {
        code + 1
    } else {
        2
    }
}

/// Write a SEEKTABLE into the PADDING block of a FLAC file. Ok(points) – 0 when there was no room.
pub fn add_seektable(path: &Path) -> std::io::Result<usize> {
    let mut f = std::fs::File::options().read(true).write(true).open(path)?;
    let file_len = f.metadata()?.len();
    let mut head = vec![0u8; 64 * 1024];
    let n = read_at(&mut f, 0, &mut head)?;
    head.truncate(n);
    if head.len() < 42 || &head[..4] != b"fLaC" {
        return Err(std::io::Error::other("not FLAC"));
    }
    // metadata blocks
    let mut i = 4usize;
    let mut padding: Option<(usize, usize, bool)> = None; // (offset, length, last)
    let (mut min_bs, mut max_bs, mut total) = (0u64, 0u64, 0u64);
    loop {
        if i + 4 > head.len() {
            return Err(std::io::Error::other("metadata too long"));
        }
        let (h, ln) = (head[i], u32::from_be_bytes([0, head[i + 1], head[i + 2], head[i + 3]]) as usize);
        match h & 0x7F {
            0 => {
                let s = &head[i + 4..i + 4 + 34];
                min_bs = u16::from_be_bytes([s[0], s[1]]) as u64;
                max_bs = u16::from_be_bytes([s[2], s[3]]) as u64;
                total = u64::from_be_bytes([0, 0, 0, s[13] & 0x0F, s[14], s[15], s[16], s[17]]);
            }
            1 => padding = Some((i, ln, h & 0x80 != 0)),
            3 => return Ok(0), // already has one
            _ => {}
        }
        i += 4 + ln;
        if h & 0x80 != 0 {
            break;
        }
    }
    let first_frame = i as u64;
    let Some((pad_at, pad_len, pad_last)) = padding else { return Ok(0) };
    if min_bs != max_bs || max_bs == 0 || total == 0 || pad_len < 18 {
        return Ok(0);
    }
    // how many 18-byte points fit (the rest stays padding, which needs a 4-byte header)
    let mut slots = pad_len / 18;
    let mut rest = pad_len - slots * 18;
    if rest > 0 && rest < 4 {
        slots -= 1;
        rest += 18;
    }
    if slots == 0 {
        return Ok(0);
    }
    let first = {
        let mut b = [0u8; 16];
        read_at(&mut f, first_frame, &mut b)?;
        frame_header(&b).ok_or_else(|| std::io::Error::other("no first frame"))?.1
    };
    let frames = total.div_ceil(max_bs);
    let audio = file_len - first_frame;
    // one point every second or so, as many as fit
    let want = (total / 44_100).clamp(1, slots as u64) as usize;
    let mut points: Vec<(u64, u64, u16)> = Vec::with_capacity(want);
    let mut win = vec![0u8; 128 * 1024];
    for k in 0..want as u64 {
        let target = total * k / want as u64;
        let est = first_frame + (audio as u128 * target as u128 / total as u128) as u64;
        let at = est.saturating_sub(32 * 1024).max(first_frame);
        let got = read_at(&mut f, at, &mut win)?;
        let w = &win[..got];
        let mut j = 0;
        while j + 16 <= w.len() {
            if w[j] == 0xFF {
                if let Some((num, hb)) = frame_header(&w[j..]) {
                    if hb[0] == first[0] && channels_of(hb[1] >> 4) == channels_of(first[1] >> 4) && hb[1] & 0x0E == first[1] & 0x0E && num < frames {
                        let sample = num * max_bs;
                        let off = at + j as u64 - first_frame;
                        if points.last().is_none_or(|p| p.0 < sample && p.1 < off) && (k > 0 || sample == 0) {
                            points.push((sample, off, (total - sample).min(max_bs) as u16));
                        }
                        break;
                    }
                }
            }
            j += 1;
        }
    }
    let mut block = Vec::with_capacity(pad_len + 4);
    let table_len = slots * 18;
    block.push(if rest == 0 && pad_last { 0x83 } else { 0x03 });
    block.extend_from_slice(&(table_len as u32).to_be_bytes()[1..]);
    for p in &points {
        block.extend_from_slice(&p.0.to_be_bytes());
        block.extend_from_slice(&p.1.to_be_bytes());
        block.extend_from_slice(&p.2.to_be_bytes());
    }
    for _ in points.len()..slots {
        // placeholder points
        block.extend_from_slice(&u64::MAX.to_be_bytes());
        block.extend_from_slice(&[0u8; 10]);
    }
    if rest > 0 {
        block.push(if pad_last { 0x81 } else { 0x01 });
        block.extend_from_slice(&((rest - 4) as u32).to_be_bytes()[1..]);
        block.extend(std::iter::repeat_n(0u8, rest - 4));
    }
    debug_assert_eq!(block.len(), pad_len + 4);
    f.seek(SeekFrom::Start(pad_at as u64))?;
    f.write_all(&block)?;
    Ok(points.len())
}

fn read_at(f: &mut std::fs::File, at: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    f.seek(SeekFrom::Start(at))?;
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// milliseconds each processing block waits, so a render in a test lasts long enough to join
    pub static PACE_MS: AtomicU64 = AtomicU64::new(0);

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("encore-render-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn have_ffmpeg() -> bool {
        std::process::Command::new("ffmpeg").arg("-version").stdout(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
    }

    #[test]
    fn key_changes_with_settings() {
        const E: &str = "bundled-0.1.0|";
        let base = cache_key(7, 1_000, 5_000, 96_000, "sinc-l-64k", "tpdf", 65_536, E);
        assert_eq!(base, cache_key(7, 1_000, 5_000, 96_000, "sinc-l-64k", "tpdf", 65_536, E));
        for other in [
            cache_key(8, 1_000, 5_000, 96_000, "sinc-l-64k", "tpdf", 65_536, E),
            cache_key(7, 1_001, 5_000, 96_000, "sinc-l-64k", "tpdf", 65_536, E),
            cache_key(7, 1_000, 5_001, 96_000, "sinc-l-64k", "tpdf", 65_536, E),
            cache_key(7, 1_000, 5_000, 192_000, "sinc-l-64k", "tpdf", 65_536, E),
            cache_key(7, 1_000, 5_000, 96_000, "sinc-m-64k", "tpdf", 65_536, E),
            cache_key(7, 1_000, 5_000, 96_000, "sinc-l-64k", "lipshitz", 65_536, E),
            cache_key(7, 1_000, 5_000, 96_000, "sinc-l-64k", "tpdf", 16_384, E),
            // an engine update, or other engine settings, render again
            cache_key(7, 1_000, 5_000, 96_000, "sinc-l-64k", "tpdf", 65_536, "updated-2.1.0|"),
            cache_key(7, 1_000, 5_000, 96_000, "sinc-l-64k", "tpdf", 65_536, "updated-2.1.0|gpu=off"),
            cache_key(7, 1_000, 5_000, 96_000, "sinc-l-64k", "tpdf", 65_536, "bundled-0.1.0|headroom_db=-3"),
        ] {
            assert_ne!(base, other);
        }
        assert!(base.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn eviction_removes_least_recently_used() {
        let d = tmp("evict");
        let now = SystemTime::now();
        for (i, k) in ["old", "mid", "new"].iter().enumerate() {
            let p = d.join(format!("{k}.flac"));
            std::fs::write(&p, vec![0u8; 1000]).unwrap();
            std::fs::write(d.join(format!("{k}.json")), "{}").unwrap();
            std::fs::File::options().append(true).open(&p).unwrap().set_modified(now - Duration::from_secs(300 - i as u64 * 100)).unwrap();
        }
        // serving "old" makes it the most recent
        touch(&d.join("old.flac"));
        assert_eq!(evict(&d, 3000), 0);
        assert_eq!(evict(&d, 2500), 1);
        assert!(!d.join("mid.flac").exists() && !d.join("mid.json").exists());
        assert!(d.join("old.flac").exists() && d.join("new.flac").exists());
        assert_eq!(evict(&d, 1000), 1);
        assert!(d.join("old.flac").exists() && !d.join("new.flac").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn seektable_fills_padding() {
        if !have_ffmpeg() {
            return;
        }
        let d = tmp("seektable");
        let f = d.join("x.part");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i", "sine=f=440:d=20:sample_rate=96000", "-ac", "2", "-c:a", "flac", "-sample_fmt", "s32", "-compression_level", "0", "-f", "flac", "-y"])
            .arg(&f)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let before = std::fs::metadata(&f).unwrap().len();
        let n = add_seektable(&f).unwrap();
        assert!(n >= 15, "{n} points");
        assert_eq!(std::fs::metadata(&f).unwrap().len(), before);
        assert_eq!(add_seektable(&f).unwrap(), 0); // already there
        // still a valid FLAC that decodes completely
        let out = std::process::Command::new("ffmpeg").args(["-v", "error", "-nostdin", "-i"]).arg(&f).args(["-f", "null", "-"]).output().unwrap();
        assert!(out.status.success() && out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
        let _ = std::fs::remove_dir_all(&d);
    }

    static TEST_SLOTS: Slots = Slots::new();

    fn test_ctx(d: &Path) -> Ctx {
        Ctx {
            dir: d.join("renders"),
            coeff: CoefficientCache::in_dir(d.join("coeff")),
            slots: &TEST_SLOTS,
            max_streams: 4,
            max_bytes: 1 << 30,
            keep_going: Arc::new(|| true),
        }
    }

    fn test_spec(d: &Path, id: i64) -> Spec {
        let src = d.join(format!("src{id}.wav"));
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i", "sine=f=440:d=6:sample_rate=44100", "-ac", "2", "-y"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        Spec {
            track_id: id,
            path: src.to_string_lossy().into_owned(),
            mtime_ns: 1,
            size: 2,
            input: InputFormat { rate: 44_100, bits: 16, channels: 2, codec: "PCM".into() },
            rate: 88_200,
            filter: super::super::choose_filter("", 4096),
            dither: DitherKind::Tpdf,
            max_taps: 4096,
            engine: EngineRun::default(),
            engine_tag: "bundled-test|".into(),
        }
    }

    async fn collect(r: Response) -> Vec<u8> {
        axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shared_render_then_cached_range() {
        if !have_ffmpeg() {
            return;
        }
        let d = tmp("shared");
        let ctx = test_ctx(&d);
        let spec = test_spec(&d, 990_001);
        PACE_MS.store(20, Ordering::SeqCst);
        let req = || Request::builder().uri("/x").body(Body::empty()).unwrap();
        // two requests for the same key while it renders: one job, one slot
        let a = serve(&ctx, &spec, 1, 0.0, req()).await.unwrap().expect("served");
        assert_eq!(a.headers()["x-encore-dsp"], "rendering");
        let b = serve(&ctx, &spec, 2, 0.0, req()).await.unwrap().expect("served");
        assert_eq!(b.headers()["x-encore-dsp"], "rendering");
        assert_eq!(TEST_SLOTS.active(), 1);
        assert_eq!(JOBS.lock().unwrap().keys().filter(|k| **k == spec.key()).count(), 1);
        let (ba, bb) = tokio::join!(collect(a), collect(b));
        PACE_MS.store(0, Ordering::SeqCst);
        assert!(ba.len() > 100_000 && ba == bb && &ba[..4] == b"fLaC");
        // finished: cached, served as a file with Range support
        for _ in 0..100 {
            if ctx.flac(&spec.key()).is_file() && JOBS.lock().unwrap().get(&spec.key()).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(TEST_SLOTS.active(), 0);
        let r = Request::builder().uri("/x").header("range", "bytes=0-131071").body(Body::empty()).unwrap();
        let c = serve(&ctx, &spec, 1, 30.0, r).await.unwrap().expect("served");
        assert_eq!(c.status(), 206);
        assert_eq!(c.headers()["x-encore-dsp"], "cached");
        assert_eq!(c.headers()["content-length"], "131072");
        assert!(c.headers()["content-range"].to_str().unwrap().starts_with("bytes 0-131071/"));
        // same audio (the header differs: final STREAMINFO and the seek table)
        assert_eq!(collect(c).await[9_000..], ba[9_000..131_072]);
        // the stored file got its seek table and decodes to the full length
        let out = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
            .arg(ctx.flac(&spec.key()))
            .output()
            .unwrap();
        let dur: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert!((dur - 6.0).abs() < 0.1, "{dur}");
        assert!(prepare(&ctx, &spec, 1)["state"] == "cached");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn user_cap_stops_oldest_waiting_render() {
        if !have_ffmpeg() {
            return;
        }
        static NO_SLOTS: Slots = Slots::new();
        let d = tmp("cap");
        let ctx = Ctx { slots: &NO_SLOTS, max_streams: 0, ..test_ctx(&d) };
        let specs: Vec<Spec> = (0..3).map(|i| test_spec(&d, 990_100 + i)).collect();
        let states: Vec<Value> = specs.iter().map(|s| prepare(&ctx, s, 42)).collect();
        assert!(states.iter().all(|s| s["state"] == "queued"), "{states:?}");
        tokio::time::sleep(Duration::from_millis(600)).await;
        let jobs = JOBS.lock().unwrap();
        assert!(jobs.get(&specs[0].key()).is_none(), "oldest stopped");
        assert!(jobs.get(&specs[1].key()).is_some() && jobs.get(&specs[2].key()).is_some());
        drop(jobs);
        for s in &specs {
            if let Some(j) = JOBS.lock().unwrap().get(&s.key()) {
                j.cancel();
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn render_ahead_leaves_a_slot_for_playing_requests() {
        if !have_ffmpeg() {
            return;
        }
        static TWO: Slots = Slots::new();
        let d = tmp("reserve");
        let ctx = Ctx { slots: &TWO, max_streams: 2, ..test_ctx(&d) };
        // the playing song holds one slot: rendering the next one ahead must not take the last
        let playing = TWO.try_acquire(2).expect("slot");
        let next = test_spec(&d, 990_200);
        assert_eq!(prepare(&ctx, &next, 7)["state"], "queued");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(TWO.active(), 1, "the queued render left the last slot free");
        assert_eq!(JOBS.lock().unwrap().get(&next.key()).unwrap().status().phase, Phase::Queued);
        // a cancelled render is never followed: a new request starts a fresh one in its place
        let old = match lookup(&ctx, &next.key()) {
            Found::Rendering(j) => j,
            _ => panic!("rendering"),
        };
        old.cancel();
        assert!(matches!(lookup(&ctx, &next.key()), Found::Nothing));
        let (fresh, started) = get_or_start(&ctx, &next, 7, None);
        assert!(started);
        let Found::Rendering(fresh) = fresh else { panic!("rendering") };
        assert!(!Arc::ptr_eq(&old, &fresh));
        // the old job ends without removing its replacement
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(JOBS.lock().unwrap().get(&next.key()).is_some_and(|j| Arc::ptr_eq(j, &fresh)));
        // the playing song's slot frees: the queued render may now run
        drop(playing);
        for _ in 0..100 {
            if ctx.flac(&next.key()).is_file() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(ctx.flac(&next.key()).is_file());
        assert!(TWO.active() <= 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
