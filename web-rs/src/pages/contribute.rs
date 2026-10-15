//! Port of web/js/pages/contribute.js – see docs/ui-rewrite/CONTRACT.md
//! Settings → Contribute: send AcoustID fingerprints and MusicBrainz ISRCs for songs the audio confirmed,
//! and how albums missing from MusicBrainz get added.
use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{esc, fmt, icon, toast};
use serde_json::{Map, Value};
use web_sys::Element;

const MASK: &str = "••••";

fn locked(meta: &Value, key: &str) -> bool { meta.o(key).s("source") == "env" }

fn secret_input(key: &str, label: &str, v: &Value, meta: &Value, help: &str) -> String {
    let l = locked(meta, key);
    let help = if help.is_empty() { String::new() } else { format!("<small>{help}</small>") };
    format!(r#"<label class="field"><span>{}</span><input class="input" type="password" data-k="{key}" data-secret="1" value="" {}
    autocomplete="new-password" placeholder="{}">{help}</label>"#,
        esc(label), if l { "disabled" } else { "" }, if l { "set by environment" } else if crate::json::truthy(v) { "saved – paste a new value to replace" } else { "not set" })
}

fn text_input(key: &str, label: &str, v: &Value, meta: &Value, help: &str) -> String {
    let l = locked(meta, key);
    let help = if help.is_empty() { String::new() } else { format!("<small>{help}</small>") };
    let val = if v.is_null() { String::new() } else { crate::json::js_string(v) };
    format!(r#"<label class="field"><span>{}</span><input class="input" type="text" data-k="{key}" value="{}" autocomplete="off" {}>{help}</label>"#,
        esc(label), esc(&val), if l { "disabled" } else { "" })
}

fn toggle(key: &str, label: &str, v: &Value, meta: &Value) -> String {
    let l = locked(meta, key);
    format!(r#"<div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" data-k="{key}" {} {}><i></i>{}</label></div>"#,
        if crate::json::truthy(v) { "checked" } else { "" }, if l { "disabled" } else { "" }, esc(label))
}

