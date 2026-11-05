//! /api/changelog: the release notes (docs/releases/<version>.md, built in by build.rs) for the What's new popup
//! the web and desktop apps show on their first start after an update, and for Settings → What's new.
//!
//! What counts as new is per account: `prefs.whats_new_seen` holds the version the person last saw, and only
//! releases with notes newer than it (and not newer than this server) are new. Hotfix builds (0.36.0.17) share
//! their release's notes, so a hotfix alone never shows the popup again. `prefs.whats_new_popup = false` turns
//! the popup off; Settings → What's new still lists everything.
//!
//! The routes, the parsing and the rules are the Krifmaz kit's `kit-server-changelog`; this module hands it
//! Encore's notes and version.
use crate::state::AppState;
use axum::Router;
use kit_server_changelog::{Changelog, ChangelogHost};
use std::sync::OnceLock;

pub use kit_server_changelog::{base, plain, unseen, Block, Release, HIGHLIGHTS, POPUP_RELEASES};

/// The release notes the build script embeds ([`generated::NOTES`]).
mod generated {
    include!(concat!(env!("OUT_DIR"), "/changelog_gen.rs"));
}

/// The What's new routes.
pub fn router() -> Router<AppState> {
    kit_server_changelog::router()
}

/// Encore's notes at this server's version, made once.
fn changelog() -> &'static Changelog {
    static ALL: OnceLock<Changelog> = OnceLock::new();
    ALL.get_or_init(|| Changelog::new("Encore", crate::VERSION, generated::NOTES))
}

impl ChangelogHost for AppState {
    fn changelog(&self) -> &Changelog {
        changelog()
    }

    /// The What's new marks are kept in the account's own preferences (`users.prefs`).
    fn prefs(&self) -> &dyn kit_server_core::PrefsStore {
        &self.kit.prefs
    }
}

/// Every release with notes, newest first.
pub fn releases() -> &'static [Release] {
    &changelog().releases
}

/// One release's markdown into its parts ("Encore 0.36.0" when its heading names no title).
pub fn parse(ver: &str, md: &str) -> Release {
    kit_server_changelog::parse("Encore", ver, md)
}

#[cfg(test)]
#[path = "changelog_tests.rs"]
mod changelog_tests;
