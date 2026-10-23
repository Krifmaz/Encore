//! Answers that are slow to work out (a walk of the download folders and `df`, a program's `--version`, a
//! status call to slskd) kept and worked out again in the background: a request reads the last answer at
//! once, and when that is older than its period (or its stamp moved, see [`Refreshed::get_blocking_at`]) a fresh
//! one is made beside it for the next request. Only the first request (or one whose inputs changed: other
//! folders, another program file) waits for an answer, and requests that arrive together while there is none
//! wait for the same one.
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The kept answer and what it was made from.
struct Slot<T> {
    /// what the answer was made from (paths, a program file and its time); another key is another answer
    key: String,
    /// the answer and when it was made
    value: Option<(Instant, T)>,
    /// what the caller's stamp was when the answer was started (a library version): another stamp makes the
    /// kept answer old at once, but it is still shown while the fresh one is made
    stamp: u64,
    /// a background refresh is under way
    running: bool,
}

/// One kept answer. Clones share it.
pub struct Refreshed<T> {
    /// the kept answer, shared by the clones
    slot: Arc<Mutex<Slot<T>>>,
    /// held while the first answer for a key is made, so callers that arrive together make it once
    first: Arc<tokio::sync::Mutex<()>>,
    /// how long an answer stays fresh
    period: Duration,
}

impl<T> Clone for Refreshed<T> {
    fn clone(&self) -> Self {
        Refreshed { slot: self.slot.clone(), first: self.first.clone(), period: self.period }
    }
}

/// Clears the running mark when a background refresh ends, also when it panicked.
struct Running<T>(Arc<Mutex<Slot<T>>>);

impl<T> Drop for Running<T> {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).running = false;
    }
}

