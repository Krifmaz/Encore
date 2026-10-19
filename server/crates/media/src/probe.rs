//! What an audio file holds, for the player's signal path, the stream decision, the quality check and
//! Cadenza: codec, sample rate, bit depth, channels, bitrate, duration, lossless or not (read with lofty).
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Answers kept per file at most (the table is cleared when full: one in a long scan over distinct files).
const MAX: usize = 4096;

/// `audio_info` answers kept per file and mtime: the stream, the signal path, the quality check and Cadenza
/// ask about the same file again and again. Clones share the table.
#[derive(Clone, Default)]
pub struct Probe(Arc<Mutex<HashMap<PathBuf, (SystemTime, Value)>>>);

impl Probe {
    /// What's in the file: codec, sample rate, bit depth, channels, bitrate. Read once per file and mtime
    /// (`{}` for a file that isn't there, which is never kept).
    pub fn audio_info(&self, path: &str) -> Value {
        let Ok(mtime) = std::fs::metadata(path).and_then(|m| m.modified()) else { return json!({}) };
        let key = PathBuf::from(path);
        if let Some((at, v)) = self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if *at == mtime {
                return v.clone();
            }
        }
        let v = read_audio_info(path);
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() >= MAX && !m.contains_key(&key) {
            m.clear();
        }
        m.insert(key, (mtime, v.clone()));
        v
    }
}

/// [`Probe::audio_info`] without the table: reads the file every time.
pub fn read_audio_info(path: &str) -> Value {
    use lofty::file::AudioFile;
    use lofty::prelude::*;
    let Ok(f) = lofty::read_from_path(path) else { return json!({}) };
    let p = f.properties();
    let ft = format!("{:?}", f.file_type()).to_lowercase();
    // lofty's file type as a format of the audio table (encore_core::audio)
    let fmt = match ft.as_str() {
        "mpeg" => "mp3", "wavpack" => "wv", "mp4" => if is_alac(path) { "alac" } else { "m4a" },
        other => other,
    };
    let lossless = encore_core::audio::LOSSLESS.contains(&fmt);
    let codec = match ft.as_str() {
        "mpeg" => "mp3", "mp4" => if lossless { "alac" } else { "aac" }, "vorbis" => "ogg vorbis", "wavpack" => "wavpack", "ape" => "ape",
        other => other,
    };
    json!({
        "codec": codec,
        "sample_rate": p.sample_rate(), "bit_depth": p.bit_depth(), "channels": p.channels(),
        "bitrate": p.audio_bitrate().or(p.overall_bitrate()), "duration": p.duration().as_secs_f64(), "lossless": lossless,
    })
}

/// Apple Lossless hides inside .m4a – browsers can't play it.
pub fn is_alac(path: &str) -> bool {
    use lofty::file::AudioFile;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    lofty::mp4::Mp4File::read_from(&mut f, lofty::config::ParseOptions::new())
        .map(|m| matches!(m.properties().codec(), Some(lofty::mp4::Mp4Codec::ALAC)))
        .unwrap_or(false)
}
