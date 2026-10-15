//! Port of web/js/app.js: the session, the hash router, the shell (sidebar + main), banners and boot.

use crate::dom::{self, by_id, document, qs, qs_in, qsa, set_html};
use crate::json::J;
use crate::ui::{self, esc, icon, toast};
use crate::{api, appearance, pages, player, remote};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use web_sys::Element;

/// `session = { user, info }`.
#[derive(Clone, Default)]
pub struct Session { pub user: Option<Value>, pub info: Value }

thread_local! {
    static SESSION: RefCell<Session> = RefCell::new(Session::default());
    static ROUTE_N: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub fn session() -> Session { SESSION.with(|s| s.borrow().clone()) }
pub fn user() -> Option<Value> { SESSION.with(|s| s.borrow().user.clone()) }
pub fn info() -> Value { SESSION.with(|s| s.borrow().info.clone()) }
pub fn set_user(u: Option<Value>) { SESSION.with(|s| s.borrow_mut().user = u); }
pub fn set_info(i: Value) { SESSION.with(|s| s.borrow_mut().info = i); }
/// `['admin','owner'].includes(user.role)`.
pub fn is_admin() -> bool { user().map(|u| matches!(u.s("role").as_str(), "admin" | "owner")).unwrap_or(false) }
pub fn is_admin_user(u: &Value) -> bool { matches!(u.s("role").as_str(), "admin" | "owner") }
/// `session.info.features.includes(f)`.
pub fn has_feature(f: &str) -> bool { info().a("features").iter().any(|x| x.as_str() == Some(f)) }

/// What a page gets: the route params, the query, the user and a place to hang cleanup on.
#[derive(Clone)]
pub struct Ctx {
    pub params: Vec<String>,
    pub query: HashMap<String, String>,
    pub user: Value,
    pub view: Element,
}
impl Ctx {
    pub fn param(&self, i: usize) -> String { self.params.get(i).cloned().unwrap_or_default() }
    pub fn q(&self, k: &str) -> String { self.query.get(k).cloned().unwrap_or_default() }
    /// The JS `onLeave(f)`: keep `x` only while this view is on screen.
    pub fn on_leave(&self, x: impl std::any::Any + 'static) { dom::keep_for(&self.view, x) }
}

pub type PageFuture = Pin<Box<dyn Future<Output = Result<(), String>>>>;
pub type PageFn = fn(Element, Ctx) -> PageFuture;

/// `navigate(path)`.
pub fn navigate(path: &str) { dom::set_hash(&format!("#{path}")); }

fn redirect(page: &str) {
    let _ = dom::window().history().and_then(|h| h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&format!("#/settings?s={page}"))));
    if let Ok(ev) = web_sys::HashChangeEvent::new("hashchange") { let _ = dom::window().dispatch_event(&ev); }
}

pub fn parse_hash() -> (String, HashMap<String, String>) {
    let raw = dom::hash();
    let raw = raw.strip_prefix('#').unwrap_or(&raw);
    let raw = if raw.is_empty() { "/" } else { raw };
    let (path, q) = raw.split_once('?').unwrap_or((raw, ""));
    let mut query = HashMap::new();
    if !q.is_empty() {
        if let Ok(p) = web_sys::UrlSearchParams::new_with_str(q) {
            for e in p.entries().into_iter().flatten() {
                if let Ok(a) = e.dyn_into::<js_sys::Array>() { query.insert(a.get(0).as_string().unwrap_or_default(), a.get(1).as_string().unwrap_or_default()); }
            }
        }
    }
    (path.to_string(), query)
}
use wasm_bindgen::JsCast;

