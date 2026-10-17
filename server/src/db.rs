//! SQLite, same file and tables as older servers, so upgrading keeps everything.
//! Rows come back as JSON objects, which is what the API returns anyway.
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{params_from_iter, Connection, OpenFlags};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub type Row = Map<String, Value>;

/// One connection for writes (they are serialized here, as SQLite would anyway) and a few for reads:
/// in WAL mode reads never wait for a write or for each other, so a long scan or a background job no
/// longer holds up every page that loads meanwhile.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    readers: Arc<Vec<Mutex<Connection>>>,
    next_reader: Arc<AtomicUsize>,
    /// goes up with every write to the songs or the artist aliases (see `library_version`)
    library_writes: Arc<AtomicU64>,
    /// goes up with every write to the accounts (see `users_version`)
    user_writes: Arc<AtomicU64>,
    /// why there are as many read connections as there are (Diagnostics)
    readers_source: ReadersSource,
    pub data_dir: PathBuf,
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
    readers_for(std::env::var("ENCORE_DB_READERS").ok().as_deref(), crate::fsinfo::of(data_dir))
}

fn readers_for(env: Option<&str>, fs: Option<crate::fsinfo::FsInfo>) -> (usize, ReadersSource) {
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

const SCHEMA: &str = include_str!("schema.sql");

/// The schema this server writes, stamped into `PRAGMA user_version`. BUMP IT whenever SCHEMA or
/// MIGRATIONS change: an older server that finds a higher number refuses to touch the file (it would
/// otherwise fail half-way through and leave a database neither version can read), and a lower number
/// means "back up, then migrate".
/// 24: round 3 (release types on requests; source blocks, track quality, contributions – see schema.sql).
/// 25: translated and romanized titles (translations).
/// 26: the lookup indexes of the 0.25.3 speed work (which added them without a bump, so no backup was
///     taken first) plus six more (artist, request and play lookups), and ANALYZE after the migration.
pub const SCHEMA_VERSION: i64 = 26;

/// Exit code of the server when the database comes from a newer Encore.
pub const EXIT_NEWER_DB: i32 = 3;

/// The database was written by a newer Encore than this one.
#[derive(Debug)]
pub struct NewerDatabase {
    pub found: i64,
    pub known: i64,
    pub backups: PathBuf,
}

impl std::fmt::Display for NewerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "This database was written by a newer Encore (schema {}, this server knows {}). Start the newer image again, or restore a backup from {}. Nothing was changed.",
            self.found, self.known, self.backups.display())
    }
}
impl std::error::Error for NewerDatabase {}

/// Columns added after the first release (old databases get them on start).
const MIGRATIONS: &[(&str, &str, &str)] = &[
    ("tracks", "isrc", "TEXT"),
    ("tracks", "acoustid_id", "TEXT"),
    ("tracks", "verify_status", "TEXT"),
    ("tracks", "verify_detail", "TEXT"),
    ("tracks", "verified_at", "REAL"),
    ("tracks", "source", "TEXT"),
    ("tracks", "source_detail", "TEXT"),
    ("tracks", "source_url", "TEXT"),
    ("tracks", "request_id", "INTEGER"),
    ("tracks", "downloaded_at", "REAL"),
    ("playlists", "remote_ids", "TEXT DEFAULT '{}'"),
    ("users", "auto_download", "INTEGER NOT NULL DEFAULT 1"),
    ("users", "prefs", "TEXT NOT NULL DEFAULT '{}'"),
    ("users", "perms", "TEXT NOT NULL DEFAULT '{}'"),
    // columns added by newer servers
    ("users", "totp_secret", "TEXT"),
    ("users", "totp_enabled", "INTEGER NOT NULL DEFAULT 0"),
    ("users", "avatar_at", "REAL"),
    // app tokens (bearer sign-in for the native app): web sessions keep kind='web'
    ("sessions", "kind", "TEXT NOT NULL DEFAULT 'web'"),
    ("sessions", "device_id", "TEXT"),
    ("sessions", "device_name", "TEXT"),
    ("sessions", "last_seen_at", "REAL"),
    // schema 22: album and artist share links (track_id stays for songs; 0 for the others)
    ("shares", "kind", "TEXT DEFAULT 'track'"),
    ("shares", "ref", "TEXT"),
];

