//! SQLite, same file and tables as older servers, so upgrading keeps everything.
//! Rows come back as JSON objects, which is what the API returns anyway.
pub mod backup;

use encore_core::fsinfo;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{params_from_iter, Connection, OpenFlags};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// One result row: column name → value.
pub type Row = Map<String, Value>;

/// One connection for writes (they are serialized here, as SQLite would anyway) and a few for reads:
/// in WAL mode reads never wait for a write or for each other, so a long scan or a background job no
/// longer holds up every page that loads meanwhile.
///
/// Two ways in: `query` / `one` / `execute` run one statement on the calling thread (the request worker,
/// the scan, tests); `read` and `write` run a closure on tokio's blocking pool with one read connection
/// or the write connection lent to it ([`Conn`]), so an async handler never holds up the runtime.
#[derive(Clone)]
pub struct Db {
    /// the write connection
    conn: Arc<Mutex<Connection>>,
    /// the read connections (none on a network file system)
    readers: Arc<Vec<Mutex<Connection>>>,
    /// round-robin index of the next read connection to try
    next_reader: Arc<AtomicUsize>,
    /// goes up with every write to the songs or the artist aliases (see `library_version`)
    library_writes: Arc<AtomicU64>,
    /// goes up with every write to the accounts (see `users_version`)
    user_writes: Arc<AtomicU64>,
    /// goes up with every write to what the per-user totals count (see `stats_version`)
    stats_writes: Arc<AtomicU64>,
    /// why there are as many read connections as there are (Diagnostics)
    readers_source: ReadersSource,
    /// how often and how long statements waited for a connection (`/api/admin/perf`)
    waits: Arc<PoolWaits>,
    /// the folder `encore.db` (and the backups) are in
    pub data_dir: PathBuf,
}

/// Waits for a connection: how many there were, their total and the longest (microseconds), for the
/// read connections (all of them busy) and for the write connection (another write held it).
#[derive(Default)]
struct PoolWaits {
    /// reads that waited for a connection
    read_n: AtomicU64,
    /// their total wait (µs)
    read_us: AtomicU64,
    /// the longest read wait (µs)
    read_max_us: AtomicU64,
    /// writes that waited for the connection
    write_n: AtomicU64,
    /// their total wait (µs)
    write_us: AtomicU64,
    /// the longest write wait (µs)
    write_max_us: AtomicU64,
}

impl PoolWaits {
    /// Count one wait that started at `since` into `n`, `total` and `max`.
    fn note(n: &AtomicU64, total: &AtomicU64, max: &AtomicU64, since: std::time::Instant) {
        let us = since.elapsed().as_micros().min(u64::MAX as u128) as u64;
        n.fetch_add(1, Ordering::Relaxed);
        total.fetch_add(us, Ordering::Relaxed);
        max.fetch_max(us, Ordering::Relaxed);
    }
}

/// Where the number of read connections came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadersSource {
    /// `ENCORE_DB_READERS` is set
    Env,
    /// the default (4) on a local disk
    Default,
    /// off: the config folder is on this network file system
    NetworkFs(&'static str),
}

impl ReadersSource {
    /// The source as `/api/admin/perf` names it.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReadersSource::Env => "env",
            ReadersSource::Default => "default",
            ReadersSource::NetworkFs(_) => "network",
        }
    }
}

/// Read connections next to the write one: `ENCORE_DB_READERS` when set (0 = every statement on one
/// connection, as servers before 0.25.3 did), else 4 on a local disk and 0 when the config folder is on
/// a network file system (NFS, SMB/CIFS, FUSE …: several connections to one file are slow or unsafe
/// with their locking).
fn readers_wanted(data_dir: &Path) -> (usize, ReadersSource) {
    readers_for(std::env::var("ENCORE_DB_READERS").ok().as_deref(), fsinfo::of(data_dir))
}

/// How many read connections to open: `ENCORE_DB_READERS` (at most 16), none on a network file system,
/// otherwise 4.
fn readers_for(env: Option<&str>, fs: Option<fsinfo::FsInfo>) -> (usize, ReadersSource) {
    if let Some(n) = env.and_then(|v| v.trim().parse::<usize>().ok()) {
        return (n.min(16), ReadersSource::Env);
    }
    match fs {
        Some(f) if f.network => (0, ReadersSource::NetworkFs(f.name)),
        _ => (4, ReadersSource::Default),
    }
}

/// Per-connection settings: WAL is safe with synchronous=NORMAL (a power cut can lose the last
/// commits, never corrupt the file); a bigger page cache and memory-mapped reads make the library
/// queries (60k songs) run from memory; the prepared-statement cache holds the server's hot statements
/// (rusqlite's default of 16 thrashed against a few hundred distinct ones); the WAL file is trimmed
/// back to 64 MB after a checkpoint instead of keeping the size a full scan grew it to.
fn tune(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.set_prepared_statement_cache_capacity(128);
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -16000)?;   // KiB: 16 MB per connection
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // these two answer with the value they set: a pragma_update would see an unexpected row
    let _ = conn.query_row("PRAGMA mmap_size=268435456", [], |_| Ok(()));
    let _ = conn.query_row("PRAGMA journal_size_limit=67108864", [], |_| Ok(()));
    Ok(())
}

/// Open `n` tuned read connections on the database at `path`.
fn open_readers(path: &Path, n: usize) -> rusqlite::Result<Vec<Connection>> {
    (0..n).map(|_| {
        let c = Connection::open(path)?;
        tune(&c)?;
        Ok(c)
    }).collect()
}

/// Each opened database counts its writes from its own starting point, so two handles on one file
/// (tests) never look alike.
fn next_instance() -> u64 {
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::SeqCst)
}

/// Statements that only read go to a read connection; anything else to the write one.
fn is_read(sql: &str) -> bool {
    let head = sql.trim_start();
    let word: String = head.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase();
    word == "SELECT" || (word == "WITH" && !["INSERT", "UPDATE", "DELETE", "REPLACE"].iter().any(|w| head.to_ascii_uppercase().contains(w)))
}

/// The schema's history, one file per version in crates/db/migrations (number, name, statements). Every
/// file runs on every open, in order, so each is idempotent: CREATE … IF NOT EXISTS, and an
/// `ALTER TABLE … ADD COLUMN` is skipped when the table has the column already. A change to the schema
/// is a new file with the next number, and SCHEMA_VERSION becomes that number.
pub const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "initial", include_str!("../migrations/0001_initial.sql")),
    (21, "playback_spot", include_str!("../migrations/0021_playback_spot.sql")),
    (22, "artist_aliases", include_str!("../migrations/0022_artist_aliases.sql")),
    (23, "cover_colors", include_str!("../migrations/0023_cover_colors.sql")),
    (24, "reports_quality_contributions", include_str!("../migrations/0024_reports_quality_contributions.sql")),
    (25, "translations", include_str!("../migrations/0025_translations.sql")),
    (26, "indexes", include_str!("../migrations/0026_indexes.sql")),
    (27, "loudness", include_str!("../migrations/0027_loudness.sql")),
    (28, "plugins", include_str!("../migrations/0028_plugins.sql")),
    (29, "translation_tiers", include_str!("../migrations/0029_translation_tiers.sql")),
    (30, "surround", include_str!("../migrations/0030_surround.sql")),
    (31, "library_types", include_str!("../migrations/0031_library_types.sql")),
    (32, "artist_credits", include_str!("../migrations/0032_artist_credits.sql")),
    (33, "album_merges", include_str!("../migrations/0033_album_merges.sql")),
    (34, "collaborative_playlists", include_str!("../migrations/0034_collaborative_playlists.sql")),
    (35, "album_splits", include_str!("../migrations/0035_album_splits.sql")),
    (36, "song_copies", include_str!("../migrations/0036_song_copies.sql")),
    (37, "playlist_item_origin", include_str!("../migrations/0037_playlist_item_origin.sql")),
    (38, "lyrics_reject", include_str!("../migrations/0038_lyrics_reject.sql")),
    (39, "own_files", include_str!("../migrations/0039_own_files.sql")),
    (40, "taste", include_str!("../migrations/0040_taste.sql")),
];

/// The schema this server writes, stamped into `PRAGMA user_version`: the number of the last file in
/// MIGRATIONS. An older server that finds a higher number refuses to touch the file (it would otherwise
/// fail half-way through and leave a database neither version can read), and a lower number means
/// "back up, then migrate" (and ANALYZE after it: new indexes have no statistics yet).
pub const SCHEMA_VERSION: i64 = 40;

/// Exit code of the server when the database comes from a newer Encore.
pub const EXIT_NEWER_DB: i32 = 3;

/// The database was written by a newer Encore than this one.
#[derive(Debug)]
pub struct NewerDatabase {
    /// the schema version in the file
    pub found: i64,
    /// the newest schema this server knows
    pub known: i64,
    /// where the backups are, to restore one from
    pub backups: PathBuf,
}

impl std::fmt::Display for NewerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "This database was written by a newer Encore (schema {}, this server knows {}). Start the newer image again, or restore a backup from {}. Nothing was changed.",
            self.found, self.known, self.backups.display())
    }
}
impl std::error::Error for NewerDatabase {}

/// A failed open or migration because the disk is full says which folder that is, what it has free and
/// how big the database is, so the owner knows what to clear; any other error is returned as it is.
fn full_disk_hint(e: anyhow::Error, data_dir: &Path) -> anyhow::Error {
    let text = e.to_string();
    if !(text.contains("disk is full") || text.contains("No space left")) {
        return e;
    }
    let free = encore_core::fsinfo::free_bytes(data_dir).map(encore_core::text::size_text).unwrap_or_else(|| "nothing".into());
    let db = encore_core::text::size_text(crate::backup::needed_bytes(data_dir));
    e.context(format!(
        "the disk of {} is full ({free} free; the database needs about {db} to work in): free space there (old copies in {}, the cache folder) and start again",
        data_dir.display(),
        crate::backup::dir(data_dir).display()
    ))
}

/// Pragmas, then every migration (tables, missing columns, indexes), then the version stamp: run on
/// every open.
fn prepare(conn: &Connection) -> anyhow::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    tune(conn)?;
    for (n, name, sql) in MIGRATIONS {
        migrate(conn, sql).map_err(|e| anyhow::anyhow!("database migration {n:04}_{name} failed: {e}"))?;
    }
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// One migration file, statement by statement; an `ALTER TABLE t ADD COLUMN c …` whose column is there
/// already is skipped.
fn migrate(conn: &Connection, sql: &str) -> rusqlite::Result<()> {
    for stmt in statements(sql) {
        if let Some((table, col)) = added_column(&stmt) {
            let mut q = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let have: Vec<String> = q.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
            if have.iter().any(|c| c.eq_ignore_ascii_case(&col)) {
                continue;
            }
        }
        conn.execute_batch(&stmt)?;
    }
    Ok(())
}

