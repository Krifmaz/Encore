//! Port of web/js/app.js: the session, the hash router, the shell (sidebar + main), banners and boot.

use crate::dom::{self, by_id, document, qs, qsa, set_html};
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
    /// The last hash that was not a Bug reports page: a new report says which page the user came from.
    static PREV_HASH: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The hash the user was on before opening Bug reports ("#/library?s=…"), "" right after boot.
pub fn previous_hash() -> String { PREV_HASH.with(|h| h.borrow().clone()) }

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
        "/downloads" => ("downloads", vec![]),
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
        "/bug-reports" => ("bug-reports", vec![]),
        _ => {
            if let Some(r) = p("/bug-reports/") { return if r.chars().all(|c| c.is_ascii_digit()) { Some(("bug-report", vec![r])) } else { None }; }
            if let Some(r) = p("/tag/") { return Some(("tag", vec![r])); }
            if let Some(r) = p("/artist/") { return seg(&r).map(|s| ("artist", vec![s])); }
            if let Some(r) = p("/artist-name/") { return Some(("artist-name", vec![r])); }
            if let Some(r) = p("/album/") { return seg(&r).map(|s| ("album", vec![s])); }
            if let Some(r) = p("/library/album/") { return seg(&r).map(|s| ("library-album", vec![s])); }
            if let Some(r) = p("/playlists/") { return if r.chars().all(|c| c.is_ascii_digit()) { Some(("playlist", vec![r])) } else { None }; }
            // a group listen's invite link (0.31): "Join <host>'s group listen?"
            if let Some(r) = p("/group/") { return seg(&r).map(|s| ("group", vec![s])); }
            // a collaborative playlist's invite link (0.37): "Join <owner>'s playlist?"
            if let Some(r) = p("/collab/") { return seg(&r).map(|s| ("collab", vec![s])); }
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
        "downloads" => pages::downloads::render,
        "soulseek" => pages::soulseek::render,
        "settings" => crate::settings::render,
        "storage" => pages::storage::render,
        "leaderboard" => pages::leaderboard::render,
        "bug-reports" => pages::bugreports::render,
        "bug-report" => pages::bugreports::render_one,
        "group" => crate::group::ui::render_link,
        "collab" => pages::collab::render_link,
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
          {downloads_nav}
          {requests}
          {playlists}
          {download}
          {soulseek}
          {leaderboard}
        </nav>
        <div id="storage-mini" style="flex:none"></div>
        <nav class="nav nav-under">
          {bugs}
        </nav>
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
        <div id="server-notice"></div>
        <div id="view"></div>
      </main>
    </div>"##,
        logo = appearance::logo(30),
        home = nav_link("/", &icon("home"), "Home", ""),
        search = nav_link("/search", &icon("search"), "Discover", ""),
        foryou = nav_link("/for-you", &icon("spark"), "For you", ""),
        library = nav_link("/library", &icon("library"), "Library", ""),
        downloads_nav = if pages::downloads::enabled() { nav_link("/downloads", &icon("saved"), "Downloads", "") } else { String::new() },
        requests = nav_link("/requests", &icon("inbox"), "Requests", r#"<span class="badge warn hidden" id="pending-badge"></span>"#),
        playlists = nav_link("/playlists", &icon("list"), "Playlists", ""),
        download = nav_link("/download", &icon("download"), "Downloader", ""),
        soulseek = if has_feature("soulseek") { nav_link("/soulseek", &icon("users"), "Soulseek", "") } else { String::new() },
        leaderboard = nav_link("/leaderboard", &icon("activity"), "Leaderboard", ""),
        bugs = nav_link("/bug-reports", &icon("bug"), "Bug reports", r#"<span class="badge warn hidden" id="bugs-badge"></span>"#),
        avatar = avatar_html(&u, 32),
        username = esc(&u.s("username")),
        role = if u.s("role") == "owner" { "Owner" } else if admin { "Admin" } else { "Member" },
        settings = icon("settings"), logout = icon("logout"), menu = icon("menu"),
    ));
    // the admin's banner of a downloads folder that can't be written, on every page (notice.rs)
    crate::notice::paint();
    if let Some(b) = qs("#logout-btn") {
        dom::on_forever(&b, "click", |_| dom::spawn(async {
            // a sign-out the server does not confirm is finished at the next start that reaches it (offline/mode.rs)
            crate::offline::mode::sign_out().await;
            dom::reload();
        }));
    }
    let meter = || { if let Some(el) = qs("#storage-mini") { pages::storage::sidebar_meter(el); } };
    meter();
    dom::interval_forever(5 * 60 * 1000, meter);
    dom::spawn(refresh_bugs_badge());
    if let Some(b) = qs("#menu-btn") {
        dom::on_forever(&b, "click", |e| { e.stop_propagation(); if let Some(sb) = qs("#sidebar") { let _ = sb.class_list().toggle("open"); } });
    }
    // Right-click (or a long press on a phone) opens the same menu as the row's ⋯ button, at the pointer:
    // climb from what was clicked to the first element that holds exactly one ⋯ button (a track row, a
    // request, the album header, the player bar), and press it. Text fields and the open menu keep the
    // browser's own menu; where there is no ⋯ nothing changes.
    dom::on_forever(&document(), "contextmenu", |e| {
        const MORE: &str = r#"button[aria-label="More"], button[aria-label="More options"], [data-act="menu"], [data-tmenu], #p-more, #f-more"#;
        let Some(target) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) else { return };
        if target.closest("input, textarea, select, [contenteditable], .menu-pop, .modal").ok().flatten().is_some() { return; }
        let mut node = Some(target);
        let mut hops = 0;
        while let Some(el) = node {
            if hops > 12 || el.tag_name() == "BODY" { break; }
            let found = el.query_selector_all(MORE).map(|l| l.length()).unwrap_or(0);
            if found == 1 {
                if let Some(b) = el.query_selector(MORE).ok().flatten() {
                    if let Some(m) = e.dyn_ref::<web_sys::MouseEvent>() { ui::next_menu_at(m.client_x() as f64, m.client_y() as f64); }
                    e.prevent_default();
                    if let Some(h) = b.dyn_ref::<web_sys::HtmlElement>() { h.click(); }
                }
                return;
            }
            if found > 1 { return; }
            node = el.parent_element();
            hops += 1;
        }
    });
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

