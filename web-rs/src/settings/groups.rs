//! Port of web/js/pages/settings.js (show) – see docs/archive/ui-rewrite/CONTRACT.md
//! A Settings → Server page drawn from the schema: its cards in the schema's order, under the heading of their
//! part, each a drop-down with its title, description, Test button, fields (schema_form.rs) and Save; plus what
//! the page has of its own: the task list (Scheduled tasks), How requests are fulfilled (Downloads, the page
//! with the Download pipeline card), the API key and About cards (General). Save, clear, the eye and copy
//! buttons and Test are wired here. The source cards of Downloads are plugin cards since 0.32 (plugins.rs).

use super::fields::{Field, Group};
use super::schema_form::{bind_show_when, form_html, form_values};
use super::{apps, general, show, tasks, test_result, Hub};
use crate::api::{self, ApiError};
use crate::dom;
use crate::json::{js_string, json, truthy, J};
use crate::ui::{copy_text, esc, icon, toast, EYE, EYE_OFF};
use serde_json::{Map, Value};
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlInputElement};

/// A saved secret, asked for when the eye or copy button is pressed (the server allows it only with the
/// "reveal" feature).
async fn secret_of(key: &str) -> Result<String, ApiError> {
    let enc: String = js_sys::encode_uri_component(key).into();
    Ok(api::get(&format!("/admin/settings/secret/{enc}")).await?.s("value"))
}
fn input_type(el: &Element) -> String { el.dyn_ref::<HtmlInputElement>().map(|i| i.type_()).unwrap_or_default() }
fn set_input_type(el: &Element, t: &str) { if let Some(i) = el.dyn_ref::<HtmlInputElement>() { i.set_type(t); } }

