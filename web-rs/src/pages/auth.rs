//! Port of web/js/pages/auth.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Sign-in, sign-up (with invite), forgot password, the reset page, the magic-link page and "Change server".

use crate::api::{self, ApiError};
use crate::app;
use crate::appearance::logo;
use crate::dom;
use crate::json::J;
use crate::ui::esc;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Element;

/// `Object.fromEntries(new FormData(form))`.
fn form_entries(form: &Element) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(f) = form.dyn_ref::<web_sys::HtmlFormElement>() else { return out };
    let Ok(fd) = web_sys::FormData::new_with_form(f) else { return out };
    for e in fd.entries().into_iter().flatten() {
        if let Ok(pair) = e.dyn_into::<js_sys::Array>() {
            out.insert(pair.get(0).as_string().unwrap_or_default(), Value::String(pair.get(1).as_string().unwrap_or_default()));
        }
    }
    out
}

/// `throw new Error(msg)` – an error without API data, so `ex.data?.two_factor` is falsy.
fn error(msg: &str) -> ApiError { ApiError { status: 0, message: msg.to_string(), data: Value::Null } }

/// `session.info?.<key> || fallback`.
fn info_or(info: &Value, key: &str, fallback: &str) -> String {
    let v = info.s(key);
    if v.is_empty() { fallback.to_string() } else { v }
}

fn show_error(err: &Element, msg: &str) {
    dom::set_text(err, msg);
    dom::remove_class(err, "hidden");
}

