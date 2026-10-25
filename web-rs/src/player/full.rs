//! The full-screen player: now playing, lyrics and Up next.
//!
//! Lyrics mode (the Lyrics tab, the Lyrics button, ⋯ → Lyrics; remembered on this device as `encore.lyricsMode`): the
//! big cover gives way to a compact bar at the top – a 56 px thumbnail, the title and artist, play / pause and a thin
//! seek line – and the lyrics take the column, the line sung now in the middle, the ones already sung dimmed; a tap on a
//! line seeks there (synced lyrics). "Back to the cover", a tap on the compact bar (thumbnail, title, artist; play / pause still just pauses), the Now playing tab and Escape go back.
//! Switching panes only changes classes (`paint_pane`): the view is not drawn again, so no cover flashes.

use super::*;

/// Lyrics mode, remembered on this device (client-shapes §5.2; the phone keeps the same switch).
const LYRICS_MODE: &str = "encore.lyricsMode";
pub(super) fn lyrics_mode() -> bool { matches!(dom::ls_get(LYRICS_MODE).as_deref(), Some("true" | "1")) }
fn set_lyrics_mode(on: bool) { dom::ls_set(LYRICS_MODE, if on { "true" } else { "false" }); }

thread_local! {
    static FULL: RefCell<Vec<EventListener>> = const { RefCell::new(Vec::new()) };
}

fn set_full_listeners(ls: Vec<EventListener>) { let old = FULL.with(|b| std::mem::replace(&mut *b.borrow_mut(), ls)); drop(old); }

thread_local! {
    /// The HTML the bar and the full player were last drawn with, the play/pause icon left out (`PP`). A play or a
    /// pause then only swaps that icon: drawing a view again re-creates every cover in it, and each one fades in
    /// over its colour again – the flash on every pause.
    static DRAWN: RefCell<[Option<String>; 2]> = const { RefCell::new([None, None]) };
}

/// Where the play/pause icon goes in a view's HTML, for `draw`.
pub(super) const PP: &str = "\u{1}pp\u{1}";

#[derive(Clone, Copy)]
pub(super) enum View { Bar = 0, Full = 1 }

/// Draws `html` (with `PP` where the play/pause icon goes) into `el`, unless the view already shows exactly that:
/// then only the buttons `toggle` selects get the icon `pp`. True when the view was drawn, so its listeners need wiring
/// again (and its long titles measuring, marquee.rs).
pub(super) fn draw(view: View, el: &Element, toggle: &str, html: &str, pp: &str) -> bool {
    if DRAWN.with(|d| d.borrow()[view as usize].as_deref() == Some(html)) {
        for b in qsa_in(el, toggle) { set_html(&b, pp); }
        return false;
    }
    set_html(el, &html.replace(PP, pp));
    DRAWN.with(|d| d.borrow_mut()[view as usize] = Some(html.to_string()));
    true
}

/// The view was drawn some other way (the empty or the linked state): the next `draw` draws it in full.
pub(super) fn forget(view: View) { DRAWN.with(|d| d.borrow_mut()[view as usize] = None); }

pub fn set_full(on: bool) {
    st_mut(|s| s.full = on);
    let Some(f) = by_id("np-full") else { return };
    dom::toggle_class(&f, "hidden", !on);
    dom::toggle_class(&body(), "np-open", on);
    if on {
        render_full();
        load_lyrics();
        // shown now: what was drawn while hidden is measured and the lyrics find their line
        measure_titles();
        sync_lyrics(true);
    }
}

/// Open the full-screen player: "lyrics" (lyrics mode, remembered), "queue" (Up next), or "now" – the cover, or the
/// lyrics when lyrics mode was on last time.
pub(super) fn open_full(pane: &str) {
    let p = match pane {
        "lyrics" => { set_lyrics_mode(true); "lyrics" }
        "queue" => "queue",
        _ => if lyrics_mode() { "lyrics" } else { "now" },
    };
    st_mut(|s| s.pane = p.into());
    set_full(true);
}

