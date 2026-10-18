//! Linked: this device locked as a remote for another one (everything you play or press here goes there), and
//! remote control of this device – the snapshot the other devices see, and the commands they send.

use super::*;

/// `link = { id, name, state, online, send(cmd) }`: this device is locked as a remote for another one.
#[derive(Clone)]
pub struct Link { pub id: String, pub name: String, pub state: Value, pub online: bool, pub send: Rc<dyn Fn(Value)> }

thread_local! {
    pub(super) static LINK: RefCell<Option<Link>> = const { RefCell::new(None) };
    /// true while running a command another device sent us (never forward those)
    static LOCAL: Cell<bool> = const { Cell::new(false) };
    static LINK_TICK: RefCell<Option<Interval>> = const { RefCell::new(None) };
}

// ---- linked: this device is locked as a remote for another one ----
// Everything you play or press here goes to that device; nothing plays locally until you unlink.
pub(super) fn is_linked() -> bool { LINK.with(|l| l.borrow().is_some()) }
pub(super) fn is_local() -> bool { LOCAL.with(|l| l.get()) }
/// Forward a command to the linked device – false when this device plays itself (or runs another device's command).
pub(super) fn via_link(c: Value) -> bool {
    if is_local() { return false; }
    let send = LINK.with(|l| l.borrow().as_ref().map(|l| l.send.clone()));
    let Some(send) = send else { return false };
    send(c);
    true
}
pub fn linked() -> Option<Link> { LINK.with(|l| l.borrow().clone()) }
pub(super) fn link_state_value() -> Value { LINK.with(|l| l.borrow().as_ref().map(|l| l.state.clone()).unwrap_or(Value::Null)) }
pub(super) fn link_name() -> String { LINK.with(|l| l.borrow().as_ref().map(|l| l.name.clone()).unwrap_or_default()) }
pub fn set_link(l: Option<Link>) {
    let was = is_linked();
    let now = l.is_some();
    let old = LINK.with(|x| x.replace(l));
    drop(old);
    if now && !was {
        if let Some(a) = audio() {
            st_mut(|s| s.want = false);
            let t = REC.with(|r| r.borrow_mut().timer.take());
            drop(t);
            if !a.paused() { let _ = a.pause(); }
        }
    }
    let old_tick = LINK_TICK.with(|t| t.borrow_mut().take());
    drop(old_tick);
    // between check-ins, move the remote's seek bar along on our own
    if now {
        let tick = Interval::new(1000, || {
            let playing = LINK.with(|l| l.borrow().as_ref().is_some_and(|l| l.state.truthy("playing")));
            if playing {
                LINK.with(|l| {
                    if let Some(l) = l.borrow_mut().as_mut() {
                        let p = l.state.f("position") + 1.0;
                        if let Some(o) = l.state.as_object_mut() { o.insert("position".into(), json!(p)); }
                    }
                });
                update_linked_time();
                sync_lyrics(false);
            }
        });
        LINK_TICK.with(|t| *t.borrow_mut() = Some(tick));
    }
    if now { cancel_preload(); }
    LYR.with(|l| *l.borrow_mut() = Lyr::default());
    render();
    if st(|s| s.full) { render_full(); load_lyrics(); }
}
/// `[id, playing, online, volume, queue.length, index, shuffle, repeat]` – what a re-render of the bar depends on.
fn link_key(l: &Link) -> (Value, Value, bool, Value, usize, Value, Value, Value) {
    (l.state.g("id").clone(), l.state.g("playing").clone(), l.online, l.state.g("volume").clone(), l.state.a("queue").len(), l.state.g("index").clone(),
     l.state.g("shuffle").clone(), l.state.g("repeat").clone())
}
/// New state from the linked device (every check-in).
pub fn link_state(s: Value, online: bool) {
    let changed = LINK.with(|l| {
        let mut guard = l.borrow_mut();
        let l = guard.as_mut()?;
        let before = link_key(l);
        l.state = if json::truthy(&s) { s } else { json!({}) };
        l.online = online;
        Some(before != link_key(l))
    });
    match changed {
        None => {}
        Some(true) => { render(); if st(|s| s.full) { render_full(); load_lyrics(); } }
        Some(false) => update_linked_time(),
    }
}
/// The linked device's song as a track object (what the full-screen view, likes and the ⋯ menu work on).
pub(super) fn remote_track() -> Option<Value> {
    let s = link_state_value();
    if !s.truthy("title") { return None; }
    Some(json!({ "id": s.g("id"), "title": s.g("title"), "artist": s.g("artist"), "artists": s.g("artists"), "album": s.g("album"), "album_id": s.g("album_id"), "format": s.g("format"), "duration": s.g("duration"), "cover": s.g("cover"), "remote": true }))
}
/// The song in front of you: the linked device's when linked, else this device's.
pub(super) fn now_track() -> Option<Value> { if is_linked() { remote_track() } else { current() } }
pub(super) fn now_position() -> f64 { if is_linked() { link_state_value().f("position") } else { position() } }
pub(super) fn now_playing() -> bool { if is_linked() { link_state_value().truthy("playing") } else { audio().is_some() && shows_playing() } }

