//! Lyrics: reading .lrc stamps, loading a song's lyrics, and the line that follows the music.

use super::*;

thread_local! {
    pub(super) static LYR: RefCell<Lyr> = RefCell::new(Lyr::default());
    static LAST_LINE: Cell<i64> = const { Cell::new(-1) };
    /// Bumped by every tap of Earlier / Later: a save waits until the taps stop (the newest tap's save runs).
    static NUDGE_GEN: Cell<u64> = const { Cell::new(0) };
}

/// One tap of Earlier / Later, ms.
const NUDGE_STEP_MS: i64 = 250;
/// The largest nudge either way, ms (the server's `SHIFT_MAX_MS`).
const NUDGE_MAX_MS: i64 = 30_000;
/// How long after the last tap the nudge is saved, ms.
const NUDGE_SAVE_AFTER_MS: u32 = 600;

/// One time-synced line: when it is stamped (seconds) and its words.
struct Line { t: f64, text: String }
/// `lyr = { id, lines, plain, loading, instrumental, shift_ms }`.
pub(super) struct Lyr {
    /// the song they belong to (null: none)
    id: Value,
    /// the time-synced lines as stamped
    lines: Vec<Line>,
    /// plain lyrics (when not time-synced)
    plain: String,
    /// asked, no answer yet
    loading: bool,
    /// LRCLIB says the song has no words
    instrumental: bool,
    /// how much later the lines are shown, ms (the server's `shift_ms`, the song's nudge for every device)
    shift_ms: i64,
}
impl Default for Lyr {
    fn default() -> Self { Self { id: Value::Null, lines: vec![], plain: String::new(), loading: false, instrumental: false, shift_ms: 0 } }
}
impl Lyr {
    /// When line `ln` is shown, seconds: its stamp plus the nudge.
    fn at(&self, ln: &Line) -> f64 { ln.t + self.shift_ms as f64 / 1000.0 }
}

// ---- lyrics ----------------------------------------------------------------------------------
/// One `[mm:ss.xx]` stamp: the regex `\[(\d+):(\d+(?:\.\d+)?)\]` applied to the bracket's content.
fn parse_stamp(s: &str) -> Option<f64> {
    let (m, rest) = s.split_once(':')?;
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
    if !digits(m) { return None; }
    match rest.split_once('.') {
        Some((sec, frac)) => { if !digits(sec) || !digits(frac) { return None; } }
        None => { if !digits(rest) { return None; } }
    }
    let minutes: f64 = m.parse().ok()?;
    let seconds: f64 = rest.parse().ok()?;
    Some(minutes * 60.0 + seconds)
}
/// `[...line.matchAll(/\[(\d+):(\d+(?:\.\d+)?)\]/g)]`.
fn lrc_stamps(line: &str) -> Vec<f64> {
    let b = line.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            if let Some(j) = b[i + 1..].iter().position(|&c| c == b']') {
                if let Some(t) = parse_stamp(&line[i + 1..i + 1 + j]) { out.push(t); i = i + 1 + j + 1; continue; }
            }
        }
        i += 1;
    }
    out
}
/// `line.replace(/\[[^\]]*\]/g, '')`.
fn strip_brackets(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        let Some(i) = rest.find('[') else { out.push_str(rest); break };
        match rest[i + 1..].find(']') {
            Some(j) => { out.push_str(&rest[..i]); rest = &rest[i + 1 + j + 1..]; }
            None => { out.push_str(rest); break }
        }
    }
    out
}
fn parse_lrc(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let stamps = lrc_stamps(line);
        let words = strip_brackets(line).trim().to_string();
        for t in stamps { out.push(Line { t, text: words.clone() }); }
    }
    out.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// The "Report lyrics" button under lyrics that are shown (wrong words or timing → the report dialog with
/// Wrong lyrics ticked; full.rs wires the click).
const REPORT_BTN: &str = r#"<div class="np-lyrics-report"><button class="btn sm ghost" data-lyrics-report>Report lyrics</button></div>"#;

/// The nudge as shown between Earlier and Later: "In sync", or "+0.25 s" / "−1.5 s" (lines later / earlier).
fn shift_label(ms: i64) -> String {
    if ms == 0 { return "In sync".into(); }
    let num = format!("{:.2}", ms.unsigned_abs() as f64 / 1000.0);
    let num = num.trim_end_matches('0').trim_end_matches('.');
    format!("{}{num} s", if ms > 0 { "+" } else { "\u{2212}" })
}

