//! The player in a group listen (crate::group holds the group): it follows the group's queue, song, position and
//! play / pause, and what the listener does here – the bar, the full player, Up next, the keyboard, the lock screen,
//! another device's remote commands – goes to the group when this device may control it.
//!
//! Pause (the owner on Oct 21: "it may pause for me but dont stop the other users playback"): every pause in a group
//! pauses only this browser – the Pause button ("Paused for you – the group keeps playing"), the keyboard, the lock
//! screen and the OS media controls, headphones taken out, another app or a call taking the sound, the browser
//! refusing to start – and is never sent to the group: "Paused here · Catch up". Play (or Catch up) then rejoins the
//! group where it is now, with one hard seek there. The group's own pause is "Pause for everyone" (the player's ⋯ and
//! the group panel) for those who control it.
//!
//! Play on a list (the owner on Oct 21: "it should just add it to the que"): an album's, a playlist's, an artist's or a
//! song's Play adds the songs from the one asked for to the group's Up next (`enqueue`), and Shuffle adds them
//! shuffled; only a group that plays nothing and has nothing in Up next starts with them (`set_queue`,
//! rules::play_starts). "Added 12 songs to the group's Up next".
//!
//! Sync rule (the same on web, desktop and phone, client-shapes §5.6, rules::sync_step): expected position = position +
//! (server_now − anchor_ms) / 1000 while playing. Under 40 ms nothing; 40 ms – 1.5 s a `playbackRate` nudge
//! proportional to the drift (0.98–1.02, eased in and out, the pitch kept) until within 20 ms, never a seek; above
//! 1.5 s one hard seek aimed ahead by the running median of what this browser's last 5 seeks took (rules::SeekLag),
//! then none for 8 s. A server-made stream (a conversion, Auto's lighter rung, Data saver, server processing) is
//! never seeked for less than 3 s: its seek is a new stream. Auto quality keeps its rung for the whole song
//! (rules::QualityLock): the original when this browser plays the file, one drop at most on a real stall. Only the
//! listener's own seek goes to the group as "seek" – reloads, recovery and corrections never do. Crossfade, the
//! library shuffle after the queue and repeat are off in a group; Shuffle shuffles the group's Up next (for those who
//! control it); gapless stays, and a song change within 3 s of the expected end uses the pre-buffered next song.

use super::*;
use crate::group::{self as g, rules};

thread_local! {
    /// the group the player follows now
    static ACTIVE: RefCell<Option<String>> = const { RefCell::new(None) };
    static TICK: RefCell<Option<Interval>> = const { RefCell::new(None) };
    /// Auto quality's rung, kept for the whole song
    static QLOCK: RefCell<rules::QualityLock> = RefCell::new(rules::QualityLock::default());
    /// the group's last (entry, position, anchor): a new one (the host seeked, played, jumped) is followed at once
    static LAST_ANCHOR: RefCell<(String, f64, f64)> = const { RefCell::new((String::new(), 0.0, 0.0)) };
    /// no drift check before then (performance.now() ms): a seek or a new song needs a moment to buffer
    static QUIET_UNTIL: Cell<f64> = const { Cell::new(0.0) };
    /// a change made here went to the group: until its answer (a rev above this) or the time, the older state is not followed
    static HOLD: Cell<(f64, i64)> = const { Cell::new((0.0, -1)) };
    static LAG: RefCell<rules::SeekLag> = RefCell::new(rules::SeekLag::default());
    /// when this module last paused the element (performance.now() ms): the 'pause' event that follows is not the
    /// listener's (an element swapped out right after never sends it to us, so a flag could swallow a later one)
    static OWN_PAUSE_AT: Cell<f64> = const { Cell::new(f64::NEG_INFINITY) };
}

fn mono() -> f64 { dom::window().performance().map(|p| p.now()).unwrap_or_else(now_ms) }
fn quiet(ms: f64) { QUIET_UNTIL.with(|q| q.set(q.get().max(mono() + ms))); }
fn qid(t: &Value) -> String { t.s("qid") }

/// In a group listen now (the player follows it).
pub(super) fn grouped() -> bool { g::in_group() }

