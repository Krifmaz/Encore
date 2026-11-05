//! Settings → Users & permissions: the users table, permissions for everyone, what new members start with.
//!
//! A Leptos view ([`Users`]) over three signals (the users, the permissions for everyone, the defaults for new
//! members), each loaded again after a change. The Permissions, Reset password and Add user dialogs are still
//! drawn as HTML.
use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{confirm_dialog, esc, fmt, icon, modal, toast, ModalOpts};
use super::schema_form::{env_locked, env_note};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::{json, Map, Value};
use wasm_bindgen::JsCast;
use web_sys::Element;

/// `view.querySelector(sel)` that has to exist (the JS would throw on `null.innerHTML`).
pub(super) fn must(root: &Element, sel: &str) -> Result<Element, String> { dom::qs_in(root, sel).ok_or_else(|| format!("{sel} not found")) }

/// A JS number as JSON (`12` stays `12`, `2.5` stays `2.5`).
fn js_num(f: f64) -> Value { if f.fract() == 0.0 && f.abs() < 1e15 { Value::from(f as i64) } else { Value::from(f) } }

/// `+s` as JSON.stringify sends it: a number, `0` for "", `null` for NaN.
pub(super) fn js_plus(s: &str) -> Value {
    let t = s.trim();
    if t.is_empty() { return Value::from(0); }
    match t.parse::<f64>() { Ok(f) if f.is_finite() => js_num(f), _ => Value::Null }
}

/// `parseInt(s, 10) || 0`.
fn parse_int(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, rest) = match t.strip_prefix('-') { Some(r) => (true, r), None => (false, t.strip_prefix('+').unwrap_or(t)) };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n = digits.parse::<i64>().unwrap_or(0);
    if neg { -n } else { n }
}

/// `{ [k]: v }`.
fn obj(k: &str, v: Value) -> Value { let mut m = Map::new(); m.insert(k.to_string(), v); Value::Object(m) }

/// `e.target.type` of an input ("" for anything else).
fn input_type(el: &Element) -> String { el.dyn_ref::<web_sys::HtmlInputElement>().map(|i| i.type_()).unwrap_or_default() }

/// `form.reportValidity()`.
fn report_validity(form: &Element) -> bool { form.dyn_ref::<web_sys::HtmlFormElement>().map(|f| f.report_validity()).unwrap_or(false) }

/// `Object.fromEntries(new FormData(form))`.
pub(super) fn form_entries(form: &Element) -> Value {
    let Some(f) = form.dyn_ref::<web_sys::HtmlFormElement>() else { return json!({}) };
    let Ok(fd) = web_sys::FormData::new_with_form(f) else { return json!({}) };
    let Ok(o) = js_sys::Object::from_entries(&fd) else { return json!({}) };
    serde_wasm_bindgen::from_value(o.into()).unwrap_or_else(|_| json!({}))
}

/// The default permissions of new members: (setting, label).
const NEW: [(&str, &str); 6] = [
    ("new_user_auto_approve", "Requests skip the approval queue"), ("new_user_auto_approve_playlists", "Playlist downloads skip the approval queue"),
    ("new_user_pick", "Pick the exact version to download"), ("new_user_fix", "Fix their own downloads"), ("new_user_storage", "See server storage"),
    ("new_user_auto_download", "Auto-download (likes and followed playlists)"),
];

/// One setting of "New members start with": its key, label, value and whether an environment variable sets it
/// (then it is shown locked, with the same note as the Server pages' fields).
#[derive(Debug, PartialEq)]
struct NewDefault {
    /// The setting.
    key: &'static str,
    /// What the switch says.
    label: &'static str,
    /// Whether it is on.
    on: bool,
    /// Whether an environment variable sets it.
    locked: bool,
}

/// The switches of "New members start with", from the answer of GET /api/admin/settings.
fn new_defaults(v: &Value) -> Vec<NewDefault> {
    let meta = v.o("_meta");
    NEW.iter().map(|&(key, label)| NewDefault { key, label, on: v.truthy(key), locked: env_locked(&meta, key) }).collect()
}

