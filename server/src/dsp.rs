//! Server-side Cadenza processing: library streams upsampled on this server by cadenza-dsp
//! (long-filter upsampling, dither / noise shaping) and sent as lossless 24-bit FLAC.
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
use serde_json::Value;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Processed streams running right now, whole server.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// One processed stream's place in the server-wide limit; frees it when dropped.
pub struct Slot(());

impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Take a slot when fewer than `max` processed streams are running.
pub fn try_acquire(max: usize) -> Option<Slot> {
    ACTIVE
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < max).then_some(n + 1))
        .ok()
        .map(|_| Slot(()))
}

pub fn active() -> usize {
    ACTIVE.load(Ordering::SeqCst)
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

/// `kind` when it fits `max_taps`; otherwise the longest catalogue filter of the same phase and
/// apodizing kind that does (or the shortest catalogue filter of that kind when none does).
pub fn cap_filter(kind: FilterKind, max_taps: usize) -> FilterKind {
    if kind.taps <= max_taps {
        return kind;
    }
    let same: Vec<FilterKind> = FilterKind::catalog().into_iter().filter(|f| f.phase == kind.phase && f.apodizing == kind.apodizing).collect();
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
pub fn choose_filter(id: &str, max_taps: usize) -> FilterKind {
    cap_filter(FilterKind::parse(id).unwrap_or_default(), max_taps)
}

pub fn choose_dither(id: &str) -> DitherKind {
    DitherKind::parse(id).unwrap_or(DitherKind::Tpdf)
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

/// What one processed stream does.
pub struct Job {
    pub path: String,
    pub input: InputFormat,
    pub seek: f64,
    pub rate: u32,
    pub filter: FilterKind,
    pub dither: DitherKind,
}

fn config(job: &Job) -> PipelineConfig {
    PipelineConfig { filter: job.filter, output_rate: Some(job.rate), output_bits: 24, dither: job.dither, ..PipelineConfig::default() }
}

/// Bytes of interleaved f64 per processing block (about 0.1 s of stereo 44.1 kHz).
const BLOCK_BYTES: usize = 64 * 1024;

/// Decode with ffmpeg → cadenza-dsp → ffmpeg FLAC encoder, streamed as the response body. Both
/// ffmpeg processes are killed and the slot is freed when the body ends or is dropped.
pub async fn stream(cache: CoefficientCache, job: Job, slot: Slot) -> ApiResult<Response> {
    let ch = job.input.channels as usize;
    let mut dec = tokio::process::Command::new("ffmpeg");
    dec.args(["-v", "error", "-nostdin"]);
    if job.seek > 0.0 {
        dec.args(["-ss", &format!("{:.2}", job.seek)]);
    }
    dec.args(["-i", &job.path, "-map", "0:a:0", "-ar", &job.input.rate.to_string(), "-ac", &ch.to_string(), "-f", "f64le", "-acodec", "pcm_f64le", "pipe:1"]);
    dec.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let mut enc = tokio::process::Command::new("ffmpeg");
    enc.args(["-v", "error", "-nostdin", "-f", "s32le", "-ar", &job.rate.to_string(), "-ac", &ch.to_string(), "-i", "pipe:0"]);
    enc.args(["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24", "-compression_level", "0", "-f", "flac", "-flush_packets", "1", "pipe:1"]);
    enc.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);

    let mut decoder = dec.spawn().map_err(|e| err(500, format!("ffmpeg: {e}")))?;
    let mut encoder = enc.spawn().map_err(|e| err(500, format!("ffmpeg: {e}")))?;
    let mut dec_out = decoder.stdout.take().ok_or_else(|| err(500, "ffmpeg gave no output"))?;
    let mut enc_in = encoder.stdin.take().ok_or_else(|| err(500, "ffmpeg took no input"))?;
    let enc_out = encoder.stdout.take().ok_or_else(|| err(500, "ffmpeg gave no output"))?;

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
    let rt = tokio::runtime::Handle::current();
    let rate = job.rate;
    tokio::task::spawn_blocking(move || {
        let mut pipe = match Pipeline::new(job.input.clone(), config(&job), &cache) {
            Ok(p) => p,
            Err(e) => {
                let _ = ready_tx.send(Err(e.to_string()));
                return;
            }
        };
        let _ = ready_tx.send(Ok(pipe.signal_path().label));
        let frame = 8 * ch;
        let mut buf = vec![0u8; BLOCK_BYTES];
        let mut pending: Vec<u8> = Vec::with_capacity(BLOCK_BYTES + frame);
        let mut samples: Vec<f64> = Vec::with_capacity(BLOCK_BYTES / 8);
        let mut bytes: Vec<u8> = Vec::new();
        let mut write = |ints: Vec<i32>, bytes: &mut Vec<u8>| -> bool {
            if ints.is_empty() {
                return true;
            }
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
                Ok(0) | Err(_) => break,
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
            if !write(pipe.process(&samples), &mut bytes) {
                return; // the listener went away
            }
        }
        write(pipe.flush(), &mut bytes);
        // dropping the encoder's stdin ends its stream
    });

    let label = match ready_rx.await {
        Ok(Ok(l)) => l,
        Ok(Err(e)) => return Err(err(500, format!("Cadenza: {e}"))),
        Err(_) => return Err(err(500, "Cadenza stopped")),
    };
    // the children (kill_on_drop) and the slot live exactly as long as the body
    let keep = (decoder, encoder, slot);
    let body = futures_util::StreamExt::map(tokio_util::io::ReaderStream::new(enc_out), move |c| {
        let _keep = &keep;
        c
    });
    let tag = format!("cadenza-{rate}");
    let mut r = ([(header::CONTENT_TYPE, "audio/flac")], Body::from_stream(body)).into_response();
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&tag) {
        h.insert(HeaderName::from_static("x-encore-stream"), v.clone());
        h.insert(HeaderName::from_static("x-transcoded"), v);
    }
    if let Ok(v) = HeaderValue::from_str(&ascii_label(&label)) {
        h.insert(HeaderName::from_static("x-cadenza-path"), v);
    }
    Ok(r)
}

/// Admin Test: build a 44.1 kHz → max rate chain with the default filter and run 1 s of silence.
pub async fn self_test(st: &AppState) -> Result<String, String> {
    let lim = Limits::of(st);
    let cache = cache(st);
    let filter = default_filter(lim.max_taps);
    let rate = lim.max_rate.max(44_100);
    tokio::task::spawn_blocking(move || {
        let input = InputFormat { rate: 44_100, bits: 16, channels: 2, codec: "PCM".into() };
        let cfg = PipelineConfig { filter, output_rate: Some(rate), output_bits: 24, dither: DitherKind::Tpdf, ..PipelineConfig::default() };
        let mut p = Pipeline::new(input, cfg, &cache).map_err(|e| e.to_string())?;
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
    Ok(format!("Cadenza ready · up to {} kHz · {}/{} streams in use", lim.max_rate / 1000, active(), lim.max_streams))
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
}