// ---- following the group --------------------------------------------------------------------------------------------

/// The group's state (or this member's own pause) changed: the player follows it, and the bar and the full player show it.
pub fn group_changed() {
    match g::current() {
        None => leave_mode(),
        Some(s) => { enter_mode(&s); follow(&s); }
    }
    render();
    if st(|s| s.full) { render_full(); }
}

/// The group's state wins again at once (a refused command, Catch up).
pub fn group_resync() {
    HOLD.with(|h| h.set((0.0, -1)));
    if let Some(s) = g::current() { follow(&s); }
    render();
    if st(|s| s.full) { render_full(); }
}

fn enter_mode(s: &Value) {
    let id = s.s("id");
    if ACTIVE.with(|a| a.borrow().as_deref() == Some(id.as_str())) { return; }
    ACTIVE.with(|a| *a.borrow_mut() = Some(id));
    // no repeat and no shuffle mode in a group (Shuffle shuffles the group's Up next once); a running crossfade ends
    st_mut(|x| { x.repeat = "off".into(); x.shuffle = false; x.order = None; x.context = "list".into(); });
    end_fade();
    LAG.with(|l| *l.borrow_mut() = rules::SeekLag::default());
    QLOCK.with(|q| *q.borrow_mut() = rules::QualityLock::default());
    LAST_ANCHOR.with(|a| *a.borrow_mut() = (String::new(), 0.0, 0.0));
    HOLD.with(|h| h.set((0.0, -1)));
    TICK.with(|t| *t.borrow_mut() = Some(Interval::new(500, check)));
}

/// Out of the group: the music stops here, the queue stays (play goes on alone).
fn leave_mode() {
    if ACTIVE.with(|a| a.borrow_mut().take()).is_none() { return; }
    TICK.with(|t| t.borrow_mut().take());
    LAG.with(|l| l.borrow_mut().cancel());
    set_rate(1.0);
    own_pause();
    st_mut(|s| s.want = false);
    emit();
    media_state();
}

/// Play only while the group plays, this member has not paused here, and the group is not past its last song.
fn wants_play(s: &Value, ei: i64, epos: f64) -> bool { s.truthy("playing") && !g::local_paused() && !rules::at_end(s, ei, epos) }

fn holding(s: &Value) -> bool { HOLD.with(|h| { let (until, rev) = h.get(); mono() < until && s.i("rev") <= rev }) }

/// A change made here went to the group: its answer is awaited before the player follows the group again.
fn hold(s: &Value) {
    HOLD.with(|h| h.set((mono() + 3000.0, s.i("rev"))));
    quiet(2500.0);
    LAG.with(|l| l.borrow_mut().cancel());
}

/// The local queue holds the group's entries, in order.
fn same_queue(q: &[Value]) -> bool {
    st(|s| s.queue.len() == q.len() && s.queue.iter().zip(q).all(|(a, b)| qid(a) == qid(b) && a.g("id") == b.g("id")))
}

/// The group's queue as the player's: entries already here keep their object (the song playing stays the same one).
/// `ei`: the group's entry now – the song that played here before the group started (or was joined) is that entry
/// when it is the same song, so it plays on without being loaded again.
fn set_queue(q: &[Value], ei: i64) {
    st_mut(|s| {
        let current = s.queue.get(s.index.max(0) as usize).filter(|_| s.index >= 0).cloned();
        let old: HashMap<String, Rc<Value>> = s.queue.iter().filter(|t| !qid(t).is_empty()).map(|t| (qid(t), t.clone())).collect();
        // (the same song added by the same person: an entry of an earlier group with the same id is not reused)
        s.queue = q.iter().map(|row| match old.get(&qid(row)) {
            Some(t) if t.g("id") == row.g("id") && t.g("added_by") == row.g("added_by") => t.clone(),
            _ => Rc::new(row.clone()),
        }).collect();
        s.index = match current {
            Some(c) if !qid(&c).is_empty() => s.queue.iter().position(|t| qid(t) == qid(&c)).map(|i| i as i64).unwrap_or(-1),
            Some(c) if q.get(ei.max(0) as usize).is_some_and(|e| e.g("id") == c.g("id") && c.g("id").is_number()) => ei,
            _ => -1,
        };
        s.order = None;
    });
    // (the next song is fetched once this one is safely ahead, not at every change of the queue)
    maybe_preload(false);
}

