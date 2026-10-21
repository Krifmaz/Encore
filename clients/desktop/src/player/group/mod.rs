//! Group listen on this computer (p6 item 8; the shapes in docs/contracts/client-shapes-0.31-0.33.md
//! §2.2, §2.3, §3.6–§3.18, the words in §5.7): several people on the server hear the same queue at
//! the same time, each on their own device. This module holds the group this computer is in (from
//! the server's answers and its event stream), the server's clock, the live group listens for
//! the Your devices panel, every action (start, join by code or from the list, leave, end, the
//! host's settings, the playback commands), and makes the engine follow the group (`sync.rs`).
//!
//! While in a group:
//! - what this computer plays is the group's: a page's Play (an album, a playlist, an artist, a
//!   song) adds the songs from the one asked for to the group's Up next and Shuffle adds them
//!   shuffled ("Added 12 songs to the group's Up next"); only a group that plays nothing and has
//!   nothing in Up next starts with them ([`model::play_starts`], those who control). Add to queue
//!   and Play next go to the group (everyone may), see [`forward_play`];
//! - every pause pauses only this computer and is never sent to the group (the owner on Oct 21:
//!   "it may pause for me but dont stop the other users playback"): the Pause button ("Paused for
//!   you – the group keeps playing"), the media keys and the system's media controls, the output
//!   going away. "Paused here · Catch up"; Play then rejoins the group where it is now. The group's
//!   own pause is "Pause for everyone" (the player's ⋯ and the group panel) for those who control it;
//! - play, next, previous, seek and Up next go to the group when this device may control it, else
//!   the control is off with the reason ("Only Alex controls playback");
//! - repeat and the library shuffle after the queue are off; Shuffle shuffles the group's Up next
//!   once (those who control it), and a page's Shuffle plays the songs in a random order the
//!   server makes, the same for everyone; linking to another device is off ("Leave the group
//!   listen to control another device");
//! - every song plays through the relay (`relay.rs`): small drift is closed with crossfaded
//!   slices of samples (Cadenza has no rate control), a large one with a seek spliced in while the
//!   music plays on; no crossfade between songs (gapless stays).
//!
//! Accounts only: guests join through the web page. A server without group listens (before 0.31,
//! or turned off) answers 404 / 403: every entry hides, with no error shown.

pub mod clock;
pub mod events;
pub mod model;
pub mod relay;
pub mod slice;
pub mod sync;
pub mod view;

pub use model::{GroupState, Summary};

use super::{Message, State};
use crate::api::{Api, ApiError};
use crate::engine::{self, Command, PlayState, Track};
use crate::fmt;
use crate::pages::Ctx;
use iced::{Subscription, Task};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ----- what the engine's play functions ask (any thread) -------------------------------------

/// In a group now (read by `engine::play` and friends).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Plays and queue changes asked for while in a group, sent to the group by the player.
static PENDING: Mutex<Vec<Pend>> = Mutex::new(Vec::new());

#[derive(Clone, Debug)]
enum Pend {
    /// Play this list from `index` at `position`: it becomes the group's queue.
    Play { ids: Vec<Option<i64>>, index: usize, position: f64 },
    /// Add to queue (`next` false) or Play next.
    Queue { ids: Vec<Option<i64>>, next: bool },
    /// A page's Shuffle: the songs in a random order the server makes (the group's queue for
    /// those who control it, else added to its Up next).
    Shuffle { ids: Vec<Option<i64>> },
}

/// Whether this computer is in a group listen.
pub fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

fn set_active(on: bool) {
    ACTIVE.store(on, Ordering::SeqCst);
}

fn ids_of(tracks: &[Track]) -> Vec<Option<i64>> {
    tracks.iter().map(super::queue::lib_id).collect()
}

/// A page's Play while in a group: the list goes to the group (see [`Msg::Flush`]). False when
/// this computer is in no group.
pub fn forward_play(tracks: &[Track], index: usize, position: f64) -> bool {
    if !active() {
        return false;
    }
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(Pend::Play {
        ids: ids_of(tracks),
        index,
        position,
    });
    true
}

/// A page's Shuffle while in a group (see [`Pend::Shuffle`]). False when this computer is in no
/// group.
pub fn forward_shuffle(tracks: &[Track]) -> bool {
    if !active() {
        return false;
    }
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(Pend::Shuffle { ids: ids_of(tracks) });
    true
}

/// Add to queue / Play next while in a group.
pub fn forward_queue(tracks: &[Track], next: bool) -> bool {
    if !active() {
        return false;
    }
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Pend::Queue { ids: ids_of(tracks), next });
    true
}

/// Whether plays wait to be sent to the group.
pub fn has_pending() -> bool {
    !PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

fn take_pending() -> Vec<Pend> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

// ----- clocks ----------------------------------------------------------------------------

/// Milliseconds on this process's monotonic clock (`Instant`), which no clock setting moves.
pub fn mono_ms() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as i64
}

/// The wall clock in Unix ms (stands in for the server's until the offset is measured).
pub fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A server without group listens is asked again after this long.
const NO_GROUPS_ASK_MS: i64 = 600_000;

/// Round trips to the server's clock: (sent, the server's `now_ms`, received) each, local times
/// in [`mono_ms`], and the server process's id. `clock.rs` makes the offset of them.
#[derive(Clone, Debug, Default)]
pub struct Measured {
    pub samples: Vec<clock::Sample>,
    pub boot: String,
}

/// `n` round trips to `GET /api/time` (8 at join, 3 for a refresh, see `clock.rs`).
async fn measure_clock(api: &Api, n: usize) -> Measured {
    let mut samples = Vec::new();
    let mut boot = String::new();
    for _ in 0..n {
        let sent = mono_ms();
        match api.get("/time").await {
            Ok(v) => {
                samples.push((sent, fmt::i(&v, "now_ms"), mono_ms()));
                if boot.is_empty() {
                    boot = fmt::s(&v, "boot");
                }
            }
            // a server without /api/time: its clock is taken as this computer's
            Err(e) if e.status != 0 => break,
            Err(_) => {}
        }
    }
    Measured { samples, boot }
}

// ----- state -----------------------------------------------------------------------------

/// The group listen dialogs.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    /// The group: the code, the invite link, the people, the host's settings, Leave / End.
    Sheet,
    Start,
    /// "Join a group listen": a code or an invite link.
    Join,
    /// "Join <host>'s group listen?" for a code that was looked up.
    Invite { code: String, summary: Option<Box<Summary>> },
    /// "End the group listen?".
    ConfirmEnd,
    /// "Clear the group's Up next?".
    ConfirmClear,
}

