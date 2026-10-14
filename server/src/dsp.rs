//! Server-side Cadenza processing: library streams upsampled on this server by cadenza-dsp
//! (long-filter upsampling, dither / noise shaping) and sent as lossless 24-bit FLAC.
//!
//! The engine is the cadenza-dsp compiled into Encore ("built in"), or – once an admin updated it under
//! Download tools – the downloaded `cadenza render` program (tools.rs), run as the middle stage between
//! the two ffmpeg processes. A downloaded engine that fails before it produced any audio falls back to
//! the built-in one for that render. The admin's engine settings (`cadenza_engine_settings`) are drawn
//! from the engine's own schema (`cadenza render --describe`).
//!
//! Off by default (`cadenza_server_enabled`); users also need the `server_dsp` permission. A
//! server-wide limit (`cadenza_max_streams`) keeps the processor from being swamped: every processed
//! stream holds a [`Slot`] until its response body ends or is dropped.
use crate::error::{err, ApiResult};
use crate::state::AppState;
use axum::body::Body;
use axum::http::{header, HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use cadenza_dsp::{CoefficientCache, DitherKind, FilterKind, InputFormat, Pipeline, PipelineConfig};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub mod render;

/// A server-wide count of running Cadenza jobs (live streams and cache renders share it).
pub struct Slots(AtomicUsize);

impl Slots {
    pub const fn new() -> Slots {
        Slots(AtomicUsize::new(0))
    }

    /// Take a slot when fewer than `max` jobs are running.
    pub fn try_acquire(&'static self, max: usize) -> Option<Slot> {
        self.0.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < max).then_some(n + 1)).ok().map(|_| Slot(self))
    }

    pub fn active(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// Processed streams and renders running right now, whole server.
static SLOTS: Slots = Slots::new();

/// One job's place in the server-wide limit; frees it when dropped.
pub struct Slot(&'static Slots);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Take a slot when fewer than `max` processed streams are running.
pub fn try_acquire(max: usize) -> Option<Slot> {
    SLOTS.try_acquire(max)
}

/// [`try_acquire`], waiting up to 1.5 s: a seek or track change often arrives before the old
/// connection's drop has freed its slot.
pub async fn acquire_with_retry(max: usize) -> Option<Slot> {
    let mut slot = try_acquire(max);
    if max > 0 {
        for _ in 0..30 {
            if slot.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            slot = try_acquire(max);
        }
    }
    slot
}

pub fn active() -> usize {
    SLOTS.active()
}

/// The admin's limits (Settings → Apps & sources → Server processing).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub enabled: bool,
    pub max_streams: usize,
    pub max_rate: u32,
    pub max_taps: usize,
}

impl Limits {
    pub fn of(st: &AppState) -> Limits {
        let rate = st.cfg.int("cadenza_max_rate");
        let taps = st.cfg.int("cadenza_max_taps");
        Limits {
            enabled: st.cfg.bool("cadenza_server_enabled"),
            max_streams: st.cfg.int("cadenza_max_streams").max(0) as usize,
            max_rate: if rate <= 0 { 192_000 } else { rate.min(u32::MAX as i64) as u32 },
            max_taps: if taps <= 0 { 65_536 } else { taps as usize },
        }
    }
}

pub fn cache(st: &AppState) -> CoefficientCache {
    CoefficientCache::in_dir(st.data_dir().join("cadenza-cache"))
}

/// The output rate: the requested rate (capped at `max_rate`), or with `requested == 0` the highest
/// ×2, ×4 … multiple of the source rate the server allows. Never below the source rate.
pub fn target_rate(src: u32, requested: u32, max_rate: u32) -> u32 {
    if requested > 0 {
        return requested.min(max_rate).max(src);
    }
    let mut r = src;
    while let Some(next) = r.checked_mul(2) {
        if next > max_rate {
            break;
        }
        r = next;
    }
    r
}

/// `kind` when it fits `max_taps`; otherwise the longest catalogue filter of the same family,
/// phase and apodizing kind that does (or the shortest catalogue filter of that kind when none does).
pub fn cap_filter(kind: FilterKind, max_taps: usize) -> FilterKind {
    if kind.taps <= max_taps {
        return kind;
    }
    let same: Vec<FilterKind> = FilterKind::catalog().into_iter().filter(|f| f.family == kind.family && f.phase == kind.phase && f.apodizing == kind.apodizing).collect();
    same.iter()
        .filter(|f| f.taps <= max_taps)
        .max_by_key(|f| f.taps)
        .or_else(|| same.iter().min_by_key(|f| f.taps))
        .copied()
        .or_else(|| FilterKind::catalog().into_iter().min_by_key(|f| f.taps))
        .unwrap_or(kind)
}

/// The filter used when the user didn't pick one.
pub fn default_filter(max_taps: usize) -> FilterKind {
    cap_filter(FilterKind::default(), max_taps)
}

/// The filter for a request: the parsed id (else the default), capped.
#[cfg(test)]
pub fn choose_filter(id: &str, max_taps: usize) -> FilterKind {
    choose_filter_or(id, "", max_taps)
}

/// The filter for a request: the parsed id, else the admin's default (`fallback`), else Cadenza's; capped.
pub fn choose_filter_or(id: &str, fallback: &str, max_taps: usize) -> FilterKind {
    cap_filter(FilterKind::parse(id).or_else(|| FilterKind::parse(fallback)).unwrap_or_default(), max_taps)
}

#[cfg(test)]
pub fn choose_dither(id: &str) -> DitherKind {
    choose_dither_or(id, "")
}

/// The dither for a request: the parsed id, else the admin's default, else TPDF.
pub fn choose_dither_or(id: &str, fallback: &str) -> DitherKind {
    DitherKind::parse(id).or_else(|| DitherKind::parse(fallback)).unwrap_or(DitherKind::Tpdf)
}

/// Catalogue filters the server allows.
pub fn filters(max_taps: usize) -> Vec<FilterKind> {
    FilterKind::catalog().into_iter().filter(|f| f.taps <= max_taps).collect()
}

/// Header-safe (ASCII) version of a signal path label.
pub fn ascii_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '→' | '⟶' | '➔' | '➜' => out.push_str("->"),
            '·' | '–' | '—' => out.push('-'),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            _ => {}
        }
    }
    out
}

