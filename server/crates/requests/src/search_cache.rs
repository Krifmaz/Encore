//! Soulseek and indexer answers kept for the picker, so opening it again shows what was found at once
//! ("Found at 14:02 · Search again") instead of searching Soulseek or every indexer again.
//!
//! The answers live in a small SQLite file of their own, `<config>/cache/search-cache.db`, not in the
//! database: backups copy the whole database, and a new table there would need a schema version. The file
//! can be deleted at any time; `CREATE … IF NOT EXISTS` makes it again. It is opened on first use and only
//! used on the blocking pool.
//!
//! - Writers: every Soulseek search that ends well (the request worker's, the picker's: see
//!   `sources::slskd::search_kept`), the Soulseek page when its search is over, and the picker's Prowlarr
//!   searches. Errors and cut-short searches are never written.
//! - Reader: the picker. The worker always searches again (its downloads start from fresh answers).
//! - Kept: answers with results for 24 hours, "nothing found" for an hour; at most 32 MB in all (the oldest
//!   go first), and an answer bigger than 2 MB compressed is not kept.
//! - When the file can't be opened or breaks, the cache is off until the next start (one warning), and
//!   every search runs as it would without it. A damaged file is set aside and started again.
use crate::Ctx;
use encore_core::audio::DOWNLOAD_EXTS;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// How long an answer with results is kept (seconds).
pub const KEEP: f64 = 24.0 * 3600.0;
/// How long "nothing found" is kept: who is online on Soulseek changes during the day.
pub const KEEP_EMPTY: f64 = 3600.0;
/// The kept answers' compressed size in all, at most.
pub const MAX_BYTES: i64 = 32 * 1024 * 1024;
/// A bigger answer (compressed) is not kept.
pub const MAX_ENTRY: usize = 2 * 1024 * 1024;
/// The file's name in `<config>/cache/`.
pub const FILE: &str = "search-cache.db";

/// What the Soulseek answers keep: per user, and per audio file.
const ANSWER_KEYS: [&str; 4] = ["username", "hasFreeUploadSlot", "uploadSpeed", "queueLength"];
/// What a kept audio file keeps (slskd's names).
const FILE_KEYS: [&str; 6] = ["filename", "size", "bitRate", "bitDepth", "sampleRate", "length"];
/// What an indexer release keeps (Prowlarr's names).
const RELEASE_KEYS: [&str; 9] = ["title", "guid", "size", "seeders", "age", "indexer", "protocol", "magnetUrl", "downloadUrl"];

/// The kept file under the config folder `data_dir`.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join(FILE)
}

/// A kept answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// the key it is kept under
    pub key: String,
    /// the text it was searched with
    pub query: String,
    /// unix seconds when the search ended
    pub searched_at: f64,
    /// the request whose attempt or picker made it (the latest writer), when known
    pub request_id: Option<i64>,
    /// Soulseek: slskd's answers ([`slim_soulseek`]); indexers: Prowlarr's releases ([`slim_releases`])
    pub results: Vec<Value>,
}

/// Why a live search shared by several callers failed, in a form each of them can have a copy of; turned
/// back into the typed errors with [`Failure::into_error`], so a person still gets 409 or 429.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// slskd has no Soulseek session ([`crate::slskd_session::NotConnected`])
    NotConnected(String),
    /// slskd is busy with another search ([`crate::slskd_session::Busy`])
    Busy(String),
    /// any other error, in words
    Other(String),
}

impl Failure {
    /// The shareable form of an error.
    pub fn of(e: &anyhow::Error) -> Failure {
        if let Some(n) = e.downcast_ref::<crate::slskd_session::NotConnected>() {
            return Failure::NotConnected(n.0.clone());
        }
        if let Some(b) = e.downcast_ref::<crate::slskd_session::Busy>() {
            return Failure::Busy(b.0.clone());
        }
        Failure::Other(format!("{e:#}"))
    }

    /// The typed error again.
    pub fn into_error(self) -> anyhow::Error {
        match self {
            Failure::NotConnected(m) => crate::slskd_session::NotConnected(m).into(),
            Failure::Busy(m) => crate::slskd_session::Busy(m).into(),
            Failure::Other(m) => anyhow::anyhow!(m),
        }
    }
}

/// A live search several callers can wait on.
type Flight = Shared<BoxFuture<'static, Result<Arc<Found>, Failure>>>;

