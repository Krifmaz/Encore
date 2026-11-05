//! Settings → Invites: the sign-up mode, the invites table and New invite.
//!
//! A Leptos view ([`Invites`]) over two signals: the sign-up mode and the last list of invites (loaded again after
//! a delete or a new invite). New invite opens a dialog whose body is a Leptos view too ([`NewInvite`]).
use super::schema_form::{env_locked, env_note};
use super::users::{form_entries, js_plus, must};
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{truthy, J};
use crate::ui::{copy_text, empty_state, icon, modal, toast, ModalOpts};
use kit_leptos::html::{html, raw};
use leptos::prelude::*;
use serde_json::{json, Value};
use std::sync::Arc;
use wasm_bindgen::JsValue;
use web_sys::Element;

/// `new Date(ts * 1000).toLocaleDateString()`.
fn locale_date(ts: f64) -> String {
    js_sys::Date::new(&JsValue::from_f64(ts * 1000.0)).to_locale_date_string("default", &JsValue::UNDEFINED).into()
}

/// Whether an invite can no longer be used: every use taken, or past its end (`now` in seconds).
fn is_dead(i: &Value, now: f64) -> bool {
    i.f("uses") >= i.f("max_uses") || (i.truthy("expires_at") && i.f("expires_at") < now)
}

/// The sign-up link of invite `code` on the server at `base` (no trailing slash).
fn invite_link(base: &str, code: &str) -> String {
    format!("{base}/#/signup?invite={}", String::from(js_sys::encode_uri_component(code)))
}

/// The sign-up modes, as the server names them and as the page says them.
const MODES: [(&str, &str); 3] = [("invite", "Invite only"), ("open", "Open"), ("closed", "Closed")];

/// The invites as last loaded, with the time they were loaded at (seconds).
type Loaded = Option<(Vec<Value>, f64)>;

/// Loads the invites into `list`; a failure keeps the last list (the old page let it go unseen too).
async fn load(list: RwSignal<Loaded>) -> Result<(), String> {
    let rows = api::get("/admin/invites").await.map_err(|e| e.message)?;
    list.set(Some((rows.items(), dom::now_ms() / 1000.0)));
    Ok(())
}

/// The Invites page: the view, mounted once the settings and the invites have come (a failure is the page's
/// error).
pub fn render_invites(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let settings = api::get("/admin/settings").await.map_err(|e| e.message)?;
        let base = settings.st("base_url").map(str::to_string).unwrap_or_else(|| dom::location().origin().unwrap_or_default());
        let base = base.trim_end_matches('/').to_string();
        dom::set_html(&view, "");
        // the signals live as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let list = owner.with(|| RwSignal::new(None));
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Invites settings list base/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        // the table's box is drawn; the page is done once its first list has come
        must(&view, "#inv")?;
        load(list).await
    })
}

