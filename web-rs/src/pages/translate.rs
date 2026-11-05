//! Translated titles: Settings → Language and ⋯ → Edit translation.
//!
//! Settings → Language is a Leptos view ([`Language`]) over one signal: the reader's choices and the server's
//! translation status. The Edit translation dialog ([`edit_translation`]) is still drawn as HTML.

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::J;
use crate::ui::{esc, icon, modal, set_translate_mode, toast, ModalOpts};
use serde_json::{json, Value};
use kit_leptos::html::raw;
use leptos::prelude::*;
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
            let (done, original) = (done.clone(), original);
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

/// What Settings → Language draws from.
#[derive(Clone)]
pub struct Lang {
    /// How translated titles show (`off`, or one of the server's modes).
    pub mode: String,
    /// The language titles are translated into.
    pub lang: String,
    /// The answer of GET /api/translate/status (counts and sources only for admins).
    pub status: Value,
}

/// Settings → Language: how translated titles show, and into which language. When the languages can't be
/// loaded the page says why in its place.
pub fn render_language(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let opts = match api::get("/translate/languages").await {
            Ok(o) => o,
            Err(err) => {
                dom::set_html(&el, &format!(r#"<div class="error-box">{}</div>"#, esc(&err.message)));
                return Ok(());
            }
        };
        let status = api::get("/translate/status").await.unwrap_or(Value::Null);
        let prefs = app::user().map(|u| u.o("prefs")).unwrap_or(Value::Null);
        let mode = prefs.st("translate_mode").unwrap_or("off").to_string();
        let lang = prefs.st("translate_lang").map(String::from).or_else(|| opts.st("default").map(String::from)).unwrap_or_else(|| "en".into());
        dom::set_html(&el, "");
        // the signal lives as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let state = owner.with(|| RwSignal::new(Lang { mode, lang, status }));
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&el, move || view! { <Language opts state/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        Ok(())
    })
}

/// The Language card and, for admins, the Translation sources card.
#[component]
pub fn Language(
    /// The answer of GET /api/translate/languages: the modes and languages to pick from.
    opts: Value,
    /// The reader's choices and the server's status.
    state: RwSignal<Lang>,
) -> impl IntoView {
    move || {
        let Lang { mode, lang, status } = state.get();
        let modes = opts
            .a("modes")
            .iter()
            .map(|x| {
                let id = x.s("id");
                let on = if mode == id { "on" } else { "" };
                let v = id.clone();
                view! { <button type="button" data-v=id class=on on:click=move |_| save(state, json!({ "translate_mode": v.clone() }))>{x.s("label")}</button> }
            })
            .collect_view();
        let langs = opts
            .a("languages")
            .iter()
            .map(|l| view! { <option value=l.s("code") selected=l.s("code") == lang>{l.s("name")}</option> })
            .collect_view();
        // which sources this server uses (the server's words)
        let missing = status.st("summary").filter(|l| !l.is_empty()).map(|l| view! { <div class="small muted">{l.to_string()}</div> });
        let counts = counts_line(&status);
        let counts = (!counts.is_empty()).then(|| view! { <div class="small faint">{counts}</div> });
        let on_lang = move |e: web_sys::Event| {
            if let Some(t) = dom::target_el(&e) {
                save(state, json!({ "translate_lang": dom::value(&t) }));
            }
        };
        view! {
            <div class="card stack">
                <h2>"Language"</h2>
                <p class="small muted" style="margin:0">"Song, album and artist names in another script – Japanese, Korean, Chinese, Cyrillic and more – can show translated or romanized. Saved to your account, so it follows you to every device and to the car."</p>
                <div class="stack" style="gap:6px"><h3>"Translated titles"</h3>
                    <div class="seg" data-key="mode">{modes}</div></div>
                <label class="stack" style="gap:4px"><span class="small muted">"Translate into"</span>
                    <select class="input" id="tr-lang" disabled=mode == "off" on:change=on_lang>{langs}</select></label>
                <div class="small muted">"Corrections by hand come first, then official names (MusicBrainz and Apple, where the label published one), romanization made on this server, then DeepL and the local model where an admin set them up. Titles on your screen are translated first, the rest of the library in the background. Fix any title with ⋯ → Edit translation."</div>
                {missing}
                {counts}
            </div>
            {sources_card(&status, state)}
        }
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

/// The server setting that switches a source on and off; `None` for one without a switch (the corrections by
/// hand).
fn tier_setting(t: &Value) -> Option<String> {
    t.st("setting").filter(|k| !k.is_empty()).map(str::to_string)
}

/// Admins: the sources of translated titles, strongest first, each with its state and its switch (the server's
/// `tiers`; the same settings as Settings → Library → Translated titles). Nothing for a member (no `tiers`).
fn sources_card(s: &Value, state: RwSignal<Lang>) -> Option<impl IntoView> {
    let tiers = s.a("tiers");
    if tiers.is_empty() {
        return None;
    }
    let rows = tiers
        .iter()
        .map(|t| {
            let switch = tier_setting(t).map(|key| {
                let k = key.clone();
                let on_change = move |e: web_sys::Event| {
                    if let Some(b) = dom::target_el(&e) {
                        switch_tier(state, k.clone(), &b);
                    }
                };
                view! { <label class="switch" title=t.s("name")><input type="checkbox" data-tier=key checked=t.truthy("on") on:change=on_change/><i></i></label> }
            });
            view! {
                <div class="row between" style="gap:12px;padding:8px 0;align-items:flex-start">
                    <div style="min-width:0"><div>{t.s("state")}</div><div class="small muted">{t.s("help")}</div></div>{switch}</div>
            }
        })
        .collect_view();
    Some(view! {
        <div class="card stack mt" id="tr-sources">
            <h2>"Translation sources"</h2>
            <p class="small muted" style="margin:0">"Strongest first: a source higher up wins over the ones below it, and its results replace theirs when it gets to a title later."</p>
            <div class="stack" style="gap:0">{rows}</div>
            <div class="small muted">"The DeepL API key and the monthly limit are under "{raw(r##"<a href="#/settings?s=translations">Settings → Library → Translated titles</a>"##)}"."</div>
        </div>
    })
}

/// A source's switch `b` for setting `key`: saved as a server setting, then the states are read again; on a
/// failure the switch goes back.
fn switch_tier(state: RwSignal<Lang>, key: String, b: &Element) {
    let on = dom::checked(b);
    let b = b.clone();
    dom::spawn(async move {
        let mut body = serde_json::Map::new();
        body.insert(key, Value::Bool(on));
        match api::put("/admin/settings", Value::Object(body)).await {
            Ok(_) => {
                dom::spawn(async { let _ = api::post_empty("/translate/run").await; });
                toast("Saved", "ok");
                if let Ok(s) = api::get("/translate/status").await {
                    state.update(|l| l.status = s);
                }
            }
            Err(err) => {
                dom::set_checked(&b, !on);
                toast(&err.message, "err");
            }
        }
    });
}

/// Saves `patch` to the reader's preferences, then takes the answer as the page's choices; the card is drawn
/// again either way (a failure puts the last saved choice back).
fn save(state: RwSignal<Lang>, patch: Value) {
    dom::spawn(async move {
        match api::put("/auth/prefs", patch).await {
            Ok(p) => {
                if let Some(mut u) = app::user() {
                    if let Some(o) = u.as_object_mut() { o.insert("prefs".into(), p.clone()); }
                    app::set_user(Some(u));
                }
                let mode = p.st("translate_mode").unwrap_or("off").to_string();
                set_translate_mode(&mode);
                state.update(|l| {
                    l.mode = mode;
                    if let Some(lang) = p.st("translate_lang") { l.lang = lang.to_string(); }
                });
                dom::spawn(async { let _ = api::post_empty("/translate/run").await; });
                toast("Saved", "ok");
            }
            Err(err) => {
                toast(&err.message, "err");
                state.update(|_| ());
            }
        }
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

    /// Every source but the corrections by hand has a switch, named by its setting.
    #[test]
    fn each_source_but_the_corrections_has_a_switch() {
        let keys: Vec<Option<String>> = tiers()["tiers"].as_array().unwrap().iter().map(tier_setting).collect();
        assert_eq!(keys[0], None);
        assert_eq!(keys[1..].iter().flatten().cloned().collect::<Vec<_>>(), ["translate_official", "translate_romanize", "translate_deepl", "translate_local_model"]);
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
