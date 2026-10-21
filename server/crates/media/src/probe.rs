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

/// [`Probe::audio_info`] without the table: reads the file every time. A file lofty can't read the codec of (an
/// .m4a holding E-AC-3, Dolby Atmos among them) or can't open at all (.mka, .ac3, .eac3, .thd, .dts) is asked of
/// ffprobe, so the stream plan never sends a browser E-AC-3 labelled as AAC.
pub fn read_audio_info(path: &str) -> Value {
    use lofty::file::AudioFile;
    use lofty::prelude::*;
    let Ok(f) = lofty::read_from_path(path) else {
        let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if SURROUND_EXTS.contains(&ext.as_str()) {
            if let Some(s) = stream_info(path) {
                return s.audio_info();
            }
        }
        return json!({});
    };
    if matches!(f.file_type(), lofty::file::FileType::Mp4) && mp4_codec_unknown(path) {
        if let Some(s) = stream_info(path) {
            return s.audio_info();
        }
    }
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

/// The extensions of surround audio lofty can't open (Matroska audio, raw AC-3, E-AC-3, TrueHD and DTS).
pub const SURROUND_EXTS: &[&str] = &["mka", "ac3", "eac3", "ec3", "thd", "dts", "mlp"];

/// An .m4a whose sample entry lofty does not know (E-AC-3, AC-3): its codec comes from ffprobe.
fn mp4_codec_unknown(path: &str) -> bool {
    use lofty::file::AudioFile;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    lofty::mp4::Mp4File::read_from(&mut f, lofty::config::ParseOptions::new()).map(|m| m.properties().codec().is_none()).unwrap_or(false)
}

/// The first audio stream of a file, as ffprobe reads it: what a surround file is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stream {
    /// ffprobe's codec name: eac3, ac3, truehd, dts, flac, alac, aac, opus, mp3 …
    pub codec: String,
    /// "Dolby Digital Plus + Dolby Atmos", "Dolby TrueHD + Dolby Atmos", "DTS-HD MA" … (empty when none)
    pub profile: String,
    pub channels: u32,
    /// "5.1(side)", "7.1", "stereo" …
    pub layout: String,
    pub sample_rate: u32,
    /// bits per second, 0 when ffprobe does not say
    pub bitrate: u32,
    pub bit_depth: u32,
    pub duration: f64,
}

impl Stream {
    /// Dolby Atmos (E-AC-3 JOC or TrueHD with Atmos): ffprobe 6.1 and newer name it in the profile.
    pub fn atmos(&self) -> bool {
        self.profile.to_lowercase().contains("atmos")
    }

    /// More than two channels, or Atmos: a file of the Surround library, never of the stereo one.
    pub fn is_surround(&self) -> bool {
        self.channels > 2 || self.atmos()
    }

    pub fn lossless(&self) -> bool {
        matches!(self.codec.as_str(), "flac" | "alac" | "truehd" | "mlp" | "wavpack" | "ape") || self.codec.starts_with("pcm_") || self.profile.contains("DTS-HD MA")
    }

    /// What a player shows: "Dolby Atmos (E-AC-3 5.1)", "TrueHD 7.1", "FLAC 5.1".
    pub fn label(&self) -> String {
        let name = match self.codec.as_str() {
            "eac3" => "E-AC-3",
            "ac3" => "AC-3",
            "truehd" => "TrueHD",
            "dts" => "DTS",
            "mlp" => "MLP",
            other => other,
        };
        let layout = layout_short(&self.layout, self.channels);
        if self.atmos() {
            format!("Dolby Atmos ({} {layout})", name.to_uppercase().replace("TRUEHD", "TrueHD"))
        } else {
            format!("{} {layout}", if name.len() <= 4 { name.to_uppercase() } else { name.to_string() })
        }
    }

    /// In the shape of [`read_audio_info`].
    pub fn audio_info(&self) -> Value {
        json!({
            "codec": self.codec, "sample_rate": self.sample_rate, "bit_depth": if self.bit_depth > 0 { Some(self.bit_depth) } else { None },
            "channels": self.channels, "bitrate": if self.bitrate > 0 { Some(self.bitrate / 1000) } else { None }, "duration": self.duration,
            "lossless": self.lossless(), "layout": self.layout, "atmos": self.atmos(),
        })
    }
}

/// "5.1(side)" → "5.1"; no layout: the channel count ("6ch").
fn layout_short(layout: &str, channels: u32) -> String {
    let l = layout.split('(').next().unwrap_or("").trim();
    if l.is_empty() || l == "unknown" { format!("{channels}ch") } else { l.to_string() }
}

