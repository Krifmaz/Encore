//! Account: profile + picture, password, username, email, apps, two-step sign-in, active sessions and
//! (for members) the About card.
//!
//! A Leptos view ([`Account`]) of GET /api/auth/me, with signals for what opens and closes on the page and the
//! sessions list. Turning two-step sign-in on or off draws the page again from a new answer, as before.

use super::general;
use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, json, truthy, J};
use crate::ui::{confirm_dialog, esc, fmt, icon, toast};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::Value;
use wasm_bindgen::JsCast;
use web_sys::{Element, FormData, HtmlInputElement};

/// "Encore app · Pixel 9 Pro · last used 2h ago" for the native app's sign-ins, the browser otherwise.
fn session_title(s: &Value) -> String {
    if s.s("kind") == "app" {
        let last = format!("last used {}", fmt::ago(if s.f("last_seen_at") > 0.0 { s.f("last_seen_at") } else { s.f("created_at") }));
        ["Encore app", s.st("device_name").unwrap_or(""), last.as_str()].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" · ")
    } else {
        s.st("device_name").or(s.st("user_agent")).unwrap_or("Unknown device").to_string()
    }
}

/// The Apps card's address line for a page opened at `origin`, from `GET /api/auth/addresses`
/// (`{public, local}`): which address this browser uses, and the local one as a link while on the
/// public one. Empty when the server has no Local network address.
pub(crate) fn address_line(origin: &str, v: &Value) -> String {
    let local = v.s("local").trim().trim_end_matches('/').to_string();
    if local.is_empty() {
        return String::new();
    }
    let here = origin.trim_end_matches('/');
    if here.eq_ignore_ascii_case(&local) {
        return format!("This browser uses the server's <b>local network address</b> ({}).", esc(&local));
    }
    format!(
        r#"This browser uses {}. On your home network <a href="{}" target="_blank" rel="noopener">{}</a> is faster (sign in there once); the desktop, phone and TV apps switch to it on their own."#,
        if v.s("public").trim_end_matches('/').eq_ignore_ascii_case(here) { "the server's <b>public address</b>" } else { "this address" },
        esc(&local),
        esc(&local),
    )
}

/// The Account page: the view of GET /api/auth/me (its failure is the page's error), the About card for
/// members, then the server's addresses and the sessions, loaded after.
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let me = api::get("/auth/me").await.map_err(|e| e.message)?;
        let admin = matches!(me.s("role").as_str(), "admin" | "owner");
        let about = if admin { String::new() } else { general::about_card().await };
        dom::set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let page = owner.with(|| Page {
            name_open: RwSignal::new(false),
            pw_open: RwSignal::new(false),
            twofa: RwSignal::new(TwoFa::Start),
            address: RwSignal::new(None),
            sessions: RwSignal::new(None),
        });
        let again = (view.clone(), ctx);
        let again = StoredValue::new_local(again);
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Account me page about again/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        // which of the server's addresses this browser uses (the apps switch on their own; a browser can't
        // carry its sign-in to another address, so it only says so and links the other one)
        if app::has_feature("addresses") {
            dom::spawn(async move {
                let Ok(v) = api::get("/auth/addresses").await else { return };
                let line = address_line(&dom::location().origin().unwrap_or_default(), &v);
                if !line.is_empty() {
                    page.address.try_set(Some(line));
                }
            });
        }
        page.load_sessions();
        Ok(())
    })
}

/// The JS `render(view, ctx)` called again from a handler (not awaited; a failure only reaches the console).
fn rerender(view: Element, ctx: Ctx) {
    dom::spawn(async move { if let Err(e) = render(view, ctx).await { dom::console_error(&e); } });
}

/// What the Two-step sign-in card shows under its line.
#[derive(Clone)]
enum TwoFa {
    /// Set up (when off) or Turn off (when on).
    Start,
    /// Setting up: the QR code (base64 PNG, may be empty) and the key, with the box for the first code.
    Setup(String, String),
    /// Turning off: the box for the password.
    Off,
}