/// Pragmas, tables, missing columns and indexes, then the version stamp: run on every open.
fn prepare(conn: &Connection) -> anyhow::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    tune(conn)?;
    conn.execute_batch(SCHEMA)?;
    for (table, col, typ) in MIGRATIONS {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let have: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
        if !have.iter().any(|c| c == col) {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {col} {typ}"), [])?;
        }
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tracks_isrc ON tracks(isrc);
         CREATE INDEX IF NOT EXISTS idx_tracks_acoustid ON tracks(acoustid_id);
         CREATE INDEX IF NOT EXISTS sessions_user_device ON sessions(user_id, device_id);
         -- 0.25.3: lookups by name (is this album / song / artist in the library? artist pages, new
         -- releases) used to read every song; older servers ignore these indexes, so no schema bump
         CREATE INDEX IF NOT EXISTS idx_tracks_lalbum ON tracks(lower(album));
         CREATE INDEX IF NOT EXISTS idx_tracks_ltitle ON tracks(lower(title));
         CREATE INDEX IF NOT EXISTS idx_tracks_lalbumartist ON tracks(lower(albumartist));
         CREATE INDEX IF NOT EXISTS idx_tracks_albumartist ON tracks(albumartist);
         CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
         CREATE INDEX IF NOT EXISTS idx_tracks_mb_albumartist ON tracks(mb_albumartistid);
         CREATE INDEX IF NOT EXISTS idx_tracks_mb_artist ON tracks(mb_artistid);
         CREATE INDEX IF NOT EXISTS idx_requests_mbid ON requests(mbid);
         CREATE TABLE IF NOT EXISTS login_links (digest TEXT PRIMARY KEY, user_id INTEGER NOT NULL, expires_at REAL NOT NULL);
         -- schema 26: songs by artist (artist pages, car links, for-you), requests by song and by user
         -- (catalog and discover pages look a request up per row; the leaderboard counts per user),
         -- playlist rows and plays by song (deletes and the cover pass), plays by time (recent plays)
         CREATE INDEX IF NOT EXISTS idx_tracks_lartist ON tracks(lower(artist));
         CREATE INDEX IF NOT EXISTS idx_requests_kind_song ON requests(kind, lower(artist), lower(title));
         CREATE INDEX IF NOT EXISTS idx_requests_user ON requests(user_id);
         CREATE INDEX IF NOT EXISTS idx_pli_track ON playlist_items(track_id);
         CREATE INDEX IF NOT EXISTS idx_plays_track ON plays(track_id);
         CREATE INDEX IF NOT EXISTS idx_plays_time ON plays(played_at);",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
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
/// one of them moves `library_version`.
const LIBRARY_TABLES: &[&str] = &["tracks", "artist_alias"];
/// A write to one of these moves `users_version` (who has translations on, roles, prefs).
const USER_TABLES: &[&str] = &["users"];

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn to_sql(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(*b as i64),
        Value::Number(n) => n.as_i64().map(SqlValue::Integer).unwrap_or_else(|| SqlValue::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

fn from_sql(v: ValueRef) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
    }
}

/// `p![a, b, c]` – query parameters of any JSON-convertible type.
#[macro_export]
macro_rules! p {
    () => { Vec::<serde_json::Value>::new() };
    ($($x:expr),+ $(,)?) => { vec![$(serde_json::json!($x)),+] };
}

impl Db {
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
            crate::backup::create(&conn, data_dir, crate::backup::Kind::BeforeUpgrade)?;
        }
        prepare(&conn)?;
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
            user_writes: Arc::new(AtomicU64::new(instance)), readers_source, data_dir: data_dir.to_path_buf() })
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

    fn note_write(&self, sql: &str) {
        let tables = tables_written(sql);
        if tables.iter().any(|t| LIBRARY_TABLES.contains(&t.as_str())) {
            self.library_writes.fetch_add(1, Ordering::SeqCst);
        }
        if tables.iter().any(|t| USER_TABLES.contains(&t.as_str())) {
            self.user_writes.fetch_add(1, Ordering::SeqCst);
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
        run(&self.conn.lock().unwrap_or_else(|e| e.into_inner()));
        for r in self.readers.iter() {
            run(&r.lock().unwrap_or_else(|e| e.into_inner()));
        }
    }

    /// A free read connection (or, when all are busy, the next one in turn).
    fn reader(&self) -> MutexGuard<'_, Connection> {
        let n = self.readers.len();
        if n == 0 {
            return self.conn.lock().unwrap();
        }
        let start = self.next_reader.fetch_add(1, Ordering::Relaxed);
        for i in 0..n {
            match self.readers[(start + i) % n].try_lock() {
                Ok(g) => return g,
                Err(std::sync::TryLockError::Poisoned(e)) => return e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
        }
        self.readers[start % n].lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `PRAGMA user_version` of the open database.
    pub fn user_version(&self) -> rusqlite::Result<i64> {
        self.conn.lock().unwrap().query_row("PRAGMA user_version", [], |r| r.get(0))
    }

    /// Copies the database to <data_dir>/backups (see `backup`).
    pub fn backup(&self, kind: crate::backup::Kind) -> anyhow::Result<crate::backup::Info> {
        let conn = self.conn.lock().unwrap();
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

    pub fn execute_batch(&self, sql: &str) -> rusqlite::Result<()> {
        let r = self.conn.lock().unwrap().execute_batch(sql);
        self.note_write(sql);
        r
    }

    pub fn query(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Vec<Row>> {
        let read = is_read(sql);
        if !read {
            self.note_write(sql);
        }
        let conn = if read { self.reader() } else { self.conn.lock().unwrap() };
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

    pub fn one(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// Runs a statement; returns the last inserted row id.
    pub fn execute(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let r = conn.execute(sql, params_from_iter(params.iter().map(to_sql)));
        // counted after the write too: a reader that looked between the two sees the change next time
        self.note_write(sql);
        r?;
        Ok(conn.last_insert_rowid())
    }

    pub fn log(&self, level: &str, message: &str) {
        let _ = self.execute("INSERT INTO activity (ts, level, message) VALUES (?,?,?)", p![now(), level, message]);
        let _ = self.execute("DELETE FROM activity WHERE id < (SELECT MAX(id) - 2000 FROM activity)", p![]);
    }
}

/// Small accessors for JSON rows.
pub trait RowExt {
    fn s(&self, k: &str) -> String;
    fn i(&self, k: &str) -> i64;
    fn f(&self, k: &str) -> f64;
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

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("encore-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

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
        assert!(SCHEMA_VERSION >= 21);
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
        assert!(SCHEMA_VERSION >= 22);
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
        assert!(old.get("ref").map_or(true, |v| v.is_null()));
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
        // the accounts have their own counter (who has translations on)
        db.execute("UPDATE users SET prefs='{}' WHERE id=-1", p![]).unwrap();
        assert_eq!((db.library_version(), db.users_version()), (lv + 2, uv + 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn read_connections_stay_off_on_a_network_file_system() {
        use crate::fsinfo::{classify, FsInfo};
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
        assert!(SCHEMA_VERSION >= 26);
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
}