/// What the source file is, from `audio_info` (unknown rate → 44.1 kHz, channels default 2).
pub fn input_format(info: &Value) -> InputFormat {
    let rate = info["sample_rate"].as_u64().filter(|r| *r > 0).unwrap_or(44_100) as u32;
    let channels = info["channels"].as_u64().filter(|c| *c > 0).unwrap_or(2) as u32;
    let bits = info["bit_depth"].as_u64().filter(|b| *b > 0).unwrap_or(16) as u32;
    let codec = info["codec"].as_str().filter(|c| !c.is_empty()).unwrap_or("audio").to_uppercase();
    InputFormat { rate, bits, channels, codec }
}

/// A downloaded Cadenza program a job runs on.
#[derive(Clone, Debug, PartialEq)]
pub struct External {
    pub path: PathBuf,
    pub version: String,
}

/// How one job is processed: on which engine, with which engine settings.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineRun {
    /// None: the built-in engine
    pub external: Option<External>,
    /// `--set key=value` for the downloaded engine: the admin's saved values of keys in its schema
    /// (filter and dither excluded – those come from the request)
    pub sets: Vec<(String, String)>,
    /// built-in engine: level change after upsampling in dB (0 = the original level)
    pub headroom_db: f64,
}

impl Default for EngineRun {
    fn default() -> Self {
        EngineRun { external: None, sets: vec![], headroom_db: 0.0 }
    }
}

/// "Cadenza 2.1.0 (updated)" / "Cadenza (built in)".
pub fn engine_label(external: Option<&External>) -> String {
    match external {
        Some(e) => format!("Cadenza {} (updated)", e.version),
        None => "Cadenza (built in)".to_string(),
    }
}

/// The admin's engine settings (`cadenza_engine_settings`, a JSON object).
pub fn saved_values(st: &AppState) -> Map<String, Value> {
    parse_values(&st.cfg.str("cadenza_engine_settings"))
}

pub fn parse_values(text: &str) -> Map<String, Value> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// A saved value as `--set` text (strings as they are, numbers and switches as JSON).
fn set_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The settings schema of the built-in engine, in the same form as `cadenza render --describe`.
pub fn builtin_schema(max_taps: usize) -> Value {
    let filters: Vec<Value> = filters(max_taps).iter().map(|f| json!([f.id(), f.label()])).collect();
    let dithers: Vec<Value> = DitherKind::catalog().iter().map(|d| json!([d.id(), d.label()])).collect();
    json!([
        { "key": "filter", "label": "Upsampling filter", "type": "select", "options": filters, "default": default_filter(max_taps).id(),
          "help": "The filter used when a listener did not pick one. Longer filters are sharper and use more CPU." },
        { "key": "dither", "label": "Dither / noise shaping", "type": "select", "options": dithers, "default": DitherKind::Tpdf.id(),
          "help": "How the 24-bit output is quantised when a listener did not pick one." },
        { "key": "headroom_db", "label": "Headroom (dB)", "type": "number", "min": -12.0, "max": 0.0, "step": 0.5, "default": 0.0,
          "help": "Level change after upsampling. 0 keeps the original level; the soft clipper catches intersample overs." },
    ])
}

/// The active engine and the admin's settings for it.
#[derive(Clone, Debug)]
pub struct Engine {
    pub external: Option<External>,
    /// the engine's settings schema
    pub settings: Value,
    /// the admin's saved values (all of them, also keys this engine doesn't know)
    pub values: Map<String, Value>,
}

impl Engine {
    /// The engine server processing uses now (looks at the downloaded program: call off the async runtime).
    pub fn of(st: &AppState) -> Engine {
        let prog = crate::tools::cadenza_external(st.data_dir());
        Engine::from_parts(prog, saved_values(st), Limits::of(st).max_taps)
    }

    pub fn from_parts(prog: Option<crate::tools::CadenzaProgram>, values: Map<String, Value>, max_taps: usize) -> Engine {
        match prog {
            Some(p) => Engine { external: Some(External { path: p.path, version: p.version }), settings: p.settings, values },
            None => Engine { external: None, settings: builtin_schema(max_taps), values },
        }
    }

    pub fn version(&self) -> String {
        match &self.external {
            Some(e) => e.version.clone(),
            None => crate::tools::builtin_cadenza_version(),
        }
    }

