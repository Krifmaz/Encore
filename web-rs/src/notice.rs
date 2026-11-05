//! The admin's banner on every page (backlog item 17): a folder downloads are saved in can't be written –
//! read-only, missing, full or not Encore's to write – so requests that save there wait. It is the
//! `downloads` part of `GET /api/requests/storage`, the server notice the storage banner of the Requests and
//! Downloader pages reads too.
//!
//! The banner, its timing (asked at start and every minute, every 15 s while a folder fails) and Check again are
//! the kit's notice banners (`kit_leptos::notice`); this module tells them where Encore's notice comes from and
//! what Check again says. Nothing for other users, and nothing from a server without the probe. A Check again
//! here or on the storage banner of the Requests and Downloader pages updates both banners
//! (`encore:storage-checked`, [`keep`]).

use kit_leptos::notice::{self, NoticeSource};
use kit_rules::notices::CheckWords;
use std::rc::Rc;

/// What the toast after Check again says: still failing, still checking (a folder that doesn't answer), fixed.
pub const CHECK_WORDS: CheckWords = CheckWords {
    failing: "A downloads folder still can't be written",
    checking: "Still checking the downloads folders",
    fixed: "The downloads folders can be written",
};

/// Where the notice comes from: the `downloads` part of the storage answer; Check again probes every folder.
pub fn source() -> NoticeSource {
    NoticeSource {
        path: "/requests/storage".into(),
        check_path: "/requests/storage/check".into(),
        part: "downloads".into(),
        words: CHECK_WORDS,
        words_for: Vec::new(),
        // the Requests and Downloader pages list and draw their own storage banner again
        after_check: Some(Rc::new(|| {
            crate::dom::dispatch("encore:requests-changed", None);
            crate::dom::dispatch("encore:storage-checked", None);
        })),
    }
}

/// Draws the banner into the shell's `#server-notice` (after the shell is drawn again too).
pub fn paint() { notice::mount(); }

/// Keeps a storage answer (from the Requests and Downloader pages' Check again, which probes the download
/// folders too) and draws the banner again.
pub fn keep(s: &serde_json::Value) { notice::keep(s); }

/// Starts asking (after signing in; the kit asks only for admins, once per page load).
pub fn start() { notice::start(source(), crate::app::is_admin()); }

#[cfg(test)]
mod tests {
    use super::*;
    use kit_rules::notices::check_toast;
    use kit_rules::toasts::ToastKind;
    use serde_json::{json, Value};

    #[test]
    fn the_banner_shows_the_servers_sentence_while_a_folder_fails() {
        let s = source();
        let text = "The downloads folder /downloads is read-only – check the disk (dmesg, fsck) or the mount";
        let n = s.notices_of(&json!({ "downloads": { "ok": false, "text": text, "waiting": 3 } }));
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].banner(), Some(text));
        // every folder writable, an older server, a member's answer: nothing
        for none in [json!({ "downloads": { "ok": true, "text": null } }), Value::Null, json!({ "downloads": { "ok": false } })] {
            assert!(s.notices_of(&none).iter().all(|n| n.banner().is_none()), "{none}");
        }
        let toast = |d: Value| check_toast(&s.notices_of(&json!({ "downloads": d }))[0], &s.words_of(""));
        assert_eq!(toast(json!({ "ok": false, "text": text })), ("A downloads folder still can't be written", ToastKind::Warn));
        assert_eq!(toast(json!({ "ok": true })), ("The downloads folders can be written", ToastKind::Ok));
        assert_eq!(toast(json!({ "ok": true, "checking": true })), ("Still checking the downloads folders", ToastKind::Warn));
    }
}
