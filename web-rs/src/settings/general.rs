//! Port of web/js/pages/settings.js (the General page's own cards) – see docs/archive/ui-rewrite/CONTRACT.md
//! Settings → General ends with two cards of its own under the schema's card: Encore's API key (only the owner
//! gets it) and About Encore (also at the bottom of Account for members).

use crate::api;
use crate::app;
use crate::dom;
use crate::json::{js_string, truthy, J};
use crate::ui::{copy_text, esc, icon, set_secret_text, toast};
use web_sys::Element;

/// Encore's own API key, at the bottom of General (only the owner may see it): copy and regenerate.
pub async fn api_card(panel: &Element) -> String {
    let key = match api::get("/system/api-key").await { Ok(r) => r.s("api_key"), Err(_) => return String::new() };
    let panel = panel.clone();
    dom::timeout(0, move || {
        let Some(card) = dom::qs_in(&panel, "#api-card") else { return };
        if let Some(cp) = dom::qs_in(&card, "#cp") {
            let card2 = card.clone();
            dom::on(&cp, "click", move |_| { if let Some(k) = dom::qs_in(&card2, "#key") { copy_text(&dom::data(&k, "value")); } });
        }
        if let Some(regen) = dom::qs_in(&card, "#regen") {
            let card2 = card.clone();
            dom::on(&regen, "click", move |_| {
                if !dom::confirm("Make a new API key? Scripts using the old one stop working.") { return; }
                let card = card2.clone();
                dom::spawn(async move {
                    match api::post_empty("/admin/settings/api-key").await {
                        Ok(r) => {
                            if let Some(k) = dom::qs_in(&card, "#key") { set_secret_text(&k, &r.s("api_key")); }
                            toast("New API key generated", "ok");
                        }
                        Err(e) => dom::console_error(&e.message),
                    }
                });
            });
        }
    });
    format!(r##"<div class="card stack mt" id="api-card">
      <h2>Encore API key</h2>
      <p class="muted small" style="margin:0">Send it in an <span class="kbd">X-Api-Key</span> header to automate Encore (scripts, Home Assistant…). It acts as the owner account – keep it private.</p>
      <div class="row wrap"><div class="code grow" id="key" data-secret data-value="{}"></div><button class="btn" id="cp" title="Copy">{}</button><button class="btn danger" id="regen">{}Regenerate</button></div>
    </div>"##, esc(&key), icon("copy"), icon("refresh"))
}

/// Version, build and app downloads – shown at the bottom of General (and of Account for members).
pub async fn about_card() -> String {
    let info = app::info();
    let server = "Encore";
    let version = { let v = info.g("version"); if truthy(v) { js_string(v) } else { "?".to_string() } };
    let build = { let v = info.g("build"); if truthy(v) { js_string(v) } else { "dev".to_string() } };
    format!(r##"<div class="card stack mt"><h2>About Encore</h2>
    <div class="row wrap" style="gap:24px"><div><div class="small muted">Version</div><div style="font-weight:700;font-size:20px">v{}</div></div>
      <div><div class="small muted">Server</div><div>{}</div></div>
      <div><div class="small muted">Build</div><div class="mono">{}</div></div></div>
    <div class="small muted">Apps for Android, Windows, macOS and Linux: <a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">github.com/Krifmaz/Encore/releases</a></div></div>"##,
        esc(&version), esc(server), esc(&build))
}
