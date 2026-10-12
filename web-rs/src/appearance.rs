//! Port of web/js/appearance.js: theme, accent colour and background style.

use crate::dom::{self, document, ls_get, ls_set};
use crate::json::J;
use serde_json::{json, Value};
use std::cell::Cell;

pub struct Accent { pub id: &'static str, pub name: &'static str, pub a: &'static str, pub b: &'static str }
pub const ACCENTS: &[Accent] = &[
    Accent { id: "violet", name: "Violet", a: "#8b5cf6", b: "#ec4899" },
    Accent { id: "encore", name: "Encore", a: "#f2a7e4", b: "#9fd2fb" },
    Accent { id: "lilac", name: "Lilac", a: "#b48cf2", b: "#7cc4fa" },
    Accent { id: "pink", name: "Pink", a: "#ec4899", b: "#f97316" },
    Accent { id: "sky", name: "Sky", a: "#0ea5e9", b: "#8b5cf6" },
    Accent { id: "teal", name: "Teal", a: "#14b8a6", b: "#0ea5e9" },
    Accent { id: "green", name: "Green", a: "#22c55e", b: "#14b8a6" },
    Accent { id: "amber", name: "Amber", a: "#f59e0b", b: "#ef4444" },
    Accent { id: "red", name: "Red", a: "#ef4444", b: "#ec4899" },
];

thread_local! { static LOGO_N: Cell<u32> = const { Cell::new(0) }; }
const LOGO_PATHS: [&str; 3] = [
    "M249.205 114.329 L510.699 32.377 C510.699 32.377 525.463 25.913 538.636 31.22 C551.81 36.528 551.358 49.709 551.358 49.709 L550.464 99.763 C550.464 99.763 550.049 113.161 538.636 121.587 C527.224 130.012 514.95 133.407 514.95 133.407 L308.537 197.811 C298.175 201.165 275.923 206.119 273.697 229.869 C273.697 229.869 275.227 403.645 275.227 443.582 C275.227 493.942 241.622 582.872 154.358 582.872 C65.76 582.872 47.709 513.389 47.709 470.406 C47.709 412.557 102.003 373.129 140.138 369.574 C178.273 366.019 204.127 385.087 204.127 385.087 L201.373 166.271 C199.204 125.931 244.035 116.052 249.205 114.329 Z",
    "M345.928 420.623 C345.928 420.623 518.078 368.274 535.733 363.243 C553.388 358.212 564.789 368.142 564.274 383.837 L564.274 419.323 C564.274 449.986 539.212 459.963 525.178 464.079 L348.687 519.256 C329.357 525.979 320.186 517.423 320.186 498.97 L320.186 446.803 C320.186 430.002 326.687 425.907 345.928 420.623 Z",
    "M339.143 254.402 L494.664 206.043 C507.578 202.394 520.795 206.043 520.795 221.613 L520.795 239.527 C520.795 274.769 508.753 302.967 450.793 319.539 L341.142 356.25 C330.333 359.74 319.414 354.699 320.186 340.976 L319.414 282.394 C319.414 269.641 322.93 259.403 339.143 254.402 Z",
];
/// The Encore logo in the accent colours.
pub fn logo(size: u32) -> String {
    let n = LOGO_N.with(|c| { c.set(c.get() + 1); c.get() });
    let id = format!("lg{n}");
    let mut s = format!(r#"<svg class="logo" viewBox="16 15 580 580" width="{size}" height="{size}" aria-hidden="true"><defs><linearGradient id="{id}" x1="71" y1="351" x2="551" y2="257" gradientUnits="userSpaceOnUse"><stop offset="0" style="stop-color:var(--accent)"/><stop offset="1" style="stop-color:var(--accent-2)"/></linearGradient></defs>"#);
    for d in LOGO_PATHS { s.push_str(&format!(r#"<path d="{d}" fill="url(#{id})"/>"#)); }
    s.push_str("</svg>");
    s
}

pub struct Preset { pub id: &'static str, pub name: &'static str, pub theme: &'static str, pub accent: &'static str, pub custom: Option<&'static str>, pub background: &'static str }
pub const PRESETS: &[Preset] = &[
    Preset { id: "default", name: "Encore dark", theme: "dark", accent: "violet", custom: None, background: "fade" },
    Preset { id: "oled", name: "OLED black", theme: "oled", accent: "violet", custom: None, background: "solid" },
    Preset { id: "pastel", name: "Pastel", theme: "dark", accent: "encore", custom: None, background: "fade" },
    Preset { id: "light", name: "Daylight", theme: "light", accent: "sky", custom: None, background: "fade" },
    Preset { id: "mono", name: "Mono", theme: "oled", accent: "custom", custom: Some("#e5e5e5"), background: "solid" },
];

pub fn defaults() -> Value { json!({ "theme": "dark", "accent": "violet", "custom": "#8b5cf6", "background": "fade" }) }
const KEY: &str = "encore.appearance";

fn rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    let full: String = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_string() };
    let n = u32::from_str_radix(&full, 16).unwrap_or(0);
    (((n >> 16) & 255) as u8, ((n >> 8) & 255) as u8, (n & 255) as u8)
}
fn to_hex(r: f64, g: f64, b: f64) -> String {
    let c = |v: f64| (v.round().clamp(0.0, 255.0)) as u8;
    format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
}
fn luminance(hex: &str) -> f64 {
    let (r, g, b) = rgb(hex);
    let lin = |v: u8| { let v = v as f64 / 255.0; if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } };
    lin(r) * 0.2126 + lin(g) * 0.7152 + lin(b) * 0.0722
}
/// A custom colour gets a partner shifted around the colour wheel for the gradient.
fn partner(hex: &str) -> String {
    let (r, g, b) = rgb(hex);
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let (mut h, mut s) = (0.0, 0.0);
    if max != min {
        let d = max - min;
        s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        h = if max == r { (g - b) / d + if g < b { 6.0 } else { 0.0 } } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
        h /= 6.0;
    }
    h = (h + 0.1) % 1.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f64| { let t = (t + 1.0) % 1.0; if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t } else if t < 0.5 { q } else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 } else { p } };
    to_hex(f(h + 1.0 / 3.0) * 255.0, f(h) * 255.0, f(h - 1.0 / 3.0) * 255.0)
}