/// Where the song is here (for a song held before its file has loaded: where it was put).
fn here() -> f64 { spot_position() }

/// A pause this module makes (never counted as the listener's own).
fn own_pause() {
    if let Some(a) = audio() { if !a.paused() { OWN_PAUSE_AT.with(|o| o.set(mono())); let _ = a.pause(); } }
}

/// The 'pause' event of the element playing (a real pause, not the end of a song or a stream picked up again): a
/// pause nobody here asked for (headphones out, another app took the sound) while the group plays pauses only this
/// browser. Not one this module made a moment ago.
pub(super) fn paused_by_listener() {
    if !grouped() || mono() - OWN_PAUSE_AT.with(|o| o.get()) < 1000.0 { return; }
    if g::current().is_some_and(|s| s.truthy("playing")) && !g::local_paused() { g::set_local_paused(true); }
}

/// The browser refused to start playing (autoplay rules): paused here until play is pressed.
pub(super) fn play_refused() { if grouped() { g::set_local_paused(true); } }

/// Within the song playing: to `sec`, playing or not. A whole file seeks in the browser; a server-made stream is asked
/// for again at `t=` (held when it should not play).
fn place(sec: f64, play: bool) {
    let Some(a) = audio() else { return };
    st_mut(|s| { s.seek_at = now_ms(); s.at = sec; });
    if !play { own_pause(); }
    if a.ready_state() < 1 && !st(|s| s.transcoded) {
        let (rung, dsp) = st(|s| (s.rung, s.dsp));
        load_opts(LoadOpts { offset: sec, rung: Some(rung), dsp: Some(dsp), resume: true, hold: !play, ..Default::default() });
        return;
    }
    seek_now_opts(sec, !play);
    if play && !st(|s| s.transcoded) && a.paused() { play_el(&a); }
}

/// Whether a seek to entry `ei` would be a new stream made by the server: the stream playing when it is the same
/// entry, else the one that entry starts with.
fn made_by_server(ei: i64, e: &Value) -> bool {
    if st(|x| x.index) == ei && cur().is_some() { here_server_made() } else { starts_server_made(e) }
}

/// To where the playing group is in entry `ei`, ahead by what a seek of that kind has been taking here.
fn seek_group(s: &Value, ei: i64, epos: f64) {
    let q = s.a("queue");
    let Some(e) = q.get(ei.max(0) as usize) else { return };
    let server_made = made_by_server(ei, e);
    let target = LAG.with(|l| l.borrow_mut().target(&qid(e), epos * 1000.0, server_made, mono())) / 1000.0;
    if st(|x| x.index) == ei { place(target, true); } else { load_entry(ei, target, true); }
    quiet(2500.0);
}

/// The player on entry `ei` at `at` seconds, playing or held. Within 3 s of a song's start the pre-buffered next song
/// plays (a whole file is then put at `at` in what it has buffered; a server-made stream starts at its start and the
/// nudge closes the rest).
fn load_entry(ei: i64, at: f64, play: bool) {
    st_mut(|x| x.index = ei);
    set_rate(1.0);
    let at = match cur() {
        Some(t) if at > 0.0 && at <= rules::BOUNDARY_S && spare_has(&t) && starts_server_made(&t) => {
            // (not the seek it was aimed as: nothing to learn from where it lands)
            LAG.with(|l| l.borrow_mut().cancel());
            0.0
        }
        _ => at,
    };
    load_opts(LoadOpts { offset: if at > 0.3 { at } else { 0.0 }, hold: !play, ..Default::default() });
    quiet(2500.0);
}

