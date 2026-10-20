//! The engine follows the group (p6 item 8, client-shapes §5.6): the group's queue, song,
//! position and play / pause on this computer's Cadenza.
//!
//! Sync rule (the same on web, desktop and phone): expected position = position + (server_now −
//! anchor_ms) / 1000 while playing. Drift above 0.5 s → seek. The web and the phone close a drift
//! of 0.15–0.5 s by playing at 0.97 / 1.03; Cadenza has no rate control, so the desktop seeks
//! only. A seek aims ahead by what this computer's seeks have been taking to start playing
//! ([`SeekLag`], learnt per kind of stream, the phone's SeekLag.kt); where seeks keep missing,
//! that song only seeks for a drift above 3 s.
//!
//! The engine's queue holds a run of the group's queue (from the entry it was loaded at), so a
//! song added, removed or moved around the one playing changes the engine's queue without a gap
//! in the music: see [`plan`]. Crossfade does not exist in Cadenza; gapless stays.
//!
//! Everything here is pure (tested below); `mod.rs` turns the [`Act`]s into engine commands.

use crate::engine::PlayState;

/// The sync rule's seek threshold (ms).
pub const SEEK_MS: i64 = 500;
/// The threshold in a song where seeking did not work (ms).
pub const BACKED_OFF_MS: i64 = 3000;
const MAX_LAG_MS: i64 = 3000;
const MISS_WINDOW_MS: i64 = 15_000;
/// After this computer moved the engine, no drift check for this long (a seek needs a moment
/// to buffer).
pub const QUIET_MS: i64 = 2500;
/// How long the engine may take to open the song it was asked to play (a stream over a slow
/// network) before another look decides again.
pub const SETTLE_MS: i64 = 5000;
/// A song of the group the engine can't play here (it skips it: a broken file, a format it can't
/// read) is loaded this many times, then left alone until the group moves on.
pub const MAX_TRIES: u32 = 3;

/// What a seek made to catch up with the group costs on this computer: learnt from where each
/// seek lands, for whole files and server-made streams apart, at most 3 s, and added to the next
/// seek's target. Two seeks within 15 s that still land more than 0.5 s off stop seeking in that
/// song below 3 s.
#[derive(Clone, Debug, Default)]
pub struct SeekLag {
    lag: [i64; 2],
    /// The kind of stream of the seek whose result is awaited.
    wait_kind: Option<usize>,
    wait_song: String,
    miss_at: i64,
    missed: bool,
    off_for: Option<String>,
}

impl SeekLag {
    /// A seek's result is awaited: the next look while playing calls [`SeekLag::landed`].
    pub fn waiting(&self) -> bool {
        self.wait_kind.is_some()
    }

    /// Where to seek in `song` (the queue entry's id) for the group's `expected_ms`: ahead by the
    /// lag learnt for that kind of stream.
    pub fn target(&mut self, song: &str, expected_ms: i64, server_made: bool) -> i64 {
        let k = usize::from(server_made);
        self.wait_kind = Some(k);
        self.wait_song = song.to_string();
        expected_ms + self.lag[k]
    }

    /// The first look while playing after a seek made with `target`: `drift_ms` = this
    /// computer's position − the group's in `song`, at `now_ms`.
    pub fn landed(&mut self, song: &str, drift_ms: i64, now_ms: i64) {
        let Some(k) = self.wait_kind.take() else {
            return;
        };
        if song != self.wait_song {
            return;
        }
        self.lag[k] = (self.lag[k] - drift_ms).clamp(0, MAX_LAG_MS);
        if drift_ms.abs() > SEEK_MS {
            if self.missed && now_ms - self.miss_at <= MISS_WINDOW_MS {
                self.off_for = Some(song.to_string());
            }
            self.missed = true;
            self.miss_at = now_ms;
        }
    }

    /// The seek awaited is not one to learn from (a pause, leaving the group).
    pub fn cancel(&mut self) {
        self.wait_kind = None;
    }

    /// Above this drift (ms) a seek is made: 0.5 s, or 3 s in a song where seeking did not work.
    pub fn seek_above_ms(&self, song: &str) -> i64 {
        if self.off_for.as_deref() == Some(song) {
            BACKED_OFF_MS
        } else {
            SEEK_MS
        }
    }

