//! The queue: what plays next and in what order (shuffle, repeat), the library shuffle that keeps the music going
//! when it runs out (no song twice in a session), and the queue and spot this device saves so the app opens
//! where you left off.

use super::*;

thread_local! {
    /// Songs heard this listening session (this tab): shuffles never repeat one until the whole library has played.
    static PLAYED: RefCell<Vec<i64>> = RefCell::new(load_played());
    static TOPPING: Cell<bool> = const { Cell::new(false) };
    /// music actually played since the last save: only then is saved_at moved on
    pub(super) static PLAYED_SINCE: Cell<bool> = const { Cell::new(false) };
    /// when saveSpot last wrote (every 5 s while playing)
    static SPOT_SAVED_AT: Cell<f64> = const { Cell::new(0.0) };
}

fn session_storage() -> Option<web_sys::Storage> { dom::window().session_storage().ok().flatten() }

// ---- played this session ------------------------------------------------------------------------
fn load_played() -> Vec<i64> {
    let raw = session_storage().and_then(|s| s.get_item("encore.played").ok().flatten()).unwrap_or_default();
    let raw = if raw.is_empty() { "[]".to_string() } else { raw };
    serde_json::from_str::<Value>(&raw).map(|v| v.items().iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default()
}
pub(super) fn mark_played(t: &Value) {
    if !t.g("id").is_number() { return; }
    let id = t.i("id");
    let list = PLAYED.with(|p| {
        let mut p = p.borrow_mut();
        if p.contains(&id) { return None; }
        p.push(id);
        Some(p.clone())
    });
    if let (Some(list), Some(s)) = (list, session_storage()) {
        let _ = s.set_item("encore.played", &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()));
    }
}
/// Random library songs not yet heard (or queued) this session; when everything has played, start a fresh round.
/// With smart shuffle on (Settings → Playback) the server weights the draw by taste (player/taste.rs).
async fn fresh_shuffle(limit: usize) -> Result<Vec<Value>, ApiError> {
    // a genre shuffle playing: more of the same genres
    let genres = genre_scope();
    let url = if smart_on() || genres.is_some() { "/taste/shuffle" } else { "/library/shuffle" };
    let body = |exclude: &[i64]| { let mut b = json!({ "limit": limit, "exclude": exclude }); if let Some(g) = &genres { b["genres"] = json!(g); } b };
    let queued: Vec<i64> = st(|s| s.queue.iter().filter(|t| t.g("id").is_number()).map(|t| t.i("id")).collect());
    let mut exclude: Vec<i64> = PLAYED.with(|p| p.borrow().clone());
    for id in queued { if !exclude.contains(&id) { exclude.push(id); } }
    let mut more = api::post(url, body(&exclude)).await?.items();
    if more.is_empty() {
        PLAYED.with(|p| p.borrow_mut().clear());
        if let Some(s) = session_storage() { let _ = s.remove_item("encore.played"); }
        let cur_id: Vec<i64> = cur().filter(|t| t.g("id").is_number()).map(|t| vec![t.i("id")]).unwrap_or_default();
        more = api::post(url, body(&cur_id)).await?.items();
    }
    Ok(more)
}
/// The Shuffle button: play the whole library in random order, no song twice this session.
pub async fn shuffle_library() -> usize {
    match fresh_shuffle(50).await {
        Ok(first) => {
            let n = first.len();
            if n > 0 { play_queue(first, 0, "library"); }
            n
        }
        // the JS let the error escape the caller's async handler (nothing shown but a console error);
        // the contract only carries a count, so answer "not empty" to keep the caller's toast away
        Err(e) => { dom::console_error(&e.message); 1 }
    }
}

