//! A plugin's log: `<config>/plugins/<id>/logs/plugin.log`, rotated at 1 MB with three old files kept
//! (`plugin.log.1` newest … `plugin.log.3` oldest). Each line starts with the time (UTC) and the command it came
//! from. What a plugin writes on stderr goes here (its `log` lines and anything else); a setting's value never
//! does: Encore writes only its own words and the plugin's stderr.
use encore_core::fsx::{self, Step};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The size at which the log is rotated.
pub const ROTATE_BYTES: u64 = 1 << 20;
/// Old files kept.
pub const KEEP: usize = 3;
/// The longest line kept (longer ones are cut, so one line cannot fill the log).
const MAX_LINE: usize = 4000;

/// One plugin's log. Clones share the lock, so lines from commands running together don't interleave halfway.
#[derive(Clone, Debug)]
pub struct Log {
    /// the folder `plugin.log` and its rotations are in
    dir: PathBuf,
    /// one writer at a time
    lock: Arc<Mutex<()>>,
}

impl Log {
    /// The log in `dir`.
    pub fn new(dir: &Path) -> Log {
        Log { dir: dir.to_path_buf(), lock: Arc::default() }
    }

    /// The current log file.
    pub fn path(&self) -> PathBuf {
        self.dir.join("plugin.log")
    }

    /// Add one line (`what`: the command, "install", "update" …). A failing disk only loses the line.
    pub fn line(&self, what: &str, text: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = self.write(what, text) {
            tracing::debug!("plugin log {}: {e}", self.dir.display());
        }
    }

    /// Append one entry (`what`, then `text`), rotating the file first when it is full.
    fn write(&self, what: &str, text: &str) -> Result<(), fsx::StepError> {
        fsx::mkdir_all(&self.dir, Step::Plugin)?;
        let p = self.path();
        if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) >= ROTATE_BYTES {
            self.rotate()?;
        }
        let mut t: String = text.chars().filter(|c| *c == '\t' || !c.is_control()).collect();
        if t.len() > MAX_LINE {
            let mut cut = MAX_LINE;
            while !t.is_char_boundary(cut) {
                cut -= 1;
            }
            t.truncate(cut);
            t.push('…');
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(fsx::at(Step::Plugin, "open", &p))?;
        writeln!(f, "{} [{what}] {t}", stamp(encore_core::time::now())).map_err(fsx::at(Step::Plugin, "write", &p))
    }

    /// Shift `plugin.log` → `.1` → `.2` …, dropping the oldest.
    fn rotate(&self) -> Result<(), fsx::StepError> {
        let name = |n: usize| if n == 0 { self.path() } else { self.dir.join(format!("plugin.log.{n}")) };
        let _ = std::fs::remove_file(name(KEEP));
        for n in (0..KEEP).rev() {
            let from = name(n);
            if from.exists() {
                fsx::rename(&from, name(n + 1), Step::Plugin)?;
            }
        }
        Ok(())
    }

    /// The last `n` lines, oldest first, across the rotated files.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<String> = vec![];
        for k in 0..=KEEP {
            let p = if k == 0 { self.path() } else { self.dir.join(format!("plugin.log.{k}")) };
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let mut lines: Vec<String> = text.lines().map(String::from).collect();
            lines.append(&mut out);
            out = lines;
            if out.len() >= n {
                break;
            }
        }
        let skip = out.len().saturating_sub(n);
        out.split_off(skip)
    }

    /// Remove every log file (Remove).
    pub fn clear(&self) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `2026-10-20 21:04:05` (UTC).
pub fn stamp(t: f64) -> String {
    let s = t as i64;
    let (y, m, d) = encore_core::time::civil_from_days(s.div_euclid(86400));
    let rem = s.rem_euclid(86400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_rotates_and_keeps_three_old_files() {
        let d = encore_core::tmp::dir("plugin-log");
        let log = Log::new(&d.join("logs"));
        log.line("search", "first line");
        log.line("search", "with\u{1b}[31m a control character");
        let tail = log.tail(10);
        assert_eq!(tail.len(), 2);
        assert!(tail[0].ends_with("[search] first line") && tail[0].starts_with("20"), "{tail:?}");
        assert!(!tail[1].contains('\u{1b}'), "control characters are dropped");
        let long = "x".repeat(MAX_LINE * 2);
        for _ in 0..(ROTATE_BYTES as usize / MAX_LINE) * 5 {
            log.line("download", &long);
        }
        assert!(d.join("logs/plugin.log.1").is_file() && d.join("logs/plugin.log.3").is_file());
        assert!(!d.join("logs/plugin.log.4").exists(), "three old files kept");
        for k in ["plugin.log", "plugin.log.1", "plugin.log.2", "plugin.log.3"] {
            assert!(std::fs::metadata(d.join("logs").join(k)).unwrap().len() <= ROTATE_BYTES + MAX_LINE as u64 + 100, "{k}");
        }
        let tail = log.tail(5000);
        assert!(tail.len() > 2 * (ROTATE_BYTES as usize / (MAX_LINE + 40)), "the tail reads across the rotated files: {}", tail.len());
        assert!(tail.iter().all(|l| l.chars().count() < MAX_LINE + 40), "long lines are cut");
        log.clear();
        assert!(log.tail(10).is_empty());
        assert_eq!(stamp(0.0), "1970-01-01 00:00:00");
        assert_eq!(stamp(1_792_530_245.0), "2026-10-20 21:04:05");
        let _ = std::fs::remove_dir_all(&d);
    }
}