/// The page: its heading with New invite, the sign-up mode and the invites table.
#[component]
pub fn Invites(
    /// The answer of GET /api/admin/settings (`signup_mode`, and `_meta` for whether the environment sets it).
    settings: Value,
    /// The invites as last loaded (`None` until the first list has come).
    list: RwSignal<Loaded>,
    /// The server's address the links start with.
    base: String,
) -> impl IntoView {
    let meta = settings.o("_meta");
    let locked = env_locked(&meta, "signup_mode");
    let mode = RwSignal::new(settings.s("signup_mode"));
    let base = Arc::new(base);
    let mode_buttons = MODES
        .iter()
        .map(|&(k, l)| {
            // a mode the environment sets is locked: its buttons are disabled and do nothing
            let pick = move |_| {
                dom::spawn(async move {
                    if api::put("/admin/settings", json!({ "signup_mode": k })).await.is_err() {
                        return;
                    }
                    mode.set(k.to_string());
                    toast(&format!("Sign-ups: {l}"), "ok");
                });
            };
            view! { <button data-m=k class=move || if mode.get() == k { "active" } else { "" } disabled=locked on:click=pick>{l}</button> }
        })
        .collect_view();
    let b2 = base.clone();
    let new_invite = move |_| new_invite_dialog(list, b2.clone());
    let table = move || {
        let (rows, now) = list.get()?;
        if rows.is_empty() {
            return Some(raw(empty_state(&icon("ticket"), "No invites yet", "Create one and send the link to a friend.")).into_any());
        }
        let rows = rows.iter().map(|i| invite_row(i, now, list, base.clone())).collect_view();
        Some(view! {
            <div class="table-wrap"><table class="table"><thead><tr><th>"Code"</th><th>"Uses"</th><th class="hide-sm">"Role"</th><th class="hide-sm">"Expires"</th><th class="hide-sm">"Used by"</th><th></th></tr></thead><tbody>
                {rows}</tbody></table></div>
        }
        .into_any())
    };
    view! {
        <div class="row wrap between" style="margin-bottom:18px">
            <div><h1>"Invites"</h1><div class="small muted">"Share an invite link so friends can create an account."</div></div>
            <button class="btn primary" id="new" on:click={new_invite} {..html(format!("{}New invite", icon("plus")))}>{""}</button>
        </div>
        <div class="card row wrap between" style="margin-bottom:18px">
            <div><b>"Sign-up mode"</b><div class="small muted">"Who is allowed to create an account on the login page."</div>{raw(env_note(&meta, "signup_mode"))}</div>
            <div class="tabs" id="mode">{mode_buttons}</div>
        </div>
        <div class="card flat" id="inv">{table}</div>
    }
}

/// One row of the invites table: the code (struck through once it can't be used), its uses, role, end and who
/// used it, with Copy link (while it can be used) and delete.
fn invite_row(i: &Value, now: f64, list: RwSignal<Loaded>, base: Arc<String>) -> impl IntoView {
    let dead = is_dead(i, now);
    let code = i.s("code");
    let id = i.s("id");
    let note = i.truthy("note").then(|| view! { <div class="small faint">{i.s("note")}</div> });
    let role = if matches!(i.s("role").as_str(), "admin" | "owner") { view! { <span class="badge warn">"admin"</span> }.into_any() } else { "user".into_any() };
    let expires = if i.truthy("expires_at") {
        if i.f("expires_at") < now { "expired".to_string() } else { locale_date(i.f("expires_at")) }
    } else {
        "never".to_string()
    };
    let link = invite_link(&base, &code);
    let copy = (!dead).then(|| view! { <button class="btn sm" data-a="copy" on:click=move |_| copy_text(&link) {..html(format!("{}Copy link", icon("copy")))}>{""}</button> });
    let del = move |_| {
        let id = id.clone();
        dom::spawn(async move {
            if api::del(&format!("/admin/invites/{id}")).await.is_ok() {
                let _ = load(list).await;
            }
        });
    };
    view! {
        <tr data-id=i.s("id") data-code=code.clone()>
            <td><span class="mono" style=format!("font-weight:700;{}", if dead { "text-decoration:line-through;opacity:.5" } else { "" })>{code.clone()}</span>{note}</td>
            <td>{format!("{} / {}", i.s("uses"), i.s("max_uses"))}</td>
            <td class="hide-sm">{role}</td>
            <td class="hide-sm small muted">{expires}</td>
            <td class="hide-sm small muted">{i.st("used_by").unwrap_or("—").to_string()}</td>
            <td style="text-align:right;white-space:nowrap">{copy}
                <button class="btn sm danger ghost" data-a="del" on:click={del} {..html(icon("trash"))}>{""}</button></td></tr>
    }
}

