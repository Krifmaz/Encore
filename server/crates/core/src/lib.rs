//! State-free helpers every Encore crate shares: the release version, the API error type, ids,
//! text escaping and folding, reading JSON as text, the audio-format table, finding programs on PATH,
//! the clock and civil dates, version numbers, file systems and scratch folders for tests, file steps that
//! name their path and errno ([`fsx`]), and the byte-bounded cache the server's caches use.
pub mod audio;
pub mod cache;
pub mod error;
pub mod fs;
pub mod fsinfo;
pub mod fsx;
pub mod ids;
pub mod json;
pub mod text;
pub mod time;
pub mod tmp;
pub mod version;
pub mod which;

/// Release version (e.g. 2026.10.11.1), set by the release build; local builds use Cargo's.
pub const VERSION: &str = match option_env!("ENCORE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
