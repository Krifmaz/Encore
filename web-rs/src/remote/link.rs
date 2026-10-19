//! Linked: this device locked as a remote for another one until you unlink (encore.link).

use super::*;

// Linked: this device is locked as a remote for another one until you unlink.
const CHAIN: &str = "encore.link";

/// `JSON.parse(localStorage.getItem(CHAIN) || 'null')` – `Some` only when that is truthy.
pub(super) fn saved_link() -> Option<Value> {
    let raw = dom::ls_get(CHAIN).filter(|s| !s.is_empty()).unwrap_or_else(|| "null".into());
    let l = serde_json::from_str::<Value>(&raw).ok().filter(crate::json::truthy)?;
    // a link to the desktop app's built-in engine from before it stopped being a device: forget it, so music
    // plays on this device again
    if l.s("id").starts_with("cadenza-desktop-") {
        dom::ls_remove(CHAIN);
        return None;
    }
    Some(l)
}

/// `(c) => sendTo(id, c).catch(e => toast(e.message, 'err'))` – the link's `send`.
pub(super) fn link_sender(id: String) -> Rc<dyn Fn(Value)> {
    Rc::new(move |c| {
        let id = id.clone();
        dom::spawn(async move { if let Err(e) = send_to(id, c).await { toast(&e.message, "err"); } });
    })
}

pub fn link_to(d: Value) {
    let name = d.s("name");
    hide_spot();
    let id = d.s("id");
    dom::ls_set(CHAIN, &json!({ "id": d.g("id"), "name": name }).to_string());
    player::set_link(Some(player::Link { id: id.clone(), name: name.clone(), state: d.o("state"), online: true, send: link_sender(id) }));
    toast(&format!("Linked – this device now controls {name}"), "ok");
    dom::spawn(beat());
}

pub fn unlink() {
    dom::ls_remove(CHAIN);
    player::set_link(None);
    toast("Unlinked – music plays on this device again", "ok");
    dom::spawn(beat());
}