/// Under time-synced lines: Earlier · the nudge (a click puts them back in sync) · Later, "Other timing" (the report
/// dialog with Lyrics out of sync ticked) and "Report lyrics" (full.rs wires the clicks).
fn timing_html(shift_ms: i64) -> String {
    format!(concat!(r#"<div class="np-lyrics-report np-lyrics-timing">"#,
        r#"<button class="btn sm ghost" data-lyrics-nudge="-1" title="Show the lines sooner">Earlier</button>"#,
        r#"<button class="btn sm ghost" data-lyrics-reset title="Back in sync">{label}</button>"#,
        r#"<button class="btn sm ghost" data-lyrics-nudge="1" title="Show the lines later">Later</button>"#,
        r#"<button class="btn sm ghost" data-lyrics-other>Other timing</button>"#,
        r#"<button class="btn sm ghost" data-lyrics-report>Report lyrics</button></div>"#),
        label = shift_label(shift_ms))
}

/// The lyrics pane's content for the song playing now: the synced lines, the plain text or why there
/// is nothing – with "Report lyrics" under whatever is shown (a song of the library only).
pub(super) fn lyrics_html() -> String {
    let Some(t) = now_track() else { return String::new() };
    if t.truthy("preview") { return r#"<div class="muted np-empty">Lyrics are available for songs in your library.</div>"#.into(); }
    if !t.g("id").is_number() { return r#"<div class="muted np-empty">No lyrics for this song.</div>"#.into(); }
    // a Surround song (ids from 1 000 000 000, server surround::BASE_ID) shows its stereo song's lyrics: reported there
    let report = if t.i("id") >= 1_000_000_000 { "" } else { REPORT_BTN };
    LYR.with(|l| {
        let l = l.borrow();
        if l.loading || l.id != *t.g("id") { return r#"<div class="muted np-empty"><span class="spinner"></span> Looking for lyrics…</div>"#.to_string(); }
        if !l.lines.is_empty() {
            // a Surround song is nudged (and reported) on its stereo song: no controls here
            let controls = if report.is_empty() { String::new() } else { timing_html(l.shift_ms) };
            return l.lines.iter().enumerate().map(|(i, ln)| {
                let text = esc(&ln.text);
                // data-t: when the line is shown (a click seeks there)
                format!(r#"<p data-t="{}" data-li="{i}">{}</p>"#, js_num(l.at(ln)), if text.is_empty() { "♪" } else { &text })
            }).collect::<String>() + &controls;
        }
        if !l.plain.is_empty() { return format!(r#"<div class="np-plain">{}</div>{report}"#, esc(&l.plain)); }
        if l.instrumental { return format!(r#"<div class="muted np-empty">Instrumental</div>{report}"#); }
        r#"<div class="muted np-empty">No lyrics found for this song.</div>"#.to_string()
    })
}

/// "Report lyrics" (`reason` "wrong_lyrics") or "Other timing" ("lyrics_sync"): the report dialog for the song
/// playing now with that reason ticked; once sent, its lyrics load again (the server has already looked for others).
pub(super) fn report_lyrics(reason: &str) {
    let Some(t) = now_track() else { return };
    if !t.g("id").is_number() { return; }
    crate::pages::report::report_modal("track", t.g("id").clone(), &t.s("title"), reason, Some(Rc::new(|_| {
        // forget the old answer so load_lyrics asks the server again
        LYR.with(|l| l.borrow_mut().id = Value::Null);
        load_lyrics();
    })));
}

/// Show the song's time-synced lines `ms` later (0: as stamped; clamped to ±30 s): they move at once, and the nudge
/// is saved for every device once the taps stop. A failed save puts the lines back and says why.
pub(super) fn nudge_lyrics_to(ms: i64) {
    let ms = ms.clamp(-NUDGE_MAX_MS, NUDGE_MAX_MS);
    let Some(id) = LYR.with(|l| { let l = l.borrow(); (!l.lines.is_empty() && l.id.is_number()).then(|| l.id.clone()) }) else { return };
    let before = LYR.with(|l| std::mem::replace(&mut l.borrow_mut().shift_ms, ms));
    let gen = NUDGE_GEN.with(|g| { g.set(g.get() + 1); g.get() });
    if let Some(b) = by_id("np-lyrics") { set_html(&b, &lyrics_html()); sync_lyrics(true); }
    dom::spawn(async move {
        dom::sleep(NUDGE_SAVE_AFTER_MS).await;
        // a newer tap saves instead; another song's lyrics are not the ones nudged
        if NUDGE_GEN.with(|g| g.get()) != gen || LYR.with(|l| l.borrow().id != id) { return; }
        let r = api::put(&format!("/library/tracks/{}/lyrics/shift", js_string(&id)), json!({ "shift_ms": ms })).await;
        if LYR.with(|l| l.borrow().id != id) { return; }
        match r {
            // the server's word: clamped, the .lrc's own [offset:] tag counted in
            Ok(v) => LYR.with(|l| l.borrow_mut().shift_ms = v.i("shift_ms")),
            Err(e) => {
                LYR.with(|l| l.borrow_mut().shift_ms = before);
                crate::ui::toast(&format!("Couldn't save the timing: {}", e.message), "err");
            }
        }
        if let Some(b) = by_id("np-lyrics") { set_html(&b, &lyrics_html()); sync_lyrics(true); }
    });
}

/// One click of Earlier (`dir` −1) or Later (+1): the nudge moves a quarter second ([`nudge_lyrics_to`]).
pub(super) fn nudge_lyrics(dir: i64) {
    let now = LYR.with(|l| l.borrow().shift_ms);
    nudge_lyrics_to(now + dir.signum() * NUDGE_STEP_MS);
}

pub(super) fn load_lyrics() {
    let t = now_track();
    let skip = match &t { None => true, Some(t) => t.truthy("preview") || !t.g("id").is_number() || LYR.with(|l| l.borrow().id == *t.g("id")) };
    if skip {
        if let Some(b) = by_id("np-lyrics") { set_html(&b, &lyrics_html()); }
        return;
    }
    let Some(t) = t else { return };
    let id = t.g("id").clone();
    LYR.with(|l| *l.borrow_mut() = Lyr { id: id.clone(), loading: true, ..Lyr::default() });
    if let Some(b) = by_id("np-lyrics") { set_html(&b, &lyrics_html()); }
    dom::spawn(async move {
        // without the server: the lyrics saved with the download
        let r = match api::get(&format!("/library/tracks/{}/lyrics", js_string(&id))).await {
            Ok(r) => Some(r),
            Err(_) => saved_lyrics(&id).await,
        };
        match r {
            Some(r) => {
                if LYR.with(|l| l.borrow().id != id) { return; }
                let synced = r.truthy("synced");
                LYR.with(|l| *l.borrow_mut() = Lyr {
                    id: id.clone(),
                    lines: if synced { parse_lrc(&r.s("lyrics")) } else { vec![] },
                    plain: if synced { String::new() } else { r.s("lyrics") },
                    loading: false,
                    instrumental: r.truthy("instrumental"),
                    // a copy saved with an older download has none: no nudge
                    shift_ms: if synced { r.i("shift_ms").clamp(-NUDGE_MAX_MS, NUDGE_MAX_MS) } else { 0 },
                });
            }
            None => { LYR.with(|l| *l.borrow_mut() = Lyr { id: id.clone(), ..Lyr::default() }); }
        }
        if let Some(b) = by_id("np-lyrics") { set_html(&b, &lyrics_html()); sync_lyrics(true); }
    });
}

pub(super) fn sync_lyrics(force: bool) {
    if !st(|s| s.full) { return; }
    let cur_id = now_track().map(|t| t.g("id").clone());
    let ok = LYR.with(|l| { let l = l.borrow(); !l.lines.is_empty() && cur_id.as_ref() == Some(&l.id) });
    if !ok { return; }
    let p = now_position() + 0.3;
    let i = LYR.with(|l| {
        let l = l.borrow();
        let mut i: i64 = -1;
        while ((i + 1) as usize) < l.lines.len() && l.at(&l.lines[(i + 1) as usize]) <= p { i += 1; }
        i
    });
    if i == LAST_LINE.with(|x| x.get()) && !force { return; }
    LAST_LINE.with(|x| x.set(i));
    let Some(b) = by_id("np-lyrics") else { return };
    for n in qsa_in(&b, "p.on") { dom::remove_class(&n, "on"); }
    // the lines already sung are dimmed (once per line, not per tick)
    for n in qsa_in(&b, "p[data-li]") { dom::toggle_class(&n, "past", num(&dom::data(&n, "li")) < i as f64); }
    if let Some(c) = qs_in(&b, &format!(r#"p[data-li="{i}"]"#)) {
        dom::add_class(&c, "on");
        let (top, h) = c.dyn_ref::<HtmlElement>().map(|h| (h.offset_top() as f64, h.offset_height() as f64)).unwrap_or((0.0, 0.0));
        let opts = ScrollToOptions::new();
        opts.set_top(top - b.client_height() as f64 / 2.0 + h / 2.0);
        opts.set_behavior(if force { ScrollBehavior::Auto } else { ScrollBehavior::Smooth });
        b.scroll_to_with_scroll_to_options(&opts);
    }
}
