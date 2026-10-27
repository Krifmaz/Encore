//! The music library: scanning files into the database ([`scan`], over the library's folders: [`folders`]),
//! artists and their spellings ([`artists`]), searching the library and the catalogues ([`search`],
//! [`nlsearch`]), telling versions of a song apart ([`matcher`]), fingerprints ([`acoustid`]), tagging and
//! importing ([`tagger`], [`meta`]), lyrics, duplicates, organizing folders, the side index of the library
//! lists ([`sections`]), each song's loudness for volume normalization ([`loudness`]), and giving
//! fingerprints and ISRCs back to AcoustID and MusicBrainz ([`contribute`]), and the Surround library of Dolby Atmos and
//! multichannel versions, kept apart from the stereo one ([`surround`]), and the recommendations made from the
//! listeners' plays, likes, skips and dislikes ([`taste`]).
//!
//! Functions that need the server's database, settings, HTTP client or caches take a [`Ctx`], which the
//! server's state implements.
pub mod acoustid;
pub mod album_types;
pub mod artists;
pub mod catalog;
pub mod contribute;
pub mod dupes;
pub mod file_names;
pub mod folders;
pub mod lists;
pub mod live_copies;
pub mod lookup;
pub mod loudness;
pub mod lyrics;
pub mod merge;
pub mod matcher;
pub mod meta;
pub mod nlsearch;
pub mod organize;
pub mod own_file;
pub mod rest_of_album;
pub mod scan;
pub mod search;
pub mod sections;
pub mod split;
pub mod surround;
pub mod taste;
pub mod tagger;
pub mod translate;
pub mod twins;
pub mod video_title;
pub mod ytmusic;

pub use scan::*;

/// What the library functions need from the server.
pub trait Ctx: encore_media::Ctx {
    /// The library state the server keeps.
    fn library(&self) -> &Library;
    /// progress of the long jobs (verify, organize …)
    fn jobs(&self) -> &encore_jobs::Jobs;
    /// A song left the library (its file deleted or replaced): what the server keeps about it elsewhere
    /// (Cadenza's renders of it) goes too.
    fn track_removed(&self, id: i64);
}
