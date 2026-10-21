//! The engine follows the group (p6 item 8, client-shapes §5.6): the group's queue, song,
//! position and play / pause on this computer's Cadenza.
//!
//! The sync rule (the same on web, phone and desktop, Oct 21): drift = this computer's position −
//! the group's (position + (server_now − anchor_ms) / 1000, through the measured clock offset).
//! - under 40 ms: nothing;
//! - 40 ms to 1.5 s: closed gently, never with a seek, until it is under 20 ms. The web and the
//!   phone nudge the playback speed (0.98–1.02); Cadenza has no rate control, so the desktop drops
//!   or repeats tiny slices of samples instead, at most 2 ms per 100 ms of audio and crossfaded
//!   (`slice.rs`, done in the relay on the way into the engine);
//! - over 1.5 s: one seek, aimed ahead by what this computer's seeks have needed (the median of
//!   the last 5, [`SeekLag`]), then no other seek for 8 s (slices only). A stream the server makes
//!   (Data saver, the server's Cadenza) is never seeked for a drift under 3 s.
//!
//! A song change within 3 s of where the group changes song is left to the engine, which runs
//! gaplessly into the next song (the relay has it buffered already); the drift across the change
//! is closed with slices like any other.
//!
//! The engine's queue holds a run of the group's queue (from the entry it was loaded at), so a
//! song added, removed or moved around the one playing changes the engine's queue without a gap
//! in the music: see [`plan`]. Crossfade does not exist in Cadenza; gapless stays.
//!
//! Everything here is pure (tested below); `mod.rs` turns the [`Act`]s into engine and relay
//! commands.

use crate::engine::PlayState;

/// Under this drift (ms) nothing is done …
pub const NUDGE_FROM_MS: i64 = 40;
/// … and a correction that runs goes on until the drift is under this (ms).
pub const NUDGE_UNTIL_MS: i64 = 20;
/// Over this drift (ms) a seek is made.
pub const SEEK_MS: i64 = 1500;
/// A stream the server makes is seeked only over this drift (ms).
pub const SERVER_MADE_SEEK_MS: i64 = 3000;
/// After a seek for drift, no other seek for this long (ms): slices only.
pub const SEEK_PAUSE_MS: i64 = 8000;
/// A paused group puts everyone at its spot when they are further off than this (ms), once for
/// each spot (as the web and the phone). Playing again follows the drift rule: a seek only over
/// 1.5 s (3 s for a stream the server makes), else play and close the rest with slices.
pub const PAUSED_SEEK_MS: i64 = 500;
/// A song change this close to the group's (ms) is left to the engine (gapless).
pub const SONG_CHANGE_MS: i64 = 3000;
/// The longest a seek is aimed ahead (ms).
const MAX_LAG_MS: i64 = 3000;
/// The seek lag is the median of this many seeks.
const LAG_SEEKS: usize = 5;
/// After this computer moved the engine, no drift check for this long (a seek needs a moment
/// to play and its status to come).
pub const QUIET_MS: i64 = 2500;
/// How long the engine may take to open the song it was asked to play (a stream over a slow
/// network) before another look decides again.
pub const SETTLE_MS: i64 = 5000;
/// The engine's status is older than this (ms) while playing: its position is not known well
/// enough to measure a drift (a guess would invent one).
pub const STALE_MS: i64 = 1500;
/// A song of the group the engine can't play here (it skips it: a broken file, a format it can't
/// read) is loaded this many times, then left alone until the group moves on.
pub const MAX_TRIES: u32 = 3;

fn median(v: &[i64]) -> i64 {
    if v.is_empty() {
        return 0;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2
    }
}

/// What a seek made to catch up with the group needs on this computer: learnt from where each
/// seek lands (the median of the last 5, for whole files and server-made streams apart, 0 … 3 s)
/// and added to the next seek's target. The relay itself waits out the time a seek takes to open
/// the stream, so what is learnt here is mostly the engine's own delay to the speakers.
#[derive(Clone, Debug, Default)]
pub struct SeekLag {
    seen: [Vec<i64>; 2],
    /// The kind of stream, the song and the lag of the seek whose result is awaited.
    wait: Option<(usize, String, i64)>,
}

impl SeekLag {
    /// A seek's result is awaited: the next look while playing calls [`SeekLag::landed`].
    pub fn waiting(&self) -> bool {
        self.wait.is_some()
    }

    /// The lag (ms) to aim a seek in `song` (the queue entry's id) ahead by.
    pub fn target(&mut self, song: &str, server_made: bool) -> i64 {
        let k = usize::from(server_made);
        let lag = median(&self.seen[k]);
        self.wait = Some((k, song.to_string(), lag));
        lag
    }

    /// The first look while playing after a seek: `drift_ms` = this computer's position − the
    /// group's in `song`. A result in another song teaches nothing.
    pub fn landed(&mut self, song: &str, drift_ms: i64) {
        let Some((k, s, used)) = self.wait.take() else {
            return;
        };
        if s != song {
            return;
        }
        let seen = &mut self.seen[k];
        seen.push((used - drift_ms).clamp(0, MAX_LAG_MS));
        if seen.len() > LAG_SEEKS {
            seen.remove(0);
        }
    }