/// New invite: a dialog with the invite's settings; Create invite makes it and shows its link to copy, and the
/// table is loaded again.
fn new_invite_dialog(list: RwSignal<Loaded>, base: Arc<String>) {
    let m = modal(ModalOpts::new("New invite", "").footer(r#"<button class="btn" data-close>Close</button><button class="btn primary" id="ok">Create invite</button>"#));
    // the body is the view over the code made (`None`: the form)
    let owner = Owner::new();
    let made: RwSignal<Option<String>> = owner.with(|| RwSignal::new(None));
    if let Some(body) = m.qs(".modal-body") {
        dom::set_html(&body, "");
        let b2 = base.clone();
        // kept while the page is on screen (the dialog closes before the page changes, or with it)
        if let Some(v) = owner.with(|| kit_leptos::rt::mount(&body, move || view! { <NewInvite made base=b2/> })) {
            dom::keep(v);
        }
    }
    dom::keep(owner);
    let Some(ok) = m.qs("#ok") else { return };
    dom::on(&ok, "click", move |ev| {
        let f = m.qs("#ni").map(|f| form_entries(&f)).unwrap_or_else(|| json!({}));
        let target = dom::target_el(ev);
        dom::spawn(async move {
            let max_uses = match js_plus(&f.s("max_uses")) {
                v if truthy(&v) => v,
                _ => Value::from(1),
            };
            let expires_days = match js_plus(&f.s("expires_days")) {
                v if truthy(&v) => v,
                _ => Value::Null,
            };
            let note = if f.truthy("note") { f.g("note").clone() } else { Value::Null };
            match api::post("/admin/invites", json!({ "max_uses": max_uses, "expires_days": expires_days, "role": f.g("role"), "note": note })).await {
                Ok(inv) => {
                    if let Some(t) = &target {
                        dom::add_class(t, "hidden");
                    }
                    made.set(Some(inv.s("code")));
                    let _ = load(list).await;
                }
                Err(e) => toast(&e.message, "err"),
            }
        });
    });
}

/// The New invite dialog's body: the form (hidden once the invite is made), then the link to send.
#[component]
fn NewInvite(
    /// The code of the invite made (`None` until then).
    made: RwSignal<Option<String>>,
    /// The server's address the link starts with.
    base: Arc<String>,
) -> impl IntoView {
    let done = move || made.get().map(|code| {
        let link = invite_link(&base, &code);
        let l2 = link.clone();
        view! {
            <p class="muted" style="margin-top:0">"Send this link – it signs them up with the invite filled in:"</p>
            <div class="code">{link}</div>
            <div class="row mt"><button class="btn primary" id="cp" on:click=move |_| copy_text(&l2) {..html(format!("{}Copy link", icon("copy")))}>{""}</button><span class="muted">"Code: "<b class="mono">{code}</b></span></div>
        }
    });
    view! {
        <form class=move || if made.get().is_some() { "stack hidden" } else { "stack" } id="ni">
            <div class="form-grid">
                <label class="field"><span>"Max uses"</span><input class="input" type="number" name="max_uses" value="1" min="1"/></label>
                <label class="field"><span>"Expires after (days)"</span><input class="input" type="number" name="expires_days" value="7" min="0" step="0.5"/><small>"0 = never"</small></label>
            </div>
            <label class="field"><span>"Role"</span><select class="input" name="role"><option value="user">"User"</option><option value="admin">"Admin"</option></select></label>
            <label class="field"><span>"Note"</span><input class="input" name="note" placeholder="e.g. for Sam"/></label>
        </form><div id="made">{done}</div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invite is dead once every use is taken or its end has passed; one without an end lives on.
    #[test]
    fn an_invite_is_dead_when_used_up_or_past_its_end() {
        assert!(is_dead(&json!({ "uses": 1, "max_uses": 1 }), 100.0));
        assert!(is_dead(&json!({ "uses": 0, "max_uses": 1, "expires_at": 50 }), 100.0));
        assert!(!is_dead(&json!({ "uses": 0, "max_uses": 2, "expires_at": 150 }), 100.0));
        assert!(!is_dead(&json!({ "uses": 0, "max_uses": 2, "expires_at": null }), 100.0));
    }
}
