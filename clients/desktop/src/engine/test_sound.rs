//! "Test sound": a two-second tone written to a WAV file in the app's own cache folder and
//! played through the engine, so the chosen output and its settings can be heard.

use super::{Command, Track};
use cadenza::decode::Source;
use std::path::PathBuf;

/// Sample rate of the tone file.
pub const RATE: u32 = 48_000;
/// Length in seconds.
pub const SECONDS: f64 = 2.0;
/// Pitch in Hz (A4).
pub const PITCH: f64 = 440.0;
/// Level in dBFS.
pub const LEVEL_DB: f64 = -12.0;

/// The samples of the tone: a sine with 20 ms fades at both ends, one channel.
pub fn samples() -> Vec<i16> {
    let n = (RATE as f64 * SECONDS) as usize;
    let fade = (RATE as f64 * 0.02) as usize;
    let amp = 10f64.powf(LEVEL_DB / 20.0) * i16::MAX as f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            let env = if i < fade {
                i as f64 / fade as f64
            } else if i + fade > n {
                (n - i) as f64 / fade as f64
            } else {
                1.0
            };
            (amp * env * (2.0 * std::f64::consts::PI * PITCH * t).sin()).round() as i16
        })
        .collect()
}

/// The tone's file name.
const FILE: &str = "encore-desktop-test-sound.wav";

/// Write the tone to `<config dir>/encore-desktop/cache/encore-desktop-test-sound.wav` (stereo,
/// 16-bit) and return the path. A per-user folder, not the shared temp folder: there the file of
/// one user cannot be rewritten by the next, and a fixed name is a symlink target.
pub fn write() -> Result<PathBuf, String> {
    write_in(&crate::config::dir().join("cache"))
}

/// Write the tone to `<dir>/encore-desktop-test-sound.wav` (the folder is created).
fn write_in(dir: &std::path::Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not write the test sound: {e}"))?;
    let path = dir.join(FILE);
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).map_err(|e| format!("Could not write the test sound: {e}"))?;
    for s in samples() {
        for _ in 0..2 {
            w.write_sample(s).map_err(|e| format!("Could not write the test sound: {e}"))?;
        }
    }
    w.finalize().map_err(|e| format!("Could not write the test sound: {e}"))?;
    Ok(path)
}

/// The tone as a queue entry.
pub fn track(path: PathBuf) -> Track {
    Track {
        id: None,
        source: Source::Path { path },
        title: "Test sound".into(),
        artist: "Encore".into(),
        album: String::new(),
        album_id: None,
        duration: Some(SECONDS),
        format: Some("WAV".into()),
        cover: None,
    }
}

/// Whether a queue entry is the tone (a queue of only the tone must not start the library
/// shuffle when it ends).
pub fn is_tone(t: &Track) -> bool {
    matches!(&t.source, Source::Path { path } if path.file_name().is_some_and(|n| n == FILE))
}

/// Play the tone now. With nothing in the queue it becomes the queue; while music plays it is
/// put in front and played at once, and the queue carries on after it.
pub fn play() -> Result<(), String> {
    let path = write()?;
    let e = super::start()?;
    let st = e.status();
    let t = track(path);
    if st.queue.is_empty() || st.index.is_none() {
        e.play_queue(vec![t], 0, 0.0);
    } else {
        e.send(Command::PlayNext(vec![t]));
        e.send(Command::Next);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tone_is_two_seconds_with_fades() {
        let s = samples();
        assert_eq!(s.len(), (RATE as f64 * SECONDS) as usize);
        assert_eq!(s[0], 0);
        assert_eq!(*s.last().unwrap(), 0);
        let peak = s.iter().map(|x| x.unsigned_abs()).max().unwrap() as f64;
        let expected = 10f64.powf(LEVEL_DB / 20.0) * i16::MAX as f64;
        assert!((peak - expected).abs() < expected * 0.02, "peak {peak} vs {expected}");
        let d = tempfile::tempdir().unwrap();
        let p = write_in(&d.path().join("cache")).unwrap();
        assert!(p.exists());
        let t = track(p.clone());
        assert_eq!(t.title, "Test sound");
        assert!(matches!(t.source, Source::Path { .. }));
        assert!(is_tone(&t));
        let song = Track {
            source: Source::Path { path: std::env::temp_dir().join("song.flac") },
            ..track(p.clone())
        };
        assert!(!is_tone(&song), "a song file is not the tone");
        let _ = std::fs::remove_file(p);
    }
}
