//! Quality checks on audio files: fake lossless (a lossy file saved as FLAC: a hard shelf at 16/19/20 kHz),
//! upsampled hi-res (24-bit / 96 kHz with nothing above 22 kHz), loudness (EBU R128: very quiet files),
//! clipping, and the length against the recording that was asked for ("wrong version: 0:24 shorter").
//! Run on every download before it enters the library (a failing file is rejected and the next candidate
//! is tried) and by the "Quality scan" task over the library (files are flagged, never removed).
//! Results are kept per track in track_quality.
use crate::ffmpeg::Ffmpeg;
use crate::{Ctx, Media};
use encore_core::audio::LOSSLESS;
use encore_db::{now, p, RowExt};
use rustfft::num_complex::Complex;
use serde_json::{json, Value};
use std::path::Path;

pub use crate::loudness::parse_ebur128;

/// FFT length of the spectrum analysis (frames of the cutoff check).
const FFT: usize = 4096;
/// Genres that are meant to be quiet: the loudness check leaves them alone.
const QUIET_GENRES: [&str; 16] = ["classical", "klassik", "classique", "baroque", "orchestra", "symphon", "piano", "score", "ambient", "drone",
    "field recording", "soundtrack", "new age", "opera", "chamber", "choral"];

/// One problem the quality check found.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    /// fake_lossless | upsampled | low_bitrate | quiet | clipped | wrong_length
    pub code: &'static str,
    /// what the UI shows
    pub text: String,
}

impl Issue {
    /// A download with this problem is turned away (the next candidate is tried); the others are only shown.
    /// "quiet" is only shown: a quiet master (classical, a solo piano piece, an interlude) without a genre
    /// tag can't be told from a bad rip.
    pub fn rejects(&self) -> bool {
        matches!(self.code, "fake_lossless" | "upsampled" | "wrong_length")
    }
}

/// What the quality check found in one file.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// the problems, in the order found
    pub issues: Vec<Issue>,
    /// where the spectrum falls off, when it does before the format's limit
    pub cutoff_hz: Option<f64>,
    /// integrated loudness (LUFS)
    pub lufs: Option<f64>,
    /// true peak (dBTP)
    pub true_peak: Option<f64>,
    /// false when nothing could be measured (no ffmpeg, unreadable file)
    pub checked: bool,
}

impl Report {
    /// The issues' texts, joined with "; ".
    pub fn summary(&self) -> String {
        self.issues.iter().map(|i| i.text.clone()).collect::<Vec<_>>().join("; ")
    }
    /// The issue that turns the download away, if any.
    pub fn rejection(&self) -> Option<&Issue> {
        self.issues.iter().find(|i| i.rejects())
    }
    /// The report as stored and sent (`status` ok, suspect or unchecked; `issues`; measurements).
    pub fn to_json(&self) -> Value {
        json!({
            "status": if !self.checked { "unchecked" } else if self.issues.is_empty() { "ok" } else { "suspect" },
            "issues": self.issues.iter().map(|i| json!({ "code": i.code, "text": i.text })).collect::<Vec<_>>(),
            "summary": self.summary(), "cutoff_hz": self.cutoff_hz, "lufs": self.lufs, "true_peak": self.true_peak,
        })
    }
}

// ---- pure measurements (unit-tested) ------------------------------------------------------------

/// Average power spectrum of a mono signal (Hann window, FFT frames, silent frames left out).
/// → (power per bin, Hz per bin); empty when there was nothing loud enough to measure.
pub fn spectrum(mono: &[f32], rate: u32) -> (Vec<f64>, f64) {
    let bin_hz = rate as f64 / FFT as f64;
    let mut planner = rustfft::FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(FFT);
    let window: Vec<f64> = (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (FFT - 1) as f64).cos()).collect();
    let mut acc = vec![0.0f64; FFT / 2];
    let mut frames = 0usize;
    let mut buf = vec![Complex::new(0.0, 0.0); FFT];
    for chunk in mono.as_chunks::<FFT>().0 {
        let rms = (chunk.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / FFT as f64).sqrt();
        if rms < 1e-4 {
            continue; // silence says nothing about the bandwidth
        }
        for (i, x) in chunk.iter().enumerate() {
            buf[i] = Complex::new(*x as f64 * window[i], 0.0);
        }
        fft.process(&mut buf);
        for (i, a) in acc.iter_mut().enumerate() {
            *a += buf[i].norm_sqr();
        }
        frames += 1;
    }
    if frames == 0 {
        return (vec![], bin_hz);
    }
    for a in acc.iter_mut() {
        *a /= frames as f64;
    }
    (acc, bin_hz)
}

