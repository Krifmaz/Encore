//! Port of web/js/pages/admin.js – see docs/ui-rewrite/CONTRACT.md
//! The admin sub-pages of Settings: Users & permissions (render_users) and Invites (render_invites).
use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom::{self, set_html};
use crate::json::{truthy, J};
use crate::ui::{confirm_dialog, copy_text, empty_state, esc, fmt, icon, modal, toast, ModalOpts};
use serde_json::{json, Map, Value};
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::Element;

/// `view.querySelector(sel)` that has to exist (the JS would throw on `null.innerHTML`).
fn must(root: &Element, sel: &str) -> Result<Element, String> { dom::qs_in(root, sel).ok_or_else(|| format!("{sel} not found")) }

/// A JS number as JSON (`12` stays `12`, `2.5` stays `2.5`).
fn js_num(f: f64) -> Value { if f.fract() == 0.0 && f.abs() < 1e15 { Value::from(f as i64) } else { Value::from(f) } }

/// `+s` as JSON.stringify sends it: a number, `0` for "", `null` for NaN.
fn js_plus(s: &str) -> Value {
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
fn form_entries(form: &Element) -> Value {
    let Some(f) = form.dyn_ref::<web_sys::HtmlFormElement>() else { return json!({}) };
    let Ok(fd) = web_sys::FormData::new_with_form(f) else { return json!({}) };
    let Ok(o) = js_sys::Object::from_entries(&fd) else { return json!({}) };
    serde_wasm_bindgen::from_value(o.into()).unwrap_or_else(|_| json!({}))
}

/// `new Date(ts * 1000).toLocaleDateString()`.
fn locale_date(ts: f64) -> String {
    js_sys::Date::new(&JsValue::from_f64(ts * 1000.0)).to_locale_date_string("default", &JsValue::UNDEFINED).into()
}

// ---------------------------------------------------------------- Users

const NEW: [(&str, &str); 6] = [
    ("new_user_auto_approve", "Requests skip the approval queue"), ("new_user_auto_approve_playlists", "Playlist downloads skip the approval queue"),
    ("new_user_pick", "Pick the exact version to download"), ("new_user_fix", "Fix their own downloads"), ("new_user_storage", "See server storage"),
    ("new_user_auto_download", "Auto-download (likes and followed playlists)"),
];

/// One `<tr>` of the users table.
fn user_row(u: &Value, me: &Value) -> String {
    let is_me = u.g("id") == me.g("id");
    let role = u.s("role");
    let disabled = u.truthy("disabled");
    let mut meta: Vec<String> = Vec::new();
    if u.truthy("email") { meta.push(esc(&u.s("email"))); }
    if u.truthy("invite_code") { meta.push(format!("invite {}", esc(&u.s("invite_code")))); }
    meta.push(format!("joined {}", fmt::ago(u.f("created_at"))));
    let role_cell = if role == "owner" { r#"<span class="badge accent">Owner</span>"#.to_string() } else {
        format!(r#"<select class="input" data-f="role" style="width:auto;padding:6px 10px" {}>
          <option value="user" {}>User</option><option value="admin" {}>Admin</option></select>"#,
            if is_me { "disabled" } else { "" }, if role == "user" { "selected" } else { "" }, if role == "admin" { "selected" } else { "" })
    };
    format!(r#"<tr data-id="{}">
        <td><div class="row">{}<div><div style="font-weight:650">{} {} {}</div>
          <div class="small faint">{}</div></div></div></td>
        <td>{}</td>
        <td class="hide-sm">{}</td>
        <td class="hide-sm"><input class="input" data-f="request_limit" type="number" min="0" value="{}" placeholder="∞" style="width:80px;padding:6px 10px"></td>
        <td><label class="switch"><input type="checkbox" data-f="auto_approve" {}><i></i></label></td>
        <td class="hide-sm small muted">{}</td>
        <td style="text-align:right;white-space:nowrap">
          {}
          <button class="btn sm" data-a="pw" title="Reset password">{}</button>
          {}
        </td></tr>"#,
        u.s("id"), app::avatar_html(u, 32), esc(&u.s("username")),
        if is_me { r#"<span class="badge">you</span>"# } else { "" },
        if disabled { r#"<span class="badge err">disabled</span>"# } else { "" },
        meta.join(" · "),
        role_cell,
        u.s("request_count"),
        if u.truthy("request_limit") { u.s("request_limit") } else { String::new() },
        if u.truthy("auto_approve") { "checked" } else { "" },
        fmt::ago(u.f("last_login")),
        if !matches!(role.as_str(), "admin" | "owner") { format!(r#"<button class="btn sm" data-a="perms" title="Permissions">{}</button>"#, icon("settings")) } else { String::new() },
        icon("shield"),
        if !is_me { format!(r#"<button class="btn sm" data-a="toggle">{}</button>
          <button class="btn sm danger ghost" data-a="del" title="Delete">{}</button>"#, if disabled { "Enable" } else { "Disable" }, icon("trash")) } else { String::new() })
}

/// The `load` of renderUsers: `GET /admin/users` into the table.
async fn load_users(bx: &Element, me: &Value) -> Result<(), String> {
    let users = api::get("/admin/users").await.map_err(|e| e.message)?;
    set_html(bx, &format!(r#"<div class="table-wrap"><table class="table"><thead><tr>
      <th>User</th><th>Role</th><th class="hide-sm">Requests</th><th class="hide-sm">Weekly limit</th><th>Auto-approve</th><th class="hide-sm">Last seen</th><th></th></tr></thead><tbody>
      {}
      </tbody></table></div>"#, users.items().iter().map(|u| user_row(u, me)).collect::<String>()));
    Ok(())
}

/// The `loadBulk` of renderUsers: `GET /admin/permissions` into the "Permissions for everyone" card.
async fn load_bulk_perms(bulk_box: &Element) {
    let Ok(d) = api::get("/admin/permissions").await else { return };
    set_html(bulk_box, &format!(r#"<div class="list">{}</div>"#, d.a("perms").iter().map(|p| format!(r#"<div class="list-item" style="flex-wrap:wrap">
      <div class="grow" style="min-width:200px"><div style="font-weight:600">{}</div><div class="small muted">{} of {} user(s) have it</div></div>
      <div class="row" style="gap:6px;flex:none"><button class="btn sm" data-bulk="{key}" data-v="1">All on</button><button class="btn sm" data-bulk="{key}" data-v="0">All off</button></div></div>"#,
        esc(&p.s("label")), p.s("on"), p.s("of"), key = p.s("key"))).collect::<String>()));
}

/// The `patch` of renderUsers: `PATCH /admin/users/{id}`, toast, reload the table.
async fn patch(id: &str, body: Value, load: &Rc<dyn Fn()>) {
    match api::patch(&format!("/admin/users/{id}"), body).await { Ok(_) => toast("Saved", "ok"), Err(e) => toast(&e.message, "err") }
    load();
}

pub fn render_users(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        set_html(&view, &format!(r##"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Users</h1><div class="small muted">Manage who can access your library and what they can request.</div></div>
      <div class="row"><a class="btn" href="#/settings?s=invites">{}Invites</a><button class="btn primary" id="add">{}Add user</button></div>
    </div>
    <div class="card flat" id="users"></div>
    <div class="section-head"><h2>Permissions for everyone</h2><span class="small muted">applies to every non-admin user; fine-tune one user with the gear in their row</span></div>
    <div class="card flat" id="bulk-perms"></div>
    <div class="section-head"><h2>New members start with</h2><span class="small muted">copied onto every new account (sign-up, invite or added here); change one person later with their gear</span></div>
    <div class="card" id="new-defaults"></div>"##, icon("ticket"), icon("plus")));
        let bx = must(&view, "#users")?;
        let bulk_box = must(&view, "#bulk-perms")?;
        let me = ctx.user.clone();
        let load_bulk: Rc<dyn Fn()> = {
            let bb = bulk_box.clone();
            Rc::new(move || { let bb = bb.clone(); dom::spawn(async move { load_bulk_perms(&bb).await; }) })
        };
        let load: Rc<dyn Fn()> = {
            let (bx, me) = (bx.clone(), me.clone());
            Rc::new(move || { let (bx, me) = (bx.clone(), me.clone()); dom::spawn(async move { let _ = load_users(&bx, &me).await; }) })
        };
        {
            let (lb, ld) = (load_bulk.clone(), load.clone());
            dom::on(&bulk_box, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-bulk]") else { return };
                let (perm, v) = (dom::data(&b, "bulk"), dom::data(&b, "v"));
                let (lb, ld) = (lb.clone(), ld.clone());
                dom::spawn(async move {
                    match api::post("/admin/users/bulk", json!({ "perm": perm, "value": v == "1" })).await {
                        Ok(_) => { toast("Updated for everyone", "ok"); lb(); ld(); }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
        load_bulk();
        let def_box = must(&view, "#new-defaults")?;
        {
            let db = def_box.clone();
            dom::spawn(async move {
                match api::get("/admin/settings").await {
                    Ok(v) => {
                        set_html(&db, &format!(r#"<div class="stack">{}
      <label class="field" style="max-width:260px"><span>Weekly request limit (0 = unlimited)</span><input class="input" type="number" min="0" data-nd="default_request_limit" value="{}"></label></div>"#,
                            NEW.iter().map(|(k, l)| format!(r#"<label class="switch"><input type="checkbox" data-nd="{k}" {}><i></i>{}</label>"#, if v.truthy(k) { "checked" } else { "" }, esc(l))).collect::<String>(),
                            esc(&if v.is_nullish("default_request_limit") { "0".to_string() } else { v.s("default_request_limit") })));
                        dom::on(&db, "change", move |e| {
                            let Some(t) = dom::target_el(e) else { return };
                            let k = dom::data(&t, "nd");
                            if k.is_empty() { return; }
                            let val = if input_type(&t) == "checkbox" { Value::Bool(dom::checked(&t)) } else { js_plus(&dom::value(&t)) };
                            dom::spawn(async move {
                                match api::put("/admin/settings", obj(&k, val)).await { Ok(_) => toast("Saved for new members", "ok"), Err(err) => toast(&err.message, "err") }
                            });
                        });
                    }
                    Err(_) => dom::set_text(&db, ""),
                }
            });
        }
        {
            let ld = load.clone();
            dom::on(&bx, "change", move |e| {
                let Some(t) = dom::target_el(e) else { return };
                let f = dom::data(&t, "f");
                if f.is_empty() { return; }
                let Some(tr) = dom::closest_of(&t, "[data-id]") else { return };
                let id = dom::data(&tr, "id");
                let val = if input_type(&t) == "checkbox" { Value::Bool(dom::checked(&t)) }
                    else if f == "request_limit" { Value::from(parse_int(&dom::value(&t))) }
                    else { Value::String(dom::value(&t)) };
                let ld = ld.clone();
                dom::spawn(async move { patch(&id, obj(&f, val), &ld).await; });
            });
        }
        {
            let (ld, lb) = (load.clone(), load_bulk.clone());
            dom::on(&bx, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-a]") else { return };
                let Some(tr) = dom::closest_of(&b, "[data-id]") else { return };
                let id = dom::data(&tr, "id");
                let Some(name_el) = dom::qs_in(&tr, r#"[style*="font-weight:650"]"#) else { return };
                let name = dom::text_of(&name_el).trim().split(' ').next().unwrap_or("").to_string();
                let a = dom::data(&b, "a");
                let is_disable = dom::text_of(&b).trim() == "Disable";
                let (ld, lb) = (ld.clone(), lb.clone());
                dom::spawn(async move {
                    match a.as_str() {
                        "perms" => {
                            let (users, d) = futures::join!(api::get("/admin/users"), api::get("/admin/permissions"));
                            let (Ok(users), Ok(d)) = (users, d) else { return };
                            let Some(u) = users.items().into_iter().find(|x| x.s("id") == id) else { return };
                            let m = modal(ModalOpts::new(&format!("Permissions – {}", esc(&name)),
                                &format!(r#"<div class="stack">{}</div>"#, d.a("perms").iter().map(|p| {
                                    let key = p.s("key");
                                    format!(r#"<label class="switch"><input type="checkbox" data-p="{key}" {}><i></i>{}</label>"#, if u.g("perms").truthy(&key) { "checked" } else { "" }, esc(&p.s("label")))
                                }).collect::<String>()))
                                .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Save</button>"#));
                            if let Some(ok) = m.qs("#ok") {
                                let (m2, id, ld, lb) = (m.clone(), id.clone(), ld.clone(), lb.clone());
                                dom::on(&ok, "click", move |_| {
                                    let mut perms = Map::new();
                                    for x in m2.qsa("[data-p]") { perms.insert(dom::data(&x, "p"), Value::Bool(dom::checked(&x))); }
                                    let (m2, id, ld, lb) = (m2.clone(), id.clone(), ld.clone(), lb.clone());
                                    dom::spawn(async move { patch(&id, json!({ "perms": Value::Object(perms) }), &ld).await; m2.close(); lb(); });
                                });
                            }
                        }
                        "toggle" => patch(&id, json!({ "disabled": is_disable }), &ld).await,
                        "del" => {
                            if confirm_dialog("Delete user", &format!("Delete {name}? Their requests stay in history."), "Delete", true).await
                                && api::del(&format!("/admin/users/{id}")).await.is_ok() { ld(); }
                        }
                        "pw" => {
                            let m = modal(ModalOpts::new(&format!("Reset password for {}", esc(&name)),
                                r#"<label class="field"><span>New password</span><input class="input" type="password" id="np" minlength="8" autocomplete="new-password"></label><p class="small muted">All of their sessions will be signed out.</p>"#)
                                .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Set password</button>"#));
                            if let Some(ok) = m.qs("#ok") {
                                let (m2, id, ld) = (m.clone(), id.clone(), ld.clone());
                                dom::on(&ok, "click", move |_| {
                                    let (m2, id, ld) = (m2.clone(), id.clone(), ld.clone());
                                    dom::spawn(async move { patch(&id, json!({ "password": dom::value_of(&m2.el, "#np") }), &ld).await; m2.close(); });
                                });
                            }
                        }
                        _ => {}
                    }
                });
            });
        }
        {
            let ld = load.clone();
            dom::on_sel(&view, "#add", "click", move |_| {
                let m = modal(ModalOpts::new("Add user", r#"<form class="stack" id="nu">
        <label class="field"><span>Username</span><input class="input" name="username" required minlength="3"></label>
        <label class="field"><span>Email (optional)</span><input class="input" name="email" type="email"></label>
        <label class="field"><span>Password</span><input class="input" type="password" name="password" required minlength="8" autocomplete="new-password"></label>
        <label class="field"><span>Role</span><select class="input" name="role"><option value="user">User</option><option value="admin">Admin</option></select></label>
      </form>"#).footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Create</button>"#));
                if let Some(ok) = m.qs("#ok") {
                    let (m2, ld) = (m.clone(), ld.clone());
                    dom::on(&ok, "click", move |_| {
                        let Some(f) = m2.qs("#nu") else { return };
                        if !report_validity(&f) { return; }
                        let body = form_entries(&f);
                        let (m2, ld) = (m2.clone(), ld.clone());
                        dom::spawn(async move {
                            match api::post("/admin/users", body).await {
                                Ok(_) => { m2.close(); toast("User created", "ok"); ld(); }
                                Err(e) => toast(&e.message, "err"),
                            }
                        });
                    });
                }
            });
        }
        load_users(&bx, &me).await
    })
}

// ---------------------------------------------------------------- Invites

/// One `<tr>` of the invites table.
fn invite_row(i: &Value, now: f64) -> String {
    let dead = i.f("uses") >= i.f("max_uses") || (i.truthy("expires_at") && i.f("expires_at") < now);
    let code = esc(&i.s("code"));
    format!(r#"<tr data-id="{}" data-code="{code}">
          <td><span class="mono" style="font-weight:700;{}">{code}</span>{}</td>
          <td>{} / {}</td>
          <td class="hide-sm">{}</td>
          <td class="hide-sm small muted">{}</td>
          <td class="hide-sm small muted">{}</td>
          <td style="text-align:right;white-space:nowrap">{}
            <button class="btn sm danger ghost" data-a="del">{}</button></td></tr>"#,
        i.s("id"),
        if dead { "text-decoration:line-through;opacity:.5" } else { "" },
        if i.truthy("note") { format!(r#"<div class="small faint">{}</div>"#, esc(&i.s("note"))) } else { String::new() },
        i.s("uses"), i.s("max_uses"),
        if matches!(i.s("role").as_str(), "admin" | "owner") { r#"<span class="badge warn">admin</span>"# } else { "user" },
        if i.truthy("expires_at") { if i.f("expires_at") < now { "expired".to_string() } else { locale_date(i.f("expires_at")) } } else { "never".to_string() },
        esc(i.st("used_by").unwrap_or("—")),
        if dead { String::new() } else { format!(r#"<button class="btn sm" data-a="copy">{}Copy link</button>"#, icon("copy")) },
        icon("trash"))
}

/// The `load` of renderInvites: `GET /admin/invites` into the table (or the empty state).
async fn load_invites(bx: &Element) -> Result<(), String> {
    let rows = api::get("/admin/invites").await.map_err(|e| e.message)?;
    let now = dom::now_ms() / 1000.0;
    let rows = rows.items();
    set_html(bx, &if rows.is_empty() {
        empty_state(&icon("ticket"), "No invites yet", "Create one and send the link to a friend.")
    } else {
        format!(r#"<div class="table-wrap"><table class="table"><thead><tr><th>Code</th><th>Uses</th><th class="hide-sm">Role</th><th class="hide-sm">Expires</th><th class="hide-sm">Used by</th><th></th></tr></thead><tbody>
      {}</tbody></table></div>"#, rows.iter().map(|i| invite_row(i, now)).collect::<String>())
    });
    Ok(())
}

pub fn render_invites(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let settings = api::get("/admin/settings").await.map_err(|e| e.message)?;
        const MODES: [(&str, &str); 3] = [("invite", "Invite only"), ("open", "Open"), ("closed", "Closed")];
        set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Invites</h1><div class="small muted">Share an invite link so friends can create an account.</div></div>
      <button class="btn primary" id="new">{}New invite</button>
    </div>
    <div class="card row wrap between" style="margin-bottom:18px">
      <div><b>Sign-up mode</b><div class="small muted">Who is allowed to create an account on the login page.</div></div>
      <div class="tabs" id="mode">{}</div>
    </div>
    <div class="card flat" id="inv"></div>"#, icon("plus"),
            MODES.iter().map(|(k, l)| format!(r#"<button data-m="{k}" class="{}">{l}</button>"#, if settings.s("signup_mode") == *k { "active" } else { "" })).collect::<String>()));
        {
            let v = view.clone();
            dom::on_sel(&view, "#mode", "click", move |e| {
                let Some(b) = dom::closest(e, "[data-m]") else { return };
                let v = v.clone();
                dom::spawn(async move {
                    if api::put("/admin/settings", json!({ "signup_mode": dom::data(&b, "m") })).await.is_err() { return; }
                    for x in dom::qsa_in(&v, "#mode button") { dom::toggle_class(&x, "active", x == b); }
                    toast(&format!("Sign-ups: {}", dom::text_of(&b)), "ok");
                });
            });
        }
        let base = settings.st("base_url").map(str::to_string).unwrap_or_else(|| dom::location().origin().unwrap_or_default());
        let base = base.trim_end_matches('/').to_string();
        let link: Rc<dyn Fn(&str) -> String> = Rc::new(move |code| format!("{base}/#/signup?invite={}", String::from(js_sys::encode_uri_component(code))));
        let bx = must(&view, "#inv")?;
        let load: Rc<dyn Fn()> = {
            let bx = bx.clone();
            Rc::new(move || { let bx = bx.clone(); dom::spawn(async move { let _ = load_invites(&bx).await; }) })
        };
        {
            let (ld, link) = (load.clone(), link.clone());
            dom::on(&bx, "click", move |e| {
                let Some(b) = dom::closest(e, "[data-a]") else { return };
                let Some(tr) = dom::closest_of(&b, "[data-id]") else { return };
                match dom::data(&b, "a").as_str() {
                    "copy" => copy_text(&link(&dom::data(&tr, "code"))),
                    "del" => {
                        let (ld, id) = (ld.clone(), dom::data(&tr, "id"));
                        dom::spawn(async move { if api::del(&format!("/admin/invites/{id}")).await.is_ok() { ld(); } });
                    }
                    _ => {}
                }
            });
        }
        {
            let (ld, link) = (load.clone(), link.clone());
            dom::on_sel(&view, "#new", "click", move |_| {
                let m = modal(ModalOpts::new("New invite", r#"<form class="stack" id="ni">
        <div class="form-grid">
          <label class="field"><span>Max uses</span><input class="input" type="number" name="max_uses" value="1" min="1"></label>
          <label class="field"><span>Expires after (days)</span><input class="input" type="number" name="expires_days" value="7" min="0" step="0.5"><small>0 = never</small></label>
        </div>
        <label class="field"><span>Role</span><select class="input" name="role"><option value="user">User</option><option value="admin">Admin</option></select></label>
        <label class="field"><span>Note</span><input class="input" name="note" placeholder="e.g. for Sam"></label>
      </form><div id="made"></div>"#).footer(r#"<button class="btn" data-close>Close</button><button class="btn primary" id="ok">Create invite</button>"#));
                if let Some(ok) = m.qs("#ok") {
                    let (m2, ld, link) = (m.clone(), ld.clone(), link.clone());
                    dom::on(&ok, "click", move |ev| {
                        let f = m2.qs("#ni").map(|f| form_entries(&f)).unwrap_or_else(|| json!({}));
                        let target = dom::target_el(ev);
                        let (m2, ld, link) = (m2.clone(), ld.clone(), link.clone());
                        dom::spawn(async move {
                            let max_uses = match js_plus(&f.s("max_uses")) { v if truthy(&v) => v, _ => Value::from(1) };
                            let expires_days = match js_plus(&f.s("expires_days")) { v if truthy(&v) => v, _ => Value::Null };
                            let note = if f.truthy("note") { f.g("note").clone() } else { Value::Null };
                            match api::post("/admin/invites", json!({ "max_uses": max_uses, "expires_days": expires_days, "role": f.g("role"), "note": note })).await {
                                Ok(inv) => {
                                    if let Some(ni) = m2.qs("#ni") { dom::add_class(&ni, "hidden"); }
                                    if let Some(t) = &target { dom::add_class(t, "hidden"); }
                                    let code = inv.s("code");
                                    if let Some(made) = m2.qs("#made") {
                                        set_html(&made, &format!(r#"<p class="muted" style="margin-top:0">Send this link – it signs them up with the invite filled in:</p>
          <div class="code">{}</div>
          <div class="row mt"><button class="btn primary" id="cp">{}Copy link</button><span class="muted">Code: <b class="mono">{}</b></span></div>"#,
                                            esc(&link(&code)), icon("copy"), esc(&code)));
                                    }
                                    if let Some(cp) = m2.qs("#cp") {
                                        let (link, code) = (link.clone(), code.clone());
                                        dom::on(&cp, "click", move |_| copy_text(&link(&code)));
                                    }
                                    ld();
                                }
                                Err(e) => toast(&e.message, "err"),
                            }
                        });
                    });
                }
            });
        }
        load_invites(&bx).await
    })
}
