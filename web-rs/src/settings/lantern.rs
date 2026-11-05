//! The Lantern card of Settings → Connections (admins only): Lantern's address and API key with Save, Test and
//! Disconnect, and the one-time move of the download settings into Lantern, with its dry run ("Preview the
//! move") listing every setting it would carry, card by card, secrets masked.
//!
//! The server side is /api/admin/lantern (encore_requests::lantern): GET the state, PUT the address and key,
//! DELETE to disconnect, POST …/migrate with `dry_run`; Test is the kit's POST /api/system/test/lantern.
//! While Lantern 1.0 is not out the server says so (`api_ready: false`, `waiting`) and the move answers 501:
//! the card shows that line and the move's error, and nothing else changes.
//!
//! A Leptos view ([`Lantern`]) over the state; a save, a disconnect or the move draws the card anew from the
//! state the server answers, as before.

use crate::api;
use crate::dom;
use crate::json::J;
use crate::settings::test_result;
use crate::ui::{confirm_dialog, esc, icon, toast};
use kit_leptos::html::html;
use leptos::prelude::*;
use serde_json::{json, Value};
use web_sys::Element;

/// The dry run's answer (`plan`: `items` and `skipped`) as a list, one heading per part ("Indexers
/// (Prowlarr)", "Torrent client", "Usenet client", "Soulseek"), in the order the server sends.
pub fn plan_html(plan: &Value) -> String {
    let items = plan.a("items");
    let skipped = plan.a("skipped");
    if items.is_empty() {
        let why: String = skipped.iter().map(|s| format!("<li>{}</li>", esc(&crate::json::js_string(s)))).collect();
        return format!(r#"<div class="small muted">Nothing to move.</div><ul class="small muted">{why}</ul>"#);
    }
    let mut html = String::from(r#"<div class="small">The move would carry:</div>"#);
    let mut part = String::new();
    for it in &items {
        if it.s("part") != part {
            if !part.is_empty() { html.push_str("</ul>"); }
            part = it.s("part");
            html.push_str(&format!(r#"<div class="small" style="margin-top:8px"><b>{}</b></div><ul class="small">"#, esc(&part)));
        }
        let value = if it.truthy("secret") {
            if it.s("value").is_empty() { "not set".to_string() } else { "set (hidden)".to_string() }
        } else {
            let v = it.s("value");
            if v.is_empty() { "empty".to_string() } else { v }
        };
        let default = if it.truthy("default") { r#" <span class="faint">(default)</span>"# } else { "" };
        html.push_str(&format!(r#"<li>{}: <span class="mono">{}</span>{default}</li>"#, esc(&it.s("label")), esc(&value)));
    }
    html.push_str("</ul>");
    for s in &skipped {
        html.push_str(&format!(r#"<div class="small muted">{}</div>"#, esc(&crate::json::js_string(s))));
    }
    html
}

/// Draw the card into `slot` (admins: the Connections page adds the slot only for them).
pub async fn mount(slot: Element) {
    let state = match api::get("/admin/lantern").await {
        Ok(s) => s,
        Err(e) => {
            dom::set_html(&slot, &format!(r#"<div class="card small muted">Lantern: {}</div>"#, esc(&e.message)));
            return;
        }
    };
    // the state lives as long as the page: the owner is dropped with it
    let owner = Owner::new();
    let state = owner.with(|| RwSignal::new(state));
    if let Some(m) = owner.with(|| kit_leptos::rt::mount(&slot, move || view! { <Lantern state/> })) {
        dom::keep(m);
    }
    dom::keep(owner);
}

/// The card's badge for the state: whether Lantern handles downloads, or is only connected.
fn badge(s: &Value) -> Option<&'static str> {
    if s.truthy("took_over") {
        Some("handles downloads")
    } else if s.truthy("connected") {
        Some("connected")
    } else {
        None
    }
}

/// The Lantern card for the state (GET /api/admin/lantern), drawn anew when a button changes it.
#[component]
pub fn Lantern(
    /// Lantern's state: `url`, `key_set`, `connected`, `took_over`, `env_locked`, `api_ready`, `waiting`.
    state: RwSignal<Value>,
) -> impl IntoView {
    // what the boxes hold, Test's answer and the dry run
    let url = RwSignal::new(state.with_untracked(|s| s.s("url")));
    let key = RwSignal::new(String::new());
    let out = RwSignal::new(String::new());
    let plan = RwSignal::new(String::new());
    // a new state draws the card anew, as before: the boxes hold what it says, the answers are gone
    let redraw = move |s: Value| {
        url.try_set(s.s("url"));
        key.try_set(String::new());
        out.try_set(String::new());
        plan.try_set(String::new());
        state.try_set(s);
    };
    move || {
        let s = state.get();
        let connected = s.truthy("connected");
        let locked = s.truthy("env_locked");
        let key_hint = if s.truthy("key_set") { "Saved – type to replace" } else { "Lantern → Settings → API key" };
        let waiting = (!s.truthy("api_ready")).then(|| view! { <div class="small muted">{s.s("waiting")}</div> });
        let lock_line = locked.then(|| view! { <div class="small muted">"Set by LANTERN_URL and LANTERN_API_KEY in the environment."</div> });
        let save = move || {
            let mut body = json!({ "url": url.get_untracked() });
            let k = key.get_untracked();
            if !k.is_empty() {
                body["api_key"] = json!(k);
            }
            async move {
                match api::put("/admin/lantern", body).await {
                    Ok(s) => {
                        toast("Lantern saved", "ok");
                        redraw(s);
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            }
        };
        let test = move || async move {
            let r = match api::post_empty("/system/test/lantern").await {
                Ok(r) => r,
                Err(e) => json!({ "ok": false, "message": e.message }),
            };
            out.try_set(test_result(&r));
        };
        let disconnect = move || async move {
            if !confirm_dialog("Disconnect Lantern", "Encore searches and downloads with its own sources and settings again.", "Disconnect", true).await {
                return;
            }
            match api::del("/admin/lantern").await {
                Ok(s) => redraw(s),
                Err(e) => toast(&e.message, "err"),
            }
        };
        let preview = move || async move {
            match api::post("/admin/lantern/migrate", json!({ "dry_run": true })).await {
                Ok(r) => {
                    plan.try_set(plan_html(r.g("plan")));
                }
                Err(e) => toast(&e.message, "err"),
            }
        };
        let take_over = move || async move {
            if !confirm_dialog("Move settings into Lantern", "Lantern takes over torrents, Usenet and Soulseek, and their cards under Settings → Plugins are hidden. Encore keeps its own copy.", "Move", false).await {
                return;
            }
            match api::post("/admin/lantern/migrate", json!({ "dry_run": false })).await {
                Ok(r) => {
                    toast("Lantern handles downloads now", "ok");
                    redraw(r.g("state").clone());
                }
                Err(e) => toast(&e.message, "err"),
            }
        };
        let disconnect = (connected && !locked).then(|| view! { <ActButton class="btn sm danger ghost" act="disconnect" label="Disconnect".to_string() run=disconnect/> });
        let mover = connected.then(|| view! {
            <div class="stack" style="border-top:1px solid var(--border);padding-top:12px">
                <div class="small">"Move Encore's torrent, Usenet and Soulseek settings into Lantern once. Encore keeps its own copy: disconnecting brings them back."</div>
                <div class="row wrap"><ActButton class="btn sm" act="preview" label=format!("{}Preview the move", icon("search")) run=preview/>
                    <ActButton class="btn sm primary" act="move" label="Move settings into Lantern".to_string() run=take_over/></div>
                <div data-lantern-plan inner_html=move || plan.get()></div>
            </div>
        });
        view! {
            <div class="card stack" data-server="lantern">
                <div class="row between"><h3>"Lantern"</h3>{badge(&s).map(|b| view! { <span class="badge ok">{b}</span> })}</div>
                <div class="small muted">"Your own grabber for torrents, Usenet and Soulseek. Once it takes over, requests are searched and downloaded through it, and Encore still imports, tags and files what it finishes."</div>
                {waiting}
                <label class="field"><span>"Address"</span><input class="input" type="url" data-lantern-url placeholder="http://lantern:8080" value=s.s("url") prop:value=move || url.get() disabled=locked
                    on:input=move |e| url.set(event_target_value(&e))/></label>
                <label class="field"><span>"API key"</span><input class="input" type="password" data-lantern-key placeholder=key_hint autocomplete="off" prop:value=move || key.get() disabled=locked
                    on:input=move |e| key.set(event_target_value(&e))/></label>
                {lock_line}
                <div class="row wrap"><ActButton class="btn sm primary" act="save" label=format!("{}Save", icon("check")) run=save locked/>
                    <ActButton class="btn sm" act="test" label="Test connection".to_string() run=test/>{disconnect}</div>
                <div data-lantern-out inner_html=move || out.get()></div>
                {mover}
            </div>
        }
    }
}

/// One of the card's buttons (`data-lantern-act`): disabled while `run` works (and for good when `locked`).
#[component]
fn ActButton<F, Fut>(
    /// The button's classes.
    class: &'static str,
    /// What it does: `save`, `test`, `disconnect`, `preview` or `move`.
    act: &'static str,
    /// Its label, as HTML (an icon may lead).
    label: String,
    /// The work it starts.
    run: F,
    /// Whether the environment holds the settings, so it is never enabled.
    #[prop(optional)]
    locked: bool,
) -> impl IntoView
where
    F: Fn() -> Fut + Copy + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let busy = RwSignal::new(false);
    let click = move |_| {
        busy.set(true);
        let work = run();
        dom::spawn(async move {
            work.await;
            busy.try_set(false);
        });
    };
    view! { <button class=class type="button" data-lantern-act=act disabled=move || locked || busy.get() on:click={click} {..html(label)}>{""}</button> }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The badge says whether Lantern handles downloads or is only connected.
    #[test]
    fn the_badge_follows_the_state() {
        assert_eq!(badge(&json!({ "connected": false })), None);
        assert_eq!(badge(&json!({ "connected": true })), Some("connected"));
        assert_eq!(badge(&json!({ "connected": true, "took_over": true })), Some("handles downloads"));
    }

    #[test]
    fn the_preview_lists_each_part_and_hides_secrets() {
        let plan = json!({ "items": [
            { "part": "Indexers (Prowlarr)", "key": "prowlarr_url", "label": "Prowlarr URL", "value": "http://prowlarr:9696", "secret": false, "default": false },
            { "part": "Indexers (Prowlarr)", "key": "prowlarr_api_key", "label": "Prowlarr API key", "value": "••••••••", "secret": true, "default": false },
            { "part": "Torrent client", "key": "torrent_client", "label": "Torrent client", "value": "qbittorrent", "secret": false, "default": true },
            { "part": "Soulseek", "key": "soulseek_password", "label": "Soulseek password", "value": "", "secret": true, "default": true },
        ], "skipped": [] });
        let html = plan_html(&plan);
        assert_eq!(html.matches("<b>").count(), 3, "{html}");
        assert!(html.contains("Prowlarr API key: <span class=\"mono\">set (hidden)</span>"));
        assert!(html.contains("Soulseek password: <span class=\"mono\">not set</span>"));
        assert!(html.contains("qbittorrent</span> <span class=\"faint\">(default)</span>"));
        assert!(!html.contains("••"));
        let none = plan_html(&json!({ "items": [], "skipped": ["Soulseek is switched off: nothing to move"] }));
        assert!(none.contains("Nothing to move") && none.contains("Soulseek is switched off"));
    }
}