/// The weekly request limit's box value ("0" when none is set).
fn limit_value(v: &Value) -> String {
    if v.is_nullish("default_request_limit") { "0".to_string() } else { v.s("default_request_limit") }
}

/// The answers the page draws from (`None` until each has come).
#[derive(Clone, Copy)]
struct Page {
    /// The answer of GET /api/admin/users.
    users: RwSignal<Option<Vec<Value>>>,
    /// The answer of GET /api/admin/permissions (the "Permissions for everyone" card).
    perms: RwSignal<Option<Value>>,
    /// The answer of GET /api/admin/settings (`Some(None)` when it failed: the card is left empty).
    defaults: RwSignal<Option<Option<Value>>>,
}

impl Page {
    /// The users table, fetched again (a failure keeps the table).
    fn load_users(self) {
        dom::spawn(async move {
            if let Ok(u) = api::get("/admin/users").await {
                self.users.try_set(Some(u.items()));
            }
        });
    }

    /// The permissions for everyone, fetched again (a failure keeps the card).
    fn load_perms(self) {
        dom::spawn(async move {
            if let Ok(d) = api::get("/admin/permissions").await {
                self.perms.try_set(Some(d));
            }
        });
    }
}

/// `PATCH /admin/users/{id}` with `body`: its toast, then the table again.
async fn patch(id: &str, body: Value, page: Page) {
    match api::patch(&format!("/admin/users/{id}"), body).await { Ok(_) => toast("Saved", "ok"), Err(e) => toast(&e.message, "err") }
    page.load_users();
}

/// Settings → Users: the view at once, then its answers; the users' failure is the page's error.
pub fn render_users(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        dom::set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let page = owner.with(|| Page { users: RwSignal::new(None), perms: RwSignal::new(None), defaults: RwSignal::new(None) });
        let me = ctx.user.clone();
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Users page me/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        page.load_perms();
        dom::spawn(async move {
            let v = api::get("/admin/settings").await.ok();
            page.defaults.try_set(Some(v));
        });
        let users = api::get("/admin/users").await.map_err(|e| e.message)?;
        page.users.set(Some(users.items()));
        Ok(())
    })
}

/// The page: the heading with Invites and Add user, the users table, the permissions for everyone and what new
/// members start with.
#[component]
fn Users(
    /// The answers.
    page: Page,
    /// The reader (who can't change their own role, nor disable or delete themselves).
    me: Value,
) -> impl IntoView {
    let table = move || {
        page.users.get().map(|users| {
            let rows = users.iter().map(|u| user_row(u, &me, page)).collect_view();
            view! {
                <div class="table-wrap"><table class="table"><thead><tr>
                    <th>"User"</th><th>"Role"</th><th class="hide-sm">"Requests"</th><th class="hide-sm">"Weekly limit"</th><th>"Auto-approve"</th><th class="hide-sm">"Last seen"</th><th></th></tr></thead><tbody>
                    {rows}
                </tbody></table></div>
            }
        })
    };
    let bulk = move || page.perms.get().map(|d| bulk_view(&d, page));
    let defaults = move || page.defaults.get().flatten().map(|v| defaults_view(&v));
    view! {
        <div class="row wrap between" style="margin-bottom:18px">
            <div><h1>"Users"</h1><div class="small muted">"Manage who can access your library and what they can request."</div></div>
            <div class="row"><a class="btn" href="#/settings?s=invites" {..html(format!("{}Invites", icon("ticket")))}>{""}</a><button class="btn primary" id="add" on:click=move |_| add_user_dialog(page) {..html(format!("{}Add user", icon("plus")))}>{""}</button></div>
        </div>
        <div class="card flat" id="users">{table}</div>
        <div class="section-head"><h2>"Permissions for everyone"</h2><span class="small muted">"applies to every non-admin user; fine-tune one user with the gear in their row"</span></div>
        <div class="card flat" id="bulk-perms">{bulk}</div>
        <div class="section-head"><h2>"New members start with"</h2><span class="small muted">"copied onto every new account (sign-up, invite or added here); change one person later with their gear"</span></div>
        <div class="card" id="new-defaults">{defaults}</div>
    }
}

