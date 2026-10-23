//! Playing music: how a song is streamed ([`stream`]: one decision, `plan_stream`, between the original, a
//! conversion made on the fly and server processing; codec negotiation), server processing with Cadenza and
//! its render cache ([`dsp`], [`cadenza`]), the devices
//! hub behind the remote ([`hub`]), group listens ([`group`]), each user's resume spot ([`spot`]), and Roon
//! ([`roon`]).
//!
//! Functions that need the server take a [`Ctx`], which the server's state implements.
pub mod cadenza;
pub mod dsp;
pub mod group;
pub mod hub;
pub mod roon;
pub mod spot;
pub mod stream;

/// What playback needs from the server.
pub trait Ctx: encore_media::Ctx {
    /// The playback state the server keeps.
    fn playback(&self) -> &Playback;
}

/// What playback keeps while the server runs. Clones share it.
#[derive(Clone, Default)]
pub struct Playback {
    /// Cadenza jobs running now, whole server (live streams and renders)
    pub slots: dsp::Slots,
    /// renders waiting or running, and how fast the last one went
    pub renders: dsp::render::Renders,
    /// what the downloaded Cadenza programs say about themselves
    pub cadenza: cadenza::Programs,
    /// the engine server processing runs on, kept per settings generation
    pub engine: dsp::EngineCache,
    /// every user's devices (the remote)
    pub devices: hub::Devices,
    /// the group listens (0.31)
    pub groups: group::Groups,
    /// the last resume spot written per user
    pub spots: spot::Spots,
    /// the link to a Roon Core
    pub roon: roon::Roon,
}
