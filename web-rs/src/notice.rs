//! The admin's banner on every page (backlog item 17): a folder downloads are saved in can't be written –
//! read-only, missing, full or not Encore's to write – so requests that save there wait. It is the
//! `downloads` part of `GET /api/requests/storage`, the server notice the storage banner of the Requests and
//! Downloader pages reads too. Asked when the app starts and every minute (every 15 s while a folder fails,
//! so the banner goes soon after the folder is fixed); Check again probes every folder at once. Nothing for
//! other users, and nothing from a server without the probe. A Check again here or on the storage banner of
//! the Requests and Downloader pages updates both banners (`encore:storage-checked`, [`keep`]).
use crate::app;
use crate::json::J;
use crate::ui::{esc, icon};
use serde_json::Value;

/// The banner for the `downloads` part of the storage answer: the server's sentence ("The downloads folder
/// /downloads is read-only – check the disk (dmesg, fsck) or the mount") with Check again; nothing while
/// every folder can be written, for an answer without the part, or without a sentence.
pub fn banner_html(d: &Value) -> String {
    if !d.is_object() || d.g("ok") != &Value::Bool(false) || !d.truthy("text") { return String::new(); }
    format!(r#"<div class="storage-banner" role="status">{}<span>{}</span><button class="btn sm" data-notice-check>{}Check again</button></div>"#,
        icon("alert"), esc(&d.s("text")), icon("refresh"))
}

/// The toast after Check again and its kind: a check that still runs (a folder that doesn't answer) says so.
pub fn check_toast(d: &Value) -> (&'static str, &'static str) {
    if d.g("ok") == &Value::Bool(false) { ("A downloads folder still can't be written", "warn") }
    else if d.truthy("checking") { ("Still checking the downloads folders", "warn") }
    else { ("The downloads folders can be written", "ok") }
}

/// Draw the banner into the shell's `#server-notice`: the kit's banner (`kit_leptos::notice`), drawn with
/// [`banner_html`]'s markup and drawn again after every answer.
pub fn paint() {
    kit_leptos::notice::mount();
}

/// Keep the `downloads` part of a storage answer and draw the banner again (also after the Requests and
/// Downloader pages' Check again, which probes the download folders too).
pub fn keep(s: &Value) {
    kit_leptos::notice::keep(s);
}

/// Start asking (admins, once per page load): GET /api/requests/storage now, then every minute (every 15 s
/// while a folder fails); Check again posts /api/requests/storage/check and says how it went ([`check_toast`]'s
/// words), then tells the Requests and Downloader pages (`encore:requests-changed`, `encore:storage-checked`).
pub fn start() {
    kit_leptos::notice::start(kit_leptos::notice::NoticeSource::encore_downloads(), app::is_admin());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_banner_shows_the_servers_sentence_while_a_folder_fails() {
        let d = json!({ "ok": false, "text": "The downloads folder /downloads is read-only – check the disk (dmesg, fsck) or the mount", "waiting": 3 });
        let html = banner_html(&d);
        assert!(html.contains("<span>The downloads folder /downloads is read-only – check the disk (dmesg, fsck) or the mount</span>"), "{html}");
        assert!(html.contains("data-notice-check") && html.contains("Check again</button>"), "{html}");
        // every folder writable, an older server, a member's answer: nothing
        assert_eq!(banner_html(&json!({ "ok": true, "text": null })), "");
        assert_eq!(banner_html(&Value::Null), "");
        assert_eq!(banner_html(&json!({ "ok": false })), "");
        assert_eq!(check_toast(&d), ("A downloads folder still can't be written", "warn"));
        assert_eq!(check_toast(&json!({ "ok": true })), ("The downloads folders can be written", "ok"));
        assert_eq!(check_toast(&json!({ "ok": true, "checking": true })), ("Still checking the downloads folders", "warn"));
    }
}