/// How a group became this computer's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Start,
    Join,
    /// Back in the group this computer was in when the app last ran: quiet, "Paused here".
    Restore,
}

/// What joining again after the stream answered 404 found.
#[derive(Clone, Debug)]
pub enum Rejoin {
    /// Back in the same group (a new membership).
    In(Value),
    /// The server could not be asked: the stream is tried again, and with it the join.
    Retry,
    /// The group is there but this device can't get back in (full, too many tries, turned off).
    Refused(u16, String),
    /// The group is gone; `restarted`: the server process is another one.
    Gone { restarted: bool },
}

#[derive(Debug, Default)]
pub struct Group {
    /// The group this computer is in.
    pub state: Option<GroupState>,
    /// The server has group listens (None: not asked yet; false: turned off, or before 0.31).
    pub available: Option<bool>,
    /// The live group listens this account may see (Your devices).
    pub live: Vec<Summary>,
    live_for: String,
    none_at: Option<i64>,
    /// This member paused here only ("Paused here · Catch up"); the group plays on.
    pub local_paused: bool,
    pub dialog: Option<Dialog>,
    /// A start or join is on its way.
    pub busy: bool,
    pub join_text: String,
    pub start_link_only: bool,
    pub start_everyone: bool,
    /// The host's "Limit to N people" while it is being changed (sent once the presses stop).
    pub limit: Option<u32>,
    limit_seq: u64,
    /// The server's clock as measured (`clock.rs`), and when it was last measured.
    clock: clock::Clock,
    clock_at: i64,
    boot: String,
    /// The event stream (None outside a group); a new epoch opens a new one.
    pub stream: Option<events::Key>,
    epoch: u64,
    rejoining: bool,
    /// The server and account `GET /api/group/current` was asked for.
    asked_for: String,
    /// When the engine's last status came (its position runs on from there while playing).
    pub status_at: i64,
    pub mem: sync::Mem,
    /// "Only Alex controls playback" (the tooltips of the controls that are off borrow it).
    pub deny: String,
    /// The group song playing here and how much of it has played (the relay's songs are counted
    /// here: the engine counts a live stream's plays only after four minutes).
    plays: Plays,
}

/// A group song's play: its relay id, the seconds heard, the position last seen, reported yet.
#[derive(Debug, Default)]
struct Plays {
    live: String,
    heard: f64,
    last: f64,
    reported: bool,
}

impl Group {
    /// In a group, and this device may not control it.
    pub fn denied(&self) -> bool {
        self.state.as_ref().is_some_and(|s| !s.you_can_control)
    }

    /// The server's clock now (Unix ms).
    pub fn server_now(&self) -> i64 {
        match self.clock.offset {
            Some(o) => mono_ms() + o,
            None => wall_ms(),
        }
    }

    /// Where the group is now: its entry and the position in it.
    pub fn expected(&self) -> Option<(usize, f64)> {
        self.state.as_ref().and_then(|s| model::expected(s, self.server_now()))
    }

    /// The entries show (the player ⋯ menu, Home, Your devices): in a group, or on a server that
    /// has them (or was not asked yet).
    pub fn shown(&self) -> bool {
        self.state.is_some() || self.available != Some(false)
    }
}

#[derive(Clone, Debug)]
pub enum Msg {
    OpenSheet,
    OpenStart,
    OpenJoin,
    Close,
    StartLinkOnly(bool),
    StartEveryone(bool),
    Start,
    JoinText(String),
    JoinSubmit,
    /// `GET /api/group/code/{code}` answered, for "Join <host>'s group listen?".
    Lookup(String, Result<Value, ApiError>),
    JoinCode(String),
    /// Join an "anyone" group from the list (or open it when this computer is in it).
    JoinId(String),
    Adopted(How, Measured, Result<Value, ApiError>),
    Leave,
    AskEnd,
    End,
    Ended(String, Result<Value, ApiError>),
    /// The host's settings: any of everyone_controls, visibility, guests_allowed, max_members.
    Patch(Value),
    /// A state answer (a command, the host's settings, a removal).
    Answer(Result<Value, ApiError>),
    /// The answer to a Play or Shuffle that added `n` songs to the group's Up next (`shuffled`):
    /// said once the group took them ("Added 12 songs to the group's Up next").
    Added(usize, bool, Result<Value, ApiError>),
    /// "Pause for everyone": the group's own pause (those who control it).
    PauseForEveryone,
    LimitDown,
    LimitUp,
    LimitSend(u64),
    /// "Remove from the group": the membership and the name.
    Remove(String, String),
    Removed(String, Result<Value, ApiError>),
    CopyLink,
    CatchUp,
    /// Up next: play the group's entry `i` now; remove it; move it up one place.
    Jump(usize),
    RemoveEntry(usize),
    MoveUp(usize),
    /// Up next's Shuffle: the group's Up next in a random order, once (the server's).
    Shuffle,
    /// Up next's Clear: asked first ("Clear the group's Up next?"), then everything after the
    /// song playing goes.
    AskClear,
    Clear,
    Event(events::Ev),
    Rejoined(Rejoin),
    /// Open the event stream again.
    Reopen,
    List(String, Result<Value, ApiError>),
    Clocked(Measured),
    /// Twice a second while in a group: the engine follows, plays go out.
    Tick,
    /// Send the plays asked for since the last look.
    Flush,
    Nothing,
}

fn g(m: Msg) -> Message {
    Message::Group(m)
}

// ----- requests ----------------------------------------------------------------------------

fn device_id(ctx: &Ctx) -> String {
    crate::link::device_id(&ctx.config.server)
}

/// `{device_id, device_name, …extra}`: every body carries this device.
fn body(ctx: &Ctx, extra: Value) -> Value {
    let mut b = json!({ "device_id": device_id(ctx), "device_name": engine::device_name() });
    if let Value::Object(m) = extra {
        for (k, v) in m {
            b[k] = v;
        }
    }
    b
}

fn path(id: &str, tail: &str) -> String {
    format!("/group/{}{tail}", crate::pages::urlencode(id))
}

