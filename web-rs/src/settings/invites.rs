//! Port of web/js/pages/admin.js (renderInvites) – see docs/archive/ui-rewrite/CONTRACT.md
//! Settings → Invites: the sign-up mode, the invites table and New invite.
use super::schema_form::{env_locked, env_note};
use super::users::{form_entries, js_plus, must};
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom::{self, set_html};
use crate::json::{truthy, J};
use crate::ui::{copy_text, empty_state, esc, icon, modal, toast, ModalOpts};
use serde_json::{json, Value};
use std::rc::Rc;
use wasm_bindgen::JsValue;
use web_sys::Element;

/// `new Date(ts * 1000).toLocaleDateString()`.
fn locale_date(ts: f64) -> String {
    js_sys::Date::new(&JsValue::from_f64(ts * 1000.0)).to_locale_date_string("default", &JsValue::UNDEFINED).into()
}

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

/// The Sign-up mode card's content: its title and the three modes, locked (with the Server pages' note) when an
/// environment variable sets the mode.
fn signup_mode_html(settings: &Value) -> String {
    const MODES: [(&str, &str); 3] = [("invite", "Invite only"), ("open", "Open"), ("closed", "Closed")];
    let meta = settings.o("_meta");
    let dis = if env_locked(&meta, "signup_mode") { "disabled" } else { "" };
    format!(r#"<div><b>Sign-up mode</b><div class="small muted">Who is allowed to create an account on the login page.</div>{}</div>
      <div class="tabs" id="mode">{}</div>"#,
        env_note(&meta, "signup_mode"),
        MODES.iter().map(|(k, l)| format!(r#"<button data-m="{k}" class="{}" {dis}>{l}</button>"#, if settings.s("signup_mode") == *k { "active" } else { "" })).collect::<String>())
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
        set_html(&view, &format!(r#"
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Invites</h1><div class="small muted">Share an invite link so friends can create an account.</div></div>
      <button class="btn primary" id="new">{}New invite</button>
    </div>
    <div class="card row wrap between" style="margin-bottom:18px">
      {}
    </div>
    <div class="card flat" id="inv"></div>"#, icon("plus"), signup_mode_html(&settings)));
        {
            let v = view.clone();
            dom::on_sel(&view, "#mode", "click", move |e| {
                let Some(b) = dom::closest(e, "[data-m]") else { return };
                if b.has_attribute("disabled") { return; }   // set by an environment variable
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

#[cfg(test)]
mod signup_mode_tests {
    use super::*;

    #[test]
    fn a_mode_set_by_the_environment_is_locked() {
        let s = json!({ "signup_mode": "open", "_meta": { "signup_mode": { "source": "env", "env": "ENCORE_SIGNUP_MODE" } } });
        let html = signup_mode_html(&s);
        assert_eq!(html.matches("disabled").count(), 3, "{html}");
        assert!(html.contains(r#"<button data-m="open" class="active" disabled>Open</button>"#), "{html}");
        assert!(html.contains(r#"🔒 Set by <span class="mono">ENCORE_SIGNUP_MODE</span>"#), "{html}");
        let html = signup_mode_html(&json!({ "signup_mode": "invite", "_meta": { "signup_mode": { "source": "ui", "env": "ENCORE_SIGNUP_MODE" } } }));
        assert!(!html.contains("disabled") && !html.contains("🔒"), "{html}");
        assert!(html.contains(r#"<button data-m="invite" class="active" >Invite only</button>"#), "{html}");
    }
}
