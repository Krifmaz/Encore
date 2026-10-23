//! MusicBrainz's answers on disk: the look-ups (an artist, its release groups, a release …, never a search)
//! kept in a small SQLite file of their own next to the database (`musicbrainz-cache.db`), so a restart or an
//! hour away doesn't send every artist and album page back to MusicBrainz's one-call-a-second line. The
//! answers live in memory for an hour as before ([`crate::mb::Mb`]); this is what is found when memory has
//! nothing. Kept for [`KEEP_SECS`] and at most [`MAX_ROWS`] answers; the oldest go first. Losing the file loses
//! nothing but time: it is made again empty.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

/// How long an answer stays on disk at all (older ones are dropped when the file is opened).
pub const KEEP_SECS: f64 = 30.0 * 86400.0;
/// The most answers kept on disk; beyond it the oldest go (an artist's release groups are one answer per 100).
pub const MAX_ROWS: i64 = 50_000;

/// The file of answers. Clones share the connection.
#[derive(Clone)]
pub struct Store {
    /// the one connection to the file (the look-ups are a few a second at most)
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Opens (or makes) the file at `path` and drops the answers older than [`KEEP_SECS`]. None when the file
    /// can't be opened (a read-only folder): MusicBrainz's answers are then only kept in memory.
    pub fn open(path: &Path) -> Option<Store> {
        let open = || -> rusqlite::Result<Connection> {
            let c = Connection::open(path)?;
            c.busy_timeout(std::time::Duration::from_secs(2))?;
            // WAL: a read while an answer is written doesn't wait; NORMAL: a lost answer is only asked again
            c.pragma_update(None, "journal_mode", "WAL")?;
            c.pragma_update(None, "synchronous", "NORMAL")?;
            c.execute_batch("CREATE TABLE IF NOT EXISTS answers (key TEXT PRIMARY KEY, body TEXT NOT NULL, fetched_at REAL NOT NULL);
                             CREATE INDEX IF NOT EXISTS answers_fetched ON answers(fetched_at);")?;
            Ok(c)
        };
        match open() {
            Ok(c) => {
                let st = Store { conn: Arc::new(Mutex::new(c)) };
                st.prune(encore_db::now());
                Some(st)
            }
            Err(e) => {
                tracing::warn!("MusicBrainz answers are kept in memory only: {} can't be opened: {e}", path.display());
                None
            }
        }
    }

    /// The connection (a panic while it was held leaves it usable: each statement stands alone).
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The answer kept for `key` and when it was fetched (seconds since 1970), if one is.
    pub fn get(&self, key: &str) -> Option<(Value, f64)> {
        let row: Option<(String, f64)> = self.lock()
            .query_row("SELECT body, fetched_at FROM answers WHERE key=?", params![key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional().ok().flatten();
        let (body, at) = row?;
        serde_json::from_str(&body).ok().map(|v| (v, at))
    }

    /// Keeps `v` as the answer for `key`, fetched at `at` (replacing an older one).
    pub fn put(&self, key: &str, v: &Value, at: f64) {
        let r = self.lock().execute("INSERT OR REPLACE INTO answers (key, body, fetched_at) VALUES (?,?,?)", params![key, v.to_string(), at]);
        if let Err(e) = r {
            tracing::debug!("MusicBrainz answer not kept on disk: {e}");
        }
    }

    /// Drops the answers older than [`KEEP_SECS`] (as of `now`) and the oldest beyond [`MAX_ROWS`].
    fn prune(&self, now: f64) {
        let c = self.lock();
        let _ = c.execute("DELETE FROM answers WHERE fetched_at < ?", params![now - KEEP_SECS]);
        let _ = c.execute("DELETE FROM answers WHERE key IN (SELECT key FROM answers ORDER BY fetched_at DESC LIMIT -1 OFFSET ?)", params![MAX_ROWS]);
    }

    /// How many answers the file holds (`/api/admin/perf`).
    pub fn len(&self) -> i64 {
        self.lock().query_row("SELECT COUNT(*) FROM answers", [], |r| r.get(0)).unwrap_or(0)
    }

    /// Whether the file holds no answers.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A file in a new temp folder.
    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("encore-mb-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("musicbrainz-cache.db")
    }

    #[test]
    fn keeps_answers_across_opens_and_drops_old_ones() {
        let p = temp("keep");
        let s = Store::open(&p).unwrap();
        let now = encore_db::now();
        s.put("artist/a?fmt=json", &json!({ "name": "A" }), now);
        s.put("artist/old?fmt=json", &json!({ "name": "Old" }), now - KEEP_SECS - 10.0);
        s.put("artist/a?fmt=json", &json!({ "name": "A2" }), now);
        drop(s);
        // a new start: the answer is there, the one past KEEP_SECS isn't
        let s = Store::open(&p).unwrap();
        let (v, at) = s.get("artist/a?fmt=json").unwrap();
        assert_eq!((v["name"].as_str(), at), (Some("A2"), now));
        assert!(s.get("artist/old?fmt=json").is_none());
        assert_eq!(s.len(), 1);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_folder_it_cannot_write_is_no_store() {
        assert!(Store::open(Path::new("/proc/encore-no-such-dir/musicbrainz-cache.db")).is_none());
    }
}