    /// The seek awaited is not one to learn from (a pause, leaving the group).
    pub fn cancel(&mut self) {
        self.wait = None;
    }

    /// The lag learnt for a kind of stream (ms).
    #[cfg(test)]
    pub fn lag_ms(&self, server_made: bool) -> i64 {
        median(&self.seen[usize::from(server_made)])
    }
}

/// Where the group is, for a seek or a load that takes a moment: the group's position `pos` at
/// `at_ms` (monotonic), running on while `playing`, and `lag_ms` to aim ahead by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    pub pos: f64,
    pub at_ms: i64,
    pub playing: bool,
    pub lag_ms: i64,
}

impl Aim {
    /// Where the first frame the engine takes at `now_ms` should come from (s).
    pub fn at(&self, now_ms: i64) -> f64 {
        if self.playing {
            self.pos + ((now_ms - self.at_ms).max(0) + self.lag_ms) as f64 / 1000.0
        } else {
            self.pos
        }
    }
}

/// How the engine's queue becomes the group's.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// The engine's queue is a run of the group's: its entry `li` is the group's `offset + li`.
    /// `append` are the group's entries after that run (indices into the group's queue).
    Aligned { offset: usize, append: Vec<usize> },
    /// The song the engine plays (its entry `keep`) is the group's current one: the rest of the
    /// engine's queue goes and the group's entries after the current one (`append`) follow it,
    /// without a gap in the music.
    Around { keep: usize, append: Vec<usize> },
    /// The group's queue from its current entry replaces the engine's.
    Full,
}

/// How to bring the engine's queue (`local`: library ids, None for files) with its current
/// entry `li` to the group's (`group` ids, current entry `ei`).
pub fn plan(group: &[i64], ei: usize, local: &[Option<i64>], li: Option<usize>) -> Plan {
    let n = local.len();
    if n > 0 && n <= group.len() && local.iter().all(Option::is_some) {
        let fits = |o: usize| (0..n).all(|k| local[k] == Some(group[o + k]));
        let offsets: Vec<usize> = (0..=group.len() - n).filter(|o| *o <= ei && fits(*o)).collect();
        // with a song twice in the queue: the run whose current entry is the group's
        let chosen = offsets
            .iter()
            .copied()
            .find(|o| li.is_some_and(|l| o + l == ei))
            .or_else(|| offsets.first().copied());
        if let Some(o) = chosen {
            if ei < o + n {
                return Plan::Aligned {
                    offset: o,
                    append: (o + n..group.len()).collect(),
                };
            }
        }
    }
    if let Some(l) = li.filter(|l| *l < n) {
        if local[l] == group.get(ei).copied() {
            return Plan::Around {
                keep: l,
                append: (ei + 1..group.len()).collect(),
            };
        }
    }
    Plan::Full
}

/// What the engine is doing, as its last status said.
#[derive(Clone, Debug)]
pub struct Local {
    /// The library id of each entry (None for files and the test sound).
    pub ids: Vec<Option<i64>>,
    pub index: Option<usize>,
    pub state: PlayState,
    /// The position now (the status's, moved on by the time since it came while playing).
    pub position: f64,
    /// The status is recent enough to measure a drift from (see [`STALE_MS`]).
    pub fresh: bool,
    /// The current entry is a server-made stream (a transcode, Data saver, the server's Cadenza).
    pub server_made: bool,
    /// The relay is opening a song or a seek for the engine: wait for it.
    pub busy: bool,
}

/// One thing to do to the engine. Indices of the group's queue are named `group`, the engine's
/// `local`.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Append these group entries.
    Enqueue(Vec<usize>),
    /// Remove this engine entry.
    Remove(usize),
    /// Load the group's queue from entry `from` and start where `aim` says; paused unless `play`.
    Full { from: usize, aim: Aim, play: bool },
    /// Play this engine entry from where `aim` says; paused unless `play`.
    PlayAt { local: usize, aim: Aim, play: bool },
    /// Seek in the song playing (spliced in by the relay); then play when `play`.
    Seek { aim: Aim, play: bool },
    /// Close this drift (ms, this computer − the group) with slices; 0 stops.
    Nudge(i64),
    Play,
    Pause,
}

/// What the group wants, at this moment.
#[derive(Clone, Debug, Default)]
pub struct Want {
    /// The group's queue (library ids).
    pub ids: Vec<i64>,
    /// The length of each of the group's entries (s; 0 when not known).
    pub durs: Vec<f64>,
    /// The current entry's `qid` (what the seek lag is learnt for).
    pub qid: String,
    /// Where the group is: its entry and the position in it.
    pub index: usize,
    pub position: f64,
    /// The group plays.
    pub group_playing: bool,
    /// The group plays and this computer is not "Paused here".
    pub play: bool,
    /// The group's current entry would play here as a server-made stream.
    pub server_made: bool,
}