/// context: 'album' | 'single' | 'list' – decides what happens when the queue ends.
fn sendable(tracks: &[Value]) -> Vec<Value> { tracks.iter().filter(|t| json::truthy(t) && t.g("id").is_number()).map(slim).collect() }
pub fn play_queue(tracks: Vec<Value>, start: usize, context: &str) {
    // another list: a genre shuffle that was playing is over (play_genres sets it again for its own list)
    clear_genre_scope();
    // in a group listen the list becomes the group's queue (player/group.rs)
    if g_play_queue(&tracks, start, context, 0.0) { return; }
    if is_linked() && !is_local() && context != "preview" {
        let list: Vec<Value> = tracks.into_iter().filter(json::truthy).collect();
        let at = (start as i64).min(list.len() as i64 - 1).max(0);
        let first_id = list.get(at as usize).map(|f| f.g("id").clone());
        let out = sendable(&list);
        if out.is_empty() { toast("Only songs from your library can be played on another device", "err"); return; }
        let index = first_id.and_then(|fid| out.iter().position(|t| *t.g("id") == fid)).map(|i| i as i64).unwrap_or(-1).max(0);
        via_link(json!({ "action": "play_queue", "tracks": out, "index": index }));
        return;
    }
    st_mut(|s| {
        s.queue = tracks.into_iter().filter(json::truthy).map(Rc::new).collect();
        s.index = (start as i64).min(s.queue.len() as i64 - 1).max(0);
        s.context = context.to_string();
        s.order = None;
        if s.shuffle && context != "preview" { shuffle_rest(s); }
    });
    if st(|s| s.shuffle) && context != "preview" { smart_order(); }
    load(false, 0.0, None, None);
}

/// A Shuffle button of an album, an artist or a playlist, in a group listen: the songs go to the group shuffled (as its
/// queue for those who control it, else added to its Up next). False outside a group: the page shuffles as usual.
pub fn group_shuffle(tracks: &[Value]) -> bool { g_shuffle_play(tracks) }

pub fn enqueue(tracks: Vec<Value>) {
    if g_add("enqueue", &tracks) { return; }
    if is_linked() && !is_local() {
        let out = sendable(&tracks);
        if !out.is_empty() { via_link(json!({ "action": "enqueue", "tracks": out })); }
        return;
    }
    st_mut(|s| {
        let add: Vec<Rc<Value>> = tracks.into_iter().map(Rc::new).collect();
        if let Some(o) = s.order.as_mut() { o.extend(add.iter().cloned()); }
        s.queue.extend(add);
    });
    if st(|s| s.index < 0) { st_mut(|s| s.index = 0); load(false, 0.0, None, None); } else { render(); emit(); preload_next(); }
}

pub fn play_next(tracks: Vec<Value>) {
    if g_add("play_next", &tracks) { return; }
    if is_linked() && !is_local() {
        let out = sendable(&tracks);
        if !out.is_empty() { via_link(json!({ "action": "play_next", "tracks": out })); }
        return;
    }
    if st(|s| s.index < 0) { play_queue(tracks, 0, "list"); return; }
    st_mut(|s| {
        let add: Vec<Rc<Value>> = tracks.into_iter().map(Rc::new).collect();
        let current = s.queue.get(s.index.max(0) as usize).cloned();
        if let Some(o) = s.order.as_mut() {
            let at = current.and_then(|c| o.iter().position(|x| Rc::ptr_eq(x, &c))).map(|i| i + 1).unwrap_or(o.len());
            o.splice(at..at, add.iter().cloned());
        }
        let at = ((s.index + 1) as usize).min(s.queue.len());
        s.queue.splice(at..at, add);
    });
    render(); emit(); preload_next();
}

/// 30-second preview of a song that isn't in the library (Deezer/iTunes preview URL).
pub fn preview(t: Value) {
    let mut p = if t.is_object() { t.clone() } else { json!({}) };
    if let Some(o) = p.as_object_mut() {
        o.insert("preview".into(), json!(true));
        o.insert("id".into(), Value::String(format!("pv-{}", t.s("preview_url"))));
    }
    play_queue(vec![p], 0, "preview");
}

pub fn queue() -> (Vec<Value>, usize) {
    st(|s| (s.queue.iter().map(|t| (**t).clone()).collect(), s.index.max(0) as usize))
}

