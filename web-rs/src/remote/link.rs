//! Linked: this device locked as a remote for another one until you unlink (encore.link), and this computer's
//! built-in Cadenza engine (the desktop app) as a device of its own.

use super::*;

// Linked: this device is locked as a remote for another one until you unlink.
const CHAIN: &str = "encore.link";
// The Cadenza engine built into the desktop app: it signs in with its own token and shows up as a device of
// its own, which this window links to on its own – unless you unlinked it (encore.engine.off = "1").
const ENGINE_OFF: &str = "encore.engine.off";
pub const ENGINE_LABEL: &str = "This computer (Cadenza)";

thread_local! {
    /// `{ id, name }` of this computer's engine (desktop app only), known before it is online
    static ENGINE: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

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

/// This computer's Cadenza engine as a device `(id, name)` – only in the desktop app, None elsewhere.
pub fn engine_device() -> Option<(String, String)> { ENGINE.with(|e| e.borrow().clone()) }
pub fn is_engine(id: &str) -> bool { ENGINE.with(|e| e.borrow().as_ref().is_some_and(|(eid, _)| eid == id)) }
/// The name a device is shown under: the built-in engine is "This computer (Cadenza)".
pub fn shown_name(d: &Value) -> String { if is_engine(&d.s("id")) { ENGINE_LABEL.to_string() } else { d.s("name") } }
/// You unlinked this computer's engine: it is not linked to again on its own.
pub fn engine_off() -> bool { dom::ls_get(ENGINE_OFF).as_deref() == Some("1") }

pub fn link_to(d: Value) { link_to_opts(d, false); }
fn link_to_opts(d: Value, silent: bool) {
    let name = shown_name(&d);
    hide_spot();
    let id = d.s("id");
    if is_engine(&id) { dom::ls_remove(ENGINE_OFF); }
    dom::ls_set(CHAIN, &json!({ "id": d.g("id"), "name": name }).to_string());
    player::set_link(Some(player::Link { id: id.clone(), name: name.clone(), state: d.o("state"), online: true, send: link_sender(id) }));
    if !silent { toast(&format!("Linked – this device now controls {name}"), "ok"); }
    dom::spawn(beat());
}

pub fn unlink() {
    if player::linked().is_some_and(|l| is_engine(&l.id)) { dom::ls_set(ENGINE_OFF, "1"); }
    dom::ls_remove(CHAIN);
    player::set_link(None);
    toast("Unlinked – music plays on this device again", "ok");
    dom::spawn(beat());
}

/// Desktop app: sign the built-in engine in. The app knows the engine's id and name before it runs; the
/// server hands out a token for them, and the app starts the engine with it. Idempotent – safe on every start.
pub(super) async fn engine_sign_in() {
    if !native::has_engine() || !native::has_engine_sign_in() { return; }
    // any failure: Settings → Playback → Cadenza engine says what is wrong
    let Some(st) = native::engine_status().await else { return };
    if !st.truthy("device_id") { return; }
    let name = if st.truthy("name") { st.s("name") } else { "Cadenza".to_string() };
    ENGINE.with(|e| *e.borrow_mut() = Some((st.s("device_id"), name.clone())));
    // already running with a token and no error: keep it – a new token would restart the engine's link
    if st.truthy("running") && !st.truthy("error") { return; }
    let Ok(r) = api::post("/auth/app-token", json!({ "device_id": st.s("device_id"), "device_name": name })).await else { return };
    let origin = dom::location().origin().unwrap_or_default();
    let r2 = native::engine_login(r.g("token"), &origin).await;
    // the app answers with the id it signed in under – that is the device to look for
    if let Some(r2) = r2.filter(|r2| r2.truthy("device_id")) {
        let name = if r2.truthy("name") { r2.s("name") } else { name };
        ENGINE.with(|e| *e.borrow_mut() = Some((r2.s("device_id"), name)));
    }
}
