//! Keeps background work (cover thumbnails, quality checks, the translator) out of the way of people
//! using the app: every API call marks the server as in use, and background passes slow down to one
//! low-priority thread while it is (and for a few seconds after).
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

static START: LazyLock<Instant> = LazyLock::new(Instant::now);
/// Milliseconds since START of the last API call (0 = none yet).
static LAST: AtomicU64 = AtomicU64::new(0);

/// How long after the last call the server still counts as in use.
pub const QUIET_AFTER: Duration = Duration::from_secs(3);

fn ms() -> u64 {
    START.elapsed().as_millis() as u64 + 1
}

/// Marks the server as in use now.
pub fn touch() {
    LAST.store(ms(), Ordering::Relaxed);
}

/// Somebody used the app within `QUIET_AFTER`.
pub fn in_use() -> bool {
    used_within(LAST.load(Ordering::Relaxed), ms())
}

fn used_within(last: u64, now: u64) -> bool {
    last != 0 && now.saturating_sub(last) < QUIET_AFTER.as_millis() as u64
}

/// Middleware: API calls (and song streams) mark the server as in use.
pub async fn layer(req: Request, next: Next) -> Response {
    if req.uri().path().starts_with("/api/") {
        touch();
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

/// Waits (sleeping, a little at a time) while the app is in use; for the extra workers of a background
/// pass, so only one keeps going meanwhile. Gives up waiting after `max`.
pub fn wait_while_in_use(max: Duration) {
    let until = Instant::now() + max;
    while in_use() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_clears() {
        touch();
        assert!(in_use());
        let q = QUIET_AFTER.as_millis() as u64;
        assert!(used_within(1000, 1000 + q - 1));
        assert!(!used_within(1000, 1000 + q + 1));
        assert!(!used_within(0, 5));
    }
}
