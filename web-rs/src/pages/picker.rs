//! Port of web/js/pages/picker.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Manual download picker: search every source, see quality/size/uploader, download the exact result you choose.

use crate::api;
use crate::dom;
use crate::json::{self, J};
use crate::ui::{self, art, esc, icon, toast, ModalOpts};
use serde_json::{json, Map, Value};
use std::cell::{Cell, RefCell};
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
#[derive(Default)]
struct State {
    tab: String,
    lossless: bool,
    /// "Don't check the audio" (R2): what is picked here is imported without the audio check; remembered on this
    /// device
    no_check: bool,
    results: HashMap<String, Vec<Value>>,
    errors: HashMap<String, PickErr>,
    loading: HashSet<String>,
    /// another Soulseek search held the turn when this one was sent (one runs at a time): say it waits
    slsk_waiting: bool,
    /// the words of the last search: what "Search again" searches, and what a kept search's words are compared with
    q: String,
    /// the sources whose rows came from a search the server kept (R2): when, and with which words
    kept: HashMap<String, Kept>,
    /// "Search again" runs for these sources (their kept rows stay meanwhile)
    again: HashSet<String>,
    /// why a "Search again" failed (the kept rows stay)
    again_errors: HashMap<String, PickErr>,
    /// why a source's answer has no rows, in the server's words ("Prowlarr's indexers found nothing for …"):
    /// shown instead of a bare "No results."
    notes: HashMap<String, String>,
    /// one more for every new search: answers of an older one are dropped
    gen: u32,
}

/// Where "Don't check the audio" is remembered on this device.
const NO_CHECK_KEY: &str = "encore.pickNoCheck";

/// What a search the server kept says beyond its rows (`POST /api/picker/search` since R2, req-slsk-cache §4).
#[derive(Clone, Debug, Default, PartialEq)]
struct Kept {
    /// when the kept search ended (unix seconds)
    cached_at: f64,
    /// the server's words for it, on its clock: "Found at 14:02"
    found: String,
    /// the words it was searched with (can differ from the search box)
    query: String,
    /// "Search again" can run now (false while Soulseek isn't connected)
    can_search: bool,
    /// why it can't: "Soulseek isn't connected – next attempt at 14:20"
    notice: String,
}

/// The answer of `POST /api/picker/search`: since R2 `{items, from, cached_at, found, query, can_search, notice}`,
/// before a bare array (an older server). The rows, and what a kept search adds (None for a live one).
fn answer_of(v: &Value) -> (Vec<Value>, Option<Kept>) {
    if v.is_array() { return (v.items(), None); }
    let items = v.a("items");
    if v.s("from") != "cache" { return (items, None); }
    let no = v.g("can_search");
    let kept = Kept {
        cached_at: v.f("cached_at"),
        found: v.s("found").trim().to_string(),
        query: v.s("query").trim().to_string(),
        can_search: !(no == &Value::Bool(false) || no.as_f64() == Some(0.0)),
        notice: v.s("notice").trim().to_string(),
    };
    (items, Some(kept))
}

/// Keep (or drop) the server's reason why source `id`'s answer `r` has no rows: only for an empty answer, and not
/// for a kept one that can't search again, whose line already shows the notice (the source's hint).
fn set_note(notes: &mut HashMap<String, String>, id: &str, r: &Value, items: &[Value], kept: Option<&Kept>) {
    let note = r.s("notice").trim().to_string();
    if items.is_empty() && !note.is_empty() && kept.is_none_or(|k| k.can_search) {
        notes.insert(id.to_string(), note);
    } else {
        notes.remove(id);
    }
}

/// The words of two searches compared without case, punctuation or word order (the phone's `PickerRules.sameQuery`).
fn same_query(a: &str, b: &str) -> bool {
    let words = |s: &str| {
        let mut w: Vec<String> = s.to_lowercase().split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|x| !x.is_empty()).map(String::from).collect();
        w.sort();
        w
    };
    words(a) == words(b)
}

/// A unix time on a clock: (day number, day of the month, month, hour, minute).
type Clock<'a> = &'a dyn Fn(f64) -> (i64, u32, u32, u32, u32);

