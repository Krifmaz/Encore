//! The full-screen player: now playing, lyrics and Up next.

use super::*;

thread_local! {
    static FULL: RefCell<Vec<EventListener>> = const { RefCell::new(Vec::new()) };
}

fn set_full_listeners(ls: Vec<EventListener>) { let old = FULL.with(|b| std::mem::replace(&mut *b.borrow_mut(), ls)); drop(old); }

pub fn set_full(on: bool) {
    st_mut(|s| s.full = on);
    let Some(f) = by_id("np-full") else { return };
    dom::toggle_class(&f, "hidden", !on);
    dom::toggle_class(&body(), "np-open", on);
    if on { render_full(); load_lyrics(); }
}

pub(super) fn render_full() {
    let Some(f) = by_id("np-full") else { return };
    let link = linked();
    let Some(t) = now_track() else {
        set_html(&f, &format!(r#"<div class="np-top"><button class="btn ghost icon" id="f-close" aria-label="Close">{}</button><span></span><span></span></div>
      <div class="empty">{}<h2>Nothing playing on {}</h2><p class="muted">Play anything – it starts there.</p></div>"#,
            icon("x"), icon("devices"), esc(&link.as_ref().map(|l| l.name.clone()).unwrap_or_else(|| "this device".into()))));
        let mut ls = Vec::new();
        wire(&mut ls, &f, "#f-close", "click", |_| set_full(false));
        set_full_listeners(ls);
        return;
    };
    let s = link.as_ref().map(|l| l.state.clone()).unwrap_or(json!({}));
    let (idx, pane, repeat, shuffle_on, q): (i64, String, String, bool, Vec<Value>) = if link.is_some() {
        let r = s.s("repeat");
        st(|x| (if s.is_nullish("index") { -1 } else { s.i("index") }, x.pane.clone(), if ["off", "all", "one"].contains(&r.as_str()) { r } else { "off".into() }, s.truthy("shuffle"), s.a("queue")))
    } else {
        st(|x| (x.index, x.pane.clone(), x.repeat.clone(), x.shuffle, x.queue.iter().map(|t| (**t).clone()).collect()))
    };
    let up_next: Vec<Value> = q.iter().skip((idx + 1).max(0) as usize).cloned().collect();
    let playing = now_playing();
    let volume = if link.is_some() { if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") } } else { js_num(user_vol()) };
    let autoplay = prefs().truthy("autoplay");
    dom::set_style(&f, "--np-bg", &format!(r#"url("{}")"#, cover_at(&t, 400)));
    let tab = |p: &str, label: &str| format!(r#"<button data-pane="{p}" class="{}">{label}</button>"#, if pane == p { "active" } else { "" });
    let artist = if t.truthy("preview") { esc(&crate::ui::tr_inline(&t, "artist")) } else { crate::ui::artist_links(&t) };
    let album = if t.truthy("album") {
        format!(" · {}", if t.truthy("album_id") { format!(r##"<a href="#/library/album/{}">{}</a>"##, esck(&t, "album_id"), esc(&crate::ui::tr_inline(&t, "album"))) } else { esc(&crate::ui::tr_inline(&t, "album")) })
    } else { String::new() };
    let device_btn = if has_encore_remote() {
        format!(r#"<button class="btn sm ghost np-device" id="f-device" title="Choose where the music plays">{}<span class="hide-sm">{}</span>{}</button>"#,
            icon("devices"), match &link { Some(l) => format!("Playing on {}", esc(&l.name)), None => "This device".into() }, icon("chevron"))
    } else { "<span></span>".to_string() };
    let list = if !up_next.is_empty() {
        up_next.iter().take(200).enumerate().map(|(k, x)| {
            let i = idx + 1 + k as i64;
            let btns = if link.is_some() { String::new() } else {
                format!(r#"<button class="btn ghost icon sm" data-up="{i}" aria-label="Move up" {}>↑</button>
            <button class="btn ghost icon sm" data-rm="{i}" aria-label="Remove">{}</button>"#, if i == idx + 1 { "disabled" } else { "" }, icon("x"))
            };
            format!(r#"<div class="np-item" data-i="{i}">
            <div class="thumb">{}</div>
            <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{}</div><div class="small muted ellipsis">{}</div></div>
            {btns}</div>"#, crate::ui::art_sized(&cover(x), &album_or_title(x), "", 200), esck(x, "title"), if x.truthy("preview") { esck(x, "artist") } else { crate::ui::artist_links(x) })
        }).collect::<String>()
    } else {
        format!(r#"<div class="muted small" style="padding:16px 4px">{}</div>"#,
            if link.is_some() { "Nothing queued there." } else if autoplay { "Nothing queued – a shuffle of your library plays next." } else { "Nothing queued." })
    };
    let actions = format!(r#"<div class="np-actions">
          {}
          {}
          <button class="btn ghost icon {}" id="f-lyrics" aria-label="Lyrics" title="Lyrics">{}</button>
          <button class="btn ghost icon" id="f-more" aria-label="More" title="More">{}</button>
        </div>"#,
        if can_like(Some(&t)) { like_button("f-like", if is_liked(&t) { "on" } else { "" }) } else { String::new() },
        if can_like(Some(&t)) { format!(r#"<button class="btn ghost icon" id="f-share" aria-label="Share" title="Share link">{}</button>"#, icon("external")) } else { String::new() },
        if pane == "lyrics" { "on" } else { "" }, icon("music"),
        icon("more"));
    let on_name = link.as_ref().map(|l| format!(" on {}", esc(&l.name))).unwrap_or_default();
    let shuffle = format!(r#"<button class="btn ghost icon {}" id="f-shuffle" title="{}" aria-label="Shuffle" aria-pressed="{shuffle_on}">{}</button>"#,
        if shuffle_on { "on" } else { "" }, if shuffle_on { "Shuffle: on" } else { "Shuffle: off" }, icon("shuffle"));
    let repeat_btn = format!(r#"<button class="btn ghost icon {}" id="f-repeat" title="{}" aria-label="Repeat" aria-pressed="{}">{}{}</button>"#,
        if repeat != "off" { "on" } else { "" }, repeat_title(&repeat), repeat != "off", icon("refresh"), if repeat == "one" { "<sup>1</sup>" } else { "" });
    let auto = if link.is_some() { String::new() } else { format!(r#"<label class="row small muted" style="gap:6px" title="When the queue ends, keep playing a shuffle of the whole library"><input type="checkbox" id="f-auto" {}> Then shuffle library</label>"#, if autoplay { "checked" } else { "" }) };
    set_html(&f, &format!(r##"
    <div class="np-backdrop"></div>
    <div class="np-top">
      <button class="btn ghost icon" id="f-close" aria-label="Close">{x}</button>
      <div class="tabs np-tabs">{tab_now}{tab_lyrics}{tab_queue}</div>
      {device_btn}
    </div>
    <div class="np-body pane-{pane}">
      <section class="np-main">
        <div class="np-art">{big_art}</div>
        <div class="np-meta">
          <h2 class="ellipsis">{title}</h2>{title_sub}
          <div class="muted ellipsis">{artist}{album}</div>
          <button class="badge mt np-quality" id="f-signal" title="Signal path: what the file is, what Encore sends and what this device does with it">{quality}</button>
        </div>
        <div class="seek np-seek"><span id="f-cur">0:00</span><input type="range" id="fseek" min="0" max="1000" value="0"><span id="f-dur">0:00</span></div>
        <div class="np-ctrls">
          {shuffle}
          <button class="btn ghost icon" id="f-prev" aria-label="Previous">{prev}</button>
          <button class="play-btn lg" id="f-toggle" aria-label="Play/Pause">{pp}</button>
          <button class="btn ghost icon" id="f-next" aria-label="Next">{next}</button>
          {repeat}
        </div>
        <div class="row np-vol"><span class="faint">{volume_ic}</span><input type="range" id="fvol" min="0" max="1" step="0.01" value="{vol}"></div>
        {actions}
      </section>
      <section class="np-lyrics" id="np-lyrics">{lyrics}</section>
      <section class="np-queue">
        <div class="row between"><h3>Up next{on_name}</h3>
          {auto}</div>
        <div class="np-list">{list}</div>
      </section>
    </div>"##,
        x = icon("x"), tab_now = tab("now", "Now playing"), tab_lyrics = tab("lyrics", "Lyrics"), tab_queue = tab("queue", "Up next"),
        big_art = art(&cover(&t), &album_or_title(&t), "big"), title = esc(&crate::ui::tr(&t, "title")), title_sub = crate::ui::tr_sub(&t, "title"), quality = esc(&quality(&t)),
        shuffle = shuffle, prev = icon("prev"), pp = if playing { icon("pause") } else { icon("play") }, next = icon("next"), repeat = repeat_btn,
        volume_ic = icon_attr("volume", r#"width="18""#), vol = volume, lyrics = lyrics_html(),
        on_name = on_name, auto = auto));
    let mut ls = Vec::new();
    wire(&mut ls, &f, "#f-close", "click", |_| set_full(false));
    wire(&mut ls, &f, ".np-tabs", "click", |e| {
        if let Some(b) = dom::closest(e, "[data-pane]") {
            let p = dom::data(&b, "pane");
            st_mut(|s| s.pane = p.clone());
            render_full();
            if p == "lyrics" { load_lyrics(); }
        }
    });
    wire(&mut ls, &f, "#f-device", "click", |_| remote_open());
    wire(&mut ls, &f, "#f-signal", "click", |_| dom::spawn(signal_path()));
    wire(&mut ls, &f, "#f-toggle", "click", |_| toggle());
    wire(&mut ls, &f, "#f-next", "click", |_| next(false));
    wire(&mut ls, &f, "#f-prev", "click", |_| prev());
    wire(&mut ls, &f, "#f-shuffle", "click", move |_| set_shuffle(!shuffle_on));
    { let r = repeat.clone(); wire(&mut ls, &f, "#f-repeat", "click", move |_| set_repeat(next_repeat(&r))); }
    wire(&mut ls, &f, "#f-lyrics", "click", |_| {
        let p = if st(|s| s.pane == "lyrics") { "now" } else { "lyrics" };
        st_mut(|s| s.pane = p.into());
        render_full();
        if p == "lyrics" { load_lyrics(); }
    });
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
    { let t2 = t.clone(); wire(&mut ls, &f, "#f-share", "click", move |_| dom::spawn(share_track(t2.clone()))); }
    wire(&mut ls, &f, "#f-more", "click", |e| { if let Some(b) = dom::closest(e, "#f-more") { more_menu(&b); } });
    if let Some(seek) = qs_in(&f, "#fseek") { bind_seek(&mut ls, &seek, "f-cur"); }
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
    wire(&mut ls, &f, "#np-lyrics", "click", |e| { if let Some(l) = dom::closest(e, "[data-t]") { seek_to(num(&dom::data(&l, "t"))); } });
    set_full_listeners(ls);
    warm_likes(Some(&t));
    update_time();
    sync_lyrics(true);
}