/// The statements of a migration file: `--` comments removed (not inside a quoted string or name), split
/// at the semicolons between statements.
fn statements(sql: &str) -> Vec<String> {
    let (mut out, mut cur) = (vec![], String::new());
    let (mut quote, mut comment) = (None::<char>, false);
    for c in sql.chars() {
        if comment {
            if c == '\n' {
                comment = false;
                cur.push(c);
            }
            continue;
        }
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '-' && cur.ends_with('-') => {
                cur.pop();
                comment = true;
            }
            None if c == ';' => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            None => {
                if c == '\'' || c == '"' {
                    quote = Some(c);
                }
                cur.push(c);
            }
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// (table, column) of an `ALTER TABLE t ADD [COLUMN] c …` statement.
fn added_column(stmt: &str) -> Option<(String, String)> {
    let words: Vec<&str> = stmt.split_whitespace().collect();
    let up = |i: usize| words.get(i).map(|w| w.to_ascii_uppercase());
    if up(0)? != "ALTER" || up(1)? != "TABLE" || up(3)? != "ADD" {
        return None;
    }
    let col = if up(4)? == "COLUMN" { words.get(5)? } else { words.get(4)? };
    Some((words[2].trim_matches('"').to_string(), col.trim_matches('"').to_string()))
}

/// After a migration the planner has no statistics for the new indexes (and a database that never had
/// ANALYZE run has none at all): a 57k-row table with 20 indexes then gets guessed plans.
fn analyze(conn: &Connection) {
    if let Err(e) = conn.execute_batch("ANALYZE") {
        tracing::warn!("ANALYZE after the schema migration failed: {e}");
    }
}

/// The tables a statement writes: the name after INSERT INTO / REPLACE INTO / UPDATE / DELETE FROM /
/// ALTER TABLE / DROP TABLE / CREATE TABLE, for every statement in the text (lower-cased, quotes and a
/// schema prefix stripped). Tables a statement only reads (a subquery) are not in it. `--` comments are
/// skipped, except inside a string literal or a quoted name.
fn tables_written(sql: &str) -> Vec<String> {
    let mut no_comments = String::with_capacity(sql.len());
    let (mut quote, mut comment) = (None::<char>, false);
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if comment {
            if c == '\n' {
                comment = false;
                no_comments.push(' ');
            }
            continue;
        }
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                no_comments.push(c);
            }
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                no_comments.push(c);
            }
            None if c == '-' && chars.peek() == Some(&'-') => comment = true,
            None => no_comments.push(c),
        }
    }
    let tokens: Vec<String> = no_comments.split(|c: char| c.is_whitespace() || matches!(c, ';' | '(' | ')' | ','))
        .filter(|t| !t.is_empty()).map(|t| t.to_ascii_lowercase()).collect();
    let at = |i: usize| tokens.get(i).map(|s| s.as_str());
    let mut out = vec![];
    let mut i = 0;
    while i < tokens.len() {
        let name_at = match tokens[i].as_str() {
            // UPDATE [OR ROLLBACK | ABORT | REPLACE | FAIL | IGNORE] table
            "update" => if at(i + 1) == Some("or") { i + 3 } else { i + 1 },
            // INSERT [OR …] INTO table, REPLACE INTO table
            "into" if i >= 1 && (matches!(at(i - 1), Some("insert" | "replace")) || (i >= 3 && at(i - 3) == Some("insert") && at(i - 2) == Some("or"))) => i + 1,
            "from" if i >= 1 && at(i - 1) == Some("delete") => i + 1,
            "table" if i >= 1 && matches!(at(i - 1), Some("alter" | "drop")) => if at(i + 1) == Some("if") { i + 3 } else { i + 1 },
            // CREATE [TEMP | TEMPORARY] TABLE [IF NOT EXISTS] table
            "table" if i >= 1 && (at(i - 1) == Some("create") || (i >= 2 && at(i - 2) == Some("create"))) => if at(i + 1) == Some("if") { i + 4 } else { i + 1 },
            _ => { i += 1; continue; }
        };
        if let Some(n) = at(name_at) {
            let n = n.rsplit('.').next().unwrap_or(n);
            out.push(n.trim_matches(|c| matches!(c, '"' | '`' | '[' | ']' | '\'')).to_string());
        }
        i = name_at + 1;
    }
    out
}

/// The tables the library answers (lists, the side bar, the artist index) are built from: a write to
/// one of them moves `library_version` (`album_types`: each album's type, which the album lists filter by).
const LIBRARY_TABLES: &[&str] = &["tracks", "artist_alias", "album_types"];
/// A write to one of these moves `users_version` (who has translations on, roles, prefs).
const USER_TABLES: &[&str] = &["users"];
/// The per-user totals (the leaderboard, the rank on Home) count these: a write to one of them moves
/// `stats_version`.
const STATS_TABLES: &[&str] = &["requests", "plays", "playlists", "playlist_items", "users"];

/// Seconds since 1970-01-01 UTC, what every time column holds.
pub use encore_core::time::now;

/// A JSON value as an SQL parameter (booleans as 0/1, arrays and objects as JSON text).
fn to_sql(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(*b as i64),
        Value::Number(n) => n.as_i64().map(SqlValue::Integer).unwrap_or_else(|| SqlValue::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

/// An SQL value as JSON (text and blobs as strings, lossily decoded; a non-finite real as null).
fn from_sql(v: ValueRef) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
    }
}

/// serde_json for `p!`, so a crate that uses the macro does not need its own dependency on it.
#[doc(hidden)]
pub use serde_json as __json;

/// `p![a, b, c]` – query parameters of any JSON-convertible type.
#[macro_export]
macro_rules! p {
    () => { Vec::<$crate::__json::Value>::new() };
    ($($x:expr),+ $(,)?) => { vec![$($crate::__json::json!($x)),+] };
}