    pub fn source(&self) -> &'static str {
        if self.external.is_some() { "updated" } else { "bundled" }
    }

    pub fn label(&self) -> String {
        engine_label(self.external.as_ref())
    }

    fn keys(&self) -> Vec<String> {
        self.settings.as_array().map(|a| a.iter().filter_map(|s| s["key"].as_str().map(String::from)).collect()).unwrap_or_default()
    }

    /// The saved value of a key this engine has, as text.
    pub fn value(&self, key: &str) -> Option<String> {
        if !self.keys().iter().any(|k| k == key) {
            return None;
        }
        self.values.get(key).and_then(set_text).filter(|s| !s.is_empty())
    }

    /// Saved values of the keys in the schema, sorted by key.
    fn known(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self.keys().into_iter().filter_map(|k| self.value(&k).map(|v| (k, v))).collect();
        out.sort();
        out
    }

    /// What a job on this engine gets.
    pub fn run(&self) -> EngineRun {
        let headroom_db = match &self.external {
            Some(_) => 0.0,
            None => self.value("headroom_db").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()).map(|v| v.clamp(-12.0, 0.0)).unwrap_or(0.0),
        };
        let sets = match &self.external {
            Some(_) => self.known().into_iter().filter(|(k, _)| k != "filter" && k != "dither").collect(),
            None => vec![],
        };
        EngineRun { external: self.external.clone(), sets, headroom_db }
    }

    /// Part of the render cache key: the engine and its version, and the settings that change its output.
    pub fn cache_tag(&self) -> String {
        let vals: Vec<String> = self.known().into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{}-{}|{}", self.source(), self.version(), vals.join("&"))
    }

    /// `engine` in GET /api/playback/dsp.
    pub fn json(&self) -> Value {
        json!({ "version": self.version(), "source": self.source(), "label": self.label(), "settings": self.settings, "values": self.values })
    }
}

/// What one processed stream does.
pub struct Job {
    pub path: String,
    pub input: InputFormat,
    pub seek: f64,
    pub rate: u32,
    pub filter: FilterKind,
    pub dither: DitherKind,
    pub engine: EngineRun,
}

fn config(job: &Job) -> PipelineConfig {
    // level stays that of the original (no automatic -3 dB headroom) unless the admin set one; the soft clipper guards intersample overs
    PipelineConfig { filter: job.filter, output_rate: Some(job.rate), output_bits: 24, dither: job.dither, headroom_db: Some(job.engine.headroom_db), ..PipelineConfig::default() }
}

/// Bytes of interleaved f64 per processing block (about 0.1 s of stereo 44.1 kHz).
const BLOCK_BYTES: usize = 64 * 1024;

/// Where the FLAC encoder writes: a pipe (live streaming) or a file (cache render: ffmpeg can then
/// seek back and finish STREAMINFO and the seek table).
pub enum Output {
    Pipe,
    File(std::path::PathBuf),
}

/// A running decoder → Cadenza → encoder chain. The decoder (and a downloaded engine) belong to the
/// middle stage's task: they are killed when it ends, which it does once the encoder is gone.
pub struct Chain {
    pub encoder: tokio::process::Child,
    /// the encoder's stdout ([`Output::Pipe`] only)
    pub out: Option<tokio::process::ChildStdout>,
    /// the signal path label (with the engine used) once processing started, or why it couldn't
    pub ready: tokio::sync::oneshot::Receiver<Result<String, String>>,
    /// true when the whole input went through (false: an error, or `progress` said stop)
    pub done: tokio::task::JoinHandle<bool>,
    /// set when the downloaded engine failed and the built-in one rendered instead: the output is
    /// not what the engine's cache tag says, so it must not be cached under it
    pub fell_back: Arc<AtomicBool>,
}

/// Called after each block with the output frames written so far; returning false stops the chain.
pub type Progress = Box<dyn FnMut(u64) -> bool + Send>;

type Ready = tokio::sync::oneshot::Sender<Result<String, String>>;

/// ffmpeg decoding the job's file to interleaved f64le at the source rate.
fn decoder(job: &Job) -> std::io::Result<tokio::process::Child> {
    let mut dec = tokio::process::Command::new("ffmpeg");
    dec.args(["-v", "error", "-nostdin"]);
    if job.seek > 0.0 {
        dec.args(["-ss", &format!("{:.2}", job.seek)]);
    }
    dec.args(["-i", &job.path, "-map", "0:a:0", "-ar", &job.input.rate.to_string(), "-ac", &job.input.channels.to_string(), "-f", "f64le", "-acodec", "pcm_f64le", "pipe:1"]);
    dec.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    dec.spawn()
}

/// Decode with ffmpeg → Cadenza (downloaded or built in) → ffmpeg FLAC encoder (24-bit at `job.rate`).
pub fn spawn_chain(cache: CoefficientCache, job: Job, output: Output, progress: Option<Progress>) -> ApiResult<Chain> {
    let ch = job.input.channels as usize;
    let mut enc = tokio::process::Command::new("ffmpeg");
    enc.args(["-v", "error", "-nostdin", "-f", "s32le", "-ar", &job.rate.to_string(), "-ac", &ch.to_string(), "-i", "pipe:0"]);
    enc.args(["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24", "-compression_level", "0", "-f", "flac", "-flush_packets", "1"]);
    match &output {
        Output::Pipe => {
            enc.arg("pipe:1").stdout(Stdio::piped());
        }
        Output::File(p) => {
            enc.arg("-y").arg(p).stdout(Stdio::null());
        }
    }
    enc.stdin(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);

    let mut encoder = enc.spawn().map_err(|e| err(500, format!("ffmpeg: {e}")))?;
    let enc_in = encoder.stdin.take().ok_or_else(|| err(500, "ffmpeg took no input"))?;
    let out = encoder.stdout.take();
    let (ready_tx, ready) = tokio::sync::oneshot::channel::<Result<String, String>>();
    let fell_back = Arc::new(AtomicBool::new(false));
    let done = match job.engine.external.clone() {
        Some(ext) => tokio::spawn(external_or_builtin(ext, cache, job, enc_in, ready_tx, progress, fell_back.clone())),
        None => {
            let dec = decoder(&job).map_err(|e| err(500, format!("ffmpeg: {e}")))?;
            let rt = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                let mut enc_in = enc_in;
                let mut progress = progress;
                builtin(&rt, &cache, &job, dec, &mut enc_in, ready_tx, &mut progress)
                // dropping the encoder's stdin ends its stream
            })
        }
    };
    Ok(Chain { encoder, out, ready, done, fell_back })
}

