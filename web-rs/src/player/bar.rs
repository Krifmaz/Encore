//! The player bar at the bottom: its buttons (and "Customize the bar…"), the seek bar and times, the quality
//! badge, likes and the ⋯ menu for the song in front of you.

use super::*;

thread_local! {
    /// the `onclick = …` handlers of the bar / the full-screen view: replaced on every render, like the elements
    static BAR: RefCell<Vec<EventListener>> = const { RefCell::new(Vec::new()) };
    /// liked track ids, fetched the first time a heart is drawn
    pub(super) static LIKED: RefCell<Option<HashSet<i64>>> = const { RefCell::new(None) };
}

fn set_bar(ls: Vec<EventListener>) { let old = BAR.with(|b| std::mem::replace(&mut *b.borrow_mut(), ls)); drop(old); }

/// `seek.style.setProperty('--p', `${seek.value / 10}%`)`.
fn set_p(seek: &Element) { dom::set_style(seek, "--p", &format!("{}%", js_num(num(&dom::value(seek)) / 10.0))); }

// ---- likes, sharing and the ⋯ menu for the song in front of you -------------------------------------
pub(super) async fn likes() -> HashSet<i64> {
    if LIKED.with(|l| l.borrow().is_none()) {
        let set: HashSet<i64> = match api::get("/likes").await {
            Ok(v) => { let s = v.items().iter().filter_map(|x| x.as_i64()).collect(); keep_likes(&s); s }
            // no server: the hearts as this device last knew them
            Err(e) if unreachable(e.status) => kept_likes(),
            Err(_) => HashSet::new(),
        };
        LIKED.with(|l| *l.borrow_mut() = Some(set));
    }
    LIKED.with(|l| l.borrow().clone().unwrap_or_default())
}
pub(super) fn is_liked(t: &Value) -> bool { t.g("id").is_number() && LIKED.with(|l| l.borrow().as_ref().is_some_and(|s| s.contains(&t.i("id")))) }
pub(super) fn can_like(t: Option<&Value>) -> bool { t.is_some_and(|t| t.g("id").is_number()) }
pub(super) async fn toggle_like(t: Value) {
    if !t.g("id").is_number() { return; }
    let id = t.i("id");
    let s = likes().await;
    let liked = !s.contains(&id);
    // no server: the change waits in the outbox and the heart answers now. A change of this song still waiting
    // there (the server was away a moment ago): this one goes after it through the outbox, so the server sees
    // them in order instead of the older one last.
    let waiting = crate::offline::outbox::has_like(id);
    if no_server() || waiting {
        like_later(id, liked);
        if waiting { sync_outbox(); }
        toast(if liked { "Added to Liked Songs" } else { "Removed from Liked Songs" }, "ok");
        paint_likes();
        return;
    }
    let r = if s.contains(&id) {
        api::del(&format!("/likes/{id}")).await.map(|_| { LIKED.with(|l| if let Some(s) = l.borrow_mut().as_mut() { s.remove(&id); }); toast("Removed from Liked Songs", "ok"); })
    } else {
        api::post_empty(&format!("/likes/{id}")).await.map(|_| { LIKED.with(|l| if let Some(s) = l.borrow_mut().as_mut() { s.insert(id); }); toast("Added to Liked Songs", "ok"); })
    };
    match r {
        Ok(_) => { if let Some(s) = LIKED.with(|l| l.borrow().clone()) { keep_likes(&s); } sync_outbox(); }
        Err(e) if unreachable(e.status) => { like_later(id, liked); toast(if liked { "Added to Liked Songs" } else { "Removed from Liked Songs" }, "ok"); paint_likes(); return; }
        Err(e) => toast(&e.message, "err"),
    }
    dom::dispatch("encore:likes", None);
    paint_likes();
}
/// Update every heart on screen without redrawing the whole bar.
pub(super) fn paint_likes() {
    let t = now_track();
    let on = t.as_ref().is_some_and(is_liked);
    for b in dom::qsa(".btn.like") {
        dom::toggle_class(&b, "on", on);
        let _ = b.set_attribute("title", if on { "Liked – tap to remove" } else { "Like" });
    }
}
pub(super) fn like_button(id: &str, cls: &str) -> String {
    format!(r#"<button class="btn ghost icon like {cls}" id="{id}" aria-label="Like" title="Like">{}</button>"#, icon("heart"))
}
/// `if (liked === null && canLike(t)) likes().then(paintLikes)`.
pub(super) fn warm_likes(t: Option<&Value>) {
    if LIKED.with(|l| l.borrow().is_none()) && can_like(t) { dom::spawn(async { likes().await; paint_likes(); }); }
}

/// "⋯" for the song in front of you (this device's or the linked device's).
pub(super) fn more_menu(anchor: &Element) {
    let Some(t) = now_track() else { return };
    let lib = t.g("id").is_number();
    let linked = is_linked();
    let mut items: Vec<Option<MenuItem>> = vec![];
    if t.truthy("artist") && !t.truthy("preview") { items.push(MenuItem::link("Go to artist", &icon("user"), &crate::ui::artist_url(&t.a("artists").iter().find(|a| a.truthy("name")).cloned().unwrap_or_else(|| serde_json::json!({ "name": t.s("artist") }))), false)); }
    if t.truthy("album_id") { items.push(MenuItem::link("View album", &icon("disc"), &format!("#/library/album/{}", t.s("album_id")), false)); }
    if lib { let t2 = t.clone(); items.push(MenuItem::new(if is_liked(&t) { "Remove from Liked Songs" } else { "Like" }, &icon("heart"), move || dom::spawn(toggle_like(t2.clone())))); }
    if lib { let id = t.i("id"); items.push(MenuItem::new("Add to playlist", &icon("plus"), move || crate::pages::playlists::add_to_playlist_modal(vec![id]))); }
    if lib { items.push(MenuItem::new("Lyrics", &icon("music"), || { st_mut(|s| s.pane = "lyrics".into()); set_full(true); })); }
    if lib { let t2 = t.clone(); items.push(MenuItem::new("Share link", &icon("external"), move || dom::spawn(share_track(t2.clone())))); }
    if lib { items.push(MenuItem::link("Download this song", &icon("download"), &format!("/api/library/tracks/{}/file", t.s("id")), true)); }
    if lib && !linked && can_save_offline() { let t2 = t.clone(); items.push(MenuItem::new("Save for offline", &icon("download"), move || { save_offline(vec![t2.clone()]); toast("Saving to this device…", "ok"); })); }
    if lib && !linked { items.extend(crate::pages::downloads::menu_items(std::slice::from_ref(&t))); }
    items.push(MenuItem::new("Signal path", &icon("activity"), || dom::spawn(signal_path())));
    if native::has_mini_player() { items.push(MenuItem::new("Mini player", &icon("music"), native::mini_player)); }
    if lib { let t2 = t.clone(); items.push(MenuItem::new("Report a problem", &icon("shield"), move || crate::pages::report::report_modal("track", t2.g("id").clone(), &t2.s("title"), "", None))); }
    if lib && crate::ui::translate_mode() != "off" { let t2 = t.clone(); items.push(MenuItem::new("Edit translation", &icon("edit"), move || crate::pages::translate::edit_translation(t2.s("title"), "title", None))); }
    if has_encore_remote() { items.push(MenuItem::new(if linked { "Your devices…" } else { "Play on another device…" }, &icon("devices"), remote_open)); }
    items.push(MenuItem::new("Customize the bar…", &icon("settings"), customize_bar_modal));
    pop_menu(anchor, items);
}

pub(super) fn update_time() {
    if is_linked() { update_linked_time(); return; }
    let dur = duration();
    let dragging = st(|s| s.dragging);
    for (sid, cid, did) in [("seek", "t-cur", "t-dur"), ("fseek", "f-cur", "f-dur")] {
        let Some(seek) = by_id(sid) else { continue };
        if !dragging {
            dom::set_value(&seek, &js_num(if truthy_num(dur) { (position() / dur) * 1000.0 } else { 0.0 }));
            if let Some(c) = by_id(cid) { dom::set_text(&c, &fmt::dur(position())); }
        }
        set_p(&seek);
        if let Some(d) = by_id(did) { dom::set_text(&d, &fmt::dur(dur)); }
    }
}

pub(super) fn quality(t: &Value) -> String {
    if t.truthy("remote") {
        let label = link_state_value().g("signal").s("label");
        return if label.is_empty() { t.s("format").to_uppercase() } else { label };
    }
    if t.truthy("preview") { return "30 s preview".into(); }
    if st(|s| s.local_copy) { return copy_quality(t); }
    let f = t.s("format").to_uppercase();
    let rung = st(|s| s.rung);
    if rung > 0 { return format!("{f} → {} {rung}k", lossy_codec().to_uppercase()); }
    if st(|s| s.dsp) { return format!("{f} → Cadenza {} kHz", khz(st(|s| s.dsp_rate))); }
    if st(|s| s.transcoded) { format!("{f} → FLAC") }
    else { format!("{f}{}", if t.truthy("bitrate") { format!(" {}k", t.s("bitrate")) } else { String::new() }) }
}
fn quality_title(t: &Value) -> String {
    if t.truthy("remote") { return format!("What {} is playing", { let n = link_name(); if n.is_empty() { "that device".to_string() } else { n } }); }
    if t.truthy("preview") { return "A 30-second preview – not in your library".into(); }
    if st(|s| s.local_copy) { return "Playing the copy downloaded to this device".into(); }
    let rung = st(|s| s.rung);
    if rung > 0 { return if prefs().s("quality") == "saver" { "Data saver: a small 96 kbps stream".into() } else { format!("Adaptive quality: {rung} kbps while the connection can't keep up with the original") }; }
    if st(|s| s.dsp) { return "Upsampled on the server by Cadenza (lossless 24-bit FLAC)".into(); }
    if st(|s| s.transcoded) { "Converted to FLAC for your browser (lossless)".into() } else { "Original file, no conversion".into() }
}

/// `link ? (link.state?.duration || 0) : duration()`.
fn seek_duration() -> f64 { if is_linked() { link_state_value().f("duration") } else { duration() } }
pub(super) fn bind_seek(out: &mut Vec<EventListener>, seek: &Element, cur_id: &str) {
    out.push(dom::listener(seek, "pointerdown", |_| st_mut(|s| s.dragging = true)));
    out.push(dom::listener(seek, "touchstart", |_| st_mut(|s| s.dragging = true)));   // passive, like the JS
    let (sk, cid) = (seek.clone(), cur_id.to_string());
    out.push(dom::listener(seek, "input", move |_| {
        st_mut(|s| s.dragging = true);
        if let Some(c) = by_id(&cid) { dom::set_text(&c, &fmt::dur((num(&dom::value(&sk)) / 1000.0) * seek_duration())); }
        set_p(&sk);
    }));
    let sk = seek.clone();
    let commit: Rc<dyn Fn()> = Rc::new(move || {
        if st(|s| s.dragging) { st_mut(|s| s.dragging = false); seek_to((num(&dom::value(&sk)) / 1000.0) * seek_duration()); }
    });
    for ev in ["change", "pointerup", "touchend"] {
        let c = commit.clone();
        out.push(dom::listener(seek, ev, move |_| c()));
    }
}

pub(super) fn update_linked_time() {
    if !is_linked() || st(|x| x.dragging) { return; }
    let s = link_state_value();
    let dur = s.f("duration");
    let pos = s.f("position").min(if truthy_num(dur) { dur } else { f64::INFINITY });
    for (sid, cid, did) in [("seek", "t-cur", "t-dur"), ("fseek", "f-cur", "f-dur")] {
        let Some(seek) = by_id(sid) else { continue };
        dom::set_value(&seek, &js_num(if truthy_num(dur) { (pos / dur) * 1000.0 } else { 0.0 }));
        set_p(&seek);
        if let Some(c) = by_id(cid) { dom::set_text(&c, &fmt::dur(pos)); }
        if let Some(d) = by_id(did) { dom::set_text(&d, &fmt::dur(dur)); }
    }
}

/// The actions that can sit on the bar as buttons (`id`, label, icon), in the order they are drawn. ⋯ and the
/// volume are always there; everything here is also in the ⋯ menu, so a hidden button loses nothing.
pub const BAR_ACTIONS: &[(&str, &str, &str)] = &[
    ("quality", "Quality badge", ""),
    ("shuffle", "Shuffle", "shuffle"),
    ("repeat", "Repeat", "refresh"),
    ("like", "Like", "heart"),
    ("album", "View album", "disc"),
    ("playlist", "Add to playlist", "plus"),
    ("lyrics", "Lyrics", "music"),
    ("share", "Share link", "external"),
    ("queue", "Up next", "list"),
    ("devices", "Other devices", "devices"),
];
const BAR_KEY: &str = "encore.barButtons";
const BAR_DEFAULT: &[&str] = &["quality", "shuffle", "repeat", "like", "lyrics", "queue", "devices"];
/// Buttons added after the first version shipped: a saved choice from before gets them once (0.26.6), so they
/// show up without a visit to "Customize the bar".
const BAR_ADDED: &[(&str, &[&str])] = &[("encore.barButtons.v2", &["shuffle", "repeat"])];

/// The buttons the user chose for the bar (localStorage `encore.barButtons`, a JSON list of BAR_ACTIONS ids).
pub fn bar_buttons() -> Vec<String> {
    let Some(saved) = dom::ls_get(BAR_KEY).and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()) else {
        return BAR_DEFAULT.iter().map(|s| s.to_string()).collect();
    };
    let mut v: Vec<String> = saved.into_iter().filter(|x| BAR_ACTIONS.iter().any(|(id, _, _)| id == x)).collect();
    for (flag, ids) in BAR_ADDED {
        if dom::ls_get(flag).is_none() {
            for id in *ids { if !v.iter().any(|x| x == id) { v.push(id.to_string()); } }
            dom::ls_set(flag, "1");
            dom::ls_set(BAR_KEY, &serde_json::to_string(&v).unwrap_or_default());
        }
    }
    v
}
pub fn set_bar_buttons(v: &[String]) {
    let same_as_default = v.len() == BAR_DEFAULT.len() && BAR_DEFAULT.iter().all(|d| v.iter().any(|x| x == d));
    if same_as_default { dom::ls_remove(BAR_KEY); } else { dom::ls_set(BAR_KEY, &serde_json::to_string(v).unwrap_or_default()); }
    for (flag, _) in BAR_ADDED { dom::ls_set(flag, "1"); }
    render();
}
/// Shuffle and repeat as the bar shows them: the linked device's when linked, else this player's.
fn mode_state() -> (bool, String) {
    if is_linked() {
        let s = link_state_value();
        let r = s.s("repeat");
        (s.truthy("shuffle"), if ["off", "all", "one"].contains(&r.as_str()) { r } else { "off".into() })
    } else {
        st(|x| (x.shuffle, x.repeat.clone()))
    }
}
fn bar_has(id: &str) -> bool { bar_buttons().iter().any(|x| x == id) }

