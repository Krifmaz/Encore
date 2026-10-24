//! The player: the engine's status as the app sees it, the bar along the bottom (`bar.rs`),
//! the full view with Now playing / Lyrics / Up next (`full.rs`), the queue's order, repeat
//! and the saved spot (`queue.rs`), lyrics (`lyrics.rs`), the Signal path dialog (`signal.rs`)
//! and Your devices with the linked mode (`devices.rs`), and the group listen (`group/`, p6 item 8).
//!
//! `Snapshot`, `State`, `Message::Engine`, `update`, `bar`, `overlay` and `subscription` keep
//! their shapes: the app and the pages call them. Pages reach the player through
//! `ctx.player_msg(player::Message::…)`: `OpenFull(Pane::Queue)`, `Customize(true)`,
//! `LikeTrack(id)`, `RefreshLikes`, `AddToPlaylist(ids)`, `Devices`.

pub mod bar;
pub mod devices;
pub mod full;
pub mod group;
pub mod lyrics;
pub mod queue;
pub mod signal;

pub use queue::Repeat;

use crate::api::ApiError;
use crate::config::{BAR_ACTIONS, BAR_DEFAULT};
use crate::engine::{self, Command, Event, PlayState, Status, Track};
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use iced::widget::image;
use iced::{Point, Subscription, Task};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// What pages see of the player (cheap to clone: one `Arc`).
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub status: Arc<Status>,
    /// The group listen this computer is in (its id), if any.
    pub group_id: Option<String>,
}

impl Snapshot {
    pub fn track(&self) -> Option<&Track> {
        self.status.track.as_ref()
    }

    /// The Encore id of the playing song (None for files).
    pub fn current_id(&self) -> Option<i64> {
        self.track().and_then(engine::tracks::encore_id)
    }
}

/// The tabs of the full view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Now,
    Lyrics,
    Queue,
}

impl Pane {
    pub fn id(self) -> &'static str {
        match self {
            Pane::Now => "now",
            Pane::Lyrics => "lyrics",
            Pane::Queue => "queue",
        }
    }

    pub fn by_id(id: &str) -> Pane {
        match id {
            "lyrics" => Pane::Lyrics,
            "queue" => Pane::Queue,
            _ => Pane::Now,
        }
    }
}

/// A dialog laid over the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    Signal,
    Devices,
    AddToPlaylist,
}

/// "Continue <song> from 2:31": a spot another device (or this computer) left.
#[derive(Clone, Debug)]
pub struct SpotOffer {
    pub spot: Value,
    /// From the server (another device) rather than this computer's `resume.json`.
    pub remote: bool,
}

/// The song in front of you: this computer's, or the linked device's.
#[derive(Clone, Debug, Default)]
pub struct Now {
    /// The library id (None for files and the test sound).
    pub id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_id: Option<String>,
    /// A cover URL or server path.
    pub cover: Option<String>,
    pub format: Option<String>,
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f64,
    /// The linked device's song, not this computer's.
    pub remote: bool,
}

#[derive(Debug)]
pub struct State {
    pub status: Arc<Status>,
    /// The seek slider while it is held.
    pub seek_drag: Option<f64>,
    /// The volume slider while it is held.
    pub vol_drag: Option<f64>,
    /// The Customize the bar dialog is open.
    pub customize: bool,
    /// The full view and its tab, when open.
    pub full: Option<Pane>,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// "Then shuffle library", once changed here (else the account's playback prefs say).
    pub autoplay: Option<bool>,
    /// Liked song ids, fetched the first time a heart is drawn.
    pub liked: Option<HashSet<i64>>,
    likes_loading: bool,
    pending_like: Option<i64>,
    pub lyrics: lyrics::Lyrics,
    /// Lyrics mode's glide to the current line: (from, to, started).
    lyrics_glide: Option<(f32, f32, std::time::Instant)>,
    /// The ⋯ menu, open at this point.
    pub menu: Option<Point>,
    /// Who added the song the ⋯ menu was opened for (0.36, `/library/tracks/{id}/added`): (song id, words), the
    /// menu's first line.
    pub menu_added: Option<(i64, String)>,
    pub dialog: Option<Dialog>,
    pub signal: signal::Info,
    pub devices: devices::Devices,
    /// The group listen this computer is in, its dialogs and the server's list.
    pub group: group::Group,
    pub spot: Option<SpotOffer>,
    spot_checked: bool,
    spot_dismissed: Option<String>,
    /// `/api/playback/dsp` for the signed-in user (whether the server may upsample their
    /// streams), and the user it was fetched for.
    dsp_info: Value,
    dsp_for: Option<i64>,
    /// The library's song dialogs (Report a problem) and Download this song, shared with the
    /// library pages.
    pub shared: crate::pages::library::Shared,
    /// Add to playlist: the playlists and the new name.
    pub playlists: Option<Vec<Value>>,
    pub new_playlist: String,
    pub add_ids: Vec<i64>,
    /// The user pressed Next / Previous / a queue row: the next track start is theirs.
    user_skip: bool,
    /// The user stopped playback: a Stopped status is not the end of the queue.
    user_stop: bool,
    /// A library shuffle is being fetched to keep the music going.
    topping: bool,
    last_save: f64,
    last_error: Option<String>,
    ticks: u64,
    /// Up next's Clear: what went, for Undo.
    cleared: Option<Cleared>,
}

/// The songs Up next's Clear took out (Undo puts them back after the song playing).
#[derive(Debug)]
struct Cleared {
    tracks: Vec<Track>,
    at: std::time::Instant,
}

/// How long Up next's Clear can be undone.
pub const UNDO_MS: u64 = 6000;

/// The toast after Up next's Clear (with Undo).
pub const UP_NEXT_CLEARED: &str = "Up next cleared";

impl Default for State {
    fn default() -> Self {
        let c = crate::config::Config::load();
        let saved = devices::saved_link().map(|(id, name)| devices::Linked {
            id,
            name,
            state: json!({}),
            online: true,
            missed: 0,
        });
        engine::set_shuffle(c.shuffle);
        let mut d = devices::Devices::default();
        if let Some(l) = saved {
            d.link_to(&json!({ "id": l.id, "name": l.name }));
        }
        Self {
            status: Arc::new(Status::default()),
            seek_drag: None,
            vol_drag: None,
            customize: false,
            full: None,
            repeat: Repeat::parse(&c.repeat),
            shuffle: c.shuffle,
            autoplay: None,
            liked: None,
            likes_loading: false,
            pending_like: None,
            lyrics: lyrics::Lyrics::default(),
            lyrics_glide: None,
            menu: None,
            menu_added: None,
            dialog: None,
            signal: signal::Info::default(),
            devices: d,
            group: group::Group::default(),
            spot: None,
            spot_checked: false,
            spot_dismissed: None,
            dsp_info: Value::Null,
            dsp_for: None,
            shared: Default::default(),
            playlists: None,
            new_playlist: String::new(),
            add_ids: Vec::new(),
            user_skip: false,
            user_stop: false,
            topping: false,
            last_save: 0.0,
            last_error: None,
            ticks: 0,
            cleared: None,
        }
    }
}