fn stat(n: f64, label: &str) -> String {
    format!(r#"<div class="stat"><b>{}</b><span>{}</span></div>"#, fmt::num(n), esc(label))
}

fn last_line(l: &Value) -> String {
    if !l.truthy("at") { return String::new(); }
    let ok = l.truthy("ok");
    format!(r#"<div class="small {}" style="{}">Last run {}: {}</div>"#, if ok { "muted" } else { "" }, if ok { "" } else { "color:var(--err)" },
        esc(&fmt::ago(l.f("at"))), esc(&l.s("message")))
}

fn rerender(view: Element, ctx: Ctx) {
    let mut ctx = ctx;
    ctx.query.clear();
    dom::spawn(async move { if let Err(e) = render(view, ctx).await { dom::console_error(&e); } });
}

async fn save(view: &Element, part: &str) -> Result<(), api::ApiError> {
    let mut body = Map::new();
    if let Some(root) = dom::qs_in(view, &format!(r#"[data-part="{part}"]"#)) {
        for el in dom::qsa_in(&root, "[data-k]") {
            if el.has_attribute("disabled") { continue; }
            let k = dom::data(&el, "k");
            if el.get_attribute("type").as_deref() == Some("checkbox") { body.insert(k, Value::Bool(dom::checked(&el))); continue; }
            let v = dom::value(&el);
            if !dom::data(&el, "secret").is_empty() && (v.trim().is_empty() || v.starts_with(MASK)) { continue; }
            body.insert(k, Value::String(v.trim().to_string()));
        }
    }
    api::put("/admin/settings", Value::Object(body)).await.map(|_| ())
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let (values, d) = futures_join(api::get("/admin/settings"), api::get("/contribute")).await;
        let values = values.map_err(|e| e.message)?;
        let d = d.map_err(|e| e.message)?;
        let meta = values.o("_meta");
        let (a, m) = (d.o("acoustid"), d.o("isrc"));
        let flash = if !ctx.q("error").is_empty() { format!(r#"<div class="test-result err">{}<span>{}</span></div>"#, icon("x"), esc(&ctx.q("error"))) }
            else if !ctx.q("mb").is_empty() { format!(r#"<div class="test-result ok">{}<span>MusicBrainz account connected</span></div>"#, icon("check")) }
            else { String::new() };
        let why = if a.truthy("why_not") && a.truthy("enabled") {
            format!(r#"<div class="small muted">{}{}</div>"#, esc(&a.s("why_not")), if a.truthy("can_fingerprint") { "" } else { ". fpcalc is not installed on this server" })
        } else { String::new() };
        let mb_buttons = if m.truthy("connected") {
            format!(r#"<button class="btn sm" data-run="isrc">{}Send now</button><button class="btn sm" data-mb="disconnect">Disconnect</button>"#, icon("refresh"))
        } else {
            format!(r#"<button class="btn sm" data-mb="connect" {}>{}Connect MusicBrainz</button>"#, if m.truthy("has_client") { "" } else { "disabled" }, icon("link"))
        };
        let callback = if m.truthy("redirect_uri") { format!(r#"<span class="mono">{}</span>"#, esc(&m.s("redirect_uri"))) }
            else { "&lt;Public URL&gt;/api/contribute/mb/callback (set the Public URL under General first)".to_string() };
        let account = if m.truthy("connected") { format!("Connected as <b>{}</b>", esc(&if m.truthy("account") { m.s("account") } else { "your account".to_string() })) } else { "Not connected".to_string() };
        let html = format!(r#"
    <div class="card stack" style="margin-bottom:16px">
      <h2>Contribute</h2>
      <p class="muted small" style="margin:0">Encore leans on AcoustID and MusicBrainz to know what a file is. Here it can give back: fingerprints of songs whose recording the audio confirmed, ISRCs MusicBrainz is missing, and albums that are only on Deezer or Apple Music. Only songs Encore is sure about are sent; nothing is sent until you add your own keys.</p>
      {flash}
    </div>
    <div class="card stack" data-part="acoustid" style="margin-bottom:16px">
      <div class="row between wrap" style="gap:8px"><h2>AcoustID fingerprints</h2><button class="btn sm" data-run="acoustid" {a_dis}>{refresh}Send now</button></div>
      <p class="muted small" style="margin:0">When a download's audio confirms the MusicBrainz recording it was asked for, its fingerprint is sent to AcoustID in batches, once per song. Your user key says who sent it; the API key under Apps &amp; sources → Verification is Encore's application key.</p>
      <div class="stats">{a_stats}</div>
      {why}
      {a_last}
      <div class="form-grid">
        {a_toggle}
        {a_key}
      </div>
      <div class="row" style="justify-content:flex-end"><button class="btn primary" data-save="acoustid">{check}Save</button></div>
    </div>
    <div class="card stack" data-part="isrc" style="margin-bottom:16px">
      <div class="row between wrap" style="gap:8px"><h2>MusicBrainz ISRCs</h2>
        <span class="row" style="gap:8px">{mb_buttons}</span></div>
      <p class="muted small" style="margin:0">Songs from Deezer, Tidal and Qobuz carry the ISRC of the recording. When the audio confirmed the MusicBrainz recording and MusicBrainz does not list that ISRC yet, Encore adds it under your MusicBrainz account. ISRCs that MusicBrainz has on another recording are left for a person to look at.</p>
      <div class="small">{account}</div>
      <div class="stats">{m_stats}</div>
      {m_last}
      <div class="form-grid">
        {m_toggle}
        {m_id}
        {m_secret}
      </div>
      <div class="row" style="justify-content:flex-end"><button class="btn primary" data-save="isrc">{check}Save</button></div>
    </div>
    <div class="card stack">
      <h2>Albums missing from MusicBrainz</h2>
      <p class="muted small" style="margin:0">Artist pages list albums found on Deezer and Apple Music that MusicBrainz does not have. Their Add to MusicBrainz link opens the MusicBrainz release editor with the title, artists, date, label, barcode, track list and store links filled in. Check it, then enter the edit under your own MusicBrainz account. Once the album is in MusicBrainz, Encore re-tags your copy on its own.</p>
    </div>"#,
            a_dis = if a.truthy("ready") { "" } else { "disabled" }, refresh = icon("refresh"), check = icon("check"),
            a_stats = stat(a.f("queued"), "Waiting to send") + &stat(a.f("submitted"), "Sent, being processed") + &stat(a.f("imported"), "In AcoustID") + &stat(a.f("failed"), "Failed"),
            a_last = last_line(a.g("last")),
            a_toggle = toggle("contribute_acoustid", "Send fingerprints of confirmed songs to AcoustID", values.g("contribute_acoustid"), &meta),
            a_key = secret_input("acoustid_user_key", "AcoustID user key", values.g("acoustid_user_key"), &meta, r#"Sign in at <a href="https://acoustid.org/new-application" target="_blank" rel="noopener">acoustid.org</a> and copy the user API key shown on your profile. It is not the application key: that one identifies Encore, this one identifies you."#),
            m_stats = stat(m.f("queued"), "Waiting to send") + &stat(m.f("submitted"), "Added to MusicBrainz") + &stat(m.f("skipped"), "Skipped") + &stat(m.f("failed"), "Failed"),
            m_last = last_line(m.g("last")),
            m_toggle = toggle("contribute_isrc", "Add missing ISRCs to MusicBrainz", values.g("contribute_isrc"), &meta),
            m_id = text_input("mb_client_id", "MusicBrainz application client ID", values.g("mb_client_id"), &meta, &format!(r#"Register an application at <a href="https://musicbrainz.org/account/applications" target="_blank" rel="noopener">musicbrainz.org/account/applications</a> (type: web application) with the callback URL {callback}."#)),
            m_secret = secret_input("mb_client_secret", "MusicBrainz application client secret", values.g("mb_client_secret"), &meta, ""),
        );
        dom::set_html(&view, &html);

        for b in dom::qsa_in(&view, "[data-save]") {
            let (view2, ctx2, part) = (view.clone(), ctx.clone(), dom::data(&b, "save"));
            dom::on(&b, "click", move |_| {
                let (view, ctx, part) = (view2.clone(), ctx2.clone(), part.clone());
                dom::spawn(async move {
                    match save(&view, &part).await {
                        Ok(()) => { toast("Settings saved", "ok"); rerender(view, ctx); }
                        Err(e) => toast(&e.message, "err"),
                    }
                });
            });
        }
        for b in dom::qsa_in(&view, "[data-run]") {
            let (view2, ctx2, b2) = (view.clone(), ctx.clone(), b.clone());
            dom::on(&b, "click", move |_| {
                dom::set_disabled(&b2, true);
                let (view, ctx, what) = (view2.clone(), ctx2.clone(), dom::data(&b2, "run"));
                dom::spawn(async move {
                    match api::post_empty(&format!("/contribute/{what}/run")).await {
                        Ok(r) => toast(&if r.truthy("message") { r.s("message") } else { "Done".to_string() }, "ok"),
                        Err(e) => toast(&e.message, "err"),
                    }
                    rerender(view, ctx);
                });
            });
        }
        for b in dom::qsa_in(&view, "[data-mb]") {
            let (view2, ctx2, b2) = (view.clone(), ctx.clone(), b.clone());
            dom::on(&b, "click", move |_| {
                dom::set_disabled(&b2, true);
                let (view, ctx, what) = (view2.clone(), ctx2.clone(), dom::data(&b2, "mb"));
                dom::spawn(async move {
                    let r: Result<(), api::ApiError> = async {
                        if what == "connect" {
                            save(&view, "isrc").await?;
                            let r = api::post_empty("/contribute/mb/connect").await?;
                            let _ = dom::location().set_href(&r.s("url"));
                            return Ok(());
                        }
                        api::post_empty("/contribute/mb/disconnect").await?;
                        toast("Disconnected", "ok");
                        Ok(())
                    }.await;
                    if what == "connect" && r.is_ok() { return; }
                    if let Err(e) = r { toast(&e.message, "err"); }
                    rerender(view, ctx);
                });
            });
        }
        Ok(())
    })
}

/// `Promise.all([a, b])`.
async fn futures_join<A: std::future::Future, B: std::future::Future>(a: A, b: B) -> (A::Output, B::Output) {
    let a = a.await;
    let b = b.await;
    (a, b)
}