/// "Customize the bar…": tick the buttons the bar shows (⋯ menu and Settings → Appearance).
pub fn customize_bar_modal() {
    let on = bar_buttons();
    // the icons get an explicit size: outside a .btn an SVG would stretch to the row
    let rows: String = BAR_ACTIONS.iter().map(|(id, label, ic)| format!(
        r#"<label class="row" style="gap:10px;padding:6px 0;align-items:center;cursor:pointer"><input type="checkbox" data-bar="{id}" {}><span class="faint" style="display:inline-flex;width:18px;height:18px;flex:none">{}</span><span>{label}</span></label>"#,
        if on.iter().any(|x| x == id) { "checked" } else { "" },
        if ic.is_empty() { String::new() } else { icon_attr(ic, r#"width="18" height="18""#) })).collect();
    let m = crate::ui::modal(ModalOpts::new("Customize the bar",
        &format!(r#"<p class="small muted" style="margin:0 0 8px">Buttons on the player bar. ⋯ and the volume always stay; everything is still in the ⋯ menu.</p><div class="stack" style="gap:0">{rows}</div>"#))
        .footer(r#"<button class="btn" data-bar-reset>Reset</button><button class="btn primary" data-close>Done</button>"#));
    let read = {
        let m = m.clone();
        move || -> Vec<String> { m.qsa("[data-bar]").iter().filter(|b| dom::checked(b)).map(|b| dom::data(b, "bar")).collect() }
    };
    {
        let read = read.clone();
        dom::on_each(&m.el, "[data-bar]", "change", move |_, _| set_bar_buttons(&read()));
    }
    if let Some(r) = m.qs("[data-bar-reset]") {
        let m2 = m.clone();
        dom::on(&r, "click", move |_| {
            for b in m2.qsa("[data-bar]") { dom::set_checked(&b, BAR_DEFAULT.contains(&dom::data(&b, "bar").as_str())); }
            set_bar_buttons(&BAR_DEFAULT.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        });
    }
}

/// The extra buttons on the right of the bar – the ones chosen in "Customize the bar" (`bar_buttons`), then ⋯,
/// the volume, and close when playing here.
fn right_html(t: Option<&Value>, closable: bool) -> String {
    let lib = t.is_some_and(|t| t.g("id").is_number());
    let btn = |id: &str, label: &str, ic: &str| format!(r#"<button class="btn ghost icon" id="p-{id}" aria-label="{label}" title="{label}">{}</button>"#, icon(ic));
    let badge = match t { Some(t) if bar_has("quality") && !t.truthy("preview") => format!(r#"<button class="badge hide-sm" id="p-signal" type="button" title="{} – click for the signal path">{}</button>"#, esc(&quality_title(t)), esc(&quality(t))), _ => String::new() };
    let (shuffle_on, repeat) = mode_state();
    let shuffle = if bar_has("shuffle") && t.is_some() {
        format!(r#"<button class="btn ghost icon mode {}" id="p-shuffle" title="{}" aria-label="Shuffle" aria-pressed="{shuffle_on}">{}</button>"#,
            if shuffle_on { "on" } else { "" }, if shuffle_on { "Shuffle: on" } else { "Shuffle: off" }, icon("shuffle"))
    } else { String::new() };
    let repeat_btn = if bar_has("repeat") && t.is_some() {
        format!(r#"<button class="btn ghost icon mode {}" id="p-repeat" title="{}" aria-label="Repeat" aria-pressed="{}">{}{}</button>"#,
            if repeat != "off" { "on" } else { "" }, repeat_title(&repeat), repeat != "off", icon("refresh"), if repeat == "one" { "<sup>1</sup>" } else { "" })
    } else { String::new() };
    let like = if bar_has("like") && can_like(t) { like_button("p-like", if t.is_some_and(is_liked) { "on" } else { "" }) } else { String::new() };
    let album = if bar_has("album") && t.is_some_and(|t| t.truthy("album_id")) { btn("album", "View album", "disc") } else { String::new() };
    let playlist = if bar_has("playlist") && lib { btn("playlist", "Add to playlist", "plus") } else { String::new() };
    let lyrics = if bar_has("lyrics") { btn("lyrics", "Lyrics", "music") } else { String::new() };
    let share = if bar_has("share") && lib { btn("share", "Share link", "external") } else { String::new() };
    let queue = if bar_has("queue") { btn("queue", "Up next", "list") } else { String::new() };
    let devices = if bar_has("devices") && has_encore_remote() { format!(r#"<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">{}</button>"#, icon("devices")) } else { String::new() };
    let vol = if is_linked() { let s = link_state_value(); if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") } } else { js_num(user_vol()) };
    format!(r#"<div class="right">
      {badge}
      {shuffle}
      {repeat_btn}
      {like}
      {album}
      {playlist}
      <button class="btn ghost icon" id="p-more" aria-label="More" title="More">{more}</button>
      {lyrics}
      {share}
      {queue}
      {devices}
      <span class="faint vol-ic">{volume}</span>
      <input type="range" id="vol" min="0" max="1" step="0.01" value="{vol}">
      {close}
    </div>"#,
        more = icon("more"), volume = icon_attr("volume", r#"width="18""#),
        close = if closable { format!(r#"<button class="btn ghost icon" id="p-close" aria-label="Close player">{}</button>"#, icon("x")) } else { String::new() })
}
fn bind_right(ls: &mut Vec<EventListener>, el: &Element) {
    wire(ls, el, "#p-like", "click", |_| { if let Some(t) = now_track() { dom::spawn(toggle_like(t)); } });
    wire(ls, el, "#p-more", "click", |e| { if let Some(b) = dom::closest(e, "#p-more") { more_menu(&b); } });
    wire(ls, el, "#p-shuffle", "click", |_| { let (on, _) = mode_state(); set_shuffle(!on); });
    wire(ls, el, "#p-repeat", "click", |_| { let (_, r) = mode_state(); set_repeat(next_repeat(&r)); });
    wire(ls, el, "#p-album", "click", |_| { if let Some(t) = now_track() { if t.truthy("album_id") { app::navigate(&format!("/library/album/{}", t.s("album_id"))); } } });
    wire(ls, el, "#p-playlist", "click", |_| { if let Some(t) = now_track() { if t.g("id").is_number() { crate::pages::playlists::add_to_playlist_modal(vec![t.i("id")]); } } });
    wire(ls, el, "#p-lyrics", "click", |_| { st_mut(|s| s.pane = "lyrics".into()); set_full(true); });
    wire(ls, el, "#p-share", "click", |_| { if let Some(t) = now_track() { if t.g("id").is_number() { dom::spawn(share_track(t)); } } });
    wire(ls, el, "#p-queue", "click", |_| { st_mut(|s| s.pane = "queue".into()); set_full(true); });
    wire(ls, el, "#p-devices", "click", |_| remote_open());
    wire(ls, el, "#p-signal", "click", |_| dom::spawn(signal_path()));
    if is_linked() {
        wire(ls, el, "#vol", "change", |e| { if let Some(v) = dom::target_el(e) { set_volume(num(&dom::value(&v))); } });
    } else {
        wire(ls, el, "#vol", "input", |e| { if let Some(v) = dom::target_el(e) { local_volume(num(&dom::value(&v))); } });
    }
    warm_likes(now_track().as_ref());
}

fn render_linked(el: &Element) {
    let Some(l) = linked() else { return };
    let s = l.state.clone();
    let t = remote_track();
    let name = esc(&l.name);
    let where_ = format!(r#"<span class="linked-to" title="This device is a remote for {name}">{}{}Playing on {name}</span>"#, icon("link"), if l.online { "" } else { "Offline · " });
    dom::add_class(&body(), "has-player");
    let has_title = s.truthy("title");
    let thumb = if has_title { art(&s.s("cover"), &album_or_title(&s), "") } else { format!(r#"<div class="ph">{}</div>"#, icon_attr("devices", r#"width="20""#)) };
    let title = if has_title { esck(&s, "title") } else { "Nothing playing".to_string() };
    let sub = if has_title {
        format!("{}{}", esc(&s.s("artist")), if s.truthy("album") { format!(" · {}", esck(&s, "album")) } else { String::new() })
    } else { "Play anything – it starts there".to_string() };
    let right = right_html(t.as_ref(), false).replacen(r#"<div class="right">"#, &format!(r#"<div class="right"><button class="btn sm hide-sm" id="p-unlink" title="Stop being a remote and play on this device again">{}Unlink</button>"#, icon("link")), 1);
    set_html(el, &format!(r##"
    <div class="now" id="p-open" role="button" tabindex="0" aria-label="Open the full-screen player" title="Open player">
      <div class="thumb">{thumb}</div>
      <div class="grow">
        <div class="ellipsis" style="font-weight:650">{title}</div>
        <div class="ellipsis small muted">{sub}</div>
        <div class="ellipsis small">{where_}</div>
      </div>
    </div>
    <div class="ctrls">
      <div class="row">
        <button class="btn ghost icon" id="p-prev" aria-label="Previous">{prev}</button>
        <button class="play-btn" id="p-toggle" aria-label="Play/Pause">{pp}</button>
        <button class="btn ghost icon" id="p-next" aria-label="Next">{next}</button>
      </div>
      <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
    </div>
    {right}"##,
        prev = icon("prev"), pp = if s.truthy("playing") { icon("pause") } else { icon("play") }, next = icon("next")));
    let mut ls = Vec::new();
    wire(&mut ls, el, "#p-toggle", "click", |_| toggle());
    wire(&mut ls, el, "#p-next", "click", |_| next(false));
    wire(&mut ls, el, "#p-prev", "click", |_| prev());
    wire(&mut ls, el, "#p-open", "click", |_| { st_mut(|s| s.pane = "now".into()); set_full(true); });
    wire(&mut ls, el, "#p-open", "keydown", |e| { if dom::key(e) == "Enter" { set_full(true); } });
    wire(&mut ls, el, "#p-unlink", "click", |_| remote_unlink());
    bind_right(&mut ls, el);
    if let Some(seek) = qs_in(el, "#seek") { bind_seek(&mut ls, &seek, "t-cur"); }
    set_bar(ls);
    update_linked_time();
}

pub(super) fn render() {
    let Some(el) = by_id("player") else { return };
    if is_linked() { super::full::forget(super::full::View::Bar); render_linked(&el); return; }
    let t = cur();
    dom::add_class(&body(), "has-player");
    let Some(t) = t else {
        super::full::forget(super::full::View::Bar);
        let devices = if has_encore_remote() {
            format!(r#"<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">{}</button>"#, icon("devices"))
        } else { String::new() };
        set_html(&el, &format!(r##"<div class="now"><div class="thumb"><div class="ph">{music}</div></div>
      <div class="grow"><div style="font-weight:650" class="faint">Nothing playing</div>
      <div class="small muted ellipsis">Open an album in <a href="#/library" style="color:var(--accent)">Library</a> and press ▶ Play</div></div></div>
      <div class="ctrls idle">
        <div class="row">
          <button class="btn ghost icon" id="p-prev" aria-label="Previous" disabled>{prev}</button>
          <button class="play-btn" id="p-toggle" aria-label="Play/Pause" disabled>{play}</button>
          <button class="btn ghost icon" id="p-next" aria-label="Next" disabled>{next}</button>
        </div>
        <div class="seek"><span>0:00</span><input type="range" min="0" max="1000" value="0" disabled aria-label="Seek"><span>0:00</span></div>
      </div>
      <div class="right">
        {devices}
        <span class="faint vol-ic">{volume}</span>
        <input type="range" id="vol" min="0" max="1" step="0.01" value="{vol}">
      </div>"##, music = icon_attr("music", r#"width="20""#), prev = icon("prev"), play = icon("play"), next = icon("next"),
            volume = icon_attr("volume", r#"width="18""#), vol = js_num(user_vol())));
        let mut ls = Vec::new();
        wire(&mut ls, &el, "#p-devices", "click", |_| remote_open());
        wire(&mut ls, &el, "#vol", "input", |e| { if let Some(v) = dom::target_el(e) { local_volume(num(&dom::value(&v))); } });
        set_bar(ls);
        if st(|s| s.full) { set_full(false); }
        return;
    };
    if audio().is_none() { return; }
    let html = format!(r##"
    <div class="now" id="p-open" role="button" tabindex="0" aria-label="Open the full-screen player" title="Open player">
      <div class="thumb">{thumb}</div>
      <div class="grow">
        <div class="ellipsis" style="font-weight:650">{title}</div>
        <div class="ellipsis small muted">{artist}{album}</div>
      </div>
    </div>
    <div class="ctrls">
      <div class="row">
        <button class="btn ghost icon" id="p-prev" aria-label="Previous">{prev}</button>
        <button class="play-btn" id="p-toggle" aria-label="Play/Pause">{pp}</button>
        <button class="btn ghost icon" id="p-next" aria-label="Next">{next}</button>
      </div>
      <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
    </div>
    {right}"##,
        thumb = crate::ui::art_sized(&cover(&t), &album_or_title(&t), "", 200),
        title = esc(&crate::ui::tr(&t, "title")),
        artist = esc(&crate::ui::tr_inline(&t, "artist")),
        album = if t.truthy("album") { format!(" · {}", esc(&crate::ui::tr_inline(&t, "album"))) } else { String::new() },
        prev = icon("prev"), pp = super::full::PP, next = icon("next"),
        right = right_html(Some(&t), true));
    if !super::full::draw(super::full::View::Bar, &el, "#p-toggle", &html, &if shows_playing() { icon("pause") } else { icon("play") }) {
        update_time();
        if st(|s| s.full) { render_full(); }
        return;
    }
    let mut ls = Vec::new();
    wire(&mut ls, &el, "#p-toggle", "click", |_| toggle());
    wire(&mut ls, &el, "#p-next", "click", |_| next(false));
    wire(&mut ls, &el, "#p-prev", "click", |_| prev());
    wire(&mut ls, &el, "#p-open", "click", |_| { st_mut(|s| s.pane = "now".into()); set_full(true); });
    wire(&mut ls, &el, "#p-open", "keydown", |e| { if dom::key(e) == "Enter" { set_full(true); } });
    bind_right(&mut ls, &el);
    if let Some(seek) = qs_in(&el, "#seek") { bind_seek(&mut ls, &seek, "t-cur"); }
    wire(&mut ls, &el, "#p-close", "click", |_| {
        end_fade();
        if let (Some(a), Some(sp)) = (audio(), spare()) {
            let _ = a.pause();
            let _ = a.remove_attribute("src");
            let _ = sp.remove_attribute("src");
            let _ = sp.remove_attribute("data-url");
        }
        st_mut(|s| { s.queue.clear(); s.index = -1; s.want = false; });
        HELD.with(|h| h.set(false));
        HELD_AT.with(|h| h.set(0.0));
        reset_rec(None);
        let w = STALL_WATCH.with(|w| w.borrow_mut().take());
        drop(w);
        dom::ls_remove(RESUME);
        render(); emit(); media_state();
    });
    set_bar(ls);
    update_time();
    if st(|s| s.full) { render_full(); }
}
