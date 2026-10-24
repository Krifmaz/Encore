//! Database backups: copies of encore.db under <data_dir>/backups (or `ENCORE_BACKUP_DIR`), taken before
//! every schema upgrade, once a day by the scheduler, before a restore, and on request from Settings →
//! Diagnostics.
//!
//! Room: a copy is only started on a disk with room for it (the database's size plus a margin). When the
//! backups folder's disk is too full, the copy goes to `.encore-backups` in the library folder
//! (`ENCORE_LIBRARY_PATH`, the music disk) instead; when neither has room, the oldest copies in the
//! backups folder are removed (the newest [`KEEP_WHEN_FULL`] copies and uploads always stay) to make it.
//! Before a schema upgrade, a backup that can't be made anywhere is a warning, not a failed start
//! ([`before_upgrade`]): `ENCORE_REQUIRE_MIGRATION_BACKUP=1` makes it fail instead, and
//! `ENCORE_SKIP_MIGRATION_BACKUP=1` skips the copy altogether.
//!
//! File names are `encore-YYYYMMDD-HHMMSS.db` (UTC), with a suffix saying why the copy was made
//! (`-before-upgrade`, `-before-restore`, `-uploaded`). Retention ([`Retention`]): Settings → General →
//! Database backups → Backups to keep (`backup_keep`, 3) is shared out daily, weekly, monthly – 3 is the
//! newest copy of the latest day, one of an earlier week and one of an earlier month – plus the newest
//! `backup_keep_upgrade` (1) pre-upgrade copies. The rest is deleted at every start and after every
//! backup. Uploaded files are someone's own and are never deleted.
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use encore_core::text::size_text;
use encore_core::time::{civil_from_days, days_from_civil, now};

/// The env var naming another folder for the backups (instead of `<data_dir>/backups`).
pub const ENV_BACKUP_DIR: &str = "ENCORE_BACKUP_DIR";
/// The env var that skips the backup before a schema upgrade (`1`, `true`, `yes` or `on`).
pub const ENV_SKIP_MIGRATION_BACKUP: &str = "ENCORE_SKIP_MIGRATION_BACKUP";
/// The env var that makes a pre-upgrade backup that can't be made stop the start (the old behaviour).
pub const ENV_REQUIRE_MIGRATION_BACKUP: &str = "ENCORE_REQUIRE_MIGRATION_BACKUP";
/// The folder in the library folder that takes the backups when the backups folder's disk is full (a dot
/// name, so the scan never reads it; .plexignore and .ndignore keep media servers out).
pub const SPILL_NAME: &str = ".encore-backups";
/// The setting with how many daily/weekly/monthly copies are kept in all (`ENCORE_BACKUP_KEEP`).
pub const KEEP_KEY: &str = "backup_keep";
/// The setting with how many pre-upgrade copies are kept besides (`ENCORE_BACKUP_KEEP_UPGRADE`).
pub const KEEP_UPGRADE_KEY: &str = "backup_keep_upgrade";

/// How many copies of each kind [`prune`] keeps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Retention {
    /// the newest copy of this many days
    pub daily: usize,
    /// then the newest copy of this many earlier weeks
    pub weekly: usize,
    /// then the newest copy of this many earlier months
    pub monthly: usize,
    /// the newest pre-upgrade copies kept besides
    pub upgrade: usize,
}

impl Retention {
    /// `keep` copies in all (at least 1) shared out daily first, then weekly, then monthly – 3 is one of
    /// each, 4 is two daily, 6 two of each – and `upgrade` pre-upgrade copies (0 or more).
    pub fn new(keep: i64, upgrade: i64) -> Retention {
        let keep = keep.clamp(1, 1000) as usize;
        let daily = keep.div_ceil(3);
        let weekly = (keep - daily).div_ceil(2);
        Retention { daily, weekly, monthly: keep - daily - weekly, upgrade: upgrade.clamp(0, 100) as usize }
    }

