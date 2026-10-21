//! The server's clock as this computer sees it (the same rule on web, phone and desktop):
//!
//! - at join: 8 round trips to `GET /api/time`; of the 3 with the shortest round trip, the
//!   median offset (the server's time taken at the middle of each round trip);
//! - then every 60 s: 3 more round trips; a round trip over twice the best one seen (at least
//!   20 ms: on a fast network twice 1–2 ms would turn every one away) says nothing (the server or
//!   the network was busy) and is left out; the new estimate is blended in (30 % of it), never
//!   moving the offset by more than 50 ms at once;
//! - after 5 refreshes in a row with nothing kept (the network changed), the best round trip seen
//!   becomes the best of the last refresh, so a network that got slower for good is followed.
//!
//! Offsets are server − this computer's monotonic clock, in ms. Pure; tested below.

/// Round trips at join.
pub const JOIN_SAMPLES: usize = 8;
/// Round trips of each refresh.
pub const REFRESH_SAMPLES: usize = 3;
/// A refresh this often while in a group (ms).
pub const REFRESH_EVERY_MS: i64 = 60_000;
/// Of the round trips at join, the shortest this many count.
const KEEP: usize = 3;
/// How much of a refresh's estimate goes in.
const BLEND: f64 = 0.3;
/// The most one refresh moves the offset (ms).
const MAX_STEP_MS: i64 = 50;
/// A refresh keeps round trips up to twice the best seen, and never turns away one this short (ms).
const MIN_BAR_MS: i64 = 20;
/// After this many refreshes in a row with nothing kept, the best seen is the last refresh's.
const RESET_AFTER: u32 = 5;

/// One round trip: (sent, the server's `now_ms`, received), local times in ms.
pub type Sample = (i64, i64, i64);

fn rtt(s: &Sample) -> i64 {
    s.2 - s.0
}

fn offset_of(s: &Sample) -> i64 {
    s.1 - (s.0 + s.2) / 2
}

fn usable(s: &Sample) -> bool {
    s.2 >= s.0 && s.1 > 0
}

fn median(mut v: Vec<i64>) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2 })
}

/// The offset this computer keeps and the best round trip seen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clock {
    pub offset: Option<i64>,
    pub best_rtt: Option<i64>,
    /// Refreshes in a row that kept nothing.
    empty: u32,
}

impl Clock {
    /// The measurement at join: the median offset of the (up to) 3 shortest round trips.
    pub fn join(samples: &[Sample]) -> Clock {
        let mut ok: Vec<&Sample> = samples.iter().filter(|s| usable(s)).collect();
        ok.sort_by_key(|s| rtt(s));
        ok.truncate(KEEP);
        Clock {
            offset: median(ok.iter().map(|s| offset_of(s)).collect()),
            best_rtt: ok.first().map(|s| rtt(s)),
            empty: 0,
        }
    }

