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
    pub data_dir: PathBuf,
}

/// Read connections next to the write one (ENCORE_DB_READERS; 0 = every statement on one connection,
/// as servers before 0.25.3 did).
fn readers_wanted() -> usize {
    std::env::var("ENCORE_DB_READERS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(4).min(16)
}

/// Per-connection settings: WAL is safe with synchronous=NORMAL (a power cut can lose the last
/// commits, never corrupt the file); a bigger page cache and memory-mapped reads make the library
/// queries (60k songs) run from memory.
fn tune(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -16000)?;   // KiB: 16 MB per connection
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // mmap_size answers with the value it set: a pragma_update would see an unexpected row
    let _ = conn.query_row("PRAGMA mmap_size=268435456", [], |_| Ok(()));
    Ok(())
}

fn open_readers(path: &Path) -> rusqlite::Result<Vec<Connection>> {
    (0..readers_wanted()).map(|_| {
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
pub const SCHEMA_VERSION: i64 = 25;

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
         CREATE TABLE IF NOT EXISTS login_links (digest TEXT PRIMARY KEY, user_id INTEGER NOT NULL, expires_at REAL NOT NULL);",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

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
        let readers = open_readers(&path)?.into_iter().map(Mutex::new).collect();
        Ok(Db { conn: Arc::new(Mutex::new(conn)), readers: Arc::new(readers), next_reader: Arc::new(AtomicUsize::new(0)), library_writes: Arc::new(AtomicU64::new(next_instance() << 40)), data_dir: data_dir.to_path_buf() })
    }

    /// Changes whenever a statement that may write the songs or the artist aliases ran through this
    /// handle (every write in the server does): caches of library answers compare it instead of reading
    /// the whole table again. Writes made by another program (a test's own connection) don't count.
    pub fn library_version(&self) -> u64 {
        self.library_writes.load(Ordering::SeqCst)
    }

    fn note_write(&self, sql: &str) {
        let l = sql.to_ascii_lowercase();
        if l.contains("tracks") || l.contains("artist_alias") {
            self.library_writes.fetch_add(1, Ordering::SeqCst);
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
                *guard = conn;
                self.library_writes.fetch_add(1, Ordering::SeqCst);
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
        match open_readers(path) {
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
