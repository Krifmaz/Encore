//! The Lantern card of Settings → Connections (admins only): Lantern's address and API key with Save, Test and
//! Disconnect, and the one-time move of the download settings into Lantern, with its dry run ("Preview the
//! move") listing every setting it would carry, card by card, secrets masked.
//!
//! The server side is /api/admin/lantern (encore_requests::lantern): GET the state, PUT the address and key,
//! DELETE to disconnect, POST …/migrate with `dry_run`; Test is the kit's POST /api/system/test/lantern.
//! While Lantern 1.0 is not out the server says so (`api_ready: false`, `waiting`) and the move answers 501:
//! the card shows that line and the move's error, and nothing else changes.

use crate::api;
use crate::dom;
use crate::json::J;
use crate::settings::test_result;
use crate::ui::{confirm_dialog, esc, icon, toast};
use serde_json::{json, Value};
use web_sys::Element;

/// The card's HTML for the state `s` (GET /api/admin/lantern), drawn into `[data-lantern]`.
pub fn card_html(s: &Value) -> String {
    let connected = s.truthy("connected");
    let badge = if s.truthy("took_over") {
        r#"<span class="badge ok">handles downloads</span>"#
    } else if connected {
        r#"<span class="badge ok">connected</span>"#
    } else {
        ""
    };
    let locked = s.truthy("env_locked");
    let dis = if locked { " disabled" } else { "" };
    let key_hint = if s.truthy("key_set") { "Saved – type to replace" } else { "Lantern → Settings → API key" };
    let lock_line = if locked { r#"<div class="small muted">Set by LANTERN_URL and LANTERN_API_KEY in the environment.</div>"# } else { "" };
    let waiting = if s.truthy("api_ready") { String::new() } else { format!(r#"<div class="small muted">{}</div>"#, esc(&s.s("waiting"))) };
    let disconnect = if connected && !locked { r#"<button class="btn sm danger ghost" type="button" data-lantern-act="disconnect">Disconnect</button>"# } else { "" };
    let mover = if connected {
        format!(r#"<div class="stack" style="border-top:1px solid var(--border);padding-top:12px">
      <div class="small">Move Encore's torrent, Usenet and Soulseek settings into Lantern once. Encore keeps its own copy: disconnecting brings them back.</div>
      <div class="row wrap"><button class="btn sm" type="button" data-lantern-act="preview">{}Preview the move</button>
        <button class="btn sm primary" type="button" data-lantern-act="move">Move settings into Lantern</button></div>
      <div data-lantern-plan></div>
    </div>"#, icon("search"))
    } else {
        String::new()
    };
    format!(r#"<div class="card stack" data-server="lantern">
    <div class="row between"><h3>Lantern</h3>{badge}</div>
    <div class="small muted">Your own grabber for torrents, Usenet and Soulseek. Once it takes over, requests are searched and downloaded through it, and Encore still imports, tags and files what it finishes.</div>
    {waiting}
    <label class="field"><span>Address</span><input class="input" type="url" data-lantern-url placeholder="http://lantern:8080" value="{url}"{dis}></label>
    <label class="field"><span>API key</span><input class="input" type="password" data-lantern-key placeholder="{key_hint}" autocomplete="off"{dis}></label>
    {lock_line}
    <div class="row wrap"><button class="btn sm primary" type="button" data-lantern-act="save"{dis}>{check}Save</button>
      <button class="btn sm" type="button" data-lantern-act="test">Test connection</button>{disconnect}</div>
    <div data-lantern-out></div>
    {mover}
  </div>"#, url = esc(&s.s("url")), check = icon("check"))
}

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

/// Draw the card into `slot` (admins: the Connections page adds the slot only for them) and handle its buttons.
pub async fn mount(slot: Element) {
    let state = match api::get("/admin/lantern").await {
        Ok(s) => s,
        Err(e) => {
            dom::set_html(&slot, &format!(r#"<div class="card small muted">Lantern: {}</div>"#, esc(&e.message)));
            return;
        }
    };
    dom::set_html(&slot, &card_html(&state));
    let root = slot.clone();
    dom::on(&slot, "click", move |e| {
        let Some(b) = dom::closest(e, "[data-lantern-act]") else { return };
        let act = dom::data(&b, "lantern-act");
        let root = root.clone();
        dom::spawn(async move {
            dom::set_disabled(&b, true);
            act_on(&root, &act).await;
            dom::set_disabled(&b, false);
        });
    });
}

/// One of the card's buttons: `save`, `test`, `disconnect`, `preview` or `move`.
async fn act_on(root: &Element, act: &str) {
    let out = |sel: &str, html: &str| if let Some(o) = dom::qs_in(root, sel) { dom::set_html(&o, html) };
    match act {
        "save" => {
            let url = dom::value_of(root, "[data-lantern-url]");
            let key = dom::value_of(root, "[data-lantern-key]");
            let mut body = json!({ "url": url });
            if !key.is_empty() { body["api_key"] = json!(key); }
            match api::put("/admin/lantern", body).await {
                Ok(s) => {
                    toast("Lantern saved", "ok");
                    redraw(root, &s);
                }
                Err(e) => toast(&e.message, "err"),
            }
        }
        "test" => match api::post_empty("/system/test/lantern").await {
            Ok(r) => out("[data-lantern-out]", &test_result(&r)),
            Err(e) => out("[data-lantern-out]", &test_result(&json!({ "ok": false, "message": e.message }))),
        },
        "disconnect" => {
            if !confirm_dialog("Disconnect Lantern", "Encore searches and downloads with its own sources and settings again.", "Disconnect", true).await { return; }
            match api::del("/admin/lantern").await {
                Ok(s) => redraw(root, &s),
                Err(e) => toast(&e.message, "err"),
            }
        }
        "preview" => match api::post("/admin/lantern/migrate", json!({ "dry_run": true })).await {
            Ok(r) => out("[data-lantern-plan]", &plan_html(r.g("plan"))),
            Err(e) => toast(&e.message, "err"),
        },
        "move" => {
            if !confirm_dialog("Move settings into Lantern", "Lantern takes over torrents, Usenet and Soulseek, and their cards under Settings → Plugins are hidden. Encore keeps its own copy.", "Move", false).await { return; }
            match api::post("/admin/lantern/migrate", json!({ "dry_run": false })).await {
                Ok(r) => {
                    toast("Lantern handles downloads now", "ok");
                    redraw(root, r.g("state"));
                }
                Err(e) => toast(&e.message, "err"),
            }
        }
        _ => {}
    }
}

/// Draw the card again for the state `s` (after a save, a disconnect or the move).
fn redraw(root: &Element, s: &Value) {
    dom::set_html(root, &card_html(s));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_follows_the_state() {
        let off = card_html(&json!({ "url": "", "key_set": false, "connected": false, "took_over": false, "api_ready": false, "waiting": "Lantern 1.0 isn't out yet" }));
        assert!(off.contains("Lantern 1.0 isn&#39;t out yet"));
        assert!(!off.contains("data-lantern-act=\"move\"") && !off.contains("Disconnect"));
        let on = card_html(&json!({ "url": "http://lantern:8080", "key_set": true, "connected": true, "took_over": false, "api_ready": false, "waiting": "w" }));
        assert!(on.contains("value=\"http://lantern:8080\"") && on.contains("Saved – type to replace"));
        assert!(on.contains("data-lantern-act=\"preview\"") && on.contains("Disconnect") && on.contains(">connected<"));
        let locked = card_html(&json!({ "url": "http://l", "connected": true, "env_locked": true, "api_ready": true }));
        assert!(locked.contains("LANTERN_URL") && !locked.contains("Disconnect") && locked.contains(" disabled"));
        assert!(card_html(&json!({ "connected": true, "took_over": true })).contains("handles downloads"));
        // an address is escaped
        assert!(card_html(&json!({ "url": "http://x/\"><b>" })).contains("http://x/&quot;&gt;&lt;b&gt;"));
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