    #[cfg(test)]
    pub fn lag_ms(&self, server_made: bool) -> i64 {
        self.lag[usize::from(server_made)]
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
    /// The current entry is a server-made stream (a transcode, Data saver, the server's Cadenza).
    pub server_made: bool,
}

/// One thing to do to the engine. Indices of the group's queue are named `group`, the engine's
/// `local`.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Append these group entries.
    Enqueue(Vec<usize>),
    /// Remove this engine entry.
    Remove(usize),
    /// Load the group's queue from entry `from` and start at `position`; paused unless `play`.
    Full { from: usize, position: f64, play: bool },
    /// Play this engine entry (from its start).
    PlayIndex(usize),
    Seek(f64),
    Play,
    Pause,
}

/// What the group wants, at this moment.
#[derive(Clone, Debug)]
pub struct Want {
    /// The group's queue (library ids).
    pub ids: Vec<i64>,
    /// The current entry's `qid` (what the seek lag is learnt for).
    pub qid: String,
    /// Where the group is: its entry and the position in it.
    pub index: usize,
    pub position: f64,
    /// The group plays.
    pub group_playing: bool,
    /// The group plays and this computer is not "Paused here".
    pub play: bool,
}

/// What this computer remembers between looks.
#[derive(Clone, Debug, Default)]
pub struct Mem {
    /// No drift check until then (monotonic ms).
    pub quiet_until: i64,
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
}