/// "Found at 14:02": the server's words when it sent them (its clock is the one of "next attempt at 14:20" beside
/// them), else made here from `cached_at`: today, "Found yesterday at 22:10", or "Found at 17/10 09:30".
fn found_text(k: &Kept, now: f64, clock: Clock) -> String {
    if !k.found.is_empty() { return k.found.clone(); }
    if k.cached_at <= 0.0 { return String::new(); }
    let (at, day, month, h, m) = clock(k.cached_at);
    let time = format!("{h:02}:{m:02}");
    match clock(now).0 - at {
        d if d <= 0 => format!("Found at {time}"),
        1 => format!("Found yesterday at {time}"),
        _ => format!("Found at {day}/{month} {time}"),
    }
}

/// The line under a source that answered from a kept search (req-slsk-cache §5, the phone's `PickerRules.foundLine`),
/// and whether "Search again" follows it:
/// - "Found at 14:02" · Search again
/// - "No results. · Found at 14:02" · Search again, when the kept search found nothing
/// - "Found at 14:02 for "Illuminate EP"", when it was searched with other words than the search box's
/// - "Found at 14:02 · Soulseek isn't connected – next attempt at 14:20", with no Search again while it can't search
fn found_line(k: &Kept, found: &str, typed: &str, empty: bool) -> (String, bool) {
    let mut parts: Vec<String> = Vec::new();
    if empty { parts.push("No results.".into()); }
    let mut f = found.to_string();
    if !k.query.is_empty() && !same_query(&k.query, typed) { f.push_str(&format!(" for \"{}\"", k.query)); }
    if !f.is_empty() { parts.push(f); }
    if !k.can_search && !k.notice.is_empty() { parts.push(k.notice.clone()); }
    (parts.join(" · "), k.can_search)
}

/// This device's clock for [`found_text`].
fn local_clock(t: f64) -> (i64, u32, u32, u32, u32) {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t * 1000.0));
    let (day, month) = (d.get_date(), d.get_month() + 1);
    (days_from_civil(d.get_full_year() as i64, month as i64, day as i64), day, month, d.get_hours(), d.get_minutes())
}

/// Days since 1970-01-01 of a calendar date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// "Don't check the audio" beside Lossless only, and whether it starts on. Only those who may take a download the
/// check turned away get it (Keep anyway's people: admins, and members with Fix their own downloads; the server
/// answers others "Ask an admin to pick without the audio check"). It starts as it was last left on this device
/// (`stored`). Hidden, it is off, so no `no_check` goes out even when this device remembers it on.
fn no_check_html(may: bool, stored: Option<&str>) -> (String, bool) {
    if !may { return (String::new(), false); }
    let on = stored == Some("1");
    (format!(r#"<label class="row small" style="gap:6px" title="Imports what you pick even when the audio check says it is another song"><input type="checkbox" id="pk-nocheck"{}> Don't check the audio</label>"#,
        if on { " checked" } else { "" }), on)
}

