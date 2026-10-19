//! Port of web/js/pages/settings.js – see docs/archive/ui-rewrite/CONTRACT.md
//! The settings hub: the nav (You, People, Server, System), the page select for narrow screens, and the panel
//! that shows one page. The Server pages are drawn from the server's settings schema
//! (GET /api/admin/settings/schema, docs/contracts/settings-schema.md): groups.rs lays out a page's cards and
//! schema_form.rs draws their fields. Every other page is a file of its own here; the cards' extras (Download
//! tools, Tidal sign-in, the Soulseek status and VPN block, Cadenza's cache and engine settings) are in apps.rs.

pub mod account;
pub mod appearance;
pub mod apps;
pub mod autodownload;
pub mod connections;
pub mod diagnostics;
pub mod fields;
pub mod general;
pub mod groups;
pub mod invites;
pub mod playback;
pub mod schema_form;
pub mod tasks;
pub mod users;

#[cfg(test)]
mod form_tests;

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, json, truthy, J};
use crate::ui::{esc, icon};
use fields::Schema;
use serde_json::Value;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use wasm_bindgen::JsValue;
use web_sys::Element;

/// What the open Settings page shares between its parts (the state the JS `render` closed over).
pub struct Hub {
    pub view: Element,
    pub panel: Element,
    pub ctx: Ctx,
    /// GET /admin/settings (admins only): the values, with `_meta` saying where each one is set
    pub values: RefCell<Value>,
    /// GET /admin/settings/schema (admins only): the Server pages, their cards and fields
    pub schema: Rc<Schema>,
    /// the server lets admins see saved secrets (the eye and copy buttons)
    pub reveal: bool,
    /// #/settings?s=<card> opens that card once
    pub focus: RefCell<Option<String>>,
}

// Pages that are whole screens of their own, shown inside Settings
fn embedded(id: &str) -> Option<app::PageFn> {
    Some(match id {
        "account" => account::render,
        "appearance" => appearance::render,
        "language" => crate::pages::translate::render_language,
        "playback" => playback::render,
        "connections" => connections::render,
        "users" => users::render_users,
        "invites" => invites::render_invites,
        "autodownload" => autodownload::render,
        "contribute" => crate::pages::contribute::render,
        "blocked" => crate::pages::report::render,
        "diagnostics" => diagnostics::render,
        _ => return None,
    })
}

/// `history.replaceState(null, '', url)`.
fn replace_state(url: &str) {
    if let Ok(h) = dom::window().history() { let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(url)); }
}

/// `<div class="test-result …">` for a /system/test result (the `paint` of the JS).
pub fn test_result(r: &Value) -> String {
    let ok = r.truthy("ok");
    let msg = { let m = r.g("message"); if truthy(m) { js_string(m) } else if ok { "Connected".to_string() } else { "Failed".to_string() } };
    format!(r##"<div class="test-result {}">{}<span>{}</span></div>"##, if ok { "ok" } else { "err" }, if ok { icon("check") } else { icon("x") }, esc(&msg))
}

/// The JS `show(id)`: paints one settings page into the panel.
pub fn show(hub: &Rc<Hub>, id: String) -> Pin<Box<dyn Future<Output = ()>>> {
    let hub = hub.clone();
    Box::pin(async move {
        for b in dom::qsa_in(&hub.view, ".settings-nav button") { dom::toggle_class(&b, "active", dom::data(&b, "s") == id); }
        if let Some(sel) = dom::qs_in(&hub.view, ".settings-select") { dom::set_value(&sel, &id); }
        replace_state(&format!("#/settings?s={id}"));
        if let Some(page) = embedded(&id) {
            dom::set_html(&hub.panel, r##"<div class="skeleton" style="height:200px"></div>"##);
            if let Err(e) = page(hub.panel.clone(), hub.ctx.clone()).await { dom::console_error(&e); }
            return;
        }
        if id == "about" || id == "api" || id == "keys" { return show(&hub, "general".to_string()).await; }
        groups::show_page(&hub, &id).await;
    })
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        // the values and the schema of the Server pages, asked for together
        let (values, schema) = if admin {
            let (values, schema) = futures::join!(api::get("/admin/settings"), api::get("/admin/settings/schema"));
            (values.map_err(|e| e.message)?, Schema::from_json(&schema.map_err(|e| e.message)?))
        } else { (json!({}), Schema::default()) };
        let schema = Rc::new(schema);
        let s = ctx.q("s");
        let mut current = if s.is_empty() { "account".to_string() } else { schema.page_of(&s) };
        let mut nav: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("You", vec![("account", "Account"), ("appearance", "Appearance"), ("language", "Language"), ("playback", "Playback"), ("connections", "Connections")]),
        ];
        if admin {
            nav.push(("People", vec![("users", "Users & permissions"), ("invites", "Invites")]));
            nav.push(("Server", schema.sections.iter().map(|x| (x.id.as_str(), x.title.as_str())).collect()));
            nav.push(("System", vec![("diagnostics", "Diagnostics")]));
        }
        let pages: Vec<&str> = nav.iter().flat_map(|(_, items)| items.iter().map(|(id, _)| *id)).collect();
        if !pages.iter().any(|p| *p == current) { current = "account".to_string(); }
        let select_html: String = nav.iter().map(|(label, items)| format!(r##"<optgroup label="{}">{}</optgroup>"##,
            esc(if label.is_empty() { "Encore" } else { label }),
            items.iter().map(|(id, t)| format!(r##"<option value="{}">{}</option>"##, esc(id), esc(t))).collect::<String>())).collect();
        let nav_html: String = nav.iter().map(|(label, items)| format!("{}{}",
            if label.is_empty() { r##"<div style="height:10px"></div>"##.to_string() } else { format!(r##"<div class="nav-label">{}</div>"##, esc(label)) },
            items.iter().map(|(id, t)| format!(r##"<button data-s="{}">{}</button>"##, esc(id), esc(t))).collect::<String>())).collect();
        dom::set_html(&view, &format!(r##"
    <h1 style="margin-bottom:18px">Settings</h1>
    <select class="input settings-select" aria-label="Settings page">{select_html}</select>
    <div class="settings-grid">
      <nav class="settings-nav">{nav_html}</nav>
      <div id="panel"></div>
    </div>"##));
        if admin { diagnostics::update_banner(&view); }
        let panel = dom::qs_in(&view, "#panel").ok_or_else(|| "Settings: no panel".to_string())?;
        let focus = if !s.is_empty() && current != s { Some(s.clone()) } else { None };
        let hub = Rc::new(Hub {
            view: view.clone(), panel, ctx: ctx.clone(), values: RefCell::new(values), schema: schema.clone(),
            reveal: app::has_feature("reveal"), focus: RefCell::new(focus),
        });

        if let Some(nav_el) = dom::qs_in(&view, ".settings-nav") {
            let hub2 = hub.clone();
            dom::on(&nav_el, "click", move |e| {
                if let Some(b) = dom::closest(e, "[data-s]") { dom::spawn(show(&hub2, dom::data(&b, "s"))); }
            });
        }
        if let Some(sel) = dom::qs_in(&view, ".settings-select") {
            let (hub2, sel2) = (hub.clone(), sel.clone());
            dom::on(&sel, "change", move |_| dom::spawn(show(&hub2, dom::value(&sel2))));
        }
        dom::spawn(show(&hub, current));
        Ok(())
    })
}