impl State {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            status: Arc::clone(&self.status),
            group_id: self.group.state.as_ref().map(|s| s.id.clone()),
        }
    }

    /// The song in front of you (the linked device's when linked).
    pub fn now(&self) -> Option<Now> {
        if let Some(l) = self.devices.linked.as_ref() {
            let s = &l.state;
            if !fmt::truthy(s, "title") {
                return None;
            }
            return Some(Now {
                id: s["id"].as_i64(),
                title: fmt::s(s, "title"),
                artist: fmt::s(s, "artist"),
                album: fmt::s(s, "album"),
                album_id: s["album_id"].as_str().filter(|a| !a.is_empty()).map(str::to_string),
                cover: s["cover"].as_str().filter(|c| !c.is_empty()).map(str::to_string),
                format: s["format"].as_str().map(str::to_string),
                playing: fmt::truthy(s, "playing"),
                position: fmt::f(s, "position"),
                duration: fmt::f(s, "duration"),
                volume: s["volume"].as_f64().unwrap_or(0.9),
                remote: true,
            });
        }
        let st = &self.status;
        let t = st.track.as_ref()?;
        Some(Now {
            id: engine::tracks::encore_id(t),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            album_id: t.album_id.clone(),
            cover: t.cover.clone(),
            format: t.format.clone(),
            playing: st.state == PlayState::Playing,
            position: st.position,
            duration: st.duration.or(t.duration).unwrap_or(0.0),
            volume: engine::volume_level(),
            remote: false,
        })
    }

    /// Whether `id` is in Liked Songs (false while the list has not arrived).
    pub fn is_liked(&self, id: Option<i64>) -> bool {
        match (id, self.liked.as_ref()) {
            (Some(id), Some(set)) => set.contains(&id),
            _ => false,
        }
    }

    /// Shuffle and repeat as the bar shows them: the linked device's when linked.
    pub fn modes(&self) -> (bool, Repeat) {
        match self.devices.linked.as_ref() {
            Some(l) => (fmt::truthy(&l.state, "shuffle"), Repeat::parse(&fmt::s(&l.state, "repeat"))),
            None => (self.shuffle, self.repeat),
        }
    }

    /// "Then shuffle library".
    pub fn autoplay(&self, ctx: &Ctx) -> bool {
        self.autoplay.unwrap_or_else(|| playback_prefs(ctx)["autoplay"] != Value::Bool(false))
    }
}

/// The account's playback preferences (`user.prefs.playback`), `{}` when there are none.
pub fn playback_prefs(ctx: &Ctx) -> Value {
    let p = &ctx.session.user["prefs"]["playback"];
    if p.is_object() {
        p.clone()
    } else {
        json!({})
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Toggle,
    Next,
    Prev,
    SeekDrag(f64),
    SeekDone,
    Volume(f64),
    VolumeDone,
    /// From the engine (status, track started, played enough).
    Engine(Box<Event>),
    /// Once a second.
    Tick,
    /// Escape: close the menu, the dialog or the full view.
    Escape,
    /// Space, when no text field has the key: play / pause (the web's in-app key).
    KeyToggle,
    /// Left / Right, when no text field has the key: seek this many seconds.
    SeekBy(f64),
    /// Open or close the Customize the bar dialog.
    Customize(bool),
    /// Add or remove a bar button (an id from `BAR_ACTIONS`).
    ToggleBarButton(String),
    ResetBar,
    OpenFull(Pane),
    CloseFull,
    SetPane(Pane),
    /// The X on the bar: stop and clear the queue.
    ClosePlayer,
    Menu,
    CloseMenu,
    CloseDialog,
    SetShuffle(bool),
    SetRepeat(Repeat),
    Autoplay(bool),
    PrefsSaved(Result<Value, ApiError>),
    /// `/api/playback/dsp` came back (for the stream query).
    DspInfo(i64, Result<Value, ApiError>),
    Like,
    LikeTrack(i64),
    Likes(Result<Value, ApiError>),
    Liked(i64, bool, Result<Value, ApiError>),
    RefreshLikes,
    Share,
    Shared(Result<Value, ApiError>),
    /// Save the song in front of you on this computer.
    SaveOffline,
    /// "Download this song": save the library file where the user picks.
    DownloadSong,
    /// "Report a problem" with the song in front of you.
    ReportSong,
    /// Lyrics mode's "Report lyrics": Report a problem with Wrong lyrics ticked.
    ReportLyrics,
    /// The library's song dialogs and downloads.
    SongAct(crate::pages::library::Act),
    ViewAlbum,
    GoToArtist,
    /// The "Library" link of the empty bar.
    OpenLibrary,
    /// "Open Trash" of the toast after albums went to the Trash (0.36): Settings → Files on the Trash.
    OpenTrash,
    OpenMini,
    Lyrics(i64, Result<Value, ApiError>),
    /// Lyrics mode: the scroll area is at `.0` and the current line needs `.1`.
    LyricsScroll(f32, f32),
    /// A frame while the lyrics glide to the current line.
    LyricsFrame(std::time::Instant),
    /// Bring the current line to the middle once the lyrics are on screen.
    LyricsCentre,
    SeekTo(f64),
    Jump(usize),
    RemoveAt(usize),
    MoveUp(usize),
    /// Up next's Clear: everything after the song playing goes ("Up next cleared", with Undo).
    ClearUpNext,
    /// Undo of Up next's Clear.
    UndoClear,
    /// The library shuffle that keeps the music going (`true` = the queue had ended).
    More(bool, Result<Value, ApiError>),
    Signal,
    TrackInfo(i64, Result<Value, ApiError>),
    /// The row of a song queued from elsewhere (its loudness, for normalization).
    GainRow(i64, Result<Value, ApiError>),
    Devices,
    DevicesList(Result<Value, ApiError>),
    Zones(Result<Value, ApiError>),
    /// A command for another device.
    Command(String, Value),
    Sent(Result<Value, ApiError>),
    LinkTo(Value),
    Unlink,
    PlayHere(Value),
    SendThere(String),
    ZoneQuery(String, String),
    ZonePlay(String),
    ZoneCommand(String, Value),
    ZoneDone(Result<Value, ApiError>),
    Spot(Result<Value, ApiError>),
    SpotPlay,
    SpotDismiss,
    SpotTracks(Value, Result<Value, ApiError>),
    AddToPlaylist(Vec<i64>),
    /// The group listen (start, join, the group dialog, its event stream).
    Group(group::Msg),
    Playlists(Result<Value, ApiError>),
    AddTo(i64),
    NewPlaylistName(String),
    CreatePlaylist,
    Created(Result<Value, ApiError>),
    Added(Result<Value, ApiError>),
    /// `/library/tracks/{id}/added` for the song the ⋯ menu opened for.
    MenuAdded(i64, Result<Value, ApiError>),
    Nothing,
}

/// A week, in seconds: older spots are not offered.
const WEEK: f64 = 7.0 * 24.0 * 3600.0;

/// Send `body` to the linked device, when there is one.
fn via_link(state: &State, ctx: &Ctx, body: Value) -> Option<Task<Message>> {
    let l = state.devices.linked.as_ref()?;
    Some(devices::send(ctx.api.clone(), l.id.clone(), body))
}

fn likes_task(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/likes").await }, Message::Likes)
}

fn shuffle_more(state: &mut State, ctx: &Ctx, start: bool) -> Task<Message> {
    if state.topping {
        return Task::none();
    }
    state.topping = true;
    let exclude: Vec<i64> = state.status.queue.iter().filter_map(queue::lib_id).collect();
    let api = ctx.api.clone();
    Task::perform(
        async move {
            let r = api.post("/library/shuffle", json!({ "limit": 50, "exclude": exclude })).await?;
            if fmt::items(&r).is_empty() {
                // everything has played: start a fresh round
                return api.post("/library/shuffle", json!({ "limit": 50, "exclude": [] })).await;
            }
            Ok(r)
        },
        move |r| Message::More(start, r),
    )
}

fn save_spot(state: &mut State, force: bool) {
    if state.devices.linked.is_some() {
        return;
    }
    let now = queue::now();
    if !force && now - state.last_save < 5.0 {
        return;
    }
    let Some(v) = queue::spot_json(&state.status, state.shuffle, state.repeat, now) else {
        return;
    };
    state.last_save = now;
    queue::save_spot(&v);
}

fn close_views(state: &mut State) {
    state.full = None;
    state.menu = None;
    state.dialog = None;
}

/// Bring the current line of the lyrics to the middle (measured on screen; see
/// `Message::LyricsScroll`).
fn lyrics_scroll(_state: &State) -> Task<Message> {
    lyrics::centre_task().map(|(now, to)| Message::LyricsScroll(now, to))
}

/// The lyrics just appeared (the mode opened, they loaded): centre the current line once they
/// are drawn.
fn lyrics_scroll_soon() -> Task<Message> {
    Task::perform(tokio::time::sleep(Duration::from_millis(80)), |_| Message::LyricsCentre)
}

/// How long the lyrics take to glide to the next line.
const LYRICS_GLIDE: Duration = Duration::from_millis(450);

fn scroll_lyrics_to(y: f32) -> Task<Message> {
    iced::widget::operation::scroll_to(
        lyrics::SCROLL_ID,
        iced::widget::operation::AbsoluteOffset { x: None, y: Some(y.max(0.0)) },
    )
}

/// The web's `encore.lyricsMode`: the full view opens on the lyrics when it was left there.
pub const LYRICS_MODE_KEY: &str = "encore.lyricsMode";

/// Whether the full view was last left in lyrics mode.
pub fn lyrics_mode(c: &crate::config::Config) -> bool {
    match c.local(LYRICS_MODE_KEY) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.as_str(), "1" | "true"),
        _ => false,
    }
}

