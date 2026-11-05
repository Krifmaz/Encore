//! Settings → Appearance: presets, theme, accent colour, background and the player bar's buttons.
//!
//! A Leptos view ([`Appearance`]) over one signal, the look as it is now. Every choice saves the look (on this
//! device, and to the account when the server keeps it) and draws the card anew, as the old page did.

use crate::app::{self, Ctx, PageFuture};
use crate::appearance;
use crate::dom;
use crate::json::{json, J};
use crate::ui::{icon, toast};
use kit_leptos::html::html;
use leptos::prelude::*;
use serde_json::Value;
use web_sys::Element;

/// The Appearance page: the view over the look this device has now.
pub fn render(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let server_prefs = app::has_feature("theme");
        dom::set_html(&el, "");
        // the signal lives as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let look = owner.with(|| RwSignal::new(appearance::current()));
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&el, move || view! { <Appearance look server_prefs/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        Ok(())
    })
}

/// `p = { ...p, ...patch }`, then saved (on this device, and to the account when `server_prefs`); the card is
/// drawn again from it.
fn set_look(look: RwSignal<Value>, patch: Value, server_prefs: bool) {
    look.update(|cur| {
        if let (Some(dst), Some(src)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
    });
    let p = look.get_untracked();
    dom::spawn(async move { appearance::save(&p, server_prefs).await });
}

/// What a typed hex colour stands for, lower case and six digits ("#F70" → "#ff7700"); `None` when it is not
/// one. The leading `#` may be left out.
pub fn parse_hex(typed: &str) -> Option<String> {
    let typed = typed.trim();
    let digits = typed.strip_prefix('#').unwrap_or(typed);
    if !((digits.len() == 3 || digits.len() == 6) && digits.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    let six: String = if digits.len() == 3 { digits.chars().flat_map(|c| [c, c]).collect() } else { digits.to_string() };
    Some(format!("#{}", six.to_lowercase()))
}

/// A row of choices for setting `key` (theme or background), the current one on.
fn seg(look: RwSignal<Value>, server_prefs: bool, p: &Value, key: &'static str, opts: &'static [(&'static str, &'static str)]) -> impl IntoView {
    let buttons = opts
        .iter()
        .map(|&(v, l)| {
            let on = if p.g(key).as_str() == Some(v) { "on" } else { "" };
            view! { <button type="button" data-v=v class=on on:click=move |_| set_look(look, json!({ key: v }), server_prefs)>{l}</button> }
        })
        .collect_view();
    view! { <div class="seg" data-key=key>{buttons}</div> }
}

/// The themes to pick from.
const THEMES: &[(&str, &str)] = &[("encore", "Encore"), ("dark", "Dark"), ("oled", "OLED black"), ("light", "Light"), ("system", "Match device")];
/// The backgrounds to pick from.
const BACKGROUNDS: &[(&str, &str)] = &[("fade", "Faded colour"), ("solid", "Solid")];

/// The Appearance card.
#[component]
pub fn Appearance(
    /// The look as it is now (theme, accent, custom colour, background).
    look: RwSignal<Value>,
    /// Whether the server keeps the look with the account.
    server_prefs: bool,
) -> impl IntoView {
    move || {
        let p = look.get();
        let presets = appearance::PRESETS
            .iter()
            .map(|x| {
                let bg = match x.theme {
                    "encore" => "#161617",
                    "oled" => "#000",
                    "light" => "#f6f6fb",
                    "dark" => "#0b0b12",
                    _ => "undefined",
                };
                let (a, b) = match appearance::ACCENTS.iter().find(|a| a.id == x.accent) {
                    Some(ac) => (ac.a, ac.b),
                    None => {
                        let c = x.custom.unwrap_or("undefined");
                        (c, c)
                    }
                };
                let on = p.s("theme") == x.theme && p.s("accent") == x.accent && p.s("background") == x.background && (x.accent != "custom" || p.g("custom").as_str() == x.custom);
                // the preset's look: its theme, accent, background and own colour
                let mut preset = json!({ "theme": x.theme, "accent": x.accent, "background": x.background });
                if let Some(c) = x.custom {
                    preset["custom"] = Value::String(c.to_string());
                }
                view! {
                    <button type="button" class=if on { "preset on" } else { "preset " } data-preset=x.id on:click=move |_| set_look(look, preset.clone(), server_prefs)>
                        <i style=format!("background:linear-gradient(135deg, {a} 0 22%, {b} 22% 34%, {bg} 34%)")></i>{x.name}</button>
                }
            })
            .collect_view();
        let swatches = appearance::ACCENTS
            .iter()
            .map(|a| {
                let cls = if p.s("accent") == a.id { "swatch on" } else { "swatch " };
                view! { <button type="button" class=cls data-accent=a.id title=a.name aria-label=a.name style=format!("background:linear-gradient(135deg, {}, {})", a.a, a.b) on:click=move |_| set_look(look, json!({ "accent": a.id }), server_prefs)></button> }
            })
            .collect_view();
        let custom = p.s("custom");
        // dragging in the colour picker: the colour shows at once (and in the hex box), saved when let go
        let on_pick = move |e: web_sys::Event| {
            let Some(t) = dom::target_el(&e) else { return };
            let v = dom::value(&t);
            look.update_untracked(|p| {
                p["accent"] = Value::String("custom".to_string());
                p["custom"] = Value::String(v.clone());
            });
            if let Some(h) = t.parent_element().and_then(|r| dom::qs_in(&r, "#custom-hex")) {
                dom::set_value(&h, &v);
            }
            appearance::apply(&look.get_untracked());
        };
        let on_picked = move |e: web_sys::Event| {
            if let Some(t) = dom::target_el(&e) {
                set_look(look, json!({ "accent": "custom", "custom": dom::value(&t) }), server_prefs);
            }
        };
        let on_hex = move |e: web_sys::Event| {
            let Some(t) = dom::target_el(&e) else { return };
            match parse_hex(&dom::value(&t)) {
                Some(custom) => set_look(look, json!({ "accent": "custom", "custom": custom }), server_prefs),
                None => toast("Use a hex colour like #ff7a00", "err"),
            }
        };
        let in_use = (p.s("accent") == "custom").then(|| view! { <span class="badge accent">"in use"</span> });
        let saved = if server_prefs { "Saved to your account, so it follows you to every device." } else { "Saved on this device." };
        view! {
            <div class="card stack">
                <h2>"Appearance"</h2>
                <p class="small muted" style="margin:0">{saved}</p>
                <div class="stack" style="gap:8px"><h3>"Presets"</h3>
                    <div class="presets">{presets}</div></div>
                <div class="stack" style="gap:6px"><h3>"Theme"</h3>{seg(look, server_prefs, &p, "theme", THEMES)}</div>
                <div class="stack" style="gap:8px"><h3>"Accent colour"</h3>
                    <div class="swatches">{swatches}</div>
                    <div class="custom-colour"><span class="small muted">"Any colour:"</span><input type="color" id="custom" value=custom.clone() title="Pick any colour" on:input=on_pick on:change=on_picked/>
                        <input class="input" id="custom-hex" value=custom maxlength="7" spellcheck="false" aria-label="Hex colour" on:change=on_hex/>
                        {in_use}</div></div>
                <div class="stack" style="gap:6px"><h3>"Background"</h3>{seg(look, server_prefs, &p, "background", BACKGROUNDS)}</div>
                <div class="row"><button class="btn primary" type="button" {..html(format!("{}Example button", icon("check")))}>{""}</button><span class="badge accent">"Example badge"</span></div>
                <div class="stack" style="gap:6px"><h3>"Player bar"</h3>
                    <p class="small muted" style="margin:0">"Pick which buttons sit on the bar at the bottom (Like, View album, Add to playlist, Lyrics, Share, Up next, Other devices, the quality badge). Saved on this device."</p>
                    <div class="row"><button class="btn" type="button" id="bar-customize" on:click=|_| crate::player::customize_bar_modal() {..html(format!("{}Customize the bar…", icon("settings")))}>{""}</button></div></div>
            </div>
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typed colour is three or six hex digits, with or without "#"; it is kept as six lower-case digits.
    #[test]
    fn a_typed_hex_colour_is_read_like_before() {
        assert_eq!(parse_hex("#FF7A00").as_deref(), Some("#ff7a00"));
        assert_eq!(parse_hex(" f70 ").as_deref(), Some("#ff7700"));
        assert_eq!(parse_hex("#ff7a0"), None);
        assert_eq!(parse_hex("orange"), None);
    }
}
