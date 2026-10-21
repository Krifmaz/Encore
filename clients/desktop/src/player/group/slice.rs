//! Small drift in a group listen, closed without a seek and without a speed change: Cadenza has
//! no playback rate, so the desktop drops (to catch up) or repeats (to wait) tiny slices of the
//! decoded samples on their way to the engine (`relay.rs`).
//!
//! - At most one slice per 100 ms of audio, and a slice is at most 2 ms long: never more than
//!   2 ms per 100 ms (2 %, the other clients' largest speed nudge).
//! - The size follows the drift (1 % of what is left per slice: the pace of the other clients'
//!   nudge), so a correction eases out as it closes; the first slices of a correction are smaller
//!   (it eases in).
//! - Every slice is a crossfade of 8 ms (raised cosine) from where the audio is to where it
//!   continues, `k` frames on (a drop) or back (a repeat), so the join can't click. `k` is picked
//!   in the second half of the wanted size where the two stretches look most alike (the
//!   waveform's phase lines up), so a steady note does not thin out for the moment of the join.
//!
//! Everything works on interleaved `f64` frames as the decoder delivers them (any channel count,
//! any rate). Pure and tested below.

/// One slice per this much audio, at most (s).
pub const SPACING_S: f64 = 0.100;
/// The longest slice (s): 2 ms per 100 ms.
pub const MAX_SLICE_S: f64 = 0.002;
/// The crossfade of a join (s).
pub const XFADE_S: f64 = 0.008;
/// Of what is left to correct, this much per slice of 100 ms: the web's and the phone's nudge
/// (0.1 % of speed per 10 ms of drift), so a drift closes at the same pace on every client.
const GAIN: f64 = 0.01;
/// The first slices of a correction grow to the full size over this many slices.
const EASE_IN: u32 = 4;

/// Drops or repeats slices of the frames that pass through it.
#[derive(Debug, Clone)]
pub struct Slicer {
    channels: usize,
    spacing: usize,
    max_k: usize,
    xfade: usize,
    /// Frames waiting to go out (interleaved); `read` is the next one, the frames before it are
    /// history (what a repeat goes back into).
    buf: Vec<f64>,
    read: usize,
    /// Frames out since the last slice.
    since: usize,
    /// What is left to correct, in frames: > 0 drop (this computer is behind), < 0 repeat.
    want: f64,
    /// Slices made in this correction (ease-in).
    run: u32,
    /// Input frames passed (consumed): the source position of the next frame out is the first
    /// input frame's plus this.
    consumed: u64,
    /// Frames out.
    produced: u64,
}

impl Slicer {
    pub fn new(rate: u32, channels: u32) -> Slicer {
        let rate = f64::from(rate.max(1));
        let frames = |s: f64| ((rate * s).round() as usize).max(1);
        Slicer {
            channels: channels.max(1) as usize,
            spacing: frames(SPACING_S),
            max_k: frames(MAX_SLICE_S),
            xfade: frames(XFADE_S).max(2),
            buf: Vec::new(),
            read: 0,
            since: 0,
            want: 0.0,
            run: 0,
            consumed: 0,
            produced: 0,
        }
    }

    /// The correction to make from now on, in frames (> 0 drop, < 0 repeat; 0 stops). A new
    /// correction (after none, or the other way) eases in again.
    pub fn set_want(&mut self, frames: f64) {
        let was = self.want;
        self.want = if frames.is_finite() { frames } else { 0.0 };
        if was.abs() < 1.0 || was.signum() != self.want.signum() {
            self.run = 0;
        }
    }

    /// What is still to correct (frames).
    pub fn want(&self) -> f64 {
        self.want
    }

    /// Input frames passed so far (see [`Slicer::consumed`] on the field).
    pub fn consumed(&self) -> u64 {
        self.consumed
    }

    /// Frames out so far.
    #[cfg(test)]
    pub fn produced(&self) -> u64 {
        self.produced
    }

    /// Frames in, not out yet.
    pub fn held(&self) -> usize {
        self.frames() - self.read
    }

    fn frames(&self) -> usize {
        self.buf.len() / self.channels
    }