/// The kept answers, and the live searches running for the picker. Clones share it.
#[derive(Clone, Default)]
pub struct Cache(Arc<Inner>);

/// The shared state behind [`Cache`].
#[derive(Default)]
struct Inner {
    /// the SQLite file, opened on first use
    file: Mutex<File>,
    /// one live search per key: a second caller waits for the first one's answer
    flights: Mutex<HashMap<String, (u64, Flight)>>,
    /// numbers the flights, so a finished one removes only itself
    next: AtomicU64,
    /// limits other than the constants (tests)
    limits: Mutex<Option<(i64, usize)>>,
}

/// The cache file's state.
#[derive(Default)]
enum File {
    /// not opened yet
    #[default]
    Closed,
    /// open
    Open {
        /// the connection
        conn: Connection,
        /// the folder the file is in
        dir: PathBuf,
    },
    /// it could not be opened or broke: off until the next start
    Off,
}

/// The cache table: per key its kind, query, searched time, request and gzipped answers.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS search_cache (
  key         TEXT PRIMARY KEY,
  kind        TEXT NOT NULL,
  query       TEXT NOT NULL,
  request_id  INTEGER,
  searched_at REAL NOT NULL,
  count       INTEGER NOT NULL,
  bytes       INTEGER NOT NULL,
  results     BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS search_cache_request ON search_cache(request_id, searched_at);
CREATE INDEX IF NOT EXISTS search_cache_age ON search_cache(searched_at);";

/// Whether an SQLite error means the file is damaged (and should be replaced).
fn damaged(e: &rusqlite::Error) -> bool {
    matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase))
}

