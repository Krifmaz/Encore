//! Tests of the domain crates that need a whole server (a database, the settings, the HTTP client): they run
//! here, on a [`crate::test_support::TestServer`].
mod github;
mod soulseek;
mod storage;
mod streamrip;
mod ytauth;
