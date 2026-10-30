//! The recommendations in the player (the server's /api/taste): the dislike button (a song never queued by a
//! shuffle or a mix again, and gone from Up next), "Don't play this artist", skips (Next before a play counted,
//! a mild "not now"), smart shuffle (Settings → Playback, on by default: shuffles and "keep going" weighted by
//! taste instead of plain random), "Start radio" (songs like this one from the library) and "Genre shuffle…" (a
//! shuffle of one or more of the library's genres, which the music after the queue keeps to).

use super::*;

thread_local! {
    /// disliked song ids, fetched the first time a dislike button is drawn
    static DISLIKED: RefCell<Option<HashSet<i64>>> = const { RefCell::new(None) };
    /// goes up with every smart reorder asked for: an older answer that arrives late is dropped
    static REORDER: Cell<u32> = const { Cell::new(0) };
}

/// The disliked song ids (asked of the server once, then kept up to date here).
pub(super) async fn dislikes() -> HashSet<i64> {
    if DISLIKED.with(|d| d.borrow().is_none()) {
        let set: HashSet<i64> = match api::get("/taste/dislikes").await {
            Ok(v) => v.a("tracks").iter().filter_map(|t| t.g("id").as_i64()).collect(),
            Err(_) => HashSet::new(),
        };
        DISLIKED.with(|d| *d.borrow_mut() = Some(set));
    }
    DISLIKED.with(|d| d.borrow().clone().unwrap_or_default())
}

/// Whether this song is disliked (as far as this tab knows).
pub(super) fn is_disliked(t: &Value) -> bool {
    t.g("id").is_number() && DISLIKED.with(|d| d.borrow().as_ref().is_some_and(|s| s.contains(&t.i("id"))))
}

/// Fetch the dislikes the first time a dislike button is drawn, then paint the buttons.
pub(super) fn warm_dislikes(t: Option<&Value>) {
    if DISLIKED.with(|d| d.borrow().is_none()) && can_like(t) && !no_server() {
        dom::spawn(async { dislikes().await; paint_dislikes(); });
    }
}

