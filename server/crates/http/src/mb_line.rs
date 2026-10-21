//! The MusicBrainz line: every call to MusicBrainz waits here for its turn, at most `musicbrainz_rate` calls a
//! second for the whole server. It used to be one first-come-first-served lock, so a page someone was looking at
//! and a song being imported waited behind hundreds of album lookups after a restart (the owner on Oct 21: "why
//! is it taking so long to import?"). Now the turn goes by priority:
//!
//! 1. [`Priority::Interactive`] – a page someone is looking at (every API call, `encore_api`'s layer)
//! 2. [`Priority::Import`] – a download being imported (its tags, the release to check it against)
//! 3. [`Priority::Request`] – the other steps of a request (which album a song is on, a discography)
//! 4. [`Priority::Background`] – everything else: translations, aliases, the retag task, contributions …
//!
//! First come, first served inside each. A class that has had someone waiting for [`SHARE`] without a turn
//! gets the next one, so background work still gets a few calls a minute however busy the line is. A call
//! says its priority by running inside [`with_priority`]; a call that doesn't is background work.
//!
//! Work that makes several calls (a song's album lookup: its recording, then the album it is on) keeps one
//! place for all of them when it runs inside [`one_place`]: its next call goes where its first one stood, not
//! to the back. So such work finishes in the order it started. Before, 300 lookups of two calls started
//! together had none finished before 600 calls had gone by.
use crate::progress;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

/// Who is waiting for MusicBrainz, most urgent first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// a page someone is looking at
    Interactive = 0,
    /// a download being imported
    Import = 1,
    /// a request's other steps
    Request = 2,
    /// work nobody waits for
    Background = 3,
}

impl Priority {
    pub const ALL: [Priority; 4] = [Priority::Interactive, Priority::Import, Priority::Request, Priority::Background];

    pub fn name(self) -> &'static str {
        match self {
            Priority::Interactive => "interactive",
            Priority::Import => "import",
            Priority::Request => "request",
            Priority::Background => "background",
        }
    }
}

tokio::task_local! {
    static PRIORITY: Priority;
    static TICKET: Ticket;
}

/// Run `f` with its MusicBrainz calls at priority `p` (an inner call overrides an outer one; a task spawned
/// from `f` doesn't inherit it).
pub async fn with_priority<F: Future>(p: Priority, f: F) -> F::Output {
    PRIORITY.scope(p, f).await
}

/// The priority of the calls made here: [`Priority::Background`] outside [`with_priority`].
pub fn priority() -> Priority {
    PRIORITY.try_with(|p| *p).unwrap_or(Priority::Background)
}

/// How long a class may have someone waiting without getting a turn before it gets the next one.
pub const SHARE: Duration = Duration::from_secs(20);
/// A wait shows in the caller's words ([`progress::waiting`]) once it is this long …
const SAY_AFTER: Duration = Duration::from_secs(2);
/// … and is updated this often (a request's message: it was every 5 s, and 600 waiting lookups were 120
/// database writes a second).
const SAY_EVERY: Duration = Duration::from_secs(15);
/// A caller that isn't next looks again at least this often (it is woken when it becomes next anyway).
const LOOK_AGAIN: Duration = Duration::from_secs(10);

/// One piece of work's place in the line ([`one_place`]): the place its first call took, and when that call
/// had its turn.
#[derive(Default)]
struct TicketState {
    place: Option<Place>,
    first_turn: Option<Instant>,
}

#[derive(Clone, Default)]
struct Ticket(Arc<Mutex<TicketState>>);