/// `GET /api/group` (quiet on any error; a server without group listens is asked again only
/// every 10 minutes).
pub fn refresh_list(gr: &mut Group, ctx: &Ctx) -> Task<Message> {
    if !ctx.session.signed_in() {
        return Task::none();
    }
    let base = ctx.base();
    if base != gr.live_for {
        gr.live_for = base.clone();
        gr.available = None;
        gr.live.clear();
        gr.none_at = None;
    }
    if gr.available == Some(false) && gr.none_at.is_some_and(|t| mono_ms() - t < NO_GROUPS_ASK_MS) {
        return Task::none();
    }
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/group").await }, move |r| g(Msg::List(base.clone(), r)))
}

/// An answer that means "no group listens here" (turned off, or a server before 0.31).
fn none_here(e: &ApiError) -> bool {
    matches!(e.status, 403 | 404 | 405 | 410)
}

fn command(gr: &Group, ctx: &Ctx, op: &str, fields: Value) -> Task<Message> {
    send(gr, ctx, op, fields, Msg::Answer)
}

/// [`command`] with the answer as `done`'s message.
fn send(
    gr: &Group,
    ctx: &Ctx,
    op: &str,
    fields: Value,
    done: impl FnOnce(Result<Value, ApiError>) -> Msg + Send + 'static,
) -> Task<Message> {
    let Some(s) = gr.state.as_ref() else {
        return Task::none();
    };
    let mut extra = json!({ "op": op });
    if let Value::Object(m) = fields {
        for (k, v) in m {
            extra[k] = v;
        }
    }
    let b = body(ctx, extra);
    let p = path(&s.id, "/command");
    let api = ctx.api.clone();
    Task::perform(async move { api.post(&p, b).await }, move |r| g(done(r)))
}

/// The key of the event stream for the group (a new `epoch` opens a new stream).
fn stream_key(gr: &mut Group, ctx: &Ctx) {
    gr.epoch += 1;
    gr.stream = gr.state.as_ref().map(|s| events::Key {
        base: ctx.base(),
        auth: crate::link::auth(ctx.config, &ctx.api),
        group: s.id.clone(),
        device: device_id(ctx),
        epoch: gr.epoch,
    });
}

// ----- in and out ------------------------------------------------------------------------

/// Now in `s`: its event stream opens and the engine follows it.
fn adopt(st: &mut State, s: GroupState, how: How, clock: Measured, ctx: &mut Ctx) -> Task<Message> {
    let gr = &mut st.group;
    let measured = clock::Clock::join(&clock.samples);
    if measured.offset.is_some() {
        gr.clock = measured;
        gr.clock_at = mono_ms();
    }
    gr.boot = if s.boot.is_empty() { clock.boot } else { s.boot.clone() };
    // linking to another device is off in a group: this computer plays the group's music itself
    if st.devices.linked.is_some() {
        st.devices.unlink();
        st.lyrics.clear();
    }
    // no shuffle and no repeat in a group (the group's queue is played as it is)
    engine::set_shuffle(false);
    super::queue::forget_order();
    st.spot = None;
    st.dialog = None;
    let gr = &mut st.group;
    if how == How::Join {
        ctx.toast_ok(if s.you_host {
            "You joined your group listen".to_string()
        } else {
            format!("You joined {}'s group listen", s.host_name)
        });
    }
    gr.deny = model::only_host(&s);
    gr.state = Some(s);
    set_active(true);
    gr.local_paused = how == How::Restore;
    gr.mem.reset();
    gr.limit = None;
    gr.rejoining = false;
    gr.dialog = (how == How::Start).then_some(Dialog::Sheet);
    stream_key(gr, ctx);
    follow(st, ctx);
    Task::none()
}

/// A state of the group this computer is in, from the server, when not older.
fn apply(st: &mut State, s: GroupState, ctx: &mut Ctx) {
    let gr = &mut st.group;
    let Some(old) = gr.state.as_ref() else {
        return;
    };
    // an answer that comes after leaving (or about another group) changes nothing
    if old.id != s.id || s.rev < old.rev {
        return;
    }
    for words in model::comings(Some(old), &s) {
        ctx.toast(words);
    }
    // the host's "Limit to N people" shows the server's number once it has taken one
    let max_changed = old.max_members != s.max_members;
    if !s.boot.is_empty() {
        gr.boot = s.boot.clone();
    }
    gr.deny = model::only_host(&s);
    if max_changed || gr.limit == Some(s.max_members) {
        gr.limit = None;
    }
    gr.state = Some(s);
    follow(st, ctx);
}

/// Out of the group (left, ended, removed): the music stops here, the queue stays; `words` as a
/// toast.
fn stop(st: &mut State, words: Option<String>, ctx: &mut Ctx) -> Task<Message> {
    let gr = &mut st.group;
    if gr.state.take().is_none() {
        return Task::none();
    }
    set_active(false);
    take_pending();
    gr.local_paused = false;
    gr.stream = None;
    gr.rejoining = false;
    gr.limit = None;
    gr.mem.reset();
    gr.plays = Plays::default();
    if matches!(gr.dialog, Some(Dialog::Sheet | Dialog::ConfirmEnd | Dialog::ConfirmClear)) {
        gr.dialog = None;
    }
    if st.status.state == PlayState::Playing {
        engine::send(Command::Pause);
    }
    end_relay();
    engine::set_shuffle(st.shuffle);
    if let Some(w) = words {
        ctx.toast(w);
    }
    refresh_list(&mut st.group, ctx)
}

/// Signing out: out of the group at once, and the request that leaves it on the server (sent
/// before the session ends).
pub fn sign_out(st: &mut State, server: &str) -> Option<(String, Value)> {
    let s = st.group.state.take()?;
    set_active(false);
    take_pending();
    end_relay();
    st.group = Group::default();
    engine::set_shuffle(st.shuffle);
    Some((
        path(&s.id, "/leave"),
        json!({ "device_id": crate::link::device_id(server), "device_name": engine::device_name() }),
    ))
}

// ----- following the group -----------------------------------------------------------------

/// The relay of the group this computer is in: started with the first song it plays here,
/// stopped when it leaves.
static RELAY: Mutex<Option<Arc<relay::Relay>>> = Mutex::new(None);

