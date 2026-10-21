//! Audio files and pictures: one ffmpeg command builder ([`ffmpeg::Ffmpeg`]), reading and writing tags
//! ([`tags`]), what a file holds ([`probe`]), album covers and their thumbnails ([`covers`]), and the
//! quality and loudness checks ([`quality`], [`loudness`]).
//!
//! Functions that need the server's database, settings or caches take a [`Ctx`], which the server's
//! state implements.
use encore_db::Db;
use std::path::Path;
use std::sync::{Arc, OnceLock};

pub mod covers;
pub mod ffmpeg;
pub mod loudness;
pub mod probe;
pub mod quality;
pub mod tags;

pub use covers::CoverIndex;
pub use ffmpeg::Ffmpeg;

/// What the media functions need from the server.
pub trait Ctx: encore_http::Ctx + Clone + 'static {
    fn db(&self) -> &Db;
    fn media(&self) -> &Media;
    /// album → cover file and version
    fn covers(&self) -> &CoverIndex;
    /// The server's config folder (database, caches, covers).
    fn data_dir(&self) -> &Path {
        &self.db().data_dir
    }
    /// The plugins (encore_plugins): what is installed and switched on, and where a builtin plugin's program
    /// is. Every crate above this one reaches them here.
    fn plugins(&self) -> &encore_plugins::Registry;
}

/// What the media helpers keep while the server runs. Clones share it.
#[derive(Clone)]
pub struct Media {
    /// `audio_info` answers per file
    pub probe: probe::Probe,
    /// the quality check runs ffmpeg at most twice at a time (imports and the scan share the slots)
    checks: Arc<tokio::sync::Semaphore>,
    /// whether ffmpeg runs at all (asked once)
    ffmpeg: Arc<OnceLock<bool>>,
}

impl Default for Media {
    fn default() -> Self {
        Media { probe: Default::default(), checks: Arc::new(tokio::sync::Semaphore::new(2)), ffmpeg: Default::default() }
    }
}

impl Media {
    /// What's in the file (codec, sample rate, bit depth, channels, bitrate, duration), kept per file and mtime.
    pub fn audio_info(&self, path: &str) -> serde_json::Value {
        self.probe.audio_info(path)
    }

    /// ffmpeg is installed and runs (asked the first time only).
    pub fn ffmpeg_ok(&self) -> bool {
        *self.ffmpeg.get_or_init(ffmpeg::available)
    }
}
