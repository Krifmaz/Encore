//! Port of web/js/pages/account.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Account: profile + picture, password, username, email, apps, two-step sign-in, active sessions and
//! (for members) the About card.

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, json, truthy, J};
use super::general;
use crate::ui::{confirm_dialog, esc, fmt, icon, toast};
use serde_json::Value;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, FormData, HtmlFormElement, HtmlInputElement};

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

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let me = api::get("/auth/me").await.map_err(|e| e.message)?;
        let has = |f: &str| app::has_feature(f);
        let admin = matches!(me.s("role").as_str(), "admin" | "owner");
        let avatar_block = if has("avatar") {
            // `icon.upload || icon.plus`
            let up = { let u = icon("upload"); if u.is_empty() { icon("plus") } else { u } };
            format!(r##"<div class="row wrap" style="gap:8px" data-dropzone><label class="btn sm">{up}Change picture<input type="file" id="avatar-file" accept="image/*" hidden></label>
          {}</div>"##, if me.truthy("avatar_at") { r#"<button class="btn sm ghost" id="avatar-remove">Remove</button>"# } else { "" })
        } else { String::new() };
        let rename = if has("rename") {
            format!(r##"<form class="stack" id="name-form" hidden>
          <div class="row wrap" style="gap:10px"><input class="input grow" name="username" value="{}" autocomplete="username" minlength="3" maxlength="32" required style="min-width:220px">
            <button class="btn primary" type="submit">Save</button><button class="btn ghost" type="button" id="name-cancel">Cancel</button></div>
          <p class="small muted" style="margin:0">What you sign in with and what others see. 3–32 letters, numbers, . _ -</p>
          {}
        </form>"##, esc(&me.s("username")),
                if me.truthy("email") { String::new() } else { r#"<p class="small" id="name-warn" style="margin:0;color:var(--warn)">⚠ There's no email on this account, so your username is the only way you'll be able to sign in – make it one you'll remember. Add an email below to be able to reset your password.</p>"#.to_string() })
        } else { String::new() };
        let name_edit = if has("rename") { format!(r#"<button class="btn ghost icon sm" type="button" id="name-edit" title="Change username" aria-label="Change username">{}</button>"#, icon("edit")) } else { String::new() };
        let twofa = if has("totp") {
            format!(r##"<div class="card stack mt" style="max-width:720px" id="twofa"><h3>Two-step sign-in</h3>
      <p class="small muted" style="margin:0">{}</p>
      <div id="twofa-body">{}</div></div>"##,
                if me.truthy("totp_enabled") { "✅ On – signing in asks for a code from your authenticator app." } else { "Ask for a 6-digit code from an authenticator app (Google Authenticator, 1Password, Aegis…) when signing in." },
                if me.truthy("totp_enabled") { r#"<button class="btn danger" id="twofa-off" style="width:fit-content">Turn off</button>"# } else { r#"<button class="btn primary" id="twofa-on" style="width:fit-content">Set up</button>"# })
        } else { String::new() };
        let logout_all = if has("logout_all") { format!(r#"<button class="btn sm danger" id="logout-all">{}Sign out everywhere</button>"#, icon("logout")) } else { String::new() };
        let about = if admin { String::new() } else { general::about_card().await };
        dom::set_html(&view, &format!(r##"
    <h1 style="margin-bottom:18px">Account</h1>
    <div class="stack" style="max-width:720px;gap:16px">
      <div class="card stack">
        <div class="row">{avatar}
          <div class="grow"><div class="row" style="gap:6px;align-items:center"><h2>{username}</h2>{name_edit}</div><div class="muted small">{role} · joined {joined}</div></div></div>
        {rename}
        {avatar_block}
        <div class="small muted">Requests this week: <b>{requests}</b>{limit}
          {auto}</div>
      </div>
      <div class="card stack" id="pw-card">
        <div class="row" style="align-items:center"><h3 class="grow">Password</h3><button class="btn sm" type="button" id="pw-toggle">Change password</button></div>
        <form class="stack" id="pw" hidden>
          <label class="field"><span>Current password</span><input class="input" type="password" name="current" autocomplete="current-password" required></label>
          <label class="field"><span>New password</span><input class="input" type="password" name="new" autocomplete="new-password" minlength="8" required></label>
          <button class="btn primary" type="submit">Update password</button>
        </form>
      </div>
    </div>
    <form class="card stack mt" id="email-form" style="max-width:720px">
      <h3>Email</h3>
      <p class="small muted" style="margin:0">Used only to send you a link if you forget your password.</p>
      <div class="row wrap" style="gap:10px"><input class="input grow" type="email" name="email" value="{email}" placeholder="you@example.com" autocomplete="email" style="min-width:220px">
        <button class="btn primary" type="submit">Save</button></div>
    </form>
    <div class="card stack mt" style="max-width:720px"><h3>Apps</h3>
      <p class="small muted" style="margin:0">Android app and desktop apps for Windows, macOS and Linux: <a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">download from the Releases page</a>. On a phone you can also use your browser's <b>Add to Home screen</b>.</p>
      <p class="small muted" id="addr-line" style="margin:0" hidden></p>
      </div>
    {twofa}
    <div class="section-head"><h2>Active sessions</h2>{logout_all}</div>
    <div class="card flat" id="sessions"></div>
    {about}"##,
            avatar = app::avatar_html(&me, 52),
            username = esc(&me.s("username")),
            role = if me.s("role") == "owner" { "Owner" } else if admin { "Administrator" } else { "Member" },
            joined = fmt::ago(me.f("created_at")),
            requests = me.s("requests_this_week"),
            limit = if me.truthy("request_limit") && !admin { format!(" of {}", me.s("request_limit")) } else { String::new() },
            auto = if me.truthy("auto_approve") || admin { r#" · <span class="badge ok">auto-approved</span>"# } else { "" },
            email = esc(&me.s("email")),
        ));

        // which of the server's addresses this browser uses (the apps switch on their own; a browser can't
        // carry its sign-in to another address, so it only says so and links the other one)
        if has("addresses") {
            if let Some(line) = dom::qs_in(&view, "#addr-line") {
                dom::spawn(async move {
                    let Ok(v) = api::get("/auth/addresses").await else { return };
                    let html = address_line(&dom::location().origin().unwrap_or_default(), &v);
                    if html.is_empty() { return }
                    dom::set_html(&line, &html);
                    if let Ok(e) = line.dyn_into::<web_sys::HtmlElement>() { e.set_hidden(false); }
                });
            }
        }
        let Some(bx) = dom::qs_in(&view, "#sessions") else { return Ok(()) };
        let load_sessions: Rc<dyn Fn()> = {
            let bx = bx.clone();
            Rc::new(move || {
                let bx = bx.clone();
                dom::spawn(async move {
                    let Ok(rows) = api::get("/auth/sessions").await else { return };
                    dom::set_html(&bx, &format!(r##"<div class="list">{}</div>"##, rows.items().iter().map(|s| format!(r##"<div class="list-item" data-sid="{}">
      <div class="grow"><div class="ellipsis" style="font-weight:600">{}</div>
      <div class="small muted">{} · signed in {}</div></div>
      {}</div>"##,
                        esc(&s.s("id")),
                        esc(&session_title(s)),
                        esc(&s.s("ip")),
                        fmt::ago(s.f("created_at")),
                        if s.truthy("current") { r#"<span class="badge ok">This device</span>"#.to_string() } else { format!(r#"<button class="btn sm danger">{}Revoke</button>"#, icon("logout")) },
                    )).collect::<String>()));
                });
            })
        };

        // username: the pencil opens the editor (with a warning when there's no email to fall back on)
        {
            let v = view.clone();
            dom::on_sel(&view, "#name-edit", "click", move |_| {
                let Some(f) = dom::qs_in(&v, "#name-form").and_then(|f| f.dyn_into::<web_sys::HtmlElement>().ok()) else { return };
                f.set_hidden(!f.hidden());
                if !f.hidden() { if let Some(i) = dom::qs_in(&v, "#name-form [name=username]") { dom::focus(&i); } }
            });
            let v = view.clone();
            let name = me.s("username");
            dom::on_sel(&view, "#name-cancel", "click", move |_| {
                if let Some(f) = dom::qs_in(&v, "#name-form").and_then(|f| f.dyn_into::<web_sys::HtmlElement>().ok()) { f.set_hidden(true); }
                if let Some(i) = dom::qs_in(&v, "#name-form [name=username]") { dom::set_value(&i, &name); }
            });
        }
        // password: a button that opens the form
        let pw_show: Rc<dyn Fn(bool)> = {
            let v = view.clone();
            Rc::new(move |on: bool| {
                let Some(f) = dom::qs_in(&v, "#pw").and_then(|f| f.dyn_into::<HtmlFormElement>().ok()) else { return };
                f.set_hidden(!on);
                if let Some(t) = dom::qs_in(&v, "#pw-toggle") { dom::set_text(&t, if on { "Cancel" } else { "Change password" }); }
                if on { if let Some(i) = dom::qs_in(&v, "#pw [name=current]") { dom::focus(&i); } } else { f.reset(); }
            })
        };
        {
            let v = view.clone();
            let show = pw_show.clone();
            dom::on_sel(&view, "#pw-toggle", "click", move |_| {
                let hidden = dom::qs_in(&v, "#pw").and_then(|f| f.dyn_into::<web_sys::HtmlElement>().ok()).map(|f| f.hidden()).unwrap_or(true);
                show(hidden);
            });
        }
        {
            let ls = load_sessions.clone();
            let show = pw_show.clone();
            dom::on_sel(&view, "#pw", "submit", move |e| {
                dom::prevent(e);
                let Some(form) = dom::target_el(e) else { return };
                let ls = ls.clone();
                let show = show.clone();
                dom::spawn(async move {
                    // Object.fromEntries(new FormData(form)): the two named inputs
                    let body = json!({ "current": dom::value_of(&form, "[name=current]"), "new": dom::value_of(&form, "[name=new]") });
                    match api::post("/auth/password", body).await {
                        Ok(_) => {
                            toast("Password updated – other sessions signed out", "ok");
                            show(false);
                            ls();
                        }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
        dom::on_sel(&view, "#name-form", "submit", |e| {
            dom::prevent(e);
            let Some(form) = dom::target_el(e) else { return };
            dom::spawn(async move {
                match api::put("/auth/profile", json!({ "username": dom::value_of(&form, "[name=username]").trim() })).await {
                    Ok(r) => {
                        toast(&format!("You're now {}", r.s("username")), "ok");
                        app::refresh_user().await;
                    }
                    Err(err) => toast(&err.message, "err"),
                }
            });
        });
        dom::on_sel(&view, "#email-form", "submit", |e| {
            dom::prevent(e);
            let Some(form) = dom::target_el(e) else { return };
            dom::spawn(async move {
                match api::put("/auth/profile", json!({ "email": dom::value_of(&form, "[name=email]") })).await {
                    Ok(_) => toast("Email saved", "ok"),
                    Err(err) => toast(&err.message, "err"),
                }
            });
        });
        // profile picture
        dom::on_sel(&view, "#avatar-file", "change", |e| {
            let Some(f) = dom::target_el(e).and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).and_then(|i| i.files()).and_then(|l| l.get(0)) else { return };
            dom::spawn(async move {
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
                    // (await r.json().catch(() => ({}))).detail || 'Upload failed'
                    let data: Value = r.text().await.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
                    let detail = data.g("detail");
                    toast(&if truthy(detail) { js_string(detail) } else { "Upload failed".to_string() }, "err");
                    return;
                }
                toast("Picture updated", "ok");
                dom::spawn(app::refresh_user());
            });
        });
        dom::on_sel(&view, "#avatar-remove", "click", |_| dom::spawn(async {
            if api::del("/auth/avatar").await.is_ok() { dom::spawn(app::refresh_user()); }
        }));
        // two-step sign-in
        {
            let (v, c) = (view.clone(), ctx.clone());
            dom::on_sel(&view, "#twofa-on", "click", move |_| {
                let (v, c) = (v.clone(), c.clone());
                dom::spawn(async move {
                    let s = match api::post_empty("/auth/2fa/setup").await {
                        Ok(s) => s,
                        Err(e) => { toast(&e.message, "err"); return; }
                    };
                    let Some(body) = dom::qs_in(&v, "#twofa-body") else { return };
                    dom::set_html(&body, &format!(r##"<div class="row wrap" style="gap:18px;align-items:flex-start">
        {}
        <div class="stack" style="flex:1;min-width:220px"><span class="small muted">Scan the code with your authenticator app, or enter this key:</span>
          <span class="code mono" style="word-break:break-all">{}</span>
          <form class="row" id="twofa-confirm"><input class="input" name="code" inputmode="numeric" autocomplete="one-time-code" placeholder="6-digit code" maxlength="8" required style="max-width:160px">
            <button class="btn primary" type="submit">Turn on</button></form></div></div>"##,
                        if s.truthy("qr") { format!(r#"<img src="data:image/png;base64,{}" alt="QR code" style="width:180px;height:180px;border-radius:12px;background:#fff;padding:8px">"#, s.s("qr")) } else { String::new() },
                        esc(&s.s("secret"))));
                    let (v2, c2) = (v.clone(), c.clone());
                    dom::on_sel(&v, "#twofa-confirm", "submit", move |e| {
                        dom::prevent(e);
                        let Some(form) = dom::target_el(e) else { return };
                        let (v, c) = (v2.clone(), c2.clone());
                        dom::spawn(async move {
                            match api::post("/auth/2fa/enable", json!({ "code": dom::value_of(&form, "[name=code]") })).await {
                                Ok(_) => { toast("Two-step sign-in is on", "ok"); rerender(v, c); }
                                Err(err) => toast(&err.message, "err"),
                            }
                        });
                    });
                });
            });
        }
        {
            let (v, c) = (view.clone(), ctx.clone());
            dom::on_sel(&view, "#twofa-off", "click", move |_| {
                let Some(body) = dom::qs_in(&v, "#twofa-body") else { return };
                dom::set_html(&body, r##"<form class="row wrap" id="twofa-off-form"><input class="input" type="password" name="password" placeholder="Your password" autocomplete="current-password" required style="max-width:240px">
      <button class="btn danger" type="submit">Turn off two-step sign-in</button></form>"##);
                let (v2, c2) = (v.clone(), c.clone());
                dom::on_sel(&v, "#twofa-off-form", "submit", move |e| {
                    dom::prevent(e);
                    let Some(form) = dom::target_el(e) else { return };
                    let (v, c) = (v2.clone(), c2.clone());
                    dom::spawn(async move {
                        match api::post("/auth/2fa/disable", json!({ "password": dom::value_of(&form, "[name=password]") })).await {
                            Ok(_) => { toast("Two-step sign-in is off", "ok"); rerender(v, c); }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                });
            });
        }
        dom::on_sel(&view, "#logout-all", "click", |_| dom::spawn(async {
            if !confirm_dialog("Sign out everywhere?", "Every phone, computer and app signed in to your account is signed out – including this one.", "Sign out everywhere", true).await { return; }
            if api::post_empty("/auth/logout-all").await.is_ok() { dom::reload(); }
        }));
        {
            let ls = load_sessions.clone();
            dom::on(&bx, "click", move |e| {
                let Some(b) = dom::closest(e, "button") else { return };
                let Some(row) = dom::closest_of(&b, "[data-sid]") else { return };
                let sid = dom::data(&row, "sid");
                let ls = ls.clone();
                dom::spawn(async move {
                    if api::del(&format!("/auth/sessions/{sid}")).await.is_ok() { ls(); }
                });
            });
        }
        load_sessions();
        Ok(())
    })
}

/// The JS `render(view, ctx)` called again from a handler (not awaited; a failure only reaches the console).
fn rerender(view: Element, ctx: Ctx) {
    dom::spawn(async move { if let Err(e) = render(view, ctx).await { dom::console_error(&e); } });
}
