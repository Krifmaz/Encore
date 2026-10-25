//! Tests of the domain crates that need a whole server (a database, the settings, the HTTP client): they run
//! here, on a [`crate::test_support::TestServer`].
mod album_types;
mod download_now;
mod downloads;
mod github;
mod loudness;
mod musicbrainz;
mod read_only_file;
mod rest_of_album;
mod plugins;
mod rejected;
mod soulseek;
mod stall;
mod storage;
mod surround;
mod streamrip;
mod torrents;
mod translate_tiers;
mod ytauth;
