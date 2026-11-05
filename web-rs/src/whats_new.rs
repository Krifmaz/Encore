//! What's new: the popup on the first start after the server was updated (GET /api/changelog/new; once shown,
//! POST /api/changelog/seen marks this version seen for the account, so the desktop app does not show it again),
//! and Settings → What's new, the full changelog with the switch that turns the popup off (`prefs.whats_new_popup`).
//! Hotfix builds share their release's notes, so they never bring the popup back on their own.
//!
//! Both are the kit's (`kit_leptos::whats_new`, on the kit's default paths, which are Encore's); Encore sets
//! where Full changelog goes ([`SETTINGS`], in `kit.rs`) and the page's opening sentence ([`INTRO`]).

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use kit_leptos::whats_new::{ChangelogAnswer, ChangelogView, ChangelogViewProps};
use std::rc::Rc;
use web_sys::Element;

/// Where the popup's Full changelog goes.
pub const SETTINGS: &str = "/settings?s=whats-new";
/// The page's opening sentence: which version the server runs, and where the list shows.
pub const INTRO: &str = "This server runs {app} {version}. After an update, a short list of what changed shows once on the website and in the desktop app.";

/// After signing in: show the popup when something is new since this account last saw it.
pub fn start() { kit_leptos::whats_new::start(Rc::new(app::navigate)); }

/// Settings → What's new: the kit's changelog page, drawn once the changelog has come (an error goes to the
/// settings page's own error box, as before).
pub fn render(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let r = api::get("/changelog").await.map_err(|e| e.message)?;
        let answer: ChangelogAnswer = serde_json::from_value(r).unwrap_or_default();
        dom::set_html(&el, "");
        // kept while this page is on screen; the next page takes it off
        if let Some(m) = kit_leptos::rt::mount(&el, move || ChangelogView(ChangelogViewProps { app: None, answer })) { dom::keep(m); }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_intro_says_what_it_said() {
        let w = kit_rules::changelog::ChangelogWords { intro: INTRO, ..kit_rules::changelog::ChangelogWords::DEFAULT };
        assert_eq!(w.intro("Encore", "0.42.20"),
            "This server runs Encore 0.42.20. After an update, a short list of what changed shows once on the website and in the desktop app.");
    }
}
