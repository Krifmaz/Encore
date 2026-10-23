//! The server's last answers, kept on this computer so the app opens and stays usable while the
//! server can't be reached (the phone app's data/api/ApiCache.kt).
//!
//! Every successful `GET` [`crate::api::Api`] makes is written to
//! `<data dir>/encore-desktop/api-cache/<account>/<url>.json` as the JSON the server sent, where
//! `<account>` is a hash of the server and the session cookie, so another account on the same
//! computer never sees these answers. When the server can't be reached the same call is answered
//! from this copy: the session (`/api/auth/me`, `/api/auth/info`) so the app signs in, and every
//! page opened before (Home, Library, Playlists, albums, Settings). Writes are never kept or
//! replayed. Trimmed to [`MAX_BYTES`], oldest-used first.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

/// At most this much is kept over all accounts (the oldest-used answers go first).
const MAX_BYTES: u64 = 40 * 1024 * 1024;

/// The trim runs after this many writes (listing the folder on every write would cost more).
const TRIM_EVERY: u32 = 40;

/// Paths (after `/api`) never kept: sign-in, sign-out and the health check must ask the server,
/// and live state (other devices, group listens, the event stream, playback rendering) is wrong
/// the moment it is old.
const SKIP_PREFIXES: &[&str] = &[
    "/health",
    "/auth/login",
    "/auth/logout",
    "/remote/",
    "/group",
    "/events",
    "/playback/",
];

/// Writes since the last trim.
static WRITES: AtomicU32 = AtomicU32::new(0);

/// `<data dir>/encore-desktop/api-cache`.
fn root() -> PathBuf {
    crate::config::data_dir().join("api-cache")
}

/// SHA-256 of `s` in hex: a file name that is safe whatever the URL holds.
fn hash(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Whose answers these are: the server and the session, hashed (empty without a session, and
/// then nothing is kept or read).
pub fn owner(base: &str, cookie: Option<&str>) -> String {
    match cookie {
        Some(c) if !c.is_empty() && !base.is_empty() => hash(&format!("{base}|{c}")),
        _ => String::new(),
    }
}

/// Whether the answer to `GET url` may be kept: an `/api/` URL on `base` that is not one of
/// [`SKIP_PREFIXES`], a song stream or a cover.
pub fn cacheable(url: &str, base: &str) -> bool {
    if base.is_empty() {
        return false;
    }
    let Some(rest) = url.strip_prefix(base) else {
        return false;
    };
    let path = rest.split('?').next().unwrap_or("");
    let Some(api_path) = path.strip_prefix("/api") else {
        return false;
    };
    if !api_path.starts_with('/') || api_path.ends_with("/stream") || api_path.ends_with("/cover") {
        return false;
    }
    !SKIP_PREFIXES.iter().any(|p| api_path.starts_with(p))
}

/// `<root>/<owner>/<hash of url>.json`.
fn file_of(root: &Path, owner: &str, url: &str) -> PathBuf {
    root.join(owner).join(format!("{}.json", hash(url)))
}

/// The answer kept for `owner` and `url`, or None when there is none.
pub fn get(owner: &str, url: &str) -> Option<String> {
    if owner.is_empty() {
        return None;
    }
    let f = file_of(&root(), owner, url);
    let text = std::fs::read_to_string(&f).ok()?;
    // "used now": the trim keeps what is read, not only what was written lately
    if let Ok(file) = std::fs::File::options().append(true).open(&f) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(text)
}

/// Keeps `text` as the answer for `owner` and `url`; written to a side file first, so a crash
/// never leaves half a file.
pub fn put(owner: &str, url: &str, text: &str) {
    if owner.is_empty() {
        return;
    }
    let root = root();
    let f = file_of(&root, owner, url);
    let Some(dir) = f.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = f.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_err() || std::fs::rename(&tmp, &f).is_err() {
        let _ = std::fs::remove_file(&tmp);
        log::debug!("api cache: not written: {}", f.display());
        return;
    }
    if WRITES.fetch_add(1, Ordering::Relaxed) + 1 >= TRIM_EVERY {
        WRITES.store(0, Ordering::Relaxed);
        trim(&root, MAX_BYTES);
    }
}

/// Removes everything kept for `owner` (sign out).
pub fn forget(owner: &str) {
    if !owner.is_empty() {
        let _ = std::fs::remove_dir_all(root().join(owner));
    }
}

/// Deletes the oldest-used answers under `root` until all of them fit in `max` bytes.
fn trim(root: &Path, max: u64) {
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let Ok(owners) = std::fs::read_dir(root) else { return };
    for o in owners.flatten() {
        let Ok(entries) = std::fs::read_dir(o.path()) else { continue };
        for e in entries.flatten() {
            if let Ok(m) = e.metadata() {
                if m.is_file() {
                    files.push((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), e.path()));
                }
            }
        }
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= max {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= max {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_api_gets_of_the_server() {
        let base = "http://music.local:8080";
        assert!(cacheable("http://music.local:8080/api/library/recent?limit=12", base));
        assert!(cacheable("http://music.local:8080/api/auth/me", base));
        assert!(!cacheable("http://other.host/api/library/recent", base));
        assert!(!cacheable("http://music.local:8080/api/health", base));
        assert!(!cacheable("http://music.local:8080/api/remote/devices", base));
        assert!(!cacheable("http://music.local:8080/api/library/tracks/4/stream", base));
        assert!(!cacheable("http://music.local:8080/api/library/albums/a/cover", base));
        assert!(!cacheable("http://music.local:8080/s/token", base));
        assert!(!cacheable("http://music.local:8080/api/library", ""));
    }

    #[test]
    fn no_owner_without_a_session() {
        assert!(owner("http://a", None).is_empty());
        assert!(owner("http://a", Some("")).is_empty());
        assert_ne!(owner("http://a", Some("x")), owner("http://a", Some("y")));
    }

    #[test]
    fn trim_drops_the_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let acct = dir.path().join("acct");
        std::fs::create_dir_all(&acct).unwrap();
        let old = acct.join("old.json");
        let new = acct.join("new.json");
        std::fs::write(&old, vec![b'x'; 600]).unwrap();
        std::fs::write(&new, vec![b'x'; 600]).unwrap();
        let past = SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().append(true).open(&old).unwrap().set_modified(past).unwrap();
        trim(dir.path(), 1000);
        assert!(!old.exists());
        assert!(new.exists());
    }
}