    /// More input (interleaved, whole frames).
    pub fn push(&mut self, pcm: &[f64]) {
        let whole = pcm.len() - pcm.len() % self.channels;
        self.buf.extend_from_slice(&pcm[..whole]);
    }

    /// The frames held and not out yet, taken out (a splice continues from them); the history
    /// goes too.
    pub fn take_held(&mut self) -> Vec<f64> {
        let out = self.buf.split_off(self.read * self.channels);
        self.buf.clear();
        self.read = 0;
        out
    }

    /// Up to `max` frames out. A few milliseconds stay held (a slice needs them ahead) unless
    /// `last` (the end of the song: everything goes, without a slice).
    pub fn pull(&mut self, max: usize, last: bool) -> Vec<f64> {
        let ch = self.channels;
        let mut out: Vec<f64> = Vec::with_capacity(max.min(self.held()) * ch);
        let reserve = if last { 0 } else { self.max_k + self.xfade };
        while out.len() / ch < max {
            let avail = self.held();
            let k = if last { 0 } else { self.next_k() };
            let size = k.unsigned_abs() as usize;
            // a repeat goes back into frames already out: none yet at the very start
            let history_ok = k >= 0 || self.read >= size;
            if k != 0 && self.since >= self.spacing && history_ok {
                let need = if k > 0 { size + self.xfade } else { self.xfade };
                if avail < need {
                    break;
                }
                // the join goes out whole: in the next pull when this one has no room left
                if !out.is_empty() && out.len() / ch + self.xfade > max {
                    break;
                }
                let k = self.best_k(k);
                self.join(k, &mut out);
                continue;
            }
            let room = max - out.len() / ch;
            let until_slice = if k == 0 {
                usize::MAX
            } else if self.since < self.spacing {
                self.spacing - self.since
            } else {
                size.saturating_sub(self.read).max(1)
            };
            let n = room.min(avail.saturating_sub(reserve)).min(until_slice);
            if n == 0 {
                break;
            }
            let from = self.read * ch;
            out.extend_from_slice(&self.buf[from..from + n * ch]);
            self.read += n;
            self.since += n;
            self.consumed += n as u64;
            self.produced += n as u64;
        }
        self.trim();
        out
    }

    /// The size of the next slice (frames, signed), 0 for none: 2 % of what is left, at least
    /// one frame, at most 2 ms, smaller for the first slices of a correction.
    fn next_k(&self) -> i64 {
        if self.want.abs() < 1.0 {
            return 0;
        }
        let ease = f64::from((self.run + 1).min(EASE_IN)) / f64::from(EASE_IN);
        let cap = (self.max_k as f64 * ease).max(1.0);
        let k = (self.want.abs() * GAIN).max(1.0).min(cap).min(self.want.abs()).round().max(1.0);
        (k as i64) * self.want.signum() as i64
    }

    /// Of the sizes from half to one and a half times `k` (same sign, at most 2 ms), the one where
    /// the stretch at `read` and the one it crossfades into are most alike.
    fn best_k(&self, k: i64) -> i64 {
        let size = k.unsigned_abs() as usize;
        if size < 4 {
            return k;
        }
        // a repeat can't go back further than the frames already out
        let top = (size * 3 / 2).min(self.max_k).min(if k < 0 { self.read } else { usize::MAX }).max(size);
        // a drop needs its frames ahead
        let top = if k > 0 { top.min(self.held().saturating_sub(self.xfade)).max(size) } else { top };
        let sign = k.signum();
        let ch = self.channels;
        let r = self.read as i64;
        let at = |frame: i64, c: usize| self.buf[frame as usize * ch + c];
        let step = (self.xfade / 256).max(1);
        let mut best = (f64::MIN, k);
        for s in (size.div_ceil(2))..=top {
            let off = sign * s as i64;
            let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
            let mut i = 0;
            while i < self.xfade {
                for c in 0..ch {
                    let a = at(r + i as i64, c);
                    let b = at(r + off + i as i64, c);
                    xy += a * b;
                    xx += a * a;
                    yy += b * b;
                }
                i += step;
            }
            let score = if xx * yy > 1e-18 { xy / (xx * yy).sqrt() } else { 0.0 };
            // ties (silence) keep the size asked for
            if score > best.0 + 1e-9 || (score >= best.0 - 1e-9 && s == size) {
                best = (score, off);
            }
        }
        best.1
    }

