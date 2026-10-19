//! Connections to other services and the sync with them: the connections data model and sign-ins
//! ([`connections`]), scrobbling ([`scrobble`]), playlists and liked songs ([`playlists`]), and the sync
//! engine that pulls likes and playlists in and pushes them out ([`engine`]). Reading playlist links
//! (Spotify, Deezer …) is the server's: some links need its download tools ([`Ctx::fetch_link`]).
//!
//! Functions that need the server take a [`Ctx`], which the server's state implements.
use encore_db::Row;
use serde_json::Value;
use std::future::Future;

pub mod connections;
pub mod engine;
pub mod playlists;
pub mod scrobble;
pub mod ytmusic;

pub use engine::*;
pub use playlists::MissingReport;

/// What the sync needs from the server.
pub trait Ctx: encore_library::Ctx {
    /// Request every song of playlist `playlist` that isn't in the library yet, as `owner` (the request
    /// queue is the server's). `auto`: the auto-download rules asked, so no approval is needed.
    fn request_missing(&self, owner: &Row, playlist: i64, auto: bool, album_mode: Option<String>) -> anyhow::Result<MissingReport>;
    /// The tracks behind a playlist link (Spotify, Deezer, Tidal, Apple Music, Amazon, ListenBrainz, YouTube …),
    /// with `user`'s own connections when given. Reading links is the server's: some need its download tools.
    fn fetch_link(&self, url: &str, user: Option<i64>) -> impl Future<Output = anyhow::Result<Value>> + Send;
    /// A Spotify Web API list with `token` (`kind` "collection": the signed-in user's liked songs).
    fn spotify_api(&self, token: &str, kind: &str, id: &str) -> impl Future<Output = anyhow::Result<Value>> + Send;
}