/// Power as dB (never minus infinity).
fn db(p: f64) -> f64 {
    10.0 * (p + 1e-30).log10()
}

/// Mean level (dB) of the band [lo, hi) Hz.
pub fn band_db(power: &[f64], bin_hz: f64, lo: f64, hi: f64) -> f64 {
    let (a, b) = ((lo / bin_hz).ceil() as usize, ((hi / bin_hz).floor() as usize).min(power.len()));
    if b <= a {
        return -300.0;
    }
    db(power[a..b].iter().sum::<f64>() / (b - a) as f64)
}

/// Where a hard shelf cuts the spectrum off, if there is one: a drop of 25 dB or more within 500 Hz
/// with nothing coming back above it. Natural roll-off of old recordings is gradual and is not a shelf.
/// Looked for between 11 kHz and `search_to` Hz.
pub fn find_cutoff(power: &[f64], bin_hz: f64, search_to: f64) -> Option<f64> {
    if power.is_empty() {
        return None;
    }
    let top = (power.len() as f64 * bin_hz).min(search_to + 2500.0);
    // the music itself must be there to judge by
    if band_db(power, bin_hz, 2000.0, 8000.0) < -100.0 {
        return None;
    }
    let step = 250.0;
    let mut f = 11000.0;
    while f + 500.0 <= search_to.min(top - 500.0) {
        let below = band_db(power, bin_hz, f - 750.0, f);
        let just_above = band_db(power, bin_hz, f + 250.0, f + 750.0);
        if below - just_above >= 25.0 {
            // everything above stays down there (500 Hz bands, all of them)
            let mut g = f + 250.0;
            let mut highest = -300.0f64;
            while g + 500.0 <= top {
                highest = highest.max(band_db(power, bin_hz, g, g + 500.0));
                g += 500.0;
            }
            if highest <= below - 25.0 {
                return Some(f);
            }
        }
        f += step;
    }
    None
}

/// A frequency in kHz rounded to the half: `16 kHz`, `19.5 kHz`.
fn khz(hz: f64) -> String {
    let k = (hz / 500.0).round() * 0.5;
    if (k - k.round()).abs() < 0.01 { format!("{} kHz", k.round() as i64) } else { format!("{k:.1} kHz") }
}

/// What the spectrum says about the file's claimed quality.
pub fn spectral_issue(power: &[f64], bin_hz: f64, rate: u32, lossless: bool, bitrate_kbps: i64) -> (Option<Issue>, Option<f64>) {
    let nyq = rate as f64 / 2.0;
    if power.is_empty() {
        return (None, None);
    }
    // a lossy encoder's low-pass sits at 16 – 20 kHz; a CD's own anti-alias filter is above 20.5 kHz
    let cut = find_cutoff(power, bin_hz, (nyq * 0.93).min(24500.0));
    if lossless {
        if let Some(c) = cut {
            if c <= 20300.0 {
                return (Some(Issue { code: "fake_lossless", text: format!("Fake lossless? cutoff {} – transcoded from a lossy file", khz(c)) }), Some(c));
            }
            if rate > 48000 && c <= 24500.0 {
                return (Some(Issue { code: "upsampled", text: format!("Upsampled: {}-kHz file with nothing above {}", (rate as f64 / 1000.0).round(), khz(c)) }), Some(c));
            }
        }
        if rate > 48000 {
            // hi-res with no content at all above 22 kHz (no shelf to find: simply empty)
            let ref_db = band_db(power, bin_hz, 10000.0, 16000.0);
            let hi = band_db(power, bin_hz, 22500.0, nyq.min(40000.0));
            if ref_db > -100.0 && hi < ref_db - 90.0 {
                return (Some(Issue { code: "upsampled", text: format!("Upsampled: {}-kHz file with nothing above 22 kHz", (rate as f64 / 1000.0).round()) }), cut.or(Some(22050.0)));
            }
        }
        return (None, cut);
    }
    // lossy: a "320 kbps" file cut off at 16 kHz was made from a 128 kbps one
    if let Some(c) = cut {
        if bitrate_kbps >= 256 && c <= 16600.0 {
            return (Some(Issue { code: "low_bitrate", text: format!("Re-encoded from a low bitrate: labelled {bitrate_kbps} kbps, cutoff {}", khz(c)) }), Some(c));
        }
    }
    (None, cut)
}