fn relay_now() -> Option<Arc<relay::Relay>> {
    RELAY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn relay_for(e: &engine::Engine) -> Arc<relay::Relay> {
    RELAY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(|| Arc::new(relay::Relay::start(e.clone())))
        .clone()
}

/// Out of the group: the engine's queue gets its real songs back (paused) and the relay stops.
fn end_relay() {
    let Some(r) = RELAY.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    match engine::engine() {
        Some(e) => r.leave(&e, &e.status()),
        None => r.stop(),
    }
}

/// The engine follows the group's queue, song, position and play / pause.
fn follow(st: &mut State, ctx: &mut Ctx) {
    let Some(s) = st.group.state.as_ref() else {
        return;
    };
    let exp = model::expected(s, st.group.server_now());
    let w = sync::Want {
        ids: s.queue.iter().map(|e| e.id).collect(),
        durs: s.queue.iter().map(|e| e.duration()).collect(),
        qid: exp.map(|(i, _)| s.queue[i].qid.clone()).unwrap_or_default(),
        index: exp.map(|(i, _)| i).unwrap_or(0),
        position: exp.map(|(_, p)| p).unwrap_or(0.0),
        group_playing: s.playing,
        play: s.playing && !st.group.local_paused,
        server_made: !engine::tracks::current_query().is_empty(),
    };
    let status = &st.status;
    let playing = status.state == PlayState::Playing;
    // the position runs on from the last status while playing; a status too old to say where
    // the engine is (it publishes twice a second) measures no drift rather than a guessed one
    let age = (mono_ms() - st.group.status_at).max(0);
    let relay = relay_now();
    let l = sync::Local {
        // only the relay's entries count as the group's: anything else is loaded again
        ids: status
            .queue
            .iter()
            .map(|t| relay::live_of(t).and(super::queue::lib_id(t)))
            .collect(),
        index: status.index,
        state: status.state,
        position: status.position + if playing { age as f64 / 1000.0 } else { 0.0 },
        fresh: !playing || age <= sync::STALE_MS,
        server_made: match status.track.as_ref() {
            Some(t) if relay::live_of(t).is_some() => relay.as_ref().is_some_and(|r| r.server_made()),
            Some(t) => relay::server_made(t),
            None => false,
        },
        busy: relay.as_ref().is_some_and(|r| r.busy()),
    };
    let acts = sync::decide(&w, &l, &mut st.group.mem, mono_ms());
    if acts.is_empty() {
        return;
    }
    let s = st.group.state.as_ref().expect("checked above");
    // corrections with slices go by twice a second: only the rest is worth a line
    if acts.iter().any(|a| !matches!(a, sync::Act::Nudge(_))) {
        log::info!(
            "group listen: following the group (entry {}, {:.1} s, {}): {acts:?}",
            w.index,
            w.position,
            if w.play { "playing" } else { "paused" }
        );
    }
    if let Err(e) = run(&acts, s, &st.status, ctx) {
        ctx.toast_err(e);
    }
}

/// The group's entries as engine queue entries (the relay's streams), with this server's
/// credential; the relay learns the real songs behind them.
fn tracks(s: &GroupState, ctx: &Ctx, r: &relay::Relay, at: impl Iterator<Item = usize>) -> Vec<Track> {
    let auth = crate::link::auth(ctx.config, &ctx.api);
    let base = ctx.base();
    let mut out = Vec::new();
    let mut real = Vec::new();
    for e in at.filter_map(|i| s.queue.get(i)) {
        if let Some(t) = engine::tracks::from_encore(&base, &auth, &e.row) {
            out.push(relay::live_track(&t, &e.qid));
            real.push((relay::live_id(&e.qid), t));
        }
    }
    r.add(real);
    out
}

/// Turn what `sync` decided into engine and relay commands (the engine itself, never
/// `engine::play`, which would send the list to the group).
fn run(acts: &[sync::Act], s: &GroupState, status: &engine::Status, ctx: &Ctx) -> Result<(), String> {
    use sync::Act;
    let e = engine::start()?;
    let r = relay_for(&e);
    for a in acts {
        match a {
            Act::Enqueue(at) => e.send(Command::Enqueue(tracks(s, ctx, &r, at.iter().copied()))),
            Act::Remove(k) => e.send(Command::Remove(*k)),
            Act::Full { from, aim, play } => {
                let list = tracks(s, ctx, &r, *from..s.queue.len());
                if let Some(live) = list.first().and_then(relay::live_of).map(str::to_string) {
                    r.load(live, *aim, relay::Then::Queue { tracks: list, play: *play });
                }
            }
            Act::PlayAt { local, aim, play } => {
                if let Some(live) = status.queue.get(*local).and_then(relay::live_of) {
                    r.load(live.to_string(), *aim, relay::Then::Index { index: *local, play: *play });
                }
            }
            Act::Seek { aim, play } => {
                let live = status.track.as_ref().and_then(relay::live_of).map(str::to_string);
                r.seek(live, status.index.unwrap_or(0), *aim, *play);
            }
            Act::Nudge(d) => r.correct(*d),
            Act::Play => e.send(Command::Play),
            Act::Pause => e.send(Command::Pause),
        }
    }
    Ok(())
}

/// A group song played here: counted like the web player (half the song or four minutes) and
/// reported once. The engine counts a live stream only after four minutes and the device link
/// reports no live stream, so the relay's songs are counted here.
pub fn count_play(st: &mut State, ctx: &Ctx) -> Task<Message> {
    let Some(t) = st.status.track.as_ref() else {
        return Task::none();
    };
    let Some(live) = relay::live_of(t) else {
        return Task::none();
    };
    let pos = st.status.position;
    let p = &mut st.group.plays;
    if p.live != live {
        *p = Plays { live: live.to_string(), heard: 0.0, last: pos, reported: false };
        return Task::none();
    }
    if st.status.state == PlayState::Playing {
        let d = pos - p.last;
        if d > 0.0 && d < 2.0 {
            p.heard += d;
        }
    }
    p.last = pos;
    let need = match t.duration {
        Some(d) if d > 0.0 => (d / 2.0).min(240.0),
        _ => 240.0,
    };
    let Some(id) = engine::tracks::encore_id(t).filter(|_| !p.reported && p.heard >= need) else {
        return Task::none();
    };
    p.reported = true;
    let seconds = p.heard;
    let api = ctx.api.clone();
    Task::perform(
        async move { api.post("/library/plays", json!({ "track_id": id, "seconds": seconds })).await },
        |_| g(Msg::Nothing),
    )
}

/// The engine's status changed: note when (its position runs on from there), and an engine pause
/// this computer did not ask for (another device through the link, the output going away) is
/// "Paused here" while the group plays on.
pub fn on_status(st: &mut State, prev: &engine::Status) {
    st.group.status_at = mono_ms();
    let gr = &mut st.group;
    let Some(s) = gr.state.as_ref() else {
        return;
    };
    let relay_paused = relay_now().is_some_and(|r| mono_ms() - r.paused_at() < 2000);
    if s.playing
        && !gr.local_paused
        && prev.state == PlayState::Playing
        && st.status.state == PlayState::Paused
        && !gr.mem.paused_by_us(mono_ms())
        && !relay_paused
    {
        gr.local_paused = true;
    }
}

// ----- the player's controls in a group --------------------------------------------------------

/// Play / pause in a group: a pause is only here, for everyone who presses it ("Paused for you –
/// the group keeps playing"); Play after it rejoins the group where it is now (Catch up); a paused
/// group plays for those who control it.
pub fn toggle(st: &mut State, ctx: &mut Ctx) -> Task<Message> {
    let Some(s) = st.group.state.as_ref() else {
        return Task::none();
    };
    let playing = s.playing;
    if st.group.local_paused {
        let t = update(st, Msg::CatchUp, ctx);
        if playing {
            return t;
        }
    } else if playing {
        // only here: the group plays on
        st.group.local_paused = true;
        st.group.mem.own_pause_at = Some(mono_ms());
        st.group.mem.lag.cancel();
        engine::send(Command::Pause);
        ctx.toast(model::PAUSED_FOR_YOU);
        return Task::none();
    }
    if st.group.state.as_ref().is_some_and(|s| s.you_can_control) {
        return command(&st.group, ctx, "play", json!({}));
    }
    ctx.toast(st.group.deny.clone());
    Task::none()
}

/// Next / Previous.
pub fn skip(st: &mut State, next: bool, ctx: &mut Ctx) -> Task<Message> {
    if st.group.denied() {
        ctx.toast(st.group.deny.clone());
        return Task::none();
    }
    command(&st.group, ctx, if next { "next" } else { "prev" }, json!({}))
}

/// A seek to `position` (seconds).
pub fn seek(st: &mut State, position: f64, ctx: &mut Ctx) -> Task<Message> {
    if st.group.denied() {
        ctx.toast(st.group.deny.clone());
        return Task::none();
    }
    command(&st.group, ctx, "seek", json!({ "position": position.max(0.0) }))
}

/// Repeat is off in a group.
pub fn no_repeat(ctx: &mut Ctx) {
    ctx.toast(view::REPEAT_OFF);
}

/// Linking to another device (and Play here, Send my music there) is off in a group.
pub fn no_linking(ctx: &mut Ctx) {
    ctx.toast("Leave the group listen to control another device");
}

/// The player's one-second tick: get back into this device's group after a start of the app,
/// out of it after signing out, and the list while Your devices is open.
pub fn on_tick(st: &mut State, ticks: u64, ctx: &mut Ctx) -> Task<Message> {
    let mut tasks = Vec::new();
    if !ctx.session.signed_in() {
        if st.group.state.is_some() {
            tasks.push(stop(st, None, ctx));
        }
        st.group.asked_for.clear();
        return Task::batch(tasks);
    }
    let me = format!("{}|{}", ctx.base(), ctx.session.user_id());
    if st.group.asked_for != me {
        st.group.asked_for = me;
        let api = ctx.api.clone();
        let p = format!("/group/current?device_id={}", crate::pages::urlencode(&device_id(ctx)));
        tasks.push(Task::perform(
            async move {
                let r = api.get(&p).await;
                let clock = match &r {
                    Ok(v) if v.is_object() => measure_clock(&api, clock::JOIN_SAMPLES).await,
                    _ => Measured::default(),
                };
                (clock, r)
            },
            |(c, r)| g(Msg::Adopted(How::Restore, c, r)),
        ));
    }
    if st.dialog == Some(super::Dialog::Devices) && ticks.is_multiple_of(20) {
        tasks.push(refresh_list(&mut st.group, ctx));
    }
    Task::batch(tasks)
}

// ----- update ------------------------------------------------------------------------------

pub fn update(st: &mut State, msg: Msg, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Msg::OpenSheet => {
            if st.group.state.is_some() {
                st.menu = None;
                st.dialog = None;
                st.group.dialog = Some(Dialog::Sheet);
            }
        }
        Msg::OpenStart => {
            st.menu = None;
            st.dialog = None;
            st.group.start_link_only = false;
            st.group.start_everyone = false;
            st.group.dialog = Some(Dialog::Start);
        }
        Msg::OpenJoin => {
            st.menu = None;
            st.dialog = None;
            st.group.join_text.clear();
            st.group.dialog = Some(Dialog::Join);
        }
        Msg::Close => st.group.dialog = None,
        Msg::StartLinkOnly(on) => st.group.start_link_only = on,
        Msg::StartEveryone(on) => st.group.start_everyone = on,
        Msg::Start => {
            if st.group.busy {
                return Task::none();
            }
            // what plays here becomes the group's queue (the linked device's, when linked)
            let (ids, index, position, playing) = match st.devices.linked.as_ref() {
                Some(l) => (
                    fmt::items(&l.state["queue"]).iter().map(engine::tracks::id_of).collect::<Vec<_>>(),
                    l.state["index"].as_u64().unwrap_or(0) as usize,
                    fmt::f(&l.state, "position"),
                    fmt::truthy(&l.state, "playing"),
                ),
                None => (
                    st.status.queue.iter().map(super::queue::lib_id).collect(),
                    st.status.index.unwrap_or(0),
                    st.status.position,
                    st.status.state == PlayState::Playing,
                ),
            };
            let (win, at) = model::window(&ids, index, 1000);
            let empty = win.is_empty();
            let b = body(
                ctx,
                json!({
                    "visibility": if st.group.start_link_only { "link" } else { "anyone" },
                    "everyone_controls": st.group.start_everyone,
                    "tracks": win,
                    "index": at,
                    "position": if empty { 0.0 } else { position.max(0.0) },
                    "playing": playing && !empty,
                }),
            );
            st.group.busy = true;
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    let c = measure_clock(&api, clock::JOIN_SAMPLES).await;
                    (c, api.post("/group", b).await)
                },
                |(c, r)| g(Msg::Adopted(How::Start, c, r)),
            );
        }
        Msg::JoinText(t) => st.group.join_text = t.chars().take(300).collect(),
        Msg::JoinSubmit => {
            let text = st.group.join_text.clone();
            let Some(code) = model::code_from(&text) else {
                ctx.toast_err("That isn't a group listen code or link");
                return Task::none();
            };
            if let Some(server) = model::server_of(&text) {
                if !model::same_server(&server, &ctx.base()) {
                    ctx.toast_err(format!("That group listen is on another server ({})", model::host_of(&server)));
                    return Task::none();
                }
                // a whole link: who hosts it is asked first ("Join <host>'s group listen?")
                if st.group.state.as_ref().is_some_and(|s| s.code == code) {
                    st.group.dialog = Some(Dialog::Sheet);
                    return Task::none();
                }
                let api = ctx.api.clone();
                let p = format!("/group/code/{}", crate::pages::urlencode(&code));
                return Task::perform(async move { api.get(&p).await }, move |r| g(Msg::Lookup(code.clone(), r)));
            }
            return update(st, Msg::JoinCode(code), ctx);
        }
        Msg::Lookup(code, r) => match r {
            Ok(v) => {
                st.group.dialog = Some(Dialog::Invite {
                    code,
                    summary: Summary::from(&v).map(Box::new),
                })
            }
            Err(e) if matches!(e.status, 403 | 404 | 429) => ctx.toast_err(e.message),
            // the lookup failed otherwise: ask without the host's name, the join says the rest
            Err(_) => st.group.dialog = Some(Dialog::Invite { code, summary: None }),
        },
        Msg::JoinCode(code) => {
            if st.group.busy {
                return Task::none();
            }
            if st.group.state.as_ref().is_some_and(|s| s.code == code) {
                st.group.dialog = Some(Dialog::Sheet);
                return Task::none();
            }
            return join(st, ctx, "/group/join".into(), json!({ "code": code }));
        }
        Msg::JoinId(id) => {
            if st.group.state.as_ref().is_some_and(|s| s.id == id) {
                st.dialog = None;
                st.group.dialog = Some(Dialog::Sheet);
                return Task::none();
            }
            if st.group.busy {
                return Task::none();
            }
            return join(st, ctx, path(&id, "/join"), json!({}));
        }
        Msg::Adopted(how, clock, r) => {
            if how != How::Restore {
                st.group.busy = false;
            }
            match r {
                Ok(v) => match GroupState::from(&v) {
                    Some(s) => {
                        st.group.available = Some(true);
                        // a restore that comes after a start or join of its own is old news
                        if how == How::Restore && st.group.state.is_some() {
                            return Task::none();
                        }
                        return adopt(st, s, how, clock, ctx);
                    }
                    // 204 on /current: this device is in no group
                    None if how == How::Restore => {}
                    None => ctx.toast_err(if how == How::Start { "Couldn't start the group listen" } else { "Couldn't join" }),
                },
                Err(e) => match how {
                    How::Restore => {
                        if none_here(&e) {
                            st.group.available = Some(false);
                            st.group.none_at = Some(mono_ms());
                        }
                    }
                    _ => ctx.toast_err(e.message),
                },
            }
        }
        Msg::Leave => {
            let Some(s) = st.group.state.as_ref() else {
                return Task::none();
            };
            let p = path(&s.id, "/leave");
            let b = body(ctx, json!({}));
            let api = ctx.api.clone();
            let out = stop(st, Some("You left the group listen".into()), ctx);
            return Task::batch([out, Task::perform(async move { api.post(&p, b).await }, |_| g(Msg::Nothing))]);
        }
        Msg::AskEnd => st.group.dialog = Some(Dialog::ConfirmEnd),
        Msg::End => {
            st.group.dialog = None;
            let Some(s) = st.group.state.as_ref() else {
                return Task::none();
            };
            let id = s.id.clone();
            let p = path(&id, "/end");
            let b = body(ctx, json!({}));
            let api = ctx.api.clone();
            return Task::perform(async move { api.post(&p, b).await }, move |r| g(Msg::Ended(id.clone(), r)));
        }
        Msg::Ended(id, r) => match r {
            // the event stream's `ended` may have come first and said it already
            Ok(_) => {
                if st.group.state.as_ref().is_some_and(|s| s.id == id) {
                    return stop(st, Some("You ended the group listen".into()), ctx);
                }
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Msg::Patch(fields) => {
            let Some(s) = st.group.state.as_ref() else {
                return Task::none();
            };
            let p = path(&s.id, "");
            let b = body(ctx, fields);
            let api = ctx.api.clone();
            return Task::perform(async move { api.patch(&p, b).await }, |r| g(Msg::Answer(r)));
        }
        Msg::Answer(r) => match r {
            Ok(v) => {
                if let Some(s) = GroupState::from(&v) {
                    apply(st, s, ctx);
                }
            }
            Err(e) => {
                st.group.limit = None;
                ctx.toast_err(e.message);
                follow(st, ctx);
            }
        },
        Msg::Added(n, shuffled, r) => {
            let took = r.is_ok();
            let t = update(st, Msg::Answer(r), ctx);
            if took {
                ctx.toast_ok(model::added_text(n, shuffled));
            }
            return t;
        }
        Msg::PauseForEveryone => {
            let Some(s) = st.group.state.as_ref() else {
                return Task::none();
            };
            if !s.you_can_control {
                ctx.toast(st.group.deny.clone());
                return Task::none();
            }
            // the group pauses: nothing is paused here only any more
            st.group.local_paused = false;
            st.group.mem.own_pause_at = Some(mono_ms());
            st.group.mem.lag.cancel();
            engine::send(Command::Pause);
            return command(&st.group, ctx, "pause", json!({}));
        }
        Msg::LimitDown | Msg::LimitUp => {
            let Some(s) = st.group.state.as_ref() else {
                return Task::none();
            };
            let floor = (s.count() as u32).max(2);
            let cur = st.group.limit.unwrap_or(s.max_members);
            let next = if matches!(msg, Msg::LimitUp) { (cur + 1).min(s.server_max) } else { cur.saturating_sub(1).max(floor) };
            st.group.limit = Some(next);
            st.group.limit_seq += 1;
            let seq = st.group.limit_seq;
            // sent once the presses stop
            return Task::perform(tokio::time::sleep(Duration::from_millis(700)), move |_| g(Msg::LimitSend(seq)));
        }
        Msg::LimitSend(seq) => {
            if seq != st.group.limit_seq {
                return Task::none();
            }
            let Some((n, s)) = st.group.limit.zip(st.group.state.as_ref()) else {
                return Task::none();
            };
            if n == s.max_members {
                st.group.limit = None;
                return Task::none();
            }
            return update(st, Msg::Patch(json!({ "max_members": n })), ctx);
        }
        Msg::Remove(member, name) => {
            let Some(s) = st.group.state.as_ref() else {
                return Task::none();
            };
            let p = format!(
                "{}?device_id={}",
                path(&s.id, &format!("/members/{}", crate::pages::urlencode(&member))),
                crate::pages::urlencode(&device_id(ctx))
            );
            let api = ctx.api.clone();
            return Task::perform(async move { api.del(&p).await }, move |r| g(Msg::Removed(name.clone(), r)));
        }
        Msg::Removed(name, r) => match r {
            Ok(v) => {
                if let Some(s) = GroupState::from(&v) {
                    apply(st, s, ctx);
                }
            }
            Err(e) => ctx.toast_err(if e.message.is_empty() { format!("Couldn't remove {name}") } else { e.message }),
        },
        Msg::CopyLink => {
            if let Some(s) = st.group.state.as_ref() {
                ctx.copy(model::invite_link(s, &ctx.base()));
                ctx.toast_ok("Invite link copied");
            }
        }
        Msg::CatchUp => {
            st.group.local_paused = false;
            st.group.mem.quiet_until = 0;
            follow(st, ctx);
        }
        Msg::Jump(i) | Msg::RemoveEntry(i) | Msg::MoveUp(i) => {
            if st.group.denied() {
                ctx.toast(st.group.deny.clone());
                return Task::none();
            }
            let Some(qid) = st.group.state.as_ref().and_then(|s| s.queue.get(i)).map(|e| e.qid.clone()) else {
                return Task::none();
            };
            return match msg {
                Msg::Jump(_) => command(&st.group, ctx, "jump", json!({ "index": i, "qid": qid })),
                Msg::RemoveEntry(_) => command(&st.group, ctx, "remove", json!({ "index": i, "qid": qid })),
                _ if i > 0 => command(&st.group, ctx, "move", json!({ "qid": qid, "from": i, "to": i - 1 })),
                _ => Task::none(),
            };
        }
        Msg::Shuffle => {
            if st.group.denied() {
                ctx.toast(st.group.deny.clone());
                return Task::none();
            }
            return command(&st.group, ctx, "shuffle", json!({}));
        }
        Msg::AskClear => {
            if st.group.denied() {
                ctx.toast(st.group.deny.clone());
                return Task::none();
            }
            st.group.dialog = Some(Dialog::ConfirmClear);
        }
        Msg::Clear => {
            st.group.dialog = None;
            if st.group.denied() {
                return Task::none();
            }
            return command(&st.group, ctx, "clear", json!({}));
        }
        Msg::Event(ev) => return on_event(st, ev, ctx),
        Msg::Rejoined(r) => {
            st.group.rejoining = false;
            let host = st.group.state.as_ref().map(|s| s.host_name.clone()).unwrap_or_default();
            match r {
                Rejoin::In(v) => {
                    if let Some(s) = GroupState::from(&v) {
                        if let Some(o) = st.group.state.as_mut().filter(|o| o.id == s.id) {
                            // taken whatever its rev; the new membership is not announced as a
                            // leave (model::comings)
                            o.rev = o.rev.min(s.rev);
                            apply(st, s, ctx);
                            stream_key(&mut st.group, ctx);
                        }
                    }
                }
                Rejoin::Retry => {
                    return Task::perform(tokio::time::sleep(Duration::from_secs(3)), |_| g(Msg::Reopen));
                }
                Rejoin::Refused(status, detail) => return stop(st, Some(model::rejoin_refused(status, &detail)), ctx),
                Rejoin::Gone { restarted } => {
                    return stop(st, Some(model::ended_text(if restarted { "restart" } else { "" }, &host)), ctx)
                }
            }
        }
        Msg::Reopen => {
            if st.group.state.is_some() {
                stream_key(&mut st.group, ctx);
            }
        }
        Msg::List(base, r) => {
            if base != ctx.base() {
                return Task::none();
            }
            match r {
                Ok(v) => {
                    st.group.live = Summary::list(&v);
                    st.group.available = Some(true);
                    st.group.none_at = None;
                }
                Err(e) if none_here(&e) => {
                    st.group.available = Some(false);
                    st.group.live.clear();
                    st.group.none_at = Some(mono_ms());
                }
                Err(_) => {}
            }
        }
        Msg::Clocked(m) => st.group.clock.refresh(&m.samples),
        Msg::Tick => {
            if st.group.state.is_none() {
                return Task::none();
            }
            let mut tasks = vec![update(st, Msg::Flush, ctx)];
            follow(st, ctx);
            // the clock is measured again every minute while in a group (and blended in)
            if mono_ms() - st.group.clock_at >= clock::REFRESH_EVERY_MS {
                st.group.clock_at = mono_ms();
                let api = ctx.api.clone();
                tasks.push(Task::perform(
                    async move { measure_clock(&api, clock::REFRESH_SAMPLES).await },
                    |c| g(Msg::Clocked(c)),
                ));
            }
            return Task::batch(tasks);
        }
        Msg::Flush => {
            let mut tasks = Vec::new();
            for p in take_pending() {
                if st.group.state.is_none() {
                    break;
                }
                // a group with nothing to play starts with the songs (those who control it); else they
                // go to its Up next
                let starts = st.group.state.as_ref().is_some_and(|s| model::play_starts(s, st.group.server_now()))
                    && !st.group.denied();
                match p {
                    Pend::Play { ids, index, position } => {
                        let (win, at) = model::window(&ids, index, 1000);
                        if win.is_empty() {
                            ctx.toast_err("Only songs from your library can be played in a group listen");
                            continue;
                        }
                        if !starts {
                            let add = model::to_add(&ids, index, 1000);
                            let n = add.len();
                            if n == 0 {
                                ctx.toast_err("Only songs from your library can be played in a group listen");
                                continue;
                            }
                            tasks.push(send(&st.group, ctx, "enqueue", json!({ "tracks": add }), move |r| Msg::Added(n, false, r)));
                            continue;
                        }
                        // the position belongs to the song asked for, when it is a library song
                        let position = if ids.get(index).is_some_and(Option::is_some) { position } else { 0.0 };
                        tasks.push(command(&st.group, ctx, "set_queue", json!({ "tracks": win, "index": at, "position": position })));
                    }
                    Pend::Queue { ids, next } => {
                        let lib: Vec<i64> = ids.into_iter().flatten().collect();
                        if lib.is_empty() {
                            ctx.toast_err("Only songs from your library can be played in a group listen");
                            continue;
                        }
                        tasks.push(command(&st.group, ctx, if next { "play_next" } else { "enqueue" }, json!({ "tracks": lib })));
                    }
                    Pend::Shuffle { ids } => {
                        // shuffled here too, and a long list sends 1000 songs picked from all of it
                        let win = model::pick_shuffled(&ids, 1000, &mut super::queue::Rng::new());
                        if win.is_empty() {
                            ctx.toast_err("Only songs from your library can be played in a group listen");
                            continue;
                        }
                        // the server shuffles, the same order for everyone: added to the group's Up
                        // next (said once it took them), or the queue of a group with nothing to play
                        if !starts {
                            let n = win.len();
                            tasks.push(send(&st.group, ctx, "enqueue", json!({ "tracks": win, "shuffle": true }), move |r| Msg::Added(n, true, r)));
                        } else {
                            tasks.push(command(
                                &st.group,
                                ctx,
                                "set_queue",
                                json!({ "tracks": win, "index": 0, "position": 0.0, "shuffle": true }),
                            ));
                        }
                    }
                }
            }
            return Task::batch(tasks);
        }
        Msg::Nothing => {}
    }
    Task::none()
}