pub fn next(auto: bool) {
    if g_next(auto) { return; }
    if !auto && via_link(json!({ "action": "next" })) { return; }
    // Next before the play counted: a skip, which the recommendations take as a mild "not now"
    if !auto { report_skip(); }
    if auto && st(|s| s.repeat == "one") { seek_to(0.0); if let Some(a) = audio() { play(&a); } return; }
    if st(|s| s.index < s.queue.len() as i64 - 1) { st_mut(|s| s.index += 1); load(false, 0.0, None, None); return; }
    if st(|s| s.repeat == "all" && !s.queue.is_empty()) { st_mut(|s| s.index = 0); load(false, 0.0, None, None); return; }
    if st(|s| s.context == "preview") { if auto { st_mut(|s| s.want = false); media_state(); } return; }
    dom::spawn(async move {
        if continue_with_library().await { st_mut(|s| s.index += 1); load(false, 0.0, None, None); }
        // the end of the music: nothing to pick up again
        else if auto { st_mut(|s| s.want = false); media_state(); }
    });
}

/// Queue ran out: keep the music going with a shuffle of the whole library.
async fn continue_with_library() -> bool {
    if grouped() { return false; }
    if !prefs().truthy("autoplay") { return false; }
    match fresh_shuffle(50).await {
        Ok(more) => {
            if more.is_empty() { return false; }
            st_mut(|s| { s.queue.extend(more.into_iter().map(Rc::new)); s.context = "library".into(); });
            true
        }
        Err(_) => false,
    }
}

pub fn prev() {
    if g_prev() { return; }
    if via_link(json!({ "action": "prev" })) { return; }
    if position() > 3.0 || st(|s| s.index == 0) { seek_to(0.0); return; }
    st_mut(|s| s.index -= 1);
    load(false, 0.0, None, None);
}

pub(super) fn jump(i: i64) {
    if g_entry("jump", i, 0) { return; }
    st_mut(|s| s.index = i); load(false, 0.0, None, None); }
