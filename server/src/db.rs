//! SQLite, shared with the previous Python server: same file, same tables, so upgrading keeps everything.
//! Rows come back as JSON objects, which is what the API returns anyway.
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{params_from_iter, Connection};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type Row = Map<String, Value>;

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    pub data_dir: PathBuf,
}

const SCHEMA: &str = include_str!("schema.sql");

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
    // Rust server additions
    ("users", "totp_secret", "TEXT"),
    ("users", "totp_enabled", "INTEGER NOT NULL DEFAULT 0"),
    ("users", "avatar_at", "REAL"),
    // app tokens (bearer sign-in for the native app): web sessions keep kind='web'
    ("sessions", "kind", "TEXT NOT NULL DEFAULT 'web'"),
    ("sessions", "device_id", "TEXT"),
    ("sessions", "device_name", "TEXT"),
    ("sessions", "last_seen_at", "REAL"),
];

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
        let conn = Connection::open(data_dir.join("encore.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(SCHEMA)?;
        let db = Db { conn: Arc::new(Mutex::new(conn)), data_dir: data_dir.to_path_buf() };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        for (table, col, typ) in MIGRATIONS {
            let have: Vec<String> = self.query(&format!("PRAGMA table_info({table})"), p![])?
                .into_iter().filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(String::from)).collect();
            if !have.iter().any(|c| c == col) {
                self.execute(&format!("ALTER TABLE {table} ADD COLUMN {col} {typ}"), p![])?;
            }
        }
        self.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_tracks_isrc ON tracks(isrc);
             CREATE INDEX IF NOT EXISTS idx_tracks_acoustid ON tracks(acoustid_id);
             CREATE INDEX IF NOT EXISTS sessions_user_device ON sessions(user_id, device_id);
             CREATE TABLE IF NOT EXISTS login_links (digest TEXT PRIMARY KEY, user_id INTEGER NOT NULL, expires_at REAL NOT NULL);",
        )?;
        Ok(())
    }

    pub fn execute_batch(&self, sql: &str) -> rusqlite::Result<()> {
        self.conn.lock().unwrap().execute_batch(sql)
    }

    pub fn query(&self, sql: &str, params: Vec<Value>) -> rusqlite::Result<Vec<Row>> {
        let conn = self.conn.lock().unwrap();
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
        conn.execute(sql, params_from_iter(params.iter().map(to_sql)))?;
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
