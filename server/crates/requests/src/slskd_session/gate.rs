//! The search gate: one Soulseek search at a time, each starting at least 3 s after the one before, none
//! while the session is down, and 60 s of quiet when the server (or slskd) turns a search away.
//!
//! People go first: the request worker's searches line up behind one another before they wait for the
//! turn, so a person's search waits at most for the search that is running (and gives up after 30 s). The
//! worker's line is the Soulseek line of [`crate::line::Lines`], keyed by request id: Download now moves a
//! request to its front, and the Requests page says where each one stands ("2nd in line for Soulseek").
use super::{clock, unix_at, Busy, NotConnected, Session};
use crate::Ctx;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// At least this long between the starts of two searches.
pub const SPACING: Duration = Duration::from_secs(3);
/// How long searches pause after a "too many searches" answer.
pub const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(60);
/// A request's search tries again after that many pauses, then fails.
pub const RATE_LIMIT_TRIES: u32 = 5;
/// A person waits this long for the turn at most.
pub const PERSON_WAIT: Duration = Duration::from_secs(30);

/// Who is searching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// the request worker: waits its turn behind other searches, and through a rate-limit pause
    Worker,
    /// someone on the picker or the Soulseek page: goes first, and is told at once when it can't run
    Person,
}

/// Lets one Soulseek search run at a time, spaced out, people before the worker.
pub(super) struct Limiter {
    /// the one search that may run
    turn: Arc<Semaphore>,
    /// when the last search started
    last_start: Mutex<Option<Instant>>,
    /// searches waiting for the turn
    queued: Arc<AtomicUsize>,
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter { turn: Arc::new(Semaphore::new(1)), last_start: Mutex::new(None), queued: Default::default() }
    }
}

impl Limiter {
    /// A search runs now.
    pub(super) fn busy(&self) -> bool {
        self.turn.available_permits() == 0
    }

    /// Searches waiting for their turn.
    pub(super) fn queued(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }
}

/// The right to search, held until the search is over (dropping it – also when the task is cancelled –
/// lets the next one go).
pub struct Turn {
    /// the search's turn: the next one waits until it goes
    _turn: OwnedSemaphorePermit,
    /// the worker's slot of the Soulseek line (at most one worker search stands in front of a person)
    _line: Option<crate::line::Slot>,
    /// a person's search starts by then or not at all (30 s after it asked)
    deadline: Option<Instant>,
}

