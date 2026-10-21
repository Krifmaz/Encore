//! The relay: in a group listen every song reaches the engine through this computer's own
//! decoder, so the music can be moved in time without a seek in the engine.
//!
//! Cadenza (pinned, not ours to change here) has no playback rate, and its seek blocks the engine
//! while an HTTP stream is searched (silence, then a jump). So in a group the engine's queue holds
//! live streams (`Source::Live`, the way Cadenza takes AirPlay) and the relay feeds them:
//!
//! - one decode thread per song reads the real stream (the original file over HTTP, or what the
//!   server makes) up to 2 s ahead;
//! - the pump (one thread) moves that audio into the engine's live stream, three quarters of a
//!   second ahead of what the engine takes, through the [`Slicer`]: small drift is closed by dropping or
//!   repeating tiny crossfaded slices (at most 2 ms per 100 ms of audio), never by a seek;
//! - a seek for a large drift opens the stream again at the new spot on a thread of its own while
//!   the song plays on, waits until the group gets there, and is spliced in with a 20 ms
//!   crossfade: no silence and no click, and exactly where the group is (what a seek takes to open
//!   is waited out, not guessed, and the engine's own delay to the speakers is measured from its
//!   status and added). A server-made stream is opened again with `t=` (it can't seek
//!   backwards);
//! - the next song is opened and buffered about 30 s before the end of the one playing, so the
//!   engine runs gaplessly into it (guests and members alike: there is nothing else to wait for);
//! - a stream that breaks off is opened again where it broke off, quietly, and nothing of it goes
//!   to the group (only a person's own seek is ever sent as one).
//!
//! The engine reports the position of a live stream from the marks the pump sets (the source
//! position of the frame it takes next), so the group sync measures drift as before. Leaving the
//! group puts the real songs back in the engine's queue ([`Relay::leave`]).
//!
//! The pure parts (marks, trimming, crossfades, URLs) are tested below; `relay_tests` plays real
//! files through the engine.

use super::slice::Slicer;
use super::sync::Aim;
use crate::engine::{Command, Engine, PlayState, Status, Track};
use cadenza::decode::live::{self, LiveStream};
use cadenza::decode::Source;
use cadenza::engine::decoder::TrackDecoder;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub use crate::engine::tracks::GROUP_LIVE as PREFIX;

/// How far the pump keeps the engine's live stream ahead of what it takes (s). More than a
/// device's buffer: the engine fills that in one go when it starts, and a live stream that runs
/// dry plays silence.
const LIVE_AHEAD_S: f64 = 0.75;
/// How far a decode thread reads ahead (s).
const DECODE_AHEAD_S: f64 = 2.0;
/// After running dry, the engine waits for this much (ms) before it plays again.
const PREBUFFER_MS: u32 = 40;
/// The pieces handed to the engine (frames): small, so a mark is never far off.
const PIECE: usize = 512;
/// The pump's beat.
const TICK: Duration = Duration::from_millis(4);
/// The engine's position is set again this often.
const MARK_EVERY: Duration = Duration::from_millis(100);
/// The next song is opened this long before the end of the one playing (s).
const NEXT_BEFORE_S: f64 = 30.0;
/// The crossfade of a splice (s).
const SPLICE_S: f64 = 0.020;
/// A seek that opened early waits at most this long for the group to get there.
const EARLY_WAIT: Duration = Duration::from_secs(4);
/// A song whose stream ended waits at most this long for the next song to be opened.
const NEXT_WAIT: Duration = Duration::from_secs(5);
/// A broken stream is opened again this many times.
const REOPENS: u32 = 3;
/// A song just opened is kept this long even when the engine's status does not show it yet.
const GRACE: Duration = Duration::from_secs(10);
/// A load or seek that has not finished after this long is given up.
const GIVE_UP: Duration = Duration::from_secs(25);

/// The live stream id of a group entry.
pub fn live_id(qid: &str) -> String {
    format!("{PREFIX}{qid}")
}

/// The relay's id of an engine entry, when it is one.
pub fn live_of(t: &Track) -> Option<&str> {
    match &t.source {
        Source::Live { live } if live.starts_with(PREFIX) => Some(live.as_str()),
        _ => None,
    }
}

/// The engine's entry for a group song: the real one, its source the relay's stream.
pub fn live_track(real: &Track, qid: &str) -> Track {
    Track {
        source: Source::Live { live: live_id(qid) },
        ..real.clone()
    }
}

/// A stream the server makes (a transcode, Data saver, the server's Cadenza): it has a query.
pub fn server_made(t: &Track) -> bool {
    matches!(&t.source, Source::Url { url, .. } if url.contains("/stream?"))
}

/// `url` starting at `t` seconds (the server's `t=`), in place of any `t` it had.
pub fn with_t(url: &str, t: f64) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut parts: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("t="))
        .map(str::to_string)
        .collect();
    parts.push(format!("t={t:.1}"));
    format!("{base}?{}", parts.join("&"))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn mono_ms() -> i64 {
    super::mono_ms()
}

// ----- decoded audio ------------------------------------------------------------------------

/// Decoded audio: `pcm` (interleaved) whose first frame is `at` seconds into the song.
#[derive(Clone, Debug, PartialEq)]
struct Chunk {
    at: f64,
    pcm: Vec<f64>,
}

/// Decoded audio waiting, with its frame count.
#[derive(Debug, Default)]
struct Run {
    chunks: VecDeque<Chunk>,
    frames: usize,
    eof: bool,
}

impl Run {
    fn push(&mut self, c: Chunk, ch: usize) {
        self.frames += c.pcm.len() / ch;
        self.chunks.push_back(c);
    }

