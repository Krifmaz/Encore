//! Pick up where you left off: this device's own spot comes back paused when the app opens, or a bar offers to
//! continue what another device played since.

use super::*;

thread_local! {
    /// the pick-up bar's click handlers and its player subscription (dropping them removes them)
    static SPOT: RefCell<Option<(Vec<dom::EventListener>, player::OnChange)>> = const { RefCell::new(None) };
}

// ---- pick up where you left off ------------------------------------------------------------------
// The server keeps your last spot (song, position, queue, and the device that played it). When the app opens
// with nothing playing, this device's own spot comes back paused – or, when another device played since,
// a bar offers to continue that here.
const DISMISSED: &str = "encore.spot.dismissed";
const WEEK: f64 = 7.0 * 24.0 * 3600.0;

/// At start (after the player is mounted): put back where you left off, when Settings → Playback allows it.
pub fn resume() { dom::spawn(resume_async()); }
async fn resume_async() {
    if !crate::playback::current().truthy("resume") || player::current().is_some() || player::linked().is_some() { return; }
    let local = player::saved_spot();
    let spot = if app::has_feature("remote") { api::get("/remote/spot").await.unwrap_or(Value::Null) } else { Value::Null };
    if player::current().is_some() || player::linked().is_some() { return; }
    let at = spot.f("updated_at");
    let other = crate::json::truthy(&spot) && !spot.is_nullish("track_id") && spot.truthy("title") && spot.s("device_id") != me() && dom::now_ms() / 1000.0 - at < WEEK;
    // another device played since this one saved its spot: offer that one (unless you closed this very spot before)
    if other && !local.as_ref().is_some_and(|l| l.f("saved_at") >= at) {
        let gone = dom::ls_get(DISMISSED);
        if gone.as_deref() != Some(spot.s("updated_at").as_str()) { show_spot(spot); return; }
    }
    player::restore(local);
}

pub(super) fn hide_spot() {
    if let Some(b) = dom::by_id("spot-bar") { dom::remove(&b); }
    let old = SPOT.with(|x| x.borrow_mut().take());
    drop(old);
}

/// "Continue <song> from 2:31 · was playing on <device>" with Play here and ×.
fn show_spot(s: Value) {
    hide_spot();
    let bar = dom::create("div");
    bar.set_id("spot-bar");
    let _ = bar.set_attribute("role", "status");
    let device = if s.truthy("device_name") { s.s("device_name") } else { "another device".to_string() };
    dom::set_html(&bar, &format!(r#"<span>Continue <b>{}</b> from {} · was playing on {}</span>
    <button class="btn sm primary" id="spot-play">Play here</button>
    <button class="btn ghost icon sm" id="spot-close" aria-label="Close" title="Close">{}</button>"#,
        esc(&s.s("title")), fmt::dur(s.f("position")), esc(&device), icon("x")));
    let _ = dom::body().append_child(&bar);
    let mut ls = vec![];
    if let Some(b) = dom::qs_in(&bar, "#spot-play") {
        let s2 = s.clone();
        ls.push(dom::listener(&b, "click", move |_| dom::spawn(play_spot(s2.clone()))));
    }
    if let Some(b) = dom::qs_in(&bar, "#spot-close") {
        let when = s.s("updated_at");
        ls.push(dom::listener(&b, "click", move |_| {
            dom::ls_set(DISMISSED, &when);
            // after this handler returns: hiding drops the listener that is running now
            dom::spawn(async {
                hide_spot();
                // back to this device's own spot, as if the bar had never been offered
                if player::current().is_none() && player::linked().is_none() { player::restore(player::saved_spot()); }
            });
        }));
    }
    // something else started here, or this device became a remote: the offer is gone
    let off = player::on_change(|t| { if t.is_some() || player::linked().is_some() { hide_spot(); } });
    SPOT.with(|x| *x.borrow_mut() = Some((ls, off)));
}

/// Play here: the other device's queue, at its song and position, on this device – and that device pauses.
async fn play_spot(s: Value) {
    let btn = dom::by_id("spot-play");
    if let Some(b) = &btn { dom::set_disabled(b, true); }
    let tid = s.i("track_id");
    let mut ids: Vec<i64> = s.a("queue").iter().filter_map(|v| v.as_i64()).take(300).collect();
    if !ids.contains(&tid) { ids = vec![tid]; }
    let path = format!("/library/tracks?ids={}", ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    let got = match api::get(&path).await {
        Ok(r) => r.items(),
        Err(e) => {
            if let Some(b) = &btn { dom::set_disabled(b, false); }
            toast(&e.message, "err");
            return;
        }
    };
    // back in the queue's own order, repeats included (the server sends each song once)
    let by_id: std::collections::HashMap<i64, Value> = got.into_iter().filter(|t| t.g("id").is_number()).map(|t| (t.i("id"), t)).collect();
    let idx = if s.is_nullish("index") { s.f("idx") } else { s.f("index") };
    let mut tracks: Vec<Value> = vec![];
    let mut index = None;
    for (i, id) in ids.iter().enumerate() {
        let Some(t) = by_id.get(id) else { continue };
        if i as f64 == idx && *id == tid { index = Some(tracks.len()); }
        tracks.push(t.clone());
    }
    let index = index.or_else(|| tracks.iter().position(|t| t.i("id") == tid));
    hide_spot();
    let Some(index) = index else { toast("That song is no longer in your library", "err"); return };
    let mut at = s.f("position");
    // still playing there: it has moved on since its last check-in
    if s.truthy("online") && s.truthy("playing") && (s.is_nullish("device_playing") || s.truthy("device_playing")) {
        at += (dom::now_ms() / 1000.0 - s.f("updated_at")).max(0.0);
        let id = s.s("device_id");
        dom::spawn(async move { let _ = send_to(id, json!({ "action": "pause" })).await; });
    }
    let dur = tracks[index].f("duration");
    if dur != 0.0 && at > dur - 2.0 { at = 0.0; }
    player::command(json!({ "action": "play_queue", "tracks": tracks, "index": index, "position": at }));
}