impl Turn {
    /// A person's search: the moment it gives up waiting (the turn, a rate-limit pause, the 3 s spacing
    /// all count against the same 30 s).
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

/// Counts a search as waiting while it lives.
struct Queued(Arc<AtomicUsize>);

impl Queued {
    /// Count one more waiting search (one less when dropped).
    fn new(n: &Arc<AtomicUsize>) -> Queued {
        n.fetch_add(1, Ordering::SeqCst);
        Queued(n.clone())
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Searches of the worker made for no request (tests, older callers): a key of their own in the line.
static ANON: AtomicI64 = AtomicI64::new(0);

impl Session {
    /// Wait for the turn to search: never while the session is down (NotConnected at once), a person
    /// for at most 30 s (Busy) – counted from here to the moment the search is sent. The session is
    /// looked at again once the turn is there. The worker's search for request `rid` waits in the Soulseek
    /// line first: its row reads "In line for Soulseek" (status `queued`) while it does.
    pub async fn turn(&self, st: &impl Ctx, who: Who, rid: Option<i64>) -> anyhow::Result<Turn> {
        let deadline = (who == Who::Person).then(|| Instant::now() + PERSON_WAIT);
        self.check(st).await?;
        if let Some(d) = deadline {
            self.rate_limit_for_a_person(d)?;
            // the search holding the turn sits out the server's pause, and goes first after it
            if self.searching() {
                if let Some(until) = self.rate_limited_until() {
                    return Err(rate_limited_busy(until).into());
                }
            }
        }
        let g = &self.0.gate;
        let turn = {
            let _q = Queued::new(&g.queued);
            match deadline {
                None => {
                    let lines = &st.requests().lines;
                    let key = rid.unwrap_or_else(|| -1 - ANON.fetch_add(1, Ordering::SeqCst));
                    let mut waited = false;
                    let line = lines.soulseek.acquire(key, crate::line::SOULSEEK_LABEL, &lines.pressed, || {
                        waited = true;
                        if let Some(id) = rid {
                            crate::worker::now::in_line_row(st, id, crate::line::SOULSEEK_LABEL);
                        }
                    }).await;
                    if let (true, Some(id)) = (waited, rid) {
                        crate::worker::now::left_line_row(st, id, "searching", "Waiting for another Soulseek search…");
                    }
                    let turn = g.turn.clone().acquire_owned().await?;
                    Turn { _turn: turn, _line: Some(line), deadline: None }
                }
                Some(d) => match tokio::time::timeout_at(d, g.turn.clone().acquire_owned()).await {
                    Ok(t) => Turn { _turn: t?, _line: None, deadline },
                    Err(_) => return Err(self.busy_for_a_person().into()),
                },
            }
        };
        self.check(st).await?;
        Ok(turn)
    }

    /// Just before a search is sent (holding the turn): wait out a rate-limit pause (a person only when it
    /// ends before the person's `deadline`, else Busy) and the 3 s since the last start, make sure the
    /// session is still there, and note the start.
    pub async fn before_search(&self, st: &impl Ctx, deadline: Option<Instant>) -> anyhow::Result<()> {
        while let Some(until) = self.rate_limited_until() {
            if let Some(d) = deadline {
                self.rate_limit_for_a_person(d)?;
            }
            tokio::time::sleep_until(until).await;
        }
        let last = *self.0.gate.last_start.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = last.map(|t| t + SPACING).filter(|t| *t > Instant::now()) {
            if deadline.is_some_and(|d| t > d) {
                return Err(self.busy_for_a_person().into());
            }
            tokio::time::sleep_until(t).await;
        }
        self.check(st).await?;
        *self.0.gate.last_start.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        self.conn().last_search_at = Some(encore_core::time::now());
        Ok(())
    }

    /// Searches are paused until then (the server turned one away).
    pub fn rate_limited_until(&self) -> Option<Instant> {
        self.conn().rate_limited_until.filter(|t| *t > Instant::now())
    }

    /// A person isn't kept waiting through a rate-limit pause that ends after their `deadline`.
    fn rate_limit_for_a_person(&self, deadline: Instant) -> Result<(), Busy> {
        match self.rate_limited_until().filter(|t| *t > deadline) {
            Some(until) => Err(rate_limited_busy(until)),
            None => Ok(()),
        }
    }

    /// Why a person's search can't start within its 30 s: the server's pause when there is one, else the
    /// search that holds the turn.
    fn busy_for_a_person(&self) -> Busy {
        match self.rate_limited_until() {
            Some(until) => rate_limited_busy(until),
            None => Busy("Soulseek is busy with another search – try again in a moment".into()),
        }
    }

    /// The server (or slskd) turned a search away for being too many: searches pause for 60 s. Returns
    /// the error a person gets.
    pub fn rate_limited(&self) -> Busy {
        let until = Instant::now() + RATE_LIMIT_PAUSE;
        self.conn().rate_limited_until = Some(until);
        tracing::info!("soulseek: search rate limited – waiting {} s", RATE_LIMIT_PAUSE.as_secs());
        rate_limited_busy(until)
    }

    /// A search failed because the session is down: say so, and have the supervisor look.
    pub fn search_found_it_down(&self) -> NotConnected {
        self.lost();
        NotConnected(format!("Soulseek isn't connected – {}", super::when_text(&self.conn())))
    }
}

/// "Rate limited by the Soulseek server – next search at 14:05".
fn rate_limited_busy(until: Instant) -> Busy {
    Busy(format!("Rate limited by the Soulseek server – next search at {}", clock(unix_at(until))))
}