pub(super) fn remove_at(i: i64) {
    if g_entry("remove", i, 0) { return; }
    if st(|s| i == s.index) { return; }
    st_mut(|s| {
        if i >= 0 && (i as usize) < s.queue.len() { s.queue.remove(i as usize); }
        if i < s.index { s.index -= 1; }
    });
    render(); emit(); preload_next();
}
pub(super) fn move_to(from: i64, to: i64) {
    if g_entry("move", from, to) { return; }
    if st(|s| from == s.index || to <= s.index || to >= s.queue.len() as i64) { return; }
    st_mut(|s| {
        if from >= 0 && (from as usize) < s.queue.len() {
            let t = s.queue.remove(from as usize);
            s.queue.insert(to as usize, t);
        }
    });
    render(); emit(); preload_next();
}
/// `shuffleRest()`: shuffle the songs after the current one, remembering the order they were in (to put back later).
fn shuffle_rest(s: &mut State) {
    if s.order.is_none() { s.order = Some(s.queue.clone()); }
    let start = (s.index + 1).max(0) as usize;
    if start < s.queue.len() {
        let rest = &mut s.queue[start..];
        for i in (1..rest.len()).rev() {
            let j = (js_sys::Math::random() * (i + 1) as f64).floor() as usize;
            rest.swap(i, j);
        }
    }
}
/// `unshuffled()`: the queue in its order from before shuffling: songs removed since stay out, songs added since go at the end.
fn unshuffled(s: &State) -> Vec<Rc<Value>> {
    let mut left: Vec<(Rc<Value>, usize)> = vec![];
    for t in &s.queue {
        match left.iter_mut().find(|(x, _)| Rc::ptr_eq(x, t)) { Some(e) => e.1 += 1, None => left.push((t.clone(), 1)) }
    }
    let mut out = vec![];
    let mut take = |t: &Rc<Value>, out: &mut Vec<Rc<Value>>| {
        if let Some(e) = left.iter_mut().find(|(x, n)| *n > 0 && Rc::ptr_eq(x, t)) { e.1 -= 1; out.push(t.clone()); }
    };
    for t in s.order.as_deref().unwrap_or(&[]) { take(t, &mut out); }
    for t in &s.queue { take(t, &mut out); }
    out
}
/// `setShuffle(on)`: shuffle mode – on shuffles the rest of the queue, off puts the original order back (the song playing stays).
pub(super) fn set_shuffle(on: bool) {
    if g_shuffle() { return; }
    if via_link(json!({ "action": "shuffle", "value": on })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() { if let Some(o) = l.state.as_object_mut() { o.insert("shuffle".into(), json!(on)); } });
        render();
        if st(|s| s.full) { render_full(); }
        return;
    }
    if st(|s| s.shuffle == on) { return; }
    st_mut(|s| {
        s.shuffle = on;
        if on { shuffle_rest(s); }
        else if s.order.is_some() {
            let current = s.queue.get(s.index.max(0) as usize).cloned();
            let q = unshuffled(s);
            if let Some(i) = current.and_then(|c| q.iter().position(|x| Rc::ptr_eq(x, &c))) { s.queue = q; s.index = i as i64; }
            s.order = None;
        }
    });
    render(); emit(); preload_next(); save_spot(true);
    if on { smart_order(); }
}
/// `setRepeat(v)`: 'off' | 'all' | 'one'.
pub(super) fn set_repeat(v: &str) {
    if g_repeat() { return; }
    if !["off", "all", "one"].contains(&v) { return; }
    if via_link(json!({ "action": "repeat", "value": v })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() { if let Some(o) = l.state.as_object_mut() { o.insert("repeat".into(), json!(v)); } });
        render();
        if st(|s| s.full) { render_full(); }
        return;
    }
    st_mut(|s| s.repeat = v.to_string());
    render();
    if st(|s| s.full) { render_full(); }
    emit(); save_spot(true);
}
/// Up next's Clear: everything after the song playing goes (the group's, asked first, in a group listen). Here the
/// toast "Up next cleared" offers Undo for 6 s, which puts the songs back after the song playing.
pub(super) fn clear_up_next() {
    if g_clear() { return; }
    if is_linked() { return; }
    let (gone, playing) = st_mut(|s| {
        let at = ((s.index + 1).max(0) as usize).min(s.queue.len());
        (s.queue.split_off(at), s.queue.get(s.index.max(0) as usize).cloned())
    });
    if gone.is_empty() { return; }
    cancel_preload();
    render(); emit(); save_spot(true);
    if st(|s| s.full) { render_full(); }
    crate::ui::toast_action("Up next cleared", "Undo", 6000, move || {
        st_mut(|s| {
            // after the song playing (the same one, when it still is), before anything added since
            let at = playing.as_ref().and_then(|p| s.queue.iter().position(|x| Rc::ptr_eq(x, p))).map(|i| i + 1)
                .unwrap_or_else(|| ((s.index + 1).max(0) as usize).min(s.queue.len()));
            s.queue.splice(at..at, gone);
        });
        render(); emit(); preload_next(); save_spot(true);
        if st(|s| s.full) { render_full(); }
    });
}
pub(super) fn next_repeat(r: &str) -> &'static str { match r { "off" => "all", "all" => "one", "one" => "off", _ => "all" } }
pub(super) fn repeat_title(r: &str) -> &'static str { match r { "one" => "Repeat: this song", "all" => "Repeat: the queue", _ => "Repeat: off" } }

/// Near the end of the queue: fetch the library shuffle ahead of time, so the next song is ready too.
pub(super) fn top_up() {
    if TOPPING.with(|t| t.get()) || grouped() || st(|s| s.context == "preview" || s.index < s.queue.len() as i64 - 2) { return; }
    if !prefs().truthy("autoplay") { return; }
    TOPPING.with(|t| t.set(true));
    dom::spawn(async move {
        if let Ok(more) = fresh_shuffle(50).await {
            if !more.is_empty() {
                st_mut(|s| s.queue.extend(more.into_iter().map(Rc::new)));
                render(); emit();
                maybe_preload(false);
            }
        }
        // offline: next() tries again
        TOPPING.with(|t| t.set(false));
    });
}

