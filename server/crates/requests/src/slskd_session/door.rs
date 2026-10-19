//! The door to the Soulseek server. slskd reaches the server only through Encore – a relay on loopback
//! (`slskd_managed`, on the direct route) or Encore's SOCKS proxy into the VPN (`encore_vpn::socks`) –
//! and the door lets a connection through only while one of Encore's own attempts is under way.
//!
//! slskd 0.26 starts its own reconnect loop again by itself: at once after a session drops, and at once
//! after a connection under way is cancelled (a DELETE /api/v0/server while it is still connecting
//! counts as an ordinary disconnect, not as "by the user"). Behind a shut door those attempts fail on
//! loopback within milliseconds ("Connection reset by peer", "connection refused") and never reach the
//! server, so the server only ever sees the attempts of Encore's schedule.
use super::Session;
use std::time::Duration;
use tokio::time::Instant;

/// How long an open door waits for slskd's connection.
pub const DOOR_WINDOW: Duration = Duration::from_secs(15);

#[derive(Default, Debug)]
pub(super) struct DoorState {
    /// the door lets the next connection through until then
    open_until: Option<Instant>,
    /// connections let through that are still open
    live: usize,
    /// when a connection last went through
    passed_at: Option<Instant>,
    /// when the last connection let through closed
    closed_at: Option<Instant>,
    /// connections let through, and turned away, since the server started
    passed: u64,
    refused: u64,
}

/// One connection through the door: open as long as this lives.
pub struct Pass(Session);

impl Drop for Pass {
    fn drop(&mut self) {
        {
            let mut d = self.0.door_state();
            d.live = d.live.saturating_sub(1);
            d.closed_at = Some(Instant::now());
        }
        // the session may have ended with it: the supervisor reads slskd's state at its next round
        self.0.conn().checked = None;
        self.0.wake();
    }
}

/// What the door has seen (for the supervisor and the tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DoorSeen {
    /// when a connection last went through
    pub passed_at: Option<Instant>,
    /// when the last connection let through closed
    pub closed_at: Option<Instant>,
    /// connections let through that are still open
    pub live: usize,
    /// connections let through, and turned away, since the server started
    pub passed: u64,
    pub refused: u64,
}

impl Session {
    fn door_state(&self) -> std::sync::MutexGuard<'_, DoorState> {
        self.0.door.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Let the next connection to the Soulseek server through (for [`DOOR_WINDOW`]).
    pub fn open_door(&self) {
        self.door_state().open_until = Some(Instant::now() + DOOR_WINDOW);
    }

    /// Let nothing more through (a connection already through stays open).
    pub fn shut_door(&self) {
        self.door_state().open_until = None;
    }

    /// slskd connects to the Soulseek server: a [`Pass`] (keep it while the connection lasts) when
    /// Encore's attempt is under way, else None (turn the connection away).
    pub fn door(&self) -> Option<Pass> {
        let now = Instant::now();
        let mut d = self.door_state();
        if d.open_until.take().is_some_and(|t| t > now) {
            d.live += 1;
            d.passed += 1;
            d.passed_at = Some(now);
            drop(d);
            return Some(Pass(self.clone()));
        }
        d.refused += 1;
        None
    }

    /// What the door has seen.
    pub fn door_seen(&self) -> DoorSeen {
        let d = self.door_state();
        DoorSeen { passed_at: d.passed_at, closed_at: d.closed_at, live: d.live, passed: d.passed, refused: d.refused }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One connection per opening, only within the window; a pass counts as open until dropped.
    #[tokio::test(start_paused = true)]
    async fn one_connection_per_attempt() {
        let s = Session::default();
        assert!(s.door().is_none(), "shut until Encore connects");
        s.open_door();
        let p = s.door().expect("open: one through");
        assert!(s.door().is_none(), "only one");
        assert_eq!((s.door_seen().live, s.door_seen().passed, s.door_seen().refused), (1, 1, 2));
        drop(p);
        assert_eq!(s.door_seen().live, 0);
        assert!(s.door_seen().closed_at.is_some());
        s.open_door();
        tokio::time::sleep(DOOR_WINDOW + Duration::from_millis(1)).await;
        assert!(s.door().is_none(), "the window passed");
        s.open_door();
        s.shut_door();
        assert!(s.door().is_none(), "shut again");
    }
}