/// The note under the picker: what happens to the file picked here.
fn note_text(no_check: bool) -> &'static str {
    if no_check {
        "The audio check is off: the file you pick is imported even when the check says it is another song. It is still tagged with MusicBrainz data and de-duplicated. If it fails, the request fails instead of trying other sources."
    } else {
        "The file you pick is still fingerprint-checked, tagged with MusicBrainz data and de-duplicated. If it fails, the request fails instead of trying other sources."
    }
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
        // "Don't check the audio", as it was last left on this device, for those who may take it
        let (no_check_switch, no_check) = no_check_html(super::common::may_fix(), dom::ls_get(NO_CHECK_KEY).as_deref());
        let q0 = [opts.g("artist"), title_or_album].iter().filter(|v| json::truthy(v)).map(|v| json::js_string(v)).collect::<Vec<_>>().join(" ");
        let body = format!(r#"
      <form class="row wrap" id="pk-form">
        <div class="search grow" style="min-width:0">{search}<input class="input" name="q" type="search" enterkeyhint="search" value="{q}" placeholder="Search, or paste a YouTube Music / Bandcamp / any link" autocomplete="off"></div>
        <button class="btn primary">Search</button>
      </form>
      <div id="pk-linkhint"></div>
      <div class="row wrap mt" style="justify-content:space-between">
        <div class="tabs" id="pk-tabs" style="max-width:100%"></div>
        <div class="row wrap" style="gap:6px 16px">
          <label class="row small" style="gap:6px"><input type="checkbox" id="pk-lossless"> Lossless only</label>
          {no_check_switch}
        </div>
      </div>
      <div id="pk-out" class="mt" style="min-height:200px"></div>
      <form class="row wrap mt" id="pk-link">
        <div class="search grow">{link}<input class="input" name="url" placeholder="…or paste a link: Qobuz, Tidal, Deezer, YouTube, Bandcamp, SoundCloud…" autocomplete="off"></div>
        <button class="btn">{download}Download link</button>
      </form>
      <p class="small faint" style="margin:8px 0 0" id="pk-note">{note}</p>"#,
            search = icon("search"), q = esc(&q0), link = icon("link"), download = icon("download"),
            note = note_text(no_check));
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
            state: RefCell::new(State { tab: "all".into(), no_check, ..State::default() }),
            on_done,
        });

        // el.addEventListener('click', …): tabs / "Show all" buttons, then the Get buttons
        let p2 = p.clone();
        dom::on(&el, "click", move |e| {
            // "Search again" under a source that answered from a kept search
            if let Some(b) = dom::closest(e, "[data-again]") {
                p2.again(&dom::data(&b, "again"));
                return;
            }
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
        let p2 = p.clone();
        dom::on_sel(&el, "#pk-nocheck", "change", move |e| {
            let on = dom::target_el(e).map(|t| dom::checked(&t)).unwrap_or(false);
            p2.state.borrow_mut().no_check = on;
            dom::ls_set(NO_CHECK_KEY, if on { "1" } else { "0" });
            if let Some(n) = dom::qs_in(&p2.el, "#pk-note") { dom::set_text(&n, note_text(on)); }
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

/// A source's whole error with its settings link and Copy (R1): "<Source name>: <error>" is what Copy takes.
fn error_html(name: &str, err: &PickErr, class: &str) -> String {
    format!(r#"<div class="error-box small row{class}" style="align-items:flex-start"><span class="grow" style="min-width:0;overflow-wrap:anywhere">{}</span>{}</div>"#,
        hint_html(&err.message, &err.settings, &err.label), crate::details::copy_button(&format!("{name}: {}", err.message)))
}

/// The line of a kept search with its "Search again" button (see [`found_line`]).
fn found_html(src: &str, text: &str, again: bool) -> String {
    let button = if again {
        format!(r#"{}<button type="button" class="linkish" data-again="{}">Search again</button>"#, if text.is_empty() { "" } else { r#"<span aria-hidden="true">·</span>"# }, esc(src))
    } else { String::new() };
    if text.is_empty() && button.is_empty() { return String::new(); }
    format!(r#"<div class="muted small pk-found">{}{button}</div>"#, if text.is_empty() { String::new() } else { format!("<span>{}</span>", esc(text)) })
}

impl Picker {
    /// The source as `/picker/sources` listed it: `{id, name, state, hint, settings, settings_label}`.
    fn source(&self, id: &str) -> Option<&Value> { self.sources.iter().find(|s| s.g("id").as_str() == Some(id)) }

    /// The source's display name (the server's; the id when it is not listed).
    fn name(&self, id: &str) -> String { self.source(id).map(|s| s.s("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string()) }

    /// A source searches only when the server says it is ready; anything else (signed out, not connected,
    /// not installed, off) shows its hint and settings link under its tab instead.
    fn ready(&self, id: &str) -> bool { self.source(id).map(|s| { let st = s.s("state"); st.is_empty() || st == "ready" }).unwrap_or(false) }

    /// Whether the picker asks a source: a ready one, and Soulseek while it isn't connected too, because the server
    /// can answer from a search it kept (one that kept none says 409, and the hint shows as before).
    fn searches(&self, id: &str) -> bool {
        self.ready(id) || (id == "slskd" && self.source(id).is_some_and(|s| s.s("state") == "not_connected"))
    }

    /// The JS `tabs()`: every source has a tab; one that can't search shows why in a word instead of a count.
    fn tabs(&self) {
        let st = self.state.borrow();
        let all: usize = st.results.values().map(|r| r.len()).sum();
        let mut entries: Vec<(String, String, String)> = vec![("all".to_string(), "All".to_string(), all.to_string())];
        for s in &self.sources {
            let id = s.s("id");
            let n = st.results.get(&id).map(|r| r.len()).unwrap_or(0);
            // a source that answered from a kept search counts its rows, ready or not
            let word = if self.ready(&id) || st.kept.contains_key(&id) { "" } else { state_word(&s.s("state")) };
            entries.push((id, s.s("name"), if word.is_empty() { n.to_string() } else { word.to_string() }));
        }
        let html: String = entries.iter().map(|(k, l, n)| {
            let spin = st.loading.contains(k) || st.again.contains(k) || (k == "all" && !st.loading.is_empty());
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
        let kept = st.kept.get(src);
        let head = if st.tab == "all" {
            let count = if self.ready(src) || kept.is_some() { format!(r#" <span class="faint small">{}</span>"#, list.len()) } else { String::new() };
            format!(r#"<div class="section-head" style="margin:14px 0 8px"><h3>{}{count}</h3></div>"#, esc(&name))
        } else { String::new() };
        // a signed-out, stopped or switched-off source: "Sign in to Tidal – Settings → streamrip" (unless a search the
        // server kept answered for it)
        if let Some(s) = self.source(src).filter(|_| !self.ready(src) && kept.is_none()) {
            return head + &format!(r#"<div class="muted small">{}</div>"#, hint_html(&s.s("hint"), &s.s("settings"), &s.s("settings_label")));
        }
        if let Some(err) = st.errors.get(src).filter(|e| !e.message.is_empty()) {
            // the source's whole error, with Copy (R1)
            return head + &error_html(&name, err, "");
        }
        if st.loading.contains(src) && src == "slskd" && st.slsk_waiting {
            return head + r#"<div class="muted small"><span class="spinner"></span> Waiting for another Soulseek search, then searching…</div>"#;
        }
        if st.loading.contains(src) {
            return head + &format!(r#"<div class="muted small"><span class="spinner"></span> Searching {}…{}</div>"#,
                esc(&name), if src == "slskd" { " (Soulseek takes a few seconds)" } else { "" });
        }
        // a kept search: "Found at 14:02 · Search again"; while Search again runs its line, and why it failed – the
        // kept rows stay under it either way
        let line = if st.again.contains(src) {
            if src == "slskd" && st.slsk_waiting {
                r#"<div class="muted small pk-found"><span class="spinner"></span> Waiting for another Soulseek search, then searching…</div>"#.to_string()
            } else {
                format!(r#"<div class="muted small pk-found"><span class="spinner"></span> Searching {} again…</div>"#, esc(&name))
            }
        } else if let Some(k) = kept {
            let (text, again) = found_line(k, &found_text(k, dom::now_s(), &local_clock), &st.q, list.is_empty());
            found_html(src, &text, again)
        } else { String::new() };
        let failed = match st.again_errors.get(src).filter(|e| !e.message.is_empty() && !st.again.contains(src)) {
            Some(err) => error_html(&name, err, " pk-again-err"),
            None => String::new(),
        };
        if list.is_empty() {
            // the server's reason when it sent one (a kept answer's line already says "No results.")
            let none = match (st.notes.get(src), kept.is_none()) {
                (Some(n), true) => format!(r#"<div class="muted small">No results – {}</div>"#, esc(n)),
                (Some(n), false) => format!(r#"<div class="muted small">{}</div>"#, esc(n)),
                (None, true) => r#"<div class="muted small">No results.</div>"#.to_string(),
                (None, false) => String::new(),
            };
            return format!("{head}{line}{failed}{none}");
        }
        let shown = &list[..list.len().min(if st.tab == "all" { 8 } else { 150 })];
        let rows: String = shown.iter().map(|(i, x)| row(x, *i)).collect();
        let more = if st.tab == "all" && list.len() > shown.len() {
            format!(r#"<button class="btn sm ghost mt" data-t="{src}">Show all {} from {}</button>"#, list.len(), esc(&name))
        } else { String::new() };
        format!("{head}{line}{failed}<div>{rows}</div>\n      {more}")
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

    /// The body of `POST /picker/search` for source `s`: the request's details, the words, the request it was opened
    /// for (its kept searches are looked up first), and `fresh` for "Search again".
    fn search_body(&self, s: &Value, q: &str, fresh: bool) -> Map<String, Value> {
        let o = &self.opts;
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
        if o.truthy("request_id") { body.insert("request_id".into(), o.g("request_id").clone()); }
        if fresh { body.insert("fresh".into(), json!(true)); }
        body
    }

    /// The JS `run(q)`: one search per source, painting as each one lands.
    fn run(self: &Rc<Self>, q: &str) {
        let gen = {
            let mut st = self.state.borrow_mut();
            st.results.clear();
            st.errors.clear();
            st.kept.clear();
            st.again.clear();
            st.again_errors.clear();
            st.notes.clear();
            st.loading.clear();
            st.slsk_waiting = false;
            st.q = q.to_string();
            st.gen = st.gen.wrapping_add(1);
            st.gen
        };
        for s in &self.sources {
            let id = s.s("id");
            // a source that is signed out, not connected or off shows its hint instead of searching (Soulseek while it
            // isn't connected is still asked: a search the server kept can answer)
            if !self.searches(&id) { continue; }
            let ready = self.ready(&id);
            if ready { self.state.borrow_mut().loading.insert(id.clone()); }
            let body = self.search_body(s, q, false);
            let p = self.clone();
            dom::spawn(async move {
                let res = p.search(&id, gen, body).await;
                {
                    let mut st = p.state.borrow_mut();
                    // a newer search started meanwhile: its answers count
                    if st.gen != gen { return; }
                    match res {
                        Ok(r) => {
                            let (items, kept) = answer_of(&r);
                            set_note(&mut st.notes, &id, &r, &items, kept.as_ref());
                            st.results.insert(id.clone(), items);
                            if let Some(k) = kept { st.kept.insert(id.clone(), k); }
                        }
                        // a 409 carries the source's state and settings link ("Soulseek isn't connected yet – Settings →
                        // Soulseek"); a source that wasn't ready and had nothing kept keeps showing its hint
                        Err(e) => { if ready { st.errors.insert(id.clone(), PickErr::from_api(&e)); } }
                    }
                    st.loading.remove(&id);
                }
                if p.el.is_connected() { p.paint(); }
            });
        }
        self.paint();
    }

    /// `POST /picker/search`. Soulseek searches one at a time: when another one runs, this one waits for it (30 s at
    /// most). That is asked before this search is sent (so it can't be this one), and said only when the answer takes
    /// more than a second – a kept search answers at once, and the waiting line would only flash.
    async fn search(self: &Rc<Self>, id: &str, gen: u32, body: Map<String, Value>) -> api::ApiResult {
        let mut answered: Option<Rc<Cell<bool>>> = None;
        if id == "slskd" {
            if let Ok(r) = api::get("/system/soulseek").await {
                if r.truthy("search_running") {
                    let flag = Rc::new(Cell::new(false));
                    let (p, f) = (self.clone(), flag.clone());
                    dom::spawn(async move {
                        dom::sleep(1000).await;
                        if f.get() || p.state.borrow().gen != gen { return; }
                        p.state.borrow_mut().slsk_waiting = true;
                        if p.el.is_connected() { p.paint(); }
                    });
                    answered = Some(flag);
                }
            }
        }
        let r = api::post("/picker/search", Value::Object(body)).await;
        if let Some(f) = answered { f.set(true); }
        if id == "slskd" {
            let mut st = self.state.borrow_mut();
            if st.gen == gen { st.slsk_waiting = false; }
        }
        r
    }

    /// "Search again" (`fresh`) for one source whose rows came from a kept search: it is searched now (Soulseek through
    /// the server's turn). Its kept rows stay until the new answer replaces them, and stay when it fails (the line then
    /// says why).
    fn again(self: &Rc<Self>, id: &str) {
        let Some(s) = self.source(id).cloned() else { return };
        let (gen, q) = {
            let mut st = self.state.borrow_mut();
            if !st.again.insert(id.to_string()) { return; }
            st.again_errors.remove(id);
            (st.gen, st.q.clone())
        };
        let body = self.search_body(&s, &q, true);
        let (p, id) = (self.clone(), id.to_string());
        self.paint();
        dom::spawn(async move {
            let res = p.search(&id, gen, body).await;
            {
                let mut st = p.state.borrow_mut();
                if st.gen != gen { return; }
                st.again.remove(&id);
                match res {
                    Ok(r) => {
                        let (items, kept) = answer_of(&r);
                        set_note(&mut st.notes, &id, &r, &items, kept.as_ref());
                        st.results.insert(id.clone(), items);
                        // a live answer has no line; a kept one (another picker's search that just ended) keeps it
                        match kept { Some(k) => { st.kept.insert(id.clone(), k); } None => { st.kept.remove(&id); } }
                        st.errors.remove(&id);
                    }
                    // "Soulseek isn't connected – next attempt at 14:20", "Soulseek is busy with another search – try
                    // again in a moment": the kept rows stay
                    Err(e) => { st.again_errors.insert(id.clone(), PickErr::from_api(&e)); }
                }
            }
            if p.el.is_connected() { p.paint(); }
        });
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
        // "Don't check the audio": imported even when the audio check says it is another song
        if self.state.borrow().no_check { body.insert("no_check".into(), json!(true)); }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A test clock in UTC: (day number, day, month, hour, minute).
    fn utc(t: f64) -> (i64, u32, u32, u32, u32) {
        let day = (t / 86_400.0).floor() as i64;
        let secs = (t - day as f64 * 86_400.0) as u32;
        // October 2026 is all these tests need: day 20_739 is Oct 13
        (day, (day - 20_739 + 13) as u32, 10, secs / 3600, secs % 3600 / 60)
    }

    /// Unix seconds of a UTC time in October 2026.
    fn at(day: i64, h: i64, m: i64) -> f64 { ((days_from_civil(2026, 10, day) * 86_400) + h * 3600 + m * 60) as f64 }

    #[test]
    fn the_answer_is_read_as_an_object_or_an_older_servers_array() {
        // an older server: a bare array, a live answer
        let (items, kept) = answer_of(&json!([{ "source": "deezer", "title": "Illuminate EP" }]));
        assert_eq!((items.len(), kept), (1, None));
        // a live answer as an object
        let (items, kept) = answer_of(&json!({ "items": [{ "source": "deezer" }, { "source": "deezer" }], "from": "live", "cached_at": null, "found": null }));
        assert_eq!((items.len(), kept), (2, None));
        // a kept one, exactly as package B answers it
        let v = json!({ "items": [{ "source": "slskd", "title": "Duumu/Illuminate EP", "lossless": true }], "from": "cache", "cached_at": 1760875320,
            "found": "Found at 14:02", "query": "Duumu Illuminate EP", "can_search": true, "notice": null });
        let (items, kept) = answer_of(&v);
        assert_eq!(items.len(), 1);
        assert_eq!(kept, Some(Kept { cached_at: 1760875320.0, found: "Found at 14:02".into(), query: "Duumu Illuminate EP".into(), can_search: true, notice: String::new() }));
        // can_search false (or 0) while Soulseek isn't connected; missing means it can
        let down = answer_of(&json!({ "items": [], "from": "cache", "can_search": false, "notice": "Soulseek isn't connected – next attempt at 14:20" })).1.unwrap();
        assert!(!down.can_search && down.notice == "Soulseek isn't connected – next attempt at 14:20");
        assert!(!answer_of(&json!({ "from": "cache", "can_search": 0 })).1.unwrap().can_search);
        assert!(answer_of(&json!({ "from": "cache" })).1.unwrap().can_search);
        // anything else: no rows
        assert_eq!(answer_of(&json!("oops")).0.len(), 0);
        // the server's reason for an empty answer is kept for the source, a full answer drops it, and a kept answer
        // that can't search shows its notice on its own line
        let mut notes = HashMap::new();
        let empty = json!({ "items": [], "from": "live", "notice": "Prowlarr's indexers found nothing for \"x\"" });
        set_note(&mut notes, "torznab", &empty, &[], None);
        assert_eq!(notes.get("torznab").map(String::as_str), Some("Prowlarr's indexers found nothing for \"x\""));
        set_note(&mut notes, "torznab", &json!({ "items": [{}] }), &[json!({})], None);
        assert!(notes.is_empty());
        let down = Kept { can_search: false, ..Kept::default() };
        set_note(&mut notes, "slskd", &empty, &[], Some(&down));
        assert!(notes.is_empty());
        assert_eq!(answer_of(&Value::Null).0.len(), 0);
    }

    #[test]
    fn a_kept_search_reads_found_at_with_search_again() {
        let kept = Kept { cached_at: at(19, 14, 2), found: "Found at 14:02".into(), query: "Duumu Illuminate EP".into(), can_search: true, notice: String::new() };
        let line = |k: &Kept, typed: &str, empty: bool| found_line(k, &found_text(k, at(19, 15, 0), &utc), typed, empty);
        assert_eq!(line(&kept, "Duumu Illuminate EP", false), ("Found at 14:02".to_string(), true));
        // a kept search that found nothing
        assert_eq!(line(&kept, "Duumu Illuminate EP", true), ("No results. · Found at 14:02".to_string(), true));
        // searched with other words than the box has (the worker's album-only search)
        let album = Kept { query: "Illuminate EP".into(), ..kept.clone() };
        assert_eq!(line(&album, "Duumu Illuminate EP", false).0, "Found at 14:02 for \"Illuminate EP\"");
        // the same words in another order, case or punctuation are the same search
        assert_eq!(line(&kept, "illuminate ep, DUUMU", false).0, "Found at 14:02");
        // Soulseek not connected: the reason instead of Search again
        let down = Kept { can_search: false, notice: "Soulseek isn't connected – next attempt at 14:20".into(), ..kept.clone() };
        assert_eq!(line(&down, "Duumu Illuminate EP", false), ("Found at 14:02 · Soulseek isn't connected – next attempt at 14:20".to_string(), false));
        assert_eq!(line(&down, "Duumu Illuminate EP", true).0, "No results. · Found at 14:02 · Soulseek isn't connected – next attempt at 14:20");
        // the HTML: the words, a dot and the button
        let html = found_html("slskd", "Found at 14:02", true);
        assert!(html.contains("<span>Found at 14:02</span>") && html.contains(r#"data-again="slskd">Search again</button>"#), "{html}");
        assert!(!found_html("slskd", "Found at 14:02 · Soulseek isn't connected – next attempt at 14:20", false).contains("Search again"));
        assert_eq!(found_html("slskd", "", false), "");
    }

    #[test]
    fn found_time_is_made_here_when_the_server_sends_none() {
        let k = |t: f64| Kept { cached_at: t, can_search: true, ..Kept::default() };
        assert_eq!(found_text(&k(at(19, 14, 2)), at(19, 15, 0), &utc), "Found at 14:02");
        assert_eq!(found_text(&k(at(18, 22, 10)), at(19, 9, 0), &utc), "Found yesterday at 22:10");
        assert_eq!(found_text(&k(at(16, 8, 5)), at(19, 9, 0), &utc), "Found at 16/10 08:05");
        assert_eq!(found_text(&k(0.0), at(19, 9, 0), &utc), "");
        // the server's words win (its clock is the one of "next attempt at …")
        let server = Kept { found: "Found at 13:02".into(), ..k(at(19, 14, 2)) };
        assert_eq!(found_text(&server, at(19, 15, 0), &utc), "Found at 13:02");
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 10, 13), 20_739);
        assert_eq!(days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 28), 2);
    }

    #[test]
    fn the_note_says_what_happens_to_the_file_picked() {
        assert!(note_text(false).starts_with("The file you pick is still fingerprint-checked"));
        assert!(note_text(true).starts_with("The audio check is off: the file you pick is imported even when the check says it is another song."));
        assert!(same_query("Ke$ha – Blow", "blow, ke ha"));
    }

    #[test]
    fn the_switch_is_only_for_those_who_may_keep_a_download_anyway() {
        // admins and members with Fix their own downloads: off by default, on as this device left it
        let (html, on) = no_check_html(true, None);
        assert!(html.contains(r#"id="pk-nocheck">"#) && html.contains("Don't check the audio</label>") && !on, "{html}");
        assert!(html.contains(r#"title="Imports what you pick even when the audio check says it is another song""#), "{html}");
        let (html, on) = no_check_html(true, Some("1"));
        assert!(html.contains(r#"id="pk-nocheck" checked>"#) && on, "{html}");
        assert!(!no_check_html(true, Some("0")).1);
        // anyone else: no switch, and off even when this device remembers it on (the server would answer 403)
        assert_eq!(no_check_html(false, Some("1")), (String::new(), false));
        assert!(!same_query("Duumu Illuminate EP", "Duumu Illuminate"));
    }
}