impl Db {
    /// Open (or create) `encore.db` in `data_dir`: refuse a newer schema, back up before an upgrade, run the
    /// migrations and open the read connections.
    pub fn open(data_dir: &Path) -> anyhow::Result<Db> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("encore.db");
        let existing = path.is_file() && std::fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false);
        let mut upgrade_from = None;
        if existing {
            // only look at the version before anything else happens to the file: a read-only connection
            // never recovers or checkpoints a WAL left by a newer server, so "nothing was changed" holds
            let ro = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
            let found: i64 = ro.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            drop(ro);
            if found > SCHEMA_VERSION {
                return Err(NewerDatabase { found, known: SCHEMA_VERSION, backups: crate::backup::dir(data_dir) }.into());
            }
            if found < SCHEMA_VERSION {
                upgrade_from = Some(found);
            }
        }
        let conn = Connection::open(&path)?;
        if let Some(found) = upgrade_from {
            tracing::info!("database schema {found} → {SCHEMA_VERSION}: backing up first");
            // a full disk is a warning here, not a start that fails over and over (see before_upgrade)
            crate::backup::before_upgrade(&conn, data_dir)?;
        }
        prepare(&conn).map_err(|e| full_disk_hint(e, data_dir))?;
        if upgrade_from.is_some() {
            analyze(&conn);
        }
        let (wanted, readers_source) = readers_wanted(data_dir);
        if let ReadersSource::NetworkFs(fs) = readers_source {
            tracing::info!("{} is on {fs}, a network file system: the database read connections are off (set ENCORE_DB_READERS=4 to turn them on anyway)", data_dir.display());
        }
        let readers = open_readers(&path, wanted)?.into_iter().map(Mutex::new).collect();
        let instance = next_instance() << 40;
        Ok(Db { conn: Arc::new(Mutex::new(conn)), readers: Arc::new(readers), next_reader: Arc::new(AtomicUsize::new(0)), library_writes: Arc::new(AtomicU64::new(instance)),
            user_writes: Arc::new(AtomicU64::new(instance)), stats_writes: Arc::new(AtomicU64::new(instance)), readers_source, waits: Default::default(), data_dir: data_dir.to_path_buf() })
    }

    /// Changes whenever a statement that writes the songs or the artist aliases ran through this
    /// handle (every write in the server does): caches of library answers compare it instead of reading
    /// the whole table again. Writes made by another program (a test's own connection) don't count.
    pub fn library_version(&self) -> u64 {
        self.library_writes.load(Ordering::SeqCst)
    }

    /// Changes whenever a statement that writes the accounts ran through this handle (prefs, roles, new
    /// or disabled users): "does anyone have translations on" is looked up again only after that.
    pub fn users_version(&self) -> u64 {
        self.user_writes.load(Ordering::SeqCst)
    }

    /// Changes whenever a statement that writes the requests, plays, playlists, their songs (likes) or
    /// the accounts ran through this handle: the per-user totals are counted again only after that.
    pub fn stats_version(&self) -> u64 {
        self.stats_writes.load(Ordering::SeqCst)
    }

    /// Move the versions the statement `sql` writes.
    fn note_write(&self, sql: &str) {
        self.bump(writes_of(sql));
    }

    /// Moves the versions a write touched (see [`writes_of`]).
    fn bump(&self, writes: u8) {
        if writes & WRITES_LIBRARY != 0 {
            self.library_writes.fetch_add(1, Ordering::SeqCst);
        }
        if writes & WRITES_USERS != 0 {
            self.user_writes.fetch_add(1, Ordering::SeqCst);
        }
        if writes & WRITES_STATS != 0 {
            self.stats_writes.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// How long statements waited for a connection since the start: `{read: {waits, total_ms, max_ms},
    /// write: {…}}` (a read waits only when every read connection is busy).
    pub fn pool_waits(&self) -> Value {
        let w = &self.waits;
        let side = |n: &AtomicU64, t: &AtomicU64, m: &AtomicU64| serde_json::json!({
            "waits": n.load(Ordering::Relaxed),
            "total_ms": t.load(Ordering::Relaxed) as f64 / 1000.0,
            "max_ms": m.load(Ordering::Relaxed) as f64 / 1000.0,
        });
        serde_json::json!({
            "read": side(&w.read_n, &w.read_us, &w.read_max_us),
            "write": side(&w.write_n, &w.write_us, &w.write_max_us),
            "readers": self.readers.len(),
        })
    }

    /// The write connection; a wait for it (another write holds it) is counted.
    fn writer(&self) -> MutexGuard<'_, Connection> {
        match self.conn.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                let since = std::time::Instant::now();
                let g = self.conn.lock().unwrap_or_else(|e| e.into_inner());
                PoolWaits::note(&self.waits.write_n, &self.waits.write_us, &self.waits.write_max_us, since);
                g
            }
        }
    }

    /// How many read connections there are and why (Diagnostics).
    pub fn readers_in_effect(&self) -> (usize, ReadersSource) {
        (self.readers.len(), self.readers_source)
    }

    /// Bytes of the database file and its WAL.
    pub fn file_size(&self) -> u64 {
        ["encore.db", "encore.db-wal"].iter().filter_map(|f| std::fs::metadata(self.data_dir.join(f)).ok()).map(|m| m.len()).sum()
    }

    /// `PRAGMA optimize` on every connection: SQLite re-analyzes the tables whose statistics its recent
    /// queries would have wanted (cheap when there is nothing to do). After scans and imports.
    pub fn optimize(&self) {
        let run = |c: &Connection| {
            if let Err(e) = c.execute_batch("PRAGMA optimize") {
                tracing::debug!("PRAGMA optimize: {e}");
            }
        };
        run(&self.writer());
        for r in self.readers.iter() {
            run(&r.lock().unwrap_or_else(|e| e.into_inner()));
        }
    }

    /// A free read connection (or, when all are busy, the next one in turn).
    fn reader(&self) -> MutexGuard<'_, Connection> {
        let n = self.readers.len();
        if n == 0 {
            return self.writer();
        }
        let start = self.next_reader.fetch_add(1, Ordering::Relaxed);
        for i in 0..n {
            match self.readers[(start + i) % n].try_lock() {
                Ok(g) => return g,
                Err(std::sync::TryLockError::Poisoned(e)) => return e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
        }
        let since = std::time::Instant::now();
        let g = self.readers[start % n].lock().unwrap_or_else(|e| e.into_inner());
        PoolWaits::note(&self.waits.read_n, &self.waits.read_us, &self.waits.read_max_us, since);
        g
    }

    /// `PRAGMA user_version` of the open database.
    pub fn user_version(&self) -> rusqlite::Result<i64> {
        self.writer().query_row("PRAGMA user_version", [], |r| r.get(0))
    }

    /// Copies the database to <data_dir>/backups (see `backup`).
    pub fn backup(&self, kind: crate::backup::Kind) -> anyhow::Result<crate::backup::Info> {
        let conn = self.writer();
        crate::backup::create(&conn, &self.data_dir, kind)
    }

    /// Replaces the database with a backup file: a safety copy of the current data is taken first, the
    /// connection is closed, the file swapped, and the connection reopened (and migrated, if the backup
    /// is from an older Encore). Every other thread waits on the lock meanwhile. Returns the safety copy.
    pub fn restore(&self, from: &Path) -> anyhow::Result<crate::backup::Info> {
        let version = crate::backup::validate_file(from)?;
        if version > SCHEMA_VERSION {
            anyhow::bail!("That backup was written by a newer Encore (schema {version}, this server knows {SCHEMA_VERSION})");
        }
        // the copy can fail (disk full, permissions): do it before the live connection is touched
        let tmp = self.data_dir.join("encore.db.restoring");
        if let Err(e) = std::fs::copy(from, &tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        let mut guard = self.conn.lock().unwrap();
        // the read connections are closed too (in order; nothing else ever holds two of these locks)
        let mut readers: Vec<MutexGuard<'_, Connection>> = self.readers.iter().map(|m| m.lock().unwrap_or_else(|e| e.into_inner())).collect();
        let safety = match crate::backup::create(&guard, &self.data_dir, crate::backup::Kind::BeforeRestore) {
            Ok(s) => s,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        let path = self.data_dir.join("encore.db");
        // close the current connection (this checkpoints and removes the WAL), then swap the file
        let placeholder = match Connection::open_in_memory() {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
        };
        drop(std::mem::replace(&mut *guard, placeholder));
        for r in readers.iter_mut() {
            if let Ok(p) = Connection::open_in_memory() {
                drop(std::mem::replace(&mut **r, p));
            }
        }
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        let swap = std::fs::rename(&tmp, &path);
        let reopen = Connection::open(&path).map_err(anyhow::Error::from).and_then(|c| prepare(&c).map(|_| c));
        match (swap, reopen) {
            (Ok(()), Ok(conn)) => {
                if version < SCHEMA_VERSION {
                    analyze(&conn);
                }
                *guard = conn;
                self.library_writes.fetch_add(1, Ordering::SeqCst);
                self.user_writes.fetch_add(1, Ordering::SeqCst);
                self.stats_writes.fetch_add(1, Ordering::SeqCst);
                Self::reopen_readers(&path, &mut readers);
                tracing::info!("database restored from {} (safety copy: {})", from.display(), safety.name);
                Ok(safety)
            }
            (swap, reopen) => {
                // whatever is on disk now (the old file or the restored one): put a real connection to
                // it back behind the lock before reporting, so the server keeps working
                let reason = swap.err().map(|e| e.to_string()).or_else(|| reopen.err().map(|e| e.to_string())).unwrap_or_default();
                let _ = std::fs::remove_file(&tmp);
                match Connection::open(&path) {
                    Ok(c) => {
                        if let Err(e) = prepare(&c) {
                            // the restored file can't be migrated: go back to the data from before
                            tracing::warn!("restored database could not be prepared ({e}); putting back {}", safety.name);
                            drop(c);
                            if let Err(e) = std::fs::copy(&safety.path, &path) {
                                tracing::error!("could not put {} back: {e}", safety.name);
                            }
                            for suffix in ["-wal", "-shm"] {
                                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
                            }
                            if let Ok(c) = Connection::open(&path) {
                                let _ = prepare(&c);
                                *guard = c;
                            }
                        } else {
                            *guard = c;
                        }
                    }
                    Err(e) => tracing::error!("database could not be reopened after a failed restore: {e}"),
                }
                Self::reopen_readers(&path, &mut readers);
                anyhow::bail!("restore failed ({reason}); the data from before is in backups/{}", safety.name)
            }
        }
    }

    /// Replace the read connections with fresh ones (after a restore swapped the file under them).
    fn reopen_readers(path: &Path, readers: &mut [MutexGuard<'_, Connection>]) {
        match open_readers(path, readers.len()) {
            Ok(fresh) => {
                for (r, c) in readers.iter_mut().zip(fresh) {
                    **r = c;
                }
            }
            Err(e) => tracing::error!("database read connections could not be reopened: {e}"),
        }
    }

    /// Run several statements (write connection).
    pub fn execute_batch(&self, sql: &str) -> rusqlite::Result<()> {
        let r = self.writer().execute_batch(sql);
        self.note_write(sql);
        r
    }

    /// Rows of a statement: reads on a read connection, anything else on the write connection.
    pub fn query(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Vec<Row>> {
        let read = is_read(sql);
        if !read {
            self.note_write(sql);
        }
        let conn = if read { self.reader() } else { self.writer() };
        rows_of(&conn, sql, &params)
    }

    /// The first row of a statement, if any.
    pub fn one(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// Runs a statement; returns the last inserted row id.
    pub fn execute(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<i64> {
        let conn = self.writer();
        let r = conn.execute(sql, params_from_iter(params.iter().map(to_sql)));
        // counted after the write too: a reader that looked between the two sees the change next time
        self.note_write(sql);
        r?;
        Ok(conn.last_insert_rowid())
    }

    /// Add a line to the activity log, keeping the newest 2000.
    pub fn log(&self, level: &str, message: &str) {
        let _ = self.execute("INSERT INTO activity (ts, level, message) VALUES (?,?,?)", p![now(), level, message]);
        let _ = self.execute("DELETE FROM activity WHERE id < (SELECT MAX(id) - 2000 FROM activity)", p![]);
    }

    /// Runs `f` on tokio's blocking pool with a read connection lent to it (the write connection when
    /// there are no read connections). Only statements that read run on it; see [`Conn`].
    pub async fn read<T, E, F>(&self, f: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<rusqlite::Error> + Send + 'static,
        F: FnOnce(&Conn) -> Result<T, E> + Send + 'static,
    {
        let db = self.clone();
        joined(tokio::task::spawn_blocking(move || db.read_now(f)).await)
    }

    /// Runs `f` on tokio's blocking pool with the write connection lent to it: other writes wait until
    /// it returns, reads go on. Its writes move the versions (library, accounts, per-user totals) like
    /// `execute`'s do.
    pub async fn write<T, E, F>(&self, f: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<rusqlite::Error> + Send + 'static,
        F: FnOnce(&Conn) -> Result<T, E> + Send + 'static,
    {
        let db = self.clone();
        joined(tokio::task::spawn_blocking(move || db.write_now(f)).await)
    }

    /// [`Db::read`] on the calling thread, for work that already runs on a blocking thread.
    pub fn read_now<T, E: From<rusqlite::Error>>(&self, f: impl FnOnce(&Conn) -> Result<T, E>) -> Result<T, E> {
        let guard = self.reader();
        lend(&Conn::new(&guard, self, false), f)
    }

    /// [`Db::write`] on the calling thread, for work that already runs on a blocking thread (the scan writes
    /// its batches with it).
    pub fn write_now<T, E: From<rusqlite::Error>>(&self, f: impl FnOnce(&Conn) -> Result<T, E>) -> Result<T, E> {
        let guard = self.writer();
        lend(&Conn::new(&guard, self, true), f)
    }
}

/// [`writes_of`]: the statement writes the songs or the artist aliases.
const WRITES_LIBRARY: u8 = 1;
/// [`writes_of`]: the statement writes the accounts.
const WRITES_USERS: u8 = 2;
/// [`writes_of`]: the statement writes what the per-user totals count.
const WRITES_STATS: u8 = 4;

/// Which versions a statement moves (`WRITES_LIBRARY`, `WRITES_USERS`, `WRITES_STATS`), from the tables it
/// writes.
fn writes_of(sql: &str) -> u8 {
    let tables = tables_written(sql);
    let mut w = 0;
    if tables.iter().any(|t| LIBRARY_TABLES.contains(&t.as_str())) {
        w |= WRITES_LIBRARY;
    }
    if tables.iter().any(|t| USER_TABLES.contains(&t.as_str())) {
        w |= WRITES_USERS;
    }
    if tables.iter().any(|t| STATS_TABLES.contains(&t.as_str())) {
        w |= WRITES_STATS;
    }
    w
}

/// Every row of a statement as JSON objects (column name → value).
fn rows_of(conn: &Connection, sql: &str, params: &[Value]) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let rows = stmt.query_map(params_from_iter(params.iter().map(to_sql)), |r| {
        let mut m = Map::new();
        for (i, c) in cols.iter().enumerate() {
            m.insert(c.clone(), from_sql(r.get_ref(i)?));
        }
        Ok(m)
    })?;
    rows.collect()
}

/// A failure that is not SQLite's own: `code` says what kind (SQLITE_READONLY: a write asked a read
/// connection; SQLITE_ABORT: the closure panicked or its task was lost), the message says what happened.
fn other_error(code: std::ffi::c_int, message: String) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), Some(message))
}

/// Calls `f` with the connection. A panic in it is caught while the connection is still held, so the
/// connection's lock is not poisoned (every later write would fail on it), and comes back as an error.
/// A transaction `f` left open (it panicked inside one) is rolled back: the next closure starts clean.
fn lend<T, E: From<rusqlite::Error>>(c: &Conn, f: impl FnOnce(&Conn) -> Result<T, E>) -> Result<T, E> {
    let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(c))) {
        Ok(r) => r,
        Err(p) => {
            let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
            Err(E::from(other_error(rusqlite::ffi::SQLITE_ABORT, format!("database work panicked: {why}"))))
        }
    };
    if !c.conn.is_autocommit() {
        tracing::warn!("database: a transaction was left open; rolled back");
        let _ = c.conn.execute_batch("ROLLBACK");
    }
    // a transaction a panic left open was rolled back above: what it wrote is gone, its versions stay
    r
}

/// The result of a blocking-pool task; a task that panicked or was cancelled becomes an `SQLITE_ABORT` error.
fn joined<T, E: From<rusqlite::Error>>(r: Result<Result<T, E>, tokio::task::JoinError>) -> Result<T, E> {
    r.unwrap_or_else(|e| Err(E::from(other_error(rusqlite::ffi::SQLITE_ABORT, format!("database work did not finish: {e}")))))
}

/// Statements on a [`Db`] (each on a fitting connection) or on a lent [`Conn`]: a helper that only runs
/// SQL takes `&impl Query` and works with both.
pub trait Query {
    /// Rows of a statement.
    fn query(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Vec<Row>>;
    /// The first row of a statement, if any.
    fn one(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }
}

impl Query for Db {
    fn query(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Vec<Row>> {
        Db::query(self, sql, params)
    }
}

impl Query for Conn<'_> {
    fn query(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Vec<Row>> {
        Conn::query(self, sql, params)
    }
}

/// A connection lent to a closure by [`Db::read`] (a read connection) or [`Db::write`] (the write
/// connection). Inside the closure use only this handle: the `Db` methods would wait for the very
/// connection the closure holds.
pub struct Conn<'a> {
    /// the connection lent to the closure
    conn: &'a Connection,
    /// the database it belongs to (for the versions)
    db: &'a Db,
    /// whether this is the write connection
    write: bool,
    /// inside `transaction`: the versions its statements move wait for the COMMIT (see `note`)
    in_tx: std::cell::Cell<bool>,
    /// the versions waiting for the COMMIT (bit set of `writes_of`)
    pending: std::cell::Cell<u8>,
}

impl<'a> Conn<'a> {
    /// A lent connection; `write` says whether it may run writes.
    fn new(conn: &'a Connection, db: &'a Db, write: bool) -> Self {
        Conn { conn, db, write, in_tx: Default::default(), pending: Default::default() }
    }

    /// A write ran: the versions move at once, or, inside a transaction, once after its COMMIT. Readers
    /// see a transaction's rows only after the COMMIT, so a cache built in between keeps the version it read
    /// before and is built again; and a batch of 200 songs moves the library version once, not 400 times.
    fn note(&self, sql: &str) {
        if self.in_tx.get() {
            self.pending.set(self.pending.get() | writes_of(sql));
        } else {
            self.db.note_write(sql);
        }
    }

    /// Moves the versions a transaction's writes waited with.
    fn flush(&self) {
        let w = self.pending.replace(0);
        if w != 0 {
            self.db.bump(w);
        }
    }

    /// Rows of a statement. On a read connection only statements that read are run (anything else is
    /// an error: it would bypass the write connection and the versions).
    pub fn query(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Vec<Row>> {
        if !is_read(sql) {
            self.writable(sql)?;
            self.note(sql);
        }
        rows_of(self.conn, sql, &params)
    }

    /// The first row of a statement, if any.
    pub fn one(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// Runs a statement (write connection only); returns the last inserted row id.
    pub fn execute(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<i64> {
        self.writable(sql)?;
        let r = self.conn.execute(sql, params_from_iter(params.iter().map(to_sql)));
        self.note(sql);
        r?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Runs `f` in a transaction (write connection only): its statements are saved together, or none of
    /// them when it returns an error. The versions move once, after the COMMIT.
    pub fn transaction<T, E: From<rusqlite::Error>>(&self, f: impl FnOnce(&Conn) -> Result<T, E>) -> Result<T, E> {
        self.writable("BEGIN")?;
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        self.in_tx.set(true);
        let r = match f(self) {
            Ok(v) => match self.conn.execute_batch("COMMIT") {
                Ok(()) => Ok(v),
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e.into())
                }
            },
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        };
        self.in_tx.set(false);
        match &r {
            Ok(_) => self.flush(),
            // nothing was saved: nothing moves
            Err(_) => self.pending.set(0),
        }
        r
    }

    /// An error unless this connection may run `sql` (only the write connection runs writes).
    fn writable(&self, sql: &str) -> rusqlite::Result<()> {
        if self.write {
            return Ok(());
        }
        let head: String = sql.trim_start().chars().take(40).collect();
        Err(other_error(rusqlite::ffi::SQLITE_READONLY, format!("a read connection runs only reads, not: {head}")))
    }
}

/// Small accessors for JSON rows.
pub trait RowExt {
    /// The column as text (empty for null or missing; other values as JSON).
    fn s(&self, k: &str) -> String;
    /// The column as an integer (0 for null, missing or not a number; reals truncated).
    fn i(&self, k: &str) -> i64;
    /// The column as a real (0 when not a number).
    fn f(&self, k: &str) -> f64;
    /// The column as a boolean (`true`, or a non-zero number).
    fn b(&self, k: &str) -> bool;
}

impl RowExt for Row {
    fn s(&self, k: &str) -> String {
        match self.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        }
    }
    fn i(&self, k: &str) -> i64 {
        self.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).unwrap_or(0)
    }
    fn f(&self, k: &str) -> f64 {
        self.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0)
    }
    fn b(&self, k: &str) -> bool {
        match self.get(k) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use encore_core::tmp::dir as tmp;

    #[test]
    fn stamps_user_version_and_backs_up_before_migrating() {
        let d = tmp("stamp");
        // a fresh database: stamped, no backup (there was nothing to protect)
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        assert!(crate::backup::list(&d).is_empty());
        db.execute("INSERT INTO settings VALUES ('marker', '\"x\"')", p![]).unwrap();
        drop(db);
        // an older (unstamped) database: backed up first, then migrated and stamped
        Connection::open(d.join("encore.db")).unwrap().pragma_update(None, "user_version", 0).unwrap();
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let backups = crate::backup::list(&d);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].kind, crate::backup::Kind::BeforeUpgrade);
        assert_eq!(crate::backup::validate_file(&backups[0].path).unwrap(), 0);
        let c = Connection::open(&backups[0].path).unwrap();
        let v: String = c.query_row("SELECT value FROM settings WHERE key='marker'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "\"x\"");
        drop(db);
        // same version: no new backup
        let _db = Db::open(&d).unwrap();
        assert_eq!(crate::backup::list(&d).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn schema_21_adds_the_playback_spot_to_an_older_database() {
        const { assert!(SCHEMA_VERSION >= 21) };
        let d = tmp("spot");
        drop(Db::open(&d).unwrap());
        // what a schema 20 server left: no playback_spot table
        {
            let c = Connection::open(d.join("encore.db")).unwrap();
            c.execute_batch("DROP TABLE playback_spot; INSERT INTO settings VALUES ('kept', '1');").unwrap();
            c.pragma_update(None, "user_version", 20).unwrap();
        }
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let backups = crate::backup::list(&d);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].kind, crate::backup::Kind::BeforeUpgrade);
        assert_eq!(crate::backup::validate_file(&backups[0].path).unwrap(), 20);
        db.execute("INSERT INTO playback_spot (user_id, track_id, position, queue, idx, device_id, device_name, playing, updated_at) VALUES (1, 2, 3.5, '[2]', 0, 'd', 'Phone', 1, 10)", p![]).unwrap();
        assert_eq!(db.one("SELECT position FROM playback_spot WHERE user_id=1", p![]).unwrap().unwrap().f("position"), 3.5);
        assert_eq!(db.one("SELECT value FROM settings WHERE key='kept'", p![]).unwrap().unwrap().s("value"), "1");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn schema_22_adds_artist_aliases_to_a_schema_21_database() {
        const { assert!(SCHEMA_VERSION >= 22) };
        let d = tmp("aliases");
        drop(Db::open(&d).unwrap());
        // what a schema 21 server left: no alias tables
        {
            let c = Connection::open(d.join("encore.db")).unwrap();
            c.execute_batch("DROP TABLE artist_alias; DROP TABLE artist_alias_lookups; INSERT INTO settings VALUES ('kept', '1');").unwrap();
            // song shares as schema 21 had them: no kind / ref
            c.execute_batch("DROP TABLE shares; CREATE TABLE shares (token TEXT PRIMARY KEY, track_id INTEGER NOT NULL, user_id INTEGER NOT NULL, created_at REAL NOT NULL);
                             INSERT INTO shares VALUES ('old', 7, 1, 1);").unwrap();
            c.pragma_update(None, "user_version", 21).unwrap();
        }
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let backups = crate::backup::list(&d);
        assert_eq!(backups.len(), 1);
        assert_eq!(crate::backup::validate_file(&backups[0].path).unwrap(), 21);
        db.execute("INSERT INTO artist_alias (artist_key, alias, alias_key) VALUES ('ye', 'Kanye West', 'kanye west')", p![]).unwrap();
        db.execute("INSERT INTO artist_alias_lookups (artist_key, mbid, looked_up_at, ok) VALUES ('ye', 'x', 1, 1)", p![]).unwrap();
        assert_eq!(db.one("SELECT alias FROM artist_alias WHERE alias_key='kanye west'", p![]).unwrap().unwrap().s("alias"), "Kanye West");
        let old = db.one("SELECT track_id, kind, ref FROM shares WHERE token='old'", p![]).unwrap().unwrap();
        assert_eq!((old.i("track_id"), old.s("kind")), (7, "track".to_string()));
        assert!(old.get("ref").is_none_or(|v| v.is_null()));
        db.execute("INSERT INTO shares (token, track_id, user_id, created_at, kind, ref) VALUES ('alb', 0, 1, 2, 'album', 'a1')", p![]).unwrap();
        assert_eq!(db.one("SELECT value FROM settings WHERE key='kept'", p![]).unwrap().unwrap().s("value"), "1");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_a_newer_database_without_touching_it() {
        let d = tmp("newer");
        drop(Db::open(&d).unwrap());
        {
            let c = Connection::open(d.join("encore.db")).unwrap();
            c.pragma_update(None, "user_version", SCHEMA_VERSION + 3).unwrap();
            c.execute_batch("CREATE TABLE from_the_future (x)").unwrap();
        }
        // a WAL "the newer server" left behind must not be recovered or checkpointed by the check
        let wal = d.join("encore.db-wal");
        std::fs::write(&wal, b"not a real wal, must be left alone").unwrap();
        let before = std::fs::read(d.join("encore.db")).unwrap();
        let err = Db::open(&d).err().expect("must refuse");
        let n = err.downcast_ref::<NewerDatabase>().expect("typed error");
        assert_eq!((n.found, n.known), (SCHEMA_VERSION + 3, SCHEMA_VERSION));
        let text = n.to_string();
        assert!(text.contains(&format!("schema {}, this server knows {SCHEMA_VERSION}", SCHEMA_VERSION + 3)), "{text}");
        assert!(text.contains("Nothing was changed"));
        assert!(text.contains(&crate::backup::dir(&d).display().to_string()));
        assert_eq!(std::fs::read(d.join("encore.db")).unwrap(), before);
        assert_eq!(std::fs::read(&wal).unwrap(), b"not a real wal, must be left alone");
        assert!(crate::backup::list(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reads_and_writes_go_to_their_connections() {
        assert!(is_read("SELECT 1"));
        assert!(is_read("  select * from tracks"));
        assert!(is_read("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(!is_read("WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x"));
        assert!(!is_read("INSERT INTO settings VALUES ('a','b')"));
        assert!(!is_read("UPDATE tracks SET year='1'"));
        assert!(!is_read("PRAGMA user_version = 3"));
        let d = tmp("pool");
        let db = Db::open(&d).unwrap();
        // a read while the write connection is held elsewhere (a long write, a backup) doesn't wait
        db.execute("INSERT INTO settings VALUES ('k', '1')", p![]).unwrap();
        let held = db.conn.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let db2 = db.clone();
        std::thread::spawn(move || {
            let v = db2.one("SELECT value FROM settings WHERE key='k'", p![]).unwrap().unwrap().s("value");
            tx.send(v).unwrap();
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(5));
        drop(held);
        if !db.readers.is_empty() {
            assert_eq!(got.unwrap(), "1");
        }
        // what was written is read back at once (WAL: a read starts from the last commit)
        for i in 0..50 {
            db.execute("UPDATE settings SET value=? WHERE key='k'", p![i.to_string()]).unwrap();
            assert_eq!(db.one("SELECT value FROM settings WHERE key='k'", p![]).unwrap().unwrap().s("value"), i.to_string());
        }
        // writes to the songs move the library version; others don't
        let v = db.library_version();
        db.execute("INSERT INTO settings VALUES ('x', '1')", p![]).unwrap();
        assert_eq!(db.library_version(), v);
        db.execute("UPDATE tracks SET year='1' WHERE id=-1", p![]).unwrap();
        assert!(db.library_version() > v);
        // a second handle on the same file never looks like this one
        let other = Db::open(&d).unwrap();
        assert_ne!(other.library_version(), db.library_version());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_library_version_moves_only_for_writes_to_its_tables() {
        let t = |sql: &str| tables_written(sql);
        assert_eq!(t("INSERT INTO plays (user_id, track_id) SELECT 1, id FROM tracks WHERE id=?"), vec!["plays"]);
        assert_eq!(t("INSERT OR IGNORE INTO track_quality (track_id, status) VALUES (?, 'ok')"), vec!["track_quality"]);
        assert_eq!(t("REPLACE INTO settings VALUES ('a', 'b')"), vec!["settings"]);
        assert_eq!(t("UPDATE OR REPLACE tracks SET year='1' WHERE id=?"), vec!["tracks"]);
        assert_eq!(t("UPDATE playlist_items SET track_id=NULL WHERE track_id=(SELECT id FROM tracks WHERE path=?)"), vec!["playlist_items"]);
        assert_eq!(t("DELETE FROM tracks WHERE path=?"), vec!["tracks"]);
        assert_eq!(t("DELETE FROM activity WHERE id < (SELECT MAX(id) - 2000 FROM activity)"), vec!["activity"]);
        assert_eq!(t("WITH x AS (SELECT 1) UPDATE main.\"artist_alias\" SET alias='a'"), vec!["artist_alias"]);
        assert_eq!(t("CREATE TABLE IF NOT EXISTS login_links (digest TEXT); CREATE INDEX IF NOT EXISTS i ON tracks(lower(artist)); ALTER TABLE tracks ADD COLUMN x TEXT"), vec!["login_links", "tracks"]);
        // a comment that mentions the songs is not a write to them
        assert_eq!(t("-- bump tracks later\nINSERT INTO settings VALUES ('a','b')"), vec!["settings"]);
        // `--` inside a literal or a quoted name is not a comment: the write after it still counts
        assert_eq!(t("WITH x AS (SELECT 'a--b' AS v) UPDATE tracks SET year='1' -- tracks\n WHERE id=?"), vec!["tracks"]);
        assert_eq!(t("UPDATE settings SET value='--' WHERE key=?; DELETE FROM \"act--ivity\""), vec!["settings", "act--ivity"]);
        assert!(t("SELECT * FROM tracks").is_empty());

        let d = tmp("whitelist");
        let db = Db::open(&d).unwrap();
        let (lv, uv) = (db.library_version(), db.users_version());
        // the substring "tracks" used to move the version: a write to another table that only reads the songs doesn't
        db.execute("DELETE FROM track_quality WHERE track_id=-1", p![]).unwrap();
        db.execute("UPDATE playlist_items SET track_id=NULL WHERE track_id=(SELECT id FROM tracks WHERE path='nothing')", p![]).unwrap();
        db.execute("INSERT INTO plays (user_id, track_id, seconds, played_at) SELECT 1, id, 1, 1 FROM tracks WHERE id=-1", p![]).unwrap();
        assert_eq!((db.library_version(), db.users_version()), (lv, uv));
        db.execute("DELETE FROM tracks WHERE id=-1", p![]).unwrap();
        assert_eq!(db.library_version(), lv + 1);
        db.execute("DELETE FROM artist_alias WHERE alias_key='nothing'", p![]).unwrap();
        assert_eq!((db.library_version(), db.users_version()), (lv + 2, uv));
        // each album's type (the lists filter by it) is part of the library too
        assert_eq!(t("UPDATE album_types SET chosen='album' WHERE album_id=?"), vec!["album_types"]);
        db.execute("UPDATE album_types SET chosen='album' WHERE album_id='nothing'", p![]).unwrap();
        assert_eq!((db.library_version(), db.users_version()), (lv + 3, uv));
        // the accounts have their own counter (who has translations on)
        db.execute("UPDATE users SET prefs='{}' WHERE id=-1", p![]).unwrap();
        assert_eq!((db.library_version(), db.users_version()), (lv + 3, uv + 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The per-user totals (leaderboard, rank on Home) are counted from the requests, plays, playlists,
    /// their songs and the accounts: a write to any of them moves `stats_version`, others don't.
    #[test]
    fn the_stats_version_moves_for_writes_to_what_the_totals_count() {
        let d = tmp("statsversion");
        let db = Db::open(&d).unwrap();
        let mut v = db.stats_version();
        for sql in ["UPDATE tracks SET year='1' WHERE id=-1", "INSERT INTO settings VALUES ('s', '1')", "DELETE FROM track_quality WHERE track_id=-1"] {
            db.execute(sql, p![]).unwrap();
            assert_eq!(db.stats_version(), v, "{sql}");
        }
        for sql in [
            "INSERT INTO plays (user_id, track_id, seconds, played_at) SELECT 1, id, 1, 1 FROM tracks WHERE id=-1",
            "UPDATE requests SET status='completed' WHERE id=-1",
            "DELETE FROM playlists WHERE id=-1",
            "UPDATE playlist_items SET position=0 WHERE id=-1",
            "UPDATE users SET disabled=1 WHERE id=-1",
        ] {
            db.execute(sql, p![]).unwrap();
            assert_eq!(db.stats_version(), v + 1, "{sql}");
            v += 1;
        }
        // inside a transaction: once, after the COMMIT
        db.write_now(|c| c.transaction(|c| {
            c.execute("DELETE FROM plays WHERE id=-1", p![])?;
            c.execute("DELETE FROM requests WHERE id=-1", p![])
        })).unwrap();
        assert_eq!(db.stats_version(), v + 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn read_connections_stay_off_on_a_network_file_system() {
        use encore_core::fsinfo::{classify, FsInfo};
        assert_eq!(readers_for(None, Some(classify(0xEF53))), (4, ReadersSource::Default));
        assert_eq!(readers_for(None, None), (4, ReadersSource::Default));
        assert_eq!(readers_for(None, Some(classify(0x6969))), (0, ReadersSource::NetworkFs("NFS")));
        assert_eq!(readers_for(None, Some(classify(0xFF53_4D42))), (0, ReadersSource::NetworkFs("CIFS")));
        assert_eq!(readers_for(None, Some(classify(0x6573_5546))), (0, ReadersSource::NetworkFs("FUSE")));
        // the variable wins either way; nonsense in it is ignored
        assert_eq!(readers_for(Some("2"), Some(classify(0x6969))), (2, ReadersSource::Env));
        assert_eq!(readers_for(Some(" 0 "), None), (0, ReadersSource::Env));
        assert_eq!(readers_for(Some("99"), None), (16, ReadersSource::Env));
        assert_eq!(readers_for(Some("many"), Some(FsInfo { name: "SMB", network: true })), (0, ReadersSource::NetworkFs("SMB")));
        assert_eq!(ReadersSource::NetworkFs("SMB").as_str(), "network");
    }

    #[test]
    fn schema_26_backs_up_before_adding_the_lookup_indexes() {
        const { assert!(SCHEMA_VERSION >= 26) };
        let d = tmp("idx26");
        drop(Db::open(&d).unwrap());
        // what a schema 25 server left: the 0.25.3 WIP's indexes may be there (they came without a bump),
        // the new ones are not; a song so that ANALYZE has something to count
        {
            let c = Connection::open(d.join("encore.db")).unwrap();
            c.execute_batch("DROP INDEX idx_tracks_lartist; DROP INDEX idx_requests_kind_song; DROP INDEX idx_requests_user; DROP INDEX idx_pli_track;
                             DROP INDEX idx_plays_track; DROP INDEX idx_plays_time; DROP INDEX idx_tracks_lalbumartist;
                             INSERT INTO settings VALUES ('kept', '1');
                             INSERT INTO tracks (path, title, artist, album, albumartist, album_id) VALUES ('/m/a.flac', 'A', 'Ann', 'Alb', 'Ann', 'x1');").unwrap();
            c.pragma_update(None, "user_version", 25).unwrap();
        }
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let backups = crate::backup::list(&d);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].kind, crate::backup::Kind::BeforeUpgrade);
        assert_eq!(crate::backup::validate_file(&backups[0].path).unwrap(), 25);
        let names: Vec<String> = db.query("SELECT name FROM sqlite_master WHERE type='index'", p![]).unwrap().iter().map(|r| r.s("name")).collect();
        for want in ["idx_tracks_lartist", "idx_requests_kind_song", "idx_requests_user", "idx_pli_track", "idx_plays_track", "idx_plays_time", "idx_tracks_lalbumartist"] {
            assert!(names.iter().any(|n| n == want), "{want} missing from {names:?}");
        }
        // ANALYZE ran after the migration: the planner has statistics, and takes the new index
        assert!(db.one("SELECT COUNT(*) n FROM sqlite_stat1", p![]).unwrap().unwrap().i("n") > 0);
        let plan = db.query("EXPLAIN QUERY PLAN SELECT id FROM tracks WHERE lower(artist)=lower(?)", p!["ann"]).unwrap();
        assert!(plan.iter().any(|r| r.s("detail").contains("idx_tracks_lartist")), "{plan:?}");
        assert_eq!(db.one("SELECT value FROM settings WHERE key='kept'", p![]).unwrap().unwrap().s("value"), "1");
        assert_eq!(db.one("SELECT title FROM tracks WHERE lower(artist)=lower('ANN')", p![]).unwrap().unwrap().s("title"), "A");
        // the statement cache and the WAL limit are on every connection
        assert_eq!(db.conn.lock().unwrap().query_row("PRAGMA journal_size_limit", [], |r| r.get::<_, i64>(0)).unwrap(), 67108864);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A database exactly as a schema 26 server (0.29, 0.30) leaves it: every migration up to 26 run on it,
    /// stamped 26, with songs in it.
    fn schema_26_database(d: &Path) {
        std::fs::create_dir_all(d).unwrap();
        let c = Connection::open(d.join("encore.db")).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        for (n, _, sql) in MIGRATIONS.iter().filter(|(n, _, _)| *n <= 26) {
            migrate(&c, sql).unwrap_or_else(|e| panic!("migration {n}: {e}"));
        }
        c.pragma_update(None, "user_version", 26).unwrap();
        c.execute_batch("INSERT INTO settings VALUES ('kept', '1');
                         INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, bitrate, duration)
                         VALUES (7, '/m/a.flac', 'A', 'Ann', 'Alb', 'Ann', 'x1', 'flac', 900, 200.5), (8, '/m/b.mp3', 'B', 'Ann', 'Alb', 'Ann', 'x1', 'mp3', 320, 180);").unwrap();
    }

    /// Schema 27 (0.31): the loudness, sample rate and bit depth columns on the songs (empty for the songs
    /// already there: the scan and the "Measure loudness" task fill them), the group listen tables, a backup
    /// first and ANALYZE after; opening it again changes nothing.
    #[test]
    fn schema_27_adds_loudness_and_group_listens_to_a_schema_26_database() {
        const { assert!(SCHEMA_VERSION >= 27) };
        let d = tmp("schema27");
        schema_26_database(&d);
        {
            let c = Connection::open(d.join("encore.db")).unwrap();
            let cols: Vec<String> = c.prepare("PRAGMA table_info(tracks)").unwrap().query_map([], |r| r.get::<_, String>(1)).unwrap().map(|r| r.unwrap()).collect();
            assert!(!cols.iter().any(|c| c == "loudness_source" || c == "sample_rate"), "a schema 26 database has none of them: {cols:?}");
        }
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let backups = crate::backup::list(&d);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].kind, crate::backup::Kind::BeforeUpgrade);
        assert_eq!(crate::backup::validate_file(&backups[0].path).unwrap(), 26);
        // the songs are kept, with nothing known yet about their loudness
        let t = db.one("SELECT * FROM tracks WHERE id=7", p![]).unwrap().unwrap();
        assert_eq!((t.s("title"), t.s("format"), t.i("bitrate")), ("A".to_string(), "flac".to_string(), 900));
        for col in ["rg_track_gain", "rg_track_peak", "rg_album_gain", "rg_album_peak", "loudness_lufs", "loudness_source", "sample_rate", "bit_depth"] {
            assert!(t.get(col).is_some_and(|v| v.is_null()), "{col}: {t:?}");
        }
        db.execute("UPDATE tracks SET rg_track_gain=-7.21, rg_track_peak=0.988, rg_album_gain=-6.84, rg_album_peak=1.0, loudness_lufs=-10.79,
                    loudness_source='tags', sample_rate=96000, bit_depth=24 WHERE id=7", p![]).unwrap();
        let t = db.one("SELECT rg_track_gain, loudness_source, sample_rate, bit_depth FROM tracks WHERE id=7", p![]).unwrap().unwrap();
        assert_eq!((t.f("rg_track_gain"), t.s("loudness_source"), t.i("sample_rate"), t.i("bit_depth")), (-7.21, "tags".to_string(), 96000, 24));
        // the "Measure loudness" task's look-up of the songs it has not looked at goes through its index
        let plan = db.query("EXPLAIN QUERY PLAN SELECT id FROM tracks WHERE loudness_source IS NULL AND id > ? ORDER BY id LIMIT 200", p![0]).unwrap();
        assert!(plan.iter().any(|r| r.s("detail").contains("idx_tracks_loudness")), "{plan:?}");
        assert!(db.one("SELECT COUNT(*) n FROM sqlite_stat1", p![]).unwrap().unwrap().i("n") > 0, "ANALYZE ran after the migration");
        // a group listen with its members; ending it takes the members with it
        db.execute("INSERT INTO group_listens (id, code, host_user_id, visibility, everyone_controls, guests_allowed, max_members, queue, idx, position, rev, started_at, created_at, updated_at)
                    VALUES ('g_1', 'K7QM4X', 1, 'link', 1, 0, 8, '[{\"id\":7,\"qid\":\"q1\"}]', 0, 63.25, 42, 1, 1, 2)", p![]).unwrap();
        db.execute("INSERT INTO group_listen_members (group_id, member, user_id, name, device_id, device_name, is_host, joined_at) VALUES ('g_1', 'm1', 1, 'Alex', 'd1', 'Firefox on Linux', 1, 1)", p![]).unwrap();
        db.execute("INSERT INTO group_listen_members (group_id, member, guest_id, name, joined_at) VALUES ('g_1', 'm2', 'gst_1', 'Kim', 2)", p![]).unwrap();
        let g = db.one("SELECT * FROM group_listens WHERE code='K7QM4X'", p![]).unwrap().unwrap();
        assert_eq!((g.s("visibility"), g.i("max_members"), g.f("position"), g.i("rev"), g.s("data")), ("link".to_string(), 8, 63.25, 42, "{}".to_string()));
        assert_eq!(db.one("SELECT COUNT(*) n FROM group_listen_members WHERE group_id='g_1'", p![]).unwrap().unwrap().i("n"), 2);
        db.execute("DELETE FROM group_listens WHERE id='g_1'", p![]).unwrap();
        assert_eq!(db.one("SELECT COUNT(*) n FROM group_listen_members", p![]).unwrap().unwrap().i("n"), 0);
        assert_eq!(db.one("SELECT value FROM settings WHERE key='kept'", p![]).unwrap().unwrap().s("value"), "1");
        drop(db);
        // opened again: same version, no second backup, the values stay
        let db = Db::open(&d).unwrap();
        assert_eq!(crate::backup::list(&d).len(), 1);
        assert_eq!(db.one("SELECT sample_rate FROM tracks WHERE id=7", p![]).unwrap().unwrap().i("sample_rate"), 96000);
        // and it ends up like a new database
        let fresh = Connection::open_in_memory().unwrap();
        for (_, _, sql) in MIGRATIONS {
            migrate(&fresh, sql).unwrap();
        }
        assert_eq!(layout(&db.conn.lock().unwrap()), layout(&fresh));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Schema 28 (0.32): the plugins table, empty on an upgrade (nothing is installed until the first start of
    /// 0.32 adopts what was in use), a backup first; the settings and songs stay; opening it again changes nothing.
    #[test]
    fn schema_28_adds_the_plugins_table_to_a_schema_27_database() {
        const { assert!(SCHEMA_VERSION >= 28) };
        let d = tmp("schema28");
        {
            std::fs::create_dir_all(&d).unwrap();
            let c = Connection::open(d.join("encore.db")).unwrap();
            c.pragma_update(None, "journal_mode", "WAL").unwrap();
            for (n, _, sql) in MIGRATIONS.iter().filter(|(n, _, _)| *n <= 27) {
                migrate(&c, sql).unwrap_or_else(|e| panic!("migration {n}: {e}"));
            }
            c.pragma_update(None, "user_version", 27).unwrap();
            c.execute_batch("INSERT INTO settings VALUES ('ytdlp_enabled', 'true');").unwrap();
            let has: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='plugins'", [], |r| r.get(0)).unwrap();
            assert_eq!(has, 0, "a schema 27 database has no plugins table");
        }
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let backups = crate::backup::list(&d);
        assert_eq!(backups.len(), 1);
        assert_eq!(crate::backup::validate_file(&backups[0].path).unwrap(), 27);
        assert_eq!(db.one("SELECT COUNT(*) n FROM plugins", p![]).unwrap().unwrap().i("n"), 0);
        let cols: Vec<String> = db.query("PRAGMA table_info(plugins)", p![]).unwrap().iter().map(|r| r.s("name")).collect();
        assert_eq!(cols, ["id", "version", "enabled", "installed_at", "updated_at", "origin", "trust", "manifest", "sha256", "auto_update", "last_check", "last_error"]);
        db.execute("INSERT INTO plugins (id, version, installed_at, updated_at, origin, trust, manifest, sha256) VALUES ('example-tone', '0.1.0', 1, 1, 'upload', 'third-party', '[plugin]', 'ab')", p![]).unwrap();
        let r = db.one("SELECT * FROM plugins WHERE id='example-tone'", p![]).unwrap().unwrap();
        assert_eq!((r.i("enabled"), r.i("auto_update"), r.s("origin"), r.get("last_error").cloned()), (1, 0, "upload".to_string(), Some(Value::Null)));
        assert!(db.execute("INSERT INTO plugins (id) VALUES ('example-tone')", p![]).is_err(), "one row per plugin");
        assert_eq!(db.one("SELECT value FROM settings WHERE key='ytdlp_enabled'", p![]).unwrap().unwrap().s("value"), "true");
        drop(db);
        let db = Db::open(&d).unwrap();
        assert_eq!(crate::backup::list(&d).len(), 1);
        assert_eq!(db.one("SELECT version FROM plugins WHERE id='example-tone'", p![]).unwrap().unwrap().s("version"), "0.1.0");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn restores_a_backup_in_place() {
        let d = tmp("restore");
        let db = Db::open(&d).unwrap();
        db.execute("INSERT INTO settings VALUES ('k', '1')", p![]).unwrap();
        let b = db.backup(crate::backup::Kind::Scheduled).unwrap();
        db.execute("UPDATE settings SET value='2' WHERE key='k'", p![]).unwrap();
        let safety = db.restore(&b.path).unwrap();
        assert_eq!(safety.kind, crate::backup::Kind::BeforeRestore);
        assert_eq!(db.one("SELECT value FROM settings WHERE key='k'", p![]).unwrap().unwrap().s("value"), "1");
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        // the safety copy has the data from just before the restore
        let c = Connection::open(&safety.path).unwrap();
        let v: String = c.query_row("SELECT value FROM settings WHERE key='k'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "2");
        // a file from a newer Encore is refused
        let future = d.join("future.db");
        std::fs::copy(&b.path, &future).unwrap();
        Connection::open(&future).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();
        assert!(db.restore(&future).unwrap_err().to_string().contains("newer Encore"));
        assert!(db.restore(&d.join("missing.db")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_restore_leaves_a_working_database() {
        let d = tmp("restore-fail");
        let db = Db::open(&d).unwrap();
        db.execute("INSERT INTO settings VALUES ('k', 'before')", p![]).unwrap();
        // a sound SQLite file whose tracks table can't be migrated (no album_id → the index fails)
        let bad = d.join("bad.db");
        {
            let c = Connection::open(&bad).unwrap();
            c.execute_batch("CREATE TABLE tracks (id); PRAGMA user_version = 1;").unwrap();
        }
        let err = db.restore(&bad).unwrap_err().to_string();
        assert!(err.contains("restore failed"), "{err}");
        assert!(err.contains("before-restore"), "{err}");
        // the connection behind the lock is a real one again, with the data from before
        assert_eq!(db.one("SELECT value FROM settings WHERE key='k'", p![]).unwrap().unwrap().s("value"), "before");
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        assert!(!d.join("encore.db.restoring").exists());
        // a missing source fails before anything is touched: no safety copy, nothing changed
        let n = crate::backup::list(&d).len();
        assert!(db.restore(&d.join("missing.db")).is_err());
        assert_eq!(crate::backup::list(&d).len(), n);
        assert_eq!(db.one("SELECT value FROM settings WHERE key='k'", p![]).unwrap().unwrap().s("value"), "before");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Every file in migrations/ is listed in MIGRATIONS under its number and name, the numbers go up,
    /// and the last one is SCHEMA_VERSION.
    #[test]
    fn migration_files_are_numbered_and_listed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut files: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        files.sort();
        let listed: Vec<String> = MIGRATIONS.iter().map(|(n, name, _)| format!("{n:04}_{name}.sql")).collect();
        assert_eq!(files, listed);
        assert!(MIGRATIONS.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(MIGRATIONS.last().unwrap().0, SCHEMA_VERSION);
        for (n, name, sql) in MIGRATIONS {
            assert_eq!(std::fs::read_to_string(dir.join(format!("{n:04}_{name}.sql"))).unwrap(), *sql);
        }
    }

    /// Tables with their columns, and the indexes, of a database.
    fn layout(c: &Connection) -> Vec<String> {
        let mut out = vec![];
        let mut q = c.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name").unwrap();
        let rows: Vec<(String, String, String, Option<String>)> = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(|r| r.unwrap()).collect();
        for (kind, name, table, sql) in rows {
            if kind == "table" {
                let mut t = c.prepare(&format!("PRAGMA table_info({name})")).unwrap();
                let cols: Vec<String> = t.query_map([], |r| Ok(format!("{} {} {} {:?} {}", r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?, r.get::<_, i64>(5)?))).unwrap().map(|r| r.unwrap()).collect();
                out.push(format!("table {name}: {}", cols.join(", ")));
            } else {
                out.push(format!("{kind} {name} on {table}: {}", sql.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ")));
            }
        }
        out
    }

    /// The files may run any number of times: a second run changes nothing.
    #[test]
    fn migrations_run_again_change_nothing() {
        let c = Connection::open_in_memory().unwrap();
        for (_, _, sql) in MIGRATIONS {
            migrate(&c, sql).unwrap();
        }
        let once = layout(&c);
        assert!(once.iter().any(|t| t.starts_with("table tracks: ") && t.contains("isrc TEXT")), "{once:?}");
        assert!(once.iter().any(|t| t.starts_with("table shares: ") && t.contains("kind TEXT 0 Some(\"'track'\")")), "{once:?}");
        for (_, _, sql) in MIGRATIONS {
            migrate(&c, sql).unwrap();
        }
        assert_eq!(layout(&c), once);
    }

    /// A database from before the version stamp (tables without the columns added later): every column,
    /// index and later table is added, and it ends up like a new one.
    #[test]
    fn a_database_from_before_the_stamp_gets_everything() {
        let d = tmp("pre20");
        {
            // schema 20's tables as an early server made them: none of the columns added one by one
            let c = Connection::open(d.join("encore.db")).unwrap();
            for stmt in statements(MIGRATIONS[0].2) {
                let up = stmt.to_uppercase();
                if up.starts_with("CREATE TABLE") {
                    c.execute_batch(&stmt).unwrap();
                }
            }
            c.execute_batch("INSERT INTO settings VALUES ('kept', '1')").unwrap();
        }
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.one("SELECT value FROM settings WHERE key='kept'", p![]).unwrap().unwrap().s("value"), "1");
        let fresh = Connection::open_in_memory().unwrap();
        for (_, _, sql) in MIGRATIONS {
            migrate(&fresh, sql).unwrap();
        }
        let old = layout(&db.conn.lock().unwrap());
        assert_eq!(old.iter().filter(|l| !l.starts_with("table sqlite")).cloned().collect::<Vec<_>>(), layout(&fresh));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn migration_statements_split_at_semicolons_outside_quotes_and_comments() {
        let sql = "-- a comment; with a semicolon\nCREATE TABLE a (x TEXT DEFAULT ';', -- trailing; comment\n y \"q;\");\n\nALTER TABLE a ADD COLUMN z INT;";
        assert_eq!(statements(sql), vec!["CREATE TABLE a (x TEXT DEFAULT ';', \n y \"q;\")".to_string(), "ALTER TABLE a ADD COLUMN z INT".to_string()]);
        assert_eq!(added_column("ALTER TABLE a ADD COLUMN z INT"), Some(("a".into(), "z".into())));
        assert_eq!(added_column("alter table \"a\" add z"), Some(("a".into(), "z".into())));
        assert_eq!(added_column("CREATE TABLE a (z)"), None);
    }

    /// `write` writes on the write connection and moves the versions; `read` reads on a read connection
    /// and refuses anything else; errors and panics come back as errors, and the writer still works after.
    #[tokio::test]
    async fn read_and_write_lend_a_connection_on_the_blocking_pool() {
        let d = tmp("lend");
        let db = Db::open(&d).unwrap();
        let (lib, users) = (db.library_version(), db.users_version());
        let id = db.write(|c| {
            c.execute("INSERT INTO tracks (path, title, artist, album, album_id) VALUES ('/m/1.flac', 'One', 'A', 'B', 'ab')", p![])?;
            c.one("SELECT id FROM tracks WHERE path='/m/1.flac'", p![]).map(|r| r.map(|r| r.i("id")).unwrap_or(0))
        }).await.unwrap();
        assert!(id > 0);
        assert_ne!(db.library_version(), lib, "a write to the songs moves the library version");
        assert_eq!(db.users_version(), users, "nothing written to the accounts");
        let rows = db.read(move |c| c.query("SELECT title FROM tracks WHERE id=?", p![id])).await.unwrap();
        assert_eq!(rows[0].s("title"), "One");
        // a read connection does not write, not even through query()
        let e = db.read(|c| c.execute("DELETE FROM tracks", p![])).await.unwrap_err();
        assert!(e.to_string().contains("only reads"), "{e}");
        assert!(db.read(|c| c.query("UPDATE tracks SET title='x'", p![])).await.is_err());
        assert_eq!(db.one("SELECT title FROM tracks WHERE id=?", p![id]).unwrap().unwrap().s("title"), "One");
        // the closure's own error type comes back as it is
        #[derive(Debug, PartialEq)]
        enum Mine {
            NotFound,
            Db(String),
        }
        impl From<rusqlite::Error> for Mine {
            fn from(e: rusqlite::Error) -> Self {
                Mine::Db(e.to_string())
            }
        }
        let mine: Result<(), Mine> = db.read(|_| Err(Mine::NotFound)).await;
        assert_eq!(mine.unwrap_err(), Mine::NotFound);
        let mine: Result<Vec<Row>, Mine> = db.read(|c| Ok(c.query("SELECT nothing FROM nowhere", p![])?)).await;
        assert!(matches!(mine, Err(Mine::Db(_))));
        // a panic in a write: an error, and the write connection is not poisoned
        let e = db.write(|_| -> rusqlite::Result<()> { panic!("boom") }).await.unwrap_err();
        assert!(e.to_string().contains("boom"), "{e}");
        db.execute("UPDATE tracks SET title='Two' WHERE id=?", p![id]).unwrap();
        assert_eq!(db.write(|c| c.one("SELECT title FROM tracks", p![])).await.unwrap().unwrap().s("title"), "Two");
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A transaction is saved whole or not at all.
    #[tokio::test]
    async fn a_transaction_keeps_all_or_nothing() {
        let d = tmp("txn");
        let db = Db::open(&d).unwrap();
        db.write(|c| c.transaction(|c| {
            c.execute("INSERT INTO settings VALUES ('a', '1')", p![])?;
            c.execute("INSERT INTO settings VALUES ('b', '2')", p![])
        })).await.unwrap();
        let failed = db.write(|c| c.transaction(|c| {
            c.execute("INSERT INTO settings VALUES ('c', '3')", p![])?;
            c.execute("INSERT INTO settings VALUES ('a', 'again')", p![])   // the key is taken
        })).await;
        assert!(failed.is_err());
        let keys: Vec<String> = db.query("SELECT key FROM settings WHERE key IN ('a','b','c') ORDER BY key", p![]).unwrap().iter().map(|r| r.s("key")).collect();
        assert_eq!(keys, vec!["a", "b"]);
        // a read connection has no transaction of its own to write in
        assert!(db.read(|c| c.transaction(|_| Ok::<_, rusqlite::Error>(()))).await.is_err());
        // a panic inside a transaction: rolled back, and the next write is saved on its own
        let e = db.write(|c| c.transaction(|c| -> rusqlite::Result<()> {
            c.execute("INSERT INTO settings VALUES ('d', '4')", p![])?;
            panic!("half way")
        })).await.unwrap_err();
        assert!(e.to_string().contains("half way"), "{e}");
        db.write(|c| c.execute("INSERT INTO settings VALUES ('e', '5')", p![])).await.unwrap();
        let keys: Vec<String> = db.query("SELECT key FROM settings WHERE key IN ('d','e') ORDER BY key", p![]).unwrap().iter().map(|r| r.s("key")).collect();
        assert_eq!(keys, vec!["e"]);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A transaction moves the library version once, after its COMMIT (a scan batch of 200 songs is one
    /// change for the caches, and no cache is built from rows a reader can't see yet under the new number);
    /// one that fails moves nothing. Statements outside a transaction move it each, as before.
    #[test]
    fn a_transaction_moves_the_versions_once_after_it_commits() {
        let d = tmp("txbump");
        let db = Db::open(&d).unwrap();
        let v0 = db.library_version();
        let seen_inside = db.write_now(|c| c.transaction(|c| {
            for i in 0..5 {
                c.execute("INSERT INTO tracks (path, title, added_at) VALUES (?, 't', ?)", p![format!("/m/{i}.flac"), i])?;
            }
            c.execute("UPDATE tracks SET year='2001' WHERE path='/m/1.flac'", p![])?;
            Ok::<_, rusqlite::Error>(db.library_version())
        })).unwrap();
        assert_eq!(seen_inside, v0, "nothing moves before the COMMIT");
        assert_eq!(db.library_version(), v0 + 1, "one move for the whole batch");
        // a failed transaction: nothing saved, nothing moved
        let r = db.write_now(|c| c.transaction(|c| {
            c.execute("INSERT INTO tracks (path, title) VALUES ('/m/9.flac', 'x')", p![])?;
            c.execute("INSERT INTO no_such_table VALUES (1)", p![])
        }));
        assert!(r.is_err());
        assert_eq!(db.library_version(), v0 + 1);
        assert!(db.one("SELECT 1 FROM tracks WHERE path='/m/9.flac'", p![]).unwrap().is_none());
        // the accounts' version the same way; a transaction on other tables moves neither
        let u0 = db.users_version();
        db.write_now(|c| c.transaction(|c| c.execute("UPDATE users SET prefs='{}'", p![]))).unwrap();
        assert_eq!(db.users_version(), u0 + 1);
        db.write_now(|c| c.transaction(|c| c.execute("INSERT INTO settings VALUES ('tx', '1')", p![]))).unwrap();
        assert_eq!((db.library_version(), db.users_version()), (v0 + 1, u0 + 1));
        // outside a transaction each write moves it
        db.write_now(|c| {
            c.execute("UPDATE tracks SET year='2002'", p![])?;
            c.execute("UPDATE tracks SET year='2003'", p![])
        }).unwrap();
        assert_eq!(db.library_version(), v0 + 3);
        // read_now lends a read connection on this thread
        let n = db.read_now(|c| c.one("SELECT COUNT(*) n FROM tracks", p![])).unwrap().unwrap().i("n");
        assert_eq!(n, 5);
        assert!(db.read_now(|c| c.execute("DELETE FROM tracks", p![])).is_err(), "a read connection writes nothing");
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Waits for a busy connection are counted (`/api/admin/perf`): a read waits only when every read
    /// connection is held, a write when another write holds the write connection.
    #[test]
    fn waits_for_a_connection_are_counted() {
        let d = tmp("waits");
        let db = Db::open(&d).unwrap();
        let w = db.pool_waits();
        assert_eq!((w["read"]["waits"].as_u64(), w["write"]["waits"].as_u64()), (Some(0), Some(0)));
        assert_eq!(w["readers"].as_u64(), Some(db.readers_in_effect().0 as u64));
        // hold the write connection a moment while another thread writes
        let held = db.writer();
        let t = {
            let db = db.clone();
            std::thread::spawn(move || db.execute("INSERT INTO settings VALUES ('w', '1')", p![]).unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(30));
        drop(held);
        t.join().unwrap();
        let w = db.pool_waits();
        assert_eq!(w["write"]["waits"].as_u64(), Some(1));
        assert!(w["write"]["max_ms"].as_f64().unwrap() >= 20.0, "{w}");
        // every read connection held: the next read waits for one
        if db.readers_in_effect().0 > 0 {
            let held: Vec<_> = db.readers.iter().map(|r| r.lock().unwrap()).collect();
            let t = {
                let db = db.clone();
                std::thread::spawn(move || db.one("SELECT 1 x", p![]).unwrap())
            };
            std::thread::sleep(std::time::Duration::from_millis(30));
            drop(held);
            t.join().unwrap();
            assert_eq!(db.pool_waits()["read"]["waits"].as_u64(), Some(1));
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Reads go on while a write holds the write connection (WAL): a page never waits for a long write.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_do_not_wait_for_a_write() {
        let d = tmp("nowait");
        let db = Db::open(&d).unwrap();
        db.execute("INSERT INTO settings VALUES ('k', '1')", p![]).unwrap();
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let writer = {
            let db = db.clone();
            tokio::spawn(async move {
                db.write(move |c| {
                    c.execute("UPDATE settings SET value='2' WHERE key='k'", p![])?;
                    held_tx.send(()).unwrap();
                    // hold the write connection until the read below has finished
                    go_rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
                    Ok::<_, rusqlite::Error>(())
                }).await
            })
        };
        tokio::task::spawn_blocking(move || held_rx.recv().unwrap()).await.unwrap();
        let read = tokio::time::timeout(std::time::Duration::from_secs(10), db.read(|c| c.one("SELECT value FROM settings WHERE key='k'", p![]))).await;
        go_tx.send(()).unwrap();
        let v = read.expect("the read waited for the write").unwrap().unwrap();
        assert_eq!(v.s("value"), "2", "the write connection commits each statement");
        writer.await.unwrap().unwrap();
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn schema_30_adds_the_surround_library_to_a_schema_29_database() {
        const { assert!(SCHEMA_VERSION >= 30) };
        let d = tmp("surround");
        drop(Db::open(&d).unwrap());
        // what a schema 29 server left: no surround tables, requests without a class
        {
            let c = Connection::open(d.join("encore.db")).unwrap();
            c.execute_batch(
                "DROP TABLE surround_tracks; DROP TABLE surround_checks; DELETE FROM sqlite_sequence WHERE name='surround_tracks';
                 DROP INDEX idx_requests_class; ALTER TABLE requests DROP COLUMN class;
                 INSERT INTO tracks (path, title) VALUES ('/music/a.flac', 'A');
                 INSERT INTO requests (user_id, kind, status, created_at, updated_at) VALUES (1, 'track', 'completed', 1, 1);",
            )
            .unwrap();
            c.pragma_update(None, "user_version", 29).unwrap();
        }
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        // an old request is a stereo one; surround ids start at a billion, apart from every stereo id
        assert_eq!(db.one("SELECT class FROM requests", p![]).unwrap().unwrap().s("class"), "stereo");
        let id = db.execute("INSERT INTO surround_tracks (path, title, channels, atmos) VALUES ('/music/Surround/a.m4a', 'A', 6, 1)", p![]).unwrap();
        assert_eq!(id, 1_000_000_000);
        // every column of tracks is a column of surround_tracks (the scanner writes either)
        let cols = |t: &str| -> Vec<String> { db.query(&format!("PRAGMA table_info({t})"), p![]).unwrap().iter().map(|r| r.s("name")).collect() };
        let surround = cols("surround_tracks");
        for c in cols("tracks") {
            assert!(surround.contains(&c), "surround_tracks has no {c}");
        }
        drop(db);
        // opening again changes nothing (the seed runs once)
        let db = Db::open(&d).unwrap();
        assert_eq!(db.execute("INSERT INTO surround_tracks (path) VALUES ('/music/Surround/b.m4a')", p![]).unwrap(), 1_000_000_001);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Schema 31 fills in who added the songs already there from the requests still in the table: a request
    /// (its user and day), one whose row was cleared (no name), a link, a Soulseek-page download, a song no
    /// request brought (found in the music folder), a request whose detail isn't JSON; running it again changes
    /// nothing.
    #[test]
    fn schema_31_backfills_who_added() {
        const { assert!(SCHEMA_VERSION >= 31) };
        let d = tmp("who-added");
        drop(Db::open(&d).unwrap());
        // what a schema 30 server left: tracks without the who-added columns
        {
            let c = Connection::open(d.join("encore.db")).unwrap();
            c.execute_batch(
                "DROP INDEX idx_tracks_added_by; DROP TABLE album_types;
                 ALTER TABLE tracks DROP COLUMN added_by; ALTER TABLE tracks DROP COLUMN added_via; ALTER TABLE tracks DROP COLUMN requested_at;
                 ALTER TABLE tracks DROP COLUMN releasetype; ALTER TABLE tracks DROP COLUMN tracktotal; ALTER TABLE tracks DROP COLUMN compilation;
                 INSERT INTO requests (id, user_id, kind, status, created_at, updated_at, detail) VALUES (1, 3, 'album', 'completed', 100, 100, '{}');
                 INSERT INTO requests (id, user_id, kind, status, created_at, updated_at, detail) VALUES (2, 4, 'url', 'completed', 200, 200, '{}');
                 INSERT INTO requests (id, user_id, kind, mbid, status, created_at, updated_at, detail)
                     VALUES (3, 5, 'track', NULL, 'completed', 300, 300, '{\"pick\": {\"source\": \"slskd\", \"payload\": {\"username\": \"peer\"}}}');
                 INSERT INTO requests (id, user_id, kind, status, created_at, updated_at, detail) VALUES (4, 6, 'track', 'completed', 400, 400, 'not json');
                 INSERT INTO tracks (id, path, title, request_id) VALUES (1, '/m/a.flac', 'A', 1);
                 INSERT INTO tracks (id, path, title, request_id, source) VALUES (2, '/m/b.flac', 'B', 99, 'slskd');
                 INSERT INTO tracks (id, path, title, request_id) VALUES (3, '/m/c.flac', 'C', 2);
                 INSERT INTO tracks (id, path, title, request_id) VALUES (4, '/m/d.flac', 'D', 3);
                 INSERT INTO tracks (id, path, title) VALUES (5, '/m/e.flac', 'E');
                 INSERT INTO tracks (id, path, title, request_id) VALUES (6, '/m/f.flac', 'F', 4);",
            )
            .unwrap();
            c.pragma_update(None, "user_version", 30).unwrap();
        }
        let db = Db::open(&d).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let rows = |db: &Db| -> Vec<(i64, Option<i64>, String, Option<f64>)> {
            db.query("SELECT id, added_by, added_via, requested_at FROM tracks ORDER BY id", p![]).unwrap().iter()
                .map(|r| (r.i("id"), r.get("added_by").and_then(|v| v.as_i64()), r.s("added_via"), r.get("requested_at").and_then(|v| v.as_f64()))).collect()
        };
        let want = vec![
            (1, Some(3), "request".to_string(), Some(100.0)),
            (2, None, "request".to_string(), None),
            (3, Some(4), "link".to_string(), Some(200.0)),
            (4, Some(5), "soulseek".to_string(), Some(300.0)),
            (5, None, "scan".to_string(), None),
            (6, Some(6), "request".to_string(), Some(400.0)),
        ];
        assert_eq!(rows(&db), want);
        // the album types table is there, empty
        assert_eq!(db.one("SELECT COUNT(*) n FROM album_types", p![]).unwrap().unwrap().i("n"), 0);
        // the request rows go (Clear finished): what the songs say stays; opening again changes nothing
        db.execute("DELETE FROM requests", p![]).unwrap();
        drop(db);
        let db = Db::open(&d).unwrap();
        assert_eq!(rows(&db), want);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}