/// The in-process engine: decoder stdout → cadenza-dsp Pipeline → encoder stdin.
fn builtin(
    rt: &tokio::runtime::Handle, cache: &CoefficientCache, job: &Job, mut dec: tokio::process::Child, enc_in: &mut tokio::process::ChildStdin, ready_tx: Ready,
    progress: &mut Option<Progress>,
) -> bool {
    let ch = job.input.channels as usize;
    let Some(mut dec_out) = dec.stdout.take() else {
        let _ = ready_tx.send(Err("ffmpeg gave no output".into()));
        return false;
    };
    let mut pipe = match Pipeline::new(job.input.clone(), config(job), cache) {
        Ok(p) => p,
        Err(e) => {
            let _ = ready_tx.send(Err(e.to_string()));
            return false;
        }
    };
    let _ = ready_tx.send(Ok(format!("{} · {}", pipe.signal_path().label, engine_label(None))));
    let frame = 8 * ch;
    let mut buf = vec![0u8; BLOCK_BYTES];
    let mut pending: Vec<u8> = Vec::with_capacity(BLOCK_BYTES + frame);
    let mut samples: Vec<f64> = Vec::with_capacity(BLOCK_BYTES / 8);
    let mut bytes: Vec<u8> = Vec::new();
    let mut frames_out: u64 = 0;
    let mut write = |ints: Vec<i32>, bytes: &mut Vec<u8>, frames_out: &mut u64| -> bool {
        if ints.is_empty() {
            return true;
        }
        *frames_out += (ints.len() / ch.max(1)) as u64;
        bytes.clear();
        bytes.reserve(ints.len() * 4);
        // cadenza-dsp quantises 24-bit output to ±2^23: shift up to full-scale s32
        for v in ints {
            bytes.extend_from_slice(&(v << 8).to_le_bytes());
        }
        rt.block_on(enc_in.write_all(bytes)).is_ok()
    };
    loop {
        let n = match rt.block_on(dec_out.read(&mut buf)) {
            Ok(0) => break,
            Err(_) => return false,
            Ok(n) => n,
        };
        pending.extend_from_slice(&buf[..n]);
        let whole = pending.len() / frame * frame;
        if whole == 0 {
            continue;
        }
        samples.clear();
        samples.extend(pending[..whole].chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())));
        pending.drain(..whole);
        if !write(pipe.process(&samples), &mut bytes, &mut frames_out) {
            return false; // the listener went away
        }
        if let Some(p) = progress.as_mut() {
            if !p(frames_out) {
                return false;
            }
        }
    }
    let ok = write(pipe.flush(), &mut bytes, &mut frames_out);
    if let Some(p) = progress.as_mut() {
        p(frames_out);
    }
    ok
}

/// The `cadenza render` arguments for a job.
pub fn render_args(job: &Job, cache_dir: Option<&std::path::Path>) -> Vec<String> {
    let mut a: Vec<String> = vec!["render".into()];
    let mut kv = |k: &str, v: String| {
        a.push(k.into());
        a.push(v);
    };
    kv("--in-rate", job.input.rate.to_string());
    kv("--channels", job.input.channels.to_string());
    kv("--in-format", "f64le".into());
    kv("--rate", job.rate.to_string());
    kv("--bits", "24".into());
    kv("--out-format", "s32le".into());
    kv("--headroom-db", "0".into());
    if let Some(d) = cache_dir {
        kv("--cache-dir", d.to_string_lossy().into_owned());
    }
    for (k, v) in &job.engine.sets {
        kv("--set", format!("{k}={v}"));
    }
    // the request's filter and dither, after the saved values (the last --set wins)
    kv("--set", format!("filter={}", job.filter.id()));
    kv("--set", format!("dither={}", job.dither.id()));
    a
}

/// The signal path label for a downloaded engine (no Pipeline here to ask).
fn external_label(job: &Job, ext: &External) -> String {
    use cadenza_dsp::signal_path::{Endpoint, SignalPath};
    let input = Endpoint { format: job.input.codec.clone(), rate: job.input.rate, bits: job.input.bits, channels: job.input.channels, device: None };
    let output = Endpoint { format: "PCM".into(), rate: job.rate, bits: 24, channels: job.input.channels, device: None };
    format!("{} · {}", SignalPath::make_label(&input, &output, false), engine_label(Some(ext)))
}

/// How long the downloaded engine may take to produce its first audio before the built-in engine takes over.
#[cfg(not(test))]
const FIRST_OUTPUT: std::time::Duration = std::time::Duration::from_secs(20);
#[cfg(test)]
const FIRST_OUTPUT: std::time::Duration = std::time::Duration::from_secs(2);

/// How a run of the downloaded engine ended.
enum Attempt {
    /// it failed before any of its audio reached the encoder: the built-in engine can take over
    Fallback(String),
    /// it ran (ready was sent): whether the whole input went through
    Finished(bool),
}

/// Run the downloaded engine; when it fails before producing audio (doesn't start, rejects an
/// argument, crashes at once), log it and run the built-in engine instead, from a fresh decoder.
async fn external_or_builtin(
    ext: External, cache: CoefficientCache, job: Job, mut enc_in: tokio::process::ChildStdin, ready_tx: Ready, mut progress: Option<Progress>, fell_back: Arc<AtomicBool>,
) -> bool {
    let mut ready = Some(ready_tx);
    match external(&ext, &cache, &job, &mut enc_in, &mut ready, &mut progress).await {
        Attempt::Finished(ok) => ok,
        Attempt::Fallback(why) => {
            tracing::warn!("cadenza: the downloaded Cadenza {} failed ({why}) – using the built-in engine for this render", ext.version);
            let Some(ready_tx) = ready.take() else { return false };
            fell_back.store(true, Ordering::SeqCst);
            let dec = match decoder(&job) {
                Ok(d) => d,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("ffmpeg: {e}")));
                    return false;
                }
            };
            let rt = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || builtin(&rt, &cache, &job, dec, &mut enc_in, ready_tx, &mut progress)).await.unwrap_or(false)
        }
    }
}