/// Open the cache file: a 2 s busy timeout, synchronous=NORMAL, and the schema.
fn open_at(file: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(file)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Open (or make) the file in `dir`; a damaged one is set aside (`search-cache.db.damaged`) and started
/// again.
fn open(dir: &Path) -> Result<Connection, String> {
    let file = path(dir);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    match open_at(&file) {
        Ok(c) => Ok(c),
        Err(e) if damaged(&e) => {
            let aside = file.with_extension("db.damaged");
            tracing::warn!("search cache: {} is damaged ({e}) – set aside as {} and started again", file.display(), aside.display());
            let _ = std::fs::rename(&file, &aside);
            let _ = std::fs::remove_file(file.with_extension("db-journal"));
            open_at(&file).map_err(|e| format!("{}: {e}", file.display()))
        }
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// The answers gzipped as JSON.
fn gzip(results: &[Value]) -> Vec<u8> {
    let json = serde_json::to_vec(results).unwrap_or_else(|_| b"[]".to_vec());
    let mut enc = flate2::write::GzEncoder::new(Vec::with_capacity(json.len() / 4), flate2::Compression::default());
    let _ = enc.write_all(&json);
    enc.finish().unwrap_or_default()
}

/// The answers from their gzipped JSON; `None` when it doesn't read.
fn gunzip(blob: &[u8]) -> Option<Vec<Value>> {
    let mut json = vec![];
    flate2::read::GzDecoder::new(blob).read_to_end(&mut json).ok()?;
    serde_json::from_slice::<Value>(&json).ok()?.as_array().cloned()
}

/// Whether an entry of `count` answers that ended at `at` is still kept at `now`.
fn fresh(count: i64, at: f64, now: f64) -> bool {
    now - at < if count > 0 { KEEP } else { KEEP_EMPTY }
}

impl Cache {
    /// The file's state, even after a panic poisoned the lock.
    fn file(&self) -> MutexGuard<'_, File> {
        self.0.file.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The size limits: (whole file in bytes, one entry).
    fn limits(&self) -> (i64, usize) {
        self.0.limits.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or((MAX_BYTES, MAX_ENTRY))
    }

    /// Other limits than [`MAX_BYTES`] and [`MAX_ENTRY`] (tests).
    pub fn set_limits(&self, max_bytes: i64, max_entry: usize) {
        *self.0.limits.lock().unwrap_or_else(|e| e.into_inner()) = Some((max_bytes, max_entry));
    }

    /// Run `f` on the file in `dir` (opened first when needed). None when the cache is off, or `f` failed:
    /// a failure says so once and turns the cache off (a damaged file is set aside and opened afresh next
    /// time instead).
    fn with<T>(&self, dir: &Path, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Option<T> {
        let mut file = self.file();
        if matches!(&*file, File::Open { dir: d, .. } if d != dir) {
            *file = File::Closed;
        }
        if matches!(&*file, File::Closed) {
            *file = match open(dir) {
                Ok(conn) => File::Open { conn, dir: dir.to_path_buf() },
                Err(e) => {
                    tracing::warn!("search cache off until the next start (searches run as usual): {e}");
                    File::Off
                }
            };
        }
        let File::Open { conn, .. } = &mut *file else { return None };
        match f(conn) {
            Ok(v) => Some(v),
            Err(e) if damaged(&e) => {
                *file = File::Closed;
                let p = path(dir);
                let aside = p.with_extension("db.damaged");
                tracing::warn!("search cache: {} is damaged ({e}) – set aside as {} and started again", p.display(), aside.display());
                let _ = std::fs::rename(&p, &aside);
                None
            }
            Err(e) => {
                tracing::warn!("search cache off until the next start (searches run as usual): {}: {e}", path(dir).display());
                *file = File::Off;
                None
            }
        }
    }

    /// The kept answer for `key`, when it is still kept at `now`.
    pub fn get(&self, dir: &Path, key: &str, now: f64) -> Option<Found> {
        let row = self.with(dir, |c| {
            c.query_row("SELECT query, request_id, searched_at, count, results FROM search_cache WHERE key=?", params![key], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, f64>(2)?, r.get::<_, i64>(3)?, r.get::<_, Vec<u8>>(4)?))
            }).optional()
        })??;
        let (query, request_id, searched_at, count, blob) = row;
        if !fresh(count, searched_at, now) {
            return None;
        }
        let results = gunzip(&blob)?;
        Some(Found { key: key.to_string(), query, searched_at, request_id, results })
    }

    /// The newest kept answer **with results** of `kind` for a request: one written for `request_id` after
    /// `since` (the request's creation: ids of a restored database may be reused), or one under any of
    /// `keys` (the texts the request's own attempts search with).
    pub fn get_for_request(&self, dir: &Path, kind: &str, request_id: Option<i64>, keys: &[String], since: f64, now: f64) -> Option<Found> {
        let row = self.with(dir, |c| {
            let marks = vec!["?"; keys.len()].join(",");
            let by_key = if keys.is_empty() { "0".to_string() } else { format!("key IN ({marks})") };
            let sql = format!("SELECT key, query, request_id, searched_at, results FROM search_cache WHERE kind=? AND count>0 AND searched_at>? \
                AND ((request_id=? AND searched_at>=?) OR {by_key}) ORDER BY searched_at DESC LIMIT 1");
            let mut args: Vec<rusqlite::types::Value> = vec![kind.to_string().into(), (now - KEEP).into(), request_id.unwrap_or(-1).into(), since.into()];
            args.extend(keys.iter().map(|k| rusqlite::types::Value::from(k.clone())));
            c.query_row(&sql, rusqlite::params_from_iter(args), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, f64>(3)?, r.get::<_, Vec<u8>>(4)?))
            }).optional()
        })??;
        let (key, query, request_id, searched_at, blob) = row;
        Some(Found { key, query, searched_at, request_id, results: gunzip(&blob)? })
    }

    /// Keep `results` under `key` (in place of what was kept), ended at `now`. Expired entries go, then
    /// the oldest while the file holds more than 32 MB; an answer over 2 MB compressed is not kept (and
    /// what was kept under its key goes). Whether it was kept.
    #[allow(clippy::too_many_arguments)]
    pub fn put(&self, dir: &Path, kind: &str, key: &str, query: &str, request_id: Option<i64>, results: &[Value], now: f64) -> bool {
        let blob = gzip(results);
        let (max_bytes, max_entry) = self.limits();
        let too_big = blob.len() > max_entry;
        let kept = self.with(dir, |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM search_cache WHERE (count>0 AND searched_at<?) OR (count=0 AND searched_at<?)", params![now - KEEP, now - KEEP_EMPTY])?;
            if too_big {
                tx.execute("DELETE FROM search_cache WHERE key=?", params![key])?;
                tx.commit()?;
                return Ok(false);
            }
            tx.execute("INSERT OR REPLACE INTO search_cache (key, kind, query, request_id, searched_at, count, bytes, results) VALUES (?,?,?,?,?,?,?,?)",
                params![key, kind, query, request_id, now, results.len() as i64, blob.len() as i64, blob])?;
            let total: i64 = tx.query_row("SELECT COALESCE(SUM(bytes),0) FROM search_cache", [], |r| r.get(0))?;
            if total > max_bytes {
                let mut over = total - max_bytes;
                let oldest: Vec<(String, i64)> = {
                    let mut s = tx.prepare("SELECT key, bytes FROM search_cache WHERE key<>? ORDER BY searched_at ASC")?;
                    let rows = s.query_map(params![key], |r| Ok((r.get(0)?, r.get(1)?)))?;
                    rows.collect::<rusqlite::Result<_>>()?
                };
                for (k, b) in oldest {
                    if over <= 0 {
                        break;
                    }
                    tx.execute("DELETE FROM search_cache WHERE key=?", params![k])?;
                    over -= b;
                }
            }
            tx.commit()?;
            Ok(true)
        });
        kept.unwrap_or(false)
    }

    /// Forget everything (the database was restored: request ids may point at other requests now). The
    /// file is made again on the next use.
    pub fn clear(&self, dir: &Path) {
        let mut file = self.file();
        if matches!(&*file, File::Open { .. }) {
            *file = File::Closed;
        }
        let p = path(dir);
        for f in [p.clone(), p.with_extension("db-journal")] {
            match std::fs::remove_file(&f) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("search cache: remove {}: {e}", f.display()),
            }
        }
    }

    /// Whether the cache is off (it could not be opened, or broke).
    pub fn is_off(&self) -> bool {
        matches!(&*self.file(), File::Off)
    }

    /// Run `work` for `key` unless it already runs, and wait for its answer: callers asking for the same
    /// key together share one search. `work` runs as a task of its own, so it goes on to the end (and is
    /// kept) when every caller has gone away – a picker closed while Soulseek searches.
    pub async fn single<F>(&self, key: &str, work: F) -> Result<Arc<Found>, Failure>
    where
        F: std::future::Future<Output = Result<Found, Failure>> + Send + 'static,
    {
        let flight = {
            let mut flights = self.0.flights.lock().unwrap_or_else(|e| e.into_inner());
            match flights.get(key) {
                Some((_, f)) => f.clone(),
                None => {
                    let gen = self.0.next.fetch_add(1, Ordering::SeqCst);
                    let me = self.clone();
                    let k = key.to_string();
                    let task = tokio::spawn(async move {
                        let out = work.await.map(Arc::new);
                        // (waits for the caller that started it to have put it in the map)
                        let mut flights = me.0.flights.lock().unwrap_or_else(|e| e.into_inner());
                        if flights.get(&k).is_some_and(|(g, _)| *g == gen) {
                            flights.remove(&k);
                        }
                        out
                    });
                    let f: Flight = async move { task.await.unwrap_or_else(|e| Err(Failure::Other(format!("the search stopped: {e}")))) }.boxed().shared();
                    flights.insert(key.to_string(), (gen, f.clone()));
                    f
                }
            }
        };
        flight.await
    }

    /// A live search for `key` is running now.
    pub fn running(&self, key: &str) -> bool {
        self.0.flights.lock().unwrap_or_else(|e| e.into_inner()).contains_key(key)
    }
}