    /// Drop everything before `t` seconds (frames of a chunk that starts before it, too).
    fn trim_to(&mut self, t: f64, rate: u32, ch: usize) {
        while let Some(c) = self.chunks.front_mut() {
            let n = c.pcm.len() / ch;
            let end = c.at + n as f64 / f64::from(rate);
            if end <= t {
                self.frames -= n;
                self.chunks.pop_front();
                continue;
            }
            let cut = (((t - c.at) * f64::from(rate)).round().max(0.0) as usize).min(n);
            if cut > 0 {
                c.pcm.drain(..cut * ch);
                c.at += cut as f64 / f64::from(rate);
                self.frames -= cut;
            }
            break;
        }
    }

    /// Up to `n` frames off the front.
    fn take(&mut self, n: usize, rate: u32, ch: usize) -> Vec<f64> {
        let mut out = Vec::with_capacity(n * ch);
        while out.len() < n * ch {
            let Some(c) = self.chunks.front_mut() else { break };
            let want = n - out.len() / ch;
            let have = c.pcm.len() / ch;
            if have <= want {
                out.extend_from_slice(&c.pcm);
                self.frames -= have;
                self.chunks.pop_front();
            } else {
                out.extend(c.pcm.drain(..want * ch));
                c.at += want as f64 / f64::from(rate);
                self.frames -= want;
            }
        }
        out
    }

    fn first_at(&self) -> Option<f64> {
        self.chunks.front().map(|c| c.at)
    }
}

/// `n` frames from `old` into `new` (raised cosine); `old` may be shorter (the rest fades in
/// from silence), so the join is never a step.
fn crossfade(old: &[f64], new: &[f64], ch: usize) -> Vec<f64> {
    let n = new.len() / ch;
    let mut out = Vec::with_capacity(n * ch);
    for i in 0..n {
        let w = 0.5 - 0.5 * (std::f64::consts::PI * (i as f64 + 0.5) / n as f64).cos();
        for c in 0..ch {
            let a = old.get(i * ch + c).copied().unwrap_or(0.0);
            out.push(a * (1.0 - w) + new[i * ch + c] * w);
        }
    }
    out
}

/// What was handed to the engine: output frames `out.0..out.1` came from `src.0..src.1` s.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Piece {
    out: (u64, u64),
    src: (f64, f64),
}

/// The song position of output frame `at`, from the pieces handed over.
fn src_at(pieces: &VecDeque<Piece>, at: u64) -> Option<f64> {
    let p = pieces.iter().find(|p| at < p.out.1).or(pieces.back())?;
    if at >= p.out.1 {
        return Some(p.src.1);
    }
    let span = (p.out.1 - p.out.0).max(1) as f64;
    let f = (at.saturating_sub(p.out.0)) as f64 / span;
    Some(p.src.0 + (p.src.1 - p.src.0) * f)
}

// ----- one song -------------------------------------------------------------------------------

/// A seek's new decoder, from an opener, for the decode thread to take over.
struct Swap {
    dec: TrackDecoder,
    offset: f64,
    aim: Aim,
    play: bool,
}

/// A seek's new audio, waiting to be spliced in.
struct Cue {
    run: Run,
    aim: Aim,
    play: bool,
    since: Instant,
}

/// What a song's decode thread and the pump share.
#[derive(Default)]
struct Shared1 {
    run: Run,
    /// The song position just after the last frame decoded (s).
    decoded_to: f64,
    stop: bool,
    swap: Option<Swap>,
    cue: Option<Cue>,
}

/// A song the relay feeds the engine.
struct Feed {
    live: String,
    track: Track,
    server_made: bool,
    duration: Option<f64>,
    rate: u32,
    ch: usize,
    q: Arc<Mutex<Shared1>>,
    stream: Arc<LiveStream>,
    slicer: Slicer,
    /// The song position of the slicer's first input frame (None until it has one).
    base: Option<f64>,
    pushed: u64,
    pieces: VecDeque<Piece>,
    last_mark: Instant,
    /// The song position jumped (a start or a splice) until then: the engine's delay is not
    /// measured from a status that may be from before it.
    jumped_until: Instant,
    closed: bool,
    eof_at: Option<Instant>,
    born: Instant,
    registered: bool,
    /// A load waiting for its audio.
    start: Option<Start>,
    /// The seeks (request numbers) whose splice this song waits for.
    seek_asked: Vec<u64>,
}

/// A load waiting for its audio: where the group is, what the engine is told then, since when,
/// and its request number.
struct Start {
    aim: Aim,
    then: Then,
    since: Instant,
    asked: u64,
}

impl Feed {
    fn new(live: String, track: Track, opened: Opened) -> Feed {
        let Opened { dec, offset } = opened;
        let fmt = dec.pcm_input();
        let rate = fmt.rate.max(1);
        let ch = fmt.channels.max(1) as usize;
        let info = dec.info();
        let duration = track.duration.filter(|d| *d > 0.0).or(dec.duration());
        let stream = Arc::new(LiveStream::new(rate, ch as u32, fmt.bits, &info.codec, PREBUFFER_MS).unbounded());
        let q = Arc::new(Mutex::new(Shared1::default()));
        let server_made = server_made(&track);
        {
            let q = q.clone();
            let source = track.source.clone();
            let _ = std::thread::Builder::new()
                .name("encore-group-decode".into())
                .spawn(move || decode_loop(q, dec, offset, source, server_made, rate, ch));
        }
        Feed {
            live,
            track,
            server_made,
            duration,
            rate,
            ch,
            q,
            stream,
            slicer: Slicer::new(rate, ch as u32),
            base: None,
            pushed: 0,
            pieces: VecDeque::new(),
            last_mark: Instant::now(),
            jumped_until: Instant::now() + Duration::from_secs(1),
            closed: false,
            eof_at: None,
            born: Instant::now(),
            registered: false,
            start: None,
            seek_asked: Vec::new(),
        }
    }