/// ffprobe's answer about the first audio stream (None when ffprobe is missing or the file has no audio).
pub fn stream_info(path: &str) -> Option<Stream> {
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries",
               "stream=codec_name,profile,channels,channel_layout,sample_rate,bit_rate,bits_per_raw_sample,duration:format=duration,bit_rate",
               "-of", "json"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ffprobe(&String::from_utf8_lossy(&out.stdout))
}

/// A surround file's stream, or None for a stereo or mono one (and a file ffprobe can't read).
pub fn surround_of(path: &str) -> Option<Stream> {
    stream_info(path).filter(Stream::is_surround)
}

/// ffprobe's JSON (`-show_entries stream=…:format=… -of json`) → the first audio stream.
pub fn parse_ffprobe(text: &str) -> Option<Stream> {
    let v: Value = serde_json::from_str(text).ok()?;
    let st = v["streams"].as_array()?.first()?;
    let num = |x: &Value| -> f64 { x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0) };
    let s = |x: &Value| x.as_str().unwrap_or("").to_string();
    Some(Stream {
        codec: s(&st["codec_name"]),
        profile: s(&st["profile"]),
        channels: num(&st["channels"]) as u32,
        layout: s(&st["channel_layout"]),
        sample_rate: num(&st["sample_rate"]) as u32,
        bitrate: [num(&st["bit_rate"]), num(&v["format"]["bit_rate"])].into_iter().find(|b| *b > 0.0).unwrap_or(0.0) as u32,
        bit_depth: num(&st["bits_per_raw_sample"]) as u32,
        duration: [num(&st["duration"]), num(&v["format"]["duration"])].into_iter().find(|d| *d > 0.0).unwrap_or(0.0),
    })
}

#[cfg(test)]
mod surround_tests {
    use super::*;

    #[test]
    fn ffprobe_answers_are_read() {
        // ffprobe 7.1 on an Apple Music Atmos .m4a (E-AC-3 JOC)
        let atmos = r#"{"programs":[],"stream_groups":[],"streams":[{"codec_name":"eac3","profile":"Dolby Digital Plus + Dolby Atmos","sample_rate":"48000","channels":6,"channel_layout":"5.1(side)","bit_rate":"768000","duration":"215.338667"}],"format":{"duration":"215.338667","bit_rate":"770123"}}"#;
        let s = parse_ffprobe(atmos).unwrap();
        assert!(s.atmos() && s.is_surround() && !s.lossless());
        assert_eq!((s.channels, s.bitrate, s.label()), (6, 768000, "Dolby Atmos (E-AC-3 5.1)".to_string()));
        assert_eq!(s.audio_info()["codec"], "eac3");
        assert_eq!(s.audio_info()["bitrate"], 768);
        let truehd = r#"{"streams":[{"codec_name":"truehd","profile":"Dolby TrueHD + Dolby Atmos","sample_rate":"48000","channels":8,"channel_layout":"7.1"}],"format":{"duration":"300.0"}}"#;
        let t = parse_ffprobe(truehd).unwrap();
        assert!(t.lossless() && t.atmos());
        assert_eq!(t.label(), "Dolby Atmos (TrueHD 7.1)");
        let flac = r#"{"streams":[{"codec_name":"flac","sample_rate":"96000","channels":6,"channel_layout":"5.1","bits_per_raw_sample":"24"}],"format":{}}"#;
        let f = parse_ffprobe(flac).unwrap();
        assert!(f.is_surround() && f.lossless() && !f.atmos());
        assert_eq!((f.label(), f.bit_depth), ("FLAC 5.1".to_string(), 24));
        let stereo = r#"{"streams":[{"codec_name":"aac","sample_rate":"44100","channels":2,"channel_layout":"stereo"}],"format":{}}"#;
        assert!(!parse_ffprobe(stereo).unwrap().is_surround());
        assert!(parse_ffprobe(r#"{"streams":[]}"#).is_none());
    }

    #[test]
    fn an_m4a_with_e_ac_3_reads_as_e_ac_3() {
        if encore_core::which::which("ffmpeg").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("encore-surround-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("five-one.m4a");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-ac", "6", "-c:a", "eac3", "-b:a", "384k", "-f", "mp4"])
            .arg(&f)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            panic!("ffmpeg could not write a 5.1 E-AC-3 .m4a");
        }
        let p = f.to_string_lossy();
        let info = read_audio_info(&p);
        assert_eq!((info["codec"].as_str(), info["channels"].as_u64()), (Some("eac3"), Some(6)), "{info}");
        assert_eq!(info["lossless"], false);
        let s = surround_of(&p).unwrap();
        assert_eq!((s.codec.as_str(), s.channels, s.atmos()), ("eac3", 6, false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
