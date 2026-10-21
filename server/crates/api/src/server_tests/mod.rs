//! Tests of the domain crates that need a whole server (a database, the settings, the HTTP client): they run
//! here, on a [`crate::test_support::TestServer`].
mod github;
mod loudness;
mod plugins;
mod rejected;
mod soulseek;
mod storage;
mod streamrip;
mod torrents;
mod ytauth;
