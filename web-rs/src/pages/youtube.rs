//! Port of web/js/pages/youtube.js – Settings → YouTube sign-in: which cookies yt-dlp uses (a sign-in
//! handed over by the Encore browser extension, the extension's sync of an everyday browser, or pasted
//! cookies), Refresh now / Sign out, and the extension token. Same words and DOM as the JS.

use crate::api;
use crate::dom;
use crate::json::J;
use crate::ui::{copy_text, esc, icon, toast};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use web_sys::Element;

fn js_round(x: f64) -> i64 { (x + 0.5).floor() as i64 }
fn ago(t: f64) -> String {
    if t == 0.0 { return "never".into(); }
    let s = dom::now_s() - t;
    if s < 90.0 { "just now".into() } else if s < 5400.0 { format!("{} min ago", js_round(s / 60.0)) } else if s < 129600.0 { format!("{} h ago", js_round(s / 3600.0)) } else { format!("{} days ago", js_round(s / 86400.0)) }
}
fn until(t: f64) -> String {
    if t == 0.0 { return String::new(); }
    let s = t - dom::now_s();
    if s <= 60.0 { "any moment".into() } else if s < 5400.0 { format!("in {} min", js_round(s / 60.0)) } else if s < 129600.0 { format!("in {} h", js_round(s / 3600.0)) } else { format!("in {} days", js_round(s / 86400.0)) }
}

/// The card's frame; the status inside is filled in by `bind_youtube`.
pub fn youtube_block() -> String {
    format!(r##"<div class="card flat" id="yt-auth" style="padding:12px 14px;margin-bottom:14px">
    <div data-yt-status><div class="small muted">Loading…</div></div>
    <div class="row wrap mt" style="gap:8px">
      <button class="btn sm primary" type="button" data-yt="extension">{link}Connect the browser extension</button>
      <button class="btn sm" type="button" data-yt="refresh">{refresh}Refresh now</button>
      <button class="btn sm danger" type="button" data-yt="signout">{x}Sign out</button>
    </div>
    <div data-yt-ext hidden></div>
  </div>"##, refresh = icon("refresh"), link = icon("link"), x = icon("x"))
}

fn source_words(key: &str) -> &'static str {
    match key { "handoff" => "Handed-over sign-in", "extension" => "Everyday browser (extension)", _ => "Pasted cookies" }
}

fn source_line(key: &str, s: &Value) -> String {
    let state = if !s.truthy("has_cookies") {
        if key == "pasted" { "none".to_string() } else { "not connected".to_string() }
    } else if s.truthy("signed_in") {
        format!("signed in{}", if s.truthy("account") { format!(" as {}", s.s("account")) } else { String::new() })
    } else {
        "signed out".to_string()
    };
    format!("<span>{}: {}</span>", esc(source_words(key)), esc(&state))
}