/// Matches the same patterns as the JS route table; returns the captured params.
fn match_route(path: &str) -> Option<(&'static str, Vec<String>)> {
    let seg = |s: &str| -> Option<String> { if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') { Some(s.to_string()) } else { None } };
    let p = |prefix: &str| path.strip_prefix(prefix).filter(|r| !r.is_empty()).map(|r| r.to_string());
    Some(match path {
        "/" => ("home", vec![]),
        "/search" => ("search", vec![]),
        "/for-you" => ("for-you", vec![]),
        "/library" => ("library", vec![]),
        "/requests" => ("requests", vec![]),
        "/playlists" => ("playlists", vec![]),
        "/connections" => ("redirect:connections", vec![]),
        "/following" => ("nav:/for-you", vec![]),
        "/download" => ("download", vec![]),
        "/soulseek" => ("soulseek", vec![]),
        "/activity" => ("nav:/download", vec![]),
        "/admin/users" => ("redirect:users", vec![]),
        "/admin/invites" => ("redirect:invites", vec![]),
        "/settings" => ("settings", vec![]),
        "/admin/autodownload" => ("redirect:autodownload", vec![]),
        "/admin/diagnostics" => ("redirect:diagnostics", vec![]),
        "/account" => ("redirect:account", vec![]),
        "/storage" => ("storage", vec![]),
        "/leaderboard" => ("leaderboard", vec![]),
        _ => {
            if let Some(r) = p("/tag/") { return Some(("tag", vec![r])); }
            if let Some(r) = p("/artist/") { return seg(&r).map(|s| ("artist", vec![s])); }
            if let Some(r) = p("/artist-name/") { return Some(("artist-name", vec![r])); }
            if let Some(r) = p("/album/") { return seg(&r).map(|s| ("album", vec![s])); }
            if let Some(r) = p("/library/album/") { return seg(&r).map(|s| ("library-album", vec![s])); }
            if let Some(r) = p("/playlists/") { return if r.chars().all(|c| c.is_ascii_digit()) { Some(("playlist", vec![r])) } else { None }; }
            return None;
        }
    })
}

fn page_for(name: &str) -> Option<PageFn> {
    Some(match name {
        "home" => pages::home::render,
        "search" => pages::discover::render_search,
        "for-you" => pages::foryou::render,
        "tag" => pages::foryou::render_tag,
        "artist" => pages::discover::render_artist,
        "artist-name" => pages::discover::render_artist_by_name,
        "album" => pages::discover::render_album,
        "library" => pages::library::render,
        "library-album" => pages::library::render_album,
        "requests" => pages::requests::render,
        "playlists" => pages::playlists::render,
        "playlist" => pages::playlists::render_playlist,
        "download" => pages::download::render,
        "soulseek" => pages::soulseek::render,
        "settings" => pages::settings::render,
        "storage" => pages::storage::render,
        "leaderboard" => pages::leaderboard::render,
        _ => return None,
    })
}

/// `avatarHtml(u, size)`.
pub fn avatar_html(u: &Value, size: u32) -> String {
    let style = if size != 32 { format!(r#" style="width:{size}px;height:{size}px;font-size:{}px""#, (size as f64 * 0.42).round() as i64) } else { String::new() };
    if u.truthy("avatar_at") {
        format!(r#"<img class="avatar" src="/api/users/{}/avatar?v={}" alt=""{style}>"#, u.s("id"), u.f("avatar_at").round() as i64)
    } else {
        let name = u.s("username");
        let first = name.chars().next().unwrap_or('?').to_string();
        format!(r#"<div class="avatar"{style}>{}</div>"#, esc(&first))
    }
}

fn nav_link(href: &str, ic: &str, label: &str, extra: &str) -> String {
    format!(r##"<a href="#{href}" data-nav="{href}">{ic}<span>{label}</span>{extra}</a>"##)
}

fn render_shell() {
    let Some(u) = user() else { return };
    let admin = is_admin_user(&u);
    let info = info();
    let app_name_raw = { let n = info.s("app_name"); if n.is_empty() { "Encore".to_string() } else { n } };
    let app_name = esc(&app_name_raw);
    dom::set_title(&app_name_raw);
    let app = by_id("app").expect("#app");
    set_html(&app, &format!(r##"
    <div class="shell">
      <aside class="sidebar" id="sidebar">
        <a class="brand" href="#/">{logo}{app_name}</a>
        <nav class="nav">
          {home}
          {search}
          {foryou}
          {library}
          {requests}
          {playlists}
          {download}
          {soulseek}
          {leaderboard}
        </nav>
        <div id="storage-mini" style="flex:none"></div>
        <div class="sidebar-foot" style="margin-top:0">
          <a href="#/settings?s=account" class="row grow" style="min-width:0">
            {avatar}
            <div class="grow"><div class="ellipsis" style="font-weight:650">{username}</div><div class="small faint">{role}</div></div>
          </a>
          <a class="btn ghost icon" href="#/settings" title="Settings" aria-label="Settings">{settings}</a>
          <button class="btn ghost icon" id="logout-btn" title="Sign out">{logout}</button>
        </div>
      </aside>
      <main class="main">
        <button class="btn ghost icon mobile-only" id="menu-btn" style="margin:-6px 0 10px -6px">{menu}</button>
        <div id="view"></div>
      </main>
    </div>"##,
        logo = appearance::logo(30),
        home = nav_link("/", &icon("home"), "Home", ""),
        search = nav_link("/search", &icon("search"), "Discover", ""),
        foryou = nav_link("/for-you", &icon("spark"), "For you", ""),
        library = nav_link("/library", &icon("library"), "Library", ""),
        requests = nav_link("/requests", &icon("inbox"), "Requests", r#"<span class="badge warn hidden" id="pending-badge"></span>"#),
        playlists = nav_link("/playlists", &icon("list"), "Playlists", ""),
        download = nav_link("/download", &icon("download"), "Downloader", ""),
        soulseek = if has_feature("soulseek") { nav_link("/soulseek", &icon("users"), "Soulseek", "") } else { String::new() },
        leaderboard = nav_link("/leaderboard", &icon("activity"), "Leaderboard", ""),
        avatar = avatar_html(&u, 32),
        username = esc(&u.s("username")),
        role = if u.s("role") == "owner" { "Owner" } else if admin { "Admin" } else { "Member" },
        settings = icon("settings"), logout = icon("logout"), menu = icon("menu"),
    ));
    if let Some(b) = qs("#logout-btn") {
        dom::on_forever(&b, "click", |_| dom::spawn(async {
            let _ = api::post_empty("/auth/logout").await;
            // the desktop app's Cadenza signed in with its own token: signing out here signs it out too
            native_call("engineLogout");
            dom::reload();
        }));
    }
    let meter = || { if let Some(el) = qs("#storage-mini") { pages::storage::sidebar_meter(el); } };
    meter();
    dom::interval_forever(5 * 60 * 1000, meter);
    if let Some(b) = qs("#menu-btn") {
        dom::on_forever(&b, "click", |e| { e.stop_propagation(); if let Some(sb) = qs("#sidebar") { let _ = sb.class_list().toggle("open"); } });
    }
    dom::on_forever(&document(), "click", |e| {
        if let Some(sb) = qs("#sidebar") {
            if dom::has_class(&sb, "open") {
                let inside = e.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()).map(|n| sb.contains(Some(&n))).unwrap_or(false);
                if !inside { dom::remove_class(&sb, "open"); }
            }
        }
    });
}

async fn refresh_pending_badge() {
    if !is_admin() { return; }
    if let Ok(rows) = api::get("/requests?status=pending&limit=500").await {
        let n = rows.items().len();
        if let Some(b) = qs("#pending-badge") {
            b.set_text_content(Some(&n.to_string()));
            dom::toggle_class(&b, "hidden", n == 0);
        }
    }
}

/// `route()`: render the page for the current hash.
pub fn route() { dom::spawn(route_async()); }

async fn route_async() {
    dom::clear_page();
    if let Some(m) = by_id("modal-root") { set_html(&m, ""); }
    let (path, query) = parse_hash();
    let Some(u) = user() else {
        if let Some(app) = by_id("app") { pages::auth::render(app, path, query).await; }
        return;
    };
    if qs("#view").is_none() { render_shell(); }
    if let Some(sb) = qs("#sidebar") { dom::remove_class(&sb, "open"); }
    for a in qsa("[data-nav]") {
        let href = dom::data(&a, "nav");
        dom::toggle_class(&a, "active", if href == "/" { path == "/" } else { path.starts_with(&href) });
    }
    // every navigation renders into a fresh container, so a slow page that finishes after the user
    // has moved on writes into a detached element instead of clobbering the current page
    let view = dom::create("div");
    if let Some(v) = qs("#view") { let _ = v.replace_children_with_node_1(&view); }
    let Some((name, params)) = match_route(&path) else {
        set_html(&view, r##"<div class="empty"><h2>Page not found</h2><p class="muted"><a href="#/">Go home</a></p></div>"##);
        return;
    };
    if let Some(page) = name.strip_prefix("redirect:") { redirect(page); return; }
    if let Some(target) = name.strip_prefix("nav:") { navigate(target); return; }
    let Some(f) = page_for(name) else { return };
    dom::scroll_top();
    let ctx = Ctx { params, query, user: u, view: view.clone() };
    if let Err(e) = f(view.clone(), ctx).await {
        if !view.is_connected() { return; }
        dom::console_error(&e);
        set_html(&view, &format!(r#"<div class="error-box">{}</div>"#, esc(&e)));
    }
}

/// `boot()`.
pub async fn boot() {
    appearance::apply(&appearance::current());
    set_info(api::get("/auth/info").await.unwrap_or_else(|_| serde_json::json!({})));
    set_user(api::get("/auth/me").await.ok());
    if let Some(u) = user() { appearance::adopt_account(&u); crate::playback::adopt_account(&u); crate::ui::set_translate_mode(&u.o("prefs").s("translate_mode")); }
    else { appearance::apply(&appearance::brand()); }
    if user().is_some() {
        render_shell();
        remote::start();
        player::mount();
        remote::resume();
        dom::spawn(refresh_pending_badge());
        dom::interval_forever(30000, || dom::spawn(refresh_pending_badge()));
    }
    route();
}

/// `refreshUser()`.
pub async fn refresh_user() {
    if let Ok(u) = api::get("/auth/me").await {
        crate::ui::set_translate_mode(&u.o("prefs").s("translate_mode"));
        appearance::apply(&appearance::current());
        appearance::adopt_account(&u);
        set_user(Some(u));
    }
    render_shell();
    route();
}

fn conn_banner(on: bool, text: &str) {
    let b = by_id("conn-banner");
    if !on { if let Some(b) = b { b.remove(); } return; }
    let b = match b { Some(b) => b, None => { let b = dom::create("div"); b.set_id("conn-banner"); let _ = dom::body().append_child(&b); b } };
    let native = js_sys::Reflect::get(&dom::window(), &"EncoreNative".into()).ok().filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|n| js_sys::Reflect::get(&n, &"openOffline".into()).ok()).map(|f| f.is_function()).unwrap_or(false);
    set_html(&b, &format!(r#"{}<span>{}</span>{}"#, icon("x"), esc(text), if native { r#"<button class="btn sm" id="go-offline">Offline music</button>"# } else { "" }));
    if let Some(btn) = qs_in(&b, "#go-offline") {
        dom::on_forever(&btn, "click", |_| { native_call("openOffline"); });
    }
}

/// `window.EncoreNative.<name>(arg)` when the app provides it.
pub fn native_call(name: &str) { native_call_with(name, None); }
pub fn native_call_with(name: &str, arg: Option<&str>) {
    let Ok(n) = js_sys::Reflect::get(&dom::window(), &"EncoreNative".into()) else { return };
    if n.is_undefined() || n.is_null() { return; }
    let Ok(f) = js_sys::Reflect::get(&n, &name.into()) else { return };
    let Ok(f) = f.dyn_into::<js_sys::Function>() else { return };
    let _ = match arg { Some(a) => f.call1(&n, &a.into()), None => f.call0(&n) };
}
/// `!!window.EncoreNative?.<name>` for a plain value (e.g. `engine: true`), not only functions.
pub fn native_truthy(name: &str) -> bool {
    js_sys::Reflect::get(&dom::window(), &"EncoreNative".into()).ok().filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|n| js_sys::Reflect::get(&n, &name.into()).ok()).map(|v| v.is_truthy()).unwrap_or(false)
}
/// `await window.EncoreNative.<name>(arg)` as JSON – None when the app has no such function or it failed.
/// A JSON string answer is parsed like the JS does (`typeof s === 'string' ? JSON.parse(s) : s`).
pub async fn native_call_async(name: &str, arg: Option<&str>) -> Option<Value> {
    let n = js_sys::Reflect::get(&dom::window(), &"EncoreNative".into()).ok().filter(|v| !v.is_undefined() && !v.is_null())?;
    let f = js_sys::Reflect::get(&n, &name.into()).ok()?.dyn_into::<js_sys::Function>().ok()?;
    let r = match arg { Some(a) => f.call1(&n, &a.into()), None => f.call0(&n) }.ok()?;
    let v = match r.dyn_into::<js_sys::Promise>() { Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await.ok()?, Err(v) => v };
    if let Some(s) = v.as_string() { return serde_json::from_str(&s).ok(); }
    serde_wasm_bindgen::from_value::<Value>(v).ok()
}
/// `!!window.EncoreNative?.<name>`.
pub fn native_has(name: &str) -> bool {
    js_sys::Reflect::get(&dom::window(), &"EncoreNative".into()).ok().filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|n| js_sys::Reflect::get(&n, &name.into()).ok()).map(|f| f.is_function()).unwrap_or(false)
}

/// Module-load work of app.js: listeners, service worker, boot.
pub fn init() {
    dom::on_window("hashchange", |_| route());
    let loc = dom::location();
    let proto = loc.protocol().unwrap_or_default();
    let host = loc.hostname().unwrap_or_default();
    if proto == "https:" || host == "localhost" {
        let _ = dom::window().navigator().service_worker().register("/sw.js");
    }
    dom::on_window("encore:unauthorized", |_| {
        if user().is_some() {
            set_user(None);
            toast("Your session expired – please sign in again", "err");
            dom::set_hash("#/login");
            dom::reload();
        }
    });
    dom::on_window("encore:requests-changed", |_| dom::spawn(refresh_pending_badge()));
    dom::on_window("offline", |_| conn_banner(true, "Not connected – check your internet or server"));
    dom::on_window("encore-native", |e| {
        if let Some(ce) = e.dyn_ref::<web_sys::CustomEvent>() {
            let d = ce.detail();
            let text = js_sys::Reflect::get(&d, &"text".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
            if !text.is_empty() {
                let kind = js_sys::Reflect::get(&d, &"kind".into()).ok().and_then(|v| v.as_string()).unwrap_or_else(|| "ok".into());
                toast(&text, &kind);
            }
        }
    });
    dom::on_window("online", |_| conn_banner(false, ""));
    dom::on_window("encore:server-down", |_| conn_banner(true, "The Encore server isn't answering"));
    dom::on_window("encore:server-up", |_| conn_banner(false, ""));
    // the JS exposes the session for the pages; here they read app::user()
    let _ = ui::icon("x");
    let _: Rc<()> = Rc::new(());
    let _ = ROUTE_N.with(|n| n.get());
    dom::spawn(boot());
}