/// A card: a drop-down with the title, description and Test button in its summary, then the test result line,
/// `body` (the fields and the card's extras) and Save.
pub fn card_html(g: &Group, body: &str, open: bool) -> String {
    let desc = if g.desc.is_empty() { String::new() } else { format!(r##"<div class="small muted" style="margin-top:4px;font-weight:400">{}</div>"##, esc(&g.desc)) };
    let test = match g.test {
        Some(_) => format!(r##"<button class="btn sm" data-test>{}{}</button>"##, icon("refresh"), esc(g.test_label.as_deref().unwrap_or("Test connection"))),
        None => String::new(),
    };
    let chev = { let c = icon("chevron"); if c.is_empty() { "▾".to_string() } else { c } };
    format!(r##"<details class="card settings-card" data-sec="{sid}" {open}>
      <summary class="row between" style="gap:10px;flex-wrap:nowrap;align-items:flex-start"><div style="min-width:0"><h2>{title}</h2>{desc}</div>
        <span class="row" style="gap:8px">{test}<span class="chev">{chev}</span></span></summary>
      <div style="height:14px"></div>
      <div data-out></div>
      {body}
      <div class="row mt" style="justify-content:flex-end"><button class="btn primary" data-save>{check}Save</button></div>
    </details>"##,
        sid = esc(&g.id), open = if open { "open" } else { "" }, title = esc(&g.title), check = icon("check"))
}

/// The cards of a page that this server has: each with the fields the values hold, cards with none left out.
pub fn visible_cards<'a>(groups: &'a [Rc<Group>], values: &Value) -> Vec<(&'a Group, Vec<&'a Field>)> {
    groups.iter()
        .map(|g| (g.as_ref(), g.fields.iter().filter(|f| values.has(&f.key)).collect::<Vec<_>>()))
        .filter(|(_, fields)| !fields.is_empty())
        .collect()
}

/// The cards a Server page draws: [`visible_cards`] without those that are plugin cards since 0.32 (streamrip,
/// Torrents & Usenet, Soulseek, yt-dlp and YouTube sign-in are drawn on the Plugins page, plugins.rs).
pub fn page_cards<'a>(groups: &'a [Rc<Group>], values: &Value) -> Vec<(&'a Group, Vec<&'a Field>)> {
    visible_cards(groups, values).into_iter().filter(|(g, _)| !super::plugins::moved(&g.id)).collect()
}

/// The cards of a page with their part headings, as one string. `focus` is the card a link asked for (it starts
/// open instead of the first one); a page with one card has it open. A link that names no card shown here (an
/// alias such as `apps`, or a card this server hides) opens the first card, as a plain link to the page does.
pub fn cards_html(cards: &[(&Group, Vec<&Field>)], values: &Value, reveal: bool, focus: Option<&str>) -> String {
    let focus = focus.filter(|f| cards.iter().any(|(g, _)| g.id == *f));
    let n = cards.len();
    let mut html = String::new();
    let mut first = true;
    let mut part: Option<&str> = None;
    for (g, fields) in cards {
        if part != Some(g.part.as_str()) {
            part = Some(g.part.as_str());
            if !g.part.is_empty() { html.push_str(&format!(r##"<div class="nav-label" style="padding:18px 4px 8px">{}</div>"##, esc(&g.part))); }
        }
        let open = (first && focus.is_none()) || n == 1 || focus == Some(g.id.as_str());
        first = false;
        html.push_str(&card_html(g, &apps::card_body(&g.id, &form_html(fields, values, reveal)), open));
    }
    html
}

/// The JS `show(id)` for a Server page: paints page `id` (the first page when there is no such page).
pub async fn show_page(hub: &Rc<Hub>, id: &str) {
    let schema = hub.schema.clone();
    let Some(page) = schema.section(id).or_else(|| schema.sections.first()) else { return };
    let values = hub.values.borrow().clone();
    let focus = hub.focus.borrow_mut().take();
    let cards = page_cards(&page.groups, &values);
    // Downloads (the page with the Download pipeline card) ends with How requests are fulfilled
    let downloads = schema.holds(&page.id, "pipeline");
    let mut html = if page.id == "tasks" { r##"<div id="tasks-host" style="margin-bottom:16px"></div>"##.to_string() } else { String::new() };
    html.push_str(&cards_html(&cards, &values, hub.reveal, focus.as_deref()));
    if downloads { html.push_str(&apps::pipeline_help()); }
    if page.id == "general" { html.push_str(&general::api_card(&hub.panel).await); html.push_str(&general::about_card().await); }
    dom::set_html(&hub.panel, &html);
    if page.id == "tasks" { if let Some(host) = dom::qs_in(&hub.panel, "#tasks-host") { dom::spawn(tasks::render_tasks(host)); } }
    if let Some(f) = focus { if let Some(c) = dom::qs_in(&hub.panel, &format!(r#"[data-sec="{f}"]"#)) { c.scroll_into_view(); } }
    for card in dom::qsa_in(&hub.panel, "[data-sec]") {
        let Some(g) = schema.group(&dom::data(&card, "sec")) else { continue };
        bind_card(hub, g, &card, id);
    }
}

/// Save, clear, the eye and copy buttons, Test, the `when` rules and the card's extras.
fn bind_card(hub: &Rc<Hub>, g: Rc<Group>, card: &Element, page: &str) {
    let Some(out) = dom::qs_in(card, "[data-out]") else { return };
    apps::bind_card(hub, &g, card);
    bind_show_when(card);
    let paint: Rc<dyn Fn(&Value)> = { let out = out.clone(); Rc::new(move |r: &Value| dom::set_html(&out, &test_result(r))) };
    if let Some(b) = dom::qs_in(card, "[data-save]") {
        let (hub2, g2, card2) = (hub.clone(), g.clone(), card.clone());
        dom::on(&b, "click", move |_| {
            let (hub, g, card) = (hub2.clone(), g2.clone(), card2.clone());
            dom::spawn(async move { let _ = hub.save(&g, false, &card).await; });
        });
    }
    bind_clear(hub, card, page);
    bind_secret_buttons(card);
    if let Some(t) = dom::qs_in(card, "[data-test]") {
        let (hub2, g2, card2, out2, t2, paint2) = (hub.clone(), g, card.clone(), out, t.clone(), paint.clone());
        dom::on(&t, "click", move |ev| {
            dom::prevent(ev);
            let _ = card2.set_attribute("open", "");
            dom::set_disabled(&t2, true);
            dom::set_html(&out2, r##"<div class="test-result"><span class="spinner"></span><span>Testing…</span></div>"##);
            let (hub, g, card, t, paint) = (hub2.clone(), g2.clone(), card2.clone(), t2.clone(), paint2.clone());
            dom::spawn(async move {
                let failed = |e: ApiError| json!({ "ok": false, "message": if e.message.is_empty() { "Test failed".to_string() } else { e.message } });
                let r = match hub.save(&g, true, &card).await {
                    Ok(()) => match api::post_empty(&format!("/system/test/{}", g.test.as_deref().unwrap_or(""))).await { Ok(r) => r, Err(e) => failed(e) },
                    Err(e) => failed(e),
                };
                paint(&r);
                dom::set_disabled(&t, false);
            });
        });
    }
}

/// A saved secret's clear link: saves it empty (PUT /admin/settings) and draws page `page` again.
pub(super) fn bind_clear(hub: &Rc<Hub>, card: &Element, page: &str) {
    let (hub2, page2) = (hub.clone(), page.to_string());
    dom::on_each(card, "[data-clear]", "click", move |a, e| {
        dom::prevent(e);
        let key = dom::data(a, "clear");
        let (hub, page) = (hub2.clone(), page2.clone());
        dom::spawn(async move {
            let mut body = Map::new();
            body.insert(key, Value::String(String::new()));
            match api::put("/admin/settings", Value::Object(body)).await {
                Ok(v) => {
                    *hub.values.borrow_mut() = v;
                    toast("Cleared", "ok");
                    show(&hub, page).await;
                }
                Err(e) => dom::console_error(&e.message),
            }
        });
    });
}

/// The eye and copy buttons of saved secrets (asked for from the server only when pressed).
pub(super) fn bind_secret_buttons(card: &Element) {
    {
        let card2 = card.clone();
        dom::on_each(card, "[data-reveal]", "click", move |b, e| {
            dom::prevent(e);
            let key = dom::data(b, "reveal");
            let Some(input) = dom::qs_in(&card2, &format!(r#"[data-k="{key}"]"#)) else { return };
            let is_textarea = input.tag_name() == "TEXTAREA";
            let hidden = if is_textarea { dom::data(&input, "shown").is_empty() } else { input_type(&input) == "password" };
            let b = b.clone();
            dom::spawn(async move {
                if hidden {
                    if dom::value(&input).is_empty() {
                        match secret_of(&key).await {
                            Ok(v) => { dom::set_value(&input, &v); dom::set_data(&input, "revealed", "1"); }
                            Err(err) => { toast(&err.message, "err"); return; }
                        }
                    }
                    if !is_textarea { set_input_type(&input, "text"); }
                    dom::set_data(&input, "shown", "1");
                } else {
                    if !dom::data(&input, "revealed").is_empty() { dom::set_value(&input, ""); let _ = input.remove_attribute("data-revealed"); }
                    if !is_textarea { set_input_type(&input, "password"); }
                    let _ = input.remove_attribute("data-shown");
                }
                if dom::has_class(&b, "eye-btn") {
                    dom::set_html(&b, if hidden { EYE_OFF } else { EYE });
                    let _ = b.set_attribute("title", if hidden { "Hide" } else { "Show" });
                } else {
                    dom::set_html(&b, &if hidden { format!("{EYE_OFF}Hide") } else { format!("{EYE}Show saved") });
                }
            });
        });
    }
    {
        let card2 = card.clone();
        dom::on_each(card, "[data-copy]", "click", move |b, e| {
            dom::prevent(e);
            let key = dom::data(b, "copy");
            let Some(input) = dom::qs_in(&card2, &format!(r#"[data-k="{key}"]"#)) else { return };
            dom::spawn(async move {
                let v = dom::value(&input);
                let text = if !v.is_empty() { v } else {
                    match secret_of(&key).await { Ok(s) => s, Err(err) => { toast(&err.message, "err"); return; } }
                };
                copy_text(&text);
            });
        });
    }
}

impl Hub {
    /// Save of a card (and the quiet save before Test or Test VPN): PUT /admin/settings with what the card's
    /// fields hold, and for Cadenza the engine's own settings. The answer becomes the values; a folder Encore
    /// cannot write to comes back as a warning under the card.
    pub async fn save(&self, g: &Group, quiet: bool, root: &Element) -> Result<(), ApiError> {
        let meta = self.values.borrow().o("_meta");
        let mut body = form_values(g, root, &meta);
        if g.id == "cadenza" && meta.o("cadenza_engine_settings").s("source") != "env" {
            if let Some(engine) = apps::engine_settings_value(root) { body.insert("cadenza_engine_settings".to_string(), Value::String(engine)); }
        }
        let engine_saved = body.get("cadenza_engine_settings").and_then(|v| v.as_str()).map(String::from);
        let app_name = body.get("app_name").cloned().unwrap_or(Value::Null);
        match api::put("/admin/settings", Value::Object(body)).await {
            Ok(mut v) => {
                // a saved folder Encore can't write to: the server says so at once, with the chown line
                let warnings: Vec<String> = v.g("_warnings").as_array().map(|a| a.iter().map(|w| w.s("message")).filter(|m| !m.is_empty()).collect()).unwrap_or_default();
                if let Some(o) = v.as_object_mut() { o.remove("_warnings"); }
                *self.values.borrow_mut() = v;
                if let Some(e) = engine_saved.filter(|e| !e.is_empty()) { apps::remember_engine_values(&e); }
                if !quiet { toast("Settings saved", "ok"); }
                if !warnings.is_empty() {
                    if let Some(out) = dom::qs_in(root, "[data-out]") {
                        dom::set_html(&out, &warnings.iter().map(|m| test_result(&json!({ "ok": false, "message": m }))).collect::<String>());
                    }
                }
                if g.id == "general" && truthy(&app_name) { dom::set_title(&js_string(&app_name)); }
                Ok(())
            }
            Err(e) => {
                toast(&e.message, "err");
                Err(e)   // `if (quiet) throw e` – the non-quiet callers ignore it
            }
        }
    }
}
