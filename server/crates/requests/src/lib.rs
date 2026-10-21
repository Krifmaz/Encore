//! Requests: what people ask for and how it gets into the library. A request is created ([`create`]),
//! approved, then worked on by the [`worker`] stage by stage (queue, pick a source, fetch, verify,
//! import) through the download [`sources`] (Soulseek through the managed engine in [`slskd_managed`]
//! and its server session in [`slskd_session`], torrents and Usenet through the download clients,
//! streamrip, yt-dlp with the YouTube sign-in of [`ytauth`]). Pages follow the requests live through [`events`]; [`report`] handles a song reported as
//! wrong (its source blocked, a replacement requested); [`missing`] requests what a playlist lacks;
//! [`upkeep`] queues lossless upgrades and clears old requests; [`notify`] tells the admin's channels.
//!
//! Functions that need the server take a [`Ctx`], which the server's state implements.
use std::sync::Arc;
use tokio::sync::Notify;

pub mod create;
pub mod events;
pub mod line;
pub mod missing;
pub mod notify;
pub mod report;
pub mod search_cache;
pub mod slskd_managed;
pub mod slskd_session;
pub mod sources;
pub mod upkeep;
pub mod worker;
pub mod ytauth;
#[cfg(test)]
pub(crate) mod test_ctx;

pub use create::{create_or_reuse, create_request, NewRequest};

/// What the request functions need from the server.
pub trait Ctx: encore_sync::Ctx + encore_system::Ctx {
    /// The request queue's running tasks, its event stream, the download slots and what the helper
    /// programs (slskd, the YouTube sign-in, streamrip's Tidal sign-in) keep while the server runs.
    fn requests(&self) -> &Requests;
    /// The VPN tunnel Soulseek runs in.
    fn vpn(&self) -> &encore_vpn::Vpn;
}

/// What the request queue keeps while the server runs. Clones share it.
#[derive(Clone, Default)]
pub struct Requests {
    /// the request tasks running now, so a cancel can stop them
    pub tasks: worker::Tasks,
    /// the live request events of open pages
    pub events: events::Events,
    /// the sources' lines: streamrip one release at a time, yt-dlp two, the indexers two searches, the
    /// worker's Soulseek searches one – and the requests someone asked to download now ([`line`])
    pub lines: line::Lines,
    /// wakes the queue's loop before its 5 s are up (Download now on a queued request)
    pub wake: Arc<Notify>,
    /// the managed Soulseek engine: its state, log tail and wake-ups
    pub slskd: slskd_managed::Managed,
    /// the YouTube sign-in's locks (its state file, a rotation in progress)
    pub youtube: ytauth::Locks,
    /// a Tidal sign-in for streamrip in progress
    pub tidal: sources::streamrip::TidalLogins,
    /// whether the library and the staging folder can be written (imports pause while they can't)
    pub storage: worker::Health,
    /// Soulseek and indexer answers kept for the picker (<config>/cache/search-cache.db)
    pub search_cache: search_cache::Cache,
}