/// The page's state.
#[derive(Clone, Copy)]
struct Page {
    /// The username editor is open.
    name_open: RwSignal<bool>,
    /// The password form is open.
    pw_open: RwSignal<bool>,
    /// The two-step sign-in card.
    twofa: RwSignal<TwoFa>,
    /// The Apps card's line about the server's addresses (HTML; `None`: hidden).
    address: RwSignal<Option<String>>,
    /// The signed-in devices (`None`: still loading).
    sessions: RwSignal<Option<Vec<Value>>>,
}

impl Page {
    /// Loads the sessions again (a failure keeps the list as it is).
    fn load_sessions(self) {
        dom::spawn(async move {
            if let Ok(rows) = api::get("/auth/sessions").await {
                self.sessions.try_set(Some(rows.items()));
            }
        });
    }
}

/// One signed-in device: what it is, where from and since when; Revoke, or "This device".
fn session_row(page: Page, s: &Value) -> impl IntoView {
    let sid = s.s("id");
    let end = if s.truthy("current") {
        view! { <span class="badge ok">"This device"</span> }.into_any()
    } else {
        let revoke = move |_| {
            let sid = sid.clone();
            dom::spawn(async move {
                if api::del(&format!("/auth/sessions/{sid}")).await.is_ok() {
                    page.load_sessions();
                }
            });
        };
        view! { <button class="btn sm danger" on:click={revoke} {..html(format!("{}Revoke", icon("logout")))}>{""}</button> }.into_any()
    };
    view! {
        <div class="list-item" data-sid=s.s("id")>
            <div class="grow"><div class="ellipsis" style="font-weight:600">{session_title(s)}</div>
                <div class="small muted">{format!("{} · signed in {}", s.s("ip"), fmt::ago(s.f("created_at")))}</div></div>
            {end}
        </div>
    }
}

/// The form's named input's value.
fn field(e: &web_sys::SubmitEvent, name: &str) -> String {
    dom::target_el(e).map(|f| dom::value_of(&f, &format!("[name={name}]"))).unwrap_or_default()
}

/// Uploads the picked picture as the account's picture (POST /api/auth/avatar, multipart).
async fn upload_avatar(f: web_sys::File) {
    let Ok(fd) = FormData::new() else { return };
    let _ = fd.append_with_blob("file", &f);
    let req = gloo_net::http::RequestBuilder::new("/api/auth/avatar")
        .method(gloo_net::http::Method::POST)
        .header("X-Requested-With", "encore")
        .credentials(web_sys::RequestCredentials::SameOrigin)
        .body(fd);
    let Ok(req) = req else { return };
    let Ok(r) = req.send().await else { return };
    if !r.ok() {
        // the answer's `detail`, else "Upload failed"
        let data: Value = r.text().await.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
        let detail = data.g("detail");
        toast(&if truthy(detail) { js_string(detail) } else { "Upload failed".to_string() }, "err");
        return;
    }
    toast("Picture updated", "ok");
    dom::spawn(app::refresh_user());
}