// ---- keys -------------------------------------------------------------------------------------------

/// The words of a search: lower case, split on anything that is not a letter, a digit or `'`.
pub fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// A Soulseek search's key: its words, each once, sorted – Soulseek finds files with all the words in
/// any order, so "Duumu Illuminate EP" and "illuminate ep duumu" are the same search.
pub fn slskd_key(text: &str) -> String {
    let mut w = words(text);
    w.sort();
    w.dedup();
    format!("slskd:{}", w.join(" "))
}

/// An indexer search's key: the categories, sorted, and the words in their order (an indexer's matching
/// may depend on it), with the symbols that stand for letters spelled out as the search sends them.
pub fn torznab_key(text: &str, cats: &[&str]) -> String {
    let mut c: Vec<&str> = cats.to_vec();
    c.sort_unstable();
    format!("torznab:{}:{}", c.join(","), words(&crate::sources::torznab::fold_symbols(text)).join(" "))
}

/// The texts a request's own Soulseek (and indexer) attempts search with, as the worker makes them: an
/// album by "artist album", then by the album alone; a song by "artist title".
pub fn worker_texts(kind: &str, artist: &str, title: &str, album: &str) -> Vec<String> {
    let q = crate::sources::slskd::query_text;
    let mut out = if kind == "album" {
        let album = if album.trim().is_empty() { title } else { album };
        vec![q(&[artist, album]), q(&[album])]
    } else {
        vec![q(&[artist, title])]
    };
    out.retain(|t| !t.is_empty());
    out.dedup();
    out
}