    /// One join: `xfade` frames out that go from the audio at `read` to the audio `k` frames on
    /// (or back), then on from there.
    fn join(&mut self, k: i64, out: &mut Vec<f64>) {
        let ch = self.channels;
        let r = self.read;
        let x = self.xfade;
        for i in 0..x {
            let w = 0.5 - 0.5 * (std::f64::consts::PI * (i as f64 + 0.5) / x as f64).cos();
            let a = (r + i) * ch;
            let b = ((r + i) as i64 + k) as usize * ch;
            for c in 0..ch {
                out.push(self.buf[a + c] * (1.0 - w) + self.buf[b + c] * w);
            }
        }
        let next = (r as i64 + k) as usize + x;
        self.consumed = (self.consumed as i64 + x as i64 + k) as u64;
        self.produced += x as u64;
        self.read = next;
        self.since = 0;
        self.want -= k as f64;
        if self.want.abs() < 1.0 {
            self.want = 0.0;
        }
        self.run += 1;
    }

    /// Keep only what a repeat could go back into.
    fn trim(&mut self) {
        let keep = self.max_k + 1;
        if self.read > keep * 4 {
            let drop = self.read - keep;
            self.buf.drain(..drop * self.channels);
            self.read -= drop;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// `secs` of a stereo sine (left) and its inverse (right).
    fn sine(freq: f64, secs: f64, amp: f64) -> Vec<f64> {
        let n = (f64::from(RATE) * secs) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(RATE)).sin();
            v.push(s);
            v.push(-s);
        }
        v
    }

    /// Everything through the slicer in decoder-sized chunks, pulled in engine-sized ones.
    fn run(input: &[f64], want: f64) -> (Vec<f64>, Slicer) {
        let mut s = Slicer::new(RATE, 2);
        s.set_want(want);
        let mut out = Vec::new();
        for chunk in input.chunks(4096 * 2) {
            s.push(chunk);
            loop {
                let got = s.pull(1024, false);
                if got.is_empty() {
                    break;
                }
                out.extend(got);
            }
        }
        out.extend(s.pull(usize::MAX / 4, true));
        (out, s)
    }