async fn external(
    ext: &External, cache: &CoefficientCache, job: &Job, enc_in: &mut tokio::process::ChildStdin, ready: &mut Option<Ready>, progress: &mut Option<Progress>,
) -> Attempt {
    let frame = 4 * job.input.channels.max(1) as u64;
    let mut dec = match decoder(job) {
        Ok(d) => d,
        Err(e) => {
            if let Some(r) = ready.take() {
                let _ = r.send(Err(format!("ffmpeg: {e}")));
            }
            return Attempt::Finished(false);
        }
    };
    // ffmpeg's output is the engine's input directly (no copy through Encore)
    let stdin: Stdio = match dec.stdout.take().map(|o| o.try_into()) {
        Some(Ok(s)) => s,
        _ => return Attempt::Fallback("the decoder's output could not be passed on".into()),
    };
    let mut cmd = tokio::process::Command::new(&ext.path);
    cmd.args(render_args(job, cache.dir())).stdin(stdin).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Attempt::Fallback(format!("could not start {}: {e}", ext.path.display())),
    };
    let stderr = child.stderr.take().map(|mut e| {
        tokio::spawn(async move {
            let mut text = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = e.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if text.len() < 4096 {
                    text.extend_from_slice(&buf[..n]);
                }
            }
            String::from_utf8_lossy(&text).trim().to_string()
        })
    });
    let Some(mut out) = child.stdout.take() else { return Attempt::Fallback("no output".into()) };
    let mut buf = vec![0u8; 256 * 1024];
    let mut written: u64 = 0;
    loop {
        // an engine that hangs before its first output (a stuck GPU init, say) is given up on
        let read = if written == 0 {
            match tokio::time::timeout(FIRST_OUTPUT, out.read(&mut buf)).await {
                Ok(r) => r,
                Err(_) => {
                    let _ = child.start_kill();
                    return Attempt::Fallback(format!("no output within {} s", FIRST_OUTPUT.as_secs()));
                }
            }
        } else {
            out.read(&mut buf).await
        };
        let n = match read {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if written == 0 => return Attempt::Fallback(e.to_string()),
            Err(_) => return Attempt::Finished(false),
        };
        if let Some(r) = ready.take() {
            let _ = r.send(Ok(external_label(job, ext)));
        }
        if enc_in.write_all(&buf[..n]).await.is_err() {
            return Attempt::Finished(false); // the listener went away
        }
        written += n as u64;
        if let Some(p) = progress.as_mut() {
            if !p(written / frame) {
                return Attempt::Finished(false);
            }
        }
    }
    let status = child.wait().await;
    let text = match stderr {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    drop(dec);
    match status {
        Ok(s) if s.success() => {
            if let Some(r) = ready.take() {
                let _ = r.send(Ok(external_label(job, ext)));
            }
            if let Some(p) = progress.as_mut() {
                p(written / frame);
            }
            Attempt::Finished(true)
        }
        other => {
            let why = match other {
                Ok(s) => format!("{s}{}", if text.is_empty() { String::new() } else { format!(": {}", text.lines().last().unwrap_or("")) }),
                Err(e) => e.to_string(),
            };
            if written == 0 {
                Attempt::Fallback(why)
            } else {
                tracing::warn!("cadenza: the downloaded Cadenza {} stopped part way ({why})", ext.version);
                Attempt::Finished(false)
            }
        }
    }
}

/// The stream headers every Cadenza response carries.
pub fn stream_headers(h: &mut axum::http::HeaderMap, rate: u32, label: &str, how: &'static str, offset: f64) {
    if let Ok(v) = HeaderValue::from_str(&format!("cadenza-{rate}")) {
        h.insert(HeaderName::from_static("x-encore-stream"), v.clone());
        h.insert(HeaderName::from_static("x-transcoded"), v);
    }
    if let Ok(v) = HeaderValue::from_str(&ascii_label(label)) {
        h.insert(HeaderName::from_static("x-cadenza-path"), v);
    }
    h.insert(HeaderName::from_static("x-encore-dsp"), HeaderValue::from_static(how));
    if let Ok(v) = HeaderValue::from_str(&format!("{}", (offset * 100.0).round() / 100.0)) {
        h.insert(HeaderName::from_static("x-encore-dsp-offset"), v);
    }
}

/// Live Cadenza: the chain's output is the response body (starting at `job.seek`). Both ffmpeg
/// processes are killed and the slot is freed when the body ends or is dropped.
pub async fn stream(cache: CoefficientCache, job: Job, slot: Slot) -> ApiResult<Response> {
    let (rate, seek) = (job.rate, job.seek);
    let mut chain = spawn_chain(cache, job, Output::Pipe, None)?;
    let label = match (&mut chain.ready).await {
        Ok(Ok(l)) => l,
        Ok(Err(e)) => return Err(err(500, format!("Cadenza: {e}"))),
        Err(_) => return Err(err(500, "Cadenza stopped")),
    };
    let enc_out = chain.out.take().ok_or_else(|| err(500, "ffmpeg gave no output"))?;
    // the children (kill_on_drop) and the slot live exactly as long as the body
    let keep = (chain, slot);
    let body = futures_util::StreamExt::map(tokio_util::io::ReaderStream::new(enc_out), move |c| {
        let _keep = &keep;
        c
    });
    let mut r = ([(header::CONTENT_TYPE, "audio/flac")], Body::from_stream(body)).into_response();
    stream_headers(r.headers_mut(), rate, &label, "live", seek);
    Ok(r)
}

