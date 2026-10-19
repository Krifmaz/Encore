//! The music library: scanning files into the database ([`scan`]), artists and their spellings
//! ([`artists`]), searching the library and the catalogues ([`search`], [`nlsearch`]), telling versions of
//! a song apart ([`matcher`]), fingerprints ([`acoustid`]), tagging and importing ([`tagger`], [`meta`]),
//! lyrics, duplicates, organizing folders, the side index of the library lists ([`sections`]), and giving
//! fingerprints and ISRCs back to AcoustID and MusicBrainz ([`contribute`]).
//!
//! Functions that need the server's database, settings, HTTP client or caches take a [`Ctx`], which the
//! server's state implements.
pub mod acoustid;
pub mod artists;
pub mod catalog;
pub mod contribute;
pub mod dupes;
pub mod lists;
pub mod lookup;
pub mod lyrics;
pub mod matcher;
pub mod meta;
pub mod nlsearch;
pub mod organize;
pub mod scan;
pub mod search;
pub mod sections;
pub mod tagger;
pub mod translate;
pub mod ytmusic;

pub use scan::*;

/// What the library functions need from the server.
pub trait Ctx: encore_media::Ctx {
    fn library(&self) -> &Library;
    /// progress of the long jobs (verify, organize …)
    fn jobs(&self) -> &encore_jobs::Jobs;
    /// A song left the library (its file deleted or replaced): what the server keeps about it elsewhere
    /// (Cadenza's renders of it) goes too.
    fn track_removed(&self, id: i64);
}