    fn frames(&self, s: f64) -> usize {
        (f64::from(self.rate) * s).round() as usize
    }

    /// Hand the decoded audio to the engine's stream until it is [`LIVE_AHEAD_S`] ahead; the
    /// stream is ended after the song's last frame unless `hold` (the next song is not ready).
    fn top_up(&mut self, hold: bool) {
        let target = self.frames(LIVE_AHEAD_S);
        let reserve = self.frames(super::slice::MAX_SLICE_S + super::slice::XFADE_S) + 1;
        while !self.closed {
            let buffered = self.stream.buffered_frames();
            if buffered >= target {
                break;
            }
            let last = {
                let mut g = lock(&self.q);
                while self.slicer.held() < PIECE + reserve {
                    let Some(c) = g.run.chunks.pop_front() else { break };
                    g.run.frames -= c.pcm.len() / self.ch;
                    if self.base.is_none() {
                        self.base = Some(c.at - self.slicer.consumed() as f64 / f64::from(self.rate));
                    }
                    self.slicer.push(&c.pcm);
                }
                g.run.eof && g.run.chunks.is_empty() && g.cue.is_none() && g.swap.is_none()
            };
            let before = self.slicer.consumed();
            let out = self.slicer.pull(PIECE.min(target - buffered).max(1), last);
            if out.is_empty() {
                if last && self.slicer.held() == 0 {
                    let since = *self.eof_at.get_or_insert_with(Instant::now);
                    if !hold || since.elapsed() > NEXT_WAIT {
                        self.stream.close();
                        self.closed = true;
                    }
                }
                break;
            }
            let base = self.base.unwrap_or(0.0);
            let rate = f64::from(self.rate);
            let n = (out.len() / self.ch) as u64;
            self.pieces.push_back(Piece {
                out: (self.pushed, self.pushed + n),
                src: (base + before as f64 / rate, base + self.slicer.consumed() as f64 / rate),
            });
            self.pushed += n;
            self.stream.push(out);
        }
    }

    /// Hand over a stretch that is not through the slicer (a splice) as coming from `from` s.
    fn push_raw(&mut self, pcm: Vec<f64>, from: f64) {
        let n = (pcm.len() / self.ch) as u64;
        if n == 0 {
            return;
        }
        self.pieces.push_back(Piece {
            out: (self.pushed, self.pushed + n),
            src: (from, from + n as f64 / f64::from(self.rate)),
        });
        self.pushed += n;
        self.stream.push(pcm);
    }

    /// Tell the engine where in the song the frame it takes next is from.
    fn mark(&mut self) {
        self.last_mark = Instant::now();
        for _ in 0..3 {
            let b1 = self.stream.buffered_frames() as u64;
            let at = self.pushed.saturating_sub(b1);
            let Some(src) = src_at(&self.pieces, at) else { return };
            self.stream.mark_position(src);
            // the engine took a piece in between: once more
            if self.stream.buffered_frames() as u64 == b1 {
                break;
            }
        }
        // what the engine has taken is not needed any more
        let taken = self.pushed.saturating_sub(self.stream.buffered_frames() as u64);
        while self.pieces.len() > 1 && self.pieces.front().is_some_and(|p| p.out.1 <= taken) {
            self.pieces.pop_front();
        }
    }

    /// The decode position, to know when to open the next song.
    fn decoded_to(&self) -> f64 {
        lock(&self.q).decoded_to
    }

    fn at_eof(&self) -> bool {
        lock(&self.q).run.eof
    }

    /// The song position of the frame the engine takes next (from what was handed over).
    fn next_taken(&self) -> Option<f64> {
        src_at(&self.pieces, self.pushed.saturating_sub(self.stream.buffered_frames() as u64))
    }