/// What this computer remembers between looks.
#[derive(Clone, Debug, Default)]
pub struct Mem {
    /// No drift check until then (monotonic ms).
    pub quiet_until: i64,
    /// No seek for drift until then (monotonic ms): slices only.
    pub no_seek_until: i64,
    /// A correction with slices runs (until the drift is under 20 ms).
    pub nudging: bool,
    /// The engine's queue we asked for and until when to wait for it (its status lags a moment).
    pub pending: Option<(Vec<i64>, i64)>,
    /// When this computer last paused the engine itself (an engine pause that is not one of ours
    /// is the listener's: "Paused here").
    pub own_pause_at: Option<i64>,
    pub lag: SeekLag,
    /// The engine entry it was asked to play and until when to wait for it to be on it.
    pub settling: Option<(usize, i64)>,
    /// The group's entry (`qid`) loaded here, and how many times.
    pub tries: (String, u32),
    /// The paused group's spot (`qid`, ms) this computer was last put at: not again for the same.
    placed: Option<(String, i64)>,
    /// The relay was busy at the last look.
    was_busy: bool,
}

impl Mem {
    pub fn reset(&mut self) {
        *self = Mem::default();
    }

    fn quiet(&mut self, now_ms: i64, ms: i64) {
        self.quiet_until = self.quiet_until.max(now_ms + ms);
    }

    /// Whether the engine paused within the last 2 s because this computer asked it to.
    pub fn paused_by_us(&self, now_ms: i64) -> bool {
        self.own_pause_at.is_some_and(|t| now_ms - t < 2000)
    }

    /// One more load of the group's entry `qid`; false when it was loaded [`MAX_TRIES`] times
    /// already (the engine can't play it here).
    fn try_load(&mut self, qid: &str) -> bool {
        if self.tries.0 != qid {
            self.tries = (qid.to_string(), 0);
        }
        if self.tries.1 >= MAX_TRIES {
            return false;
        }
        self.tries.1 += 1;
        true
    }

    /// Stop a correction with slices that runs.
    fn calm(&mut self, out: &mut Vec<Act>) {
        if self.nudging {
            self.nudging = false;
            out.push(Act::Nudge(0));
        }
    }

    /// The slices for a drift: start over 40 ms, go on until under 20 ms.
    fn nudge(&mut self, drift_ms: i64, out: &mut Vec<Act>) {
        let d = drift_ms.abs();
        if (self.nudging && d >= NUDGE_UNTIL_MS) || d >= NUDGE_FROM_MS {
            self.nudging = true;
            out.push(Act::Nudge(drift_ms));
        } else {
            self.calm(out);
        }
    }
}

/// The drift (ms) across a song change: the engine on the song after the group's (ahead) or on
/// the one before (behind), from the songs' lengths. None when a length is not known.
fn across(w: &Want, t: usize, l: &Local) -> Option<i64> {
    let li = l.index?;
    let dur = |g: usize| w.durs.get(g).copied().filter(|d| *d > 0.0);
    if li == t + 1 {
        // ahead: the rest of the group's song, plus where this computer is in the next one
        let d = dur(w.index)?;
        Some(((d - w.position).max(0.0) * 1000.0 + l.position * 1000.0).round() as i64)
    } else if li + 1 == t {
        // behind: the rest of this computer's song, plus where the group is in its next one
        let d = dur(w.index.checked_sub(1)?)?;
        Some(-(((d - l.position).max(0.0) + w.position) * 1000.0).round() as i64)
    } else {
        None
    }
}

