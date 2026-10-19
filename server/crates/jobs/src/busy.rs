//! Keeps background work (cover thumbnails, quality checks, the translator) out of the way of people
//! using the app: every API call marks the server as in use, and background passes slow down to one
//! low-priority thread while it is (and for a few seconds after).
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long after the last call the server still counts as in use.
pub const QUIET_AFTER: Duration = Duration::from_secs(3);

/// When the app was last used. Clones share the same clock.
#[derive(Clone)]
pub struct Busy(Arc<Clock>);

struct Clock {
    start: Instant,
    /// milliseconds since `start` of the last API call (0 = none yet)
    last: AtomicU64,
}

impl Default for Busy {
    fn default() -> Self {
        Busy(Arc::new(Clock { start: Instant::now(), last: AtomicU64::new(0) }))
    }
}

impl Busy {
    fn ms(&self) -> u64 {
        self.0.start.elapsed().as_millis() as u64 + 1
    }

    /// Marks the server as in use now.
    pub fn touch(&self) {
        self.0.last.store(self.ms(), Ordering::Relaxed);
    }

    /// Somebody used the app within `QUIET_AFTER`.
    pub fn in_use(&self) -> bool {
        used_within(self.0.last.load(Ordering::Relaxed), self.ms())
    }

    /// Waits (sleeping, a little at a time) while the app is in use; for the extra workers of a background
    /// pass, so only one keeps going meanwhile. Gives up waiting after `max`.
    pub fn wait_while_in_use(&self, max: Duration) {
        let until = Instant::now() + max;
        while self.in_use() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

fn used_within(last: u64, now: u64) -> bool {
    last != 0 && now.saturating_sub(last) < QUIET_AFTER.as_millis() as u64
}

/// Middleware: API calls (and song streams) mark the server as in use.
pub async fn layer(State(busy): State<Busy>, req: Request, next: Next) -> Response {
    if req.uri().path().starts_with("/api/") {
        busy.touch();
    }
    next.run(req).await
}

/// This thread at nice 10 and idle disk priority: it only gets the CPU and the disk when nothing else
/// wants them. (Linux; elsewhere nothing changes.)
pub fn lower_this_thread() {
    #[cfg(target_os = "linux")]
    unsafe {
        // on Linux the nice value and the I/O priority belong to the thread
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS as _, tid, 10);
        idle_io(tid as i32);
    }
}

/// For a child process (call in `pre_exec`): idle disk priority.
pub fn idle_io_for_child() {
    #[cfg(target_os = "linux")]
    unsafe {
        idle_io(0)
    }
}

#[cfg(target_os = "linux")]
unsafe fn idle_io(who: i32) {
    const IOPRIO_WHO_PROCESS: libc::c_long = 1;
    const IOPRIO_CLASS_IDLE: libc::c_long = 3;
    libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, who as libc::c_long, IOPRIO_CLASS_IDLE << 13);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_clears() {
        let busy = Busy::default();
        assert!(!busy.in_use(), "nobody has used a new server yet");
        busy.touch();
        assert!(busy.in_use());
        assert!(!Busy::default().in_use(), "each server has its own clock");
        let q = QUIET_AFTER.as_millis() as u64;
        assert!(used_within(1000, 1000 + q - 1));
        assert!(!used_within(1000, 1000 + q + 1));
        assert!(!used_within(0, 5));
    }
}