impl<T: Clone + Send + 'static> Refreshed<T> {
    /// An answer that is made again in the background once it is older than `period`.
    pub fn new(period: Duration) -> Self {
        Refreshed { slot: Arc::new(Mutex::new(Slot { key: String::new(), value: None, stamp: 0, running: false })), first: Default::default(), period }
    }

    /// The slot; a poisoned lock is taken over.
    fn lock(&self) -> std::sync::MutexGuard<'_, Slot<T>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The answer for `key`: the kept one at once (a fresh one is made in the background when it is older
    /// than the period), or, when there is none for this key yet, the one `make` gives now.
    pub async fn get<F, Fut>(&self, key: &str, make: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T> + Send + 'static,
    {
        let fut = self.fetch(key, 0, move || {
            let f = make();
            async move { Some(f.await) }
        });
        match fut.await {
            Ok(v) => v,
            Err(()) => unreachable!("the answer is always made"),
        }
    }

    /// [`Refreshed::get`] for work that blocks (file system walks, programs): `make` runs on tokio's blocking
    /// pool. When it panics the kept answer stays (`T::default()` when there was none).
    pub async fn get_blocking<F>(&self, key: &str, make: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Default,
    {
        self.get_blocking_at(key, 0, make).await
    }

    /// [`Refreshed::get_blocking`] with a stamp (a library version): when the kept answer was started at
    /// another stamp it is shown once more and a fresh one is made in the background, as when it is older than
    /// the period. Only another key waits for its answer.
    pub async fn get_blocking_at<F>(&self, key: &str, stamp: u64, make: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Default,
    {
        self.fetch(key, stamp, move || async move { tokio::task::spawn_blocking(make).await.ok() }).await.unwrap_or_default()
    }

    /// The kept answer for `key` while it may be shown: (the answer, whether a refresh should start).
    fn kept(&self, key: &str, stamp: u64) -> Option<(T, bool)> {
        let mut s = self.lock();
        let (at, v) = s.value.as_ref().filter(|_| s.key == key)?;
        let v = v.clone();
        if (at.elapsed() < self.period && s.stamp == stamp) || s.running {
            return Some((v, false));
        }
        s.running = true;
        Some((v, true))
    }

    /// The kept answer, or `make`'s; a `make` that gives None (it failed) changes nothing and, when nothing
    /// was kept, is the Err.
    async fn fetch<F, Fut>(&self, key: &str, stamp: u64, make: F) -> Result<T, ()>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Option<T>> + Send + 'static,
    {
        let first = match self.kept(key, stamp) {
            Some((v, false)) => return Ok(v),
            Some((old, true)) => {
                let (slot, key, fut) = (self.slot.clone(), key.to_string(), make());
                tokio::spawn(async move {
                    let _running = Running(slot.clone());
                    let Some(v) = fut.await else { return };
                    let mut s = slot.lock().unwrap_or_else(|e| e.into_inner());
                    // a newer answer for other inputs is not replaced by this one
                    if s.key == key {
                        s.value = Some((Instant::now(), v));
                        s.stamp = stamp;
                    }
                });
                return Ok(old);
            }
            None => self.first.lock().await,
        };
        // another caller may have made it while this one waited for its turn
        if let Some((v, refresh)) = self.kept(key, stamp) {
            if refresh {
                self.lock().running = false;
            }
            return Ok(v);
        }
        let v = make().await.ok_or(())?;
        let mut s = self.lock();
        s.key = key.to_string();
        s.value = Some((Instant::now(), v.clone()));
        s.stamp = stamp;
        drop(s);
        drop(first);
        Ok(v)
    }

    /// How old the kept answer is (None: none yet), for `/api/admin/perf`.
    pub fn age(&self) -> Option<Duration> {
        self.lock().value.as_ref().map(|(at, _)| at.elapsed())
    }

    /// Forget the kept answer: the next request waits for a fresh one (after a change the caller knows of).
    pub fn forget(&self) {
        self.lock().value = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn the_kept_answer_comes_at_once_and_is_made_again_behind_it() {
        let r: Refreshed<usize> = Refreshed::new(Duration::from_millis(50));
        let calls = Arc::new(AtomicUsize::new(0));
        let make = |calls: Arc<AtomicUsize>| move || async move { calls.fetch_add(1, Ordering::SeqCst) + 1 };
        // the first one waits for its answer
        assert_eq!(r.get("k", make(calls.clone())).await, 1);
        // fresh: kept, nothing made
        assert_eq!(r.get("k", make(calls.clone())).await, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(60)).await;
        // old: the kept answer at once, a new one made in the background
        assert_eq!(r.get("k", make(calls.clone())).await, 1);
        for _ in 0..100 {
            if r.age().is_some_and(|a| a < Duration::from_millis(40)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(r.get("k", make(calls.clone())).await, 2);
        // other inputs: waits for an answer of its own
        assert_eq!(r.get("other", make(calls.clone())).await, 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        r.forget();
        assert!(r.age().is_none());
        assert_eq!(r.get_blocking("other", || 42).await, 42);
    }

    /// Callers that arrive together while there is no answer yet wait for one `make`, not one each.
    #[tokio::test]
    async fn the_first_answer_is_made_once_for_callers_that_arrive_together() {
        let r: Refreshed<usize> = Refreshed::new(Duration::from_secs(60));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut joins = vec![];
        for _ in 0..5 {
            let (r, calls) = (r.clone(), calls.clone());
            joins.push(tokio::spawn(async move {
                r.get_blocking("k", move || {
                    std::thread::sleep(Duration::from_millis(50));
                    calls.fetch_add(1, Ordering::SeqCst) + 1
                }).await
            }));
        }
        for j in joins {
            assert_eq!(j.await.unwrap(), 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Another stamp shows the kept answer once more and makes a fresh one behind it; another key waits.
    #[tokio::test]
    async fn another_stamp_refreshes_in_the_background() {
        let r: Refreshed<usize> = Refreshed::new(Duration::from_secs(60));
        assert_eq!(r.get_blocking_at("k", 1, || 10).await, 10);
        assert_eq!(r.get_blocking_at("k", 1, || 11).await, 10, "same stamp, fresh: kept");
        assert_eq!(r.get_blocking_at("k", 2, || 12).await, 10, "another stamp: the kept one at once");
        for _ in 0..100 {
            if r.get_blocking_at("k", 2, || 13).await == 12 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(r.get_blocking_at("k", 2, || 14).await, 12, "made behind it, with the new stamp");
        assert_eq!(r.get_blocking_at("other", 2, || 20).await, 20, "another key waits for its own");
    }

    #[tokio::test]
    async fn a_background_refresh_runs_once_at_a_time() {
        let r: Refreshed<usize> = Refreshed::new(Duration::from_millis(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let slow = |calls: Arc<AtomicUsize>| move || async move {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            7
        };
        r.get("k", slow(calls.clone())).await;
        for _ in 0..10 {
            assert_eq!(r.get("k", slow(calls.clone())).await, 7);
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the first answer and one refresh behind it");
        // a refresh that panics leaves the kept answer and lets the next one run
        let r2: Refreshed<usize> = Refreshed::new(Duration::from_millis(0));
        r2.get_blocking("k", || 5).await;
        assert_eq!(r2.get_blocking("k", || panic!("broken")).await, 5);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(r2.get_blocking("k", || 6).await, 5);
    }
}
