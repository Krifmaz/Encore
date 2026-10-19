//! Loudness (EBU R128): integrated loudness and true peak of a file, measured by ffmpeg's ebur128 filter.
use crate::ffmpeg::Ffmpeg;
use std::path::Path;

/// (integrated loudness in LUFS, true peak in dBFS) of the whole file; (None, None) when ffmpeg can't read it.
pub async fn measure(path: &Path, low_priority: bool) -> (Option<f64>, Option<f64>) {
    let run = Ffmpeg::new().low_priority_if(low_priority).args(["-nostats", "-i"]).arg(path).args(["-filter_complex", "ebur128=peak=true", "-f", "null", "-"]);
    let Ok(out) = run.output().await else { return (None, None) };
    parse_ebur128(&String::from_utf8_lossy(&out.stderr))
}

/// The summary ebur128 prints at the end: (I in LUFS, true peak in dBFS).
pub fn parse_ebur128(text: &str) -> (Option<f64>, Option<f64>) {
    let summary = text.rsplit("Summary:").next().unwrap_or("");
    let num = |key: &str| summary.lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix(key).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse::<f64>().ok())
    });
    (num("I:"), num("Peak:"))
}