    /// A seek's new audio, spliced in once the group is where it starts (`delay`: the engine's
    /// own delay from taking a frame to playing it, s). Returns whether the engine should play
    /// then.
    ///
    /// While the engine plays, the new audio goes in after what its stream holds, crossfaded. A
    /// paused engine (or a paused group's spot) takes nothing: what its stream holds would be heard
    /// first when it plays again (the old spot), and its position would stay there. So the stream
    /// is emptied and the new audio starts it, faded in from silence: the next frame the engine
    /// takes is the new spot, and its position says so.
    fn try_splice(&mut self, delay: f64, engine_playing: bool) -> Option<bool> {
        if self.closed {
            // the song had ended here already: the seek can't go in (the sync loads it again)
            let mut g = lock(&self.q);
            return g.cue.take().map(|_| false);
        }
        let mut g = lock(&self.q);
        let cue = g.cue.as_mut()?;
        let fresh = !engine_playing || !cue.aim.playing;
        // the new audio goes in after what the engine's stream holds (nothing when it starts the
        // stream afresh), and is heard the engine's delay after it takes it; a paused group's spot
        // is aimed past by that delay too, so it is the spot that is heard when the group plays
        let held = if fresh { 0.0 } else { self.stream.buffered_frames() as f64 / f64::from(self.rate) };
        let latency = held + delay;
        let desired = if cue.aim.playing { cue.aim.at(mono_ms() + (latency * 1000.0) as i64) } else { cue.aim.pos + delay };
        cue.run.trim_to(desired, self.rate, self.ch);
        let first = cue.run.first_at()?;
        let early = cue.aim.playing && first > desired + 0.002;
        if early && cue.since.elapsed() < EARLY_WAIT {
            return None;
        }
        let n = self.frames(SPLICE_S).max(2);
        if cue.run.frames < n && !cue.run.eof {
            return None;
        }
        let Cue { mut run, play, .. } = g.cue.take()?;
        // the old song's next frames: what the slicer holds, then what is decoded (none when the
        // stream starts afresh: the new audio fades in from silence)
        let mut old = self.slicer.take_held();
        if fresh {
            old.clear();
        } else if old.len() < n * self.ch {
            let more = g.run.take(n - old.len() / self.ch, self.rate, self.ch);
            old.extend(more);
        }
        let new = run.take(n, self.rate, self.ch);
        let joined = crossfade(&old, &new, self.ch);
        let after = first + (new.len() / self.ch) as f64 / f64::from(self.rate);
        g.run = run;
        g.decoded_to = g.run.chunks.back().map(|c| c.at + (c.pcm.len() / self.ch) as f64 / f64::from(self.rate)).unwrap_or(after);
        drop(g);
        let want = self.slicer.want();
        self.slicer = Slicer::new(self.rate, self.ch as u32);
        self.slicer.set_want(want);
        self.base = None;
        self.eof_at = None;
        self.jumped_until = Instant::now() + Duration::from_secs(1);
        if fresh {
            // nothing of the old spot stays: not in the stream, not in what the marks are read from
            self.stream.flush();
            self.pieces.clear();
        }
        self.push_raw(joined, first);
        if fresh {
            // the stream full again before the engine is told to play (it waits for a prebuffer
            // after the flush)
            self.top_up(true);
        }
        self.mark();
        log::info!(
            "group listen: spliced in at {first:.2} s ({}, wanted {desired:.2} s{})",
            if early { "early" } else { "on time" },
            if fresh { ", afresh" } else { "" }
        );
        Some(play)
    }

    /// A load's first audio, once it covers where the group is: handed to the engine's stream,
    /// registered and marked. Returns the load, for the engine to be told.
    fn try_start(&mut self) -> Option<Start> {
        let (aim, since) = self.start.as_ref().map(|s| (s.aim, s.since))?;
        let desired = if aim.playing { aim.at(mono_ms()) } else { aim.pos };
        {
            let mut g = lock(&self.q);
            g.run.trim_to(desired, self.rate, self.ch);
            let first = g.run.first_at();
            let enough = g.run.frames >= self.frames(LIVE_AHEAD_S) || g.run.eof;
            if !enough {
                return None;
            }
            if aim.playing && first.is_some_and(|f| f > desired + 0.002) && since.elapsed() < EARLY_WAIT {
                return None;
            }
        }
        let start = self.start.take()?;
        self.top_up(false);
        live::register(&self.live, self.stream.clone());
        self.registered = true;
        self.jumped_until = Instant::now() + Duration::from_secs(1);
        self.mark();
        Some(start)
    }
}

/// Read a song ahead, open it again where it broke off, take over a seek's new decoder.
fn decode_loop(q: Arc<Mutex<Shared1>>, mut dec: TrackDecoder, mut offset: f64, source: Source, server_made: bool, rate: u32, ch: usize) {
    let ahead = (f64::from(rate) * DECODE_AHEAD_S) as usize;
    let mut fails = 0u32;
    // after opening again where the stream broke off: what comes before that spot is dropped
    // (the stream opens at a packet at or before it), so nothing plays twice
    let mut skip_to: Option<f64> = None;
    loop {
        {
            let mut g = lock(&q);
            if g.stop {
                return;
            }
            if let Some(sw) = g.swap.take() {
                dec = sw.dec;
                offset = sw.offset;
                fails = 0;
                skip_to = None;
                g.cue = Some(Cue { run: Run::default(), aim: sw.aim, play: sw.play, since: Instant::now() });
            }
            let (frames, eof) = match &g.cue {
                Some(c) => (c.run.frames, c.run.eof),
                None => (g.run.frames, g.run.eof),
            };
            if eof || frames >= ahead {
                drop(g);
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        }
        let at = offset + dec.position_seconds();
        let got = dec.next_chunk();
        let mut g = lock(&q);
        if g.stop {
            return;
        }
        if g.swap.is_some() {
            // the old decoder's: the new one takes over at the top
            continue;
        }
        match got {
            Ok(Some(mut pcm)) => {
                fails = 0;
                let mut at = at;
                if let Some(to) = skip_to {
                    let n = pcm.len() / ch;
                    let cut = (((to - at) * f64::from(rate)).round().max(0.0) as usize).min(n);
                    pcm.drain(..cut * ch);
                    at += cut as f64 / f64::from(rate);
                    if cut < n {
                        skip_to = None;
                    }
                }
                let n = pcm.len() / ch;
                if n == 0 {
                    continue;
                }
                let end = at + n as f64 / f64::from(rate);
                match g.cue.as_mut() {
                    Some(c) => c.run.push(Chunk { at, pcm }, ch),
                    None => {
                        g.run.push(Chunk { at, pcm }, ch);
                        g.decoded_to = end;
                    }
                }
            }
            Ok(None) => match g.cue.as_mut() {
                Some(c) => c.run.eof = true,
                None => g.run.eof = true,
            },
            Err(e) => {
                drop(g);
                fails += 1;
                if fails > REOPENS {
                    log::warn!("group listen: the stream broke off at {at:.1} s and could not be opened again: {e}");
                    let mut g = lock(&q);
                    match g.cue.as_mut() {
                        Some(c) => c.run.eof = true,
                        None => g.run.eof = true,
                    }
                    continue;
                }
                log::warn!("group listen: the stream broke off at {at:.1} s ({e}); opening it again there");
                std::thread::sleep(Duration::from_millis(400 * u64::from(fails)));
                match open_at(&source, server_made, at) {
                    Ok(o) => {
                        dec = o.dec;
                        offset = o.offset;
                        skip_to = Some(at);
                    }
                    Err(e) => log::warn!("group listen: opening the stream again: {e}"),
                }
            }
        }
    }
}

/// An opened stream, positioned: its decoder and where its own time 0 is in the song.
struct Opened {
    dec: TrackDecoder,
    offset: f64,
}

/// Open `source` at `at` seconds: a seek in the file, or (a stream the server makes, which can't
/// seek back) the server's `t=`.
fn open_at(source: &Source, server_made: bool, at: f64) -> Result<Opened, String> {
    let at = at.max(0.0);
    if server_made && at >= 0.05 {
        if let Source::Url { url, headers } = source {
            // a tenth of a second at or before: the rest is trimmed exactly
            let t = (at * 10.0).floor() / 10.0;
            let s = Source::Url { url: with_t(url, t), headers: headers.clone() };
            let dec = TrackDecoder::open(&s).map_err(|e| e.to_string())?;
            return Ok(Opened { dec, offset: t });
        }
    }
    let mut dec = TrackDecoder::open(source).map_err(|e| e.to_string())?;
    if at > 0.0 {
        dec.seek(at).map_err(|e| e.to_string())?;
    }
    Ok(Opened { dec, offset: 0.0 })
}

// ----- the relay ------------------------------------------------------------------------------

/// What the engine is told once a load's audio is ready.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// Replace the engine's queue with these (relay) entries and play the first.
    Queue { tracks: Vec<Track>, play: bool },
    /// Play this engine entry.
    Index { index: usize, play: bool },
}