/// Runs of 4 or more samples in a row at full scale, per channel (interleaved input).
pub fn clip_runs(interleaved: &[f32], channels: usize) -> usize {
    let channels = channels.max(1);
    let mut runs = 0;
    for ch in 0..channels {
        let mut run = 0;
        for x in interleaved.iter().skip(ch).step_by(channels) {
            if x.abs() >= 0.9995 {
                run += 1;
                if run == 4 {
                    runs += 1;
                }
            } else {
                run = 0;
            }
        }
    }
    runs
}

/// A `quiet` issue when the loudness is under `min_lufs` (and not silence, and not a quiet genre).
pub fn loudness_issue(lufs: Option<f64>, genre: &str, min_lufs: f64) -> Option<Issue> {
    let l = lufs?;
    let g = genre.to_lowercase();
    if l < min_lufs && l > -70.0 && !QUIET_GENRES.iter().any(|q| g.contains(q)) {
        return Some(Issue { code: "quiet", text: format!("Very quiet ({l:.0} LUFS) – likely a bad rip or a recording off the speakers") });
    }
    None
}

/// A `clipped` issue when there are as many flattened peaks as seconds (in more than 5 s).
pub fn clip_issue(runs: usize, seconds: f64) -> Option<Issue> {
    // a few clipped peaks are normal in loud masters; one every second is not
    (seconds > 5.0 && runs as f64 >= seconds).then(|| Issue { code: "clipped", text: format!("Clipped: {runs} flattened peaks in {} s", seconds.round()) })
}