impl Ticket {
    fn state(&self) -> MutexGuard<'_, TicketState> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Run `f` with all its MusicBrainz calls at one place in the line: the place its first call took. Its later
/// calls go before the calls that joined after that first one (at their own priority), so work that makes
/// several calls finishes before the work that came after it. An inner `one_place` has a place of its own.
pub async fn one_place<F: Future>(f: F) -> F::Output {
    TICKET.scope(Ticket::default(), f).await
}

/// [`one_place`], giving up (None) when no call of `f` has had a turn within `first`, or when `f` isn't done
/// `rest` after its first turn. A long line ends the wait for the first turn; once the work has had it, its
/// later calls keep that place and come quickly.
pub async fn one_place_within<F: Future>(first: Duration, rest: Duration, f: F) -> Option<F::Output> {
    let ticket = Ticket::default();
    let start = Instant::now();
    let work = TICKET.scope(ticket.clone(), f);
    tokio::pin!(work);
    loop {
        let first_turn = ticket.state().first_turn;
        let until = match first_turn {
            Some(t) => t + rest,
            None => start + first,
        };
        if Instant::now() >= until {
            return None;
        }
        tokio::select! {
            biased;
            out = &mut work => return Some(out),
            _ = tokio::time::sleep_until(until) => {}
        }
    }
}

struct Waiter {
    /// this waiter alone (calls of one piece of work can share `seq`)
    id: u64,
    /// its place: the lower goes first in its class
    seq: u64,
    class: Priority,
    since: Instant,
    wake: Arc<Notify>,
}

#[derive(Default)]
struct State {
    /// the earliest moment the next call may go out
    next: Option<Instant>,
    waiters: Vec<Waiter>,
    seq: u64,
    ids: u64,
    /// when each class last had a turn
    served: [Option<Instant>; 4],
    /// turns given since the start, by class
    turns: [u64; 4],
}

impl State {
    /// Since when `class` has waited without a turn, when that is [`SHARE`] or longer.
    fn starved_since(&self, class: Priority, now: Instant) -> Option<Instant> {
        let oldest = self.waiters.iter().filter(|w| w.class == class).map(|w| w.since).min()?;
        let since = self.served[class as usize].map_or(oldest, |s| s.max(oldest));
        (now.saturating_duration_since(since) >= SHARE).then_some(since)
    }

    /// The waiter whose turn is next: the one of a starved class that waited longest, else the most urgent class's
    /// first.
    fn head(&self, now: Instant) -> Option<usize> {
        let starved = Priority::ALL[1..].iter().filter_map(|c| self.starved_since(*c, now).map(|s| (s, *c))).min();
        let class = match starved {
            Some((_, c)) => c,
            None => self.waiters.iter().map(|w| w.class).min()?,
        };
        self.waiters.iter().enumerate().filter(|(_, w)| w.class == class).min_by_key(|(_, w)| (w.seq, w.id)).map(|(i, _)| i)
    }

    fn position(&self, id: u64) -> Option<usize> {
        self.waiters.iter().position(|w| w.id == id)
    }

    /// How many waiters go before this one (the shares of starved classes aside).
    fn ahead(&self, id: u64, seq: u64, class: Priority) -> usize {
        self.waiters.iter().filter(|w| w.class < class || (w.class == class && (w.seq, w.id) < (seq, id))).count()
    }

    fn wake_head(&self, now: Instant) -> Option<Arc<Notify>> {
        self.head(now).map(|i| self.waiters[i].wake.clone())
    }
}

/// A caller's place in the line, kept when it joins again after a 503 (and by every call of a piece of work
/// inside [`one_place`]) so it doesn't go to the back.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Place {
    seq: u64,
    since: Instant,
}

/// The line itself (one per server: [`crate::mb::Mb`]). Clones share it.
#[derive(Clone, Default)]
pub struct Line(Arc<Mutex<State>>);

/// Takes a waiter that gave up (its future was dropped) out of the line and hands the turn on.
struct Leave<'a> {
    line: &'a Line,
    id: u64,
    done: bool,
}

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let wake = {
            let mut s = self.line.lock();
            if let Some(i) = s.position(self.id) {
                s.waiters.remove(i);
            }
            s.wake_head(Instant::now())
        };
        if let Some(w) = wake {
            w.notify_one();
        }
    }
}

/// "1 lookup ahead", "120 lookups ahead".
fn lookups(n: usize) -> String {
    if n == 1 { "1 lookup ahead".into() } else { format!("{n} lookups ahead") }
}

/// What a caller with `ahead` callers before it waits for, in words.
pub fn wait_words(ahead: usize, secs: f64) -> String {
    if ahead == 0 {
        return "waiting for MusicBrainz (next in line)".into();
    }
    format!("waiting for MusicBrainz ({}, {})", progress::about(secs), lookups(ahead))
}