    /// The largest step between two frames (left channel).
    fn max_step(v: &[f64]) -> f64 {
        v.chunks(2).zip(v.chunks(2).skip(1)).map(|(a, b)| (b[0] - a[0]).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn nothing_to_correct_passes_every_frame_untouched() {
        let input = sine(440.0, 1.0, 0.5);
        let (out, s) = run(&input, 0.0);
        assert_eq!(out, input);
        assert_eq!((s.consumed(), s.produced()), (44_100, 44_100));
    }

    #[test]
    fn a_drop_catches_up_at_most_2_ms_per_100_ms() {
        let input = sine(440.0, 3.0, 0.5);
        let n_in = input.len() / 2;
        // 0.5 s behind: far more than 3 s of audio can close
        let (out, s) = run(&input, 0.5 * f64::from(RATE));
        let n_out = out.len() / 2;
        let dropped = n_in - n_out;
        assert_eq!(s.consumed() as usize, n_in);
        assert_eq!(s.produced() as usize, n_out);
        // the whole 3 s: at most 30 slices of at most 2 ms (88 frames)
        assert!(dropped > 0, "nothing dropped");
        assert!(dropped <= 30 * 88, "dropped {dropped}");
        // and never more than 2 ms in any 100 ms: one slice per 4410 frames out, each ≤ 88
        assert!(dropped as f64 / n_out as f64 <= 0.02 * 1.02, "{dropped} of {n_out}");
        // most of the budget is used once eased in (it does correct)
        assert!(dropped >= 20 * 70, "dropped only {dropped}");
    }

    #[test]
    fn a_repeat_waits_at_most_2_ms_per_100_ms() {
        let input = sine(440.0, 3.0, 0.5);
        let n_in = input.len() / 2;
        let (out, s) = run(&input, -0.5 * f64::from(RATE));
        let n_out = out.len() / 2;
        assert!(n_out > n_in);
        assert!((n_out - n_in) as f64 / n_out as f64 <= 0.02 * 1.02);
        assert_eq!(s.consumed() as usize, n_in);
    }

    #[test]
    fn a_drift_closes_at_the_pace_it_asks_for_and_the_count_is_exact() {
        let input = sine(220.0, 4.0, 0.5);
        // 100 ms behind = 4410 frames: 1 % of what is left per 100 ms, after the ease-in
        let (out, s) = run(&input, 4410.0);
        let dropped = (input.len() - out.len()) / 2;
        // every frame dropped is counted off what was asked
        assert_eq!(dropped as f64, 4410.0 - s.want());
        // about 40 slices of about 1 % of what is left (each picked between half and one and a
        // half times that, where the waveform lines up): a good part closed, not all of it
        assert!((400..2600).contains(&dropped), "{dropped}");
        // a drift that is gone stops the slices
        let mut s = Slicer::new(RATE, 2);
        s.set_want(30.0);
        s.set_want(0.0);
        s.push(&input);
        let out = s.pull(usize::MAX / 4, false);
        assert_eq!(&out[..], &input[..out.len()]);
    }

    #[test]
    fn the_joins_are_crossfaded_and_do_not_click() {
        // a pure tone is where a cut would be heard most: every join must be as smooth as the
        // tone itself (no step larger than the tone's own steepest), on both channels, at low and
        // high pitch, catching up and waiting
        for freq in [55.0, 440.0, 1000.0, 3150.0] {
            for want in [0.3, -0.3] {
                let input = sine(freq, 2.0, 0.8);
                let natural = max_step(&input);
                let (out, _) = run(&input, want * f64::from(RATE));
                let step = max_step(&out);
                assert!(step <= natural * 1.25 + 1e-9, "{freq} Hz, {want}: step {step} vs {natural}");
                // the right channel stays the left one's inverse (the channels are not mixed)
                assert!(out.chunks(2).all(|f| (f[0] + f[1]).abs() < 1e-12));
            }
        }
        // the same drop as a plain cut, without the crossfade, clicks (the check above would see it)
        let input = sine(440.0, 0.2, 0.8);
        let natural = max_step(&input);
        let mut cut = input[..2000 * 2].to_vec();
        cut.extend_from_slice(&input[(2000 + 50) * 2..]);
        assert!(max_step(&cut) > natural * 1.25);
    }

    #[test]
    fn noise_and_silence_go_through_without_jumps() {
        // silence stays silence
        let silent = vec![0.0; 44_100 * 2];
        let (out, _) = run(&silent, 2000.0);
        assert!(out.iter().all(|v| *v == 0.0));
        // a stretch that starts loud after silence: no frame larger than the input's largest
        let mut v = vec![0.0; 22_050 * 2];
        v.extend(sine(300.0, 1.0, 0.9));
        let (out, _) = run(&v, -3000.0);
        let peak = out.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        assert!(peak <= 0.9 + 1e-9);
    }

    #[test]
    fn the_first_slices_of_a_correction_are_smaller() {
        let mut s = Slicer::new(RATE, 2);
        s.set_want(10_000.0);
        let mut sizes = Vec::new();
        let input = sine(440.0, 1.0, 0.5);
        s.push(&input);
        let mut last = s.want();
        while sizes.len() < 6 {
            if s.pull(256, false).is_empty() {
                break;
            }
            if s.want() != last {
                sizes.push(last - s.want());
                last = s.want();
            }
        }
        assert!(sizes.len() >= 5, "{sizes:?}");
        assert!(sizes[0] < sizes[3], "{sizes:?}");
        assert!(sizes.iter().all(|k| *k <= 88.0), "{sizes:?}");
    }

    #[test]
    fn held_frames_can_be_taken_for_a_splice() {
        let mut s = Slicer::new(RATE, 2);
        let input = sine(440.0, 0.1, 0.5);
        s.push(&input);
        let out = s.pull(1000, false);
        let held = s.take_held();
        assert_eq!(out.len() + held.len(), input.len());
        assert_eq!(&held[..], &input[out.len()..]);
        assert_eq!(s.held(), 0);
    }
}