    /// The retention in effect for the database in `data_dir`, as the settings say (the environment
    /// variable, then the saved value, then the default 3 and 1). The value is read straight from the
    /// settings table, so this works before the server's settings are loaded (a schema upgrade).
    pub fn read(data_dir: &Path) -> Retention {
        let conn = Connection::open_with_flags(data_dir.join("encore.db"), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok();
        let get = |key: &str, env: &str, default: i64| -> i64 {
            if let Some(n) = std::env::var(env).ok().and_then(|v| v.trim().parse().ok()) {
                return n;
            }
            // saved values are JSON: 3 or "3"
            let saved: Option<String> = conn.as_ref().and_then(|c| c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).ok());
            saved
                .and_then(|v| serde_json::from_str::<Value>(&v).ok())
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
                .unwrap_or(default)
        };
        Retention::new(get(KEEP_KEY, "ENCORE_BACKUP_KEEP", 3), get(KEEP_UPGRADE_KEY, "ENCORE_BACKUP_KEEP_UPGRADE", 1))
    }
}

impl Default for Retention {
    /// 3 in all (one daily, one weekly, one monthly) and the newest pre-upgrade copy.
    fn default() -> Retention {
        Retention::new(3, 1)
    }
}
/// Copies of this server's own database that freeing room never removes (the newest ones).
pub const KEEP_WHEN_FULL: usize = 2;
/// Room left over on top of the database's size before a copy is started: SQLite's own bookkeeping and
/// whatever else writes to the disk meanwhile.
const MARGIN: u64 = 16 << 20;

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
    /// What a backup of this kind adds to its file name (nothing for a scheduled one).
    fn suffix(self) -> &'static str {
        match self {
            Kind::Scheduled => "",
            Kind::BeforeUpgrade => "-before-upgrade",
            Kind::BeforeRestore => "-before-restore",
            Kind::Uploaded => "-uploaded",
        }
    }
    /// The kind as the API and the UI name it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Scheduled => "scheduled",
            Kind::BeforeUpgrade => "before upgrade",
            Kind::BeforeRestore => "before restore",
            Kind::Uploaded => "uploaded",
        }
    }
    /// The kind a file name's suffix stands for (the inverse of `suffix`).
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

/// The first 16 bytes of every SQLite database file (checked before a file is restored).
pub const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// One backup file.
#[derive(Clone, Debug)]
pub struct Info {
    /// file name (`encore-YYYYMMDD-HHMMSS[-suffix].db`)
    pub name: String,
    /// where the file is
    pub path: PathBuf,
    /// size in bytes
    pub size: u64,
    /// Unix time the copy was taken (from the name; the file's mtime when the name has none).
    pub time: f64,
    /// why it was taken
    pub kind: Kind,
}

impl Info {
    /// The backup as the API lists it.
    pub fn json(&self) -> Value {
        json!({ "name": self.name, "size": self.size, "time": self.time, "kind": self.kind.label() })
    }
}

/// The folder the backups are kept in: `ENCORE_BACKUP_DIR` when set, `<data_dir>/backups` otherwise.
pub fn dir(data_dir: &Path) -> PathBuf {
    match std::env::var(ENV_BACKUP_DIR) {
        Ok(d) if !d.trim().is_empty() => PathBuf::from(d.trim()),
        _ => data_dir.join("backups"),
    }
}

/// `.encore-backups` in the library folder (`ENCORE_LIBRARY_PATH`), where copies go when the backups
/// folder's disk is full; None without a library folder, or when it is the backups folder itself.
pub fn spill_dir(data_dir: &Path) -> Option<PathBuf> {
    let lib = std::env::var("ENCORE_LIBRARY_PATH").ok()?;
    let lib = Path::new(lib.trim());
    if lib.as_os_str().is_empty() || !lib.is_dir() {
        return None;
    }
    let d = lib.join(SPILL_NAME);
    (d != dir(data_dir)).then_some(d)
}

/// Every folder backups may be in: the backups folder, then the spill folder (when there is one).
pub fn dirs(data_dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![dir(data_dir)];
    out.extend(spill_dir(data_dir));
    out
}