/// The group's queue, song, position and play state on this player.
fn follow(s: &Value) {
    let q = s.a("queue");
    let (ei, epos) = rules::expected(s, g::server_now());
    if ei < 0 {
        // an empty queue: nothing plays
        own_pause();
        if st(|x| !x.queue.is_empty()) {
            if let Some(a) = audio() { let _ = a.remove_attribute("src"); a.load(); }
            st_mut(|x| { x.queue.clear(); x.index = -1; x.want = false; });
            emit();
            media_state();
        }
        return;
    }
    // the group has not answered a change made here yet: only its queue is taken over
    if holding(s) {
        if !same_queue(&q) && st(|x| x.queue.get(x.index.max(0) as usize).map(|t| qid(t))) == q.get(ei as usize).map(qid) { set_queue(&q, ei); }
        return;
    }
    let want = wants_play(s, ei, epos);
    if !same_queue(&q) { set_queue(&q, ei); }
    if st(|x| x.index) != ei || cur().is_none() {
        // a song change a moment before (or after) the group's settles by itself (the tick looks again)
        if want && cur().is_some() && audio().is_some_and(|a| !a.paused()) && rules::near_boundary(st(|x| x.index), here(), duration(), ei, epos) { return; }
        let e = &q[ei as usize];
        let target = if want { LAG.with(|l| l.borrow_mut().target(&qid(e), epos * 1000.0, starts_server_made(e), mono())) / 1000.0 } else { epos };
        load_entry(ei, target, want);
        return;
    }
    let Some(a) = audio() else { return };
    if want && a.paused() && !rec_need() {
        // after a pause here (or a song held), back where the group is
        if (here() - epos).abs() > 0.5 { seek_group(s, ei, epos); } else { play_el(&a); }
        quiet(1500.0);
    } else if !want && !a.paused() {
        own_pause();
        st_mut(|x| x.want = false);
        LAG.with(|l| l.borrow_mut().cancel());
        media_state();
    } else if want && !a.paused() && mono() >= QUIET_UNTIL.with(|x| x.get()) && a.ready_state() >= 3 && !a.seeking() {
        // the host seeked: there at once when it is past the seek threshold (a smaller drift is the tick's nudge); a new
        // anchor is followed even right after a correction seek
        let song = qid(&q[ei as usize]);
        let d = (position() - epos) * 1000.0;
        if LAG.with(|l| l.borrow().waiting()) { LAG.with(|l| l.borrow_mut().landed(&song, d)); }
        let since = if new_anchor(s, &song) { f64::INFINITY } else { LAG.with(|l| l.borrow().since_ms(mono())) };
        if rules::sync_step(d, a.playback_rate(), since, here_server_made()) == rules::Act::Seek { set_rate(1.0); seek_group(s, ei, epos); }
    }
    // a paused group: everyone at the same spot
    if !s.truthy("playing") && !a.seeking() && (here() - epos).abs() > 0.5 && !rec_need() { place(epos, false); }
}

/// Whether the group's (entry, position, anchor) changed since the last look: the host seeked, played or jumped.
fn new_anchor(s: &Value, song: &str) -> bool {
    let now = (song.to_string(), s.f("position"), s.f("anchor_ms"));
    LAST_ANCHOR.with(|a| { let mut a = a.borrow_mut(); let fresh = *a != now; *a = now; fresh })
}

/// Every half second while in a group: the song, the drift, the rate.
fn check() {
    let Some(s) = g::current() else { return };
    if holding(&s) { return; }
    let q = s.a("queue");
    if !same_queue(&q) { follow(&s); render(); return; }
    let (ei, epos) = rules::expected(&s, g::server_now());
    if ei < 0 { return; }
    let Some(a) = audio() else { return };
    if !wants_play(&s, ei, epos) {
        set_rate(1.0);
        LAG.with(|l| l.borrow_mut().cancel());
        if !a.paused() { follow(&s); render(); }
        return;
    }
    if rec_need() { return; }
    // paused while it should play (held, or a start that has not come yet): once the last try had its moment
    if a.paused() { if mono() >= QUIET_UNTIL.with(|x| x.get()) { follow(&s); render(); } return; }
    if mono() < QUIET_UNTIL.with(|x| x.get()) || a.ready_state() < 3 || a.seeking() { return; }
    let cur_i = st(|x| x.index);
    let pos = position();
    if cur_i != ei {
        // gapless into the next song a little before (or after) the group: wait for the song change to settle
        if !rules::near_boundary(cur_i, pos, duration(), ei, epos) { seek_group(&s, ei, epos); render(); }
        return;
    }
    let song = qid(&q[ei as usize]);
    let d = (pos - epos) * 1000.0;
    if LAG.with(|l| l.borrow().waiting()) { LAG.with(|l| l.borrow_mut().landed(&song, d)); }
    let since = if new_anchor(&s, &song) { f64::INFINITY } else { LAG.with(|l| l.borrow().since_ms(mono())) };
    match rules::sync_step(d, a.playback_rate(), since, here_server_made()) {
        rules::Act::Seek => { set_rate(1.0); seek_group(&s, ei, epos); }
        rules::Act::Rate(r) => set_rate(r),
        rules::Act::Keep => {}
    }
}