/// `current()`: the defaults overlaid with this device's saved appearance.
pub fn current() -> Value {
    let mut v = defaults();
    if let Some(saved) = ls_get(KEY).and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        if let (Some(dst), Some(src)) = (v.as_object_mut(), saved.as_object()) {
            for (k, val) in src { dst.insert(k.clone(), val.clone()); }
        }
    }
    v
}

thread_local! { static MQ: Cell<bool> = const { Cell::new(false) }; }

/// `apply(p)`.
pub fn apply(p: &Value) {
    let root = document().document_element().expect("html");
    let theme = p.s("theme");
    let dark = if theme == "system" { dom::matches_media("(prefers-color-scheme: dark)") } else { theme != "light" };
    dom::set_data(&root, "theme", if !dark { "light" } else if theme == "oled" { "oled" } else { "dark" });
    if theme == "system" && !MQ.with(|m| m.get()) {
        MQ.with(|m| m.set(true));
        if let Ok(Some(mq)) = dom::window().match_media("(prefers-color-scheme: dark)") {
            dom::on_forever(&mq, "change", |_| apply(&current()));
        }
    }
    let accent = p.s("accent");
    let preset = ACCENTS.iter().find(|x| x.id == accent);
    let a = match preset { Some(x) => x.a.to_string(), None => { let c = p.s("custom"); if c.is_empty() { defaults().s("custom") } else { c } } };
    let b = match preset { Some(x) => x.b.to_string(), None => partner(&a) };
    let (ar, ag, ab) = rgb(&a);
    let (br, bg, bb) = rgb(&b);
    dom::set_style(&root, "--accent", &a);
    dom::set_style(&root, "--accent-2", &b);
    dom::set_style(&root, "--accent-rgb", &format!("{ar}, {ag}, {ab}"));
    dom::set_style(&root, "--accent-2-rgb", &format!("{br}, {bg}, {bb}"));
    dom::set_style(&root, "--on-accent", if (luminance(&a) + luminance(&b)) / 2.0 > 0.45 { "#16161f" } else { "#fff" });
    dom::set_data(&root, "bg", if p.s("background") == "solid" { "solid" } else { "fade" });
    if let Some(meta) = dom::qs("meta[name=theme-color]") {
        let bg = dom::window().get_computed_style(&root).ok().flatten().and_then(|cs| cs.get_property_value("--bg").ok()).map(|s| s.trim().to_string()).unwrap_or_default();
        let _ = meta.set_attribute("content", if bg.is_empty() { "#0b0b12" } else { &bg });
    }
}

/// `save(p, { serverPrefs })`: this device, and the account where the server keeps preferences.
pub async fn save(p: &Value, server_prefs: bool) {
    let mut clean = defaults();
    if let (Some(dst), Some(src)) = (clean.as_object_mut(), p.as_object()) {
        for (k, v) in src { dst.insert(k.clone(), v.clone()); }
    }
    ls_set(KEY, &clean.to_string());
    apply(&clean);
    if server_prefs {
        let _ = crate::api::put("/auth/prefs", json!({ "appearance": clean })).await;
    }
}

/// `adoptAccount(user)`: the account's appearance wins over this device's.
pub fn adopt_account(user: &Value) {
    let a = user.g("prefs").g("appearance").clone();
    if a.is_object() {
        dom::spawn(async move { save(&a, false).await; });
    }
}

/// Module-load work of appearance.js: migrate the old dark/light toggle.
pub fn init() {
    if let Some(old) = ls_get("encore.theme") {
        if ls_get(KEY).is_none() {
            let mut v = defaults();
            v["theme"] = Value::String(old);
            ls_set(KEY, &v.to_string());
        }
    }
}
