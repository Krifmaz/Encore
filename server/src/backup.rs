//! Database backups: copies of encore.db under <data_dir>/backups, taken before every schema upgrade,
//! once a day by the scheduler, before a restore, and on request from Settings → Diagnostics.
//!
//! File names are `encore-YYYYMMDD-HHMMSS.db` (UTC), with a suffix saying why the copy was made
//! (`-before-upgrade`, `-before-restore`, `-uploaded`). Retention: everything from the last 7 days,
//! then the newest of each week for 8 weeks, plus the last 3 pre-upgrade copies; the newest backup is
//! never deleted.
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Why a backup was taken – it ends up as the file name's suffix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Daily (scheduler), at start when the newest copy is older than a day, or "Back up now".
    Scheduled,
    /// Taken before the schema is migrated to a newer version.
    BeforeUpgrade,
    /// Safety copy of the current database before a restore replaces it.
    BeforeRestore,
    /// A file an admin uploaded through the UI.
    Uploaded,
}

impl Kind {
    fn suffix(self) -> &'static str {
        match self {
            Kind::Scheduled => "",
            Kind::BeforeUpgrade => "-before-upgrade",
            Kind::BeforeRestore => "-before-restore",
            Kind::Uploaded => "-uploaded",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Scheduled => "scheduled",
            Kind::BeforeUpgrade => "before upgrade",
            Kind::BeforeRestore => "before restore",
            Kind::Uploaded => "uploaded",
        }
    }
    fn from_suffix(s: &str) -> Option<Kind> {
        match s {
            "" => Some(Kind::Scheduled),
            "-before-upgrade" => Some(Kind::BeforeUpgrade),
            "-before-restore" => Some(Kind::BeforeRestore),
            "-uploaded" => Some(Kind::Uploaded),
            _ => None,
        }
    }
}

pub const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// One backup file.
#[derive(Clone, Debug)]
pub struct Info {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    /// Unix time the copy was taken (from the name; the file's mtime when the name has none).
    pub time: f64,
    pub kind: Kind,
}

impl Info {
    pub fn json(&self) -> Value {
        json!({ "name": self.name, "size": self.size, "time": self.time, "kind": self.kind.label() })
    }
}

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

// ---- dates without a date crate ----------------------------------------------------------------

/// (year, month, day) of a day count since 1970-01-01 (Howard Hinnant's civil_from_days).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since 1970-01-01 of a civil date (days_from_civil).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `YYYYMMDD-HHMMSS` in UTC.
pub fn stamp(unix: f64) -> String {
    let secs = unix.max(0.0) as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let t = secs.rem_euclid(86400);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

/// Unix time of a `YYYYMMDD-HHMMSS` stamp.
fn unstamp(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() != 15 || b[8] != b'-' || !b.iter().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit()) {
        return None;
    }
    let n = |a: usize, l: usize| s[a..a + l].parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0, 4)?, n(4, 2)?, n(6, 2)?, n(9, 2)?, n(11, 2)?, n(13, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some((days_from_civil(y, mo as u32, d as u32) * 86400 + h * 3600 + mi * 60 + se) as f64)
}

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Picks apart `encore-YYYYMMDD-HHMMSS[-suffix].db`; None for anything else (so nothing but our own
/// files is ever listed, served or deleted).
pub fn parse_name(name: &str) -> Option<(f64, Kind)> {
    let rest = name.strip_prefix("encore-")?.strip_suffix(".db")?;
    if rest.len() < 15 {
        return None;
    }
    let (stamp, suffix) = rest.split_at(15);
    Some((unstamp(stamp)?, Kind::from_suffix(suffix)?))
}

/// A safe file name for a backup: exactly the shape we write, nothing path-like.
pub fn valid_name(name: &str) -> bool {
    parse_name(name).is_some()
}

pub fn file_name(kind: Kind, unix: f64) -> String {
    format!("encore-{}{}.db", stamp(unix), kind.suffix())
}

/// All backups, newest first.
pub fn list(data_dir: &Path) -> Vec<Info> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir(data_dir)) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some((time, kind)) = parse_name(&name) else { continue };
        let Ok(md) = e.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        out.push(Info { name, path: e.path(), size: md.len(), time, kind });
    }
    out.sort_by(|a, b| b.time.partial_cmp(&a.time).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.name.cmp(&a.name)));
    out
}