/// "Permissions for everyone": each permission with how many have it, and All on / All off.
fn bulk_view(d: &Value, page: Page) -> impl IntoView {
    let rows = d
        .a("perms")
        .iter()
        .map(|p| {
            let key = p.s("key");
            let set = move |on: bool| {
                let perm = key.clone();
                move |_| {
                    let perm = perm.clone();
                    dom::spawn(async move {
                        match api::post("/admin/users/bulk", json!({ "perm": perm, "value": on })).await {
                            Ok(_) => {
                                toast("Updated for everyone", "ok");
                                page.load_perms();
                                page.load_users();
                            }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                }
            };
            view! {
                <div class="list-item" style="flex-wrap:wrap">
                    <div class="grow" style="min-width:200px"><div style="font-weight:600">{p.s("label")}</div><div class="small muted">{format!("{} of {} user(s) have it", p.s("on"), p.s("of"))}</div></div>
                    <div class="row" style="gap:6px;flex:none"><button class="btn sm" data-bulk=p.s("key") data-v="1" on:click=set(true)>"All on"</button><button class="btn sm" data-bulk=p.s("key") data-v="0" on:click=set(false)>"All off"</button></div></div>
            }
        })
        .collect_view();
    view! { <div class="list">{rows}</div> }
}

/// "New members start with": a switch per default permission and the weekly request limit; a change is saved
/// at once.
fn defaults_view(v: &Value) -> impl IntoView {
    let meta = v.o("_meta");
    let on_change = move |e: web_sys::Event| {
        let Some(t) = dom::target_el(&e) else { return };
        let k = dom::data(&t, "nd");
        if k.is_empty() {
            return;
        }
        let val = if input_type(&t) == "checkbox" { Value::Bool(dom::checked(&t)) } else { js_plus(&dom::value(&t)) };
        dom::spawn(async move {
            match api::put("/admin/settings", obj(&k, val)).await {
                Ok(_) => toast("Saved for new members", "ok"),
                Err(err) => toast(&err.message, "err"),
            }
        });
    };
    let switches = new_defaults(v)
        .into_iter()
        .map(|d| {
            let switch = view! { <label class="switch"><input type="checkbox" data-nd=d.key checked=d.on disabled=d.locked/><i></i>{d.label}</label> };
            if d.locked {
                view! { <div class="field">{switch}{raw(env_note(&meta, d.key))}</div> }.into_any()
            } else {
                switch.into_any()
            }
        })
        .collect_view();
    view! {
        <div class="stack" on:change=on_change>{switches}
            <label class="field" style="max-width:260px"><span>"Weekly request limit (0 = unlimited)"</span><input class="input" type="number" min="0" data-nd="default_request_limit" value=limit_value(v) disabled=env_locked(&meta, "default_request_limit")/>{raw(env_note(&meta, "default_request_limit"))}</label></div>
    }
}

/// One row of the users table: who, their role, requests, weekly limit, auto-approve, last seen, and the
/// buttons (permissions for members, reset password, disable and delete for anyone but the reader).
fn user_row(u: &Value, me: &Value, page: Page) -> impl IntoView {
    let is_me = u.g("id") == me.g("id");
    let role = u.s("role");
    let disabled = u.truthy("disabled");
    let id = u.s("id");
    // the name the dialogs say: the first word of the name cell, as before
    let name = u.s("username").split(' ').next().unwrap_or("").to_string();
    let mut meta: Vec<String> = Vec::new();
    if u.truthy("email") { meta.push(u.s("email")); }
    if u.truthy("invite_code") { meta.push(format!("invite {}", u.s("invite_code"))); }
    meta.push(format!("joined {}", fmt::ago(u.f("created_at"))));
    // a role, the weekly limit or auto-approve: saved at once, then the table again
    let i2 = id.clone();
    let on_change = move |e: web_sys::Event| {
        let Some(t) = dom::target_el(&e) else { return };
        let f = dom::data(&t, "f");
        if f.is_empty() {
            return;
        }
        let val = if input_type(&t) == "checkbox" {
            Value::Bool(dom::checked(&t))
        } else if f == "request_limit" {
            Value::from(parse_int(&dom::value(&t)))
        } else {
            Value::String(dom::value(&t))
        };
        let id = i2.clone();
        dom::spawn(async move { patch(&id, obj(&f, val), page).await });
    };
    let role_cell = if role == "owner" {
        view! { <span class="badge accent">"Owner"</span> }.into_any()
    } else {
        view! {
            <select class="input" data-f="role" style="width:auto;padding:6px 10px" disabled=is_me>
                <option value="user" selected=role == "user">"User"</option><option value="admin" selected=role == "admin">"Admin"</option></select>
        }
        .into_any()
    };
    let perms = (!matches!(role.as_str(), "admin" | "owner")).then(|| {
        let (id, name) = (id.clone(), name.clone());
        view! { <button class="btn sm" data-a="perms" title="Permissions" on:click=move |_| perms_dialog(id.clone(), name.clone(), page) {..html(icon("settings"))}>{""}</button> }
    });
    let (i3, n3) = (id.clone(), name.clone());
    let pw = move |_| password_dialog(i3.clone(), &n3, page);
    let others = (!is_me).then(|| {
        let i4 = id.clone();
        let toggle = move |_| {
            let id = i4.clone();
            dom::spawn(async move { patch(&id, json!({ "disabled": !disabled }), page).await });
        };
        let (i5, n5) = (id.clone(), name.clone());
        let del = move |_| {
            let (id, name) = (i5.clone(), n5.clone());
            dom::spawn(async move {
                if confirm_dialog("Delete user", &format!("Delete {name}? Their requests stay in history."), "Delete", true).await && api::del(&format!("/admin/users/{id}")).await.is_ok() {
                    page.load_users();
                }
            });
        };
        view! {
            <button class="btn sm" data-a="toggle" on:click=toggle>{if disabled { "Enable" } else { "Disable" }}</button>
            " "<button class="btn sm danger ghost" data-a="del" title="Delete" on:click={del} {..html(icon("trash"))}>{""}</button>
        }
    });
    view! {
        <tr data-id=id on:change=on_change>
            <td><div class="row">{raw(app::avatar_html(u, 32))}<div><div style="font-weight:650">{u.s("username")}" "{is_me.then(|| view! { <span class="badge">"you"</span> })}" "{disabled.then(|| view! { <span class="badge err">"disabled"</span> })}</div>
                <div class="small faint">{meta.join(" · ")}</div></div></div></td>
            <td>{role_cell}</td>
            <td class="hide-sm">{u.s("request_count")}</td>
            <td class="hide-sm"><input class="input" data-f="request_limit" type="number" min="0" value=if u.truthy("request_limit") { u.s("request_limit") } else { String::new() } placeholder="∞" style="width:80px;padding:6px 10px"/></td>
            <td><label class="switch"><input type="checkbox" data-f="auto_approve" checked=u.truthy("auto_approve")/><i></i></label></td>
            <td class="hide-sm small muted">{fmt::ago(u.f("last_login"))}</td>
            <td style="text-align:right;white-space:nowrap">
                {perms}" "
                <button class="btn sm" data-a="pw" title="Reset password" on:click={pw} {..html(icon("shield"))}>{""}</button>" "
                {others}
            </td></tr>
    }
}

/// A member's own permissions, each a switch; Save sends them all.
fn perms_dialog(id: String, name: String, page: Page) {
    dom::spawn(async move {
        let (users, d) = futures::join!(api::get("/admin/users"), api::get("/admin/permissions"));
        let (Ok(users), Ok(d)) = (users, d) else { return };
        let Some(u) = users.items().into_iter().find(|x| x.s("id") == id) else { return };
        let m = modal(ModalOpts::new(&format!("Permissions – {}", esc(&name)),
            &format!(r#"<div class="stack">{}</div>"#, d.a("perms").iter().map(|p| {
                let key = p.s("key");
                format!(r#"<label class="switch"><input type="checkbox" data-p="{key}" {}><i></i>{}</label>"#, if u.g("perms").truthy(&key) { "checked" } else { "" }, esc(&p.s("label")))
            }).collect::<String>()))
            .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Save</button>"#));
        let Some(ok) = m.qs("#ok") else { return };
        dom::on(&ok, "click", move |_| {
            let mut perms = Map::new();
            for x in m.qsa("[data-p]") { perms.insert(dom::data(&x, "p"), Value::Bool(dom::checked(&x))); }
            let (m, id) = (m.clone(), id.clone());
            dom::spawn(async move {
                patch(&id, json!({ "perms": Value::Object(perms) }), page).await;
                m.close();
                page.load_perms();
            });
        });
    });
}

/// Reset password: a new one for the user (all their sessions are signed out).
fn password_dialog(id: String, name: &str, page: Page) {
    let m = modal(ModalOpts::new(&format!("Reset password for {}", esc(name)),
        r#"<label class="field"><span>New password</span><input class="input" type="password" id="np" minlength="8" autocomplete="new-password"></label><p class="small muted">All of their sessions will be signed out.</p>"#)
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Set password</button>"#));
    let Some(ok) = m.qs("#ok") else { return };
    dom::on(&ok, "click", move |_| {
        let (m, id) = (m.clone(), id.clone());
        dom::spawn(async move {
            patch(&id, json!({ "password": dom::value_of(&m.el, "#np") }), page).await;
            m.close();
        });
    });
}

/// Add user: name, email, password and role; Create makes the account.
fn add_user_dialog(page: Page) {
    let m = modal(ModalOpts::new("Add user", r#"<form class="stack" id="nu">
        <label class="field"><span>Username</span><input class="input" name="username" required minlength="3"></label>
        <label class="field"><span>Email (optional)</span><input class="input" name="email" type="email"></label>
        <label class="field"><span>Password</span><input class="input" type="password" name="password" required minlength="8" autocomplete="new-password"></label>
        <label class="field"><span>Role</span><select class="input" name="role"><option value="user">User</option><option value="admin">Admin</option></select></label>
      </form>"#).footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Create</button>"#));
    let Some(ok) = m.qs("#ok") else { return };
    dom::on(&ok, "click", move |_| {
        let Some(f) = m.qs("#nu") else { return };
        if !report_validity(&f) { return; }
        let body = form_entries(&f);
        let m = m.clone();
        dom::spawn(async move {
            match api::post("/admin/users", body).await {
                Ok(_) => { m.close(); toast("User created", "ok"); page.load_users(); }
                Err(e) => toast(&e.message, "err"),
            }
        });
    });
}

#[cfg(test)]
mod new_defaults_tests {
    use super::*;

    /// A default an environment variable sets is locked; one set in the UI is not.
    #[test]
    fn values_set_by_the_environment_are_locked() {
        let v = json!({ "new_user_pick": true, "new_user_fix": false, "default_request_limit": 5,
            "_meta": { "new_user_pick": { "source": "env", "env": "ENCORE_NEW_USER_PICK" }, "new_user_fix": { "source": "ui", "env": "ENCORE_NEW_USER_FIX" },
                       "default_request_limit": { "source": "env", "env": "ENCORE_REQUEST_LIMIT" } } });
        let d = new_defaults(&v);
        let of = |k: &str| d.iter().find(|x| x.key == k).unwrap();
        assert!(of("new_user_pick").on && of("new_user_pick").locked);
        assert!(!of("new_user_fix").locked);
        assert!(env_locked(&v.o("_meta"), "default_request_limit"));
        assert_eq!(limit_value(&v), "5");
    }

    /// Without `_meta` everything stays editable, and the limit box shows 0.
    #[test]
    fn without_meta_everything_stays_editable() {
        let v = json!({ "new_user_pick": true });
        assert!(new_defaults(&v).iter().all(|d| !d.locked));
        assert_eq!(limit_value(&v), "0");
    }
}
