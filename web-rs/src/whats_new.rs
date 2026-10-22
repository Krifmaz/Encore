//! What's new: the popup on the first start after the server was updated (GET /api/changelog/new; once shown,
//! POST /api/changelog/seen marks this version seen for the account, so the desktop app does not show it again),
//! and Settings → What's new, the full changelog with the switch that turns the popup off (`prefs.whats_new_popup`).
//! Hotfix builds share their release's notes, so they never bring the popup back on their own.

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{esc, modal, toast, ModalOpts};
use serde_json::{json, Value};
use web_sys::Element;

/// Where the popup's Full changelog goes.
pub const SETTINGS: &str = "/settings?s=whats-new";

/// After signing in: show the popup when something is new since this account last saw it.
pub fn start() {
    dom::spawn(async {
        let Ok(r) = api::get("/changelog/new").await else { return };
        if !r.truthy("show") { return; }
        popup(&r.a("releases"));
        let _ = api::post_empty("/changelog/seen").await;
    });
}

fn release_html(r: &Value) -> String {
    let summary = r.s("summary");
    let highlights: String = r.a("highlights").iter().map(|h| format!("<li>{}</li>", esc(h.as_str().unwrap_or("")))).collect();
    format!(r##"<div class="stack" style="gap:6px"><h3 style="margin:0">{}</h3>{}{}</div>"##,
        esc(&r.s("title")),
        if summary.is_empty() { String::new() } else { format!(r##"<p class="muted" style="margin:0">{}</p>"##, esc(&summary)) },
        if highlights.is_empty() { String::new() } else { format!(r##"<ul style="margin:0;padding-left:20px">{highlights}</ul>"##) })
}

fn popup(releases: &[Value]) {
    let body = format!(r##"<div class="stack" style="gap:18px">{}</div>"##, releases.iter().map(release_html).collect::<String>());
    let foot = r##"<label class="switch" style="margin-right:auto"><input type="checkbox" id="wn-on" checked><i></i><span>Show this after updates</span></label>
      <button class="btn" type="button" id="wn-full">Full changelog</button><button class="btn primary" type="button" data-close>Close</button>"##;
    let m = modal(ModalOpts::new("What's new", &body).footer(foot));
    if let Some(full) = m.qs("#wn-full") {
        let m2 = m.clone();
        dom::on(&full, "click", move |_| { m2.close(); app::navigate(SETTINGS); });
    }
    if let Some(sw) = m.qs("#wn-on") {
        let sw2 = sw.clone();
        dom::on(&sw, "change", move |_| save_popup(dom::checked(&sw2)));
    }
}

fn save_popup(on: bool) {
    dom::spawn(async move {
        match api::put("/auth/prefs", json!({ "whats_new_popup": on })).await {
            Ok(_) => toast(if on { "What's new will show after updates" } else { "What's new won't show after updates – it stays in Settings" }, "ok"),
            Err(e) => toast(&e.message, "err"),
        }
    });
}

/// One part of a release's notes as the server sends it (`k`: h, p, li, row).
fn block_html(b: &Value) -> String {
    match b.s("k").as_str() {
        "h" => format!(r##"<h3 style="margin:10px 0 0">{}</h3>"##, esc(&b.s("text"))),
        "p" => format!(r##"<p style="margin:0">{}</p>"##, esc(&b.s("text"))),
        "li" => {
            let lead = b.s("lead");
            format!(r##"<li>{}{}</li>"##, if lead.is_empty() { String::new() } else { format!("<strong>{}</strong> ", esc(&lead)) }, esc(&b.s("text")))
        }
        "row" => {
            let tag = if b.truthy("head") { "th" } else { "td" };
            format!("<tr>{}</tr>", b.a("cells").iter().map(|c| format!(r##"<{tag} style="text-align:left;padding:4px 10px 4px 0;vertical-align:top">{}</{tag}>"##, esc(c.as_str().unwrap_or("")))).collect::<String>())
        }
        _ => String::new(),
    }
}

/// A release's notes: bullets and table rows grouped into their lists and tables.
pub fn notes_html(blocks: &[Value]) -> String {
    let mut out = String::new();
    let mut open = "";
    for b in blocks {
        let want = match b.s("k").as_str() { "li" => "ul", "row" => "table", _ => "" };
        if want != open {
            if !open.is_empty() { out.push_str(&format!("</{open}>")); }
            match want {
                "ul" => out.push_str(r##"<ul style="margin:0;padding-left:20px" class="stack">"##),
                "table" => out.push_str(r##"<table class="small" style="border-collapse:collapse">"##),
                _ => {}
            }
            open = want;
        }
        out.push_str(&block_html(b));
    }
    if !open.is_empty() { out.push_str(&format!("</{open}>")); }
    out
}

/// Settings → What's new.
pub fn render(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let r = api::get("/changelog").await.map_err(|e| e.message)?;
        let releases = r.a("releases");
        let list: String = releases.iter().enumerate().map(|(i, x)| format!(r##"<details class="card settings-card" {open}>
          <summary class="row" style="justify-content:space-between"><h2 style="margin:0">{title}</h2><span class="row" style="gap:8px"><span class="badge">{ver}</span><span class="chev">{chev}</span></span></summary>
          <div class="stack" style="gap:10px;margin-top:12px">{notes}</div></details>"##,
            open = if i == 0 { "open" } else { "" }, title = esc(&x.s("title")), ver = esc(&x.s("version")),
            chev = { let c = crate::ui::icon("chevron"); if c.is_empty() { "▾".to_string() } else { c } }, notes = notes_html(&x.a("blocks")))).collect();
        dom::set_html(&el, &format!(r##"<div class="stack">
          <div class="card stack">
            <h2>What's new</h2>
            <p class="small muted" style="margin:0">This server runs Encore {ver}. After an update, a short list of what changed shows once on the website and in the desktop app.</p>
            <label class="switch"><input type="checkbox" id="wn-on" {on}><i></i><span>Show what's new after updates</span></label>
          </div>
          {list}
        </div>"##, ver = esc(&r.s("version")), on = if r.truthy("popup") { "checked" } else { "" },
            list = if releases.is_empty() { r##"<div class="card muted">No release notes in this build.</div>"##.to_string() } else { list }));
        if let Some(sw) = dom::qs_in(&el, "#wn-on") {
            let sw2 = sw.clone();
            dom::on(&sw, "change", move |_| save_popup(dom::checked(&sw2)));
        }
        Ok(())
    })
}