impl Then {
    fn play(&self) -> bool {
        match self {
            Then::Queue { play, .. } | Then::Index { play, .. } => *play,
        }
    }
}

enum Cmd {
    Load { live: String, aim: Aim, then: Then },
    Seek { live: Option<String>, index: usize, aim: Aim, play: bool },
    Correct(f64),
}

/// Why an opener ran.
enum Purpose {
    Load { aim: Aim, then: Then, asked: u64 },
    Seek { aim: Aim, play: bool, asked: u64 },
    Next,
}

struct Done {
    live: String,
    track: Track,
    purpose: Purpose,
    took_ms: i64,
    result: Result<Opened, String>,
}

/// What the player's side and the pump share.
#[derive(Default)]
struct Shared {
    /// Every group entry the engine may be given: relay id → the real song.
    tracks: Mutex<HashMap<String, Track>>,
    /// Loads and seeks asked for (the player's side) and taken in (the pump).
    asked: AtomicU64,
    taken: AtomicU64,
    /// A load or seek taken in is under way.
    busy: AtomicBool,
    /// The song playing is a stream the server makes.
    server_made: AtomicBool,
    /// When the relay last paused the engine itself (monotonic ms).
    paused_at: AtomicI64,
    stop: AtomicBool,
}

/// The handle the group sync uses.
pub struct Relay {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
}

impl Relay {
    /// Start the pump for `engine`.
    pub fn start(engine: Engine) -> Relay {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        let _ = std::thread::Builder::new()
            .name("encore-group-relay".into())
            .spawn(move || Pump::new(engine, rx, s).run());
        Relay { tx, shared }
    }

    /// Group entries the engine may be given (relay id → the real song).
    pub fn add(&self, list: Vec<(String, Track)>) {
        lock(&self.shared.tracks).extend(list);
    }

    /// Open `live` where `aim` says, then tell the engine `then`.
    pub fn load(&self, live: String, aim: Aim, then: Then) {
        self.shared.asked.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Load { live, aim, then });
    }

    /// Seek in the song playing (`live`, the engine's entry `index`), spliced in; then play when
    /// `play`.
    pub fn seek(&self, live: Option<String>, index: usize, aim: Aim, play: bool) {
        self.shared.asked.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Seek { live, index, aim, play });
    }

    /// Close `drift_ms` (this computer − the group) with slices; 0 stops.
    pub fn correct(&self, drift_ms: i64) {
        let _ = self.tx.send(Cmd::Correct(-(drift_ms as f64) / 1000.0));
    }

    /// A load or seek is under way.
    pub fn busy(&self) -> bool {
        self.shared.asked.load(Ordering::SeqCst) > self.shared.taken.load(Ordering::SeqCst)
            || self.shared.busy.load(Ordering::SeqCst)
    }

    /// The song playing is a stream the server makes.
    pub fn server_made(&self) -> bool {
        self.shared.server_made.load(Ordering::SeqCst)
    }

    /// When the relay last paused the engine (monotonic ms; 0 never).
    pub fn paused_at(&self) -> i64 {
        self.shared.paused_at.load(Ordering::SeqCst)
    }

    /// Out of the group: the engine's queue gets the real songs back (paused where it was), then
    /// the relay stops.
    pub fn leave(&self, engine: &Engine, st: &Status) {
        let tracks = lock(&self.shared.tracks).clone();
        if st.queue.iter().any(|t| live_of(t).is_some()) {
            let queue: Vec<Track> = st
                .queue
                .iter()
                .map(|t| live_of(t).and_then(|l| tracks.get(l).cloned()).unwrap_or_else(|| t.clone()))
                .filter(|t| live_of(t).is_none())
                .collect();
            if queue.is_empty() {
                engine.send(Command::Stop);
            } else {
                let index = st.index.unwrap_or(0).min(queue.len() - 1);
                engine.play_queue(queue, index, st.position);
                engine.send(Command::Pause);
            }
        }
        self.stop();
    }

    /// Stop the pump and every song's thread (the streams go out of the registry).
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Pump {
    engine: Engine,
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
    /// The songs fed to the engine (the one playing, the next).
    feeds: HashMap<String, Feed>,
    /// A load whose audio is coming (not given to the engine yet).
    starting: Option<Feed>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    /// Next songs being opened, and those that failed to open (tried again after a moment).
    opening: HashMap<String, Instant>,
    failed: HashMap<String, Instant>,
    /// What is left to correct with slices (s; > 0 drop).
    want: f64,
    /// Loads and seeks under way (request number → when asked).
    asked: u64,
    pending: HashMap<u64, Instant>,
    /// The newest load: an older one that opens later is dropped.
    newest_load: u64,
    /// How long opening at a spot took (ms), whole files and server-made apart.
    open_ms: [Vec<i64>; 2],
    /// The engine's delay from taking a frame to playing it (s): its chain and its device's
    /// buffer, measured from its status (the song position it reports against the frame it
    /// takes next), smoothed.
    delay: Option<f64>,
    last_status: Option<Arc<Status>>,
}

