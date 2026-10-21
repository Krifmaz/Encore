//! What a long step says it is at (encore_http::progress) written into its request's message, from one
//! place: a single task writes the latest words of every step running, all of them in one transaction on the
//! blocking pool, at most once a second (a step's end at once). Where the words are said nothing waits for
//! the database. They used
//! to be written there, on the threads that serve the API, and every waiter in MusicBrainz's line said its
//! wait every 5 s: 600 waiting lookups were 120 writes a second, and while a backup held the database every
//! one of those threads stopped behind it.
//!
//! A write only lands on the message the step last saw in the row (the one it started with, or its own last
//! words), and only while the row has the step's status: once something else wrote one (the request
//! finished, say) the step's words stop. When the step ends its row gets the message it started with again,
//! and [`Scope::end`] returns once that is written, so no word of the step lands after it.
use encore_db::{now, p, Db, RowExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

/// The writes of words (not of a step's end) are at least this far apart.
const BATCH_EVERY: Duration = Duration::from_secs(1);
/// How long a step's end waits for its last write at most (a backup can hold the database for minutes).
const END_WAIT: Duration = Duration::from_secs(30);

struct Entry {
    id: i64,
    status: String,
    /// the message the row had when the step began: written again when it ends
    before: String,
    /// the message the row has, as far as the step knows
    have: String,
    /// what it should say now
    want: String,
    /// the step is over
    done: bool,
    /// a write found another message in the row: the step's words stop
    lost: bool,
    /// told once the step's last write is done
    ended: Arc<Notify>,
}

#[derive(Default)]
struct Inner {
    entries: Mutex<HashMap<u64, Entry>>,
    next: Mutex<u64>,
    wake: Notify,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// The steps running now and the task that writes their words. Clones share them.
#[derive(Clone, Default)]
pub struct Steps(Arc<Inner>);

/// A running step's words ([`Steps::begin`]). Dropped without [`Scope::end`] (its request's task was
/// stopped), the step still ends: the row gets its message back unless something else wrote one.
pub(super) struct Scope {
    steps: Steps,
    key: u64,
    ended: Arc<Notify>,
    finished: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Steps {
    /// A step of request `id` begins while its row has `status` and the message `before`.
    pub(super) fn begin(&self, db: &Db, id: i64, status: &str, before: &str) -> Scope {
        let key = {
            let mut n = lock(&self.0.next);
            *n += 1;
            *n
        };
        let ended = Arc::new(Notify::new());
        lock(&self.0.entries).insert(key, Entry { id, status: status.into(), before: before.into(), have: before.into(), want: before.into(),
                                                   done: false, lost: false, ended: ended.clone() });
        let mut task = lock(&self.0.task);
        if task.as_ref().is_none_or(|t| t.is_finished()) {
            *task = Some(tokio::spawn(run(self.0.clone(), db.clone())));
        }
        Scope { steps: self.clone(), key, ended, finished: false }
    }

    /// The step's row should say `words` now.
    fn say(&self, key: u64, words: String) {
        let changed = match lock(&self.0.entries).get_mut(&key) {
            Some(e) if !e.done && !e.lost && e.want != words => {
                e.want = words;
                true
            }
            _ => false,
        };
        if changed {
            self.0.wake.notify_one();
        }
    }

    fn finish(&self, key: u64) {
        if let Some(e) = lock(&self.0.entries).get_mut(&key) {
            e.want = e.before.clone();
            e.done = true;
        }
        self.0.wake.notify_one();
    }
}

impl Scope {
    /// What the step's sink hands its words to.
    pub(super) fn sayer(&self) -> impl Fn(String) + Send + Sync + 'static {
        let (steps, key) = (self.steps.clone(), self.key);
        move |w| steps.say(key, w)
    }

    /// The step is over: its row gets the message it had before (unless something else wrote one), and this
    /// returns once that is written.
    pub(super) async fn end(mut self) {
        self.finished = true;
        self.steps.finish(self.key);
        let _ = tokio::time::timeout(END_WAIT, self.ended.notified()).await;
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if !self.finished {
            self.steps.finish(self.key);
        }
    }
}

/// One write: (entry, request, status, the message expected in the row, the new one).
type Write = (u64, i64, String, String, String);

impl Inner {
    /// The writes due now: every ended step's last one, and the others' words when `batch_ok`. Ended steps
    /// whose row says what it should (or that lost their row to another message) are told and dropped.
    fn due(&self, batch_ok: bool) -> Vec<Write> {
        let mut entries = lock(&self.entries);
        let mut out = vec![];
        entries.retain(|key, e| {
            if e.want == e.have || e.lost {
                if e.done {
                    e.ended.notify_one();
                    return false;
                }
                return true;
            }
            if e.done || batch_ok {
                out.push((*key, e.id, e.status.clone(), e.have.clone(), e.want.clone()));
            }
            true
        });
        out
    }

    /// Whether some step's words wait for the next batch.
    fn waiting(&self) -> bool {
        lock(&self.entries).values().any(|e| e.want != e.have && !e.lost)
    }

    fn wrote(&self, writes: &[Write], landed: &[bool]) {
        let mut entries = lock(&self.entries);
        for ((key, .., want), ok) in writes.iter().zip(landed) {
            if let Some(e) = entries.get_mut(key) {
                if *ok {
                    e.have = want.clone();
                } else {
                    e.lost = true;
                }
            }
        }
    }
}

/// The writer: every step's latest words, in one transaction, on the blocking pool.
async fn run(inner: Arc<Inner>, db: Db) {
    let mut batch_at = Instant::now();
    loop {
        let writes = inner.due(Instant::now() >= batch_at);
        if writes.is_empty() {
            if inner.waiting() {
                let _ = tokio::time::timeout_at(batch_at, inner.wake.notified()).await;
            } else {
                inner.wake.notified().await;
            }
            continue;
        }
        let todo: Vec<(i64, String, String, String)> = writes.iter().map(|(_, id, status, have, want)| (*id, status.clone(), have.clone(), want.clone())).collect();
        let n = todo.len();
        let landed: Vec<bool> = db.write(move |c| c.transaction(|c| {
            let t = now();
            todo.iter().map(|(id, status, have, want)| {
                c.execute("UPDATE requests SET message=?, updated_at=? WHERE id=? AND status=? AND COALESCE(message, '')=?", p![want, t, id, status, have])?;
                Ok(c.one("SELECT changes() AS n", p![])?.is_some_and(|r| r.i("n") > 0))
            }).collect::<rusqlite::Result<Vec<bool>>>()
        })).await.unwrap_or_else(|_: rusqlite::Error| vec![false; n]);
        inner.wrote(&writes, &landed);
        batch_at = Instant::now() + BATCH_EVERY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn db() -> (Db, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("encore-steps-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        (Db::open(&dir).unwrap(), dir)
    }

    fn request(db: &Db, status: &str, message: &str) -> i64 {
        db.execute("INSERT INTO requests (kind, artist, title, status, message, created_at, updated_at) VALUES ('track','Band','Song',?,?,?,?)",
            p![status, message, now(), now()]).unwrap()
    }

    fn message(db: &Db, id: i64) -> String {
        db.one("SELECT message FROM requests WHERE id=?", p![id]).unwrap().unwrap().s("message")
    }

    /// Until request `id` says `want` (at most 5 s).
    async fn says(db: &Db, id: i64, want: &str) {
        let t0 = Instant::now();
        while message(db, id) != want {
            assert!(t0.elapsed() < Duration::from_secs(5), "{id} says {:?}, not {want:?}", message(db, id));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A step's words reach its row from the writer task – the latest of them, at most once a second – and the
    /// row says what it said before once the step has ended; a step whose task was stopped ends too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_words_are_written_from_one_place_and_the_message_comes_back() {
        let (db, dir) = db();
        let steps = Steps::default();
        let id = request(&db, "importing", "Importing…");
        let scope = steps.begin(&db, id, "importing", "Importing…");
        let say = scope.sayer();
        say("Importing – checking the audio".into());
        says(&db, id, "Importing – checking the audio").await;
        // three more at once: the last of them is written, a second after the first words
        let t0 = Instant::now();
        for w in ["Importing – a", "Importing – b", "Importing – looking it up on MusicBrainz"] {
            say(w.into());
        }
        says(&db, id, "Importing – looking it up on MusicBrainz").await;
        assert!(t0.elapsed() >= Duration::from_millis(900), "{:?}", t0.elapsed());
        // the end: the message it had, written before `end` returns
        scope.end().await;
        assert_eq!(message(&db, id), "Importing…");
        assert!(lock(&steps.0.entries).is_empty());

        // a step whose task is stopped (its future dropped) ends as well
        let scope = steps.begin(&db, id, "importing", "Importing…");
        scope.sayer()("Importing – moving into the library".into());
        says(&db, id, "Importing – moving into the library").await;
        drop(scope);
        says(&db, id, "Importing…").await;
        entries_gone(&steps).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn entries_gone(steps: &Steps) {
        let t0 = Instant::now();
        while !lock(&steps.0.entries).is_empty() {
            assert!(t0.elapsed() < Duration::from_secs(5));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Once something else wrote the message (the request finished), or the row has another status, the step's
    /// words never land on it, its end included.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_words_never_land_on_another_message() {
        let (db, dir) = db();
        let steps = Steps::default();
        let id = request(&db, "importing", "Importing…");
        let scope = steps.begin(&db, id, "importing", "Importing…");
        let say = scope.sayer();
        say("Importing – checking the audio".into());
        says(&db, id, "Importing – checking the audio").await;
        db.execute("UPDATE requests SET status='completed', message='Downloaded from Soulseek' WHERE id=?", p![id]).unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        say("Importing – finding the cover".into());
        tokio::time::sleep(Duration::from_millis(1100)).await;
        scope.end().await;
        assert_eq!(message(&db, id), "Downloaded from Soulseek");
        // another status from the start: nothing is written
        let other = request(&db, "searching", "Looking up which album this song is on…");
        let scope = steps.begin(&db, other, "importing", "Looking up which album this song is on…");
        scope.sayer()("Importing – reading tags".into());
        tokio::time::sleep(Duration::from_millis(1100)).await;
        scope.end().await;
        assert_eq!(message(&db, other), "Looking up which album this song is on…");
        // and a row whose message was NULL is one with no message
        db.execute("UPDATE requests SET message=NULL, status='searching' WHERE id=?", p![id]).unwrap();
        let scope = steps.begin(&db, id, "searching", "");
        scope.sayer()("Looking up discography – waiting for MusicBrainz (next in line)".into());
        says(&db, id, "Looking up discography – waiting for MusicBrainz (next in line)").await;
        scope.end().await;
        assert_eq!(message(&db, id), "");
        let _ = std::fs::remove_dir_all(dir);
    }
}