/// Another pane of the open player – only classes change, nothing is drawn again. Lyrics turns lyrics mode on and
/// Now playing turns it off (remembered); Up next leaves it as it is.
pub(super) fn set_pane(p: &str) {
    if p == "lyrics" { set_lyrics_mode(true); } else if p == "now" { set_lyrics_mode(false); }
    st_mut(|s| s.pane = p.into());
    let Some(f) = by_id("np-full") else { return };
    paint_pane(&f);
    if p == "lyrics" { load_lyrics(); sync_lyrics(true); }
    measure_titles();
}

/// The pane's classes: the body's layout, the active tab and the Lyrics button.
fn paint_pane(f: &Element) {
    let pane = st(|s| s.pane.clone());
    if let Some(b) = qs_in(f, ".np-body") {
        for p in ["now", "lyrics", "queue"] { dom::toggle_class(&b, &format!("pane-{p}"), p == pane); }
    }
    for t in qsa_in(f, ".np-tabs [data-pane]") { dom::toggle_class(&t, "active", dom::data(&t, "pane") == pane); }
    if let Some(l) = qs_in(f, "#f-lyrics") { dom::toggle_class(&l, "on", pane == "lyrics"); }
}

pub(super) fn render_full() {
    let Some(f) = by_id("np-full") else { return };
    let link = linked();
    let Some(t) = now_track() else {
        forget(View::Full);
        set_html(&f, &format!(r#"<div class="np-top"><button class="btn ghost icon" id="f-close" aria-label="Close">{}</button><span></span><span></span></div>
      <div class="empty">{}<h2>Nothing playing on {}</h2><p class="muted">Play anything – it starts there.</p></div>"#,
            icon("x"), icon("devices"), esc(&link.as_ref().map(|l| l.name.clone()).unwrap_or_else(|| "this device".into()))));
        let mut ls = Vec::new();
        wire(&mut ls, &f, "#f-close", "click", |_| set_full(false));
        set_full_listeners(ls);
        return;
    };
    let s = link.as_ref().map(|l| l.state.clone()).unwrap_or(json!({}));
    let (idx, repeat, shuffle_on, q): (i64, String, bool, Vec<Value>) = if link.is_some() {
        let r = s.s("repeat");
        (if s.is_nullish("index") { -1 } else { s.i("index") }, if ["off", "all", "one"].contains(&r.as_str()) { r } else { "off".into() }, s.truthy("shuffle"), s.a("queue"))
    } else {
        st(|x| (x.index, x.repeat.clone(), x.shuffle, x.queue.iter().map(|t| (**t).clone()).collect()))
    };
    let up_next: Vec<Value> = q.iter().skip((idx + 1).max(0) as usize).cloned().collect();
    let playing = now_playing();
    let volume = if link.is_some() { if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") } } else { js_num(user_vol()) };
    let autoplay = prefs().truthy("autoplay");
    dom::set_style(&f, "--np-bg", &format!(r#"url("{}")"#, cover_at(&t, 400)));
    // the pane is left out of the HTML (paint_pane sets it), so switching panes draws nothing again
    let tab = |p: &str, label: &str| format!(r#"<button data-pane="{p}">{label}</button>"#);
    let artist = if t.truthy("preview") { esc(&crate::ui::tr_inline(&t, "artist")) } else { crate::ui::artist_links(&t) };
    let album = if t.truthy("album") {
        format!(" · {}", if t.truthy("album_id") { format!(r##"<a href="#/library/album/{}">{}</a>"##, esck(&t, "album_id"), esc(&crate::ui::tr_inline(&t, "album"))) } else { esc(&crate::ui::tr_inline(&t, "album")) })
    } else { String::new() };
    let device_btn = if has_encore_remote() {
        format!(r#"<button class="btn sm ghost np-device" id="f-device" title="Choose where the music plays">{}<span class="hide-sm">{}</span>{}</button>"#,
            icon("devices"), match &link { Some(l) => format!("Playing on {}", esc(&l.name)), None => "This device".into() }, icon("chevron"))
    } else { "<span></span>".to_string() };
    // in a group listen: who added each song, and the queue's controls only for those who control it
    let group = grouped() && link.is_none();
    let list = if !up_next.is_empty() {
        up_next.iter().take(200).enumerate().map(|(k, x)| {
            let i = idx + 1 + k as i64;
            let btns = if link.is_some() || (group && !crate::group::can_control()) { String::new() } else {
                format!(r#"<button class="btn ghost icon sm" data-up="{i}" aria-label="Move up" {}>↑</button>
            <button class="btn ghost icon sm" data-rm="{i}" aria-label="Remove">{}</button>"#, if i == idx + 1 { "disabled" } else { "" }, icon("x"))
            };
            format!(r#"<div class="np-item" data-i="{i}">
            <div class="thumb">{}</div>
            <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{}</div><div class="small muted ellipsis">{}</div>{}</div>
            {btns}</div>"#, crate::ui::art_sized(&cover(x), &album_or_title(x), "", 200), esck(x, "title"), if x.truthy("preview") { esck(x, "artist") } else { crate::ui::artist_links(x) },
                if group { added_by(x) } else { String::new() })
        }).collect::<String>()
    } else {
        format!(r#"<div class="muted small" style="padding:16px 4px">{}</div>"#,
            if link.is_some() { "Nothing queued there." } else if group { "Nothing queued. Anyone in the group can add songs with Add to queue." } else if autoplay { "Nothing queued – a shuffle of your library plays next." } else { "Nothing queued." })
    };
    let actions = format!(r#"<div class="np-actions">
          {}
          {}
          <button class="btn ghost icon" id="f-lyrics" aria-label="Lyrics" title="Lyrics">{}</button>
          <button class="btn ghost icon" id="f-more" aria-label="More" title="More">{}</button>
        </div>"#,
        if can_like(Some(&t)) { like_button("f-like", if is_liked(&t) { "on" } else { "" }) + &dislike_button("f-dislike", if is_disliked(&t) { "on" } else { "" }) } else { String::new() },
        if can_like(Some(&t)) { format!(r#"<button class="btn ghost icon" id="f-share" aria-label="Share" title="Share link">{}</button>"#, icon("external")) } else { String::new() },
        icon("music"),
        icon("more"));
    let on_name = link.as_ref().map(|l| format!(" on {}", esc(&l.name))).unwrap_or_default();
    let shuffle = format!(r#"<button class="btn ghost icon {}" id="f-shuffle" title="{}" aria-label="Shuffle" aria-pressed="{shuffle_on}"{}>{}</button>"#,
        if shuffle_on { "on" } else { "" }, shuffle_tip(if shuffle_on { "Shuffle: on" } else { "Shuffle: off" }), shuffle_attr(), icon("shuffle"));
    let repeat_btn = format!(r#"<button class="btn ghost icon {}" id="f-repeat" title="{}" aria-label="Repeat" aria-pressed="{}"{}>{}{}</button>"#,
        if repeat != "off" { "on" } else { "" }, repeat_tip(repeat_title(&repeat)), repeat != "off", repeat_attr(), icon("refresh"), if repeat == "one" { "<sup>1</sup>" } else { "" });
    let auto = if link.is_some() { String::new() } else if group { queue_note() } else { format!(r#"<label class="row small muted" style="gap:6px" title="When the queue ends, keep playing a shuffle of the whole library"><input type="checkbox" id="f-auto" {}> Then shuffle library</label>"#, if autoplay { "checked" } else { "" }) };
    let html = format!(r##"
    <div class="np-backdrop"></div>
    <div class="np-top">
      <button class="btn ghost icon" id="f-close" aria-label="Close">{x}</button>
      <div class="tabs np-tabs">{tab_now}{tab_lyrics}{tab_queue}</div>
      <div class="np-top-r">{pill}{device_btn}</div>
    </div>
    <div class="np-body">
      <section class="np-main">
        <div class="np-art">{big_art}</div>
        <div class="np-meta">
          <h2 class="ellipsis mq"><span class="mq-in">{title}</span></h2>{title_sub}
          <div class="muted ellipsis mq"><span class="mq-in">{artist}{album}</span></div>
          <button class="badge mt np-quality" id="f-signal" title="Signal path: what the file is, what Encore sends and what this device does with it">{quality}</button>
          {paused}
        </div>
        <div class="seek np-seek"{ctl_title}><span id="f-cur">0:00</span><input type="range" id="fseek" min="0" max="1000" value="0"{ctl}><span id="f-dur">0:00</span></div>
        <div class="np-ctrls">
          {shuffle}
          <button class="btn ghost icon" id="f-prev" aria-label="Previous"{ctl}>{prev}</button>
          <button class="play-btn lg" id="f-toggle" aria-label="Play/Pause">{pp}</button>
          <button class="btn ghost icon" id="f-next" aria-label="Next"{ctl}>{next}</button>
          {repeat}
        </div>
        <div class="row np-vol"><span class="faint">{volume_ic}</span><input type="range" id="fvol" min="0" max="1" step="0.01" value="{vol}"></div>
        {actions}
      </section>
      <section class="np-mini">
        <button type="button" class="np-mini-art" id="f-cover" aria-label="Back to the cover" title="Back to the cover"><div class="thumb">{mini_art}</div></button>
        <div class="np-mini-meta" id="f-mini-meta" title="Back to the cover">
          <div class="ellipsis mq np-mini-title"><span class="mq-in">{title}</span></div>
          <div class="ellipsis mq small muted"><span class="mq-in">{mini_artist}</span></div>
        </div>
        <button class="play-btn" id="f-mini-toggle" aria-label="Play/Pause">{pp}</button>
        <input type="range" id="lseek" min="0" max="1000" value="0" aria-label="Seek"{ctl}>
      </section>
      <section class="np-lyrics" id="np-lyrics"></section>
      <div class="np-back"><button type="button" class="btn sm" id="f-back">Back to the cover</button></div>
      <section class="np-queue">
        <div class="row between"><h3>Up next{on_name}</h3>
          <div class="row wrap np-q-acts">{q_btns}{auto}</div></div>
        <div class="np-list">{list}</div>
      </section>
    </div>"##,
        x = icon("x"), tab_now = tab("now", "Now playing"), tab_lyrics = tab("lyrics", "Lyrics"), tab_queue = tab("queue", "Up next"),
        big_art = art(&cover(&t), &album_or_title(&t), "big"), title = esc(&crate::ui::tr(&t, "title")), title_sub = crate::ui::tr_sub(&t, "title"), quality = esc(&quality(&t)),
        mini_art = crate::ui::art_sized(&cover(&t), &album_or_title(&t), "", 200), mini_artist = esc(&crate::ui::tr_inline(&t, "artist")),
        shuffle = shuffle, prev = icon("prev"), pp = PP, next = icon("next"), repeat = repeat_btn,
        volume_ic = icon_attr("volume", r#"width="18""#), vol = volume,
        on_name = on_name, auto = auto, q_btns = queue_buttons(link.is_some(), up_next.len()),
        pill = if link.is_none() { pill_html("f-group") } else { String::new() }, paused = paused_html("f-catchup"), ctl = ctl_attr(), ctl_title = ctl_attr().replace(" disabled", ""));
    let drawn = draw(View::Full, &f, "#f-toggle, #f-mini-toggle", &html, &if playing { icon("pause") } else { icon("play") });
    show_paused(&f, "f-catchup");
    paint_pane(&f);
    if !drawn {
        update_time();
        return;
    }
    // the lyrics fill in by themselves as they load (lyrics.rs), so they stay out of the HTML `draw` compares
    if let Some(l) = qs_in(&f, "#np-lyrics") { set_html(&l, &lyrics_html()); }
    let mut ls = Vec::new();
    wire(&mut ls, &f, "#f-close", "click", |_| set_full(false));
    wire(&mut ls, &f, ".np-tabs", "click", |e| {
        if let Some(b) = dom::closest(e, "[data-pane]") { set_pane(&dom::data(&b, "pane")); }
    });
    wire(&mut ls, &f, "#f-device", "click", |_| remote_open());
    wire_group(&mut ls, &f, "#f-group", "#f-catchup");
    wire(&mut ls, &f, "#f-signal", "click", |_| dom::spawn(signal_path()));
    wire(&mut ls, &f, "#f-toggle", "click", |_| toggle());
    wire(&mut ls, &f, "#f-mini-toggle", "click", |_| toggle());
    wire(&mut ls, &f, "#f-cover", "click", |_| set_pane("now"));
    wire(&mut ls, &f, "#f-back", "click", |_| set_pane("now"));
    wire(&mut ls, &f, "#f-mini-meta", "click", |_| set_pane("now"));
    wire(&mut ls, &f, "#f-next", "click", |_| next(false));
    wire(&mut ls, &f, "#f-prev", "click", |_| prev());
    wire(&mut ls, &f, "#f-shuffle", "click", move |_| set_shuffle(!shuffle_on));
    { let r = repeat.clone(); wire(&mut ls, &f, "#f-repeat", "click", move |_| set_repeat(next_repeat(&r))); }
    wire(&mut ls, &f, "#f-lyrics", "click", |_| set_pane(if st(|s| s.pane == "lyrics") { "now" } else { "lyrics" }));
    wire(&mut ls, &f, "#f-q-clear", "click", |_| clear_up_next());
    wire(&mut ls, &f, "#f-q-shuffle", "click", |_| { g_shuffle(); });
    wire(&mut ls, &f, "#f-auto", "change", |e| {
        let on = dom::target_el(e).map(|c| dom::checked(&c)).unwrap_or(false);
        let mut p = prefs();
        p["autoplay"] = json!(on);
        let server_prefs = app::has_feature("theme");
        dom::spawn(async move { playback::save(&p, server_prefs).await; });
    });
    if link.is_some() {
        wire(&mut ls, &f, "#fvol", "change", |e| { if let Some(v) = dom::target_el(e) { set_volume(num(&dom::value(&v))); } });
    } else {
        wire(&mut ls, &f, "#fvol", "input", |e| { if let Some(v) = dom::target_el(e) { local_volume(num(&dom::value(&v))); } });
    }
    { let t2 = t.clone(); wire(&mut ls, &f, "#f-like", "click", move |_| dom::spawn(toggle_like(t2.clone()))); }
    { let t2 = t.clone(); wire(&mut ls, &f, "#f-dislike", "click", move |_| dom::spawn(toggle_dislike(t2.clone()))); }
    { let t2 = t.clone(); wire(&mut ls, &f, "#f-share", "click", move |_| dom::spawn(share_track(t2.clone()))); }
    wire(&mut ls, &f, "#f-more", "click", |e| { if let Some(b) = dom::closest(e, "#f-more") { more_menu(&b); } });
    if let Some(seek) = qs_in(&f, "#fseek") { bind_seek(&mut ls, &seek, "f-cur"); }
    if let Some(seek) = qs_in(&f, "#lseek") { bind_seek(&mut ls, &seek, ""); }
    let queue_for_link = s.a("queue");
    let is_link = link.is_some();
    wire(&mut ls, &f, ".np-list", "click", move |e| {
        if dom::closest(e, "a[href]").is_some() { set_full(false); return; }
        if is_link {
            if let Some(it) = dom::closest(e, "[data-i]") {
                if !queue_for_link.is_empty() { via_link(json!({ "action": "play_queue", "tracks": queue_for_link, "index": num(&dom::data(&it, "i")) as i64 })); }
            }
            return;
        }
        if let Some(rm) = dom::closest(e, "[data-rm]") { remove_at(num(&dom::data(&rm, "rm")) as i64); return; }
        if let Some(up) = dom::closest(e, "[data-up]") { let i = num(&dom::data(&up, "up")) as i64; move_to(i, i - 1); return; }
        if let Some(it) = dom::closest(e, "[data-i]") { jump(num(&dom::data(&it, "i")) as i64); }
    });
    wire(&mut ls, &f, "#np-lyrics", "click", |e| {
        // "Report lyrics" under the lyrics, else a line: seek there
        if dom::closest(e, "[data-lyrics-report]").is_some() { report_lyrics(); return; }
        if let Some(l) = dom::closest(e, "[data-t]") { seek_to(num(&dom::data(&l, "t"))); }
    });
    set_full_listeners(ls);
    warm_likes(Some(&t));
    warm_dislikes(Some(&t));
    update_time();
    sync_lyrics(true);
    measure_titles();
}