/// The backup called `name` (a [`valid_name`]) in any of the [`dirs`]; None when there's no such file.
pub fn find(data_dir: &Path, name: &str) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    dirs(data_dir).into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Is the env var `name` set to something that means yes (`1`, `true`, `yes`, `on`)?
fn env_flag(name: &str) -> bool {
    std::env::var(name).map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")).unwrap_or(false)
}

/// Bytes a copy of the database in `data_dir` needs: the file and its WAL (a `VACUUM INTO` copy is never
/// bigger) plus [`MARGIN`].
pub fn needed_bytes(data_dir: &Path) -> u64 {
    let db: u64 = ["encore.db", "encore.db-wal"].iter().filter_map(|f| std::fs::metadata(data_dir.join(f)).ok()).map(|m| m.len()).sum();
    db + MARGIN
}

/// Bytes free on the disk of `folder`, asked of its nearest existing parent (the folder itself may not be
/// made yet); None when that can't be told.
fn free_at(folder: &Path) -> Option<u64> {
    folder.ancestors().find(|p| p.exists()).and_then(encore_core::fsinfo::free_bytes)
}

// ---- dates without a date crate ----------------------------------------------------------------

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
    Some((days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se) as f64)
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

/// The file name of a `kind` backup taken at `unix` time.
pub fn file_name(kind: Kind, unix: f64) -> String {
    format!("encore-{}{}.db", stamp(unix), kind.suffix())
}

/// All backups in every one of the [`dirs`], newest first (a name in both folders is listed once, from
/// the backups folder).
pub fn list(data_dir: &Path) -> Vec<Info> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for d in dirs(data_dir) {
        out.extend(list_in(&d).into_iter().filter(|b| seen.insert(b.name.clone())));
    }
    out.sort_by(|a, b| b.time.partial_cmp(&a.time).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.name.cmp(&a.name)));
    out
}

/// The backups in the one folder `d`, newest first.
fn list_in(d: &Path) -> Vec<Info> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(d) else { return out };
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

/// The newest copy of this server's own database (uploads don't count: they are someone else's data).
pub fn newest_copy(data_dir: &Path) -> Option<Info> {
    list(data_dir).into_iter().find(|b| b.kind != Kind::Uploaded)
}

/// Copies the open database to a new backup file and returns it. The copy goes to the backups folder when
/// its disk has room ([`needed_bytes`]), else to the spill folder on the music disk ([`spill_dir`]), else
/// to the backups folder after the oldest copies there were removed to make room ([`to_free`]). When none
/// of that works the error names each folder, what it has free and what the copy needs.
pub fn create(conn: &Connection, data_dir: &Path, kind: Kind) -> anyhow::Result<Info> {
    let need = needed_bytes(data_dir);
    let main = dir(data_dir);
    // what each folder said, for the error when nowhere works
    let mut tried: Vec<String> = Vec::new();
    // a disk that can't say how much room it has is tried anyway (the copy itself then tells)
    let has_room = |d: &Path| free_at(d).is_none_or(|f| f >= need);
    let attempt = |d: &Path, tried: &mut Vec<String>| -> Option<Info> {
        match create_in(conn, d, kind) {
            Ok(b) => Some(b),
            Err(e) => {
                tracing::warn!("database backup to {} failed: {e}", d.display());
                tried.push(format!("{}: {e}", d.display()));
                None
            }
        }
    };
    let note_full = |d: &Path, tried: &mut Vec<String>| {
        tried.push(format!("{} has {} free", d.display(), free_at(d).map(size_text).unwrap_or_else(|| "no".into())));
    };
    // 1. the backups folder, when its disk has room
    if has_room(&main) {
        if let Some(b) = attempt(&main, &mut tried) {
            return Ok(b);
        }
    } else {
        note_full(&main, &mut tried);
    }
    // 2. the music disk, when it has room
    if let Some(spill) = spill_dir(data_dir) {
        if has_room(&spill) {
            tracing::warn!("{} hasn't room for a database backup ({} needed): writing it to {} instead", main.display(), size_text(need), spill.display());
            if let Some(b) = attempt(&spill, &mut tried) {
                mark_spill(&spill);
                return Ok(b);
            }
        } else {
            note_full(&spill, &mut tried);
        }
    }
    // 3. the oldest copies in the backups folder make room, when removing them can
    if let Some(free) = free_at(&main) {
        let short = need.saturating_sub(free);
        let gone = to_free(&list_in(&main), short);
        if short > 0 && !gone.is_empty() {
            for name in &gone {
                match std::fs::remove_file(main.join(name)) {
                    Ok(()) => tracing::warn!("old backup removed to make room for a new one: {name}"),
                    Err(e) => tracing::warn!("could not remove old backup {name}: {e}"),
                }
            }
            if has_room(&main) {
                if let Some(b) = attempt(&main, &mut tried) {
                    return Ok(b);
                }
            } else {
                note_full(&main, &mut tried);
            }
        }
    }
    anyhow::bail!(
        "backup failed: no room for a {} copy of the database ({}). Free space on that disk (old copies are in {}), or set {ENV_BACKUP_DIR} to a folder with room",
        size_text(need),
        tried.join("; "),
        main.display()
    )
}