/// The status part of the card (from GET /api/youtube/auth).
pub fn youtube_status_html(r: &Value) -> String {
    let source = r.s("source");
    let has_source = r.truthy("source");
    let sources = r.o("sources");
    let ok = has_source && sources.o(&source).truthy("signed_in");
    let badge = if !has_source { r##"<span class="badge">No cookies</span>"##.to_string() } else if ok { format!(r##"<span class="badge ok">{} Signed in</span>"##, icon("check")) } else { r##"<span class="badge warn">Signed out</span>"##.to_string() };
    let next = if r.truthy("next_refresh") { r.f("next_refresh") } else { 0.0 };
    let when = if has_source {
        format!("Last refresh {}{}", ago(r.f("last_refresh")), if next != 0.0 { format!(" · next refresh {}", until(next)) } else { String::new() })
    } else { String::new() };
    let label = if r.truthy("source_label") { r.s("source_label") } else { "Not signed in".to_string() };
    format!(r##"<div class="row between wrap" style="gap:8px;align-items:flex-start">
      <div><div style="font-weight:650">{label}</div>{account}</div>{badge}</div>
    {when}
    <div class="small muted row wrap" style="gap:4px 14px;margin-top:6px">{lines}</div>
    <div class="small faint" style="margin-top:6px">{diag}</div>"##,
        label = esc(&label),
        account = if r.truthy("account") { format!(r##"<div class="small muted">Account: {}</div>"##, esc(&r.s("account"))) } else { String::new() },
        when = if when.is_empty() { String::new() } else { format!(r##"<div class="small muted" style="margin-top:6px">{}</div>"##, esc(&when)) },
        lines = ["handoff", "extension", "pasted"].iter().map(|k| source_line(k, &sources.o(k))).collect::<String>(),
        diag = esc(&r.s("diagnostics")))
}

/// The token box shown after "Connect the browser extension".
pub fn extension_html(token: &str) -> String {
    format!(r##"<div class="card flat mt" style="padding:12px 14px">
    <div style="font-weight:650">Connect the Encore extension</div>
    <ol class="small muted" style="margin:6px 0 10px;padding-left:18px">
      <li>Install it on a computer: download Encore-youtube-extension-&lt;version&gt;.zip from the releases page and unzip it. Chrome, Edge or Brave: Extensions → Developer mode → Load unpacked → the chrome folder, then Details → Allow in Incognito. Firefox: about:debugging → Load Temporary Add-on → firefox/manifest.json, then about:addons → Run in Private Windows → Allow.</li>
      <li>Open its options and paste this Encore’s address and the token below.</li>
      <li>Press <b>Hand over a YouTube sign-in</b> in the extension. A private window opens: sign in to Google there. Once YouTube Music shows you signed in, the extension sends the sign-in to Encore and closes the window without signing out.</li>
    </ol>
    <div class="row wrap" style="gap:8px"><div class="code grow mono" data-yt-token>{token}</div><button class="btn sm" type="button" data-yt-copy>{copy}Copy</button></div>
    <div class="small faint" style="margin-top:6px">Shown once. It can only send YouTube cookies to Encore; making a new one stops the old one. After the hand-off Encore owns that sign-in and refreshes it itself every 4 hours, also while your computer is off.</div>
  </div>"##, token = esc(token), copy = icon("copy"))
}

struct Card {
    status: Element,
    ext_box: Element,
    info: RefCell<Option<Value>>,
}

impl Card {
    fn paint(&self, r: &Value) {
        dom::set_html(&self.status, &youtube_status_html(r));
        *self.info.borrow_mut() = Some(r.clone());
    }
}

fn load(card: Rc<Card>) {
    dom::spawn(async move {
        match api::get("/youtube/auth").await {
            Ok(r) => card.paint(&r),
            Err(e) => dom::set_html(&card.status, &format!(r##"<div class="small" style="color:var(--err)">{}</div>"##, esc(&e.message))),
        }
    });
}

/// Wire the card: status, buttons and the extension token.
pub fn bind_youtube(root: &Element) {
    let Some(host) = dom::qs_in(root, "#yt-auth") else { return };
    let (Some(status), Some(ext_box)) = (dom::qs_in(&host, "[data-yt-status]"), dom::qs_in(&host, "[data-yt-ext]")) else { return };
    let card = Rc::new(Card { status, ext_box, info: RefCell::new(None) });
    {
        let card = card.clone();
        dom::on_each(&host, "[data-yt]", "click", move |b, e| {
            dom::prevent(e);
            let a = dom::data(b, "yt");
            let (card, b) = (card.clone(), b.clone());
            match a.as_str() {
                "refresh" => dom::spawn(async move {
                    dom::set_disabled(&b, true);
                    match api::post_empty("/youtube/refresh").await {
                        Ok(r) => {
                            card.paint(&r);
                            let ok = r.truthy("refreshed");
                            // `rotated` 0: Google answered without new cookies – the sign-in was checked, not renewed
                            let nothing_renewed = r.g("rotated").as_u64() == Some(0);
                            // still signed in, but the renewal itself failed: the server's own words
                            let trouble = r.o("sources").o(&r.s("source")).s("message");
                            toast(if !ok { "YouTube signed out" } else if !trouble.is_empty() { trouble.as_str() } else if nothing_renewed { "Still signed in – Google sent no renewed cookies, so nothing was refreshed" } else { "Refreshed – still signed in" },
                                if !ok { "err" } else if nothing_renewed || !trouble.is_empty() { "warn" } else { "ok" });
                        }
                        Err(err) => toast(&err.message, "err"),
                    }
                    dom::set_disabled(&b, false);
                }),
                "signout" => {
                    if !dom::confirm("Forget the handed-over sign-in and the extension’s cookies? Pasted cookies stay (clear them below).") { return; }
                    dom::spawn(async move {
                        match api::post("/youtube/signout", json!({ "source": "all" })).await {
                            Ok(r) => { card.paint(&r); toast("Signed out", "ok"); }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                }
                "extension" => {
                    let had = card.info.borrow().as_ref().map(|i| i.o("extension").truthy("token_at")).unwrap_or(false);
                    if had && !dom::confirm("Make a new token? The extension stops working until you enter the new one in its options.") { return; }
                    dom::spawn(async move {
                        match api::post_empty("/youtube/extension-token").await {
                            Ok(r) => {
                                let token = r.s("token");
                                dom::hide(&card.ext_box, false);
                                dom::set_html(&card.ext_box, &extension_html(&token));
                                if let Some(c) = dom::qs_in(&card.ext_box, "[data-yt-copy]") {
                                    dom::on(&c, "click", move |_| copy_text(&token));
                                }
                                load(card.clone());
                            }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                }
                _ => {}
            }
        });
    }
    load(card);
}