/// Join with `path` (`/group/join` with the code, or `/group/{id}/join`): out of the group this
/// computer is in first.
fn join(st: &mut State, ctx: &mut Ctx, p: String, extra: Value) -> Task<Message> {
    let mut tasks = Vec::new();
    if let Some(old) = st.group.state.as_ref() {
        let lp = path(&old.id, "/leave");
        let lb = body(ctx, json!({}));
        let api = ctx.api.clone();
        tasks.push(Task::perform(async move { api.post(&lp, lb).await }, |_| g(Msg::Nothing)));
        tasks.push(stop(st, None, ctx));
    }
    st.group.busy = true;
    let b = body(ctx, extra);
    let api = ctx.api.clone();
    tasks.push(Task::perform(
        async move {
            let c = measure_clock(&api, clock::JOIN_SAMPLES).await;
            (c, api.post(&p, b).await)
        },
        |(c, r)| g(Msg::Adopted(How::Join, c, r)),
    ));
    Task::batch(tasks)
}

fn on_event(st: &mut State, ev: events::Ev, ctx: &mut Ctx) -> Task<Message> {
    let Some(cur) = st.group.state.as_ref() else {
        return Task::none();
    };
    match ev {
        events::Ev::State(v) => {
            if let Some(s) = GroupState::from(&v) {
                apply(st, s, ctx);
            }
            Task::none()
        }
        events::Ev::Ended(reason) => {
            let words = if reason == "ended" && cur.you_host {
                "You ended the group listen".to_string()
            } else {
                model::ended_text(&reason, &cur.host_name)
            };
            stop(st, Some(words), ctx)
        }
        events::Ev::Gone(403, detail) => {
            let words = if detail.is_empty() { model::ended_text("off", "") } else { detail };
            stop(st, Some(words), ctx)
        }
        // the credential changed under the stream (a new app token): open it again
        events::Ev::Gone(401, _) => Task::perform(tokio::time::sleep(Duration::from_secs(3)), |_| g(Msg::Reopen)),
        events::Ev::Gone(..) => {
            // dropped (2 minutes without the stream) or the group is gone: back in with the code
            if st.group.rejoining {
                return Task::none();
            }
            let (id, code, boot) = (cur.id.clone(), cur.code.clone(), st.group.boot.clone());
            st.group.rejoining = true;
            let b = body(ctx, json!({ "code": code }));
            let leave = body(ctx, json!({}));
            let api = ctx.api.clone();
            Task::perform(
                async move {
                    let gone = |api: Arc<Api>, boot: String| async move {
                        let now = api.get("/time").await.map(|v| fmt::s(&v, "boot")).unwrap_or_default();
                        Rejoin::Gone {
                            restarted: !now.is_empty() && !boot.is_empty() && now != boot,
                        }
                    };
                    match api.post("/group/join", b).await {
                        Ok(v) if fmt::s(&v, "id") == id => Rejoin::In(v),
                        Ok(v) => {
                            // the code names another group now (this one ended and the code was
                            // given out again): not that one
                            let _ = api.post(&path(&fmt::s(&v, "id"), "/leave"), leave).await;
                            gone(api, boot).await
                        }
                        Err(e) if e.status == 404 => gone(api, boot).await,
                        Err(e) if e.status == 0 || e.status >= 500 => Rejoin::Retry,
                        Err(e) => Rejoin::Refused(e.status, e.message),
                    }
                },
                |r| g(Msg::Rejoined(r)),
            )
        }
    }
}

