//! Global keyboard shortcuts (Settings → This computer): play / pause, next, previous, volume
//! up and down, from any program. The strings are the config's (`CmdOrCtrl+Alt+P`, or what the
//! settings page records: `Ctrl+Alt+P`, `Super+PageUp`, `F7`). A shortcut that cannot be used
//! gets a reason in [`errors`] (the classic app's words) and the others still work.
//!
//! The manager lives on the main thread (Windows and macOS deliver the key presses through the
//! event loop there; on Linux it opens its own X11 connection). Without an X display (Wayland
//! only, a server) the shortcuts simply do nothing.

use super::{guarded, send, Message};
use crate::config::Hotkeys;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, Once};

thread_local! {
    static MANAGER: RefCell<Option<GlobalHotKeyManager>> = const { RefCell::new(None) };
    /// The shortcuts registered now (to unregister them on a change).
    static BOUND: RefCell<Vec<HotKey>> = const { RefCell::new(Vec::new()) };
}

/// Registered shortcut id → action.
static ACTIONS: Mutex<Option<HashMap<u32, &'static str>>> = Mutex::new(None);
/// action → why its shortcut is not in use.
static ERRORS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
/// The shortcuts as last applied.
static APPLIED: Mutex<Option<Hotkeys>> = Mutex::new(None);

/// A shortcut string as a hotkey, or the reason in the classic app's words.
pub fn parse(key: &str) -> Result<HotKey, String> {
    let k = key.trim();
    k.parse::<HotKey>()
        .map_err(|_| format!("“{k}” isn't a shortcut Encore understands"))
}

/// The shortcuts to register (action, hotkey) and the ones that cannot be (action → why):
/// bad strings, and a key already used by an earlier action.
pub fn plan(h: &Hotkeys) -> (Vec<(&'static str, HotKey)>, BTreeMap<String, String>) {
    let mut keys: Vec<(&'static str, HotKey)> = Vec::new();
    let mut errors = BTreeMap::new();
    if !h.on {
        return (keys, errors);
    }
    for (action, key) in h.bindings() {
        match parse(key) {
            Err(e) => {
                errors.insert(action.to_string(), e);
            }
            Ok(hk) if keys.iter().any(|(_, k)| k.id() == hk.id()) => {
                errors.insert(
                    action.to_string(),
                    format!("{} is already used above", key.trim()),
                );
            }
            Ok(hk) => keys.push((action, hk)),
        }
    }
    (keys, errors)
}

/// Why each shortcut is not in use (action → words), for Settings → This computer.
pub fn errors() -> BTreeMap<String, String> {
    ERRORS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The shortcuts as last applied (None before the first time).
pub fn applied() -> Option<Hotkeys> {
    APPLIED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The message of an action.
fn message(action: &str) -> Option<Message> {
    Some(match action {
        "toggle" => Message::Toggle,
        "next" => Message::Next,
        "prev" => Message::Prev,
        "volume_up" => Message::VolumeUp,
        "volume_down" => Message::VolumeDown,
        _ => return None,
    })
}

fn on_event(e: GlobalHotKeyEvent) {
    if e.state() != HotKeyState::Pressed {
        return;
    }
    let action = ACTIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&e.id()).copied());
    if let Some(m) = action.and_then(message) {
        send(m);
    }
}

/// (Re-)register the shortcuts (main thread). Returns action → problem for the ones that did
/// not work, as [`errors`] does afterwards.
pub fn apply(h: &Hotkeys) -> BTreeMap<String, String> {
    *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = Some(h.clone());
    let (keys, mut errors) = plan(h);
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_none() && !keys.is_empty() {
        log::info!("global shortcuts are off: there is no X display");
    }
    static HANDLER: Once = Once::new();
    HANDLER.call_once(|| GlobalHotKeyEvent::set_event_handler(Some(on_event)));
    MANAGER.with(|m| {
        let mut m = m.borrow_mut();
        if m.is_none() && !keys.is_empty() {
            *m = match guarded("Global shortcuts", GlobalHotKeyManager::new) {
                Some(Ok(mgr)) => Some(mgr),
                Some(Err(e)) => {
                    log::warn!("Global shortcuts are not available here: {e}");
                    None
                }
                None => None,
            };
        }
        let Some(mgr) = m.as_ref() else { return };
        BOUND.with(|b| {
            let old = std::mem::take(&mut *b.borrow_mut());
            if !old.is_empty() {
                let _ = guarded("Global shortcuts", || mgr.unregister_all(&old));
            }
        });
        let mut ids = HashMap::new();
        let mut bound = Vec::new();
        for (action, hk) in keys {
            match guarded("Global shortcuts", || mgr.register(hk)) {
                Some(Ok(())) => {
                    ids.insert(hk.id(), action);
                    bound.push(hk);
                }
                _ => {
                    let key = crate::config::Hotkeys::bindings(h)
                        .into_iter()
                        .find(|(a, _)| *a == action)
                        .map(|(_, k)| k.to_string())
                        .unwrap_or_default();
                    errors.insert(
                        action.to_string(),
                        format!("{key} is taken by another program"),
                    );
                }
            }
        }
        BOUND.with(|b| *b.borrow_mut() = bound);
        *ACTIONS.lock().unwrap_or_else(|e| e.into_inner()) = Some(ids);
    });
    if !errors.is_empty() {
        log::info!("shortcuts not set: {errors:?}");
    }
    *ERRORS.lock().unwrap_or_else(|e| e.into_inner()) = errors.clone();
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_and_recorded_shortcuts_parse() {
        for (_, k) in Hotkeys::default().bindings() {
            assert!(parse(k).is_ok(), "{k}");
        }
        // what Settings → This computer records (pages/settings/this_computer.rs accelerator())
        for k in [
            "Ctrl+Alt+P",
            "Ctrl+Shift+Up",
            "Alt+1",
            "Super+Shift+F7",
            "Ctrl+Alt+Space",
            "Ctrl+Alt+Comma",
            "F7",
            " Ctrl+Alt+N ",
        ] {
            assert!(parse(k).is_ok(), "{k}");
        }
        assert_eq!(
            parse("Ctrl+Nonsense").unwrap_err(),
            "“Ctrl+Nonsense” isn't a shortcut Encore understands"
        );
    }

    #[test]
    fn a_plan_skips_bad_and_doubled_keys() {
        let h = Hotkeys {
            next: "Ctrl+Alt+P".into(),
            prev: "Ctrl+Wat".into(),
            volume_down: String::new(),
            ..Hotkeys::default()
        };
        let (keys, errors) = plan(&h);
        assert_eq!(
            keys.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            ["toggle", "volume_up"]
        );
        assert_eq!(errors["next"], "Ctrl+Alt+P is already used above");
        assert!(errors["prev"].contains("isn't a shortcut"));
        assert!(
            !errors.contains_key("volume_down"),
            "an empty shortcut is just not set"
        );
        let off = Hotkeys {
            on: false,
            ..Hotkeys::default()
        };
        assert!(plan(&off).0.is_empty() && plan(&off).1.is_empty());
        assert_eq!(message("volume_up"), Some(Message::VolumeUp));
        assert_eq!(message("mini"), None);
    }
}