/// One look: what to do so the engine follows the group. `now_ms` is monotonic.
pub fn decide(w: &Want, l: &Local, mem: &mut Mem, now_ms: i64) -> Vec<Act> {
    let mut out = Vec::new();
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
        if l.state == PlayState::Playing {
            mem.own_pause_at = Some(now_ms);
            out.push(Act::Pause);
        }
        return out;
    }
    let ei = w.index.min(w.ids.len() - 1);
    let target_ms = |mem: &mut Mem, server_made: bool| -> f64 {
        mem.lag.target(&w.qid, (w.position * 1000.0).round() as i64, server_made) as f64 / 1000.0
    };
    let offset = match plan(&w.ids, ei, &l.ids, l.index) {
        Plan::Full => {
            if !mem.try_load(&w.qid) {
                // the engine can't play this song here: quiet until the group moves on
                if l.state == PlayState::Playing {
                    mem.own_pause_at = Some(now_ms);
                    out.push(Act::Pause);
                }
                return out;
            }
            let position = if w.play { target_ms(mem, false) } else { w.position };
            if !w.play {
                mem.lag.cancel();
                mem.own_pause_at = Some(now_ms);
            }
            out.push(Act::Full {
                from: ei,
                position,
                play: w.play,
            });
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
        // gapless into the next song a moment before (or after) the group: let it settle
        let near = w.play
            && playing
            && ((l.index == Some(t + 1) && pos_ms < 1500) || (l.index.map(|c| c + 1) == Some(t) && exp_ms < 1500));
        if near {
            return out;
        }
        if !mem.try_load(&w.qid) {
            // the engine can't play this song here (it skipped it): quiet until the group moves on
            if playing {
                mem.own_pause_at = Some(now_ms);
                out.push(Act::Pause);
            }
            return out;
        }
        mem.settling = Some((t, now_ms + SETTLE_MS));
        out.push(Act::PlayIndex(t));
        if w.play {
            if exp_ms > 300 {
                out.push(Act::Seek(target_ms(mem, l.server_made)));
            }
        } else {
            if exp_ms > 0 {
                out.push(Act::Seek(w.position));
            }
            mem.own_pause_at = Some(now_ms);
            out.push(Act::Pause);
        }
        mem.quiet(now_ms, QUIET_MS);
        return out;
    }
    if w.play && !playing {
        // after a pause here (or the group's), back where the group is
        if (pos_ms - exp_ms).abs() > SEEK_MS {
            out.push(Act::Seek(target_ms(mem, l.server_made)));
        }
        out.push(Act::Play);
        mem.quiet(now_ms, 1500);
        return out;
    }
    if !w.play {
        if playing {
            mem.lag.cancel();
            mem.own_pause_at = Some(now_ms);
            out.push(Act::Pause);
        }
        // a paused group: everyone at the same spot
        if !w.group_playing && l.state == PlayState::Paused && (pos_ms - exp_ms).abs() > SEEK_MS && now_ms >= mem.quiet_until {
            out.push(Act::Seek(w.position));
            mem.quiet(now_ms, 1000);
        }
        return out;
    }
    if now_ms < mem.quiet_until {
        return out;
    }
    let drift = pos_ms - exp_ms;
    // the first look at a seek made here, playing again: what it took, for the next one
    if mem.lag.waiting() {
        mem.lag.landed(&w.qid, drift, now_ms);
    }
    if drift.abs() > mem.lag.seek_above_ms(&w.qid) {
        out.push(Act::Seek(target_ms(mem, l.server_made)));
        mem.quiet(now_ms, QUIET_MS);
    }
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
            server_made: false,
        }
    }

    fn want(ids: &[i64], index: usize, position: f64, play: bool) -> Want {
        Want {
            ids: ids.to_vec(),
            qid: format!("q{index}"),
            index,
            position,
            group_playing: play,
            play,
        }
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
        assert_eq!(acts, vec![Act::Full { from: 1, position: 42.0, play: true }]);
        // the engine has not taken it yet: nothing more meanwhile
        assert!(decide(&want(&[1, 2, 3], 1, 42.5, true), &local(&[7], 0, PlayState::Playing, 3.5), &mut mem, 10_500).is_empty());
        // a paused group (or "Paused here") loads paused, at the group's spot
        let mut mem = Mem::default();
        let acts = decide(&want(&[1, 2], 0, 10.0, false), &local(&[], 0, PlayState::Stopped, 0.0), &mut mem, 0);
        assert_eq!(acts, vec![Act::Full { from: 0, position: 10.0, play: false }]);
        assert!(mem.paused_by_us(1000));
    }

    #[test]
    fn the_desktop_seeks_only() {
        let mut mem = Mem::default();
        let w = want(&[1, 2], 0, 60.0, true);
        // 0.3 s off: within the rule for the desktop (no rate control)
        assert!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 60.3), &mut mem, 10_000).is_empty());
        // 0.6 s behind: seek
        assert_eq!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 59.4), &mut mem, 10_000), vec![Act::Seek(60.0)]);
        // not again while the seek settles
        assert!(decide(&w, &local(&[1, 2], 0, PlayState::Playing, 59.4), &mut mem, 11_000).is_empty());
    }

    #[test]
    fn the_song_and_the_pause_follow_the_group() {
        let mut mem = Mem::default();
        // the host skipped to the second song, 5 s in
        let acts = decide(&want(&[1, 2], 1, 5.0, true), &local(&[1, 2], 0, PlayState::Playing, 80.0), &mut mem, 0);
        assert_eq!(acts, vec![Act::PlayIndex(1), Act::Seek(5.0)]);
        // gapless into the next song a moment before the group: left alone
        let mut mem = Mem::default();
        assert!(decide(&want(&[1, 2], 0, 199.5, true), &local(&[1, 2], 1, PlayState::Playing, 0.4), &mut mem, 0).is_empty());
        // the group paused: pause here, at its spot
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2], 0, 30.0, false), &local(&[1, 2], 0, PlayState::Playing, 30.2), &mut mem, 0), vec![Act::Pause]);
        assert_eq!(
            decide(&want(&[1, 2], 0, 30.0, false), &local(&[1, 2], 0, PlayState::Paused, 32.0), &mut mem, 5000),
            vec![Act::Seek(30.0)]
        );
        // it plays again: from where it is
        let mut mem = Mem::default();
        assert_eq!(decide(&want(&[1, 2], 0, 30.0, true), &local(&[1, 2], 0, PlayState::Paused, 30.1), &mut mem, 0), vec![Act::Play]);
        assert_eq!(
            decide(&want(&[1, 2], 0, 90.0, true), &local(&[1, 2], 0, PlayState::Paused, 30.0), &mut mem, 9000),
            vec![Act::Seek(90.0), Act::Play]
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
    fn a_song_this_computer_cant_play_is_tried_three_times_then_left_alone() {
        let mut mem = Mem::default();
        let w = want(&[1, 2, 3], 0, 5.0, true);
        let mut now = 0;
        let mut loads = 0;
        // the engine skips song 1 every time (a file it can't read) and plays song 2
        for _ in 0..40 {
            let acts = decide(&w, &local(&[1, 2, 3], 1, PlayState::Playing, 3.0), &mut mem, now);
            loads += acts.iter().filter(|a| matches!(a, Act::PlayIndex(0))).count();
            now += 500;
        }
        assert_eq!(loads, MAX_TRIES as usize);
        // then it stops there instead of playing another song than the group's
        assert_eq!(decide(&w, &local(&[1, 2, 3], 1, PlayState::Playing, 3.0), &mut mem, now), vec![Act::Pause]);
        assert!(decide(&w, &local(&[1, 2, 3], 1, PlayState::Paused, 2.0), &mut mem, now + 500).is_empty());
        // the group moves on: its next song plays here
        let next = Want { qid: "q1".into(), ..want(&[1, 2, 3], 1, 0.5, true) };
        assert_eq!(decide(&next, &local(&[1, 2, 3], 1, PlayState::Paused, 2.0), &mut mem, now + 1000), vec![Act::Seek(0.5), Act::Play]);
        // a slow stream: no second load while the engine is still opening the first
        let mut mem = Mem::default();
        assert_eq!(decide(&w, &local(&[1, 2, 3], 2, PlayState::Playing, 1.0), &mut mem, 0), vec![Act::PlayIndex(0), Act::Seek(5.0)]);
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
    fn a_slow_seek_is_learnt_and_does_not_loop() {
        // a server-made stream that starts playing 0.8 s after every seek, 2 s behind the group at
        // first; the desktop looks every 0.5 s, not within 2.5 s of its own seek
        let mut lag = SeekLag::default();
        let latency = 800;
        let mut seek_at = -1i64;
        let mut seek_target = 0i64;
        let group = |x: i64| 60_000 + x;
        let mut quiet_until = 0;
        let mut seeks = 0;
        let mut x = 500;
        let phone = |x: i64, seek_at: i64, seek_target: i64| {
            if seek_at < 0 {
                58_000 + x
            } else if x < seek_at + latency {
                seek_target
            } else {
                seek_target + (x - seek_at - latency)
            }
        };
        while x <= 120_000 {
            if x >= quiet_until {
                let d = phone(x, seek_at, seek_target) - group(x);
                if lag.waiting() {
                    lag.landed("q1", d, x);
                }
                if d.abs() > lag.seek_above_ms("q1") {
                    seek_target = lag.target("q1", group(x), true);
                    seek_at = x;
                    quiet_until = x + QUIET_MS;
                    seeks += 1;
                }
            }
            x += 500;
        }
        assert_eq!(seeks, 2);
        assert_eq!(lag.lag_ms(true), 800);
        assert_eq!(lag.lag_ms(false), 0);
        assert_eq!(phone(120_000, seek_at, seek_target) - group(120_000), 0);
    }

    #[test]
    fn seeks_that_keep_missing_stop_for_the_song() {
        let mut lag = SeekLag::default();
        lag.target("q1", 10_000, true);
        lag.landed("q1", -900, 3_000);
        assert_eq!(lag.lag_ms(true), 900);
        assert_eq!(lag.seek_above_ms("q1"), 500);
        lag.target("q1", 20_000, true);
        lag.landed("q1", 700, 9_000);
        assert_eq!(lag.lag_ms(true), 200);
        assert_eq!(lag.seek_above_ms("q1"), 3000);
        // the next song seeks as usual; a result in another song teaches nothing; within 0 … 3 s
        assert_eq!(lag.seek_above_ms("q2"), 500);
        lag.target("q2", 0, false);
        lag.landed("q3", -2000, 10_000);
        assert_eq!(lag.lag_ms(false), 0);
        lag.target("q2", 0, false);
        lag.landed("q2", -9000, 40_000);
        assert_eq!(lag.lag_ms(false), 3000);
    }
}