fn remember_lyrics_mode(ctx: &mut Ctx, on: bool) {
    if lyrics_mode(ctx.config) != on {
        ctx.edit_config(move |c| {
            c.local.insert(LYRICS_MODE_KEY.into(), Value::Bool(on));
        });
    }
}

fn on_event(state: &mut State, ev: Event, ctx: &mut Ctx) -> Task<Message> {
    match ev {
        Event::Status(s) => {
            let prev = std::mem::replace(&mut state.status, s);
            let now = Arc::clone(&state.status);
            group::on_status(state, &prev);
            // in a group the group's queue plays as it is: no repeat, no library shuffle after it
            let grouped = group::active();
            let mut tasks = vec![group::count_play(state, ctx)];
            if let Some(e) = now.error.as_ref() {
                if state.last_error.as_ref() != Some(e) {
                    state.last_error = Some(e.clone());
                    ctx.toast_err(e.clone());
                }
            } else {
                state.last_error = None;
            }
            // A queue of only the Test sound just stops: no repeat, no library shuffle after it.
            let only_tone = now.queue.iter().all(engine::test_sound::is_tone);
            if queue::reached_end(&prev, &now) && !state.user_stop && !only_tone && !grouped {
                let last = now.queue.len() - 1;
                match queue::at_end(state.repeat, state.autoplay(ctx), last) {
                    queue::AtEnd::Nothing => {}
                    queue::AtEnd::Restart => engine::send(Command::PlayIndex(0)),
                    queue::AtEnd::Again(i) => engine::send(Command::PlayIndex(i)),
                    queue::AtEnd::Library => tasks.push(shuffle_more(state, ctx, true)),
                }
            }
            if now.state == PlayState::Stopped {
                state.user_stop = false;
            }
            if now.index != prev.index && !grouped {
                // The song changed: on its own (the previous one ended) or because the user
                // pressed Next / Previous / a queue row.
                let auto = !state.user_skip;
                state.user_skip = false;
                if let (Some(p), Some(i)) = (prev.index, now.index) {
                    if auto
                        && state.repeat == Repeat::One
                        && now.state != PlayState::Stopped
                        && i == p + 1
                        && prev.queue == now.queue
                    {
                        engine::send(Command::PlayIndex(p));
                    }
                }
                if state.repeat == Repeat::Off
                    && state.autoplay(ctx)
                    && now.index.is_some_and(|i| i + 1 >= now.queue.len())
                    && now.queue.iter().any(|q| queue::lib_id(q).is_some())
                {
                    tasks.push(shuffle_more(state, ctx, false));
                }
            }
            if state.full == Some(Pane::Lyrics) && state.devices.linked.is_none() && state.lyrics.sync(now.position) {
                tasks.push(lyrics_scroll(state));
            }
            let paused_now = prev.state == PlayState::Playing && now.state == PlayState::Paused;
            if (now.state == PlayState::Playing || paused_now) && !grouped {
                save_spot(state, paused_now);
            }
            Task::batch(tasks)
        }
        Event::TrackStarted(t) => {
            let mut tasks = Vec::new();
            state.user_stop = false;
            let id = engine::tracks::encore_id(&t);
            // a song queued from elsewhere (another device through the link): its row says how
            // loud it is, for normalization
            if let Some(id) = id.filter(|i| engine::normalize::prefs().on() && !engine::normalize::seen(*i)) {
                let api = ctx.api.clone();
                tasks.push(Task::perform(
                    async move { api.get(&format!("/library/tracks/{id}")).await },
                    move |r| Message::GainRow(id, r),
                ));
            }
            if state.full == Some(Pane::Lyrics) && state.devices.linked.is_none() {
                match id {
                    Some(id) => tasks.push(state.lyrics.load(ctx.api.clone(), id, Message::Lyrics)),
                    None => state.lyrics.clear(),
                }
            } else if state.lyrics.id != id {
                state.lyrics.clear();
            }
            if state.dialog == Some(Dialog::Signal) {
                if let Some(id) = id {
                    tasks.push(state.signal.load(ctx.api.clone(), id));
                }
            }
            if state.liked.is_none() && !state.likes_loading && id.is_some() {
                state.likes_loading = true;
                tasks.push(likes_task(ctx));
            }
            // "now playing" on Last.fm / ListenBrainz: the device link reports it while it
            // runs; before a token exists the app does it itself (and always for a group
            // listen's songs, which the link does not see as library songs)
            if let Some(id) = id.filter(|_| !crate::link::connected() || group::relay::live_of(&t).is_some()) {
                let api = ctx.api.clone();
                tasks.push(Task::perform(
                    async move { api.post("/library/now-playing", json!({ "track_id": id })).await },
                    |_| Message::Nothing,
                ));
            }
            state.spot = None;
            if !group::active() {
                save_spot(state, true);
            }
            Task::batch(tasks)
        }
        Event::TrackPlayed { track, seconds } => {
            // Counted like the web player (half the song or four minutes, the engine's
            // threshold); the link logs it while it runs. A group listen's songs are counted by
            // `group::count_play` (the engine counts a live stream only after four minutes).
            if group::relay::live_of(&track).is_some() {
                return Task::none();
            }
            match engine::tracks::encore_id(&track).filter(|_| !crate::link::connected()) {
                Some(id) => {
                    let api = ctx.api.clone();
                    Task::perform(
                        async move {
                            api.post("/library/plays", json!({ "track_id": id, "seconds": seconds }))
                                .await
                        },
                        |_| Message::Nothing,
                    )
                }
                None => Task::none(),
            }
        }
    }
}