/// The newest backup, whatever its kind.
pub fn newest(data_dir: &Path) -> Option<Info> {
    list(data_dir).into_iter().next()
}

/// Copies the open database to a new backup file with SQLite's `VACUUM INTO` (a consistent snapshot,
/// WAL included) and returns it. Logged at info level.
pub fn create(conn: &Connection, data_dir: &Path, kind: Kind) -> anyhow::Result<Info> {
    let d = dir(data_dir);
    std::fs::create_dir_all(&d)?;
    let mut t = unix_now();
    let mut name = file_name(kind, t);
    // two backups within the same second: wait for the next one rather than overwrite
    while d.join(&name).exists() {
        t += 1.0;
        name = file_name(kind, t);
    }
    let path = d.join(&name);
    let tmp = d.join(format!("{name}.part"));
    let _ = std::fs::remove_file(&tmp);
    let res = conn.execute("VACUUM INTO ?1", [tmp.to_string_lossy().as_ref()]);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("backup failed: {e}");
    }
    if let Err(e) = validate_file(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("backup is not readable: {e}");
    }
    std::fs::rename(&tmp, &path)?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    tracing::info!("database backed up to {} ({} bytes, {})", path.display(), size, kind.label());
    Ok(Info { name, path, size, time: t, kind })
}

/// Checks that a file is a SQLite database that passes `PRAGMA integrity_check`; returns its user_version.
pub fn validate_file(path: &Path) -> anyhow::Result<i64> {
    let mut head = [0u8; 16];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?;
        let n = f.read(&mut head)?;
        if n < 16 || &head != SQLITE_HEADER {
            anyhow::bail!("not a SQLite database");
        }
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if ok != "ok" {
        anyhow::bail!("integrity check failed: {ok}");
    }
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(v)
}

/// Applies the retention rules and returns the names that were deleted.
pub fn prune(data_dir: &Path) -> Vec<String> {
    let all = list(data_dir);
    let mut removed = Vec::new();
    for name in to_prune(&all, unix_now()) {
        let p = dir(data_dir).join(&name);
        match std::fs::remove_file(&p) {
            Ok(()) => {
                tracing::info!("old backup removed: {name}");
                removed.push(name);
            }
            Err(e) => tracing::warn!("could not remove old backup {name}: {e}"),
        }
    }
    removed
}