/// The event stream and the twice-a-second look while in a group.
pub fn subscription(gr: &Group) -> Subscription<Message> {
    let Some(key) = gr.stream.clone() else {
        return Subscription::none();
    };
    Subscription::batch([
        events::subscription(key).map(|e| g(Msg::Event(e))),
        iced::time::every(Duration::from_millis(500)).map(|_| g(Msg::Tick)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64) -> Track {
        Track {
            id: Some(id.to_string()),
            source: cadenza::decode::Source::Url {
                url: format!("http://s/api/library/tracks/{id}/stream"),
                headers: vec![],
            },
            ..Default::default()
        }
    }

    #[test]
    fn plays_go_to_the_group_only_while_in_one() {
        set_active(false);
        take_pending();
        assert!(!forward_play(&[track(1)], 0, 0.0));
        assert!(!forward_queue(&[track(1)], true));
        set_active(true);
        assert!(forward_play(&[Track::default(), track(4), track(5)], 2, 12.0));
        assert!(forward_queue(&[track(7)], true));
        assert!(has_pending());
        let p = take_pending();
        assert!(matches!(&p[0], Pend::Play { ids, index: 2, .. } if ids == &vec![None, Some(4), Some(5)]));
        assert!(matches!(&p[1], Pend::Queue { ids, next: true } if ids == &vec![Some(7)]));
        assert!(!has_pending());
        set_active(false);
    }

    #[test]
    fn a_stream_the_server_makes_is_told_by_its_query() {
        let mut t = track(3);
        assert!(!relay::server_made(&t));
        t.source = cadenza::decode::Source::Url {
            url: "http://s/api/library/tracks/3/stream?q=aac&kbps=96".into(),
            headers: vec![],
        };
        assert!(relay::server_made(&t));
    }
}