impl Pump {
    fn new(engine: Engine, rx: Receiver<Cmd>, shared: Arc<Shared>) -> Pump {
        let (done_tx, done_rx) = mpsc::channel();
        Pump {
            engine,
            rx,
            shared,
            feeds: HashMap::new(),
            starting: None,
            done_tx,
            done_rx,
            opening: HashMap::new(),
            failed: HashMap::new(),
            want: 0.0,
            asked: 0,
            pending: HashMap::new(),
            newest_load: 0,
            open_ms: [Vec::new(), Vec::new()],
            delay: None,
            last_status: None,
        }
    }

    fn run(mut self) {
        while !self.shared.stop.load(Ordering::SeqCst) {
            loop {
                match self.rx.try_recv() {
                    Ok(c) => {
                        if !matches!(c, Cmd::Correct(_)) {
                            self.shared.taken.fetch_add(1, Ordering::SeqCst);
                        }
                        self.command(c);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.shutdown();
                        return;
                    }
                }
            }
            while let Ok(d) = self.done_rx.try_recv() {
                self.opened(d);
            }
            self.tick();
            std::thread::sleep(TICK);
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(f) = self.starting.take() {
            lock(&f.q).stop = true;
        }
        for (id, f) in self.feeds.drain() {
            lock(&f.q).stop = true;
            if f.registered {
                live::unregister(&id);
            }
        }
    }

    fn ask(&mut self) -> u64 {
        self.asked += 1;
        self.pending.insert(self.asked, Instant::now());
        self.asked
    }

    fn finish(&mut self, n: u64) {
        self.pending.remove(&n);
    }

    /// How long opening at a spot has been taking (ms): the median of the last 5.
    fn open_estimate(&self, server_made: bool) -> i64 {
        let v = &self.open_ms[usize::from(server_made)];
        if v.is_empty() {
            return if server_made { 1500 } else { 600 };
        }
        let mut s = v.clone();
        s.sort_unstable();
        s[s.len() / 2]
    }

    /// The real song for an id, with the stream query of now (a change of quality waits for the
    /// next song, it never changes the one playing).
    fn real(&self, live: &str) -> Option<Track> {
        let mut t = lock(&self.shared.tracks).get(live).cloned()?;
        crate::engine::tracks::refresh_query(&mut t);
        Some(t)
    }

    fn spawn_open(&mut self, live: String, track: Track, at: f64, purpose: Purpose) {
        if matches!(purpose, Purpose::Next) {
            self.opening.insert(live.clone(), Instant::now());
        }
        let tx = self.done_tx.clone();
        let made = server_made(&track);
        let _ = std::thread::Builder::new().name("encore-group-open".into()).spawn(move || {
            let t0 = Instant::now();
            let result = open_at(&track.source, made, at);
            let took_ms = t0.elapsed().as_millis() as i64;
            let _ = tx.send(Done { live, track, purpose, took_ms, result });
        });
    }

    fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Correct(s) => self.want = s,
            Cmd::Load { live, aim, then } => {
                // a newer load replaces one whose audio is still coming
                if let Some(old) = self.starting.take() {
                    lock(&old.q).stop = true;
                    if let Some(st) = old.start {
                        self.finish(st.asked);
                    }
                }
                let Some(track) = self.real(&live) else {
                    log::warn!("group listen: no song for {live}");
                    return;
                };
                let asked = self.ask();
                self.newest_load = asked;
                // aimed past where the group will be once the stream is open: the rest is waited
                let at = if aim.playing { aim.at(mono_ms() + self.open_estimate(server_made(&track)) + 200) } else { aim.pos };
                log::info!("group listen: opening {} at {at:.1} s", track.title);
                self.spawn_open(live, track, at, Purpose::Load { aim, then, asked });
            }
            Cmd::Seek { live, index, aim, play } => {
                let feed = live.as_ref().and_then(|l| self.feeds.get(l)).filter(|f| f.registered && !f.closed);
                match feed {
                    Some(f) => {
                        let (live, track, made) = (f.live.clone(), f.track.clone(), f.server_made);
                        let asked = self.ask();
                        let at = if aim.playing { aim.at(mono_ms() + self.open_estimate(made) + 200) } else { aim.pos };
                        log::info!("group listen: seeking to {at:.1} s");
                        self.spawn_open(live, track, at, Purpose::Seek { aim, play, asked });
                    }
                    // not one of the relay's songs (yet), or one that ended here: load it where
                    // the group is
                    None => {
                        let Some(live) = live else { return };
                        self.command(Cmd::Load { live, aim, then: Then::Index { index, play } });
                    }
                }
            }
        }
    }

    fn opened(&mut self, d: Done) {
        if matches!(d.purpose, Purpose::Next) {
            self.opening.remove(&d.live);
        }
        let made = server_made(&d.track);
        let asked = match &d.purpose {
            Purpose::Load { asked, .. } | Purpose::Seek { asked, .. } => Some(*asked),
            Purpose::Next => None,
        };
        let opened = match d.result {
            Ok(o) => o,
            Err(e) => {
                log::warn!("group listen: could not open {}: {e}", d.track.title);
                match asked {
                    Some(n) => self.finish(n),
                    None => {
                        self.failed.insert(d.live, Instant::now());
                    }
                }
                return;
            }
        };
        if asked.is_some() {
            let v = &mut self.open_ms[usize::from(made)];
            v.push(d.took_ms);
            if v.len() > 5 {
                v.remove(0);
            }
        }
        match d.purpose {
            Purpose::Next => {
                if self.feeds.contains_key(&d.live) {
                    return;
                }
                let mut f = Feed::new(d.live.clone(), d.track, opened);
                live::register(&f.live, f.stream.clone());
                f.registered = true;
                log::info!("group listen: the next song is ready ({})", f.track.title);
                self.feeds.insert(d.live, f);
            }
            Purpose::Load { aim, then, asked } => {
                if asked != self.newest_load {
                    // a newer load came meanwhile
                    self.finish(asked);
                    return;
                }
                let mut f = Feed::new(d.live, d.track, opened);
                f.start = Some(Start { aim, then, since: Instant::now(), asked });
                self.starting = Some(f);
            }
            Purpose::Seek { aim, play, asked } => match self.feeds.get_mut(&d.live) {
                Some(f) => {
                    lock(&f.q).swap = Some(Swap { dec: opened.dec, offset: opened.offset, aim, play });
                    f.seek_asked.push(asked);
                }
                None => self.finish(asked),
            },
        }
    }

    fn tick(&mut self) {
        let st = self.engine.status();
        let cur = st.track.as_ref().and_then(live_of).map(str::to_string);
        let next = st.index.and_then(|i| st.queue.get(i + 1)).and_then(live_of).map(str::to_string);
        // a load whose audio is ready: the engine is told
        if let Some(start) = self.starting.as_mut().and_then(Feed::try_start) {
            let f = self.starting.take().expect("checked above");
            log::info!("group listen: {} starts at {:.2} s", f.track.title, start.aim.at(mono_ms()));
            let play = start.then.play();
            // the same song loaded again: the engine takes the new stream when told below
            if let Some(old) = self.feeds.remove(&f.live) {
                lock(&old.q).stop = true;
            }
            self.feeds.insert(f.live.clone(), f);
            match start.then {
                Then::Queue { tracks, .. } => self.engine.play_queue(tracks, 0, 0.0),
                Then::Index { index, .. } => self.engine.send(Command::PlayIndex(index)),
            }
            if !play {
                self.shared.paused_at.store(mono_ms(), Ordering::SeqCst);
                self.engine.send(Command::Pause);
            }
            self.finish(start.asked);
        }
        // a new status of the song playing: the engine's delay to the speakers
        let new_status = !self.last_status.as_ref().is_some_and(|l| Arc::ptr_eq(l, &st));
        if new_status && st.state == PlayState::Playing {
            if let Some(f) = cur.as_ref().and_then(|c| self.feeds.get(c)).filter(|f| Instant::now() >= f.jumped_until) {
                if let Some(next) = f.next_taken() {
                    let d = next - st.position;
                    if (0.0..2.0).contains(&d) {
                        self.delay = Some(self.delay.map_or(d, |x| x * 0.8 + d * 0.2));
                    }
                }
            }
        }
        self.last_status = Some(st.clone());
        // seeks ready to splice
        let delay = self.delay.unwrap_or(0.0);
        let engine_playing = st.state == PlayState::Playing;
        let (mut plays, mut spliced) = (false, false);
        let mut done = Vec::new();
        for f in self.feeds.values_mut() {
            if let Some(play) = f.try_splice(delay, engine_playing) {
                plays |= play;
                spliced = true;
                done.append(&mut f.seek_asked);
            }
        }
        for n in done {
            self.finish(n);
        }
        if plays && !engine_playing {
            self.engine.send(Command::Play);
        } else if spliced && !engine_playing {
            // a paused engine publishes its status only when told something: told to stay paused,
            // its status shows the new spot at once (the sync does not seek again for a spot it
            // reached)
            self.shared.paused_at.store(mono_ms(), Ordering::SeqCst);
            self.engine.send(Command::Pause);
        }
        // the song playing: corrections, audio, marks
        let next_ready = next.as_ref().is_some_and(|n| self.feeds.get(n).is_some_and(|f| f.registered));
        let mut open_next = false;
        if let Some(f) = cur.as_ref().and_then(|c| self.feeds.get_mut(c)) {
            let rate = f64::from(f.rate);
            if (f.slicer.want() - self.want * rate).abs() > 0.5 {
                f.slicer.set_want(self.want * rate);
            }
            f.top_up(next.is_some() && !next_ready);
            self.want = f.slicer.want() / rate;
            if f.last_mark.elapsed() >= MARK_EVERY {
                f.mark();
            }
            self.shared.server_made.store(f.server_made, Ordering::SeqCst);
            open_next = f.at_eof() || f.duration.is_none_or(|d| f.decoded_to() + NEXT_BEFORE_S >= d);
        }
        // the next song: opened ahead and its first audio handed over
        if let Some(n) = next.as_ref() {
            match self.feeds.get_mut(n) {
                Some(f) => {
                    if Some(n) != cur.as_ref() {
                        f.top_up(false);
                    }
                }
                None => {
                    let retry = self.failed.get(n).is_none_or(|t| t.elapsed() > Duration::from_secs(2));
                    if open_next && retry && !self.opening.contains_key(n) {
                        if let Some(track) = self.real(n) {
                            self.failed.remove(n);
                            self.spawn_open(n.clone(), track, 0.0, Purpose::Next);
                        }
                    }
                }
            }
        }
        // songs the engine is done with
        let gone: Vec<String> = self
            .feeds
            .iter()
            .filter(|(k, f)| Some(*k) != cur.as_ref() && Some(*k) != next.as_ref() && f.born.elapsed() >= GRACE)
            .map(|(k, _)| k.clone())
            .collect();
        for id in gone {
            if let Some(f) = self.feeds.remove(&id) {
                lock(&f.q).stop = true;
                if f.registered {
                    live::unregister(&id);
                }
                for n in f.seek_asked {
                    self.pending.remove(&n);
                }
            }
        }
        // requests that never finish are given up
        self.pending.retain(|_, t| t.elapsed() < GIVE_UP);
        self.shared.busy.store(!self.pending.is_empty(), Ordering::SeqCst);
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod relay_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(at: f64, frames: usize, v: f64) -> Chunk {
        Chunk { at, pcm: vec![v; frames * 2] }
    }

    #[test]
    fn a_server_made_stream_is_opened_again_with_t() {
        assert_eq!(with_t("http://s/api/library/tracks/3/stream?q=aac&kbps=96", 61.25), "http://s/api/library/tracks/3/stream?q=aac&kbps=96&t=61.2");
        assert_eq!(with_t("http://s/x/stream?t=4.0&dsp=1", 7.0), "http://s/x/stream?dsp=1&t=7.0");
        assert_eq!(with_t("http://s/x/stream", 1.0), "http://s/x/stream?t=1.0");
        let t = Track {
            source: Source::Url { url: "http://s/api/library/tracks/3/stream?q=aac&kbps=96".into(), headers: vec![] },
            ..Default::default()
        };
        assert!(server_made(&t));
        let lt = live_track(&t, "abc");
        assert_eq!(live_of(&lt), Some("encore-group/abc"));
        assert_eq!((lt.title.as_str(), live_of(&t)), ("", None));
    }

    #[test]
    fn decoded_audio_is_trimmed_to_the_frame_and_taken_in_order() {
        let mut r = Run::default();
        r.push(chunk(10.0, 100, 1.0), 2);
        r.push(chunk(10.0 + 100.0 / 1000.0, 100, 2.0), 2);
        assert_eq!(r.frames, 200);
        // to 10.05 s at 1 kHz: 50 frames go
        r.trim_to(10.05, 1000, 2);
        assert_eq!(r.frames, 150);
        assert!((r.first_at().unwrap() - 10.05).abs() < 1e-9);
        // a spot past the whole first chunk: it goes
        r.trim_to(10.12, 1000, 2);
        assert_eq!(r.frames, 80);
        let got = r.take(30, 1000, 2);
        assert_eq!(got, vec![2.0; 60]);
        assert!((r.first_at().unwrap() - 10.15).abs() < 1e-9);
        assert_eq!(r.frames, 50);
        // more than there is: what there is
        assert_eq!(r.take(500, 1000, 2).len(), 100);
        assert_eq!(r.frames, 0);
    }

    #[test]
    fn a_splice_crossfades_and_never_steps() {
        // from a held level of 1.0 into -1.0: every frame between, no frame jumps more than the
        // crossfade allows
        let old = vec![1.0; 40];
        let new = vec![-1.0; 40];
        let x = crossfade(&old, &new, 2);
        assert_eq!(x.len(), 40);
        assert!(x[0] > 0.9 && x[39] < -0.9);
        let steps = x.chunks(2).zip(x.chunks(2).skip(1)).map(|(a, b)| (b[0] - a[0]).abs()).fold(0.0, f64::max);
        assert!(steps < 0.2, "{steps}");
        // nothing old (the stream had run dry): it fades in from silence
        let x = crossfade(&[], &new, 2);
        assert!(x[0].abs() < 0.01 && x[39] < -0.9);
    }

    #[test]
    fn the_engine_is_told_the_song_position_of_the_frame_it_takes_next() {
        let mut p = VecDeque::new();
        p.push_back(Piece { out: (0, 100), src: (5.0, 5.1) });
        // a slice dropped 2 ms in the second piece: 100 frames out carry 102 ms of song
        p.push_back(Piece { out: (100, 200), src: (5.1, 5.202) });
        // a splice: the third piece comes from 60 s
        p.push_back(Piece { out: (200, 300), src: (60.0, 60.1) });
        assert_eq!(src_at(&p, 0), Some(5.0));
        assert!((src_at(&p, 50).unwrap() - 5.05).abs() < 1e-9);
        assert!((src_at(&p, 150).unwrap() - 5.151).abs() < 1e-9);
        assert_eq!(src_at(&p, 200), Some(60.0));
        // past what was handed over (the engine ran dry): the end of it
        assert_eq!(src_at(&p, 400), Some(60.1));
        assert_eq!(src_at(&VecDeque::new(), 0), None);
    }
}
