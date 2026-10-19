//! Keeping the server up to date: Encore's own updates and "Restart server" ([`update`]), the container
//! swap of "Update now" ([`selfupdate`], over the Docker Engine API in [`docker`]), and the download tools
//! that update without a new image ([`tools`]: yt-dlp, streamrip, Cadenza; slskd and the translator are
//! installed and run by other crates and reach the tools card through [`Ctx`]).
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub mod docker;
pub mod selfupdate;
pub mod tools;
pub mod update;

pub use tools::ToolInfo;

/// What the update and tool functions need from the server.
pub trait Ctx: encore_playback::Ctx {
    fn system(&self) -> &System;
    /// The newest release of a helper program run by another crate ("slskd", "translator"):
    /// (version or tag, download address).
    fn helper_latest(&self, id: &str) -> impl Future<Output = anyhow::Result<(String, String)>> + Send;
    /// Install release `tag` of a helper program from `url`; Ok(the version now installed).
    fn helper_install(&self, id: &str, tag: &str, url: &str) -> impl Future<Output = anyhow::Result<String>> + Send;
    /// A helper program's card, with what the last check found (`last`).
    fn helper_info(&self, id: &str, last: &Value) -> ToolInfo;
    /// Remove a helper program's downloaded copy.
    fn helper_reset(&self, id: &str) -> anyhow::Result<()>;
}

/// What the update and tool functions keep while the server runs. Clones share it.
#[derive(Clone)]
pub struct System {
    /// the `--version` answers and the running tool updates
    pub tools: tools::Tools,
    /// "Restart server" asked for
    pub restart: update::Restart,
    /// an "Update now" is on its way
    updating: Arc<AtomicBool>,
    /// GitHub's API (tests point it at a stand-in)
    pub github_api: String,
    /// where the Docker socket is mounted when the owner wants "Update now"
    pub docker_socket: PathBuf,
}

impl Default for System {
    fn default() -> Self {
        System { tools: Default::default(), restart: Default::default(), updating: Default::default(), github_api: "https://api.github.com".into(),
            docker_socket: docker::SOCKET.into() }
    }
}