/// The Account page.
#[component]
fn Account(
    /// The answer of GET /api/auth/me.
    me: Value,
    /// The page's state.
    page: Page,
    /// The About card for members (HTML; empty for admins).
    about: String,
    /// The page's element and context, to draw it again after two-step sign-in changes.
    again: StoredValue<(Element, Ctx), LocalStorage>,
) -> impl IntoView {
    let has = app::has_feature;
    let admin = matches!(me.s("role").as_str(), "admin" | "owner");
    let username = me.s("username");
    let name_input = NodeRef::<leptos::html::Input>::new();
    let pw_form = NodeRef::<leptos::html::Form>::new();

    // username: the pencil opens the editor (with a warning when there's no email to fall back on)
    let name_edit = has("rename").then(|| {
        let toggle = move |_| {
            let open = !page.name_open.get_untracked();
            page.name_open.set(open);
            if open {
                dom::timeout(0, move || { if let Some(i) = name_input.get_untracked() { let _ = i.focus(); } });
            }
        };
        view! { <button class="btn ghost icon sm" type="button" id="name-edit" title="Change username" aria-label="Change username" on:click={toggle} {..html(icon("edit"))}>{""}</button> }
    });
    let rename = has("rename").then(|| {
        let name = username.clone();
        let cancel = move |_| {
            page.name_open.set(false);
            if let Some(i) = name_input.get_untracked() { i.set_value(&name); }
        };
        let save = move |e: web_sys::SubmitEvent| {
            e.prevent_default();
            let username = field(&e, "username").trim().to_string();
            dom::spawn(async move {
                match api::put("/auth/profile", json!({ "username": username })).await {
                    Ok(r) => {
                        toast(&format!("You're now {}", r.s("username")), "ok");
                        app::refresh_user().await;
                    }
                    Err(err) => toast(&err.message, "err"),
                }
            });
        };
        let warn = (!me.truthy("email")).then(|| view! {
            <p class="small" id="name-warn" style="margin:0;color:var(--warn)">"⚠ There's no email on this account, so your username is the only way you'll be able to sign in – make it one you'll remember. Add an email below to be able to reset your password."</p>
        });
        view! {
            <form class="stack" id="name-form" hidden=move || !page.name_open.get() on:submit=save>
                <div class="row wrap" style="gap:10px"><input class="input grow" name="username" value=username.clone() autocomplete="username" minlength="3" maxlength="32" required style="min-width:220px" node_ref=name_input/>
                    <button class="btn primary" type="submit">"Save"</button><button class="btn ghost" type="button" id="name-cancel" on:click=cancel>"Cancel"</button></div>
                <p class="small muted" style="margin:0">"What you sign in with and what others see. 3–32 letters, numbers, . _ -"</p>
                {warn}
            </form>
        }
    });
    let avatar_block = has("avatar").then(|| {
        let up = { let u = icon("upload"); if u.is_empty() { icon("plus") } else { u } };
        let picked = move |e: web_sys::Event| {
            let Some(f) = dom::target_el(&e).and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).and_then(|i| i.files()).and_then(|l| l.get(0)) else { return };
            dom::spawn(upload_avatar(f));
        };
        let remove = me.truthy("avatar_at").then(|| view! {
            <button class="btn sm ghost" id="avatar-remove" on:click=|_| dom::spawn(async {
                if api::del("/auth/avatar").await.is_ok() { dom::spawn(app::refresh_user()); }
            })>"Remove"</button>
        });
        view! {
            <div class="row wrap" style="gap:8px" data-dropzone><label class="btn sm">{raw(up)}"Change picture"<input type="file" id="avatar-file" accept="image/*" hidden on:change=picked/></label>
                {remove}</div>
        }
    });
    let role = if me.s("role") == "owner" { "Owner" } else if admin { "Administrator" } else { "Member" };
    let limit = (me.truthy("request_limit") && !admin).then(|| format!(" of {}", me.s("request_limit")));
    let auto = (me.truthy("auto_approve") || admin).then(|| view! { " · "<span class="badge ok">"auto-approved"</span> });

    // password: a button that opens the form
    let show_pw = move |on: bool| {
        page.pw_open.set(on);
        if on {
            dom::timeout(0, move || {
                if let Some(i) = pw_form.get_untracked().and_then(|f| dom::qs_in(&f, "[name=current]")) { dom::focus(&i); }
            });
        } else if let Some(f) = pw_form.get_untracked() {
            f.reset();
        }
    };
    let change_pw = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let body = json!({ "current": field(&e, "current"), "new": field(&e, "new") });
        dom::spawn(async move {
            match api::post("/auth/password", body).await {
                Ok(_) => {
                    toast("Password updated – other sessions signed out", "ok");
                    show_pw(false);
                    page.load_sessions();
                }
                Err(err) => toast(&err.message, "err"),
            }
        });
    };
    let save_email = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let email = field(&e, "email");
        dom::spawn(async move {
            match api::put("/auth/profile", json!({ "email": email })).await {
                Ok(_) => toast("Email saved", "ok"),
                Err(err) => toast(&err.message, "err"),
            }
        });
    };

    // two-step sign-in
    let totp_on = me.truthy("totp_enabled");
    let redraw = move || again.with_value(|(v, c)| rerender(v.clone(), c.clone()));
    let twofa = has("totp").then(|| {
        let set_up = move |_| {
            dom::spawn(async move {
                match api::post_empty("/auth/2fa/setup").await {
                    Ok(s) => { page.twofa.try_set(TwoFa::Setup(s.s("qr"), s.s("secret"))); }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        };
        let enable = move |e: web_sys::SubmitEvent| {
            e.prevent_default();
            let code = field(&e, "code");
            dom::spawn(async move {
                match api::post("/auth/2fa/enable", json!({ "code": code })).await {
                    Ok(_) => { toast("Two-step sign-in is on", "ok"); redraw(); }
                    Err(err) => toast(&err.message, "err"),
                }
            });
        };
        let disable = move |e: web_sys::SubmitEvent| {
            e.prevent_default();
            let password = field(&e, "password");
            dom::spawn(async move {
                match api::post("/auth/2fa/disable", json!({ "password": password })).await {
                    Ok(_) => { toast("Two-step sign-in is off", "ok"); redraw(); }
                    Err(err) => toast(&err.message, "err"),
                }
            });
        };
        let body = move || match page.twofa.get() {
            TwoFa::Start if totp_on => view! { <button class="btn danger" id="twofa-off" style="width:fit-content" on:click=move |_| page.twofa.set(TwoFa::Off)>"Turn off"</button> }.into_any(),
            TwoFa::Start => view! { <button class="btn primary" id="twofa-on" style="width:fit-content" on:click=set_up>"Set up"</button> }.into_any(),
            TwoFa::Setup(qr, secret) => {
                let qr = (!qr.is_empty()).then(|| view! { <img src=format!("data:image/png;base64,{qr}") alt="QR code" style="width:180px;height:180px;border-radius:12px;background:#fff;padding:8px"/> });
                view! {
                    <div class="row wrap" style="gap:18px;align-items:flex-start">
                        {qr}
                        <div class="stack" style="flex:1;min-width:220px"><span class="small muted">"Scan the code with your authenticator app, or enter this key:"</span>
                            <span class="code mono" style="word-break:break-all">{secret}</span>
                            <form class="row" id="twofa-confirm" on:submit=enable><input class="input" name="code" inputmode="numeric" autocomplete="one-time-code" placeholder="6-digit code" maxlength="8" required style="max-width:160px"/>
                                <button class="btn primary" type="submit">"Turn on"</button></form></div></div>
                }.into_any()
            }
            TwoFa::Off => view! {
                <form class="row wrap" id="twofa-off-form" on:submit=disable><input class="input" type="password" name="password" placeholder="Your password" autocomplete="current-password" required style="max-width:240px"/>
                    <button class="btn danger" type="submit">"Turn off two-step sign-in"</button></form>
            }.into_any(),
        };
        view! {
            <div class="card stack mt" style="max-width:720px" id="twofa"><h3>"Two-step sign-in"</h3>
                <p class="small muted" style="margin:0">{if totp_on { "✅ On – signing in asks for a code from your authenticator app." } else { "Ask for a 6-digit code from an authenticator app (Google Authenticator, 1Password, Aegis…) when signing in." }}</p>
                <div id="twofa-body">{body}</div></div>
        }
    });
    let logout_all = has("logout_all").then(|| {
        let go = |_| dom::spawn(async {
            if !confirm_dialog("Sign out everywhere?", "Every phone, computer and app signed in to your account is signed out – including this one.", "Sign out everywhere", true).await { return; }
            if api::post_empty("/auth/logout-all").await.is_ok() { dom::reload(); }
        });
        view! { <button class="btn sm danger" id="logout-all" on:click={go} {..html(format!("{}Sign out everywhere", icon("logout")))}>{""}</button> }
    });
    let sessions = move || page.sessions.get().map(|rows| view! { <div class="list">{rows.iter().map(|s| session_row(page, s)).collect_view()}</div> });
    view! {
        <h1 style="margin-bottom:18px">"Account"</h1>
        <div class="stack" style="max-width:720px;gap:16px">
            <div class="card stack">
                <div class="row">{raw(app::avatar_html(&me, 52))}
                    <div class="grow"><div class="row" style="gap:6px;align-items:center"><h2>{username}</h2>{name_edit}</div><div class="muted small">{format!("{role} · joined {}", fmt::ago(me.f("created_at")))}</div></div></div>
                {rename}
                {avatar_block}
                <div class="small muted">"Requests this week: "<b>{me.s("requests_this_week")}</b>{limit}" "{auto}</div>
            </div>
            <div class="card stack" id="pw-card">
                <div class="row" style="align-items:center"><h3 class="grow">"Password"</h3>
                    <button class="btn sm" type="button" id="pw-toggle" on:click=move |_| show_pw(!page.pw_open.get_untracked())>{move || if page.pw_open.get() { "Cancel" } else { "Change password" }}</button></div>
                <form class="stack" id="pw" hidden=move || !page.pw_open.get() node_ref=pw_form on:submit=change_pw>
                    <label class="field"><span>"Current password"</span><input class="input" type="password" name="current" autocomplete="current-password" required/></label>
                    <label class="field"><span>"New password"</span><input class="input" type="password" name="new" autocomplete="new-password" minlength="8" required/></label>
                    <button class="btn primary" type="submit">"Update password"</button>
                </form>
            </div>
        </div>
        <form class="card stack mt" id="email-form" style="max-width:720px" on:submit=save_email>
            <h3>"Email"</h3>
            <p class="small muted" style="margin:0">"Used only to send you a link if you forget your password."</p>
            <div class="row wrap" style="gap:10px"><input class="input grow" type="email" name="email" value=me.s("email") placeholder="you@example.com" autocomplete="email" style="min-width:220px"/>
                <button class="btn primary" type="submit">"Save"</button></div>
        </form>
        <div class="card stack mt" style="max-width:720px"><h3>"Apps"</h3>
            <p class="small muted" style="margin:0">"Android app and desktop apps for Windows, macOS and Linux: "<a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">"download from the Releases page"</a>". On a phone you can also use your browser's "<b>"Add to Home screen"</b>"."</p>
            <p class="small muted" id="addr-line" style="margin:0" hidden=move || page.address.with(Option::is_none) inner_html=move || page.address.get().unwrap_or_default()></p>
        </div>
        {twofa}
        <div class="section-head"><h2>"Active sessions"</h2>{logout_all}</div>
        <div class="card flat" id="sessions">{sessions}</div>
        {raw(about)}
    }
}

#[cfg(test)]
mod address_tests {
    use super::address_line;
    use serde_json::json;

    #[test]
    fn says_which_address_and_links_the_local_one() {
        let v = json!({ "public": "https://encore.example.uk", "local": "http://10.0.1.7:8080" });
        assert_eq!(address_line("http://10.0.1.7:8080", &v), "This browser uses the server's <b>local network address</b> (http://10.0.1.7:8080).");
        let away = address_line("https://encore.example.uk", &v);
        assert!(away.starts_with("This browser uses the server's <b>public address</b>. On your home network <a href=\"http://10.0.1.7:8080\""), "{away}");
        assert!(address_line("http://localhost:8080", &v).starts_with("This browser uses this address."));
        assert_eq!(address_line("https://encore.example.uk", &json!({ "public": "https://encore.example.uk", "local": "" })), "");
    }
}