/// The pitch stays when the rate moves (`preservesPitch`, on by default where it exists; set again to be sure).
fn keep_pitch(a: &HtmlMediaElement) {
    for k in ["preservesPitch", "mozPreservesPitch", "webkitPreservesPitch"] {
        if js_sys::Reflect::has(a, &k.into()).unwrap_or(false) {
            let _ = js_sys::Reflect::set(a, &k.into(), &JsValue::TRUE);
            return;
        }
    }
}

/// The playing element's rate (the spare stays at 1.0, so the next song starts at normal speed).
fn set_rate(r: f64) {
    if let Some(a) = audio() { if a.playback_rate() != r { if r != 1.0 { keep_pitch(&a); } a.set_playback_rate(r); } }
    if let Some(sp) = spare() { if sp.playback_rate() != 1.0 { sp.set_playback_rate(1.0); } }
}

// ---- what the listener does here ------------------------------------------------------------------------------------

fn deny() { toast(&g::only_host(), ""); }

/// Play / pause in a group; false when not in one.
pub(super) fn g_toggle() -> bool {
    if !grouped() { return false; }
    if shows_playing() { g_pause(); } else { g_play(); }
    true
}

/// Pause in a group: only here, for everyone who presses it ("Paused for you – the group keeps playing"); the group
/// plays on. The group's own pause is [`pause_for_everyone`].
pub(super) fn g_pause() -> bool {
    let Some(s) = g::current() else { return false };
    own_pause();
    st_mut(|x| x.want = false);
    if s.truthy("playing") && !g::local_paused() {
        g::set_local_paused(true);
        toast(rules::PAUSED_FOR_YOU, "");
    }
    render();
    media_state();
    true
}

/// "Pause for everyone" (the player's ⋯ and the group panel): the group's pause, for those who control it.
pub fn pause_for_everyone() {
    let Some(s) = g::current() else { return };
    if !g::can_control() { deny(); return; }
    hold(&s);
    own_pause();
    st_mut(|x| x.want = false);
    // (the group pauses: nothing is paused here only any more)
    g::set_local_paused(false);
    g::command("pause", json!({}));
    render();
    media_state();
}

/// Play: after a pause here, back with the group where it is now (one hard seek there: "Catch up"); a paused group
/// plays for those who control it.
pub(super) fn g_play() -> bool {
    let Some(s) = g::current() else { return false };
    if g::local_paused() {
        g::catch_up();
        if s.truthy("playing") { return true; }
    }
    if !s.truthy("playing") {
        // (after the queue ran out the server plays the first song added since, or the last song again from its
        // start when nothing was added)
        if g::can_control() { hold(&s); g::command("play", json!({})); } else { deny(); }
        return true;
    }
    // the group plays and this browser does not (the browser refused to start): play with it now
    HOLD.with(|h| h.set((0.0, -1)));
    follow(&s);
    true
}

/// A seek by the listener: the group's when allowed.
pub(super) fn g_seek(sec: f64) -> bool {
    let Some(s) = g::current() else { return false };
    if !g::can_control() { deny(); update_time(); return true; }
    let (ei, epos) = rules::expected(&s, g::server_now());
    hold(&s);
    place(sec.max(0.0), wants_play(&s, ei, epos));
    g::command("seek", json!({ "position": sec.max(0.0) }));
    update_time();
    true
}