// ---- remote control: what this device is playing, and commands from your other devices ----
const SLIM_KEYS: [&str; 7] = ["id", "title", "artist", "album", "album_id", "format", "duration"];
pub(super) fn slim(t: &Value) -> Value {
    let mut m = Map::new();
    for k in SLIM_KEYS { if t.has(k) { m.insert(k.into(), t.g(k).clone()); } }
    Value::Object(m)
}
pub fn snapshot() -> Value {
    let (Some(t), Some(_)) = (cur(), audio()) else { return json!({ "playing": false }) };
    let mut m = match slim(&t) { Value::Object(m) => m, _ => Map::new() };
    // `...withArtists(t)`: each artist of the song for links, only when it has them
    if !t.a("artists").is_empty() { m.insert("artists".into(), t.g("artists").clone()); }
    // `cover: cover(t)` – an undefined cover is left out, like JSON.stringify does
    let cov = cover_v(&t);
    if t.truthy("preview") || !t.truthy("album_id") { if t.has("cover") { m.insert("cover".into(), cov); } } else { m.insert("cover".into(), cov); }
    m.insert("playing".into(), json!(shows_playing()));
    m.insert("position".into(), json!(spot_position()));
    m.insert("duration".into(), json!(duration()));
    m.insert("volume".into(), json!(user_vol()));
    let (shuffle, repeat) = st(|s| (s.shuffle, s.repeat.clone()));
    m.insert("shuffle".into(), json!(shuffle));
    m.insert("repeat".into(), json!(repeat));
    let rate = CTX.with(|c| c.borrow().as_ref().map(|c| c.sample_rate() as f64));
    m.insert("signal".into(), json!({ "label": quality(&t), "transport": transport().map(|(k, _)| k), "output_rate": rate }));
    let (q, index) = lib_window();
    m.insert("index".into(), json!(index));
    m.insert("queue".into(), Value::Array(q));
    Value::Object(m)
}

pub fn command(c: Value) {
    LOCAL.with(|l| l.set(true));
    run_command(&c);
    LOCAL.with(|l| l.set(false));
}
fn run_command(c: &Value) {
    match c.s("action").as_str() {
        "toggle" => toggle(),
        "play" => { if cur().is_some() { user_play(); } }
        "pause" | "stop" => { if audio().is_some() { user_pause(); } }
        "next" => next(false),
        "prev" => prev(),
        "seek" => seek_to(c.f("value")),
        "volume" => set_volume(c.f("value")),
        "shuffle" => set_shuffle(c.truthy("value")),
        "repeat" => set_repeat(&c.s("value")),
        "enqueue" => { if c.g("tracks").is_array() { enqueue(c.a("tracks")); } }
        "play_next" => { if c.g("tracks").is_array() { play_next(c.a("tracks")); } }
        "play_queue" => {
            if !c.g("tracks").is_array() || c.a("tracks").is_empty() { return; }
            play_queue(c.a("tracks"), c.i("index").max(0) as usize, "list");
            if c.f("position") > 1.0 {
                let at = c.f("position");
                if let Some(a) = audio() { EventListener::once(&a, "loadedmetadata", move |_| seek_to(at)).forget(); }
            }
        }
        _ => {}
    }
}