impl Line {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wait for a turn at priority `class`; the turn after it comes `gap` later. `place`: the place a caller
    /// had before (it joins again after a 503 at that place); without one, the place of the work it is part
    /// of ([`one_place`]), else a new one at the back. Says how long the wait is to the caller's [`progress`]
    /// sink while it lasts.
    pub(crate) async fn turn(&self, class: Priority, gap: Duration, place: Option<Place>) -> Place {
        let ticket = TICKET.try_with(|t| t.clone()).ok();
        let place = place.or_else(|| {
            let kept = ticket.as_ref().and_then(|t| t.state().place);
            kept.map(|p| Place { seq: p.seq, since: Instant::now() })
        });
        let wake = Arc::new(Notify::new());
        let (place, id) = {
            let mut s = self.lock();
            let place = place.unwrap_or_else(|| {
                s.seq += 1;
                Place { seq: s.seq, since: Instant::now() }
            });
            s.ids += 1;
            let id = s.ids;
            s.waiters.push(Waiter { id, seq: place.seq, class, since: place.since, wake: wake.clone() });
            (place, id)
        };
        let mut leave = Leave { line: self, id, done: false };
        let saying = progress::active();
        let mut say_at = Instant::now() + SAY_AFTER;
        let mut said = false;
        loop {
            let now = Instant::now();
            let (until, other) = {
                let mut s = self.lock();
                let head = s.head(now);
                let me = s.position(id);
                if head.is_some() && head == me {
                    match s.next {
                        Some(n) if n > now => (n, None),
                        _ => {
                            // our turn: the next one `gap` from now
                            s.waiters.remove(me.unwrap_or_default());
                            s.next = Some(now + gap);
                            s.served[class as usize] = Some(now);
                            s.turns[class as usize] += 1;
                            let next = s.wake_head(now);
                            drop(s);
                            leave.done = true;
                            if let Some(w) = next {
                                w.notify_one();
                            }
                            if said {
                                progress::resume();
                            }
                            if let Some(t) = &ticket {
                                let mut t = t.state();
                                t.place.get_or_insert(place);
                                t.first_turn.get_or_insert(now);
                            }
                            return place;
                        }
                    }
                } else {
                    // the one whose turn it is may not know yet (a class's share came due meanwhile)
                    (now + LOOK_AGAIN, head.map(|i| s.waiters[i].wake.clone()))
                }
            };
            if let Some(w) = other {
                w.notify_one();
            }
            let until = if saying { until.min(say_at) } else { until };
            tokio::select! {
                _ = wake.notified() => {}
                _ = tokio::time::sleep_until(until) => {}
            }
            if saying && Instant::now() >= say_at {
                let (ahead, secs) = {
                    let s = self.lock();
                    let left = s.next.map_or(0.0, |n| n.saturating_duration_since(Instant::now()).as_secs_f64());
                    let ahead = s.ahead(id, place.seq, class);
                    (ahead, ahead as f64 * gap.as_secs_f64() + left)
                };
                progress::waiting(&wait_words(ahead, secs));
                said = true;
                say_at = Instant::now() + SAY_EVERY;
            }
        }
    }

    /// How many wait at `class` or a more urgent one (what a caller at `class` joining now would have ahead).
    pub fn ahead_of(&self, class: Priority) -> usize {
        self.lock().waiters.iter().filter(|w| w.class <= class).count()
    }

