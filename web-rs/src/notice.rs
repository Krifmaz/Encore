//! The admin's banner on every page (backlog item 17): a folder downloads are saved in can't be written –
//! read-only, missing, full or not Encore's to write – so requests that save there wait. It is the
//! `downloads` part of `GET /api/requests/storage`, the server notice the storage banner of the Requests and
//! Downloader pages reads too. Asked when the app starts and every minute (every 15 s while a folder fails,
//! so the banner goes soon after the folder is fixed); Check again probes every folder at once. Nothing for
//! other users, and nothing from a server without the probe. A Check again here or on the storage banner of
//! the Requests and Downloader pages updates both banners (`encore:storage-checked`, [`keep`]).
use crate::json::J;
use crate::ui::{esc, icon, toast};
use crate::{api, app, dom};
use serde_json::Value;
use std::cell::{Cell, RefCell};

thread_local! {
    /// The last `downloads` answer.
    static LAST: RefCell<Value> = const { RefCell::new(Value::Null) };
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

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

fn failing() -> bool { LAST.with(|l| l.borrow().g("ok") == &Value::Bool(false)) }

/// Draw the banner into the shell's `#server-notice` (after the shell is drawn again too).
pub fn paint() {
    let Some(slot) = dom::by_id("server-notice") else { return };
    let html = if app::is_admin() { LAST.with(|l| banner_html(&l.borrow())) } else { String::new() };
    dom::set_html(&slot, &html);
}

/// Keep the `downloads` part of a storage answer and draw the banner again (also after the Requests and
/// Downloader pages' Check again, which probes the download folders too).
pub fn keep(s: &Value) {
    LAST.with(|l| *l.borrow_mut() = s.g("downloads").clone());
    paint();
}

async fn load() {
    // offline, or an older server: the banner keeps what it last knew
    if let Ok(s) = api::get("/requests/storage").await { keep(&s); }
}

/// Start asking (admins, once per page load).
pub fn start() {
    if !app::is_admin() || STARTED.with(|s| s.replace(true)) { return; }
    dom::spawn(load());
    let ticks = Cell::new(0u32);
    dom::interval_forever(15_000, move || {
        ticks.set(ticks.get().wrapping_add(1));
        if failing() || ticks.get().is_multiple_of(4) { dom::spawn(load()); }
    });
    dom::on_forever(&dom::document(), "click", |e| {
        let Some(b) = dom::closest(e, "[data-notice-check]") else { return };
        dom::set_disabled(&b, true);
        dom::set_html(&b, r#"<span class="spinner" style="width:12px;height:12px"></span> Checking…"#);
        dom::spawn(async move {
            match api::post_empty("/requests/storage/check").await {
                Ok(s) => {
                    keep(&s);
                    let (msg, kind) = check_toast(s.g("downloads"));
                    toast(msg, kind);
                    dom::dispatch("encore:requests-changed", None);
                    // the page's storage banner draws itself again too
                    dom::dispatch("encore:storage-checked", None);
                }
                Err(err) => {
                    toast(&err.message, "err");
                    dom::set_disabled(&b, false);
                    dom::set_html(&b, &format!("{}Check again", icon("refresh")));
                }
            }
        });
    });
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
