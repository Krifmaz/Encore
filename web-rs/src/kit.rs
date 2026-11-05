//! Encore on the Krifmaz kit's web components (`kit-leptos`): the kit is told once, at start-up, which app it
//! works for, draws its toasts into the page's `#toasts`, and reloads the page into a new build of the web app by
//! itself ([`start_updates`]).
//!
//! The kit's defaults already say Encore's words and draw Encore's markup (the kit was made from them), so only
//! the app itself is set here. The page keeps Encore's own stylesheet (`css/app.css`), which styles every class
//! the kit's components use; the kit's `kit.css` is not loaded, because its buttons differ from Encore's (a
//! filled danger button) and the page must look exactly as before.

use kit_leptos::config::{self, AppConfig};
use kit_leptos::update::UpdateConfig;
use kit_rules::app::App;

/// The update toast's second line while a song plays: a reload would stop it, and the browser would not start
/// it again without a tap.
pub const WAITS_FOR_MUSIC: &str = "It reloads when the music stops";
/// … while files are uploaded.
pub const WAITS_FOR_UPLOAD: &str = "It reloads when the upload is done";
/// … while songs are saved for offline.
pub const WAITS_FOR_SAVING: &str = "It reloads when the songs are saved";

/// Configures the kit for Encore (id `encore`: its storage keys, events, the `X-Requested-With` header and the
/// service worker's `encore:version` answer) and draws its toasts into the page's `#toasts`. Call once, first
/// thing at start-up.
pub fn init() {
    config::set(AppConfig::new(App::named("Encore")));
    kit_leptos::toasts::mount();
}

/// What Encore's page waits for before it reloads into a new build, in order: the music, an upload, songs being
/// saved for offline (the kit adds an open dialog last).
pub fn update_config() -> UpdateConfig {
    UpdateConfig::from_config()
        .hold(WAITS_FOR_MUSIC, crate::player::playing)
        .hold(WAITS_FOR_UPLOAD, crate::pages::files::uploading)
        .hold(WAITS_FOR_SAVING, crate::offline::download::downloading)
}

/// Starts the kit's self-update (once, at boot): `/api/health` names the build the server serves, and the page
/// reloads into it when nothing holds it. Wherever the browser allows a service worker (https, localhost), the
/// kit also registers `/sw.js` (static/sw.js, filled in by hooks/sw.sh), which keeps the app for offline use.
pub fn start_updates() {
    kit_leptos::update::start(update_config());
    if crate::dom::window().is_secure_context() {
        kit_leptos::update::register_worker(&config::get().paths.worker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit_rules::update::{waiting_for, Flow};

    #[test]
    fn what_holds_the_page_comes_in_order() {
        let order = |music, upload, saving, dialog| {
            waiting_for(&[(music, WAITS_FOR_MUSIC), (upload, WAITS_FOR_UPLOAD), (saving, WAITS_FOR_SAVING), (dialog, kit_rules::update::WAITS_FOR_DIALOG)])
        };
        assert_eq!(order(false, false, false, false), None);
        assert_eq!(order(true, true, true, true), Some(WAITS_FOR_MUSIC));
        assert_eq!(order(false, true, true, true), Some(WAITS_FOR_UPLOAD));
        assert_eq!(order(false, false, true, true), Some(WAITS_FOR_SAVING));
        assert_eq!(order(false, false, false, true), Some("It reloads when this dialog is closed"));
    }

    #[test]
    fn the_kit_says_encores_words() {
        let w = kit_rules::update::UpdateWords::DEFAULT;
        assert_eq!(w.ready("Encore"), "A new version of Encore is ready");
        assert_eq!(w.reload, "Reload");
        assert_eq!(kit_rules::update::tried_key("encore"), "encore.update.tried", "the loop guard keeps its key");
    }

    #[test]
    fn the_messages_match_the_worker() {
        // static/sw.js reads d.type === 'skip-waiting' and d.type === 'version?', and answers 'encore:version'
        let sw = include_str!("../static/sw.js");
        assert_eq!(Flow::skip_waiting_message()["type"], "skip-waiting");
        assert_eq!(Flow::version_question()["type"], "version?");
        assert!(sw.contains("d.type === 'skip-waiting'"));
        assert!(sw.contains("d.type === 'version?'"));
        assert!(sw.contains("type: 'encore:version', cache: CACHE"));
        // the kit takes the worker's answer as Encore's: its shell cache is `encore-shell-…`
        let mut f = Flow::new("encore");
        assert_eq!(f.message(&serde_json::json!({ "type": "encore:version", "cache": "encore-shell-abc" })).as_deref(), Some("encore-shell-abc"));
        // install waits: the page decides when the new worker takes over
        let install = sw.split("addEventListener('install'").nth(1).unwrap().split("addEventListener('activate'").next().unwrap();
        let code: String = install.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
        assert!(!code.contains("skipWaiting"), "install must not call skipWaiting()");
        // activate drops old shells only, never the saved songs
        assert!(sw.contains("k.startsWith(SHELL_PREFIX) && k !== CACHE"));
        assert!(sw.contains("const SHELL_PREFIX = 'encore-shell-'"));
        // what the worker never touches
        for p in ["p.startsWith('/api/')", "p === '/sw.js'", "url.origin !== self.location.origin"] {
            assert!(sw.contains(p), "{p}");
        }
        assert!(sw.contains("const CACHE = '__CACHE__';") && sw.contains("const SHELL = [__SHELL__];"), "the hook's placeholders");
    }

    #[test]
    fn the_server_names_its_build_in_health() {
        // the kit reads `web` from /api/health and the build from the page's `encore-web-<hash>.js`
        let lib = include_str!("../../server/crates/api/src/lib.rs");
        assert!(lib.contains("kit_server_health::Health::new(crate::VERSION, web)"), "/api/health sends `web`");
        assert!(lib.contains(r#"const WEB_SCRIPT: &str = "encore-web-";"#), "the server reads the same script name");
        assert_eq!(kit_leptos::config::AppConfig::new(App::named("Encore")).script, "encore-web");
        let head = r#"<link rel="modulepreload" href="/encore-web-f92ab5b84b305a59.js">"#;
        assert_eq!(kit_rules::update::build_in(head, "encore-web").as_deref(), Some("f92ab5b84b305a59"));
    }
}