/// The backup before a schema upgrade, which never stops the start on its own: skipped with
/// `ENCORE_SKIP_MIGRATION_BACKUP=1`; when [`create`] fails (a full disk), a loud warning and Ok(None), so
/// the upgrade goes ahead without a copy – unless `ENCORE_REQUIRE_MIGRATION_BACKUP=1`, then the error.
/// Old copies are pruned first, so the one about to be made has the room they took.
pub fn before_upgrade(conn: &Connection, data_dir: &Path) -> anyhow::Result<Option<Info>> {
    if env_flag(ENV_SKIP_MIGRATION_BACKUP) {
        tracing::warn!("{ENV_SKIP_MIGRATION_BACKUP} is set: upgrading the database without a backup first");
        return Ok(None);
    }
    prune(data_dir);
    match create(conn, data_dir, Kind::BeforeUpgrade) {
        Ok(b) => Ok(Some(b)),
        Err(e) if env_flag(ENV_REQUIRE_MIGRATION_BACKUP) => Err(e.context(format!("{ENV_REQUIRE_MIGRATION_BACKUP} is set, so the upgrade waits for a backup (or set {ENV_SKIP_MIGRATION_BACKUP}=1)"))),
        Err(e) => {
            tracing::warn!("{e}");
            tracing::warn!("upgrading the database WITHOUT a backup first; free space on that disk so the daily backup can run again (set {ENV_REQUIRE_MIGRATION_BACKUP}=1 to stop the start instead)");
            Ok(None)
        }
    }
}

/// The spill folder gets a .plexignore and an .ndignore, so Plex and Navidrome leave the copies alone.
fn mark_spill(d: &Path) {
    for f in [".plexignore", ".ndignore"] {
        if !d.join(f).exists() {
            let _ = std::fs::write(d.join(f), "*\n");
        }
    }
}

/// Which copies in one folder (its listing, newest first) to remove to free `short` bytes: the oldest of
/// this server's own copies first, never the newest [`KEEP_WHEN_FULL`] of them nor an uploaded file.
/// Nothing when that can't free enough (then nothing is removed for nothing).
pub fn to_free(in_dir: &[Info], short: u64) -> Vec<String> {
    if short == 0 {
        return Vec::new();
    }
    let own: Vec<&Info> = in_dir.iter().filter(|b| b.kind != Kind::Uploaded).collect();
    let (mut out, mut freed) = (Vec::new(), 0u64);
    // oldest first, past the newest KEEP_WHEN_FULL
    for b in own.iter().skip(KEEP_WHEN_FULL).rev() {
        if freed >= short {
            break;
        }
        freed += b.size;
        out.push(b.name.clone());
    }
    if freed >= short { out } else { Vec::new() }
}

