//! The engines a Cadenza job runs on, behind one trait ([`Renderer`]): [`BuiltIn`], the cadenza-dsp compiled
//! into Encore, and [`Downloaded`], the `cadenza render` program an admin installed under Download tools.
//!
//! Live streams and the render cache run every job the same way ([`super::spawn_chain`]): ffmpeg decodes
//! the file, the renderer upsamples, ffmpeg encodes 24-bit FLAC. A renderer that fails before any of its
//! audio reached the encoder says so ([`Outcome::Unavailable`]) and the chain runs the job on the built-in
//! engine instead. More renderers (0.32's plugins) implement the same trait.
use super::{config, engine_label, External, Job, Progress, Ready, BLOCK_BYTES};
use cadenza_dsp::{CoefficientCache, Pipeline};
use encore_media::Ffmpeg;
use futures_util::future::BoxFuture;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Where a renderer writes and how it reports.
pub struct Io {
    /// the FLAC encoder's input: interleaved s32le at the job's rate (24-bit audio in full-scale words)
    pub out: tokio::process::ChildStdin,
    /// sent once: the signal path label (with the engine) when processing started, or why it could not start
    pub ready: Option<Ready>,
    /// called with the output frames written so far; false stops the run
    pub progress: Option<Progress>,
}

/// How a run ended.
pub enum Outcome {
    /// it ran (`ready` was sent): whether the whole input went through
    Finished(bool),
    /// it failed before any of its audio reached the encoder (it did not start, rejected an argument,
    /// crashed at once, stayed silent too long): another engine can take `io` over
    Unavailable { why: String, io: Io },
}

/// An engine server processing runs on.
pub trait Renderer: Send + Sync {
    /// How the signal path names it: "Cadenza (built in)", "Cadenza 2.1.0 (updated)".
    fn label(&self) -> String;
    /// Decode `job`'s file (with a decoder of its own), process it and write the result to `io.out`.
    fn render(&self, job: Job, cache: CoefficientCache, io: Io) -> BoxFuture<'static, Outcome>;
    /// The admin Test: `job` (no file: one second of silence in its input format) through the engine.
    fn self_test(&self, job: Job, cache: CoefficientCache) -> BoxFuture<'static, Result<(), String>>;
}

/// ffmpeg decoding the job's file to interleaved f64le at the source rate.
fn decoder(job: &Job) -> std::io::Result<tokio::process::Child> {
    Ffmpeg::new().quiet().seek(job.seek).input(&job.path)
        .args(["-map", "0:a:0", "-ar", &job.input.rate.to_string(), "-ac", &job.input.channels.to_string(), "-f", "f64le", "-acodec", "pcm_f64le", "pipe:1"])
        .pipe_out()
        .spawn()
}

fn send(ready: &mut Option<Ready>, v: Result<String, String>) {
    if let Some(r) = ready.take() {
        let _ = r.send(v);
    }
}

// ---- the built-in engine ------------------------------------------------------------------------------

/// The cadenza-dsp compiled into Encore, run in this process. It is always there, and it is what every
/// other renderer falls back to.
pub struct BuiltIn;

impl Renderer for BuiltIn {
    fn label(&self) -> String {
        engine_label(None)
    }

    fn render(&self, job: Job, cache: CoefficientCache, io: Io) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            let Io { mut out, mut ready, mut progress } = io;
            let dec = match decoder(&job) {
                Ok(d) => d,
                Err(e) => {
                    send(&mut ready, Err(format!("ffmpeg: {e}")));
                    return Outcome::Finished(false);
                }
            };
            let rt = tokio::runtime::Handle::current();
            // dropping the encoder's input when this ends is the end of its stream
            let ok = tokio::task::spawn_blocking(move || builtin(&rt, &cache, &job, dec, &mut out, ready, &mut progress)).await.unwrap_or(false);
            Outcome::Finished(ok)
        })
    }

    fn self_test(&self, job: Job, cache: CoefficientCache) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut p = Pipeline::new(job.input.clone(), config(&job), &cache).map_err(|e| e.to_string())?;
                let silence = vec![0.0f64; job.input.rate as usize * job.input.channels as usize];
                let mut n = p.process(&silence).len();
                n += p.flush().len();
                if n == 0 {
                    return Err("Cadenza produced no output".to_string());
                }
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())?
        })
    }
}