// ---- what is kept -----------------------------------------------------------------------------------

/// The fields of `v` named in `keys` that are set.
fn pick(v: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter().filter_map(|k| v.get(*k).filter(|x| !x.is_null()).map(|x| (k.to_string(), x.clone()))).collect()
}

/// slskd's answers as kept: per answer its user, free slot, speed and queue; per file (audio files only)
/// its name, size, bit rate, bit depth, sample rate and length – everything the picker's rows, the
/// worker's ranking and the Soulseek page read. Answers without audio files are left out.
pub fn slim_soulseek(responses: &[Value]) -> Vec<Value> {
    responses.iter().filter_map(|r| {
        let files: Vec<Value> = r["files"].as_array()?.iter()
            .filter(|f| DOWNLOAD_EXTS.contains(&crate::sources::slskd::ext(f["filename"].as_str().unwrap_or("")).as_str()))
            .map(|f| Value::Object(pick(f, &FILE_KEYS))).collect();
        if files.is_empty() {
            return None;
        }
        let mut o = pick(r, &ANSWER_KEYS);
        o.insert("files".into(), Value::Array(files));
        Some(Value::Object(o))
    }).collect()
}

/// Prowlarr's releases as kept: what the picker's rows read (whether a release is usable is decided when
/// it is read, so a download client set up later counts).
pub fn slim_releases(releases: &[Value]) -> Vec<Value> {
    releases.iter().map(|r| Value::Object(pick(r, &RELEASE_KEYS))).collect()
}

// ---- with the server --------------------------------------------------------------------------------

/// Keep a Soulseek search's answers (`text` as sent), for the picker. On the blocking pool.
pub async fn keep_slskd(st: &impl Ctx, text: &str, request_id: Option<i64>, responses: &[Value]) -> bool {
    let results = slim_soulseek(responses);
    keep(st, "slskd", slskd_key(text), text.to_string(), request_id, results).await
}

/// Keep an indexer search's releases (`text` as searched, in `cats`), for the picker. On the blocking pool.
pub async fn keep_releases(st: &impl Ctx, text: &str, cats: &[&str], request_id: Option<i64>, releases: &[Value]) -> bool {
    let results = slim_releases(releases);
    keep(st, "torznab", torznab_key(text, cats), text.to_string(), request_id, results).await
}

/// Keep a search's answers (on a blocking thread); false when they weren't kept.
async fn keep(st: &impl Ctx, kind: &'static str, key: String, query: String, request_id: Option<i64>, results: Vec<Value>) -> bool {
    let cache = st.requests().search_cache.clone();
    let dir = st.data_dir().to_path_buf();
    let now = encore_core::time::now();
    tokio::task::spawn_blocking(move || cache.put(&dir, kind, &key, &query, request_id, &results, now)).await.unwrap_or(false)
}

// ---- words ------------------------------------------------------------------------------------------

/// (day number, "14:02") of unix seconds on this server's clock.
fn local_day(unix: f64) -> (i64, String) {
    let t = unix as libc::time_t;
    // SAFETY: localtime_r reads `t` and writes only the struct handed to it, which lives on this stack.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    let day = if ok {
        encore_core::time::days_from_civil(tm.tm_year as i64 + 1900, tm.tm_mon as i64 + 1, tm.tm_mday as i64)
    } else {
        (unix / 86_400.0).floor() as i64
    };
    (day, crate::slskd_session::clock(unix))
}

/// The line's sentence for an answer kept since `at`, on this server's clock (the same clock as "next
/// attempt at 14:20" next to it): "Found at 14:02", "Found yesterday at 22:10", "Found at 17/10 09:30".
pub fn found_words(at: f64, now: f64) -> String {
    let (day, time) = local_day(at);
    let (today, _) = local_day(now);
    match today - day {
        d if d <= 0 => format!("Found at {time}"),
        1 => format!("Found yesterday at {time}"),
        _ => {
            let (_, m, d) = encore_core::time::civil_from_days(day);
            format!("Found at {d}/{m} {time}")
        }
    }
}

#[cfg(test)]
#[path = "search_cache_tests.rs"]
mod tests;