fn reset_page(root: &Element, query: &HashMap<String, String>, app: &str) {
    dom::set_html(root, &format!(r#"<div class="auth-wrap"><div class="auth-card">
      <div class="brand">{}{app}</div>
      <p class="muted" style="text-align:center;margin:0">Choose a new password</p>
      <form>
        <label class="field"><span>New password</span><input class="input" type="password" name="password" autocomplete="new-password" minlength="8" required autofocus></label>
        <label class="field"><span>Repeat it</span><input class="input" type="password" name="password2" autocomplete="new-password" required></label>
        <div class="error-box hidden" id="auth-err"></div>
        <button class="btn primary" type="submit" style="padding:12px">Save new password</button>
      </form></div></div>"#, logo(30)));
    let form = dom::qs_in(root, "form").expect("form");
    let err = dom::qs_in(root, "#auth-err").expect("#auth-err");
    let token = query.get("token").cloned().unwrap_or_default();
    let f = form.clone();
    dom::on(&form, "submit", move |e| {
        dom::prevent(e);
        let d = form_entries(&f);
        dom::add_class(&err, "hidden");
        let (form, err, token) = (f.clone(), err.clone(), token.clone());
        dom::spawn(async move {
            let res = async {
                if d.get("password") != d.get("password2") { return Err(error("Passwords do not match")); }
                let mut body = Map::new();
                body.insert("token".into(), Value::String(token));
                if let Some(p) = d.get("password") { body.insert("password".into(), p.clone()); }
                api::post("/auth/reset", Value::Object(body)).await
            }.await;
            match res {
                Ok(_) => form.set_outer_html(r##"<div class="test-result ok"><span>Password changed – sign in with your new password.</span></div><a class="btn primary" href="#/login" style="justify-content:center">Sign in</a>"##),
                Err(ex) => show_error(&err, &ex.message),
            }
        });
    });
}

fn forgot_form(root: &Element) {
    let Some(card) = dom::qs_in(root, ".auth-card form") else { return };
    card.set_outer_html(r##"<form id="forgot">
    <p class="small muted" style="margin:0">Enter your username or email. If your account has an email address, we'll send a reset link.</p>
    <label class="field"><span>Username or email</span><input class="input" name="login" required autofocus></label>
    <div class="error-box hidden" id="forgot-err"></div>
    <button class="btn primary" type="submit" style="padding:12px">Send reset link</button>
    <a href="#/login" class="small muted" style="text-align:center" onclick="location.reload()">Back to sign in</a></form>"##);
    let (Some(f), Some(err)) = (dom::qs_in(root, "#forgot"), dom::qs_in(root, "#forgot-err")) else { return };
    let f2 = f.clone();
    dom::on(&f, "submit", move |e| {
        dom::prevent(e);
        dom::add_class(&err, "hidden");
        let body = Value::Object(form_entries(&f2));
        let (f, err) = (f2.clone(), err.clone());
        dom::spawn(async move {
            match api::post("/auth/forgot", body).await {
                Ok(r) => dom::set_html(&f, &format!(r#"<div class="test-result ok"><span>{}</span></div>"#, esc(&r.s("message")))),
                Err(ex) => show_error(&err, &ex.message),
            }
        });
    });
}

/// Sign-in link from an email. Accounts with two-step sign-in also enter their code.
fn magic_page(root: &Element, query: &HashMap<String, String>, app: &str) {
    dom::set_html(root, &format!(r#"<div class="auth-wrap"><div class="auth-card">
      <div class="brand">{}{app}</div>
      <p class="muted" style="text-align:center;margin:0" id="magic-msg">Signing you in…</p>
      <form class="hidden" id="magic-code"><label class="field"><span>Code from your authenticator app</span>
        <input class="input" name="code" inputmode="numeric" autocomplete="one-time-code" maxlength="8" required></label>
        <button class="btn primary" type="submit" style="padding:12px">Sign in</button></form>
      <div class="error-box hidden" id="auth-err"></div></div></div>"#, logo(30)));
    let err = dom::qs_in(root, "#auth-err").expect("#auth-err");
    let form = dom::qs_in(root, "#magic-code").expect("#magic-code");
    let token = query.get("token").cloned().unwrap_or_default();
    let go: Rc<dyn Fn(Option<String>)> = {
        let (root, err, form) = (root.clone(), err.clone(), form.clone());
        Rc::new(move |code: Option<String>| {
            dom::add_class(&err, "hidden");
            let (root, err, form, token) = (root.clone(), err.clone(), form.clone(), token.clone());
            dom::spawn(async move {
                // `{ token, code }` – JSON.stringify leaves `code` out when it is undefined
                let mut body = Map::new();
                body.insert("token".into(), Value::String(token));
                if let Some(c) = &code { body.insert("code".into(), Value::String(c.clone())); }
                match api::post("/auth/magic", Value::Object(body)).await {
                    Ok(_) => { dom::set_hash("#/"); dom::reload(); }
                    Err(ex) => {
                        if ex.data.truthy("two_factor") {
                            dom::remove_class(&form, "hidden");
                            if let Some(m) = dom::qs_in(&root, "#magic-msg") { dom::set_text(&m, "One more step"); }
                            if matches!(code.as_deref(), Some(c) if !c.is_empty()) { show_error(&err, &ex.message); }
                            if let Some(c) = dom::qs_in(&form, "[name=code]") { dom::focus(&c); }
                            return;
                        }
                        if let Some(m) = dom::qs_in(&root, "#magic-msg") { dom::set_text(&m, ""); }
                        dom::set_html(&err, &format!(r##"{} – <a href="#/login" onclick="location.reload()">sign in another way</a>"##, esc(&ex.message)));
                        dom::remove_class(&err, "hidden");
                    }
                }
            });
        })
    };
    let (g, f) = (go.clone(), form.clone());
    dom::on(&form, "submit", move |e| { dom::prevent(e); g(Some(dom::value_of(&f, "[name=code]"))); });
    go(None);
}

fn email_link_form(root: &Element) {
    let Some(card) = dom::qs_in(root, ".auth-card form") else { return };
    card.set_outer_html(r##"<form id="email-link">
    <p class="small muted" style="margin:0">We'll email you a link that signs you in – no password needed.</p>
    <label class="field"><span>Username or email</span><input class="input" name="login" required autofocus></label>
    <div class="error-box hidden" id="link-err"></div>
    <button class="btn primary" type="submit" style="padding:12px">Email me a sign-in link</button>
    <a href="#/login" class="small muted" style="text-align:center" onclick="location.reload()">Back to sign in</a></form>"##);
    let (Some(f), Some(err)) = (dom::qs_in(root, "#email-link"), dom::qs_in(root, "#link-err")) else { return };
    let f2 = f.clone();
    dom::on(&f, "submit", move |e| {
        dom::prevent(e);
        dom::add_class(&err, "hidden");
        let body = Value::Object(form_entries(&f2));
        let (f, err) = (f2.clone(), err.clone());
        dom::spawn(async move {
            match api::post("/auth/email-link", body).await {
                Ok(r) => dom::set_html(&f, &format!(r#"<div class="test-result ok"><span>{}</span></div>"#, esc(&r.s("message")))),
                Err(ex) => show_error(&err, &ex.message),
            }
        });
    });
}

pub async fn render(root: Element, path: String, query: HashMap<String, String>) {
    let info = app::info();
    let app_name = esc(&info_or(&info, "app_name", "Encore"));
    if path == "/reset" { return reset_page(&root, &query, &app_name); }
    if path == "/magic" { return magic_page(&root, &query, &app_name); }
    let mode = info_or(&info, "signup_mode", "invite");
    let invite = query.get("invite").cloned().unwrap_or_default();
    let is_signup = path == "/signup" || (!invite.is_empty() && path != "/login");
    let app = app_name;
    let foot = if is_signup {
        r##"Already have an account? <a href="#/login" style="color:var(--accent)">Sign in</a>"##.to_string()
    } else if mode == "closed" {
        "Sign-ups are closed. Ask the admin for an account.".to_string()
    } else {
        format!(r##"New here? <a href="#/signup" style="color:var(--accent)">{}</a>"##, if mode == "invite" { "Sign up with an invite" } else { "Create an account" })
    };
    dom::set_html(&root, &format!(r##"
    <div class="auth-wrap">
      <div class="auth-card">
        <div class="brand">{logo}{app}</div>
        <p class="muted" style="text-align:center;margin:0">{title}</p>
        <div id="auth-err" class="error-box hidden" style="margin-top:16px"></div>
        {form}
        <p class="small muted" style="text-align:center;margin:18px 0 0">
          {foot}
        </p>
        {change_server}
      </div>
    </div>"##,
        logo = logo(30),
        title = if is_signup { "Create your account" } else { "Sign in to request music and manage the library" },
        form = if is_signup { signup_form(&mode, &invite) } else { login_form(info.truthy("email_login")) },
        change_server = if app::native_has("changeServer") { r##"<p class="small muted" style="text-align:center;margin:10px 0 0"><a href="#" id="change-server" style="color:var(--accent)">Change server</a></p>"## } else { "" },
    ));
    let r = root.clone();
    dom::on_sel(&root, "#forgot-link", "click", move |e| { dom::prevent(e); forgot_form(&r); });
    let r = root.clone();
    dom::on_sel(&root, "#email-link-btn", "click", move |e| { dom::prevent(e); email_link_form(&r); });
    dom::on_sel(&root, "#change-server", "click", |e| { dom::prevent(e); app::native_call("changeServer"); });
    let Some(form) = dom::qs_in(&root, "form") else { return };
    let err = dom::qs_in(&root, "#auth-err").expect("#auth-err");
    let f = form.clone();
    dom::on(&form, "submit", move |e| {
        dom::prevent(e);
        let data = form_entries(&f);
        let Some(btn) = dom::qs_in(&f, "button[type=submit]") else { return };
        dom::set_disabled(&btn, true);
        dom::add_class(&err, "hidden");
        let (form, err) = (f.clone(), err.clone());
        dom::spawn(async move {
            let res = async {
                let mut data = data;
                if is_signup {
                    if data.get("password") != data.get("password2") { return Err(error("Passwords do not match")); }
                    data.remove("password2");
                    api::post("/auth/signup", Value::Object(data)).await
                } else {
                    api::post("/auth/login", Value::Object(data)).await
                }
            }.await;
            match res {
                Ok(_) => { dom::set_hash("#/"); dom::reload(); }
                Err(ex) => {
                    dom::set_disabled(&btn, false);
                    if ex.data.truthy("two_factor") && dom::qs_in(&form, "[name=code]").is_none() {
                        // two-step sign-in: ask for the authenticator code, keep what was typed
                        let _ = btn.insert_adjacent_html("beforebegin", r#"<label class="field"><span>Code from your authenticator app</span>
          <input class="input" name="code" inputmode="numeric" autocomplete="one-time-code" maxlength="8" required></label>"#);
                        if let Some(c) = dom::qs_in(&form, "[name=code]") { dom::focus(&c); }
                        return;
                    }
                    show_error(&err, &ex.message);
                }
            }
        });
    });
}

fn login_form(email_login: bool) -> String {
    format!(r##"<form autocomplete="on">
    <label class="field"><span>Username</span><input class="input" name="username" autocomplete="username" required autofocus></label>
    <label class="field"><span>Password</span><input class="input" type="password" name="password" autocomplete="current-password" required></label>
    <button class="btn primary" type="submit" style="padding:12px">Sign in</button>
    {}
    <a href="#" id="forgot-link" class="small muted" style="text-align:center">Forgot password?</a>
  </form>"##, if email_login { r#"<button type="button" class="btn" id="email-link-btn" style="padding:12px">Email me a sign-in link</button>"# } else { "" })
}

fn signup_form(mode: &str, invite: &str) -> String {
    if mode == "closed" { return r#"<div class="error-box" style="margin-top:18px">Sign-ups are currently closed.</div>"#.to_string(); }
    format!(r#"<form autocomplete="on">
    {}
    <label class="field"><span>Username</span><input class="input" name="username" autocomplete="username" minlength="3" maxlength="32" required></label>
    <label class="field"><span>Email <small>(optional)</small></span><input class="input" type="email" name="email" autocomplete="email"></label>
    <label class="field"><span>Password</span><input class="input" type="password" name="password" autocomplete="new-password" minlength="8" required></label>
    <label class="field"><span>Confirm password</span><input class="input" type="password" name="password2" autocomplete="new-password" minlength="8" required></label>
    <button class="btn primary" type="submit" style="padding:12px">Create account</button>
  </form>"#, if mode == "invite" || !invite.is_empty() {
        format!(r#"<label class="field"><span>Invite code</span><input class="input mono" name="invite" value="{}" placeholder="XXXX-XXXX-XXXX" required></label>"#, esc(invite))
    } else { String::new() })
}