/// The dislike button (thumbs down), `on` while the song is disliked.
pub(super) fn dislike_button(id: &str, cls: &str) -> String {
    format!(r#"<button class="btn ghost icon dislike {cls}" id="{id}" aria-label="Dislike" title="Dislike – don't play this song in shuffles and mixes">{}</button>"#, icon("thumbsdown"))
}

/// Update every dislike button on screen without redrawing.
pub(super) fn paint_dislikes() {
    let on = now_track().as_ref().is_some_and(is_disliked);
    for b in dom::qsa(".btn.dislike") {
        dom::toggle_class(&b, "on", on);
        let _ = b.set_attribute("title", if on { "Disliked – tap to take it back" } else { "Dislike – don't play this song in shuffles and mixes" });
    }
}

/// The dislike button: dislike the song (it leaves Liked Songs and Up next, and when it is playing here the
/// player moves on), or take a dislike back.
pub(super) async fn toggle_dislike(t: Value) {
    if !t.g("id").is_number() { return; }
    let id = t.i("id");
    let was = dislikes().await.contains(&id);
    let r = if was { api::del(&format!("/taste/dislikes/tracks/{id}")).await } else { api::put(&format!("/taste/dislikes/tracks/{id}"), json!({})).await };
    if let Err(e) = r { toast(&e.message, "err"); return; }
    DISLIKED.with(|d| if let Some(s) = d.borrow_mut().as_mut() { if was { s.remove(&id); } else { s.insert(id); } });
    if was {
        toast("Dislike taken back", "ok");
        paint_dislikes();
        return;
    }
    // the opposite of a like: the server took it out of Liked Songs, the hearts follow
    LIKED.with(|l| if let Some(s) = l.borrow_mut().as_mut() { s.remove(&id); });
    paint_likes();
    dom::dispatch("encore:likes", None);
    toast("Got it – you won't hear this song in shuffles and mixes again", "ok");
    drop_from_up_next(|x| x.g("id").as_i64() == Some(id));
    paint_dislikes();
    // playing here: on to the next song (a dislike is not a skip, so none is reported)
    if !is_linked() && now_track().is_some_and(|c| c.g("id").as_i64() == Some(id)) { next(false); }
}

/// "Don't play this artist": none of the artist's songs is queued by a shuffle or a mix again, and they leave Up next.
pub(super) async fn dislike_artist(name: String) {
    if name.trim().is_empty() { return; }
    if let Err(e) = api::put(&format!("/taste/dislikes/artists/{}", String::from(js_sys::encode_uri_component(&name))), json!({})).await {
        toast(&e.message, "err");
        return;
    }
    let key = name.trim().to_lowercase();
    drop_from_up_next(|x| x.s("artist").trim().to_lowercase() == key);
    toast(&format!("Got it – no more {name} in shuffles and mixes"), "ok");
}

/// Takes the songs `gone` matches out of Up next (after the song playing; the song playing stays).
fn drop_from_up_next(gone: impl Fn(&Value) -> bool) {
    if grouped() || is_linked() { return; }
    let changed = st_mut(|s| {
        let at = (s.index + 1).max(0) as usize;
        if at >= s.queue.len() { return false; }
        let before = s.queue.len();
        let rest: Vec<Rc<Value>> = s.queue.split_off(at).into_iter().filter(|x| !gone(x)).collect();
        s.queue.extend(rest);
        s.queue.len() != before
    });
    if changed { render(); if st(|s| s.full) { render_full(); } emit(); preload_next(); save_spot(true); }
}

/// Smart shuffle is on (Settings → Playback; on unless switched off).
pub(super) fn smart_on() -> bool {
    prefs().g("smart_shuffle") != &json!(false)
}

/// Next was pressed before the song's play counted: tell the server it was skipped (it counts only after the first
/// seconds; a disliked song's skip is not sent, the dislike says more).
pub(super) fn report_skip() {
    let Some(t) = cur() else { return };
    if t.truthy("preview") || !t.g("id").is_number() || st(|s| s.logged) || no_server() || is_disliked(&t) { return; }
    let p = position();
    if !p.is_finite() || p < 2.0 { return; }
    let id = t.i("id");
    dom::spawn(async move { let _ = api::post("/taste/skip", json!({ "track_id": id, "seconds": p })).await; });
}

/// A song whose play counted is over (the next one starts): tell the server how far it got, so the
/// recommendations weigh the play by it (heard to the end counts more than left soon after half). Nothing when
/// no play counted, or without the server (the play itself waits in the outbox; it counts as an ordinary play).
pub(super) fn report_heard() {
    let Some((id, p)) = st_mut(|s| s.heard.take()) else { return };
    if !p.is_finite() || p <= 0.0 || no_server() { return; }
    dom::spawn(async move { let _ = api::post("/taste/listened", json!({ "track_id": id, "seconds": p })).await; });
}

/// After the rest of the queue was shuffled with smart shuffle on: the server puts it in a taste-weighted order
/// (favourites more often near the front, artists spread out, disliked songs out). The local shuffle stays when
/// the server can't answer, and an answer for a queue that changed meanwhile is dropped.
pub(super) fn smart_order() {
    if !smart_on() || grouped() || is_linked() || no_server() { return; }
    let (at, rest): (usize, Vec<Rc<Value>>) = st(|s| {
        let at = (s.index + 1).max(0) as usize;
        (at, s.queue.get(at..).map(|r| r.to_vec()).unwrap_or_default())
    });
    let ids: Vec<i64> = rest.iter().filter_map(|t| t.g("id").as_i64()).collect();
    // songs from elsewhere (previews, catalogue songs) have no library id: leave such a queue as shuffled
    if ids.len() < 3 || ids.len() != rest.len() { return; }
    let ticket = REORDER.with(|r| { r.set(r.get() + 1); r.get() });
    dom::spawn(async move {
        let Ok(v) = api::post("/taste/shuffle", json!({ "ids": ids })).await else { return };
        if REORDER.with(|r| r.get()) != ticket { return; }
        let order: Vec<i64> = v.items().iter().filter_map(|x| x.as_i64()).collect();
        let applied = st_mut(|s| {
            // still the same queue after the same song?
            let now_at = (s.index + 1).max(0) as usize;
            let same = now_at == at && s.queue.len() == at + rest.len() && s.queue[at..].iter().zip(&rest).all(|(a, b)| Rc::ptr_eq(a, b));
            if !same { return false; }
            let mut pool: Vec<Option<Rc<Value>>> = rest.iter().cloned().map(Some).collect();
            let mut out: Vec<Rc<Value>> = Vec::with_capacity(order.len());
            for id in &order {
                if let Some(slot) = pool.iter_mut().find(|x| x.as_ref().is_some_and(|t| t.g("id").as_i64() == Some(*id))) {
                    if let Some(t) = slot.take() { out.push(t); }
                }
            }
            s.queue.truncate(at);
            s.queue.extend(out);
            true
        });
        if applied { render(); if st(|s| s.full) { render_full(); } emit(); preload_next(); save_spot(true); }
    });
}

/// "Start radio": this song, then songs like it from the library (its artist and the ones heard with it, the same
/// genres and era, leaning to your taste).
pub(super) async fn start_radio(t: Value) {
    if !t.g("id").is_number() { return; }
    match api::get(&format!("/taste/like-this/{}?limit=50", t.i("id"))).await {
        Ok(v) => {
            let mut list = vec![t];
            list.extend(v.items());
            if list.len() < 2 { toast("Nothing like it in the library yet", ""); return; }
            play_queue(list, 0, "list");
        }
        Err(e) => toast(&e.message, "err"),
    }
}

/// Plays one of the "For you" mixes (`/api/taste/mix`): `id` as the server named it.
pub async fn play_mix(id: &str) -> bool {
    match api::post("/taste/mix", json!({ "id": id, "limit": 100 })).await {
        Ok(v) => {
            let list = v.items();
            if list.is_empty() { toast("This mix is empty right now", ""); return false; }
            play_queue(list, 0, "list");
            true
        }
        Err(e) => { toast(&e.message, "err"); false }
    }
}

thread_local! {
    /// the genres of the genre shuffle playing (Genre shuffle…): the music after the queue keeps to them; any other
    /// list played clears it
    static GENRES: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// The genres the music after the queue keeps to (a genre shuffle is playing), if any.
pub(super) fn genre_scope() -> Option<Vec<String>> {
    GENRES.with(|g| g.borrow().clone())
}

/// Another list starts playing: the genre shuffle is over.
pub(super) fn clear_genre_scope() {
    GENRES.with(|g| *g.borrow_mut() = None);
}

/// Plays a genre shuffle: library songs of any of `genres`, weighted by taste with smart shuffle on (the server
/// always leaves disliked songs out); the music after the queue keeps to the same genres.
pub async fn play_genres(genres: Vec<String>) -> bool {
    if genres.is_empty() { return false; }
    match api::post("/taste/shuffle", json!({ "genres": genres, "limit": 100 })).await {
        Ok(v) => {
            let list = v.items();
            if list.is_empty() { toast("No songs of those genres right now", ""); return false; }
            play_queue(list, 0, "library");
            GENRES.with(|g| *g.borrow_mut() = Some(genres));
            true
        }
        Err(e) => { toast(&e.message, "err"); false }
    }
}

/// "Genre shuffle…": tick one or more of the library's genres (the ones you like most first) and shuffle them.
pub fn genre_shuffle_modal() {
    let m = crate::ui::modal(ModalOpts::new("Genre shuffle", r#"<div class="muted"><span class="spinner"></span> Loading your genres…</div>"#)
        .footer(r#"<button class="btn" data-close>Cancel</button><button class="btn primary" data-gs-play disabled>Shuffle</button>"#));
    dom::spawn(async move {
        let Some(body) = m.qs(".modal-body") else { return };
        let list = match api::get("/taste/genres").await {
            Ok(v) => v.items(),
            Err(e) => { set_html(&body, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))); return; }
        };
        if list.is_empty() {
            set_html(&body, r#"<p class="muted" style="margin:0">No genres yet: the songs' genre tags make them, and an artist's untagged songs take the artist's usual genres.</p>"#);
            return;
        }
        set_html(&body, &format!(r#"<p class="small muted" style="margin:0 0 8px">Tick one or more. The shuffle keeps to them, and so does the music after it.</p><div class="stack" style="gap:0">{}</div>"#,
            list.iter().map(|g| format!(r#"<label class="row" style="gap:10px;padding:6px 0;align-items:center;cursor:pointer"><input type="checkbox" data-genre="{}"><span class="grow">{}</span><span class="small muted">{} songs</span></label>"#,
                esc(&g.s("genre")), esc(&g.s("title")), g.i("songs"))).collect::<String>()));
        let ticked = { let m = m.clone(); move || -> Vec<String> { m.qsa("[data-genre]").iter().filter(|b| dom::checked(b)).map(|b| dom::data(b, "genre")).collect() } };
        {
            let (m2, ticked) = (m.clone(), ticked.clone());
            dom::on_each(&m.el, "[data-genre]", "change", move |_, _| {
                if let Some(b) = m2.qs("[data-gs-play]") { dom::set_disabled(&b, ticked().is_empty()); }
            });
        }
        if let Some(b) = m.qs("[data-gs-play]") {
            let m2 = m.clone();
            dom::on(&b, "click", move |_| {
                let genres = ticked();
                let m3 = m2.clone();
                dom::spawn(async move { if play_genres(genres).await { m3.close(); } });
            });
        }
    });
}