/// Next: the group's. At the end of a song here (`auto`) the player goes on to the next entry by itself, as the
/// server does at the same moment (no command), so gapless playback is not interrupted.
pub(super) fn g_next(auto: bool) -> bool {
    if !grouped() { return false; }
    if auto {
        let (i, n) = st(|x| (x.index, x.queue.len() as i64));
        if i + 1 < n { load_entry(i + 1, 0.0, true); } else { st_mut(|x| x.want = false); media_state(); render(); }
        return true;
    }
    if !g::can_control() { deny(); return true; }
    g::command("next", json!({}));
    true
}

pub(super) fn g_prev() -> bool {
    if !grouped() { return false; }
    if !g::can_control() { deny(); return true; }
    g::command("prev", json!({}));
    true
}

/// Up next: play entry `i` now, take it out, or move it (by its queue id, client-shapes §3.15).
pub(super) fn g_entry(op: &str, i: i64, to: i64) -> bool {
    if !grouped() { return false; }
    if !g::can_control() { deny(); return true; }
    let Some(e) = st(|x| x.queue.get(i.max(0) as usize).cloned()) else { return true };
    let fields = match op {
        "move" => json!({ "qid": qid(&e), "from": i, "to": to }),
        _ => json!({ "qid": qid(&e), "index": i }),
    };
    g::command(op, fields);
    true
}

/// The library songs of `tracks` (ids).
fn lib_ids(tracks: &[Value]) -> Vec<i64> { tracks.iter().filter(|t| t.g("id").is_number() && !t.truthy("preview")).map(|t| t.i("id")).collect() }

/// Whether Play in the group starts it with the songs (`set_queue`) rather than adding them to its Up next: the group
/// plays nothing and has nothing in Up next (rules::play_starts), and this device may control it.
fn play_starts() -> bool {
    g::current().is_some_and(|s| rules::play_starts(&s, g::server_now())) && g::can_control()
}

/// Adds `ids` to the group's Up next and says so once the group took them ("Added 12 songs to the group's Up next").
fn add_to_group(ids: Vec<i64>, shuffled: bool) {
    let n = ids.len();
    let mut fields = json!({ "tracks": ids });
    if shuffled { fields["shuffle"] = json!(true); }
    g::command_then("enqueue", fields, move || toast(&rules::added_text(n, shuffled), "ok"));
}

/// "Play" on a list (an album, a playlist, an artist, a song) while in a group: the songs from the one asked for are
/// added to the group's Up next; a group with nothing to play starts with the list instead (for those who control it).
pub(super) fn g_play_queue(tracks: &[Value], start: usize, context: &str, position: f64) -> bool {
    if !grouped() { return false; }
    let ids = lib_ids(tracks);
    if context == "preview" || ids.is_empty() { toast("Only songs from your library can be played in a group listen", "err"); return true; }
    let at = tracks.iter().take(start).filter(|t| t.g("id").is_number() && !t.truthy("preview")).count();
    if play_starts() {
        let (win, at) = rules::window(&ids, at, 1000);
        g::command("set_queue", json!({ "tracks": win, "index": at, "position": position.max(0.0) }));
        return true;
    }
    let add = rules::to_add(&ids, at, 1000);
    if add.is_empty() { toast("Only songs from your library can be played in a group listen", "err"); return true; }
    add_to_group(add, false);
    true
}

/// Add to queue / Play next: every member may.
pub(super) fn g_add(op: &str, tracks: &[Value]) -> bool {
    if !grouped() { return false; }
    let ids = lib_ids(tracks);
    if ids.is_empty() { toast("Only songs from your library can be played in a group listen", "err"); return true; }
    g::command(op, json!({ "tracks": ids }));
    true
}

/// What the player's Repeat says in a group.
pub(super) const REPEAT_OFF: &str = "Repeat is off during a group listen";

/// Repeat: off during a group listen.
pub(super) fn g_repeat() -> bool {
    if !grouped() { return false; }
    toast(REPEAT_OFF, "");
    true
}

