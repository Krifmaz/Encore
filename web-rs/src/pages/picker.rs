//! Port of web/js/pages/picker.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Manual download picker: search every source, see quality/size/uploader, download the exact result you choose.

use crate::api;
use crate::dom;
use crate::json::{self, J};
use crate::ui::{self, art, esc, icon, toast, ModalOpts};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use web_sys::Element;

/// A failed search on one source: the message and, when the server pointed at a setting (a 409: the
/// source is signed out, stopped or off), the settings link and its label.
struct PickErr {
    message: String,
    settings: String,
    label: String,
}

impl PickErr {
    fn from_api(e: &api::ApiError) -> PickErr {
        PickErr { message: e.message.clone(), settings: e.data.s("settings"), label: e.data.s("settings_label") }
    }
}

/// `hint – Settings → X`: a hint with its settings link (the link alone when the hint is empty).
fn hint_html(hint: &str, settings: &str, label: &str) -> String {
    let link = if settings.is_empty() { String::new() } else {
        format!(r#"<a href="{}" style="color:var(--accent)">{}</a>"#, esc(settings), esc(if label.is_empty() { "Settings" } else { label }))
    };
    match (hint.is_empty(), link.is_empty()) {
        (false, false) => format!("{} – {link}", esc(hint)),
        (false, true) => esc(hint),
        _ => link,
    }
}

/// The word after a tab's name for a source that can't search right now.
fn state_word(state: &str) -> &'static str {
    match state {
        "sign_in" => "sign in",
        "not_connected" => "not connected",
        "not_installed" => "not installed",
        "off" => "off",
        _ => "",
    }
}

/// The JS `state = { tab: 'all', lossless: false, results: {}, errors: {}, loading: new Set() }`.
struct State {
    tab: String,
    lossless: bool,
    results: HashMap<String, Vec<Value>>,
    errors: HashMap<String, PickErr>,
    loading: HashSet<String>,
    /// another Soulseek search held the turn when this one was sent (one runs at a time): say it waits
    slsk_waiting: bool,
}

/// What the JS closures captured: `opts`, `kind`, `info.sources`, the modal, `el`, `out` and `state`.
struct Picker {
    opts: Value,
    kind: &'static str,
    sources: Vec<Value>,
    m: ui::Modal,
    el: Element,
    out: Element,
    state: RefCell<State>,
    on_done: Option<Rc<dyn Fn(Value)>>,
}

/// `pickerModal(opts)` – opts: { kind: 'album'|'track', artist, title, album, mbid, artist_mbid, track_count, length, year, cover_url, isrc, request_id }.
pub fn picker_modal(opts: Value) { picker_modal_with(opts, None) }