/// The sidebar's Bug reports badge: reports with something new for this user (GET /api/bug-reports/unread).
async fn refresh_bugs_badge() {
    if user().is_none() { return; }
    if let Ok(d) = api::get("/bug-reports/unread").await {
        let n = d.i("count");
        if let Some(b) = qs("#bugs-badge") {
            b.set_text_content(Some(&n.to_string()));
            dom::toggle_class(&b, "hidden", n <= 0);
        }
    }
}

/// `route()`: render the page for the current hash.
pub fn route() { dom::spawn(route_async()); }

async fn route_async() {
    dom::clear_page();
    if let Some(m) = by_id("modal-root") { set_html(&m, ""); }
    let (path, query) = parse_hash();
    let path = crate::offline::mode::route_path(path);
    let Some(u) = user() else {
        // an invite link opened by someone who is not signed in: the group listen's guest page
        if let Some(code) = path.strip_prefix("/group/").filter(|c| !c.is_empty()) {
            if let Some(app) = by_id("app") { crate::group::guest::render(app, code.to_string()).await; }
            return;
        }
        if let Some(app) = by_id("app") { pages::auth::render(app, path, query).await; }
        return;
    };
    if qs("#view").is_none() { render_shell(); }
    if !path.starts_with("/bug-reports") { PREV_HASH.with(|h| *h.borrow_mut() = dom::hash()); }
    if let Some(sb) = qs("#sidebar") { dom::remove_class(&sb, "open"); }
    for a in qsa("[data-nav]") {
        let href = dom::data(&a, "nav");
        dom::toggle_class(&a, "active", if href == "/" { path == "/" } else { path == href || path.starts_with(&format!("{href}/")) });
    }
    // every navigation renders into a fresh container, so a slow page that finishes after the user
    // has moved on writes into a detached element instead of clobbering the current page
    let view = dom::create("div");
    if let Some(v) = qs("#view") { v.replace_children_with_node_1(&view); }
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
    // where offline mode could open (or a sign-out waits for the server), a server that doesn't answer gets a
    // few seconds, not the connect timeout
    let quick = crate::offline::signout_pending() || crate::offline::mode::could_open().await;
    let wait = quick.then_some(crate::offline::mode::WAIT_MS);
    let ask = |path: &'static str| async move { match wait { Some(ms) => api::get_within(path, ms).await, None => api::get(path).await } };
    let (info, me) = futures::join!(ask("/auth/info"), ask("/auth/me"));
    set_info(info.unwrap_or_else(|_| serde_json::json!({})));
    let me = crate::offline::mode::settle_sign_out(me).await;
    // the server can't be reached but songs are downloaded here: offline mode (offline/mode.rs)
    let offline = match &me { Err(e) => crate::offline::mode::try_enter(e.status).await, Ok(_) => false };
    if !offline { set_user(me.ok()); }
    if let Some(u) = user() { appearance::adopt_account(&u); crate::playback::adopt_account(&u); crate::ui::set_translate_mode(&u.o("prefs").s("translate_mode")); }
    else { appearance::apply(&appearance::brand()); }
    if offline {
        render_shell();
        player::mount();
        crate::offline::mode::shell_ready();
    } else if let Some(u) = user() {
        crate::offline::mode::signed_in(&u);
        render_shell();
        remote::start();
        player::mount();
        remote::resume();
        // back into the group listen this browser was in, and a group link opened before signing in
        crate::group::start();
        dom::spawn(refresh_pending_badge());
        dom::interval_forever(30000, || dom::spawn(refresh_pending_badge()));
        dom::interval_forever(60000, || dom::spawn(refresh_bugs_badge()));
        crate::notice::start();
        crate::whats_new::start();
    }
    route();
}

