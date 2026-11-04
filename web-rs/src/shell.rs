//! The app frame, drawn by Leptos: the sidebar (brand, the nav with its badges, the storage meter's slot, Bug
//! reports, the account, Settings and Sign out), the phone's menu button, the notice banner's slot
//! (`#server-notice`, the kit's banner) and `#view`, the slot the hand-written router draws every page into.
//! The player bar is not part of it: `player::mount` appends `#player` to `body` after the shell, as before.
//!
//! The markup is the one `app.rs` `render_shell` wrote with `set_html`, node for node – the line breaks and
//! indents between the tags included, since they are text nodes of the page. The shell is mounted once; when
//! the account changes (`app::refresh_user`) the parts that show it (brand, Downloads and Soulseek entries,
//! avatar, name, role) are drawn again from the session, and `#view` is never touched by Leptos, so the page in
//! it stays until the router replaces it. The active nav link follows [`set_route`].

use crate::app::{avatar_html, has_feature, info, is_admin_user, user};
use crate::json::J;
use crate::ui::icon;
use crate::{appearance, dom, pages};
use kit_leptos::html::raw;
use leptos::prelude::*;
use std::cell::Cell;

thread_local! {
    /// Bumped when the session changes: the parts that show the account are drawn again.
    static TICK: ArcRwSignal<u32> = ArcRwSignal::new(0);
    /// The current route's path, for the active nav link.
    static ROUTE: ArcRwSignal<String> = ArcRwSignal::new(String::new());
    static MOUNTED: Cell<bool> = const { Cell::new(false) };
}

fn tick() -> ArcRwSignal<u32> { TICK.with(Clone::clone) }

/// The path the router shows: the matching nav link gets `active` (Home only for `/`, the others for their
/// path and what is under it).
pub fn set_route(path: &str) {
    ROUTE.with(|r| if r.get_untracked() != path { r.set(path.to_string()) });
}

fn is_active(href: &str, path: &str) -> bool {
    if href == "/" { path == "/" } else { path == href || path.starts_with(&format!("{href}/")) }
}

/// `<a href="#{href}" data-nav="{href}">{icon}<span>{label}</span>{extra}</a>`, `extra` a badge's id.
fn nav_link(href: &'static str, ic: &'static str, label: &'static str, badge: Option<&'static str>) -> AnyView {
    let route = ROUTE.with(Clone::clone);
    let extra = kit_leptos::html::opt(badge.map(|id| view! { <span {..()} class="badge warn hidden" id=id>{""}</span> }));
    view! {
        <a {..()} href=format!("#{href}") data-nav=href class:active=move || is_active(href, &route.get())>{raw(icon(ic))}<span>{label}</span>{extra}</a>
    }
    .into_any()
}

/// A nav link drawn only while `shown()` holds (Downloads, Soulseek), drawn again when the session changes.
fn nav_link_if(shown: fn() -> bool, href: &'static str, ic: &'static str, label: &'static str) -> impl IntoView {
    let t = tick();
    move || {
        t.track();
        if shown() { nav_link(href, ic, label, None) } else { "".into_any() }
    }
}