/// Seconds as `m:ss`.
fn mmss(s: f64) -> String {
    let s = s.abs().round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The file against the length of the recording that was asked for (±7 s).
pub fn length_issue(duration: f64, expected: Option<f64>) -> Option<Issue> {
    let e = expected.filter(|e| *e > 0.0)?;
    if duration <= 0.0 {
        return None;
    }
    let d = duration - e;
    (d.abs() > 7.0).then(|| Issue { code: "wrong_length", text: format!("Wrong version: {} {} than the recording", mmss(d), if d < 0.0 { "shorter" } else { "longer" }) })
}

// ---- measuring a file ----------------------------------------------------------------------------

/// 30 seconds from about a third into the song, as interleaved f32 at the file's own rate. `low`: at low
/// priority (the library scan never competes with playback or downloads).
async fn decode_segment(path: &Path, start: f64, channels: usize, low: bool) -> Option<Vec<f32>> {
    let out = Ffmpeg::new().low_priority_if(low).args(["-v", "error", "-ss", &format!("{start:.2}"), "-t", "30", "-i"]).arg(path)
        .args(["-ac", &channels.to_string(), "-f", "f32le", "-"]).output().await.ok()?;
    if !out.status.success() || out.stdout.len() < FFT * 4 * channels {
        return None;
    }
    Some(out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// Measure one file. `expected` is the length (s) of the recording that was asked for, if known.
pub async fn analyze(media: &Media, path: &Path, expected: Option<f64>, min_lufs: f64, low_priority: bool) -> Report {
    let mut rep = Report::default();
    if !media.ffmpeg_ok() || !path.is_file() {
        return rep;
    }
    // both checks taken (another import, Report a problem, the quality scan): the import says what it waits for
    let busy = media.checks.available_permits() == 0;
    if busy {
        encore_http::progress::waiting("waiting for another sound check to finish");
    }
    let _slot = media.checks.acquire().await;
    if busy {
        encore_http::progress::resume();
    }
    let ps = path.to_string_lossy().to_string();
    let m = media.clone();
    let info = tokio::task::spawn_blocking(move || m.audio_info(&ps)).await.unwrap_or(json!({}));
    let tags = { let ps = path.to_string_lossy().to_string(); tokio::task::spawn_blocking(move || crate::tags::read_tags(&ps)).await.unwrap_or_default() };
    let rate = info["sample_rate"].as_u64().unwrap_or(44100).max(8000) as u32;
    let duration = info["duration"].as_f64().unwrap_or(0.0);
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let lossless = info["lossless"].as_bool().unwrap_or(LOSSLESS.contains(&ext.as_str()));
    let bitrate = info["bitrate"].as_i64().unwrap_or(0);
    let start = if duration > 60.0 { (duration * 0.33).min(duration - 32.0).max(0.0) } else { 0.0 };
    if let Some(pcm) = decode_segment(path, start, 2, low_priority).await {
        let seconds = pcm.len() as f64 / 2.0 / rate as f64;
        let mono: Vec<f32> = pcm.as_chunks::<2>().0.iter().map(|c| (c[0] + c[1]) * 0.5).collect();
        let (power, bin_hz) = tokio::task::spawn_blocking(move || spectrum(&mono, rate)).await.unwrap_or((vec![], 1.0));
        let (issue, cut) = spectral_issue(&power, bin_hz, rate, lossless, bitrate);
        rep.cutoff_hz = cut;
        rep.issues.extend(issue);
        rep.issues.extend(clip_issue(clip_runs(&pcm, 2), seconds));
        rep.checked = true;
    }
    let (lufs, peak) = crate::loudness::measure(path, low_priority).await;
    rep.lufs = lufs;
    rep.true_peak = peak;
    rep.checked |= lufs.is_some();
    rep.issues.extend(loudness_issue(lufs, &tags.s("genre"), min_lufs));
    rep.issues.extend(length_issue(duration, expected));
    rep
}

// ---- import gate, storage, library scan ----------------------------------------------------------

/// Whether imports are checked (`quality_check_on_import`).
pub fn on_import(st: &impl Ctx) -> bool {
    st.cfg().bool("quality_check_on_import")
}

/// The loudness under which a file counts as quiet (`quality_min_lufs`, −28 when 0).
pub fn min_lufs(st: &impl Ctx) -> f64 {
    let v = st.cfg().float("quality_min_lufs");
    if v == 0.0 { -28.0 } else { v }
}

/// Check a download before it enters the library. Err(reason) when it should be turned away.
pub async fn gate(st: &impl Ctx, path: &Path, expected: Option<f64>) -> (Report, Result<(), String>) {
    if !on_import(st) {
        return (Report::default(), Ok(()));
    }
    // background work (an import): ffmpeg at idle priority, so it never competes with the music being played
    encore_http::progress::step("checking the sound quality");
    let rep = analyze(st.media(), path, expected, min_lufs(st), true).await;
    let res = match rep.rejection() {
        Some(i) => Err(i.text.clone()),
        None => Ok(()),
    };
    (rep, res)
}

/// Check the files of a downloaded album: Err(reason) when most of them fail (a fake-lossless rip is
/// usually the whole album). Each file's report comes back for storing once the files are in place.
pub async fn gate_album(st: &impl Ctx, paths: &[std::path::PathBuf]) -> (Vec<(std::path::PathBuf, Report)>, Result<(), String>) {
    if !on_import(st) {
        return (vec![], Ok(()));
    }
    let mut out = vec![];
    for (i, p) in paths.iter().enumerate() {
        encore_http::progress::step(&format!("checking the sound quality ({} of {})", i + 1, paths.len()));
        out.push((p.clone(), analyze(st.media(), p, None, min_lufs(st), true).await));
    }
    let bad: Vec<&Issue> = out.iter().filter_map(|(_, r)| r.rejection()).collect();
    let res = if !bad.is_empty() && bad.len() * 2 >= paths.len().max(1) {
        Err(format!("{} of {} files: {}", bad.len(), paths.len(), bad[0].text))
    } else {
        Ok(())
    };
    (out, res)
}

/// Remember a file's report against its track row (by the path it now has in the library).
pub fn store(st: &impl Ctx, path: &Path, rep: &Report) {
    if !rep.checked {
        return;
    }
    let Ok(Some(t)) = st.db().one("SELECT id, mtime FROM tracks WHERE path=?", p![path.to_string_lossy()]) else { return };
    store_id(st, t.i("id"), t.f("mtime"), rep);
}

/// Store the report of track `id` (and the file time it was made for).
fn store_id(st: &impl Ctx, id: i64, mtime: f64, rep: &Report) {
    let j = rep.to_json();
    let _ = st.db().execute("INSERT OR REPLACE INTO track_quality (track_id, status, issues, summary, cutoff_hz, lufs, true_peak, mtime, checked_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![id, j["status"], j["issues"].to_string(), j["summary"], rep.cutoff_hz, rep.lufs, rep.true_peak, mtime, now()]);
}

/// The stored report of one track (null when it was never checked).
pub fn for_track(st: &impl Ctx, id: i64) -> Value {
    match st.db().one("SELECT * FROM track_quality WHERE track_id=?", p![id]) {
        Ok(Some(q)) => json!({
            "status": q.s("status"), "summary": q.s("summary"), "issues": serde_json::from_str::<Value>(&q.s("issues")).unwrap_or(json!([])),
            "cutoff_hz": q.get("cutoff_hz"), "lufs": q.get("lufs"), "true_peak": q.get("true_peak"), "checked_at": q.get("checked_at"),
        }),
        _ => Value::Null,
    }
}

/// Settings → Scheduled tasks → Quality scan: check library songs that were never checked (or changed
/// since), a batch per run, at low priority. Flags only – nothing is removed.
pub async fn scan(st: &impl Ctx, limit: usize) -> anyhow::Result<String> {
    if !st.media().ffmpeg_ok() {
        anyhow::bail!("needs ffmpeg");
    }
    let rows = st.db().query("SELECT t.id, t.path, t.mtime FROM tracks t LEFT JOIN track_quality q ON q.track_id=t.id
        WHERE q.track_id IS NULL OR q.mtime IS NULL OR abs(q.mtime - t.mtime) > 0.001 ORDER BY t.id LIMIT ?", p![limit as i64])?;
    let (mut n, mut flagged) = (0, 0);
    for t in rows {
        let rep = analyze(st.media(), Path::new(&t.s("path")), None, min_lufs(st), true).await;
        if !rep.checked {
            // unreadable: remember it so the next run moves on
            let _ = st.db().execute("INSERT OR REPLACE INTO track_quality (track_id, status, issues, summary, mtime, checked_at) VALUES (?,?,?,?,?,?)",
                p![t.i("id"), "unchecked", "[]", "", t.f("mtime"), now()]);
            continue;
        }
        n += 1;
        if !rep.issues.is_empty() {
            flagged += 1;
        }
        store_id(st, t.i("id"), t.f("mtime"), &rep);
    }
    let left = st.db().one("SELECT COUNT(*) n FROM tracks t LEFT JOIN track_quality q ON q.track_id=t.id WHERE q.track_id IS NULL", p![])?.map(|r| r.i("n")).unwrap_or(0);
    if flagged > 0 {
        st.db().log("warning", &format!("Quality scan: {flagged} of {n} song(s) look suspect – see Settings → Blocked"));
    }
    Ok(format!("{n} checked, {flagged} suspect{}", if left > 0 { format!(", {left} still to check") } else { String::new() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..n).map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * 0.3
        }).collect()
    }

    /// White noise with everything above `cut` Hz removed (an ideal lossy encoder's low-pass).
    fn brickwall(x: &[f32], rate: u32, cut: f64) -> Vec<f32> {
        let n = x.len().next_power_of_two();
        let mut planner = rustfft::FftPlanner::<f64>::new();
        let (f, i) = (planner.plan_fft_forward(n), planner.plan_fft_inverse(n));
        let mut buf: Vec<Complex<f64>> = (0..n).map(|k| Complex::new(*x.get(k).unwrap_or(&0.0) as f64, 0.0)).collect();
        f.process(&mut buf);
        let hz = rate as f64 / n as f64;
        for (k, c) in buf.iter_mut().enumerate() {
            let fk = (if k <= n / 2 { k } else { n - k }) as f64 * hz;
            if fk > cut {
                *c = Complex::new(0.0, 0.0);
            }
        }
        i.process(&mut buf);
        buf.iter().take(x.len()).map(|c| (c.re / n as f64) as f32).collect()
    }

    #[test]
    fn spectral_shelf_is_found() {
        let x = noise(44100 * 4, 7);
        let (p, hz) = spectrum(&x, 44100);
        let (issue, _) = spectral_issue(&p, hz, 44100, true, 1000);
        assert!(issue.is_none(), "full-band noise is fine: {issue:?}");
        let y = brickwall(&x, 44100, 16000.0);
        let (p, hz) = spectrum(&y, 44100);
        let (issue, cut) = spectral_issue(&p, hz, 44100, true, 1000);
        let issue = issue.expect("a 16 kHz shelf in a FLAC is fake lossless");
        assert_eq!(issue.code, "fake_lossless");
        assert!(issue.text.contains("16 kHz"), "{}", issue.text);
        assert!((cut.unwrap() - 16000.0).abs() <= 500.0);
        // the same shelf in a "320 kbps" MP3: made from a 128 kbps one
        let (issue, _) = spectral_issue(&p, hz, 44100, false, 320);
        assert_eq!(issue.unwrap().code, "low_bitrate");
        assert!(spectral_issue(&p, hz, 44100, false, 128).0.is_none());
    }

    #[test]
    fn upsampled_hires_is_found() {
        let x = noise(96000 * 4, 3);
        let y = brickwall(&x, 96000, 21000.0);
        let (p, hz) = spectrum(&y, 96000);
        assert_eq!(spectral_issue(&p, hz, 96000, true, 4000).0.unwrap().code, "upsampled");
        let (p, hz) = spectrum(&x, 96000);
        assert!(spectral_issue(&p, hz, 96000, true, 4000).0.is_none());
    }

    #[test]
    fn silence_and_clipping() {
        let (p, _) = spectrum(&vec![0.0; 44100], 44100);
        assert!(p.is_empty());
        let mut x = noise(44100 * 2 * 10, 5);
        assert_eq!(clip_runs(&x, 2), 0);
        for k in (0..x.len()).step_by(4000) {
            for j in 0..12 {
                if let Some(v) = x.get_mut(k + j) {
                    *v = 1.0;
                }
            }
        }
        let runs = clip_runs(&x, 2);
        assert!(runs > 100, "{runs}");
        assert_eq!(clip_issue(runs, 10.0).unwrap().code, "clipped");
        assert!(clip_issue(3, 10.0).is_none());
    }

    #[test]
    fn loudness_and_length() {
        let text = "[Parsed_ebur128_0] Summary:\n\n  Integrated loudness:\n    I:         -35.2 LUFS\n    Threshold: -45.0 LUFS\n\n  True peak:\n    Peak:       -9.1 dBFS\n";
        assert_eq!(parse_ebur128(text), (Some(-35.2), Some(-9.1)));
        assert_eq!(loudness_issue(Some(-35.2), "Pop", -28.0).unwrap().code, "quiet");
        assert!(loudness_issue(Some(-35.2), "Classical", -28.0).is_none());
        assert!(loudness_issue(Some(-35.2), "Baroque", -28.0).is_none());
        // shown, never a reason to turn a download away
        assert!(!loudness_issue(Some(-35.2), "", -28.0).unwrap().rejects());
        assert!(loudness_issue(Some(-14.0), "", -28.0).is_none());
        let l = length_issue(216.0, Some(240.0)).unwrap();
        assert_eq!(l.text, "Wrong version: 0:24 shorter than the recording");
        assert!(length_issue(236.0, Some(240.0)).is_none());
        assert!(length_issue(236.0, None).is_none());
    }

    fn ffmpeg(args: &[&str]) -> bool {
        std::process::Command::new("ffmpeg").args(["-v", "error", "-y"]).args(args).status().is_ok_and(|s| s.success())
    }

    #[tokio::test]
    async fn synthetic_files() {
        let media = Media::default();
        if !media.ffmpeg_ok() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("encore-quality-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (fake, full, quiet) = (dir.join("fake.flac"), dir.join("full.flac"), dir.join("quiet.flac"));
        assert!(ffmpeg(&["-f", "lavfi", "-i", "anoisesrc=d=8:c=white:a=0.3:r=44100", "-af", "firequalizer=gain='if(lt(f,16000),0,-200)'", "-ac", "2", fake.to_str().unwrap()]));
        assert!(ffmpeg(&["-f", "lavfi", "-i", "anoisesrc=d=8:c=white:a=0.3:r=44100", "-ac", "2", full.to_str().unwrap()]));
        assert!(ffmpeg(&["-f", "lavfi", "-i", "anoisesrc=d=8:c=pink:a=0.1:r=44100", "-ac", "2", quiet.to_str().unwrap()]));
        let r = analyze(&media, &fake, None, -28.0, false).await;
        assert!(r.issues.iter().any(|i| i.code == "fake_lossless"), "{:?}", r.issues);
        let r = analyze(&media, &full, None, -28.0, false).await;
        assert!(r.checked && r.issues.is_empty(), "{:?}", r.issues);
        let r = analyze(&media, &full, Some(40.0), -28.0, false).await;
        assert_eq!(r.rejection().unwrap().code, "wrong_length");
        let r = analyze(&media, &quiet, None, -28.0, false).await;
        assert!(r.issues.iter().any(|i| i.code == "quiet"), "{:?} {:?}", r.lufs, r.issues);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
