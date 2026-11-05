//! Encore on the Krifmaz kit's web components (`kit-leptos`): the kit is told once, at start-up, which app it
//! works for, and draws its toasts into the page's `#toasts`.
//!
//! The kit's defaults already say Encore's words and draw Encore's markup (the kit was made from them), so only
//! the app itself is set here. The page keeps Encore's own stylesheet (`css/app.css`), which styles every class
//! the kit's components use; the kit's `kit.css` is not loaded, because its buttons differ from Encore's (a
//! filled danger button) and the page must look exactly as before.

use kit_leptos::config::{self, AppConfig};
use kit_rules::app::App;

/// Configures the kit for Encore (id `encore`: its storage keys, events and the `X-Requested-With` header) and
/// draws its toasts into the page's `#toasts`. Call once, first thing at start-up.
pub fn init() {
    config::set(AppConfig::new(App::named("Encore")));
    kit_leptos::toasts::mount();
}