/// Run 1 s of 44.1 kHz stereo silence through a downloaded engine; Ok(output bytes).
pub async fn external_self_test(ext: &External, job: &Job, cache: &CoefficientCache) -> Result<usize, String> {
    let mut cmd = tokio::process::Command::new(&ext.path);
    cmd.args(render_args(job, cache.dir())).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", ext.path.display()))?;
    let mut stdin = child.stdin.take().ok_or("no input")?;
    let frames = job.input.rate as usize;
    let silence = vec![0u8; frames * job.input.channels as usize * 8];
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&silence).await;
        // dropping stdin is the end of the input
    });
    let fut = child.wait_with_output();
    let o = tokio::time::timeout(std::time::Duration::from_secs(120), fut).await.map_err(|_| "no answer within 2 minutes".to_string())?.map_err(|e| e.to_string())?;
    let _ = writer.await;
    if !o.status.success() {
        let text = String::from_utf8_lossy(&o.stderr);
        return Err(format!("it exited with {}{}", o.status, text.lines().last().map(|l| format!(": {l}")).unwrap_or_default()));
    }
    if o.stdout.is_empty() {
        return Err("it produced no output".into());
    }
    Ok(o.stdout.len())
}

/// Admin Test: build a 44.1 kHz → max rate chain with the default filter on the active engine and run
/// 1 s of silence.
pub async fn self_test(st: &AppState) -> Result<String, String> {
    let lim = Limits::of(st);
    let cache = cache(st);
    let st2 = st.clone();
    let engine = tokio::task::spawn_blocking(move || Engine::of(&st2)).await.map_err(|e| e.to_string())?;
    let filter = choose_filter_or("", &engine.value("filter").unwrap_or_default(), lim.max_taps);
    let dither = choose_dither_or("", &engine.value("dither").unwrap_or_default());
    let rate = lim.max_rate.max(44_100);
    let run = engine.run();
    let name = engine.label();
    let input = InputFormat { rate: 44_100, bits: 16, channels: 2, codec: "PCM".into() };
    let job = Job { path: String::new(), input, seek: 0.0, rate, filter, dither, engine: run };
    match job.engine.external.clone() {
        Some(ext) => {
            external_self_test(&ext, &job, &cache).await.map_err(|e| format!("{name} failed: {e} – processing falls back to the built-in engine"))?;
        }
        None => {
            tokio::task::spawn_blocking(move || {
                let mut p = Pipeline::new(job.input.clone(), config(&job), &cache).map_err(|e| e.to_string())?;
                let silence = vec![0.0f64; 44_100 * 2];
                let mut n = p.process(&silence).len();
                n += p.flush().len();
                if n == 0 {
                    return Err("Cadenza produced no output".to_string());
                }
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())??;
        }
    }
    let mut msg = format!("{name} ready · up to {} kHz · {}/{} streams in use", lim.max_rate / 1000, active(), lim.max_streams);
    msg.push_str(" · ");
    msg.push_str(&render::usage_text(st));
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadenza_dsp::Phase;

    #[test]
    fn target_rate_rule() {
        // auto: highest ×2, ×4 … the server allows
        assert_eq!(target_rate(44_100, 0, 192_000), 176_400);
        assert_eq!(target_rate(48_000, 0, 192_000), 192_000);
        assert_eq!(target_rate(44_100, 0, 96_000), 88_200);
        assert_eq!(target_rate(96_000, 0, 768_000), 768_000);
        assert_eq!(target_rate(44_100, 0, 768_000), 705_600);
        // nothing fits: stay at the source rate
        assert_eq!(target_rate(192_000, 0, 192_000), 192_000);
        assert_eq!(target_rate(96_000, 0, 192_000), 192_000);
        assert_eq!(target_rate(176_400, 0, 96_000), 176_400);
        // requested: capped, never below the source
        assert_eq!(target_rate(44_100, 96_000, 192_000), 96_000);
        assert_eq!(target_rate(44_100, 384_000, 192_000), 192_000);
        assert_eq!(target_rate(96_000, 48_000, 192_000), 96_000);
        assert_eq!(target_rate(192_000, 96_000, 96_000), 192_000);
    }

    #[test]
    fn filter_cap() {
        let f = |id: &str| FilterKind::parse(id).unwrap();
        // fits: unchanged
        assert_eq!(cap_filter(f("sinc-l-64k"), 65_536), f("sinc-l-64k"));
        assert_eq!(cap_filter(f("sinc-m-16k"), 65_536), f("sinc-m-16k"));
        // too long: longest of the same kind that fits
        assert_eq!(cap_filter(f("sinc-l-1m"), 65_536), f("sinc-l-64k"));
        assert_eq!(cap_filter(f("sinc-ma-4m"), 262_144), f("sinc-ma-256k"));
        assert_eq!(cap_filter(f("sinc-la-256k"), 16_384), f("sinc-la-16k"));
        assert_eq!(cap_filter(f("sinc-m-100000"), 65_536), f("sinc-m-64k"));
        // nothing fits: the shortest
        let tiny = cap_filter(f("sinc-l-64k"), 10);
        assert_eq!(tiny.taps, FilterKind::catalog().iter().map(|k| k.taps).min().unwrap());
        assert_eq!(tiny.phase, Phase::Linear);
        // request handling
        assert_eq!(choose_filter("nonsense", 65_536), FilterKind::default());
        assert_eq!(choose_filter("", 16_384), f("sinc-l-16k"));
        assert_eq!(default_filter(1_048_576), FilterKind::default());
        assert!(filters(65_536).iter().all(|k| k.taps <= 65_536));
        assert!(filters(65_536).contains(&f("sinc-ma-64k")));
        assert_eq!(choose_dither("bogus"), DitherKind::Tpdf);
        assert_eq!(choose_dither("lipshitz"), DitherKind::Lipshitz);
    }

    #[test]
    fn slots_and_labels() {
        let a = try_acquire(usize::MAX).unwrap();
        let before = active();
        assert!(before >= 1);
        assert!(try_acquire(0).is_none());
        drop(a);
        assert_eq!(ascii_label("FLAC 16/44.1 → 24/96"), "FLAC 16/44.1 -> 24/96");
        assert!(ascii_label("FLAC 16/44.1 · bit-perfect").is_ascii());
        let i = input_format(&serde_json::json!({}));
        assert_eq!((i.rate, i.channels), (44_100, 2));
    }

    fn prog(settings: Value) -> crate::tools::CadenzaProgram {
        crate::tools::CadenzaProgram { path: PathBuf::from("/x/cadenza"), version: "2.1.0".into(), settings }
    }

    #[test]
    fn engine_settings_follow_the_schema() {
        let values = parse_values(r#"{"filter":"sinc-m-16k","dither":"lipshitz","headroom_db":-3,"gpu":"off","soft_clip":false,"future_key":"x","note":null}"#);
        assert!(parse_values("not json").is_empty() && parse_values("[1]").is_empty() && parse_values("").is_empty());
        // built in: filter, dither, headroom_db from cadenza-dsp's catalogue
        let b = Engine::from_parts(None, values.clone(), 65_536);
        let keys: Vec<&str> = b.settings.as_array().unwrap().iter().map(|s| s["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["filter", "dither", "headroom_db"]);
        assert!(b.settings[0]["options"].as_array().unwrap().iter().all(|o| FilterKind::parse(o[0].as_str().unwrap()).is_some_and(|f| f.taps <= 65_536)));
        assert_eq!(b.source(), "bundled");
        assert_eq!(b.label(), "Cadenza (built in)");
        let run = b.run();
        assert_eq!((run.external.clone(), run.sets.len(), run.headroom_db), (None, 0, -3.0));
        assert_eq!(b.value("gpu"), None, "the built-in engine has no gpu setting");
        assert_eq!(b.value("filter").as_deref(), Some("sinc-m-16k"));
        assert_eq!(b.cache_tag(), format!("bundled-{}|dither=lipshitz&filter=sinc-m-16k&headroom_db=-3", crate::tools::builtin_cadenza_version()));
        assert_eq!(b.json()["values"]["future_key"], "x");
        // headroom is clamped
        assert_eq!(Engine::from_parts(None, parse_values(r#"{"headroom_db":"-40"}"#), 65_536).run().headroom_db, -12.0);
        assert_eq!(Engine::from_parts(None, parse_values(r#"{"headroom_db":"loud"}"#), 65_536).run().headroom_db, 0.0);
        // downloaded: its own schema; keys it doesn't have are skipped, filter/dither come from the request
        let schema = json!([{ "key": "filter" }, { "key": "dither" }, { "key": "headroom_db" }, { "key": "soft_clip" }, { "key": "gpu" }]);
        let e = Engine::from_parts(Some(prog(schema)), values, 65_536);
        assert_eq!((e.source(), e.version(), e.label()), ("updated", "2.1.0".to_string(), "Cadenza 2.1.0 (updated)".to_string()));
        let run = e.run();
        assert_eq!(run.external, Some(External { path: PathBuf::from("/x/cadenza"), version: "2.1.0".into() }));
        assert_eq!(run.sets, vec![("gpu".to_string(), "off".to_string()), ("headroom_db".into(), "-3".into()), ("soft_clip".into(), "false".into())]);
        assert_eq!(run.headroom_db, 0.0);
        assert_eq!(e.cache_tag(), "updated-2.1.0|dither=lipshitz&filter=sinc-m-16k&gpu=off&headroom_db=-3&soft_clip=false");
        assert_ne!(e.cache_tag(), Engine::from_parts(Some(prog(json!([{ "key": "gpu" }]))), parse_values(r#"{"gpu":"on"}"#), 65_536).cache_tag());
        // the admin's filter/dither are the defaults for requests without one
        assert_eq!(choose_filter_or("", "sinc-m-16k", 65_536), FilterKind::parse("sinc-m-16k").unwrap());
        assert_eq!(choose_filter_or("sinc-l-4k", "sinc-m-16k", 65_536), FilterKind::parse("sinc-l-4k").unwrap());
        assert_eq!(choose_filter_or("", "sinc-l-1m", 65_536), FilterKind::parse("sinc-l-64k").unwrap(), "capped");
        assert_eq!(choose_dither_or("", "lipshitz"), DitherKind::Lipshitz);
        assert_eq!(choose_dither_or("tpdf", "lipshitz"), DitherKind::Tpdf);
    }

    fn job(path: &str, engine: EngineRun) -> Job {
        Job {
            path: path.into(),
            input: InputFormat { rate: 44_100, bits: 16, channels: 2, codec: "WAV".into() },
            seek: 0.0,
            rate: 88_200,
            filter: choose_filter("sinc-l-4k", 65_536),
            dither: DitherKind::Tpdf,
            engine,
        }
    }

    #[test]
    fn render_arguments() {
        let run = EngineRun { external: None, sets: vec![("gpu".into(), "off".into()), ("filter".into(), "ignored-later".into())], headroom_db: 0.0 };
        let a = render_args(&job("x", run), Some(std::path::Path::new("/c/cadenza-cache")));
        let s = a.join(" ");
        assert!(s.starts_with("render --in-rate 44100 --channels 2 --in-format f64le --rate 88200 --bits 24 --out-format s32le --headroom-db 0 --cache-dir /c/cadenza-cache"), "{s}");
        assert!(s.ends_with("--set gpu=off --set filter=ignored-later --set filter=sinc-l-4k --set dither=tpdf"), "{s}");
    }

    fn have_ffmpeg() -> bool {
        std::process::Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("encore-dsp-test-{name}-{}-{}", std::process::id(), crate::auth::random_hex(4)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fake `cadenza`, renamed into place so no open write handle points at the file that is run.
    fn script(dir: &std::path::Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("cadenza");
        let t = dir.join(".cadenza.tmp");
        std::fs::write(&t, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&t, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&t, &p).unwrap();
        p
    }

    fn wav(dir: &std::path::Path, secs: u32) -> String {
        let p = dir.join("in.wav");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i", &format!("sine=f=1000:d={secs}:sample_rate=44100"), "-ac", "2", "-y"])
            .arg(&p)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        p.to_string_lossy().into_owned()
    }

    fn duration(p: &std::path::Path) -> f64 {
        let o = std::process::Command::new("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"]).arg(p).output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0.0)
    }

    /// Run a chain into a FLAC file: (label, completed, last progress frames, file, fell back).
    async fn run_chain(d: &std::path::Path, engine: EngineRun) -> (String, bool, u64, PathBuf, bool) {
        let src = wav(d, 2);
        let out = d.join("out.flac");
        let frames = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let f2 = frames.clone();
        let progress: Progress = Box::new(move |n| {
            f2.store(n, Ordering::SeqCst);
            true
        });
        let mut chain = spawn_chain(CoefficientCache::in_dir(d.join("coeff")), job(&src, engine), Output::File(out.clone()), Some(progress)).unwrap();
        let label = (&mut chain.ready).await.unwrap().unwrap();
        let ok = (&mut chain.done).await.unwrap();
        assert!(chain.encoder.wait().await.unwrap().success());
        (label, ok, frames.load(Ordering::SeqCst), out, chain.fell_back.load(Ordering::SeqCst))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chain_runs_the_downloaded_engine() {
        if !have_ffmpeg() {
            return;
        }
        let d = tmp("external");
        // a "Cadenza" that passes its input through: every f64 input byte reaches the encoder as s32 output
        let exe = script(&d, "[ \"$1\" = render ] || exit 9\nexec cat");
        let ext = External { path: exe, version: "2.1.0".into() };
        let (label, ok, frames, out, fell_back) = run_chain(&d, EngineRun { external: Some(ext), sets: vec![("gpu".into(), "off".into())], headroom_db: 0.0 }).await;
        assert!(ok);
        assert!(!fell_back);
        assert!(label.ends_with("· Cadenza 2.1.0 (updated)"), "{label}");
        assert!(label.starts_with("WAV 16/44.1 → 24/88.2"), "{label}");
        // 2 s of f64 stereo = 2 × the frames as s32 stereo: 4 s at 44.1 kHz = 2 s at 88.2 kHz
        assert_eq!(frames, 2 * 2 * 44_100);
        assert!((duration(&out) - 2.0).abs() < 0.05, "{}", duration(&out));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chain_falls_back_to_the_built_in_engine() {
        if !have_ffmpeg() {
            return;
        }
        // exits at once; isn't there; hangs without output (given up on after FIRST_OUTPUT)
        for (name, ext) in [("exit2", Some("echo 'cadenza: bad value for gpu' >&2\nexit 2")), ("missing", None), ("hang", Some("exec sleep 60"))] {
            let d = tmp(name);
            let path = match ext {
                Some(body) => script(&d, body),
                None => d.join("not-there").join("cadenza"),
            };
            let run = EngineRun { external: Some(External { path, version: "2.1.0".into() }), sets: vec![], headroom_db: 0.0 };
            let t = std::time::Instant::now();
            let (label, ok, frames, out, fell_back) = run_chain(&d, run).await;
            assert!(ok, "{name}");
            assert!(fell_back, "{name}: a fallback render must be flagged so it isn't cached under the downloaded engine's key");
            assert!(t.elapsed() < std::time::Duration::from_secs(30), "{name}: took {:?}", t.elapsed());
            assert!(label.ends_with("· Cadenza (built in)"), "{name}: {label}");
            // the built-in engine upsampled 2 s to 88.2 kHz
            assert!((frames as i64 - 2 * 88_200).abs() < 100, "{name}: {frames}");
            assert!((duration(&out) - 2.0).abs() < 0.05, "{name}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chain_built_in_and_self_test_of_a_downloaded_engine() {
        if !have_ffmpeg() {
            return;
        }
        let d = tmp("builtin");
        let (label, ok, frames, _, fell_back) = run_chain(&d, EngineRun::default()).await;
        assert!(!fell_back);
        assert!(ok && label.ends_with("· Cadenza (built in)"), "{label}");
        assert!((frames as i64 - 2 * 88_200).abs() < 100);
        // the admin Test runs 1 s of silence through a downloaded engine
        let cache = CoefficientCache::in_dir(d.join("coeff"));
        let good = External { path: script(&d, "exec cat"), version: "2.1.0".into() };
        let j = job("", EngineRun { external: Some(good.clone()), ..EngineRun::default() });
        assert_eq!(external_self_test(&good, &j, &cache).await.unwrap(), 44_100 * 2 * 8);
        let bad = External { path: script(&d, "echo 'cadenza: unknown filter' >&2\nexit 2"), version: "2.1.0".into() };
        let e = external_self_test(&bad, &j, &cache).await.unwrap_err();
        assert!(e.contains("unknown filter"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