/// Shuffle in a group: the group's Up next in a random order, once (the server makes it, the same for everyone) –
/// for those who control the group.
pub(super) fn g_shuffle() -> bool {
    if !grouped() { return false; }
    if !g::can_control() { deny(); return true; }
    g::command("shuffle", json!({}));
    true
}

/// Clear of the group's Up next (for those who control it): asked first, then everything after the song playing goes.
pub(super) fn g_clear() -> bool {
    if !grouped() { return false; }
    if !g::can_control() { deny(); return true; }
    dom::spawn(async {
        if crate::ui::confirm_dialog("Clear the group's Up next?", "", "Clear", true).await { g::command("clear", json!({})); }
    });
    true
}

/// A Shuffle button of an album, an artist or a playlist in a group: the songs are added to the group's Up next in a
/// random order (`enqueue` with `shuffle`); a group with nothing to play starts with them shuffled instead (`set_queue`
/// with `shuffle`, for those who control it). At most 1000 songs, picked at random from the whole list. False outside
/// a group.
pub(super) fn g_shuffle_play(tracks: &[Value]) -> bool {
    if !grouped() { return false; }
    let ids = lib_ids(tracks);
    if ids.is_empty() { toast("Only songs from your library can be played in a group listen", "err"); return true; }
    // shuffled here too, and a long list sends 1000 songs picked from all of it (the server shuffles them again)
    let win = rules::pick_shuffled(&ids, 1000, js_sys::Math::random);
    if play_starts() {
        g::command("set_queue", json!({ "tracks": win, "index": 0, "position": 0.0, "shuffle": true }));
    } else {
        // said once the group took them (a full queue or an ended group refuses)
        add_to_group(win, true);
    }
    true
}

// ---- quality and recovery in a group ----------------------------------------------------------------------------------

/// The rung a song starts on in a group with Auto quality: the original when this browser plays the file (and the song
/// before had no stall), else what the estimate allows. None outside a group or with another quality setting.
pub(super) fn group_start_rung(t: &Value, auto_rung: i64) -> Option<i64> {
    if !grouped() || prefs().s("quality") != "auto" { return None; }
    Some(QLOCK.with(|q| q.borrow().start_rung(&qid(t), !convert(t), auto_rung)))
}

/// A song starts on this player (its rung is kept for the whole song in a group).
pub(super) fn group_song_started(t: &Value) {
    if grouped() { QLOCK.with(|q| q.borrow_mut().start(&qid(t))); }
}

/// May Auto quality drop the rung now? Outside a group always; in one only for a real stall, once per song.
pub(super) fn group_may_drop() -> bool {
    if !grouped() { return true; }
    let song = cur().map(|t| qid(&t)).unwrap_or_default();
    QLOCK.with(|q| q.borrow_mut().may_change(&song, true))
}

/// Where a reload of the song playing (a stream picked up again, a rung dropped) should start in a group: where the
/// group is now, ahead by what such a reload takes – not where it broke off, which would land behind and cost a second
/// seek. None outside a group, or when the group is on another song (or paused).
pub(super) fn group_spot(server_made: bool) -> Option<f64> {
    let s = g::current()?;
    let (ei, epos) = rules::expected(&s, g::server_now());
    let e = s.a("queue").get(ei.max(0) as usize).cloned()?;
    if ei < 0 || st(|x| x.index) != ei || !s.truthy("playing") || g::local_paused() { return None; }
    quiet(2500.0);
    Some(LAG.with(|l| l.borrow_mut().target(&qid(&e), epos * 1000.0, server_made, mono())) / 1000.0)
}

// ---- what the bar and the full player show ----------------------------------------------------------------------------