    /// A refresh: round trips over twice the best seen (at least 20 ms) are left out; the median
    /// of the rest is blended in, at most 50 ms of change. After 5 refreshes in a row with nothing
    /// kept, the best seen becomes this refresh's. Without an offset yet it is taken like a join.
    pub fn refresh(&mut self, samples: &[Sample]) {
        let Some(old) = self.offset else {
            *self = Clock::join(samples);
            return;
        };
        let ok: Vec<&Sample> = samples.iter().filter(|s| usable(s)).collect();
        let Some(newest) = ok.iter().map(|s| rtt(s)).min() else {
            return;
        };
        let best = self.best_rtt.map_or(newest, |x| x.min(newest));
        self.best_rtt = Some(best);
        let bar = (best * 2).max(MIN_BAR_MS);
        let kept: Vec<i64> = ok.iter().filter(|s| rtt(s) <= bar).map(|s| offset_of(s)).collect();
        let Some(est) = median(kept) else {
            self.empty += 1;
            if self.empty >= RESET_AFTER {
                self.best_rtt = Some(newest);
                self.empty = 0;
            }
            return;
        };
        self.empty = 0;
        let step = ((est - old) as f64 * BLEND).round() as i64;
        self.offset = Some(old + step.clamp(-MAX_STEP_MS, MAX_STEP_MS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A round trip sent at `at` with `rtt` ms there and back, the server `off` ms ahead and its
    /// clock read `late` ms after the middle (the server was busy before answering).
    fn trip(at: i64, rtt: i64, off: i64, late: i64) -> Sample {
        (at, at + rtt / 2 + off + late, at + rtt)
    }

    #[test]
    fn the_join_takes_the_median_of_the_three_shortest_round_trips() {
        let samples = [
            trip(0, 40, 5000, 0),
            trip(100, 400, 5000, 180), // slow and late: left out
            trip(600, 30, 5000, 4),
            trip(700, 300, 5000, -120),
            trip(1100, 36, 5000, -2),
            trip(1200, 900, 5000, 400),
            trip(2200, 60, 5000, 20),
            trip(2300, 33, 5000, 2),
        ];
        let c = Clock::join(&samples);
        // the three shortest: 30 (+4), 33 (+2), 36 (−2) → the median, +2
        assert_eq!(c.offset, Some(5002));
        assert_eq!(c.best_rtt, Some(30));
        assert_eq!(Clock::join(&[]), Clock::default());
        assert_eq!(Clock::join(&[(10, 0, 20)]).offset, None);
    }

    #[test]
    fn a_refresh_blends_in_and_never_steps_more_than_50_ms() {
        let mut c = Clock { offset: Some(5000), best_rtt: Some(30), ..Default::default() };
        // the server's clock 100 ms further: 30 % → +30
        c.refresh(&[trip(0, 30, 5100, 0), trip(100, 32, 5100, 0), trip(200, 31, 5100, 0)]);
        assert_eq!(c.offset, Some(5030));
        // a far estimate moves 50 ms at most
        c.refresh(&[trip(300, 30, 6000, 0)]);
        assert_eq!(c.offset, Some(5080));
        // round trips over twice the best one seen say nothing
        let before = c.clone();
        c.refresh(&[trip(400, 70, 9000, 0), trip(500, 200, 9000, 0)]);
        assert_eq!((c.offset, c.best_rtt), (before.offset, before.best_rtt));
        // nothing usable: unchanged
        c.refresh(&[]);
        assert_eq!((c.offset, c.best_rtt), (before.offset, before.best_rtt));
        // a shorter round trip than any before becomes the best
        c.refresh(&[trip(600, 10, 5080, 0)]);
        assert_eq!(c.best_rtt, Some(10));
        assert_eq!(c.offset, Some(5080));
        // without an offset yet a refresh measures like a join
        let mut fresh = Clock::default();
        fresh.refresh(&[trip(0, 20, 300, 0)]);
        assert_eq!(fresh.offset, Some(300));
    }

    #[test]
    fn repeated_refreshes_settle_on_the_servers_clock() {
        let mut c = Clock { offset: Some(0), best_rtt: Some(20), ..Default::default() };
        for i in 0..40 {
            c.refresh(&[trip(i * 60_000, 20, 200, 0)]);
        }
        assert!((c.offset.unwrap() - 200).abs() <= 2, "{:?}", c.offset);
    }

    #[test]
    fn a_fast_network_and_a_network_that_got_slower_are_followed() {
        // on a LAN the best round trip is 1 ms: a 3 ms one is still kept (the bar is at least 20 ms)
        let mut c = Clock::join(&[trip(0, 1, 1000, 0)]);
        assert_eq!(c.best_rtt, Some(1));
        c.refresh(&[trip(60_000, 3, 1100, 0)]);
        assert_eq!(c.offset, Some(1030));
        // the network got slower for good (100 ms): 5 refreshes keep nothing, the 5th makes its
        // round trip the best, the next one counts
        for i in 1..=5 {
            c.refresh(&[trip(i * 60_000, 100, 1200, 0)]);
            assert_eq!(c.offset, Some(1030), "refresh {i}");
        }
        assert_eq!(c.best_rtt, Some(100));
        c.refresh(&[trip(400_000, 100, 1200, 0)]);
        assert_eq!(c.offset, Some(1080));
        // one refresh that keeps something starts the count again
        let mut c = Clock::join(&[trip(0, 1, 1000, 0)]);
        for i in 1..=4 {
            c.refresh(&[trip(i * 60_000, 100, 1200, 0)]);
        }
        c.refresh(&[trip(300_000, 2, 1000, 0)]);
        for i in 6..=9 {
            c.refresh(&[trip(i * 60_000, 100, 1200, 0)]);
        }
        assert_eq!((c.offset, c.best_rtt), (Some(1000), Some(1)));
    }
}