fn tick(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.ticks += 1;
    let mut tasks = Vec::new();
    for c in devices::take_pending() {
        if let Some(e) = c["error"].as_str() {
            ctx.toast_err(e.to_string());
        } else if let Some(t) = via_link(state, ctx, c) {
            tasks.push(t);
        }
    }
    let linked = state.devices.linked.is_some();
    if linked {
        state.devices.tick();
        if state.full == Some(Pane::Lyrics) {
            let pos = state.devices.linked.as_ref().map(|l| fmt::f(&l.state, "position")).unwrap_or(0.0);
            if state.lyrics.sync(pos) {
                tasks.push(lyrics_scroll(state));
            }
        }
    }
    let watching = linked || state.dialog == Some(Dialog::Devices);
    if watching && state.ticks.is_multiple_of(2) && ctx.session.signed_in() {
        tasks.push(devices::fetch(ctx.api.clone()));
        if state.dialog == Some(Dialog::Devices) && ctx.has_feature("roon") {
            tasks.push(devices::fetch_zones(ctx.api.clone()));
        }
    }
    if ctx.session.signed_in() {
        // the account's Quality and server upsampling choose what the streams ask for
        let uid = ctx.session.user_id();
        if state.dsp_for != Some(uid) {
            state.dsp_for = Some(uid);
            state.dsp_info = Value::Null;
            let api = ctx.api.clone();
            tasks.push(Task::perform(async move { api.get("/playback/dsp").await }, move |r| {
                Message::DspInfo(uid, r)
            }));
        }
        let rate = state.status.output_format.map(|f| f.rate).unwrap_or(0);
        engine::tracks::set_query(engine::tracks::stream_query(&playback_prefs(ctx), &state.dsp_info, rate));
    }
    let ticks = state.ticks;
    tasks.push(group::on_tick(state, ticks, ctx));
    if !state.spot_checked && ctx.session.signed_in() {
        state.spot_checked = true;
        let resume = playback_prefs(ctx)["resume"] != Value::Bool(false);
        if resume && state.status.track.is_none() && !linked {
            let api = ctx.api.clone();
            if ctx.has_feature("remote") {
                tasks.push(Task::perform(async move { api.get("/remote/spot").await }, Message::Spot));
            } else {
                tasks.push(Task::done(Message::Spot(Ok(Value::Null))));
            }
        }
    }
    Task::batch(tasks)
}

/// Encore rows as queue entries with this server's credential.
fn tracks_of(ctx: &Ctx, rows: &[Value]) -> Vec<Track> {
    let auth = crate::link::auth(ctx.config, &ctx.api);
    engine::tracks::many(&ctx.base(), &auth, rows)
}

fn play_rows(ctx: &mut Ctx, rows: &[Value], index: usize, position: f64) {
    let list = tracks_of(ctx, rows);
    if let Err(e) = engine::play(list, index, position) {
        ctx.toast_err(e);
    }
}