fn shell_view() -> impl IntoView {
    let (t1, t2, t3, t4) = (tick(), tick(), tick(), tick());
    let brand = move || {
        t1.track();
        let n = info().s("app_name");
        let name = if n.is_empty() { "Encore".to_string() } else { n };
        view! { {raw(appearance::logo(30))}{name} }
    };
    let avatar = move || { t2.track(); raw(user().map(|u| avatar_html(&u, 32)).unwrap_or_default()) };
    let username = move || { t3.track(); user().map(|u| u.s("username")).unwrap_or_default() };
    let role = move || {
        t4.track();
        let Some(u) = user() else { return "" };
        if u.s("role") == "owner" { "Owner" } else if is_admin_user(&u) { "Admin" } else { "Member" }
    };
    let logout = |_| dom::spawn(async {
        // a sign-out the server does not confirm is finished at the next start that reaches it (offline/mode.rs)
        crate::offline::mode::sign_out().await;
        dom::reload();
    });
    let menu = |e: web_sys::MouseEvent| {
        e.stop_propagation();
        if let Some(sb) = dom::qs("#sidebar") { let _ = sb.class_list().toggle("open"); }
    };
    view! {
        {"\n    "}
        <div class="shell">
            {"\n      "}
            <aside class="sidebar" id="sidebar">
                {"\n        "}
                <a class="brand" href="#/">{brand}</a>
                {"\n        "}
                <nav class="nav">
                    {"\n          "}{nav_link("/", "home", "Home", None)}
                    {"\n          "}{nav_link("/search", "search", "Discover", None)}
                    {"\n          "}{nav_link("/for-you", "spark", "For you", None)}
                    {"\n          "}{nav_link("/library", "library", "Library", None)}
                    {"\n          "}{nav_link_if(pages::downloads::enabled, "/downloads", "saved", "Downloads")}
                    {"\n          "}{nav_link("/requests", "inbox", "Requests", Some("pending-badge"))}
                    {"\n          "}{nav_link("/playlists", "list", "Playlists", None)}
                    {"\n          "}{nav_link("/download", "download", "Downloader", None)}
                    {"\n          "}{nav_link_if(|| has_feature("soulseek"), "/soulseek", "users", "Soulseek")}
                    {"\n          "}{nav_link("/leaderboard", "activity", "Leaderboard", None)}
                    {"\n        "}
                </nav>
                {"\n        "}
                <div {..()} id="storage-mini" style="flex:none">{""}</div>
                {"\n        "}
                <nav class="nav nav-under">
                    {"\n          "}{nav_link("/bug-reports", "bug", "Bug reports", Some("bugs-badge"))}
                    {"\n        "}
                </nav>
                {"\n        "}
                <div {..()} class="sidebar-foot" style="margin-top:0">
                    {"\n          "}
                    <a {..()} href="#/settings?s=account" class="row grow" style="min-width:0">
                        {"\n            "}{avatar}
                        {"\n            "}
                        <div class="grow"><div {..()} class="ellipsis" style="font-weight:650">{username}</div><div class="small faint">{role}</div></div>
                        {"\n          "}
                    </a>
                    {"\n          "}
                    <a class="btn ghost icon" href="#/settings" title="Settings" aria-label="Settings">{raw(icon("settings"))}</a>
                    {"\n          "}
                    <button class="btn ghost icon" id="logout-btn" title="Sign out" on:click=logout>{raw(icon("logout"))}</button>
                    {"\n        "}
                </div>
                {"\n      "}
            </aside>
            {"\n      "}
            <main class="main">
                {"\n        "}
                <button {..()} class="btn ghost icon mobile-only" id="menu-btn" style="margin:-6px 0 10px -6px" on:click=menu>{raw(icon("menu"))}</button>
                {"\n        "}
                <div id="server-notice">{""}</div>
                {"\n        "}
                // the router's slot: Leptos draws it once and never again (a signal here would wipe the page)
                <div id="view">{""}</div>
                {"\n      "}
            </main>
            {"\n    "}
        </div>
    }
}

/// `renderShell()`: draws the shell into `#app` the first time, after that draws the account's parts again.
/// Each call also does what the hand-written shell did on every draw: the title, the badges from scratch
/// (Bug reports asked again), the storage meter, and the document's right-click and click listeners.
pub fn render() {
    let Some(_) = user() else { return };
    let i = info();
    let n = i.s("app_name");
    dom::set_title(if n.is_empty() { "Encore" } else { &n });
    if MOUNTED.with(|m| m.replace(true)) {
        TICK.with(|t| t.update(|n| *n += 1));
        // the badges as a fresh shell has them until their next answer
        for id in ["pending-badge", "bugs-badge"] {
            if let Some(b) = dom::by_id(id) { b.set_text_content(Some("")); dom::add_class(&b, "hidden"); }
        }
    } else if let Some(el) = dom::by_id("app") {
        dom::set_html(&el, "");
        if let Some(m) = kit_leptos::rt::mount(&el, shell_view) { m.forever(); }
    }
}
