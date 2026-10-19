//! Port of web/js/pages/settings.js (renderAppearance) – see docs/archive/ui-rewrite/CONTRACT.md
//! Settings → Appearance: presets, theme, accent colour, background and the player bar's buttons.

use crate::app::{self, Ctx, PageFuture};
use crate::appearance;
use crate::dom;
use crate::json::{json, J};
use crate::ui::{esc, icon, toast};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::rc::Rc;
use web_sys::Element;

/// The state of the JS `renderAppearance` closure: the panel, `p` and `serverPrefs`.
struct Look {
    el: Element, p: RefCell<Value>, server_prefs: bool,
}

/// Settings → Appearance: theme, accent colour, faded/solid background.
pub fn render(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let server_prefs = app::has_feature("theme");
        let st = Rc::new(Look { el, p: RefCell::new(appearance::current()), server_prefs });
        paint_appearance(&st);
        Ok(())
    })
}

fn seg(p: &Value, key: &str, opts: &[(&str, &str)]) -> String {
    format!(r##"<div class="seg" data-key="{key}">{}</div>"##,
        opts.iter().map(|(v, l)| format!(r##"<button type="button" data-v="{v}" class="{}">{l}</button>"##, if p.g(key).as_str() == Some(*v) { "on" } else { "" })).collect::<String>())
}

fn paint_appearance(st: &Rc<Look>) {
    let p = st.p.borrow().clone();
    let presets: String = appearance::PRESETS.iter().map(|x| {
        let bg = match x.theme { "encore" => "#161617", "oled" => "#000", "light" => "#f6f6fb", "dark" => "#0b0b12", _ => "undefined" };
        let (a, b) = match appearance::ACCENTS.iter().find(|a| a.id == x.accent) {
            Some(ac) => (ac.a, ac.b),
            None => { let c = x.custom.unwrap_or("undefined"); (c, c) }
        };
        let on = p.s("theme") == x.theme && p.s("accent") == x.accent && p.s("background") == x.background && (x.accent != "custom" || p.g("custom").as_str() == x.custom);
        format!(r##"<button type="button" class="preset {}" data-preset="{}"><i style="background:linear-gradient(135deg, {a} 0 22%, {b} 22% 34%, {bg} 34%)"></i>{}</button>"##,
            if on { "on" } else { "" }, x.id, esc(x.name))
    }).collect();
    let swatches: String = appearance::ACCENTS.iter().map(|a| format!(r##"<button type="button" class="swatch {}" data-accent="{}" title="{}" aria-label="{}" style="background:linear-gradient(135deg, {}, {})"></button>"##,
        if p.s("accent") == a.id { "on" } else { "" }, a.id, esc(a.name), esc(a.name), a.a, a.b)).collect();
    let custom = esc(&p.s("custom"));
    dom::set_html(&st.el, &format!(r##"<div class="card stack">
      <h2>Appearance</h2>
      <p class="small muted" style="margin:0">{saved}</p>
      <div class="stack" style="gap:8px"><h3>Presets</h3>
        <div class="presets">{presets}</div></div>
      <div class="stack" style="gap:6px"><h3>Theme</h3>{theme}</div>
      <div class="stack" style="gap:8px"><h3>Accent colour</h3>
        <div class="swatches">{swatches}</div>
        <div class="custom-colour"><span class="small muted">Any colour:</span><input type="color" id="custom" value="{custom}" title="Pick any colour">
          <input class="input" id="custom-hex" value="{custom}" maxlength="7" spellcheck="false" aria-label="Hex colour">
          {in_use}</div></div>
      <div class="stack" style="gap:6px"><h3>Background</h3>{background}</div>
      <div class="row"><button class="btn primary" type="button">{check}Example button</button><span class="badge accent">Example badge</span></div>
      <div class="stack" style="gap:6px"><h3>Player bar</h3>
        <p class="small muted" style="margin:0">Pick which buttons sit on the bar at the bottom (Like, View album, Add to playlist, Lyrics, Share, Up next, Other devices, the quality badge). Saved on this device.</p>
        <div class="row"><button class="btn" type="button" id="bar-customize">{settings}Customize the bar…</button></div></div>
    </div>"##,
        saved = if st.server_prefs { "Saved to your account, so it follows you to every device." } else { "Saved on this device." },
        theme = seg(&p, "theme", &[("encore", "Encore"), ("dark", "Dark"), ("oled", "OLED black"), ("light", "Light"), ("system", "Match device")]),
        in_use = if p.s("accent") == "custom" { r##"<span class="badge accent">in use</span>"## } else { "" },
        background = seg(&p, "background", &[("fade", "Faded colour"), ("solid", "Solid")]),
        check = icon("check"), settings = icon("settings")));
    if let Some(b) = dom::qs_in(&st.el, "#bar-customize") { dom::on(&b, "click", |_| crate::player::customize_bar_modal()); }
    {
        let st2 = st.clone();
        dom::on_each(&st.el, ".seg button", "click", move |b, _| {
            let Some(sg) = dom::closest_of(b, ".seg") else { return };
            let mut patch = Map::new();
            patch.insert(dom::data(&sg, "key"), Value::String(dom::data(b, "v")));
            set_appearance(&st2, Value::Object(patch));
        });
    }
    {
        let st2 = st.clone();
        dom::on_each(&st.el, "[data-accent]", "click", move |b, _| set_appearance(&st2, json!({ "accent": dom::data(b, "accent") })));
    }
    {
        let st2 = st.clone();
        dom::on_each(&st.el, "[data-preset]", "click", move |b, _| {
            let preset = dom::data(b, "preset");
            let Some(x) = appearance::PRESETS.iter().find(|x| x.id == preset) else { return };
            // const { id, name, ...look } = preset
            let mut look = json!({ "theme": x.theme, "accent": x.accent, "background": x.background });
            if let Some(c) = x.custom { look["custom"] = Value::String(c.to_string()); }
            set_appearance(&st2, look);
        });
    }
    if let Some(custom_el) = dom::qs_in(&st.el, "#custom") {
        {
            let (st2, c2) = (st.clone(), custom_el.clone());
            dom::on(&custom_el, "input", move |_| {
                let v = dom::value(&c2);
                let p = {
                    let mut p = st2.p.borrow_mut();
                    p["accent"] = Value::String("custom".to_string());
                    p["custom"] = Value::String(v.clone());
                    p.clone()
                };
                if let Some(h) = dom::qs_in(&st2.el, "#custom-hex") { dom::set_value(&h, &v); }
                appearance::apply(&p);
            });
        }
        {
            let (st2, c2) = (st.clone(), custom_el.clone());
            dom::on(&custom_el, "change", move |_| set_appearance(&st2, json!({ "accent": "custom", "custom": dom::value(&c2) })));
        }
    }
    if let Some(hex) = dom::qs_in(&st.el, "#custom-hex") {
        let (st2, h2) = (st.clone(), hex.clone());
        dom::on(&hex, "change", move |_| {
            let mut v = dom::value(&h2).trim().to_string();
            if !v.starts_with('#') { v = format!("#{v}"); }
            let digits = &v[1..];
            // /^#([0-9a-f]{3}|[0-9a-f]{6})$/i
            if (digits.len() == 3 || digits.len() == 6) && digits.chars().all(|c| c.is_ascii_hexdigit()) {
                let custom = if v.len() == 4 { format!("#{}", digits.chars().flat_map(|c| [c, c]).collect::<String>()) } else { v.to_lowercase() };
                set_appearance(&st2, json!({ "accent": "custom", "custom": custom }));
            } else {
                toast("Use a hex colour like #ff7a00", "err");
            }
        });
    }
}

/// `set(patch)`: `p = { ...p, ...patch }`, save (device + account), repaint.
fn set_appearance(st: &Rc<Look>, patch: Value) {
    let p = {
        let mut cur = st.p.borrow_mut();
        if let (Some(dst), Some(src)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in src { dst.insert(k.clone(), v.clone()); }
        }
        cur.clone()
    };
    let server_prefs = st.server_prefs;
    dom::spawn(async move { appearance::save(&p, server_prefs).await; });
    paint_appearance(st);
}