/// Copies the open database into the folder `d` with SQLite's `VACUUM INTO` (a consistent snapshot, WAL
/// included) and returns it. Logged at info level.
fn create_in(conn: &Connection, d: &Path, kind: Kind) -> anyhow::Result<Info> {
    std::fs::create_dir_all(d)?;
    let mut t = now();
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
    // VACUUM INTO doesn't fsync its output: make sure the copy (and its directory entry) survive a power loss
    std::fs::File::open(&tmp)?.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    if let Ok(dir) = std::fs::File::open(d) {
        let _ = dir.sync_all();
    }
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

/// Applies the retention the settings ask for ([`Retention::read`]) and returns the names that were deleted.
pub fn prune(data_dir: &Path) -> Vec<String> {
    prune_with(data_dir, Retention::read(data_dir))
}

/// Applies the retention `r` and returns the names that were deleted.
pub fn prune_with(data_dir: &Path, r: Retention) -> Vec<String> {
    let all = list(data_dir);
    let mut removed = Vec::new();
    for name in to_prune(&all, r) {
        // the file where the listing found it (the backups folder or the spill folder)
        let Some(p) = all.iter().find(|b| b.name == name).map(|b| b.path.clone()) else { continue };
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

/// The retention rule, as a pure function of the listing (newest first): what isn't kept goes.
///
/// Kept: the newest `r.upgrade` pre-upgrade copies; of the other copies of this server's own database
/// (newest first), the newest of each day until `r.daily` days have one, then the newest of each earlier
/// week until `r.weekly` weeks have one, then of each earlier month until `r.monthly` months have one. A
/// week or month that already has a kept copy doesn't take another, so the tiers reach further back
/// instead of overlapping. Uploaded files always stay.
pub fn to_prune(all: &[Info], r: Retention) -> Vec<String> {
    use std::collections::HashSet;
    const DAY: f64 = 86400.0;
    let mut keep: HashSet<&str> = HashSet::new();
    for b in all.iter().filter(|b| b.kind == Kind::BeforeUpgrade).take(r.upgrade) {
        keep.insert(&b.name);
    }
    // the day, week and month (year * 12 + month) a copy was made in, in UTC
    let buckets = |b: &Info| {
        let day = (b.time / DAY).floor() as i64;
        let (y, m, _) = civil_from_days(day);
        (day, day.div_euclid(7), y * 12 + m as i64)
    };
    // the days, weeks and months that have a kept copy, and how many each tier took
    let (mut days, mut weeks, mut months) = (HashSet::new(), HashSet::new(), HashSet::new());
    let (mut n_weekly, mut n_monthly) = (0, 0);
    for b in all.iter().filter(|b| !matches!(b.kind, Kind::BeforeUpgrade | Kind::Uploaded)) {
        let (d, w, m) = buckets(b);
        let kept = if days.len() < r.daily && !days.contains(&d) {
            days.insert(d)
        } else if days.len() < r.daily {
            // a later copy of a day that has its copy
            false
        } else if n_weekly < r.weekly {
            // a week that already has a kept copy (a daily one) doesn't take another
            let new = !weeks.contains(&w);
            n_weekly += new as usize;
            new
        } else if n_monthly < r.monthly {
            // likewise a month that has a daily or weekly copy
            let new = !months.contains(&m);
            n_monthly += new as usize;
            new
        } else {
            false
        };
        if kept {
            keep.insert(&b.name);
            weeks.insert(w);
            months.insert(m);
        }
    }
    all.iter().filter(|b| b.kind != Kind::Uploaded && !keep.contains(b.name.as_str())).map(|b| b.name.clone()).collect()
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
    fn the_count_is_shared_out_daily_weekly_monthly() {
        let t = |k, u| { let r = Retention::new(k, u); (r.daily, r.weekly, r.monthly, r.upgrade) };
        assert_eq!(t(3, 1), (1, 1, 1, 1));
        assert_eq!(Retention::default(), Retention::new(3, 1));
        assert_eq!(t(1, 0), (1, 0, 0, 0));
        assert_eq!(t(2, 1), (1, 1, 0, 1));
        assert_eq!(t(4, 1), (2, 1, 1, 1));
        assert_eq!(t(6, 3), (2, 2, 2, 3));
        // nonsense is clamped: always one copy, never a negative count
        assert_eq!(t(0, -5), (1, 0, 0, 0));
    }

    #[test]
    fn keeps_one_daily_one_weekly_one_monthly_and_the_newest_pre_upgrade() {
        const DAY: f64 = 86400.0;
        let now = 1_800_000_000.0;
        // a daily copy for 200 days, five pre-upgrade copies in the last two days, an old upload
        let mut all: Vec<Info> = (0..200).map(|d| info(Kind::Scheduled, now - d as f64 * DAY - 3600.0)).collect();
        all.extend((0..5).map(|i| info(Kind::BeforeUpgrade, now - i as f64 * 0.4 * DAY - 60.0)));
        all.push(info(Kind::Uploaded, now - 50.0 * DAY));
        all.sort_by(|a, b| b.time.partial_cmp(&a.time).unwrap());
        let week = |b: &Info| ((b.time / DAY).floor() as i64).div_euclid(7);
        let month = |b: &Info| { let (y, m, _) = civil_from_days((b.time / DAY).floor() as i64); y * 12 + m as i64 };
        let kept_with = |r| { let gone: std::collections::HashSet<String> = to_prune(&all, r).into_iter().collect(); all.iter().filter(|b| !gone.contains(&b.name)).cloned().collect::<Vec<Info>>() };
        // the default: 3 copies (the newest, one of an earlier week, one of an earlier month) + 1 pre-upgrade
        let kept = kept_with(Retention::default());
        let sched: Vec<&Info> = kept.iter().filter(|b| b.kind == Kind::Scheduled).collect();
        assert_eq!(sched.len(), 3);
        assert_eq!(sched[0].name, all.iter().find(|b| b.kind == Kind::Scheduled).unwrap().name);
        assert_ne!(week(sched[1]), week(sched[0]));
        assert!(month(sched[2]) != month(sched[0]) && month(sched[2]) != month(sched[1]));
        assert_eq!(kept.iter().filter(|b| b.kind == Kind::BeforeUpgrade).count(), 1);
        assert_eq!(kept.iter().filter(|b| b.kind == Kind::Uploaded).count(), 1);
        // 9 in all: 3 days, 3 earlier weeks, 3 earlier months; 3 pre-upgrade
        let kept = kept_with(Retention::new(9, 3));
        let sched: Vec<&Info> = kept.iter().filter(|b| b.kind == Kind::Scheduled).collect();
        assert_eq!(sched.len(), 9);
        let ages: Vec<f64> = sched.iter().map(|b| (now - b.time) / DAY).collect();
        assert!(ages[..3].iter().all(|a| *a < 3.0), "{ages:?}");
        let weeks: std::collections::HashSet<i64> = sched[..6].iter().map(|b| week(b)).collect();
        assert!(weeks.len() >= 4, "{ages:?}");
        let months: std::collections::HashSet<i64> = sched[6..].iter().map(|b| month(b)).collect();
        assert_eq!(months.len(), 3, "{ages:?}");
        assert!(sched[6..].iter().all(|b| sched[..6].iter().all(|k| month(k) != month(b))), "{ages:?}");
        assert_eq!(kept.iter().filter(|b| b.kind == Kind::BeforeUpgrade).count(), 3);
        // four copies on the same day: that day's newest stays, the rest go
        let same: Vec<Info> = (0..4).map(|i| info(Kind::Scheduled, now - 3600.0 - i as f64 * 60.0)).collect();
        assert_eq!(to_prune(&same, Retention::default()), same[1..].iter().map(|b| b.name.clone()).collect::<Vec<_>>());
        // a lone, ancient copy is never deleted
        assert!(to_prune(&[info(Kind::Scheduled, now - 400.0 * DAY)], Retention::new(1, 0)).is_empty());
    }

    #[test]
    fn the_saved_setting_decides_the_retention() {
        let dir = encore_core::tmp::dir("backup-keep");
        // no database yet: the defaults
        assert_eq!(Retention::read(&dir), Retention::default());
        let conn = Connection::open(dir.join("encore.db")).unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT); INSERT INTO settings VALUES ('backup_keep', '6'), ('backup_keep_upgrade', '\"2\"');").unwrap();
        assert_eq!(Retention::read(&dir), Retention::new(6, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn freeing_room_takes_the_oldest_and_keeps_the_newest() {
        let sized = |kind, t: f64, size| Info { size, ..info(kind, t) };
        // newest first: two newest stay whatever happens, uploads are never taken
        let all = vec![
            sized(Kind::Scheduled, 5_000.0, 100),
            sized(Kind::BeforeUpgrade, 4_000.0, 100),
            sized(Kind::Uploaded, 3_500.0, 1_000),
            sized(Kind::Scheduled, 3_000.0, 100),
            sized(Kind::Scheduled, 2_000.0, 100),
            sized(Kind::BeforeUpgrade, 1_000.0, 100),
        ];
        assert!(to_free(&all, 0).is_empty());
        // 150 bytes short: the two oldest own copies
        assert_eq!(to_free(&all, 150), vec![all[5].name.clone(), all[4].name.clone()]);
        // all three removable copies free only 300: asking for more removes nothing
        assert_eq!(to_free(&all, 300).len(), 3);
        assert!(to_free(&all, 301).is_empty());
    }

    #[test]
    fn a_backup_that_cant_be_made_doesnt_stop_an_upgrade() {
        let dir = encore_core::tmp::dir("backup-cant");
        let conn = Connection::open(dir.join("encore.db")).unwrap();
        conn.execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1);").unwrap();
        // the backups folder can't be made (a file is in its place): create says so, the upgrade goes on
        std::fs::write(dir.join("backups"), b"not a folder").unwrap();
        let e = create(&conn, &dir, Kind::BeforeUpgrade).unwrap_err().to_string();
        assert!(e.contains("backup failed") && e.contains(&dir.join("backups").display().to_string()), "{e}");
        assert!(before_upgrade(&conn, &dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_room_a_copy_needs_is_the_database_and_a_margin() {
        let dir = encore_core::tmp::dir("backup-need");
        assert_eq!(needed_bytes(&dir), MARGIN);
        std::fs::write(dir.join("encore.db"), vec![0u8; 4096]).unwrap();
        std::fs::write(dir.join("encore.db-wal"), vec![0u8; 1024]).unwrap();
        assert_eq!(needed_bytes(&dir), MARGIN + 5120);
        assert!(free_at(&dir.join("not/made/yet")).is_some());
        let _ = std::fs::remove_dir_all(&dir);
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
        assert_eq!(l[0].name, b.name);
        // an uploaded file is newer but is not a copy of this database
        let up = dir.join("backups").join(file_name(Kind::Uploaded, b.time + 60.0));
        std::fs::copy(&b.path, &up).unwrap();
        assert_eq!(list(&dir)[0].kind, Kind::Uploaded);
        assert_eq!(newest_copy(&dir).unwrap().name, b.name);
        std::fs::remove_file(&up).unwrap();
        // three older files on one day: that day keeps its newest, the other two go, the newest copy stays
        let olds: Vec<PathBuf> = [1_000_000_000.0, 1_000_000_060.0, 1_000_000_120.0].iter().map(|t| dir.join("backups").join(file_name(Kind::Scheduled, *t))).collect();
        for o in &olds {
            std::fs::copy(&b.path, o).unwrap();
        }
        assert_eq!(list(&dir).len(), 4);
        let removed = prune_with(&dir, Retention::default());
        assert_eq!(removed.len(), 2);
        assert!(!olds[0].exists() && !olds[1].exists() && olds[2].exists() && b.path.exists());
        // garbage is not a database
        std::fs::write(dir.join("x.db"), b"hello").unwrap();
        assert!(validate_file(&dir.join("x.db")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
