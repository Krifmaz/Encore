//! The audio formats Encore knows: the file extensions the library scan picks up, which formats are
//! lossless, the content type a format is served as (and which of them a browser plays), and the codec a
//! file's extension stands for.

/// Extensions of the files the library scan takes as songs (lowercase, without the dot).
pub const AUDIO_EXTS: &[&str] = &["flac", "mp3", "m4a", "mp4", "aac", "ogg", "opus", "wav", "aiff", "aif", "wma", "ape", "wv", "alac"];

/// Lossless formats, by extension or by the format name a song row carries.
pub const LOSSLESS: [&str; 7] = ["flac", "alac", "wav", "aiff", "aif", "ape", "wv"];

/// Extensions a download counts as songs: a Soulseek search or folder, a finished torrent or Usenet
/// download, a file a download client left behind. The scan's list without mp4 and aif.
pub const DOWNLOAD_EXTS: [&str; 12] = ["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "alac", "aiff", "wma", "ape", "wv"];
/// The lossless ones among [`DOWNLOAD_EXTS`].
pub const DOWNLOAD_LOSSLESS: [&str; 6] = ["flac", "wav", "alac", "aiff", "ape", "wv"];
/// What streamrip writes (Qobuz, Tidal and Deezer downloads).
pub const STREAMRIP_EXTS: [&str; 8] = ["flac", "mp3", "m4a", "alac", "ogg", "opus", "wav", "aiff"];
/// What yt-dlp writes (YouTube and the other sites it knows).
pub const YTDLP_EXTS: [&str; 8] = ["mp3", "m4a", "opus", "ogg", "flac", "webm", "wav", "aac"];

/// The content type browsers play a format as; None for a format they cannot play.
pub fn mime(fmt: &str) -> Option<&'static str> {
    Some(match fmt {
        "mp3" => "audio/mpeg", "flac" => "audio/flac", "m4a" | "mp4" => "audio/mp4", "aac" => "audio/aac",
        "ogg" | "opus" | "oga" => "audio/ogg", "wav" => "audio/wav", "webm" => "audio/webm",
        _ => return None,
    })
}

/// The content type a file of this format is served as: [`mime`] for the ones browsers play, and the ones
/// only players and Cadenza read (DSD, AIFF, WavPack, APE); None for the rest (WMA, a bare .alac).
pub fn content_type(fmt: &str) -> Option<&'static str> {
    mime(fmt).or(match fmt {
        "dsf" => Some("audio/x-dsf"), "dff" => Some("audio/x-dff"), "aiff" | "aif" => Some("audio/aiff"), "wv" => Some("audio/x-wavpack"),
        "ape" => Some("audio/x-ape"),
        _ => None,
    })
}

/// The codec a file of this extension holds when nothing better is known, in the decoders' words: .m4a and
/// .mp4 count as AAC, .ogg as Vorbis, .aif as AIFF, .wv as WavPack; other extensions name their codec.
pub fn codec_of_ext(fmt: &str) -> &str {
    match fmt {
        "m4a" | "mp4" => "aac", "ogg" | "oga" => "vorbis", "aif" => "aiff", "wv" => "wavpack",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_agrees_with_itself() {
        // every lossless format the scan picks up is a song file
        for f in LOSSLESS {
            assert!(AUDIO_EXTS.contains(&f), "{f}");
        }
        for f in DOWNLOAD_LOSSLESS {
            assert!(DOWNLOAD_EXTS.contains(&f) && LOSSLESS.contains(&f), "{f}");
        }
        for f in DOWNLOAD_EXTS {
            assert!(AUDIO_EXTS.contains(&f), "{f}");
        }
        assert_eq!(mime("flac"), Some("audio/flac"));
        assert_eq!(mime("alac"), None);
        // every song format has a content type except WMA and a bare .alac; the browser types are a part of them
        for f in AUDIO_EXTS {
            assert_eq!(content_type(f).is_some(), !["wma", "alac"].contains(f), "{f}");
            if let Some(m) = mime(f) {
                assert_eq!(content_type(f), Some(m), "{f}");
            }
        }
        for f in ["dsf", "dff"] {
            assert!(mime(f).is_none() && content_type(f).is_some(), "{f}");
        }
        // the codec of an extension: the same word for the two spellings of one format
        assert_eq!(codec_of_ext("aif"), codec_of_ext("aiff"));
        assert_eq!(codec_of_ext("mp4"), codec_of_ext("m4a"));
        assert_eq!(codec_of_ext("oga"), codec_of_ext("ogg"));
        assert_eq!((codec_of_ext("wv"), codec_of_ext("flac"), codec_of_ext("dsf")), ("wavpack", "flac", "dsf"));
    }
}