/// The in-process engine: decoder stdout → cadenza-dsp Pipeline → encoder stdin.
fn builtin(
    rt: &tokio::runtime::Handle, cache: &CoefficientCache, job: &Job, mut dec: tokio::process::Child, enc_in: &mut tokio::process::ChildStdin, mut ready: Option<Ready>,
    progress: &mut Option<Progress>,
) -> bool {
    let ch = job.input.channels as usize;
    let Some(mut dec_out) = dec.stdout.take() else {
        send(&mut ready, Err("ffmpeg gave no output".into()));
        return false;
    };
    let mut pipe = match Pipeline::new(job.input.clone(), config(job), cache) {
        Ok(p) => p,
        Err(e) => {
            send(&mut ready, Err(e.to_string()));
            return false;
        }
    };
    send(&mut ready, Ok(format!("{} · {}", pipe.signal_path().label, engine_label(None))));
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
        samples.extend(pending[..whole].as_chunks::<8>().0.iter().map(|b| f64::from_le_bytes(*b)));
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

// ---- the downloaded program ---------------------------------------------------------------------------

/// The downloaded `cadenza render` program, run as the middle stage between the two ffmpeg processes
/// (the decoder's output is its input directly, its output goes to the encoder through Encore).
pub struct Downloaded(pub External);

impl Renderer for Downloaded {
    fn label(&self) -> String {
        engine_label(Some(&self.0))
    }

    fn render(&self, job: Job, cache: CoefficientCache, io: Io) -> BoxFuture<'static, Outcome> {
        let ext = self.0.clone();
        Box::pin(async move { external(&ext, &cache, &job, io).await })
    }

    fn self_test(&self, job: Job, cache: CoefficientCache) -> BoxFuture<'static, Result<(), String>> {
        let (ext, name) = (self.0.clone(), self.label());
        Box::pin(async move {
            external_self_test(&ext, &job, &cache).await.map(|_| ()).map_err(|e| format!("{name} failed: {e} – processing falls back to the built-in engine"))
        })
    }
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

async fn external(ext: &External, cache: &CoefficientCache, job: &Job, mut io: Io) -> Outcome {
    let frame = 4 * job.input.channels.max(1) as u64;
    let mut dec = match decoder(job) {
        Ok(d) => d,
        Err(e) => {
            send(&mut io.ready, Err(format!("ffmpeg: {e}")));
            return Outcome::Finished(false);
        }
    };
    let unavailable = |why: String, io: Io| Outcome::Unavailable { why, io };
    // ffmpeg's output is the engine's input directly (no copy through Encore)
    let stdin: Stdio = match dec.stdout.take().map(|o| o.try_into()) {
        Some(Ok(s)) => s,
        _ => return unavailable("the decoder's output could not be passed on".into(), io),
    };
    let mut cmd = tokio::process::Command::new(&ext.path);
    cmd.args(render_args(job, cache.dir())).stdin(stdin).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return unavailable(format!("could not start {}: {e}", ext.path.display()), io),
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
    let Some(mut out) = child.stdout.take() else { return unavailable("no output".into(), io) };
    let mut buf = vec![0u8; 256 * 1024];
    let mut written: u64 = 0;
    loop {
        // an engine that hangs before its first output (a stuck GPU init, say) is given up on
        let read = if written == 0 {
            match tokio::time::timeout(FIRST_OUTPUT, out.read(&mut buf)).await {
                Ok(r) => r,
                Err(_) => {
                    let _ = child.start_kill();
                    return unavailable(format!("no output within {} s", FIRST_OUTPUT.as_secs()), io);
                }
            }
        } else {
            out.read(&mut buf).await
        };
        let n = match read {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if written == 0 => return unavailable(e.to_string(), io),
            Err(_) => return Outcome::Finished(false),
        };
        send(&mut io.ready, Ok(external_label(job, ext)));
        if io.out.write_all(&buf[..n]).await.is_err() {
            return Outcome::Finished(false); // the listener went away
        }
        written += n as u64;
        if let Some(p) = io.progress.as_mut() {
            if !p(written / frame) {
                return Outcome::Finished(false);
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
            send(&mut io.ready, Ok(external_label(job, ext)));
            if let Some(p) = io.progress.as_mut() {
                p(written / frame);
            }
            Outcome::Finished(true)
        }
        other => {
            let why = match other {
                Ok(s) => format!("{s}{}", if text.is_empty() { String::new() } else { format!(": {}", text.lines().last().unwrap_or("")) }),
                Err(e) => e.to_string(),
            };
            if written == 0 {
                unavailable(why, io)
            } else {
                tracing::warn!("cadenza: the downloaded Cadenza {} stopped part way ({why})", ext.version);
                Outcome::Finished(false)
            }
        }
    }
}

/// Run 1 s of silence in the job's input format through a downloaded engine; Ok(output bytes).
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
