//! The background loops that run as long as the server: the VPN tunnel, the Soulseek engine (slskd),
//! the YouTube sign-in, the translator, Roon, the request worker and the scheduler. Each crate gives its
//! loop as a [`Supervisor`]; the server starts them all with [`start`] and, before a restart, stops the
//! helper programs they run with [`stop_all`].
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A future a supervisor hands out (boxed, so supervisors of different crates fit in one list).
pub type Task = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// One background loop.
pub trait Supervisor: Send + Sync + 'static {
    /// Its name in the log.
    fn name(&self) -> &'static str;
    /// The loop itself; it runs until the server stops.
    fn run(&self) -> Task;
    /// Called once before the server restarts: stop the helper program it runs, if any.
    fn stop(&self) -> Task {
        Box::pin(async {})
    }
}

/// A supervisor made of two closures: the loop, and what stops its helper program.
pub struct Loop<R, S> {
    name: &'static str,
    run: R,
    stop: S,
}

/// A supervisor whose loop is `run()` (called once) and that runs no helper program.
pub fn looping<R, F>(name: &'static str, run: R) -> Arc<dyn Supervisor>
where
    R: Fn() -> F + Send + Sync + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    Arc::new(Loop { name, run, stop: || async {} })
}

/// A supervisor whose loop is `run()` and whose helper program `stop()` ends.
pub fn with_stop<R, F, S, G>(name: &'static str, run: R, stop: S) -> Arc<dyn Supervisor>
where
    R: Fn() -> F + Send + Sync + 'static,
    F: Future<Output = ()> + Send + 'static,
    S: Fn() -> G + Send + Sync + 'static,
    G: Future<Output = ()> + Send + 'static,
{
    Arc::new(Loop { name, run, stop })
}

impl<R, F, S, G> Supervisor for Loop<R, S>
where
    R: Fn() -> F + Send + Sync + 'static,
    F: Future<Output = ()> + Send + 'static,
    S: Fn() -> G + Send + Sync + 'static,
    G: Future<Output = ()> + Send + 'static,
{
    fn name(&self) -> &'static str {
        self.name
    }
    fn run(&self) -> Task {
        Box::pin((self.run)())
    }
    fn stop(&self) -> Task {
        Box::pin((self.stop)())
    }
}

/// Starts every loop on the runtime.
pub fn start(all: &[Arc<dyn Supervisor>]) {
    for s in all {
        tracing::debug!("starting {}", s.name());
        tokio::spawn(s.run());
    }
}

/// Stops the helper programs of every loop, one after the other.
pub async fn stop_all(all: &[Arc<dyn Supervisor>]) {
    for s in all {
        s.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn loops_start_and_their_programs_stop() {
        let runs = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let (r, s) = (runs.clone(), stops.clone());
        let all = vec![
            with_stop("one", move || { let r = r.clone(); async move { r.fetch_add(1, Ordering::SeqCst); } },
                      move || { let s = s.clone(); async move { s.fetch_add(1, Ordering::SeqCst); } }),
            looping("two", || async {}),
        ];
        assert_eq!(all.iter().map(|s| s.name()).collect::<Vec<_>>(), ["one", "two"]);
        start(&all);
        for _ in 0..100 {
            if runs.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        stop_all(&all).await;
        assert_eq!(stops.load(Ordering::SeqCst), 1, "only the loop with a program has something to stop");
    }
}
