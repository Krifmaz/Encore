//! The form of the Settings → Server cards, drawn from the schema (fields.rs) by one renderer: a switch for
//! `bool`, a row of check boxes for `checks`, a drop-down for `select`, a text area, a password input that starts
//! empty (with the eye and copy buttons when the server lets admins see saved secrets), and a text or number
//! input for the rest (`path` and `url` are text inputs). Under each field: its help line and where its value
//! comes from (an environment variable locks it; a saved secret says so and can be cleared). A field with a
//! `when` rule shows only while its select has the value. The markup is the one the static table of 0.28 drew;
//! form_tests.rs holds it to that. Reading the inputs back for Save is here too ([`form_values`]).

use super::apps;
use super::fields::{type_key, Field, Group};
use crate::dom;
use crate::json::{js_string, truthy, J};
use crate::ui::{esc, escv, icon, EYE};
use serde_json::{Map, Value};
use web_sys::Element;

/// What GET /admin/settings sends for a saved secret ("•••• (saved)").
const MASK: &str = "••••";
fn is_masked(v: &str) -> bool { v.starts_with(MASK) }

/// The note under a field: set by an environment variable (locked), a saved secret (with clear), a secret that
/// is not set, or the environment variable that would set it.
fn source_note(f: &Field, v: &Value, meta: &Value) -> String {
    let m = meta.o(&f.key);
    let secret = f.is_secret();
    if m.s("source") == "env" {
        return format!(r##"<small class="src env">🔒 Set by <span class="mono">{}</span> in your compose/env – change it there (environment always wins)</small>"##, esc(&m.s("env")));
    }
    if secret && truthy(v) { return format!(r##"<small class="src">Saved ✓ (hidden) · <a href="#" data-clear="{}">clear</a></small>"##, esc(&f.key)); }
    if secret {
        let env = if m.truthy("env") { format!(r##" – or set <span class="mono">{}</span> in your compose file"##, esc(&m.s("env"))) } else { String::new() };
        return format!(r##"<small class="src">Not set{env}</small>"##);
    }
    if m.s("source") == "ui" { return String::new(); }
    if m.truthy("env") { format!(r##"<small class="src faint">Default · env var <span class="mono">{}</span></small>"##, esc(&m.s("env"))) } else { String::new() }
}

/// One field: `v` is its value from GET /admin/settings, `meta` that answer's `_meta`, `reveal` whether the
/// server lets admins see saved secrets (the "reveal" feature of /api/auth/info).
pub fn field_html(f: &Field, v: &Value, meta: &Value, reveal: bool) -> String {
    let key = esc(&f.key);
    let id = format!("f-{key}");
    let locked = meta.o(&f.key).s("source") == "env";
    let dis = if locked { "disabled" } else { "" };
    let help = format!("{}{}", f.help.as_deref().map(|h| format!("<small>{}</small>", esc(h))).unwrap_or_default(), source_note(f, v, meta));
    let cls = if f.full || f.kind == "textarea" { "full" } else { "" };
    let label = esc(&f.label);
    match f.kind.as_str() {
        "bool" => {
            return format!(r##"<div class="field {cls}" style="justify-content:center"><label class="switch"><input type="checkbox" data-k="{key}" {checked} {dis}><i></i>{label}</label>{help}</div>"##,
                checked = if truthy(v) { "checked" } else { "" });
        }
        "checks" => {
            let vs = if v.is_null() { String::new() } else { js_string(v) };
            let on: Vec<String> = vs.split(',').map(type_key).collect();
            let boxes: String = f.options.iter().map(|(val, lab)| format!(r##"<label class="row" style="gap:6px;align-items:center"><input type="checkbox" value="{}" {} {dis}> {}</label>"##,
                esc(val), if on.contains(&type_key(val)) { "checked" } else { "" }, esc(lab))).collect();
            return format!(r##"<div class="field {cls}" data-k="{key}"><span>{label}</span><div class="row wrap" style="gap:6px 14px">{boxes}</div>{help}</div>"##);
        }
        "select" => {
            // option values are text; a numeric setting (streamrip_quality = 2) compares as text
            let vs = js_string(v);
            let opts_html: String = f.options.iter().map(|(val, lab)| format!(r##"<option value="{}" {}>{}</option>"##, esc(val), if *val == vs { "selected" } else { "" }, esc(lab))).collect();
            return format!(r##"<label class="field {cls}"><span>{label}</span><select class="input" data-k="{key}" id="{id}" {dis}>
      {opts_html}</select>{help}</label>"##);
        }
        _ => {}
    }
    // also for values set by environment variables (read-only, but viewable); never the WireGuard private key
    let reveal = reveal && truthy(v) && f.key != "vpn_wireguard_config";
    if f.kind == "textarea" {
        let btns = if reveal {
            format!(r##"<span class="row" style="gap:8px"><button type="button" class="btn sm" data-reveal="{key}">{eye}Show saved</button><button type="button" class="btn sm" data-copy="{key}">{copy}Copy</button></span>"##, eye = EYE, copy = icon("copy"))
        } else { String::new() };
        return format!(r##"<label class="field {cls}"><span>{label}</span><textarea class="input mono" data-k="{key}" {dis} placeholder="{ph}"></textarea>{btns}{help}</label>"##,
            ph = if truthy(v) { "Saved – paste to replace" } else { "" });
    }
    if f.kind == "password" {
        // secrets are only sent to the browser when you press the eye/copy button: empty input = keep the saved value
        let input = format!(r##"<input class="input" type="password" data-k="{key}" value="" {dis} {eye}
      autocomplete="new-password" placeholder="{ph}">"##,
            eye = if reveal { r#"data-eye="1""# } else { "" },
            ph = if locked { "set by environment" } else if truthy(v) { "saved – paste a new value to replace" } else { "not set" });
        let wrapped = if reveal {
            format!(r##"<span class="pw-wrap secret">{input}<button type="button" class="eye-btn" data-reveal="{key}" title="Show" aria-label="Show">{eye}</button><button type="button" class="eye-btn" data-copy="{key}" title="Copy" aria-label="Copy">{copy}</button></span>"##, eye = EYE, copy = icon("copy"))
        } else { input };
        return format!(r##"<label class="field {cls}"><span>{label}</span>{wrapped}{help}</label>"##);
    }
    // text, path, url and number (and any type this web app does not know yet)
    let kind = if f.kind == "number" { "number" } else { "text" };
    format!(r##"<label class="field {cls}"><span>{label}</span><input class="input" type="{kind}" data-k="{key}" {step}
    value="{val}" autocomplete="off" {dis}>{help}</label>"##,
        step = f.step.as_deref().map(|st| format!(r#"step="{}""#, esc(st))).unwrap_or_default(), val = escv(v))
}

/// A field with a `when` rule carries it on its outer element (`data-when` names the select, `data-when-in`
/// the value it shows for, or `data-when-not` for a `!=` rule) and starts hidden when the value does not match.
/// The same markup as showWhen in web/js/pages/settings.js.
pub fn show_when(f: &Field, html: String, values: &Value) -> String {
    let Some(w) = &f.when else { return html };
    let on = w.shows(&js_string(values.g(&w.key)));
    let rule = if w.equal { "data-when-in" } else { "data-when-not" };
    html.replacen(r#" class="field "#, &format!(r#" data-when="{}" {rule}="{}"{} class="field "#, esc(&w.key), esc(&w.value), if on { "" } else { " hidden" }), 1)
}

/// Shows and hides the fields with a `when` rule as their select changes.
pub fn bind_show_when(card: &Element) {
    let card2 = card.clone();
    dom::on_each(card, "select[data-k]", "change", move |sel, _| {
        let (k, v) = (dom::data(sel, "k"), dom::value(sel));
        for el in dom::qsa_in(&card2, &format!(r#"[data-when="{k}"]"#)) {
            let shown = if el.has_attribute("data-when-not") { dom::data(&el, "whenNot") != v } else { dom::data(&el, "whenIn").split(' ').any(|x| x == v) };
            dom::hide(&el, !shown);
        }
    });
}

/// The settings grid of a card, one cell per field drawn by `one`. The vpn_ fields (Soulseek) get a sub-block
/// of their own, in place, with the tunnel's state and Test VPN under them (apps.rs).
pub fn fields_html(fields: &[&Field], one: impl Fn(&Field) -> String) -> String {
    let is_vpn = |f: &Field| f.key.starts_with("vpn_");
    let Some(at) = fields.iter().position(|f| is_vpn(f)) else {
        return format!(r##"<div class="form-grid">{}</div>"##, fields.iter().map(|f| one(f)).collect::<String>());
    };
    let rest: Vec<&Field> = fields.iter().copied().filter(|f| !is_vpn(f)).collect();
    let at = at.min(rest.len());
    let vpn: String = fields.iter().filter(|f| is_vpn(f)).map(|f| one(f)).collect();
    let after: String = rest[at..].iter().map(|f| one(f)).collect();
    format!(r##"<div class="form-grid">{}</div>{}{}"##,
        rest[..at].iter().map(|f| one(f)).collect::<String>(), apps::vpn_block(&vpn),
        if rest.len() > at { format!(r##"<div class="form-grid mt">{after}</div>"##) } else { String::new() })
}

/// The fields of a card with their values: [`fields_html`] of [`field_html`] with the `when` rules applied.
pub fn form_html(fields: &[&Field], values: &Value, reveal: bool) -> String {
    let meta = values.o("_meta");
    fields_html(fields, |f| show_when(f, field_html(f, values.g(&f.key), &meta, reveal), values))
}

/// What Save sends for a card: every field of `g` found under `root`, except those an environment variable
/// sets, an empty secret (keeps the saved one) and a mask the server sent; `checks` as comma-separated values.
pub fn form_values(g: &Group, root: &Element, meta: &Value) -> Map<String, Value> {
    let mut body = Map::new();
    for el in dom::qsa_in(root, "[data-k]") {
        let k = dom::data(&el, "k");
        let Some(f) = g.field(&k) else { continue };
        if meta.o(&f.key).s("source") == "env" { continue; }            // locked by environment
        let secret = f.is_secret();
        if f.kind == "checks" {
            let on: Vec<String> = dom::qsa_in(&el, "input:checked").iter().map(|i| i.get_attribute("value").unwrap_or_default()).collect();
            body.insert(f.key.clone(), Value::String(on.join(",")));
        }
        else if el.get_attribute("type").as_deref() == Some("checkbox") { body.insert(f.key.clone(), Value::Bool(dom::checked(&el))); }
        else {
            let v = dom::value(&el);
            if secret && v.trim().is_empty() { /* keep saved secret */ }
            else if is_masked(&v) { /* never echo a mask back */ }
            else { body.insert(f.key.clone(), Value::String(if secret { v.trim().to_string() } else { v })); }
        }
    }
    body
}