/// `refreshUser()`.
pub async fn refresh_user() {
    if let Ok(u) = api::get("/auth/me").await {
        crate::offline::remember_me(&u);
        crate::ui::set_translate_mode(&u.o("prefs").s("translate_mode"));
        appearance::apply(&appearance::current());
        appearance::adopt_account(&u);
        set_user(Some(u));
    }
    render_shell();
    route();
}

fn conn_banner(on: bool, text: &str) {
    // offline mode has its own banner
    if on && crate::offline::mode::active() { return; }
    let b = by_id("conn-banner");
    if !on { if let Some(b) = b { b.remove(); } return; }
    let b = match b { Some(b) => b, None => { let b = dom::create("div"); b.set_id("conn-banner"); let _ = dom::body().append_child(&b); b } };
    set_html(&b, &format!(r#"{}<span>{}</span>"#, icon("x"), esc(text)));
}

/// Module-load work of app.js: listeners, service worker, boot.
pub fn init() {
    dom::on_window("hashchange", |_| route());
    // wherever the browser allows it (https, localhost, 127.0.0.1, *.localhost): the same test as downloads
    if dom::window().is_secure_context() {
        crate::sw::register();
    }
    // reload into a new version of the web app once the server serves one (update.rs)
    crate::update::start();
    dom::on_window("encore:unauthorized", |_| {
        if user().is_some() {
            set_user(None);
            toast("Your session expired – please sign in again", "err");
            dom::set_hash("#/login");
            dom::reload();
        }
    });
    dom::on_window("encore:requests-changed", |_| dom::spawn(refresh_pending_badge()));
    dom::on_window("encore:bugs-changed", |_| dom::spawn(refresh_bugs_badge()));
    dom::on_window("offline", |_| conn_banner(true, "Not connected – check your internet or server"));
    dom::on_window("online", |_| conn_banner(false, ""));
    dom::on_window("encore:server-down", |_| conn_banner(true, "The Encore server isn't answering"));
    dom::on_window("encore:server-up", |_| conn_banner(false, ""));
    // the JS exposes the session for the pages; here they read app::user()
    let _ = ui::icon("x");
    let _: Rc<()> = Rc::new(());
    let _ = ROUTE_N.with(|n| n.get());
    dom::spawn(boot());
}
