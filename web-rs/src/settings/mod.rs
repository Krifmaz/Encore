//! Port of web/js/pages/settings.js – see docs/archive/ui-rewrite/CONTRACT.md
//! The settings hub: the nav (You, People, Server, System), the page select for narrow screens, and the panel
//! that shows one page. The Server pages are drawn from the server's settings schema
//! (GET /api/admin/settings/schema, docs/contracts/settings-schema.md): groups.rs lays out a page's cards and
//! schema_form.rs draws their fields. Every other page is a file of its own here; the cards' extras (Tidal
//! sign-in, the Soulseek status and VPN block, Cadenza's cache and engine settings) are in apps.rs. The Plugins
//! page (plugins.rs, 0.32) holds the source cards that were on Downloads.

pub mod account;
pub mod appearance;
pub mod apps;
pub mod autodownload;
pub mod connections;
pub mod eq_graph;
pub mod diagnostics;
pub mod fields;
pub mod general;
pub mod groups;
pub mod invites;
pub mod playback;
pub mod plugins;
pub mod schema_form;
pub mod tasks;
pub mod users;
pub mod wrapper;

#[cfg(test)]
mod form_tests;
#[cfg(test)]
mod plugins_tests;

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, json, truthy, J};
use crate::ui::{esc, icon};
use fields::Schema;
use kit_leptos::html::seq;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use serde_json::Value;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use wasm_bindgen::JsValue;
use web_sys::Element;

/// GET /admin/settings/schema as the kit reads it, parsed on first use (Account and the other pages of
/// everyone do without it).
pub struct KitSchema {
    json: Value,
    parsed: std::cell::OnceCell<kit_leptos::schema_form::Schema>,
}

impl KitSchema {
    fn new(json: Value) -> Self { KitSchema { json, parsed: std::cell::OnceCell::new() } }
    /// The parsed schema (empty for a member, who has no Server pages).
    pub fn get(&self) -> &kit_leptos::schema_form::Schema {
        self.parsed.get_or_init(|| if self.json.is_null() { Default::default() } else { kit_leptos::schema_form::Schema::from_json(&self.json) })
    }
}

/// What the open Settings page shares between its parts (the state the JS `render` closed over).
pub struct Hub {
    pub view: Element,
    pub panel: Element,
    pub ctx: Ctx,
    /// GET /admin/settings (admins only): the values, with `_meta` saying where each one is set
    pub values: RefCell<Value>,
    /// GET /admin/settings/schema (admins only): the Server pages, their cards and fields
    pub schema: Rc<Schema>,
    /// The same schema as the kit reads it, parsed when a Server page is first shown: the kit's settings cards
    /// draw the Server pages from it (groups.rs)
    pub kit_schema: KitSchema,
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
        "files" => crate::pages::files::render,
        "whats-new" => crate::whats_new::render,
        _ => return None,
    })
}

/// The page `#/settings?s=<s>` opens: the Plugins page for its own names and the cards that moved there in
/// 0.32 (streamrip, torznab, slskd, ytdlp, youtube, the old Download tools), else the schema's page for a page,
/// an alias or a card (anything else comes back as it is).
pub fn page_for(schema: &Schema, s: &str) -> String {
    if plugins::opens_plugins(s) && schema.section(s).is_none() { "plugins".to_string() } else { schema.page_of(s) }
}

/// The Server pages in the nav: the schema's, with Plugins after Downloads (the page with the Download pipeline
/// card), or at the end when there is no such page.
pub fn server_nav(schema: &Schema) -> Vec<(&str, &str)> {
    let mut items: Vec<(&str, &str)> = schema.sections.iter().map(|x| (x.id.as_str(), x.title.as_str())).collect();
    if schema.section("plugins").is_none() {
        let at = schema.sections.iter().position(|x| schema.holds(&x.id, "pipeline")).map(|i| i + 1).unwrap_or(items.len());
        items.insert(at, ("plugins", "Plugins"));
    }
    items
}

/// `history.replaceState(null, '', url)`.
fn replace_state(url: &str) {
    if let Ok(h) = dom::window().history() { let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(url)); }
}

/// `<div class="test-result …">` for a /system/test result (the `paint` of the JS). A failure has a Copy button
/// for its whole text (R1).
pub fn test_result(r: &Value) -> String {
    let ok = r.truthy("ok");
    let msg = { let m = r.g("message"); if truthy(m) { js_string(m) } else if ok { "Connected".to_string() } else { "Failed".to_string() } };
    let copy = if ok { String::new() } else { crate::details::copy_button(&msg) };
    format!(r##"<div class="test-result {}">{}<span>{}</span>{copy}</div>"##, if ok { "ok" } else { "err" }, if ok { icon("check") } else { icon("x") }, esc(&msg))
}

thread_local! {
    /// The page the hub shows: its nav button is the active one.
    static PAGE: ArcRwSignal<String> = ArcRwSignal::new(String::new());
    /// The Leptos view drawn into `#panel` (a Server page's cards), taken off when another page is shown.
    static PANEL_VIEW: RefCell<Option<kit_leptos::rt::Mounted>> = const { RefCell::new(None) };
}

/// Draws `f` into the hub's `#panel` (emptied first) in place of what was there.
pub fn mount_panel<V: IntoView + 'static>(panel: &Element, f: impl FnOnce() -> V + 'static) {
    drop(PANEL_VIEW.with(|p| p.borrow_mut().take()));
    dom::set_html(panel, "");
    let m = kit_leptos::rt::mount(panel, f);
    PANEL_VIEW.with(|p| *p.borrow_mut() = m);
}