    /// The line now, for /api/admin/perf: who waits at each priority, the longest wait, and the turns given.
    pub fn stats(&self) -> Value {
        let s = self.lock();
        let now = Instant::now();
        let mut out = serde_json::Map::new();
        for c in Priority::ALL {
            let ws: Vec<&Waiter> = s.waiters.iter().filter(|w| w.class == c).collect();
            let oldest = ws.iter().map(|w| now.saturating_duration_since(w.since).as_secs_f64()).fold(0.0, f64::max);
            out.insert(c.name().into(), json!({ "waiting": ws.len(), "longest_wait_s": (oldest * 10.0).round() / 10.0, "turns": s.turns[c as usize] }));
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    type Log = Arc<StdMutex<Vec<(&'static str, f64)>>>;

    /// A caller that waits for its turn at `class` and writes down when it got it (seconds since `t0`).
    fn caller(line: &Line, log: &Log, t0: Instant, name: &'static str, class: Priority, gap: Duration) -> tokio::task::JoinHandle<()> {
        let (line, log) = (line.clone(), log.clone());
        tokio::spawn(async move {
            line.turn(class, gap, None).await;
            log.lock().unwrap().push((name, (Instant::now() - t0).as_secs_f64()));
        })
    }

    /// Let every spawned caller join the line before the clock moves.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    fn spaced(log: &[(&'static str, f64)], gap: f64) {
        for w in log.windows(2) {
            assert!(w[1].1 - w[0].1 >= gap - 1e-6, "{} came {:.3} s after {}: faster than the rate", w[1].0, w[1].1 - w[0].1, w[0].0);
        }
    }

    /// A page goes first, then an import, then a request's lookup, then background work; one call a second.
    #[tokio::test(start_paused = true)]
    async fn turns_go_by_priority_at_the_rate() {
        let (line, log, t0, gap) = (Line::default(), Log::default(), Instant::now(), Duration::from_secs(1));
        let mut hs = vec![caller(&line, &log, t0, "first", Priority::Background, gap)];
        settle().await;
        for (n, c) in [("bg", Priority::Background), ("req", Priority::Request), ("imp", Priority::Import), ("page", Priority::Interactive), ("req2", Priority::Request)] {
            hs.push(caller(&line, &log, t0, n, c, gap));
            settle().await;
        }
        for h in hs {
            h.await.unwrap();
        }
        let log = log.lock().unwrap().clone();
        let names: Vec<&str> = log.iter().map(|x| x.0).collect();
        assert_eq!(names, ["first", "page", "imp", "req", "req2", "bg"]);
        spaced(&log, 1.0);
        assert!(log[0].1 < 0.01 && (log[5].1 - 5.0).abs() < 0.01, "{log:?}");
        assert_eq!(line.stats()["request"]["turns"], 2);
        assert_eq!(line.stats()["request"]["waiting"], 0);
    }

    /// However long the line of request lookups, background work gets a turn every SHARE.
    #[tokio::test(start_paused = true)]
    async fn background_work_never_starves() {
        let (line, log, t0, gap) = (Line::default(), Log::default(), Instant::now(), Duration::from_secs(1));
        let mut hs = vec![];
        for _ in 0..60 {
            hs.push(caller(&line, &log, t0, "req", Priority::Request, gap));
        }
        settle().await;
        hs.push(caller(&line, &log, t0, "bg", Priority::Background, gap));
        hs.push(caller(&line, &log, t0, "bg", Priority::Background, gap));
        settle().await;
        assert_eq!(line.ahead_of(Priority::Request), 59);
        for h in hs {
            h.await.unwrap();
        }
        let log = log.lock().unwrap().clone();
        spaced(&log, 1.0);
        let bg: Vec<f64> = log.iter().filter(|x| x.0 == "bg").map(|x| x.1).collect();
        // the first after SHARE, the second SHARE after that – not after all 60 lookups
        assert!((bg[0] - 20.0).abs() < 0.01 && (bg[1] - 40.0).abs() < 0.01, "{bg:?}");
        assert!((log.last().unwrap().1 - 61.0).abs() < 0.01);
    }

    /// A caller that gives up (its future dropped: a page's deadline) leaves the line; the next one still gets
    /// its turn on time.
    #[tokio::test(start_paused = true)]
    async fn a_caller_that_gives_up_leaves_the_line() {
        let (line, log, t0, gap) = (Line::default(), Log::default(), Instant::now(), Duration::from_secs(1));
        let first = caller(&line, &log, t0, "first", Priority::Request, gap);
        settle().await;
        let l = line.clone();
        let quitter = tokio::spawn(async move { tokio::time::timeout(Duration::from_millis(500), l.turn(Priority::Interactive, gap, None)).await.is_err() });
        settle().await;
        let next = caller(&line, &log, t0, "next", Priority::Request, gap);
        assert!(quitter.await.unwrap(), "the page gave up");
        first.await.unwrap();
        next.await.unwrap();
        let log = log.lock().unwrap().clone();
        assert_eq!(log.iter().map(|x| x.0).collect::<Vec<_>>(), ["first", "next"]);
        assert!((log[1].1 - 1.0).abs() < 0.01, "{log:?}");
        assert_eq!(line.stats()["interactive"]["waiting"], 0);
    }

    /// A caller that joins again after a 503 keeps its place: it goes before the ones that came after it.
    #[tokio::test(start_paused = true)]
    async fn a_caller_back_after_a_503_keeps_its_place() {
        let (line, gap) = (Line::default(), Duration::from_secs(1));
        let place = line.turn(Priority::Request, gap, None).await;
        let log = Log::default();
        let t0 = Instant::now();
        let later = caller(&line, &log, t0, "later", Priority::Request, gap);
        settle().await;
        let (l, lg) = (line.clone(), log.clone());
        let back = tokio::spawn(async move {
            l.turn(Priority::Request, gap, Some(place)).await;
            lg.lock().unwrap().push(("back", (Instant::now() - t0).as_secs_f64()));
        });
        later.await.unwrap();
        back.await.unwrap();
        assert_eq!(log.lock().unwrap().iter().map(|x| x.0).collect::<Vec<_>>(), ["back", "later"]);
    }

    /// A caller with a progress sink is told how long it waits and how many go before it.
    #[tokio::test(start_paused = true)]
    async fn a_long_wait_is_said_in_words() {
        let (line, gap) = (Line::default(), Duration::from_secs(1));
        let mut hs = vec![];
        for _ in 0..90 {
            let l = line.clone();
            hs.push(tokio::spawn(async move { l.turn(Priority::Import, gap, None).await }));
        }
        settle().await;
        // (it joins half a second later, so its looks never fall on the moment a turn is given)
        tokio::time::sleep(Duration::from_millis(500)).await;
        let said = Arc::new(StdMutex::new(Vec::<String>::new()));
        let s = said.clone();
        let l = line.clone();
        let me = tokio::spawn(progress::with_sink(move |w| s.lock().unwrap().push(w.to_string()), async move {
            progress::step("looking it up on MusicBrainz");
            l.turn(Priority::Import, gap, None).await;
        }));
        me.await.unwrap();
        for h in hs {
            h.await.unwrap();
        }
        let said = said.lock().unwrap().clone();
        assert_eq!(said[0], "looking it up on MusicBrainz");
        // at 2.5 s: 87 ahead (90 joined before it, 3 served), the next turn in half a second
        assert_eq!(said[1], "waiting for MusicBrainz (about a minute, 87 lookups ahead)");
        // and every 15 s after: at 77.5 s 78 were served
        assert!(said.iter().any(|w| w == "waiting for MusicBrainz (under a minute, 12 lookups ahead)"), "{said:?}");
        // and the step again once its turn came
        assert_eq!(said.last().unwrap(), "looking it up on MusicBrainz");
        assert_eq!(wait_words(0, 0.5), "waiting for MusicBrainz (next in line)");
        assert_eq!(wait_words(1, 1.0), "waiting for MusicBrainz (under a minute, 1 lookup ahead)");
        assert_eq!(wait_words(120, 120.0), "waiting for MusicBrainz (about 2 minutes, 120 lookups ahead)");
    }

    /// Work that makes two calls keeps one place for both: three lookups started together finish at 1, 3 and
    /// 5 s, one after the other – not all three at the end, each call going to the back.
    #[tokio::test(start_paused = true)]
    async fn the_calls_of_one_piece_of_work_keep_its_place() {
        let (line, log, t0, gap) = (Line::default(), Log::default(), Instant::now(), Duration::from_secs(1));
        let mut hs = vec![];
        for name in ["a", "b", "c"] {
            let (l, lg) = (line.clone(), log.clone());
            hs.push(tokio::spawn(one_place(async move {
                l.turn(Priority::Request, gap, None).await;
                l.turn(Priority::Request, gap, None).await;
                lg.lock().unwrap().push((name, (Instant::now() - t0).as_secs_f64()));
            })));
            settle().await;
        }
        for h in hs {
            h.await.unwrap();
        }
        let log = log.lock().unwrap().clone();
        assert_eq!(log.iter().map(|x| x.0).collect::<Vec<_>>(), ["a", "b", "c"]);
        for (got, want) in log.iter().map(|x| x.1).zip([1.0, 3.0, 5.0]) {
            assert!((got - want).abs() < 0.01, "{log:?}");
        }
        // outside `one_place` every call goes to the back: the first lookup's second call comes after the others' first
        let (log2, t1) = (Log::default(), Instant::now());
        let mut hs = vec![];
        for name in ["a", "b"] {
            let (l, lg) = (line.clone(), log2.clone());
            hs.push(tokio::spawn(async move {
                l.turn(Priority::Request, gap, None).await;
                l.turn(Priority::Request, gap, None).await;
                lg.lock().unwrap().push((name, (Instant::now() - t1).as_secs_f64()));
            }));
            settle().await;
        }
        for h in hs {
            h.await.unwrap();
        }
        let log2 = log2.lock().unwrap().clone();
        assert!((log2[0].1 - 3.0).abs() < 0.01 && (log2[1].1 - 4.0).abs() < 0.01, "{log2:?}");
    }

    /// Work that gets no turn within its first wait gives up and leaves the line; work whose first turn comes in
    /// time is done, its next call coming at once after it; and work still busy `rest` after its first turn
    /// gives up too.
    #[tokio::test(start_paused = true)]
    async fn work_gives_up_when_its_first_turn_is_too_far_off() {
        let (line, log, t0, gap) = (Line::default(), Log::default(), Instant::now(), Duration::from_secs(1));
        let (first, rest) = (Duration::from_secs(5), Duration::from_secs(5));
        let two = |l: Line| async move {
            l.turn(Priority::Request, gap, None).await;
            l.turn(Priority::Request, gap, None).await;
            Instant::now()
        };
        let mut hs = vec![];
        for _ in 0..10 {
            hs.push(caller(&line, &log, t0, "req", Priority::Request, gap));
        }
        settle().await;
        // ten ahead: its first turn would come at 10 s
        assert!(one_place_within(first, rest, two(line.clone())).await.is_none());
        assert!(((Instant::now() - t0).as_secs_f64() - 5.0).abs() < 0.01);
        settle().await;
        // six served (0 to 5 s), four left: it isn't waiting any more
        assert_eq!(line.stats()["request"]["waiting"], 4, "{}", line.stats());
        for h in hs {
            h.await.unwrap();
        }
        // three ahead: its first turn at 3 s, its second call at 4 s
        tokio::time::sleep(Duration::from_secs(2)).await;
        let t1 = Instant::now();
        let mut hs = vec![];
        for _ in 0..3 {
            hs.push(caller(&line, &log, t0, "req", Priority::Request, gap));
        }
        settle().await;
        let l = line.clone();
        let late = tokio::spawn(async move {
            settle().await;
            l.turn(Priority::Request, gap, None).await;
            Instant::now()
        });
        let done = one_place_within(first, rest, two(line.clone())).await.expect("its first turn came in time");
        assert!(((done - t1).as_secs_f64() - 4.0).abs() < 0.01, "{:?}", done - t1);
        assert!(late.await.unwrap() > done, "a call that joined after its first one waits for its second");
        for h in hs {
            h.await.unwrap();
        }
        // a turn at once, then too long a while
        let t2 = Instant::now();
        let slow = |l: Line| async move {
            l.turn(Priority::Request, gap, None).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        };
        assert!(one_place_within(first, rest, slow(line.clone())).await.is_none());
        let took = (Instant::now() - t2).as_secs_f64();
        // (its turn came within a second, then 5 s more)
        assert!((4.99..=6.01).contains(&took), "{took}");
    }

    #[tokio::test]
    async fn the_priority_follows_the_future() {
        assert_eq!(priority(), Priority::Background);
        with_priority(Priority::Request, async {
            assert_eq!(priority(), Priority::Request);
            with_priority(Priority::Import, async { assert_eq!(priority(), Priority::Import) }).await;
            assert_eq!(priority(), Priority::Request);
            // a spawned task is background work unless it says otherwise
            assert_eq!(tokio::spawn(async { priority() }).await.unwrap(), Priority::Background);
        }).await;
    }
}
