//! Tests of the domain crates that need a whole server (a database, the settings, the HTTP client): they run
//! here, on a [`crate::test_support::TestServer`].
mod download_now;
mod downloads;
mod github;
mod loudness;
mod plugins;
mod rejected;
mod soulseek;
mod storage;
mod surround;
mod streamrip;
mod torrents;
mod translate_tiers;
mod ytauth;