/// The retention rules, as a pure function of the listing (newest first) and the current time.
pub fn to_prune(all: &[Info], now: f64) -> Vec<String> {
    const DAY: f64 = 86400.0;
    let mut keep: std::collections::HashSet<&str> = std::collections::HashSet::new();
    // the newest copy always stays – never delete the only backup
    if let Some(n) = all.first() {
        keep.insert(&n.name);
    }
    // the last 3 pre-upgrade copies
    for b in all.iter().filter(|b| b.kind == Kind::BeforeUpgrade).take(3) {
        keep.insert(&b.name);
    }
    // everything from the last 7 days, then one per week (the newest of each) for 8 more weeks
    let mut weeks_seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for b in all {
        let age = now - b.time;
        if age < 7.0 * DAY {
            keep.insert(&b.name);
        } else if age < (7.0 + 8.0 * 7.0) * DAY {
            let week = ((b.time / DAY).floor() as i64).div_euclid(7);
            if weeks_seen.insert(week) {
                keep.insert(&b.name);
            }
        }
    }
    all.iter().filter(|b| !keep.contains(b.name.as_str())).map(|b| b.name.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_round_trip() {
        assert_eq!(stamp(0.0), "19700101-000000");
        assert_eq!(stamp(1_700_000_000.0), "20231114-221320");
        assert_eq!(unstamp("20231114-221320"), Some(1_700_000_000.0));
        assert_eq!(unstamp("2023111-221320"), None);
        assert_eq!(unstamp("20231399-221320"), None);
        for t in [0.0, 951_782_400.0, 1_700_000_000.0, 4_102_444_800.0] {
            assert_eq!(unstamp(&stamp(t)), Some(t));
        }
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(parse_name("encore-20240102-030405.db"), Some((unstamp("20240102-030405").unwrap(), Kind::Scheduled)));
        assert_eq!(parse_name("encore-20240102-030405-before-upgrade.db").map(|x| x.1), Some(Kind::BeforeUpgrade));
        assert!(!valid_name("../encore.db"));
        assert!(!valid_name("encore-20240102-030405-x.db"));
        assert!(!valid_name("encore-20240102-030405.db.part"));
        assert!(!valid_name("encore-2024010-030405.db"));
        assert!(!valid_name("/encore-20240102-030405.db"));
        assert!(valid_name(&file_name(Kind::Uploaded, 1_700_000_000.0)));
    }

    fn info(kind: Kind, time: f64) -> Info {
        let name = file_name(kind, time);
        Info { path: PathBuf::from(&name), name, size: 1, time, kind }
    }

    #[test]
    fn retention_rules() {
        const DAY: f64 = 86400.0;
        let now = 1_800_000_000.0;
        let mut all = Vec::new();
        // one per day for 100 days
        for d in 0..100 {
            all.push(info(Kind::Scheduled, now - d as f64 * DAY - 3600.0));
        }
        // pre-upgrade copies: 5, all older than the window
        for d in [70, 80, 90, 95, 99] {
            all.push(info(Kind::BeforeUpgrade, now - d as f64 * DAY - 7200.0));
        }
        all.sort_by(|a, b| b.time.partial_cmp(&a.time).unwrap());
        let gone: std::collections::HashSet<String> = to_prune(&all, now).into_iter().collect();
        let kept: Vec<&Info> = all.iter().filter(|b| !gone.contains(&b.name)).collect();
        // all of the last 7 days
        assert_eq!(kept.iter().filter(|b| now - b.time < 7.0 * DAY).count(), 7);
        // weekly after that: at most one per week bucket, none older than 63 days (except pre-upgrade)
        let weekly: Vec<&&Info> = kept.iter().filter(|b| now - b.time >= 7.0 * DAY && b.kind == Kind::Scheduled).collect();
        assert!(weekly.len() >= 8 && weekly.len() <= 9, "{}", weekly.len());
        let mut weeks: Vec<i64> = weekly.iter().map(|b| ((b.time / DAY).floor() as i64).div_euclid(7)).collect();
        weeks.dedup();
        assert_eq!(weeks.len(), weekly.len());
        assert!(weekly.iter().all(|b| now - b.time < 63.0 * DAY));
        // exactly the 3 newest pre-upgrade copies
        let up: Vec<&&Info> = kept.iter().filter(|b| b.kind == Kind::BeforeUpgrade).collect();
        assert_eq!(up.len(), 3);
        assert!(up.iter().all(|b| now - b.time < 91.0 * DAY));
        // a lone, ancient backup is never deleted
        let lone = vec![info(Kind::Scheduled, now - 400.0 * DAY)];
        assert!(to_prune(&lone, now).is_empty());
        let two = vec![info(Kind::Scheduled, now - 400.0 * DAY), info(Kind::Scheduled, now - 500.0 * DAY)];
        assert_eq!(to_prune(&two, now), vec![two[1].name.clone()]);
    }

    #[test]
    fn creates_a_valid_copy_and_prunes() {
        let dir = std::env::temp_dir().join(format!("encore-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("encore.db")).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1), (2), (3); PRAGMA user_version = 7;").unwrap();
        let b = create(&conn, &dir, Kind::BeforeUpgrade).unwrap();
        assert!(b.path.exists());
        assert!(b.name.ends_with("-before-upgrade.db"));
        assert_eq!(validate_file(&b.path).unwrap(), 7);
        let c2 = Connection::open(&b.path).unwrap();
        let n: i64 = c2.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 3);
        let l = list(&dir);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].kind, Kind::BeforeUpgrade);
        assert_eq!(newest(&dir).unwrap().name, b.name);
        // an old file is pruned, the newest stays
        let old = dir.join("backups").join(file_name(Kind::Scheduled, 1_000_000_000.0));
        std::fs::copy(&b.path, &old).unwrap();
        assert_eq!(list(&dir).len(), 2);
        let removed = prune(&dir);
        assert_eq!(removed.len(), 1);
        assert!(!old.exists() && b.path.exists());
        // garbage is not a database
        std::fs::write(dir.join("x.db"), b"hello").unwrap();
        assert!(validate_file(&dir.join("x.db")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