/// The same with the JS `opts.onDone(r)` callback (a `Value` cannot carry a function).
pub fn picker_modal_with(opts: Value, on_done: Option<Rc<dyn Fn(Value)>>) {
    dom::spawn(async move {
        let info = match api::get("/picker/sources").await { Ok(v) => v, Err(e) => return toast(&e.message, "err") };
        if !info.truthy("allowed") { return toast("Manual search is limited to admins", "err"); }
        let kind = if opts.g("kind").as_str() == Some("track") { "track" } else { "album" };
        // opts.title || opts.album
        let title_or_album = if opts.truthy("title") { opts.g("title") } else { opts.g("album") };
        let title = format!("Pick a download · {}", ui::escv(title_or_album));
        let q0 = [opts.g("artist"), title_or_album].iter().filter(|v| json::truthy(v)).map(|v| json::js_string(v)).collect::<Vec<_>>().join(" ");
        let body = format!(r#"
      <form class="row wrap" id="pk-form">
        <div class="search grow" style="min-width:0">{search}<input class="input" name="q" type="search" enterkeyhint="search" value="{q}" placeholder="Search, or paste a YouTube Music / Bandcamp / any link" autocomplete="off"></div>
        <button class="btn primary">Search</button>
      </form>
      <div id="pk-linkhint"></div>
      <div class="row wrap mt" style="justify-content:space-between">
        <div class="tabs" id="pk-tabs" style="max-width:100%"></div>
        <label class="row small" style="gap:6px"><input type="checkbox" id="pk-lossless"> Lossless only</label>
      </div>
      <div id="pk-out" class="mt" style="min-height:200px"></div>
      <form class="row wrap mt" id="pk-link">
        <div class="search grow">{link}<input class="input" name="url" placeholder="…or paste a link: Qobuz, Tidal, Deezer, YouTube, Bandcamp, SoundCloud…" autocomplete="off"></div>
        <button class="btn">{download}Download link</button>
      </form>
      <p class="small faint" style="margin:8px 0 0">The file you pick is still fingerprint-checked, tagged with MusicBrainz data and de-duplicated. If it fails, the request fails instead of trying other sources.</p>"#,
            search = icon("search"), q = esc(&q0), link = icon("link"), download = icon("download"));
        let m = ui::modal(ModalOpts::new(&title, &body).wide(true));
        let el = m.el.clone();
        let Some(out) = dom::qs_in(&el, "#pk-out") else { return };
        let sources = info.a("sources");
        if sources.is_empty() {
            dom::set_html(&out, r#"<div class="empty muted">No download sources are enabled – set them up in Settings.</div>"#);
        }
        let p = Rc::new(Picker {
            opts,
            kind,
            sources,
            m,
            el: el.clone(),
            out,
            state: RefCell::new(State { tab: "all".into(), lossless: false, results: HashMap::new(), errors: HashMap::new(), loading: HashSet::new(), slsk_waiting: false }),
            on_done,
        });

        // el.addEventListener('click', …): tabs / "Show all" buttons, then the Get buttons
        let p2 = p.clone();
        dom::on(&el, "click", move |e| {
            if let Some(t) = dom::closest(e, "[data-t]") {
                p2.state.borrow_mut().tab = dom::data(&t, "t");
                p2.paint();
                return;
            }
            if let Some(b) = dom::closest(e, "[data-pick]") {
                let pick = dom::data(&b, "pick");
                let Some((src, i)) = pick.split_once(':') else { return };
                let x = {
                    let st = p2.state.borrow();
                    i.parse::<usize>().ok().and_then(|i| st.results.get(src).and_then(|l| l.get(i)).cloned())
                };
                let Some(x) = x else { return };
                p2.download(src, x.get("payload").cloned(), x.get("title").cloned(), b);
            }
        });
        let p2 = p.clone();
        dom::on_sel(&el, "#pk-lossless", "change", move |e| {
            p2.state.borrow_mut().lossless = dom::target_el(e).map(|t| dom::checked(&t)).unwrap_or(false);
            p2.paint();
        });
        let Some(q_in) = dom::qs_in(&el, "input[name=q]") else { return };
        let hint = {
            let (p, q_in, el) = (p.clone(), q_in.clone(), el.clone());
            move || {
                let v = js_trim(&dom::value(&q_in));
                if let Some(h) = dom::qs_in(&el, "#pk-linkhint") {
                    let html = if is_url(&v) {
                        format!(r#"<div class="pick-row" style="border:1px solid var(--accent);border-radius:12px;margin-top:10px;padding:10px 12px">
        <div class="grow"><div class="pt">Download this link</div><div class="ps">{}</div></div>
        <button class="btn sm primary" id="pk-getlink">{}Get</button></div>"#, esc(&v), icon("download"))
                    } else { String::new() };
                    dom::set_html(&h, &html);
                }
                if let Some(b) = dom::qs_in(&el, "#pk-getlink") {
                    let (p, btn) = (p.clone(), b.clone());
                    // e.currentTarget is the button the listener sits on
                    dom::on(&b, "click", move |_| p.download("link", Some(json!({ "url": v })), Some(json!(v)), btn.clone()));
                }
            }
        };
        dom::on(&q_in, "input", move |_| hint());
        if let Some(form) = dom::qs_in(&el, "#pk-form") {
            let (p, f) = (p.clone(), form.clone());
            dom::on(&form, "submit", move |e| {
                dom::prevent(e);
                let v = js_trim(&dom::value_of(&f, "[name=q]"));
                if is_url(&v) {
                    if let Some(btn) = dom::qs_in(&f, "button") { p.download("link", Some(json!({ "url": v })), Some(json!(v)), btn); }
                    return;
                }
                p.run(&v);
            });
        }
        if let Some(form) = dom::qs_in(&el, "#pk-link") {
            let (p, f) = (p.clone(), form.clone());
            dom::on(&form, "submit", move |e| {
                dom::prevent(e);
                let url = js_trim(&dom::value_of(&f, "[name=url]"));
                if !url.is_empty() {
                    if let Some(btn) = dom::qs_in(&f, "button") { p.download("link", Some(json!({ "url": url })), Some(json!(url)), btn); }
                }
            });
        }
        p.run(&dom::value(&q_in));
    });
}

/// `/^https?:\/\/\S+$/i.test(v.trim())` – the very same JavaScript regex.
fn is_url(v: &str) -> bool { js_sys::RegExp::new(r"^https?:\/\/\S+$", "i").test(&js_trim(v)) }

/// JavaScript's `String.prototype.trim` (same whitespace set as the JS).
fn js_trim(s: &str) -> String { String::from(js_sys::JsString::from(s).trim()) }

/// The JS `row(x, i)`.
fn row(x: &Value, i: usize) -> String {
    let title = x.s("title");
    format!(r#"<div class="pick-row">
      {cover}
      <div class="grow">
        <div class="pt">{title_html}</div>
        {sub}
        <div class="pm"><span class="badge {ok}">{quality}</span>
          {tracks}{duration}{size}
          {info}</div>
        {warning}
      </div>
      <button class="btn sm primary" data-pick="{source}:{i}">{dl}Get</button>
    </div>"#,
        cover = if x.truthy("cover") { format!(r#"<div class="thumb">{}</div>"#, art(&x.s("cover"), &title, "")) } else { String::new() },
        title_html = esc(&title),
        sub = if x.truthy("subtitle") && x.g("subtitle") != x.g("title") { format!(r#"<div class="ps">{}</div>"#, esc(&x.s("subtitle"))) } else { String::new() },
        ok = if x.truthy("lossless") { "ok" } else { "" },
        quality = if x.truthy("quality") { esc(&x.s("quality")) } else { "?".to_string() },
        tracks = if x.truthy("tracks") { format!("<span>{} file{}</span>", x.s("tracks"), if x.g("tracks").as_f64() == Some(1.0) { "" } else { "s" }) } else { String::new() },
        duration = if x.truthy("duration") { format!("<span>{}</span>", ui::fmt::dur(x.f("duration"))) } else { String::new() },
        size = if x.truthy("size") { format!("<span>{}</span>", ui::fmt::size(x.f("size"))) } else { String::new() },
        info = if x.truthy("info") { format!("<span>{}</span>", esc(&x.s("info"))) } else { String::new() },
        warning = if x.truthy("warning") { format!(r#"<div class="small" style="color:var(--warn)">{}</div>"#, esc(&x.s("warning"))) } else { String::new() },
        source = esc(&x.s("source")),
        dl = icon("download"),
    )
}

impl Picker {
    /// The source as `/picker/sources` listed it: `{id, name, state, hint, settings, settings_label}`.
    fn source(&self, id: &str) -> Option<&Value> { self.sources.iter().find(|s| s.g("id").as_str() == Some(id)) }

    /// The source's display name (the server's; the id when it is not listed).
    fn name(&self, id: &str) -> String { self.source(id).map(|s| s.s("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string()) }

    /// A source searches only when the server says it is ready; anything else (signed out, not connected,
    /// not installed, off) shows its hint and settings link under its tab instead.
    fn ready(&self, id: &str) -> bool { self.source(id).map(|s| { let st = s.s("state"); st.is_empty() || st == "ready" }).unwrap_or(false) }

    /// The JS `tabs()`: every source has a tab; one that can't search shows why in a word instead of a count.
    fn tabs(&self) {
        let st = self.state.borrow();
        let all: usize = st.results.values().map(|r| r.len()).sum();
        let mut entries: Vec<(String, String, String)> = vec![("all".to_string(), "All".to_string(), all.to_string())];
        for s in &self.sources {
            let id = s.s("id");
            let n = st.results.get(&id).map(|r| r.len()).unwrap_or(0);
            let word = if self.ready(&id) { "" } else { state_word(&s.s("state")) };
            entries.push((id, s.s("name"), if word.is_empty() { n.to_string() } else { word.to_string() }));
        }
        let html: String = entries.iter().map(|(k, l, n)| {
            let spin = st.loading.contains(k) || (k == "all" && !st.loading.is_empty());
            format!(r#"<button data-t="{k}" class="{}">{} {}</button>"#,
                if st.tab == *k { "active" } else { "" }, esc(l),
                if spin { r#"<span class="spinner" style="width:12px;height:12px"></span>"#.to_string() } else { format!(r#"<span class="faint">{}</span>"#, esc(n)) })
        }).collect();
        if let Some(t) = dom::qs_in(&self.el, "#pk-tabs") { dom::set_html(&t, &html); }
    }

    /// The JS `section(src)`.
    fn section(&self, src: &str) -> String {
        let st = self.state.borrow();
        let full: &[Value] = st.results.get(src).map(|v| v.as_slice()).unwrap_or(&[]);
        // (index in the unfiltered list = the JS `state.results[src].indexOf(x)`, x)
        let list: Vec<(usize, &Value)> = full.iter().enumerate().filter(|(_, x)| !st.lossless || x.truthy("lossless")).collect();
        let name = self.name(src);
        let head = if st.tab == "all" {
            let count = if self.ready(src) { format!(r#" <span class="faint small">{}</span>"#, list.len()) } else { String::new() };
            format!(r#"<div class="section-head" style="margin:14px 0 8px"><h3>{}{count}</h3></div>"#, esc(&name))
        } else { String::new() };
        // a signed-out, stopped or switched-off source: "Sign in to Tidal – Settings → streamrip"
        if let Some(s) = self.source(src).filter(|_| !self.ready(src)) {
            return head + &format!(r#"<div class="muted small">{}</div>"#, hint_html(&s.s("hint"), &s.s("settings"), &s.s("settings_label")));
        }
        if let Some(err) = st.errors.get(src).filter(|e| !e.message.is_empty()) {
            return head + &format!(r#"<div class="error-box small">{}</div>"#, hint_html(&err.message, &err.settings, &err.label));
        }
        if st.loading.contains(src) && src == "slskd" && st.slsk_waiting {
            return head + r#"<div class="muted small"><span class="spinner"></span> Waiting for another Soulseek search, then searching…</div>"#;
        }
        if st.loading.contains(src) {
            return head + &format!(r#"<div class="muted small"><span class="spinner"></span> Searching {}…{}</div>"#,
                esc(&name), if src == "slskd" { " (Soulseek takes a few seconds)" } else { "" });
        }
        if list.is_empty() { return head + r#"<div class="muted small">No results.</div>"#; }
        let shown = &list[..list.len().min(if st.tab == "all" { 8 } else { 150 })];
        let rows: String = shown.iter().map(|(i, x)| row(x, *i)).collect();
        let more = if st.tab == "all" && list.len() > shown.len() {
            format!(r#"<button class="btn sm ghost mt" data-t="{src}">Show all {} from {}</button>"#, list.len(), esc(&name))
        } else { String::new() };
        format!("{head}<div>{rows}</div>\n      {more}")
    }

    /// The JS `paint()`.
    fn paint(&self) {
        self.tabs();
        if self.sources.is_empty() { return; }
        let ids: Vec<String> = {
            let st = self.state.borrow();
            if st.tab == "all" { self.sources.iter().map(|s| s.s("id")).collect() } else { vec![st.tab.clone()] }
        };
        let html: String = ids.iter().map(|id| self.section(id)).collect();
        dom::set_html(&self.out, &html);
    }

    /// The JS `run(q)`: one search per source, painting as each one lands.
    fn run(self: &Rc<Self>, q: &str) {
        {
            let mut st = self.state.borrow_mut();
            st.results.clear();
            st.errors.clear();
            st.slsk_waiting = false;
        }
        let o = &self.opts;
        for s in &self.sources {
            let id = s.s("id");
            // a source that is signed out, not connected or off shows its hint instead of searching
            if !self.ready(&id) { continue; }
            self.state.borrow_mut().loading.insert(id.clone());
            let mut body = Map::new();
            if s.has("id") { body.insert("source".into(), s.g("id").clone()); }
            body.insert("kind".into(), json!(self.kind));
            body.insert("artist".into(), if o.truthy("artist") { o.g("artist").clone() } else { json!("") });
            body.insert("title".into(), if o.truthy("title") { o.g("title").clone() } else if o.truthy("album") { o.g("album").clone() } else { json!("") });
            body.insert("query".into(), json!(q));
            if self.kind == "track" && o.has("album") { body.insert("album".into(), o.g("album").clone()); }
            // JSON.stringify drops undefined: only keys opts actually has go out
            for k in ["mbid", "artist_mbid", "track_count", "length"] {
                if o.has(k) { body.insert(k.into(), o.g(k).clone()); }
            }
            let p = self.clone();
            dom::spawn(async move {
                // Soulseek searches one at a time: when another one runs, this one waits for it (30 s at most) –
                // asked before this search is sent, so it can't be this one
                if id == "slskd" {
                    if let Ok(r) = api::get("/system/soulseek").await {
                        if r.truthy("search_running") {
                            p.state.borrow_mut().slsk_waiting = true;
                            if p.el.is_connected() { p.paint(); }
                        }
                    }
                }
                match api::post("/picker/search", Value::Object(body)).await {
                    Ok(r) => { p.state.borrow_mut().results.insert(id.clone(), r.items()); }
                    // a 409 carries the source's state and settings link ("Soulseek isn't connected yet – Settings → Soulseek")
                    Err(e) => { p.state.borrow_mut().errors.insert(id.clone(), PickErr::from_api(&e)); }
                }
                p.state.borrow_mut().loading.remove(&id);
                if p.el.is_connected() { p.paint(); }
            });
        }
        self.paint();
    }

    /// The JS `download(source, payload, title, btn)`.
    fn download(self: &Rc<Self>, source: &str, payload: Option<Value>, title: Option<Value>, btn: Element) {
        dom::set_disabled(&btn, true);
        let o = &self.opts;
        let mut req = Map::new();
        req.insert("kind".into(), json!(self.kind));
        req.insert("mbid".into(), if o.truthy("mbid") { o.g("mbid").clone() } else { Value::Null });
        if o.has("artist") { req.insert("artist".into(), o.g("artist").clone()); }
        // title: opts.title || opts.album
        if o.truthy("title") { req.insert("title".into(), o.g("title").clone()); }
        else if o.has("album") { req.insert("title".into(), o.g("album").clone()); }
        // album: opts.album || (kind === 'album' ? opts.title : null)
        if o.truthy("album") { req.insert("album".into(), o.g("album").clone()); }
        else if self.kind == "album" { if o.has("title") { req.insert("album".into(), o.g("title").clone()); } }
        else { req.insert("album".into(), Value::Null); }
        for k in ["year", "cover_url", "length", "isrc"] {
            if o.has(k) { req.insert(k.into(), o.g(k).clone()); }
        }
        req.insert("album_mode".into(), json!("song"));
        if o.truthy("ext") { req.insert("ext".into(), o.g("ext").clone()); }
        let mut body = Map::new();
        body.insert("source".into(), json!(source));
        if let Some(p) = payload { body.insert("payload".into(), p); }
        if let Some(t) = title { body.insert("title".into(), t); }
        if o.truthy("request_id") { body.insert("request_id".into(), o.g("request_id").clone()); }
        else { body.insert("request".into(), Value::Object(req)); }
        let p = self.clone();
        dom::spawn(async move {
            match api::post("/picker/pick", Value::Object(body)).await {
                Ok(r) => {
                    toast(if r.g("status").as_str() == Some("pending") { "Sent for approval – your pick will be used" } else { "Downloading your pick" }, "ok");
                    dom::dispatch("encore:requests-changed", None);
                    p.m.close();
                    if let Some(f) = &p.on_done { f(r); }
                }
                Err(e) => {
                    toast(&e.message, "err");
                    dom::set_disabled(&btn, false);
                }
            }
        });
    }
}