fn offer_spot(state: &mut State, ctx: &Ctx, remote: Value) {
    if state.status.track.is_some() || state.devices.linked.is_some() || state.group.state.is_some() {
        return;
    }
    let me = crate::link::device_id(&ctx.config.server);
    let local = queue::saved_spot();
    let at = fmt::f(&remote, "updated_at");
    let other = remote.is_object()
        && remote["track_id"].is_number()
        && fmt::truthy(&remote, "title")
        && fmt::s(&remote, "device_id") != me
        && queue::now() - at < WEEK;
    if other && !local.as_ref().is_some_and(|l| fmt::f(l, "saved_at") >= at) {
        let when = fmt::s(&remote, "updated_at");
        if state.spot_dismissed.as_deref() != Some(when.as_str()) {
            state.spot = Some(SpotOffer {
                spot: remote,
                remote: true,
            });
            return;
        }
    }
    if let Some(l) = local {
        state.spot = Some(SpotOffer { spot: l, remote: false });
    }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Toggle if state.group.state.is_some() => return group::toggle(state, ctx),
        Message::Next | Message::Prev if state.group.state.is_some() => {
            return group::skip(state, matches!(msg, Message::Next), ctx)
        }
        Message::SeekTo(v) if state.group.state.is_some() => return group::seek(state, v, ctx),
        // Shuffle in a group: the group's Up next in a random order, once (those who control it);
        // repeat is off
        Message::SetShuffle(_) if state.group.state.is_some() => return group::update(state, group::Msg::Shuffle, ctx),
        Message::SetRepeat(_) if state.group.state.is_some() => group::no_repeat(ctx),
        // Up next shows the group's queue (its own messages): the engine's indices mean nothing
        Message::Jump(_) | Message::RemoveAt(_) | Message::MoveUp(_) if state.group.state.is_some() => {}
        Message::ClearUpNext if state.group.state.is_some() => return group::update(state, group::Msg::AskClear, ctx),
        Message::UndoClear if state.group.state.is_some() => state.cleared = None,
        Message::LinkTo(_) | Message::PlayHere(_) | Message::SendThere(_) if state.group.state.is_some() => {
            group::no_linking(ctx)
        }
        // the group plays on: the bar has no close button in a group
        Message::ClosePlayer if state.group.state.is_some() => {}
        Message::Group(m) => return group::update(state, m, ctx),
        Message::Toggle => {
            if state.devices.linked.is_some() {
                let playing = state.devices.linked.as_ref().is_some_and(|l| fmt::truthy(&l.state, "playing"));
                state.devices.assume("playing", json!(!playing));
                return via_link(state, ctx, json!({ "action": "toggle" })).unwrap_or_else(Task::none);
            }
            engine::send(Command::Toggle);
        }
        Message::Next => {
            if let Some(t) = via_link(state, ctx, json!({ "action": "next" })) {
                return t;
            }
            state.user_skip = true;
            engine::send(Command::Next);
        }
        Message::Prev => {
            if let Some(t) = via_link(state, ctx, json!({ "action": "prev" })) {
                return t;
            }
            state.user_skip = true;
            engine::send(Command::Prev);
        }
        Message::SeekDrag(v) => state.seek_drag = Some(v),
        Message::SeekDone => {
            if let Some(v) = state.seek_drag.take() {
                return update(state, Message::SeekTo(v), ctx);
            }
        }
        Message::KeyToggle => {
            // a window of the player on top takes the key; nothing to toggle without a song
            let window = state.menu.is_some()
                || state.dialog.is_some()
                || state.customize
                || state.shared.dialog.is_some()
                || state.group.dialog.is_some();
            if !window && (state.status.track.is_some() || state.devices.linked.is_some()) {
                return update(state, Message::Toggle, ctx);
            }
        }
        Message::SeekBy(d) => {
            if let Some(n) = state.now() {
                return update(state, Message::SeekTo(n.position + d), ctx);
            }
        }
        Message::SeekTo(v) => {
            let v = v.max(0.0);
            if state.devices.linked.is_some() {
                state.devices.assume("position", json!(v));
                return via_link(state, ctx, json!({ "action": "seek", "value": v })).unwrap_or_else(Task::none);
            }
            engine::send(Command::Seek(v));
        }
        Message::Volume(v) => {
            state.vol_drag = Some(v);
            if state.devices.linked.is_some() {
                state.devices.assume("volume", json!(v));
            } else {
                engine::set_volume(v);
            }
        }
        Message::VolumeDone => {
            if let Some(v) = state.vol_drag.take() {
                if let Some(t) = via_link(state, ctx, json!({ "action": "volume", "value": v })) {
                    return t;
                }
                ctx.edit_config(move |c| c.volume = v);
            }
        }
        Message::Engine(ev) => return on_event(state, *ev, ctx),
        Message::Tick => return tick(state, ctx),
        Message::Escape => {
            if state.group.dialog.is_some() {
                state.group.dialog = None;
            } else if state.menu.is_some() {
                state.menu = None;
            } else if state.shared.dialog.is_some() {
                state.shared.reset();
            } else if state.dialog.is_some() {
                state.dialog = None;
            } else if state.customize {
                state.customize = false;
            } else if state.full == Some(Pane::Lyrics) {
                // lyrics mode: back to the cover first
                return update(state, Message::SetPane(Pane::Now), ctx);
            } else if state.full.is_some() {
                state.full = None;
            }
        }
        Message::Customize(on) => {
            state.customize = on;
            state.menu = None;
        }
        Message::ToggleBarButton(id) => ctx.edit_config(move |c| {
            let mut chosen = c.bar_buttons();
            if let Some(i) = chosen.iter().position(|x| *x == id) {
                chosen.remove(i);
            } else {
                chosen.push(id);
            }
            // Keep the bar's order the table's order.
            c.bar_buttons = BAR_ACTIONS
                .iter()
                .filter(|(k, _, _)| chosen.iter().any(|x| x == k))
                .map(|(k, _, _)| k.to_string())
                .collect();
        }),
        Message::ResetBar => ctx.edit_config(|c| c.bar_buttons = BAR_DEFAULT.iter().map(|s| s.to_string()).collect()),
        Message::OpenFull(pane) => {
            state.menu = None;
            // the player opens in lyrics mode when it was left in it (encore.lyricsMode)
            let pane = if state.full.is_none() && pane == Pane::Now && lyrics_mode(ctx.config) {
                Pane::Lyrics
            } else {
                pane
            };
            if pane == Pane::Lyrics {
                remember_lyrics_mode(ctx, true);
            }
            state.lyrics_glide = None;
            state.full = Some(pane);
            if pane == Pane::Lyrics {
                match state.now().filter(|n| !n.remote).and_then(|n| n.id) {
                    Some(id) => {
                        let t = state.lyrics.load(ctx.api.clone(), id, Message::Lyrics);
                        state.lyrics.sync(state.status.position);
                        return Task::batch([t, lyrics_scroll_soon()]);
                    }
                    None => {
                        if let Some(id) = state.now().and_then(|n| n.id) {
                            return state.lyrics.load(ctx.api.clone(), id, Message::Lyrics);
                        }
                        state.lyrics.clear();
                    }
                }
            }
        }
        Message::CloseFull => state.full = None,
        Message::SetPane(p) => {
            if p == Pane::Now {
                // "Back to the cover"
                remember_lyrics_mode(ctx, false);
            }
            // the pane asked for, not the remembered one
            state.full.get_or_insert(p);
            return update(state, Message::OpenFull(p), ctx);
        }
        Message::ClosePlayer => {
            state.user_stop = true;
            if let Some(e) = engine::engine() {
                e.send(Command::Stop);
                e.play_queue(Vec::new(), 0, 0.0);
            }
            queue::forget_order();
            queue::clear_spot();
            state.lyrics.clear();
            close_views(state);
        }
        Message::Menu => {
            state.menu = Some(ctx.cursor);
            state.menu_added = None;
            // a library song: who added it, the menu's first line once the server says
            if let Some(id) = state.now().and_then(|n| n.id) {
                let api = ctx.api.clone();
                return Task::perform(async move { api.get(&format!("/library/tracks/{id}/added")).await }, move |r| {
                    Message::MenuAdded(id, r)
                });
            }
        }
        Message::MenuAdded(id, Ok(v)) => {
            let text = crate::pages::library_types::added_of(&v, fmt::date);
            if state.menu.is_some() && state.now().and_then(|n| n.id) == Some(id) && !text.is_empty() {
                state.menu_added = Some((id, text));
            }
        }
        Message::MenuAdded(_, Err(_)) => {}
        Message::CloseMenu => state.menu = None,
        Message::DownloadSong => {
            state.menu = None;
            let Some(n) = state.now().filter(|n| n.id.is_some()) else {
                return Task::none();
            };
            // the row the library's download builds its file name from
            let ext = n
                .format
                .as_deref()
                .and_then(|f| f.split_whitespace().next())
                .unwrap_or("")
                .to_lowercase();
            let row = json!({ "id": n.id, "title": n.title, "artist": n.artist, "format": ext });
            let act = crate::pages::library::Act::DownloadFile(Box::new(row));
            return crate::pages::library::shared_update(&mut state.shared, act, ctx).map(Message::SongAct);
        }
        Message::ReportSong => {
            state.menu = None;
            if let Some(n) = state.now() {
                if let Some(id) = n.id {
                    crate::pages::library::open_report(&mut state.shared, "track", json!(id), &n.title, "");
                }
            }
        }
        Message::ReportLyrics => {
            state.menu = None;
            if let Some(n) = state.now() {
                if let Some(id) = n.id {
                    crate::pages::library::open_report(&mut state.shared, "track", json!(id), &n.title, "wrong_lyrics");
                }
            }
        }
        Message::SongAct(a) => {
            // a lyrics report went through: the server already looked for other lyrics, so ask it again
            let lyrics_changed = matches!(&a, crate::pages::library::Act::Reported(Ok(res))
                if crate::pages::library::is_lyrics_reason(&crate::fmt::s(res, "reason")));
            let mut task = crate::pages::library::shared_update(&mut state.shared, a, ctx).map(Message::SongAct);
            if lyrics_changed {
                state.lyrics.clear();
                if state.full == Some(Pane::Lyrics) {
                    if let Some(id) = state.now().and_then(|n| n.id) {
                        task = Task::batch([task, state.lyrics.load(ctx.api.clone(), id, Message::Lyrics)]);
                    }
                }
            }
            state.shared.take_changed();
            return task;
        }
        Message::CloseDialog => state.dialog = None,
        Message::SetShuffle(on) => {
            if state.devices.linked.is_some() {
                state.devices.assume("shuffle", json!(on));
                return via_link(state, ctx, json!({ "action": "shuffle", "value": on })).unwrap_or_else(Task::none);
            }
            if state.shuffle != on {
                state.shuffle = on;
                queue::set_shuffle(on, &state.status);
                ctx.edit_config(move |c| c.shuffle = on);
                save_spot(state, true);
            }
        }
        Message::SetRepeat(r) => {
            if state.devices.linked.is_some() {
                state.devices.assume("repeat", json!(r.id()));
                return via_link(state, ctx, json!({ "action": "repeat", "value": r.id() })).unwrap_or_else(Task::none);
            }
            state.repeat = r;
            let id = r.id().to_string();
            ctx.edit_config(move |c| c.repeat = id);
            save_spot(state, true);
        }
        Message::Autoplay(on) => {
            state.autoplay = Some(on);
            // where the server keeps preferences they follow the account, as in the web app
            if ctx.has_feature("theme") {
                let mut prefs = playback_prefs(ctx);
                prefs["autoplay"] = json!(on);
                let api = ctx.api.clone();
                return Task::perform(
                    async move { api.put("/auth/prefs", json!({ "playback": prefs })).await },
                    Message::PrefsSaved,
                );
            }
        }
        Message::PrefsSaved(r) => match r {
            Ok(_) => ctx.refresh_session(),
            Err(e) => ctx.toast_err(e.message),
        },
        Message::Like => {
            if let Some(id) = state.now().and_then(|n| n.id) {
                return update(state, Message::LikeTrack(id), ctx);
            }
        }
        Message::LikeTrack(id) => {
            let Some(set) = state.liked.as_ref() else {
                state.pending_like = Some(id);
                if !state.likes_loading {
                    state.likes_loading = true;
                    return likes_task(ctx);
                }
                return Task::none();
            };
            let api = ctx.api.clone();
            let now_liked = !set.contains(&id);
            return Task::perform(
                async move {
                    if now_liked {
                        api.post_empty(&format!("/likes/{id}")).await
                    } else {
                        api.del(&format!("/likes/{id}")).await
                    }
                },
                move |r| Message::Liked(id, now_liked, r),
            );
        }
        Message::Likes(r) => {
            state.likes_loading = false;
            match r {
                Ok(v) => {
                    state.liked = Some(fmt::items(&v).iter().filter_map(Value::as_i64).collect());
                    if let Some(id) = state.pending_like.take() {
                        return update(state, Message::LikeTrack(id), ctx);
                    }
                }
                Err(_) => state.liked = Some(HashSet::new()),
            }
        }
        Message::Liked(id, on, r) => match r {
            Ok(_) => {
                if let Some(set) = state.liked.as_mut() {
                    if on {
                        set.insert(id);
                    } else {
                        set.remove(&id);
                    }
                }
                ctx.toast_ok(if on { "Added to Liked Songs" } else { "Removed from Liked Songs" });
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Message::RefreshLikes => {
            state.likes_loading = true;
            return likes_task(ctx);
        }
        Message::SaveOffline => {
            state.menu = None;
            if let Some(n) = state.now().filter(|n| n.id.is_some() && !n.remote) {
                let row = json!({
                    "id": n.id,
                    "title": n.title,
                    "artist": n.artist,
                    "album": n.album,
                    "album_id": n.album_id,
                    "format": n.format,
                    "duration": n.duration,
                });
                crate::desktop::offline::save(ctx, vec![row]);
            }
        }
        Message::Share => {
            state.menu = None;
            if let Some(id) = state.now().and_then(|n| n.id) {
                let api = ctx.api.clone();
                return Task::perform(
                    async move { api.post_empty(&format!("/library/tracks/{id}/share")).await },
                    Message::Shared,
                );
            }
        }
        Message::Shared(r) => match r {
            Ok(v) => {
                let path = fmt::s(&v, "path");
                let url = if path.is_empty() { fmt::s(&v, "url") } else { format!("{}{path}", ctx.base()) };
                ctx.copy(url);
                ctx.toast_ok("Link copied – anyone with it can play this song");
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Message::ViewAlbum => {
            if let Some(a) = state.now().and_then(|n| n.album_id) {
                close_views(state);
                ctx.navigate(Page::LibraryAlbum(a));
            }
        }
        Message::GoToArtist => {
            if let Some(a) = state.now().map(|n| n.artist).filter(|a| !a.is_empty()) {
                close_views(state);
                ctx.navigate(Page::ArtistName(a));
            }
        }
        Message::OpenLibrary => ctx.navigate(Page::library("")),
        Message::OpenTrash => {
            crate::pages::files::open_on_trash();
            ctx.navigate(Page::Settings(crate::pages::Section::Files));
        }
        Message::OpenMini => {
            state.menu = None;
            ctx.edit_config(|c| c.mini_player = true);
        }
        Message::Lyrics(id, r) => {
            state.lyrics.loaded(id, r);
            let pos = state.now().map(|n| n.position).unwrap_or(0.0);
            state.lyrics.sync(pos);
            return lyrics_scroll_soon();
        }
        Message::LyricsCentre => {
            if state.full == Some(Pane::Lyrics) {
                return lyrics_scroll(state);
            }
        }
        Message::LyricsScroll(now, to) => {
            if (to - now).abs() < 1.0 {
                state.lyrics_glide = None;
            } else if crate::widgets::marquee::motion_on() {
                state.lyrics_glide = Some((now, to, std::time::Instant::now()));
            } else {
                state.lyrics_glide = None;
                return scroll_lyrics_to(to);
            }
        }
        Message::LyricsFrame(at) => {
            if let Some((from, to, started)) = state.lyrics_glide {
                let elapsed = at.saturating_duration_since(started);
                if elapsed >= LYRICS_GLIDE || state.full != Some(Pane::Lyrics) {
                    state.lyrics_glide = None;
                }
                return scroll_lyrics_to(lyrics::eased(from, to, elapsed, LYRICS_GLIDE));
            }
        }
        Message::Jump(i) => {
            if let Some(l) = state.devices.linked.as_ref() {
                let q = l.state["queue"].clone();
                if q.as_array().is_some_and(|a| !a.is_empty()) {
                    return via_link(state, ctx, json!({ "action": "play_queue", "tracks": q, "index": i }))
                        .unwrap_or_else(Task::none);
                }
                return Task::none();
            }
            state.user_skip = true;
            queue::jump(i);
        }
        Message::RemoveAt(i) => queue::remove_at(&state.status, i),
        Message::MoveUp(i) => queue::move_up(&state.status, i),
        Message::ClearUpNext => {
            // another device's queue is that device's
            if state.devices.linked.is_some() {
                return Task::none();
            }
            let gone = queue::clear_rest(&state.status);
            if gone.is_empty() {
                return Task::none();
            }
            state.cleared = Some(Cleared { tracks: gone, at: std::time::Instant::now() });
            ctx.toast_action(UP_NEXT_CLEARED, "Undo", Message::UndoClear, UNDO_MS);
            save_spot(state, true);
        }
        Message::UndoClear => {
            let Some(c) = state.cleared.take() else {
                return Task::none();
            };
            if c.at.elapsed().as_millis() as u64 > UNDO_MS + 500 || state.devices.linked.is_some() {
                return Task::none();
            }
            // after the song playing, before anything added since
            queue::order_insert_after(&state.status, &c.tracks);
            engine::send(Command::PlayNext(c.tracks));
            save_spot(state, true);
        }
        Message::More(start, r) => {
            state.topping = false;
            match r {
                Ok(v) => {
                    let rows = fmt::items(&v);
                    if rows.is_empty() {
                        return Task::none();
                    }
                    let list = tracks_of(ctx, &rows);
                    if start {
                        if let Err(e) = engine::play(list, 0, 0.0) {
                            ctx.toast_err(e);
                        }
                    } else if let Some(e) = engine::engine() {
                        queue::order_push(&list);
                        e.send(Command::Enqueue(list));
                    }
                }
                Err(e) => log::warn!("library shuffle: {}", e.message),
            }
        }
        Message::Signal => {
            state.menu = None;
            state.dialog = Some(Dialog::Signal);
            if let Some(id) = state.now().and_then(|n| n.id) {
                return state.signal.load(ctx.api.clone(), id);
            }
        }
        Message::TrackInfo(id, r) => state.signal.loaded(id, r),
        Message::GainRow(id, r) => {
            if let Ok(mut row) = r {
                if row.is_object() && engine::tracks::id_of(&row).is_none() {
                    row["id"] = json!(id);
                }
                engine::normalize::remember(&row);
                if state.snapshot().current_id() == Some(id) {
                    engine::normalize::reapply();
                }
            }
        }
        Message::Devices => {
            state.menu = None;
            state.dialog = Some(Dialog::Devices);
            let mut tasks = vec![devices::fetch(ctx.api.clone()), group::refresh_list(&mut state.group, ctx)];
            if ctx.has_feature("roon") {
                tasks.push(devices::fetch_zones(ctx.api.clone()));
            }
            return Task::batch(tasks);
        }
        Message::DevicesList(r) => match r {
            Ok(v) => {
                let me = crate::link::device_id(&ctx.config.server);
                state.devices.got(fmt::items(&v), &me);
                // the linked device moved on to another song: its lyrics
                if state.full == Some(Pane::Lyrics) {
                    if let Some(id) = state.devices.linked.as_ref().and_then(|l| l.state["id"].as_i64()) {
                        return state.lyrics.load(ctx.api.clone(), id, Message::Lyrics);
                    }
                }
            }
            Err(_) => state.devices.loaded = true,
        },
        Message::Zones(r) => {
            if let Ok(v) = r {
                state.devices.zones = fmt::items(&v);
            }
        }
        Message::Command(id, body) => return devices::send(ctx.api.clone(), id, body),
        Message::Sent(r) => devices::sent(r, ctx),
        Message::LinkTo(d) => {
            state.spot = None;
            state.devices.link_to(&d);
            state.lyrics.clear();
            engine::send(Command::Pause);
            ctx.toast_ok(format!("Linked – this device now controls {}", fmt::s(&d, "name")));
            return devices::fetch(ctx.api.clone());
        }
        Message::Unlink => {
            state.devices.unlink();
            state.lyrics.clear();
            ctx.toast_ok("Unlinked – music plays on this device again");
        }
        Message::PlayHere(d) => {
            let s = &d["state"];
            let rows = fmt::items(&s["queue"]);
            if rows.is_empty() {
                return Task::none();
            }
            if state.devices.linked.is_some() {
                state.devices.unlink();
            }
            let index = s["index"].as_u64().unwrap_or(0) as usize;
            let position = fmt::f(s, "position").max(0.0);
            play_rows(ctx, &rows, index, position);
            state.dialog = None;
            return devices::send(ctx.api.clone(), fmt::s(&d, "id"), json!({ "action": "pause" }));
        }
        Message::SendThere(id) => match queue::send_payload(&state.status) {
            Some(body) => {
                engine::send(Command::Pause);
                return devices::send(ctx.api.clone(), id, body);
            }
            None => ctx.toast_err("Only songs from your library can be sent"),
        },
        Message::ZoneQuery(id, q) => {
            state.devices.zone_for = id;
            state.devices.zone_query = q;
        }
        Message::ZonePlay(id) => {
            let q = state.devices.zone_query.trim().to_string();
            if q.is_empty() || state.devices.zone_for != id {
                return Task::none();
            }
            return devices::zone(ctx.api.clone(), id, "play", json!({ "query": q }));
        }
        Message::ZoneCommand(id, body) => return devices::zone(ctx.api.clone(), id, "command", body),
        Message::ZoneDone(r) => match r {
            Ok(v) => {
                let playing = fmt::s(&v, "playing");
                if !playing.is_empty() {
                    ctx.toast_ok(format!("Playing {playing} in Roon"));
                    state.devices.zone_query.clear();
                }
                return devices::fetch_zones(ctx.api.clone());
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Message::Spot(r) => offer_spot(state, ctx, r.unwrap_or(Value::Null)),
        Message::DspInfo(uid, r) => {
            if state.dsp_for == Some(uid) {
                state.dsp_info = r.unwrap_or(Value::Null);
            }
        }
        Message::SpotPlay => {
            let Some(offer) = state.spot.take() else {
                return Task::none();
            };
            if !offer.remote {
                let s = offer.spot;
                let rows = fmt::items(&s["queue"]);
                let index = s["index"].as_u64().unwrap_or(0) as usize;
                let at = fmt::f(&s, "position");
                let dur = rows.get(index).map(|t| fmt::f(t, "duration")).unwrap_or(0.0);
                let at = if at > 1.0 && !(dur > 0.0 && at > dur - 2.0) { at } else { 0.0 };
                let r = Repeat::parse(&fmt::s(&s, "repeat"));
                state.repeat = r;
                // the saved queue is already in its shuffled order
                state.shuffle = fmt::truthy(&s, "shuffle");
                engine::set_shuffle(false);
                play_rows(ctx, &rows, index, at);
                engine::set_shuffle(state.shuffle);
                return Task::none();
            }
            let s = offer.spot;
            let tid = fmt::i(&s, "track_id");
            let mut ids: Vec<i64> = fmt::items(&s["queue"]).iter().filter_map(Value::as_i64).take(300).collect();
            if !ids.contains(&tid) {
                ids = vec![tid];
            }
            let api = ctx.api.clone();
            let path = format!(
                "/library/tracks?ids={}",
                ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
            );
            return Task::perform(async move { api.get(&path).await }, move |r| Message::SpotTracks(s.clone(), r));
        }
        Message::SpotDismiss => {
            if let Some(o) = state.spot.take() {
                if o.remote {
                    state.spot_dismissed = Some(fmt::s(&o.spot, "updated_at"));
                    if let Some(l) = queue::saved_spot() {
                        state.spot = Some(SpotOffer { spot: l, remote: false });
                    }
                }
            }
        }
        Message::SpotTracks(s, r) => {
            let got = match r {
                Ok(v) => fmt::items(&v),
                Err(e) => {
                    ctx.toast_err(e.message);
                    return Task::none();
                }
            };
            let tid = fmt::i(&s, "track_id");
            let ids: Vec<i64> = {
                let mut ids: Vec<i64> = fmt::items(&s["queue"]).iter().filter_map(Value::as_i64).take(300).collect();
                if !ids.contains(&tid) {
                    ids = vec![tid];
                }
                ids
            };
            let by_id: std::collections::HashMap<i64, Value> =
                got.into_iter().filter(|t| t["id"].is_number()).map(|t| (fmt::i(&t, "id"), t)).collect();
            let idx = if s["index"].is_null() { fmt::f(&s, "idx") } else { fmt::f(&s, "index") };
            let mut tracks: Vec<Value> = Vec::new();
            let mut index = None;
            for (i, id) in ids.iter().enumerate() {
                let Some(t) = by_id.get(id) else { continue };
                if i as f64 == idx && *id == tid {
                    index = Some(tracks.len());
                }
                tracks.push(t.clone());
            }
            let index = index.or_else(|| tracks.iter().position(|t| fmt::i(t, "id") == tid));
            let Some(index) = index else {
                ctx.toast_err("That song is no longer in your library");
                return Task::none();
            };
            let mut at = fmt::f(&s, "position");
            let mut tasks = Vec::new();
            if fmt::truthy(&s, "online")
                && fmt::truthy(&s, "playing")
                && (s["device_playing"].is_null() || fmt::truthy(&s, "device_playing"))
            {
                at += (queue::now() - fmt::f(&s, "updated_at")).max(0.0);
                tasks.push(devices::send(ctx.api.clone(), fmt::s(&s, "device_id"), json!({ "action": "pause" })));
            }
            let dur = fmt::f(&tracks[index], "duration");
            if dur != 0.0 && at > dur - 2.0 {
                at = 0.0;
            }
            play_rows(ctx, &tracks, index, at);
            return Task::batch(tasks);
        }
        Message::AddToPlaylist(ids) => {
            state.menu = None;
            if ids.is_empty() {
                return Task::none();
            }
            state.add_ids = ids;
            state.playlists = None;
            state.new_playlist.clear();
            state.dialog = Some(Dialog::AddToPlaylist);
            let api = ctx.api.clone();
            return Task::perform(async move { api.get("/playlists").await }, Message::Playlists);
        }
        Message::Playlists(r) => match r {
            Ok(v) => state.playlists = Some(fmt::items(&v)),
            Err(e) => {
                state.dialog = None;
                ctx.toast_err(e.message);
            }
        },
        Message::AddTo(pid) => {
            let ids = state.add_ids.clone();
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post(&format!("/playlists/{pid}/tracks"), json!({ "track_ids": ids })).await },
                Message::Added,
            );
        }
        Message::NewPlaylistName(s) => state.new_playlist = s,
        Message::CreatePlaylist => {
            let name = state.new_playlist.trim().to_string();
            let name = if name.is_empty() { "New playlist".to_string() } else { name };
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/playlists", json!({ "name": name })).await },
                Message::Created,
            );
        }
        Message::Created(r) => match r {
            Ok(v) => return update(state, Message::AddTo(fmt::i(&v, "id")), ctx),
            Err(e) => ctx.toast_err(e.message),
        },
        Message::Added(r) => match r {
            Ok(v) => {
                let dup = fmt::i(&v, "duplicates_skipped");
                ctx.toast_ok(format!(
                    "Added {}{}",
                    fmt::s(&v, "added"),
                    if dup > 0 { format!(" ({dup} already there)") } else { String::new() }
                ));
                state.dialog = None;
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Message::Nothing => {}
    }
    Task::none()
}

/// `.player`: the 84px bar (with the pick-up offer above it when there is one).
pub fn bar<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    bar::view(state, ctx)
}

/// The full view, the dialogs and the ⋯ menu, whichever is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if let Some(o) = group::view::overlay(state, ctx) {
        return Some(o);
    }
    if state.customize {
        return Some(bar::customize(state, ctx));
    }
    if let Some(o) = crate::pages::library::shared_overlay(&state.shared, ctx) {
        return Some(o.map(Message::SongAct));
    }
    if let Some(d) = state.dialog {
        return Some(match d {
            Dialog::Signal => crate::widgets::modal("Signal path", signal::view(state, ctx), None, false, Message::CloseDialog),
            Dialog::Devices => crate::widgets::modal("Your devices", devices::view(state, ctx), None, true, Message::CloseDialog),
            Dialog::AddToPlaylist => bar::add_to_playlist(state, ctx),
        });
    }
    if let Some(at) = state.menu {
        return Some(bar::more_menu(state, ctx, at));
    }
    if state.full.is_some() {
        return Some(full::view(state, ctx));
    }
    None
}

/// The cover of `url` (a library cover path or a full URL) at `size` px, when loaded.
pub fn cover_handle(ctx: &Ctx, url: &str, size: u32) -> Option<image::Handle> {
    if url.is_empty() {
        return None;
    }
    // An offline song's cover is a file on this computer.
    let path = std::path::Path::new(url);
    if path.is_absolute() && path.is_file() {
        return Some(image::Handle::from_path(path));
    }
    let u = crate::widgets::cover::absolute(&ctx.base(), &devices::with_size(url, size));
    ctx.covers.get(&u)
}

/// The engine's events, a tick every second, Escape, and the web player's in-app keys: Space
/// plays or pauses, Left / Right seek 10 s – only when no widget took the key (a focused text
/// field keeps it) and without Ctrl, Alt or the logo key (the global hotkeys use those).
pub fn subscription(state: &State) -> Subscription<Message> {
    use iced::keyboard::{key::Named, Event as Kb, Key};
    // frames only while the lyrics glide to a line
    let glide = if state.lyrics_glide.is_some() {
        iced::window::frames().map(Message::LyricsFrame)
    } else {
        Subscription::none()
    };
    Subscription::batch([
        glide,
        group::subscription(&state.group),
        engine::subscription().map(|e| Message::Engine(Box::new(e))),
        iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
        iced::event::listen_with(|e, status, _| match e {
            iced::Event::Keyboard(Kb::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            }) => Some(Message::Escape),
            iced::Event::Keyboard(Kb::KeyPressed {
                key: Key::Named(named),
                modifiers,
                ..
            }) if status == iced::event::Status::Ignored
                && !modifiers.control()
                && !modifiers.alt()
                && !modifiers.logo() =>
            {
                match named {
                    Named::Space => Some(Message::KeyToggle),
                    Named::ArrowLeft => Some(Message::SeekBy(-10.0)),
                    Named::ArrowRight => Some(Message::SeekBy(10.0)),
                    _ => None,
                }
            }
            _ => None,
        }),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyrics_mode_is_read_like_the_web_keeps_it() {
        let mut c = crate::config::Config::default();
        assert!(!lyrics_mode(&c));
        c.local.insert(LYRICS_MODE_KEY.into(), json!(true));
        assert!(lyrics_mode(&c));
        c.local.insert(LYRICS_MODE_KEY.into(), json!("1"));
        assert!(lyrics_mode(&c));
        c.local.insert(LYRICS_MODE_KEY.into(), json!("false"));
        assert!(!lyrics_mode(&c));
    }

    #[test]
    fn panes() {
        assert_eq!(Pane::by_id("queue"), Pane::Queue);
        assert_eq!(Pane::by_id("x"), Pane::Now);
        assert_eq!(Pane::Lyrics.id(), "lyrics");
    }
}
