//! Port of web/js/pages/translate.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Translated titles: Settings → Language and ⋯ → Edit translation.

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{esc, icon, modal, set_translate_mode, toast, ModalOpts};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use web_sys::Element;

fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

fn kind_label(kind: &str) -> &'static str {
    match kind { "album" => "Album title", "artist" => "Artist name", _ => "Song title" }
}

/// `editTranslation(text, kind, onSaved)`: correct the translation and romanization of one title
/// (shared with everyone who reads the same language).
pub fn edit_translation(text: String, kind: &'static str, on_saved: Option<Rc<dyn Fn()>>) {
    dom::spawn(async move {
        let e = match api::get(&format!("/translate/entry?text={}", enc(&text))).await {
            Ok(e) => e,
            Err(err) => { toast(&err.message, "err"); return; }
        };
        let source = match e.s("source").as_str() {
            "official" => "Official (MusicBrainz or Apple)",
            "model" => "The Translator on this server",
            "manual" => "Corrected by hand",
            _ => "Not translated yet",
        };
        let body = format!(r#"<div class="stack" style="gap:12px">
      <div><div class="small muted">{}</div><div style="font-weight:650">{}</div></div>
      <label class="field"><span>Translation</span><input class="input" id="tr-t" value="{}" placeholder="Leave empty to show none"></label>
      <label class="field"><span>Romanization</span><input class="input" id="tr-r" value="{}" placeholder="Leave empty to show none"></label>
      <div class="small muted">Now from: {}. Your correction is kept for everyone who reads this language and is never replaced by the automatic translation.</div>
    </div>"#, esc(kind_label(kind)), esc(&e.s("text")), esc(&e.s("translation")), esc(&e.s("romanization")), esc(source));
        let footer = format!(r#"{}<button class="btn" data-close>Cancel</button><button class="btn primary" id="tr-save">{}Save</button>"#,
            if e.truthy("manual") { r#"<button class="btn" id="tr-reset">Use the automatic translation</button>"# } else { "" }, icon("check"));
        let m = modal(ModalOpts::new("Edit translation", &body).footer(&footer));
        let original = e.s("text");
        let done = {
            let (m, on_saved) = (m.clone(), on_saved.clone());
            Rc::new(move |msg: &str| { m.close(); toast(msg, "ok"); if let Some(f) = &on_saved { f(); } })
        };
        if let Some(b) = m.qs("#tr-save") {
            let (m2, done, original) = (m.clone(), done.clone(), original.clone());
            dom::on(&b, "click", move |_| {
                let body = json!({
                    "text": original, "kind": kind,
                    "translation": m2.qs("#tr-t").map(|x| dom::value(&x)).unwrap_or_default().trim(),
                    "romanization": m2.qs("#tr-r").map(|x| dom::value(&x)).unwrap_or_default().trim(),
                });
                let done = done.clone();
                dom::spawn(async move {
                    match api::put("/translate/entry", body).await {
                        Ok(_) => done("Translation saved"),
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
        if let Some(b) = m.qs("#tr-reset") {
            let (done, original) = (done.clone(), original.clone());
            dom::on(&b, "click", move |_| {
                let (done, original) = (done.clone(), original.clone());
                dom::spawn(async move {
                    match api::del(&format!("/translate/entry?text={}", enc(&original))).await {
                        Ok(_) => done("Back to the automatic translation"),
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
    });
}

struct Lang {
    el: Element,
    opts: Value,
    status: Value,
    mode: RefCell<String>,
    lang: RefCell<String>,
}

/// `renderLanguage(el)`: Settings → Language – how translated titles show, and into which language.
pub fn render_language(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let opts = match api::get("/translate/languages").await {
            Ok(o) => o,
            Err(err) => { dom::set_html(&el, &format!(r#"<div class="error-box">{}</div>"#, esc(&err.message))); return Ok(()); }
        };
        let status = api::get("/translate/status").await.unwrap_or(Value::Null);
        let prefs = app::user().map(|u| u.o("prefs")).unwrap_or(Value::Null);
        let mode = prefs.st("translate_mode").unwrap_or("off").to_string();
        let lang = prefs.st("translate_lang").map(String::from).or_else(|| opts.st("default").map(String::from)).unwrap_or_else(|| "en".into());
        let st = Rc::new(Lang { el, opts, status, mode: RefCell::new(mode), lang: RefCell::new(lang) });
        paint(&st);
        Ok(())
    })
}

fn paint(st: &Rc<Lang>) {
    let mode = st.mode.borrow().clone();
    let lang = st.lang.borrow().clone();
    let modes: String = st.opts.a("modes").iter().map(|x| format!(r#"<button type="button" data-v="{}" class="{}">{}</button>"#,
        esc(&x.s("id")), if mode == x.s("id") { "on" } else { "" }, esc(&x.s("label")))).collect();
    let langs: String = st.opts.a("languages").iter().map(|l| format!(r#"<option value="{}" {}>{}</option>"#,
        esc(&l.s("code")), if l.s("code") == lang { "selected" } else { "" }, esc(&l.s("name")))).collect();
    let s = &st.status;
    let missing = if s.g("translator").as_bool() == Some(false) {
        r#"<div class="small" style="color:var(--warn)">The Translator is not installed on this server, so only official translations show. An admin can install it under Settings → Plugins.</div>"#.to_string()
    } else { String::new() };
    let counts = if !s.is_nullish("rows") && s.has("rows") {
        format!(r#"<div class="small faint">{} of {} titles have a translation · {} waiting for the Translator{}</div>"#,
            s.i("shown"), s.i("rows"), s.i("waiting"), if s.truthy("running") { " · running now" } else { "" })
    } else { String::new() };
    dom::set_html(&st.el, &format!(r#"<div class="card stack">
      <h2>Language</h2>
      <p class="small muted" style="margin:0">Song, album and artist names in another script – Japanese, Korean, Chinese, Cyrillic and more – can show translated or romanized. Saved to your account, so it follows you to every device and to the car.</p>
      <div class="stack" style="gap:6px"><h3>Translated titles</h3>
        <div class="seg" data-key="mode">{modes}</div></div>
      <label class="stack" style="gap:4px"><span class="small muted">Translate into</span>
        <select class="input" id="tr-lang" {}>{langs}</select></label>
      <div class="small muted">Official translations come first (MusicBrainz and Apple, where the label published one); the rest come from the Translator on this server. Titles on your screen are translated first, the rest of the library in the background. Fix any title with ⋯ → Edit translation.</div>
      {missing}
      {counts}
    </div>"#, if mode == "off" { "disabled" } else { "" }));
    {
        let st2 = st.clone();
        dom::on_each(&st.el, r#".seg[data-key="mode"] button"#, "click", move |b, _| save(&st2, json!({ "translate_mode": dom::data(b, "v") })));
    }
    if let Some(sel) = dom::qs_in(&st.el, "#tr-lang") {
        let (st2, sel2) = (st.clone(), sel.clone());
        dom::on(&sel, "change", move |_| save(&st2, json!({ "translate_lang": dom::value(&sel2) })));
    }
}

fn save(st: &Rc<Lang>, patch: Value) {
    let st = st.clone();
    dom::spawn(async move {
        match api::put("/auth/prefs", patch).await {
            Ok(p) => {
                if let Some(mut u) = app::user() {
                    if let Some(o) = u.as_object_mut() { o.insert("prefs".into(), p.clone()); }
                    app::set_user(Some(u));
                }
                *st.mode.borrow_mut() = p.st("translate_mode").unwrap_or("off").to_string();
                if let Some(l) = p.st("translate_lang") { *st.lang.borrow_mut() = l.to_string(); }
                set_translate_mode(&st.mode.borrow());
                dom::spawn(async { let _ = api::post_empty("/translate/run").await; });
                toast("Saved", "ok");
            }
            Err(err) => toast(&err.message, "err"),
        }
        paint(&st);
    });
}
