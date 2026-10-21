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
            "romanized" | "letters" => "Romanization built into the server",
            "deepl" => "DeepL",
            "model" => "The local model on this server",
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
    status: RefCell<Value>,
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
        let st = Rc::new(Lang { el, opts, status: RefCell::new(status), mode: RefCell::new(mode), lang: RefCell::new(lang) });
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
    let status = st.status.borrow().clone();
    let s = &status;
    // which sources this server uses (the server's words)
    let missing = match s.st("summary") {
        Some(line) if !line.is_empty() => format!(r#"<div class="small muted">{}</div>"#, esc(line)),
        _ => String::new(),
    };
    let counts = counts_line(s);
    let counts = if counts.is_empty() { counts } else { format!(r#"<div class="small faint">{}</div>"#, esc(&counts)) };
    let sources = sources_card(s);
    dom::set_html(&st.el, &format!(r#"<div class="card stack">
      <h2>Language</h2>
      <p class="small muted" style="margin:0">Song, album and artist names in another script – Japanese, Korean, Chinese, Cyrillic and more – can show translated or romanized. Saved to your account, so it follows you to every device and to the car.</p>
      <div class="stack" style="gap:6px"><h3>Translated titles</h3>
        <div class="seg" data-key="mode">{modes}</div></div>
      <label class="stack" style="gap:4px"><span class="small muted">Translate into</span>
        <select class="input" id="tr-lang" {}>{langs}</select></label>
      <div class="small muted">Corrections by hand come first, then official names (MusicBrainz and Apple, where the label published one), romanization made on this server, then DeepL and the local model where an admin set them up. Titles on your screen are translated first, the rest of the library in the background. Fix any title with ⋯ → Edit translation.</div>
      {missing}
      {counts}
    </div>{sources}"#, if mode == "off" { "disabled" } else { "" }));
    {
        let st2 = st.clone();
        dom::on_each(&st.el, r#".seg[data-key="mode"] button"#, "click", move |b, _| save(&st2, json!({ "translate_mode": dom::data(b, "v") })));
    }
    if let Some(sel) = dom::qs_in(&st.el, "#tr-lang") {
        let (st2, sel2) = (st.clone(), sel.clone());
        dom::on(&sel, "change", move |_| save(&st2, json!({ "translate_lang": dom::value(&sel2) })));
    }
    for b in dom::qsa_in(&st.el, "[data-tier]") {
        let (st2, b2) = (st.clone(), b.clone());
        dom::on(&b, "change", move |_| switch_tier(&st2, &b2));
    }
}

/// Admins: how far the translations are ("3120 of 3400 titles have a translation"), with the titles left for
/// the local model only while it can run – installed, downloaded and switched on; otherwise they would read as
/// a queue that never moves. Empty for everyone else (no counts).
fn counts_line(s: &Value) -> String {
    if s.is_nullish("rows") || !s.has("rows") { return String::new(); }
    let model_on = s.a("tiers").iter().find(|t| t.s("id") == "model").is_none_or(|t| t.truthy("on"));
    let waiting = if s.truthy("translator") && model_on { format!(" · {} waiting for the local model", s.i("waiting")) } else { String::new() };
    format!("{} of {} titles have a translation{waiting}{}", s.i("shown"), s.i("rows"), if s.truthy("running") { " · running now" } else { "" })
}

/// Admins: the sources of translated titles, strongest first, each with its state and its switch (the server's
/// `tiers`; the same settings as Settings → Library → Translated titles).
fn sources_card(s: &Value) -> String {
    let tiers = s.a("tiers");
    if tiers.is_empty() {
        return String::new();
    }
    let rows: String = tiers.iter().map(|t| {
        let switch = match t.st("setting") {
            Some(key) if !key.is_empty() => format!(r#"<label class="switch" title="{}"><input type="checkbox" data-tier="{}" {}><i></i></label>"#,
                esc(&t.s("name")), esc(key), if t.truthy("on") { "checked" } else { "" }),
            _ => String::new(),
        };
        format!(r#"<div class="row between" style="gap:12px;padding:8px 0;align-items:flex-start">
        <div style="min-width:0"><div>{}</div><div class="small muted">{}</div></div>{switch}</div>"#, esc(&t.s("state")), esc(&t.s("help")))
    }).collect();
    format!(r##"<div class="card stack mt" id="tr-sources">
      <h2>Translation sources</h2>
      <p class="small muted" style="margin:0">Strongest first: a source higher up wins over the ones below it, and its results replace theirs when it gets to a title later.</p>
      <div class="stack" style="gap:0">{rows}</div>
      <div class="small muted">The DeepL API key and the monthly limit are under <a href="#/settings?s=translations">Settings → Library → Translated titles</a>.</div>
    </div>"##)
}

/// A source's switch: saved as a server setting, then the states are read again.
fn switch_tier(st: &Rc<Lang>, b: &Element) {
    let (key, on) = (dom::data(b, "tier"), dom::checked(b));
    let (st, b) = (st.clone(), b.clone());
    dom::spawn(async move {
        let mut body = serde_json::Map::new();
        body.insert(key, Value::Bool(on));
        match api::put("/admin/settings", Value::Object(body)).await {
            Ok(_) => {
                dom::spawn(async { let _ = api::post_empty("/translate/run").await; });
                toast("Saved", "ok");
                if let Ok(s) = api::get("/translate/status").await {
                    *st.status.borrow_mut() = s;
                    paint(&st);
                }
            }
            Err(err) => { dom::set_checked(&b, !on); toast(&err.message, "err"); }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tiers() -> Value {
        json!({ "tiers": [
            { "id": "manual", "name": "Corrections by hand", "setting": null, "on": true, "state": "Corrections by hand: none yet", "help": "Made with ⋯ → Edit translation." },
            { "id": "official", "name": "Official names", "setting": "translate_official", "on": true, "state": "Official names: MusicBrainz and Apple", "help": "MusicBrainz." },
            { "id": "romanize", "name": "Romanization", "setting": "translate_romanize", "on": false, "state": "Romanization: off", "help": "Kana." },
            { "id": "deepl", "name": "DeepL", "setting": "translate_deepl", "on": true, "state": "DeepL: 12 340 of 400 000 characters this month", "help": "Titles." },
            { "id": "model", "name": "Local model", "setting": "translate_local_model", "on": true, "state": "Local model: not installed – install it in Plugins", "help": "Last." } ] })
    }

    /// One row per source with its state, a switch where it has a setting (none for the corrections by hand),
    /// the settings link; nothing for a member's answer (no `tiers`).
    #[test]
    fn the_translation_sources_card() {
        let html = sources_card(&tiers());
        assert!(html.contains("<h2>Translation sources</h2>"), "{html}");
        assert_eq!(html.matches("data-tier=").count(), 4, "{html}");
        assert!(html.contains(r#"data-tier="translate_deepl" checked"#) && html.contains(r#"data-tier="translate_romanize" >"#), "{html}");
        assert!(!html.contains(r#"title="Corrections by hand""#), "no switch for the corrections: {html}");
        assert!(html.contains("DeepL: 12 340 of 400 000 characters this month") && html.contains("Local model: not installed – install it in Plugins"), "{html}");
        assert!(html.contains(r##"href="#/settings?s=translations""##), "{html}");
        assert_eq!(sources_card(&json!({ "translator": false, "summary": "x" })), "");
    }

    /// The titles left for the local model are counted only while it can run.
    #[test]
    fn the_counts_say_the_model_only_when_it_runs() {
        let mut s = tiers();
        s["rows"] = json!(3400);
        s["shown"] = json!(3120);
        s["waiting"] = json!(280);
        s["translator"] = json!(false);
        assert_eq!(counts_line(&s), "3120 of 3400 titles have a translation", "not installed");
        s["translator"] = json!(true);
        assert_eq!(counts_line(&s), "3120 of 3400 titles have a translation · 280 waiting for the local model");
        s["tiers"][4]["on"] = json!(false);
        assert_eq!(counts_line(&s), "3120 of 3400 titles have a translation", "switched off");
        assert_eq!(counts_line(&json!({ "translator": true })), "", "a member");
    }
}