/// ` disabled title="Only Alex controls playback"` on a control this member may not use (else "").
pub(super) fn ctl_attr() -> String {
    if !grouped() || g::can_control() { return String::new(); }
    format!(r#" disabled title="{}""#, esc(&g::only_host()))
}

/// Repeat in a group: off (` disabled`, else "").
pub(super) fn repeat_attr() -> &'static str { if grouped() { " disabled" } else { "" } }
/// Its tooltip: the reason in a group, else `title`.
pub(super) fn repeat_tip(title: &str) -> String { if grouped() { REPEAT_OFF.into() } else { title.to_string() } }
/// Shuffle in a group: Shuffle of the group's Up next for those who control it (` disabled` for anyone else).
pub(super) fn shuffle_attr() -> &'static str { if grouped() && !g::can_control() { " disabled" } else { "" } }
/// Its tooltip: "Shuffle" (the group's Up next, once) in a group, or why it is off; else `title`.
pub(super) fn shuffle_tip(title: &str) -> String {
    if !grouped() { return title.to_string(); }
    if g::can_control() { "Shuffle".into() } else { g::only_host() }
}

/// The pill "Group · 3" that opens the group listen (nothing outside a group).
pub(super) fn pill_html(id: &str) -> String {
    let Some(s) = g::current() else { return String::new() };
    let whose = if s.g("you").truthy("host") { "Your group listen".to_string() } else { format!("{}'s group listen", rules::host_name(&s)) };
    format!(r#"<button class="badge accent group-pill" id="{id}" type="button" title="{}">{}<span>{}</span></button>"#,
        esc(&whose), icon_attr("users", r#"width="13" height="13""#), esc(&rules::pill(&s)))
}

/// "Paused here · Catch up", hidden: always the same in the drawn HTML, so a pause here leaves a view's HTML as it
/// was and `full::draw` only swaps the play/pause button (no cover is made again). [`show_paused`] shows it.
pub(super) fn paused_html(id: &str) -> String {
    format!(r#"<div class="small group-paused hidden" id="{id}-slot"><span>Paused here</span> · <button type="button" class="linkish" id="{id}">Catch up</button></div>"#)
}

/// Shows "Paused here · Catch up" in a drawn view after a pause here in a group, and hides it otherwise.
pub(super) fn show_paused(root: &Element, id: &str) {
    if let Some(slot) = qs_in(root, &format!("#{id}-slot")) {
        dom::toggle_class(&slot, "hidden", !(grouped() && g::local_paused()));
    }
}

/// Up next in a group: who added a song ("added by Sam").
pub(super) fn added_by(t: &Value) -> String {
    let by = t.g("added_by");
    let name = if by.is_object() { by.s("name") } else { by.as_str().unwrap_or("").to_string() };
    if name.is_empty() { String::new() } else { format!(r#"<div class="small faint ellipsis">added by {}</div>"#, esc(&name)) }
}

/// The buttons over Up next (`next`: how many songs it has): Shuffle (two or more) and Clear of the group's, for those
/// who control it; Clear of this player's outside a group; none when it is empty or another device's queue.
pub(super) fn queue_buttons(linked: bool, next: usize) -> String {
    if linked || next == 0 { return String::new(); }
    let clear = r#"<button class="btn sm ghost" id="f-q-clear" type="button">Clear</button>"#;
    if !grouped() { return clear.to_string(); }
    if !g::can_control() { return String::new(); }
    let shuffle = if next >= 2 { format!(r#"<button class="btn sm ghost" id="f-q-shuffle" type="button">{}Shuffle</button>"#, icon_attr("shuffle", r#"width="14" height="14""#)) } else { String::new() };
    format!("{shuffle}{clear}")
}

/// The line over the group's Up next: who may change it.
pub(super) fn queue_note() -> String {
    let text = if g::can_control() { "Everyone in the group can add songs with Add to queue.".to_string() } else { format!("{}. You can add songs with Add to queue.", g::only_host()) };
    format!(r#"<div class="small muted" style="margin:-4px 0 6px">{}</div>"#, esc(&text))
}

/// Wires the pill and Catch up in a view of the player.
pub(super) fn wire_group(ls: &mut Vec<EventListener>, root: &Element, pill: &str, catch: &str) {
    wire(ls, root, pill, "click", |e| { e.stop_propagation(); crate::group::ui::sheet(); });
    wire(ls, root, catch, "click", |e| { e.stop_propagation(); g::catch_up(); });
}
