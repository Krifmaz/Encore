//! Lyrics: reading .lrc stamps, loading a song's lyrics, and the line that follows the music.

use super::*;

thread_local! {
    pub(super) static LYR: RefCell<Lyr> = RefCell::new(Lyr::default());
    static LAST_LINE: Cell<i64> = const { Cell::new(-1) };
}

struct Line { t: f64, text: String }
/// `lyr = { id, lines, plain, loading, instrumental }`.
pub(super) struct Lyr { id: Value, lines: Vec<Line>, plain: String, loading: bool, instrumental: bool }
impl Default for Lyr {
    fn default() -> Self { Self { id: Value::Null, lines: vec![], plain: String::new(), loading: false, instrumental: false } }
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

pub(super) fn lyrics_html() -> String {
    let Some(t) = now_track() else { return String::new() };
    if t.truthy("preview") { return r#"<div class="muted np-empty">Lyrics are available for songs in your library.</div>"#.into(); }
    if !t.g("id").is_number() { return r#"<div class="muted np-empty">No lyrics for this song.</div>"#.into(); }
    LYR.with(|l| {
        let l = l.borrow();
        if l.loading || l.id != *t.g("id") { return r#"<div class="muted np-empty"><span class="spinner"></span> Looking for lyrics…</div>"#.to_string(); }
        if !l.lines.is_empty() {
            return l.lines.iter().enumerate().map(|(i, ln)| {
                let text = esc(&ln.text);
                format!(r#"<p data-t="{}" data-li="{i}">{}</p>"#, js_num(ln.t), if text.is_empty() { "♪" } else { &text })
            }).collect::<String>();
        }
        if !l.plain.is_empty() { return format!(r#"<div class="np-plain">{}</div>"#, esc(&l.plain)); }
        format!(r#"<div class="muted np-empty">{}</div>"#, if l.instrumental { "Instrumental" } else { "No lyrics found for this song." })
    })
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
    LYR.with(|l| *l.borrow_mut() = Lyr { id: id.clone(), lines: vec![], plain: String::new(), loading: true, instrumental: false });
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
                });
            }
            None => { LYR.with(|l| *l.borrow_mut() = Lyr { id: id.clone(), lines: vec![], plain: String::new(), loading: false, instrumental: false }); }
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
        while ((i + 1) as usize) < l.lines.len() && l.lines[(i + 1) as usize].t <= p { i += 1; }
        i
    });
    if i == LAST_LINE.with(|x| x.get()) && !force { return; }
    LAST_LINE.with(|x| x.set(i));
    let Some(b) = by_id("np-lyrics") else { return };
    for n in qsa_in(&b, "p.on") { dom::remove_class(&n, "on"); }
    if let Some(c) = qs_in(&b, &format!(r#"p[data-li="{i}"]"#)) {
        dom::add_class(&c, "on");
        let (top, h) = c.dyn_ref::<HtmlElement>().map(|h| (h.offset_top() as f64, h.offset_height() as f64)).unwrap_or((0.0, 0.0));
        let opts = ScrollToOptions::new();
        opts.set_top(top - b.client_height() as f64 / 2.0 + h / 2.0);
        opts.set_behavior(if force { ScrollBehavior::Auto } else { ScrollBehavior::Smooth });
        b.scroll_to_with_scroll_to_options(&opts);
    }
}
