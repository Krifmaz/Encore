//! Tests of the domain crates that need a whole server (a database, the settings, the HTTP client): they run
//! here, on a [`crate::test_support::TestServer`].
mod album_types;
mod download_now;
mod downloads;
mod github;
mod loudness;
mod musicbrainz;
mod plugins;
mod quality_upgrade;
mod read_only_file;
mod rejected;
mod rest_of_album;
mod soulseek;
mod stall;
mod storage;
mod streamrip;
mod surround;
mod torrents;
mod translate_tiers;
mod ytauth;