/// One look: what to do so the engine follows the group. `now_ms` is monotonic.
pub fn decide(w: &Want, l: &Local, mem: &mut Mem, now_ms: i64) -> Vec<Act> {
    let mut out = Vec::new();
    if l.busy {
        // the relay opens a song or a seek: the engine is not where it will be yet (a pause of
        // the group still pauses here at once)
        mem.was_busy = true;
        if !w.play && l.state == PlayState::Playing {
            mem.calm(&mut out);
            mem.lag.cancel();
            mem.own_pause_at = Some(now_ms);
            out.push(Act::Pause);
        }
        return out;
    }
    if mem.was_busy {
        // done: give the engine's status a moment to say where it is now
        mem.was_busy = false;
        mem.quiet(now_ms, 1000);
    }
    if let Some((ids, until)) = mem.pending.as_ref() {
        let have: Vec<Option<i64>> = ids.iter().copied().map(Some).collect();
        if have != l.ids && now_ms < *until {
            // the engine has not taken the queue we gave it yet
            return out;
        }
        mem.pending = None;
    }
    if let Some((idx, until)) = mem.settling {
        if l.index != Some(idx) && now_ms < until {
            // the engine is still opening the song it was asked to play
            return out;
        }
        mem.settling = None;
    }
    if w.ids.is_empty() {
        mem.calm(&mut out);
        if l.state == PlayState::Playing {
            mem.own_pause_at = Some(now_ms);
            out.push(Act::Pause);
        }
        return out;
    }
    let ei = w.index.min(w.ids.len() - 1);
    // where the group is, for a seek or a load (aimed ahead by the lag while it plays)
    let aim = |mem: &mut Mem, server_made: bool, playing: bool| -> Aim {
        let lag_ms = if playing { mem.lag.target(&w.qid, server_made) } else { 0 };
        Aim { pos: w.position, at_ms: now_ms, playing, lag_ms }
    };
    let offset = match plan(&w.ids, ei, &l.ids, l.index) {
        Plan::Full => {
            mem.calm(&mut out);
            if !mem.try_load(&w.qid) {
                // the engine can't play this song here: quiet until the group moves on
                if l.state == PlayState::Playing {
                    mem.own_pause_at = Some(now_ms);
                    out.push(Act::Pause);
                }
                return out;
            }
            let a = aim(mem, w.server_made, w.play);
            if !w.play {
                mem.lag.cancel();
                mem.own_pause_at = Some(now_ms);
            }
            out.push(Act::Full { from: ei, aim: a, play: w.play });
            mem.pending = Some((w.ids[ei..].to_vec(), now_ms + 3000));
            mem.settling = Some((0, now_ms + SETTLE_MS));
            mem.quiet(now_ms, QUIET_MS);
            return out;
        }
        Plan::Around { keep, append } => {
            for k in (keep + 1..l.ids.len()).rev() {
                out.push(Act::Remove(k));
            }
            for _ in 0..keep {
                out.push(Act::Remove(0));
            }
            if !append.is_empty() {
                out.push(Act::Enqueue(append));
            }
            mem.pending = Some((w.ids[ei..].to_vec(), now_ms + 3000));
            // the song plays on: the next look sees the new queue
            return out;
        }
        Plan::Aligned { offset, append } => {
            if !append.is_empty() {
                let mut ids: Vec<i64> = l.ids.iter().flatten().copied().collect();
                ids.extend(append.iter().map(|g| w.ids[*g]));
                mem.pending = Some((ids, now_ms + 3000));
                out.push(Act::Enqueue(append));
            }
            offset
        }
    };
    let t = ei - offset;
    let pos_ms = (l.position * 1000.0).round() as i64;
    let exp_ms = (w.position * 1000.0).round() as i64;
    let playing = l.state == PlayState::Playing;
    // stopped at the group's song is fine while the group is paused (the end of its last song)
    if l.index != Some(t) || (w.play && l.state == PlayState::Stopped) {
        // a song change close to the group's: the engine runs gaplessly into the next song (or
        // the group does a moment later); the drift across it is closed with slices
        if w.play && playing {
            let near = match across(w, t, l) {
                Some(d) if d.abs() <= SONG_CHANGE_MS => {
                    if now_ms >= mem.quiet_until && l.fresh {
                        mem.nudge(d, &mut out);
                    }
                    true
                }
                Some(_) => false,
                None => {
                    (l.index == Some(t + 1) && pos_ms < SONG_CHANGE_MS)
                        || (l.index.map(|c| c + 1) == Some(t) && exp_ms < SONG_CHANGE_MS)
                }
            };
            if near {
                return out;
            }
        }
        mem.calm(&mut out);
        if !mem.try_load(&w.qid) {
            // the engine can't play this song here (it skipped it): quiet until the group moves on
            if playing {
                mem.own_pause_at = Some(now_ms);
                out.push(Act::Pause);
            }
            return out;
        }
        mem.settling = Some((t, now_ms + SETTLE_MS));
        let a = aim(mem, w.server_made, w.play);
        if !w.play {
            mem.lag.cancel();
            mem.own_pause_at = Some(now_ms);
        }
        out.push(Act::PlayAt { local: t, aim: a, play: w.play });
        mem.quiet(now_ms, QUIET_MS);
        return out;
    }
    if w.play && !playing {
        // after a pause here (or the group's): the drift rule. Far off (a long "Paused here"), back
        // where the group is: the seek is made while paused (the relay replaces what the engine's
        // stream held, so nothing of the old spot is heard) and the engine plays once it is in.
        // Closer, it plays from where it is and the slices close the rest (a group's pause and play
        // leaves everyone a moment past the spot: never a seek for that).
        mem.calm(&mut out);
        mem.placed = None;
        let seek_above = if l.server_made { SERVER_MADE_SEEK_MS } else { SEEK_MS };
        if (pos_ms - exp_ms).abs() > seek_above {
            let a = aim(mem, l.server_made, true);
            out.push(Act::Seek { aim: a, play: true });
            mem.no_seek_until = now_ms + SEEK_PAUSE_MS;
        } else {
            out.push(Act::Play);
        }
        mem.quiet(now_ms, 1500);
        return out;
    }
    if !w.play {
        mem.calm(&mut out);
        if playing {
            mem.lag.cancel();
            mem.own_pause_at = Some(now_ms);
            out.push(Act::Pause);
        }
        // a paused group: everyone at the same spot (once for each spot: a spot reached is not
        // sought again, whatever the engine's status says)
        let spot = (w.qid.clone(), exp_ms);
        if !w.group_playing
            && l.state == PlayState::Paused
            && (pos_ms - exp_ms).abs() > PAUSED_SEEK_MS
            && now_ms >= mem.quiet_until
            && mem.placed.as_ref() != Some(&spot)
        {
            out.push(Act::Seek {
                aim: Aim { pos: w.position, at_ms: now_ms, playing: false, lag_ms: 0 },
                play: false,
            });
            mem.placed = Some(spot);
            mem.quiet(now_ms, 1000);
        }
        return out;
    }
    mem.placed = None;
    if now_ms < mem.quiet_until || !l.fresh {
        return out;
    }
    let drift = pos_ms - exp_ms;
    // the first look at a seek made here, playing again: what it needed, for the next one
    if mem.lag.waiting() {
        mem.lag.landed(&w.qid, drift);
    }
    let seek_above = if l.server_made { SERVER_MADE_SEEK_MS } else { SEEK_MS };
    if drift.abs() > seek_above && now_ms >= mem.no_seek_until {
        mem.calm(&mut out);
        let a = aim(mem, l.server_made, true);
        out.push(Act::Seek { aim: a, play: false });
        mem.no_seek_until = now_ms + SEEK_PAUSE_MS;
        mem.quiet(now_ms, QUIET_MS);
        return out;
    }
    mem.nudge(drift, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(ids: &[i64], index: usize, state: PlayState, position: f64) -> Local {
        Local {
            ids: ids.iter().copied().map(Some).collect(),
            index: Some(index),
            state,
            position,
            fresh: true,
            server_made: false,
            busy: false,
        }
    }

    fn want(ids: &[i64], index: usize, position: f64, play: bool) -> Want {
        Want {
            ids: ids.to_vec(),
            durs: vec![200.0; ids.len()],
            qid: format!("q{index}"),
            index,
            position,
            group_playing: play,
            play,
            server_made: false,
        }
    }

    fn aim(pos: f64, at_ms: i64, playing: bool) -> Aim {
        Aim { pos, at_ms, playing, lag_ms: 0 }
    }

    /// The seek or load of a list of acts, if any.
    fn seek_of(acts: &[Act]) -> Option<Aim> {
        acts.iter().find_map(|a| match a {
            Act::Seek { aim, .. } | Act::Full { aim, .. } | Act::PlayAt { aim, .. } => Some(*aim),
            _ => None,
        })
    }

    #[test]
    fn the_engines_queue_becomes_the_groups_without_a_gap() {
        // the same run: nothing to do
        assert_eq!(plan(&[1, 2, 3], 1, &[Some(1), Some(2), Some(3)], Some(1)), Plan::Aligned { offset: 0, append: vec![] });
        // loaded from the group's second entry (the head left out)
        assert_eq!(plan(&[1, 2, 3], 2, &[Some(2), Some(3)], Some(1)), Plan::Aligned { offset: 1, append: vec![] });
        // a song added at the end: appended
        assert_eq!(plan(&[1, 2, 3, 4], 1, &[Some(1), Some(2), Some(3)], Some(1)), Plan::Aligned { offset: 0, append: vec![3] });
        // a song played next (inserted after the current one): rebuilt around the one playing
        assert_eq!(plan(&[1, 2, 9, 3], 1, &[Some(1), Some(2), Some(3)], Some(1)), Plan::Around { keep: 1, append: vec![2, 3] });
        // the current one removed from the group: it moved on, load again
        assert_eq!(plan(&[1, 3], 1, &[Some(1), Some(2), Some(3)], Some(1)), Plan::Full);
        // a file in the engine's queue, or another queue altogether
        assert_eq!(plan(&[1, 2], 0, &[Some(1), None], Some(1)), Plan::Full);
        // a file before the song that plays: the song plays on, the file goes
        assert_eq!(plan(&[1, 2], 0, &[None, Some(1)], Some(1)), Plan::Around { keep: 1, append: vec![1] });
        assert_eq!(plan(&[1, 2], 0, &[Some(7), Some(8)], Some(0)), Plan::Full);
        assert_eq!(plan(&[1, 2], 0, &[], None), Plan::Full);
        // the group went back before the run the engine holds
        assert_eq!(plan(&[1, 2, 3], 0, &[Some(2), Some(3)], Some(0)), Plan::Full);
        // the same song twice: the run whose current entry is the group's
        assert_eq!(plan(&[5, 5, 5], 2, &[Some(5), Some(5)], Some(1)), Plan::Aligned { offset: 1, append: vec![] });
    }

    #[test]
    fn joining_loads_the_groups_song_where_the_group_is() {
        let mut mem = Mem::default();
        let acts = decide(&want(&[1, 2, 3], 1, 42.0, true), &local(&[7], 0, PlayState::Playing, 3.0), &mut mem, 10_000);
        assert_eq!(acts, vec![Act::Full { from: 1, aim: aim(42.0, 10_000, true), play: true }]);
        // the engine has not taken it yet: nothing more meanwhile
        assert!(decide(&want(&[1, 2, 3], 1, 42.5, true), &local(&[7], 0, PlayState::Playing, 3.5), &mut mem, 10_500).is_empty());
        // a paused group (or "Paused here") loads paused, at the group's spot
        let mut mem = Mem::default();
        let acts = decide(&want(&[1, 2], 0, 10.0, false), &local(&[], 0, PlayState::Stopped, 0.0), &mut mem, 0);
        assert_eq!(acts, vec![Act::Full { from: 0, aim: aim(10.0, 0, false), play: false }]);
        assert!(mem.paused_by_us(1000));
    }

    #[test]
    fn under_40_ms_nothing_then_slices_until_under_20_ms_and_never_a_seek_below_1_5_s() {
        let mut mem = Mem::default();
        let w = want(&[1, 2], 0, 60.0, true);
        // 30 ms off: nothing
        assert!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 60.03), &mut mem, 10_000).is_empty());
        // 120 ms behind: slices, no seek
        assert_eq!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 59.88), &mut mem, 10_500), vec![Act::Nudge(-120)]);
        // still 30 ms behind: the correction goes on …
        assert_eq!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 59.97), &mut mem, 11_000), vec![Act::Nudge(-30)]);
        // … until under 20 ms
        assert_eq!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 59.99), &mut mem, 11_500), vec![Act::Nudge(0)]);
        assert!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 59.99), &mut mem, 12_000).is_empty());
        // 1.4 s ahead: still slices
        assert_eq!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 61.4), &mut mem, 12_500), vec![Act::Nudge(1400)]);
    }

    #[test]
    fn over_1_5_s_one_seek_then_slices_only_for_8_s() {
        let mut mem = Mem::default();
        let w = want(&[1, 2], 0, 60.0, true);
        let acts = decide(&w, &local(&[1, 2], 0, PlayState::Playing, 57.9), &mut mem, 10_000);
        assert_eq!(acts, vec![Act::Seek { aim: aim(60.0, 10_000, true), play: false }]);
        // not again while the seek settles
        assert!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 57.9), &mut mem, 11_000).is_empty());
        // still 2 s off after it: within 8 s of the seek only slices
        assert_eq!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 58.0), &mut mem, 13_000), vec![Act::Nudge(-2000)]);
        assert_eq!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 58.0), &mut mem, 17_500), vec![Act::Nudge(-2000)]);
        // 8 s on: a seek again (the correction that ran stops first)
        let acts = decide(&w, &local(&[1, 2], 0, PlayState::Playing, 58.0), &mut mem, 18_000);
        assert!(matches!(acts.as_slice(), [Act::Nudge(0), Act::Seek { play: false, .. }]), "{acts:?}");
    }

    #[test]
    fn a_stream_the_server_makes_is_seeked_only_over_3_s() {
        let mut mem = Mem::default();
        let w = want(&[1, 2], 0, 60.0, true);
        let mut l = local(&[1, 2], 0, PlayState::Playing, 57.5);
        l.server_made = true;
        assert_eq!(decide(&w, &l, &mut mem, 10_000), vec![Act::Nudge(-2500)]);
        l.position = 56.5;
        assert!(matches!(decide(&w, &l, &mut mem, 10_500).as_slice(), [Act::Nudge(0), Act::Seek { .. }]));
    }

    #[test]
    fn an_old_status_or_a_busy_relay_measures_no_drift() {
        let mut mem = Mem::default();
        let w = want(&[1, 2], 0, 60.0, true);
        let mut l = local(&[1, 2], 0, PlayState::Playing, 50.0);
        l.fresh = false;
        assert!(decide(&w, &l, &mut mem, 10_000).is_empty());
        l.fresh = true;
        l.busy = true;
        assert!(decide(&w, &l, &mut mem, 10_000).is_empty());
        // the group pausing meanwhile still pauses here
        let paused = Want { play: false, group_playing: false, ..w.clone() };
        assert_eq!(decide(&paused, &l, &mut mem, 10_100), vec![Act::Pause]);
        // done: a moment for the status to catch up, then the drift counts
        l.busy = false;
        assert!(decide(&w, &l, &mut mem, 10_500).is_empty());
        assert!(matches!(decide(&w, &l, &mut mem, 11_500).as_slice(), [Act::Seek { .. }]));
    }

    #[test]
    fn the_song_and_the_pause_follow_the_group() {
        let mut mem = Mem::default();
        // the host skipped to the second song, 5 s in
        let acts = decide(&want(&[1, 2], 1, 5.0, true), &local(&[1, 2], 0, PlayState::Playing, 80.0), &mut mem, 0);
        assert_eq!(acts, vec![Act::PlayAt { local: 1, aim: aim(5.0, 0, true), play: true }]);
        // the group paused: pause here, at its spot
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2], 0, 30.0, false), &local(&[1, 2], 0, PlayState::Playing, 30.2), &mut mem, 0), vec![Act::Pause]);
        assert_eq!(
            decide(&want(&[1, 2], 0, 30.0, false), &local(&[1, 2], 0, PlayState::Paused, 32.0), &mut mem, 5000),
            vec![Act::Seek { aim: aim(30.0, 5000, false), play: false }]
        );
        // it plays again: from where it is
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2], 0, 30.0, true), &local(&[1, 2], 0, PlayState::Paused, 30.05), &mut mem, 0), vec![Act::Play]);
        // or where the group is now, seeked while paused and played once it is in
        assert_eq!(
            decide(&want(&[1, 2], 0, 90.0, true), &local(&[1, 2], 0, PlayState::Paused, 30.0), &mut mem, 9000),
            vec![Act::Seek { aim: aim(90.0, 9000, true), play: true }]
        );
        // "Paused here": the group plays on, this computer stays paused
        let mut mem = Mem::default();
        let mut here = want(&[1, 2], 0, 90.0, true);
        here.play = false;
        assert!(decide(&here, &local(&[1, 2], 0, PlayState::Paused, 30.0), &mut mem, 0).is_empty());
        // the group stopped at the end of its last song, the engine at the end of it too
        let end = want(&[1, 2], 1, 100.0, false);
        assert!(decide(&end, &local(&[1, 2], 1, PlayState::Stopped, 0.0), &mut Mem::default(), 0).is_empty());
        // an emptied queue stops here
        assert_eq!(decide(&want(&[], 0, 0.0, false), &local(&[1], 0, PlayState::Playing, 3.0), &mut Mem::default(), 0), vec![Act::Pause]);
    }

    #[test]
    fn a_paused_group_is_placed_once_and_playing_again_follows_the_drift_rule() {
        // the group paused; the engine stopped 0.3 s past its spot (the pause came late): left there
        let mut mem = Mem::default();
        let paused = want(&[1, 2], 0, 30.0, false);
        assert!(decide(&paused, &local(&[1, 2], 0, PlayState::Paused, 30.3), &mut mem, 0).is_empty());
        // the host moved the paused group to 50 s: everyone there, once
        let moved = want(&[1, 2], 0, 50.0, false);
        let acts = decide(&moved, &local(&[1, 2], 0, PlayState::Paused, 30.3), &mut mem, 1000);
        assert_eq!(acts, vec![Act::Seek { aim: aim(50.0, 1000, false), play: false }]);
        // the engine's status still says 30.3 (a paused engine says little): not sought again
        for t in [2500, 4000, 9000, 20_000] {
            assert!(decide(&moved, &local(&[1, 2], 0, PlayState::Paused, 30.3), &mut mem, t).is_empty(), "at {t}");
        }
        // another spot: placed again
        let again = want(&[1, 2], 0, 80.0, false);
        assert!(matches!(decide(&again, &local(&[1, 2], 0, PlayState::Paused, 50.0), &mut mem, 21_000).as_slice(), [Act::Seek { play: false, .. }]));
        // the group plays again 0.3 s from where this computer is: play, the slices close it
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2], 0, 30.0, true), &local(&[1, 2], 0, PlayState::Paused, 30.3), &mut mem, 0), vec![Act::Play]);
        // 1.4 s off: still no seek; a stream the server makes not even at 2.5 s
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2], 0, 30.0, true), &local(&[1, 2], 0, PlayState::Paused, 31.4), &mut mem, 0), vec![Act::Play]);
        let mut made = local(&[1, 2], 0, PlayState::Paused, 32.5);
        made.server_made = true;
        assert_eq!(decide(&want(&[1, 2], 0, 30.0, true), &made, &mut Mem::default(), 0), vec![Act::Play]);
        made.position = 33.5;
        assert!(matches!(decide(&want(&[1, 2], 0, 30.0, true), &made, &mut Mem::default(), 0).as_slice(), [Act::Seek { play: true, .. }]));
    }

    #[test]
    fn a_song_change_within_3_s_is_left_to_the_engine_and_its_drift_sliced() {
        // the engine ran into the next song 0.4 s before the group (its song is 200 s): 0.4 s ahead
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2], 0, 199.6, true), &local(&[1, 2], 1, PlayState::Playing, 0.0), &mut mem, 0), vec![Act::Nudge(400)]);
        // the group moved on 2.5 s before the engine: behind, still the engine's to do
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2], 1, 1.0, true), &local(&[1, 2], 0, PlayState::Playing, 198.5), &mut mem, 0), vec![Act::Nudge(-2500)]);
        // further than 3 s: the group's song is loaded
        let mut mem = Mem::default();
        let acts = decide(&want(&[1, 2], 1, 2.5, true), &local(&[1, 2], 0, PlayState::Playing, 198.0), &mut mem, 0);
        assert!(matches!(acts.as_slice(), [Act::PlayAt { local: 1, .. }]), "{acts:?}");
        // lengths not known: the old rule (within 3 s of the change), without slices
        let mut w = want(&[1, 2], 0, 199.0, true);
        w.durs = vec![];
        assert!(decide(&w, &local(&[1, 2], 1, PlayState::Playing, 1.0), &mut Mem::default(), 0).is_empty());
    }

    #[test]
    fn a_song_this_computer_cant_play_is_tried_three_times_then_left_alone() {
        let mut mem = Mem::default();
        let w = want(&[1, 2, 3], 0, 5.0, true);
        let mut now = 0;
        let mut loads = 0;
        // the engine skips song 1 every time (a file it can't read) and plays song 3
        for _ in 0..40 {
            let acts = decide(&w, &local(&[1, 2, 3], 2, PlayState::Playing, 3.0), &mut mem, now);
            loads += acts.iter().filter(|a| matches!(a, Act::PlayAt { local: 0, .. })).count();
            now += 500;
        }
        assert_eq!(loads, MAX_TRIES as usize);
        // then it stops there instead of playing another song than the group's
        assert_eq!(decide(&w, &local(&[1, 2, 3], 2, PlayState::Playing, 3.0), &mut mem, now), vec![Act::Pause]);
        assert!(decide(&w, &local(&[1, 2, 3], 2, PlayState::Paused, 2.0), &mut mem, now + 500).is_empty());
        // the group moves on: its next song plays here
        let next = Want { qid: "q1".into(), ..want(&[1, 2, 3], 1, 0.5, true) };
        let acts = decide(&next, &local(&[1, 2, 3], 1, PlayState::Paused, 4.0), &mut mem, now + 1000);
        assert!(matches!(acts.as_slice(), [Act::Seek { play: true, .. }]), "{acts:?}");
        // a slow stream: no second load while the engine is still opening the first
        let mut mem = Mem::default();
        let acts = decide(&w, &local(&[1, 2, 3], 2, PlayState::Playing, 1.0), &mut mem, 0);
        assert!(matches!(acts.as_slice(), [Act::PlayAt { local: 0, .. }]));
        assert!(decide(&w, &local(&[1, 2, 3], 2, PlayState::Playing, 1.5), &mut mem, 3000).is_empty());
    }

    #[test]
    fn a_song_added_to_the_group_is_appended_and_a_play_next_rebuilt_around() {
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2, 3], 0, 10.0, true), &local(&[1, 2], 0, PlayState::Playing, 10.0), &mut mem, 0), vec![Act::Enqueue(vec![2])]);
        let mut mem = Mem::default();
        assert_eq!(
            decide(&want(&[1, 2, 9, 3], 1, 10.0, true), &local(&[1, 2, 3], 1, PlayState::Playing, 10.0), &mut mem, 0),
            vec![Act::Remove(2), Act::Remove(0), Act::Enqueue(vec![2, 3])]
        );
    }

    #[test]
    fn the_seek_lag_is_the_median_of_the_last_five_seeks() {
        let mut lag = SeekLag::default();
        assert_eq!(lag.target("q1", false), 0);
        // landed 300 ms behind: 300 needed
        lag.landed("q1", -300);
        assert_eq!(lag.lag_ms(false), 300);
        // seeks aimed with what was learnt land close; one wild result barely moves the median
        for off in [20, -2200, -10, 30] {
            let used = lag.target("q1", false);
            lag.landed("q1", used - 300 + off);
        }
        assert!((270..=310).contains(&lag.lag_ms(false)), "{}", lag.lag_ms(false));
        // the other kind of stream learns apart
        assert_eq!(lag.lag_ms(true), 0);
        // a result in another song teaches nothing; within 0 … 3 s
        lag.target("q2", true);
        lag.landed("q3", -2000);
        assert_eq!(lag.lag_ms(true), 0);
        lag.target("q2", true);
        lag.landed("q2", -9000);
        assert_eq!(lag.lag_ms(true), 3000);
        // only the last five count
        for _ in 0..5 {
            lag.target("q4", true);
            lag.landed("q4", 3000);
        }
        assert_eq!(lag.lag_ms(true), 0);
    }

    #[test]
    fn a_seek_aims_ahead_by_the_learnt_lag_and_the_aim_runs_on() {
        let mut mem = Mem::default();
        mem.lag.target("q0", false);
        mem.lag.landed("q0", -250);
        let w = want(&[1, 2], 0, 60.0, true);
        let acts = decide(&w, &local(&[1, 2], 0, PlayState::Playing, 50.0), &mut mem, 10_000);
        let a = seek_of(&acts).unwrap();
        assert_eq!(a.lag_ms, 250);
        // 2 s later (the stream took that long to open) the first frame comes from 62.25
        assert!((a.at(12_000) - 62.25).abs() < 1e-9);
        // paused: the spot itself
        assert_eq!(aim(30.0, 0, false).at(99_000), 30.0);
    }

    #[test]
    fn a_steady_drift_is_closed_with_slices_and_never_a_seek() {
        // a computer 600 ms behind, closing at the slices' pace (1 % of what is left per 100 ms,
        // at most 2 %), looked at twice a second: it never seeks and ends within 20 ms
        let mut mem = Mem::default();
        let mut drift = -600.0f64;
        let mut want_ms = 0.0f64;
        let mut seeks = 0;
        for i in 0..400 {
            let now = 10_000 + i * 500;
            let w = want(&[1, 2], 0, 60.0, true);
            let l = local(&[1, 2], 0, PlayState::Playing, 60.0 + drift / 1000.0);
            for a in decide(&w, &l, &mut mem, now) {
                match a {
                    Act::Nudge(d) => want_ms = -(d as f64),
                    Act::Seek { .. } => seeks += 1,
                    _ => {}
                }
            }
            // half a second of slices: 5 of them
            for _ in 0..5 {
                let k = (want_ms.abs() * 0.01).clamp(0.02, 2.0).min(want_ms.abs()) * want_ms.signum();
                drift += k;
                want_ms -= k;
            }
        }
        assert_eq!(seeks, 0);
        assert!(drift.abs() < 20.0, "{drift}");
        assert!(!mem.nudging);
    }
}
