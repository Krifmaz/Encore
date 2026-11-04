//! What's new: the popup on the first start after the server was updated (GET /api/changelog/new; once shown,
//! POST /api/changelog/seen marks this version seen for the account, so the desktop app does not show it again),
//! and Settings → What's new, the full changelog with the switch that turns the popup off (`prefs.whats_new_popup`).
//! Hotfix builds share their release's notes, so they never bring the popup back on their own.

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use web_sys::Element;

/// Where the popup's Full changelog goes.
pub const SETTINGS: &str = "/settings?s=whats-new";

/// After signing in: show the popup when something is new since this account last saw it (the kit's
/// `whats_new`: "What's new", each release's title, summary and highlights, the switch "Show this after
/// updates", Full changelog and Close; the switch saves `prefs.whats_new_popup` and says "What's new will show
/// after updates" or "What's new won't show after updates – it stays in Settings").
pub fn start() {
    kit_leptos::whats_new::start(std::rc::Rc::new(app::navigate));
}

/// Settings → What's new: the kit's changelog page (`ChangelogView`) for this server's answer.
pub fn render(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let r = api::get("/changelog").await.map_err(|e| e.message)?;
        let answer: kit_leptos::whats_new::ChangelogAnswer = serde_json::from_value(r).unwrap_or_default();
        dom::set_html(&el, "");
        dom::mount_view(&el, move || leptos::view! { <kit_leptos::whats_new::ChangelogView app="Encore" answer=answer/> });
        Ok(())
    })
}