// ---- remember where you left off: this device's queue and spot, put back paused when the app opens --------
pub(super) const RESUME: &str = "encore.resume";
pub(super) fn is_lib(x: &Value) -> bool { json::truthy(x) && !x.truthy("preview") && x.g("id").is_number() }
/// Save the queue and the spot (every 5 s while playing; at once on pause, a new song and when the page goes away).
pub(super) fn save_spot(force: bool) {
    // a group listen's queue is the group's, not this device's spot
    if is_linked() || grouped() || HELD.with(|h| h.get()) || audio().is_none() { return; }
    let Some(t) = cur() else { return };
    if !is_lib(&t) { return; }
    let now = now_ms();
    if !force && now - SPOT_SAVED_AT.with(|c| c.get()) < 5000.0 { return; }
    SPOT_SAVED_AT.with(|c| c.set(now));
    let (queue, index) = lib_window();
    let (context, repeat, shuffle) = st(|s| (s.context.clone(), s.repeat.clone(), s.shuffle));
    // saved_at is when music last played here: a pause or a page closed while paused keeps the old one, so a
    // song played on another device since still wins when the app opens again
    let saved_at = if PLAYED_SINCE.with(|p| p.get()) { now / 1000.0 } else {
        saved_spot().map(|s| s.f("saved_at")).filter(|x| x.is_finite() && *x != 0.0).unwrap_or(now / 1000.0)
    };
    if audio().is_some_and(|a| a.paused()) { PLAYED_SINCE.with(|p| p.set(false)); }
    dom::ls_set(RESUME, &json!({ "queue": queue, "index": index, "position": position(), "shuffle": shuffle,
        "repeat": repeat, "context": context, "saved_at": saved_at }).to_string());
}
/// The library songs of the queue – at most 300, around the current song – and where the current song is in them.
pub(super) fn lib_window() -> (Vec<Value>, i64) {
    let (mut queue, mut index) = st(|s| {
        let q: Vec<Value> = s.queue.iter().filter(|x| is_lib(x)).map(|x| slim(x)).collect();
        let i = s.queue.iter().take(s.index.max(0) as usize).filter(|x| is_lib(x)).count() as i64;
        (q, i)
    });
    if queue.len() > 300 {
        let from = (index - 50).min(queue.len() as i64 - 300).max(0) as usize;
        queue = queue[from..from + 300].to_vec();
        index -= from as i64;
    }
    (queue, index)
}
/// Where the song is – for a held (restored) song whose file hasn't loaded yet, where it was restored to.
pub(super) fn spot_position() -> f64 {
    let at = HELD_AT.with(|h| h.get());
    if HELD.with(|h| h.get()) && at != 0.0 && audio().is_some_and(|a| a.ready_state() < 1) { at } else { position() }
}
/// What saveSpot saved on this device: { queue, index, position, shuffle, repeat, context, saved_at } – None when nothing is saved.
pub fn saved_spot() -> Option<Value> {
    let s = serde_json::from_str::<Value>(&dom::ls_get(RESUME).unwrap_or_default()).ok()?;
    if s.g("queue").as_array().is_some_and(|q| q.iter().any(is_lib)) { Some(s) } else { None }
}
/// Put a saved spot back: the queue, paused at the song and position you left off (one tap plays).
pub fn restore(s: Option<Value>) -> bool {
    let Some(s) = s else { return false };
    if is_linked() || grouped() || cur().is_some() || audio().is_none() || !s.g("queue").is_array() { return false; }
    let q: Vec<Value> = s.a("queue").into_iter().filter(is_lib).collect();
    if q.is_empty() { return false; }
    let ctx = s.s("context");
    let context = if ["album", "single", "list", "library"].contains(&ctx.as_str()) { ctx } else if s.truthy("shuffle") { "library".to_string() } else { "list".to_string() };
    let repeat = s.s("repeat");
    let index = s.f("index");
    st_mut(|x| {
        let n = q.len() as i64;
        x.queue = q.into_iter().map(Rc::new).collect();
        x.index = (if index.is_finite() { index as i64 } else { 0 }).min(n - 1).max(0);
        x.context = context;
        if ["off", "all", "one"].contains(&repeat.as_str()) { x.repeat = repeat; }
        // shuffle on: the saved queue is already in its shuffled order (the original order isn't kept)
        x.shuffle = s.g("shuffle") == &json!(true) && x.context != "library";
        x.order = None;
    });
    let dur = cur().map(|t| t.f("duration")).unwrap_or(0.0);
    let at = s.f("position");
    let offset = if at > 1.0 && !(dur > 0.0 && at > dur - 2.0) { at } else { 0.0 };
    load_opts(LoadOpts { offset, hold: true, ..Default::default() });
    true
}