/// The hub, drawn by Leptos: the title, the page select for narrow screens, the nav (per group a label and its
/// pages' buttons, the shown page's `active`) and `#panel`, which the pages draw into. `groups` are (label,
/// [(page, title)]); `pick` shows a page. The markup is the one `render` wrote with `set_html` before, the line
/// breaks between the tags included.
fn hub_view(groups: Vec<(String, Vec<(String, String)>)>, pick: SendWrapper<impl Fn(String) + 'static>) -> impl IntoView {
    let pick = std::sync::Arc::new(pick);
    let page = PAGE.with(Clone::clone);
    let select = seq(groups.iter().map(|(label, items)| {
        let label = if label.is_empty() { "Encore".to_string() } else { label.clone() };
        let opts = seq(items.iter().map(|(id, t)| view! { <option value=id.clone()>{t.clone()}</option> }));
        view! { <optgroup label=label>{opts}</optgroup> }
    }));
    let nav = seq(groups.into_iter().map(|(label, items)| {
        let head = if label.is_empty() { view! { <div {..()} style="height:10px">{""}</div> }.into_any() } else { view! { <div class="nav-label">{label}</div> }.into_any() };
        let buttons = seq(items.into_iter().map(|(id, t)| {
            let (page, pick, id2) = (page.clone(), pick.clone(), id.clone());
            view! { <button {..()} data-s=id.clone() class:active=move || page.get() == id on:click=move |_| (*pick)(id2.clone())>{t}</button> }
        }));
        view! { {head}{buttons} }
    }));
    let p2 = pick.clone();
    view! {
        {"\n    "}
        <h1 {..()} style="margin-bottom:18px">Settings</h1>
        {"\n    "}
        <select class="input settings-select" aria-label="Settings page" on:change=move |e| (*p2)(event_target_value(&e))>{select}</select>
        {"\n    "}
        <div class="settings-grid">
            {"\n      "}
            <nav class="settings-nav">{nav}</nav>
            {"\n      "}
            <div id="panel">{""}</div>
            {"\n    "}
        </div>
    }
}

/// The JS `show(id)`: paints one settings page into the panel.
pub fn show(hub: &Rc<Hub>, id: String) -> Pin<Box<dyn Future<Output = ()>>> {
    let hub = hub.clone();
    Box::pin(async move {
        // the nav's active button (drawn by Leptos, settings_view)
        PAGE.with(|p| if p.get_untracked() != id { p.set(id.clone()) });
        if let Some(sel) = dom::qs_in(&hub.view, ".settings-select") { dom::set_value(&sel, &id); }
        replace_state(&format!("#/settings?s={id}"));
        // the Leptos view of the page before (if it was one) goes
        drop(PANEL_VIEW.with(|p| p.borrow_mut().take()));
        if let Some(page) = embedded(&id) {
            dom::set_html(&hub.panel, r##"<div class="skeleton" style="height:200px"></div>"##);
            if let Err(e) = page(hub.panel.clone(), hub.ctx.clone()).await { dom::console_error(&e); }
            return;
        }
        if id == "about" || id == "api" || id == "keys" { return show(&hub, "general".to_string()).await; }
        if id == "plugins" { return plugins::show_page(&hub).await; }
        groups::show_page(&hub, &id).await;
    })
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        // the values and the schema of the Server pages, asked for together
        let (values, schema, kit_schema) = if admin {
            let (values, schema) = futures::join!(api::get("/admin/settings"), api::get("/admin/settings/schema"));
            let schema = schema.map_err(|e| e.message)?;
            (values.map_err(|e| e.message)?, Schema::from_json(&schema), schema)
        } else { (json!({}), Schema::default(), Value::Null) };
        let schema = Rc::new(schema);
        let s = ctx.q("s");
        let mut current = if s.is_empty() { "account".to_string() } else { page_for(&schema, &s) };
        let mut nav: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("You", vec![("account", "Account"), ("appearance", "Appearance"), ("language", "Language"), ("playback", "Playback"), ("connections", "Connections")]),
        ];
        if admin {
            nav.push(("People", vec![("users", "Users & permissions"), ("invites", "Invites")]));
            nav.push(("Server", server_nav(&schema)));
            nav.push(("System", vec![("diagnostics", "Diagnostics"), ("files", "Files")]));
        }
        nav.push(("Encore", vec![("whats-new", "What's new")]));
        let pages: Vec<&str> = nav.iter().flat_map(|(_, items)| items.iter().map(|(id, _)| *id)).collect();
        if !pages.iter().any(|p| *p == current) { current = "account".to_string(); }
        dom::set_html(&view, "");
        let groups: Vec<(String, Vec<(String, String)>)> = nav.iter()
            .map(|(label, items)| (label.to_string(), items.iter().map(|(id, t)| (id.to_string(), t.to_string())).collect())).collect();
        let picked: Rc<RefCell<Option<Rc<Hub>>>> = Rc::new(RefCell::new(None));
        let p2 = picked.clone();
        let pick = SendWrapper::new(move |id: String| { if let Some(h) = p2.borrow().clone() { dom::spawn(show(&h, id)); } });
        // the page shown first is active from the start (no `class=""` left on the one shown before)
        PAGE.with(|p| p.set(current.clone()));
        dom::mount_view(&view, move || hub_view(groups, pick));
        if admin { diagnostics::update_banner(&view); }
        let panel = dom::qs_in(&view, "#panel").ok_or_else(|| "Settings: no panel".to_string())?;
        let focus = if !s.is_empty() && current != s { Some(s) } else { None };
        let hub = Rc::new(Hub {
            view: view.clone(), panel, ctx: ctx.clone(), values: RefCell::new(values), schema: schema.clone(), kit_schema: KitSchema::new(kit_schema),
            reveal: app::has_feature("reveal"), focus: RefCell::new(focus),
        });

        *picked.borrow_mut() = Some(hub.clone());
        dom::spawn(show(&hub, current));
        Ok(())
    })
}
