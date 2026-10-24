//! The library's folders: `library_path`, where Encore files the songs it imports, and the extra library
//! folders (`library_extra_roots`, one folder per line, filled by the file manager's import that keeps the
//! files where they are). The scan walks all of them; only songs in `library_path` are ever moved, renamed or
//! re-tagged by Encore on its own (organize, renaming after Edit details, automatic re-tagging).
use encore_config::Config;
use std::path::{Path, PathBuf};

/// The extra library folders written in the setting's text: one absolute folder per line (blank lines and
/// relative paths are left out, each folder once, without a trailing `/`).
pub fn parse_extra(text: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let p = PathBuf::from(line.trim_end_matches('/'));
        if p.is_absolute() && p.parent().is_some() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// The extra library folders (`library_extra_roots`).
pub fn extra_folders(cfg: &Config) -> Vec<PathBuf> {
    parse_extra(&cfg.str("library_extra_roots"))
}

/// Every folder a scan walks: `library_path` first, then the extra library folders.
pub fn scan_roots(cfg: &Config) -> Vec<String> {
    let lib = cfg.str("library_path");
    std::iter::once(lib.trim().to_string())
        .chain(extra_folders(cfg).iter().map(|p| p.to_string_lossy().into_owned()))
        .filter(|r| !r.is_empty())
        .collect()
}

/// Whether a song's path (as its row names it) lies in `library_path`: only those songs are moved, renamed or
/// re-tagged by Encore on its own. A song in an extra library folder stays where and as it is.
pub fn in_library_path(cfg: &Config, path: &Path) -> bool {
    let lib = cfg.str("library_path");
    let lib = lib.trim();
    !lib.is_empty() && path.starts_with(lib)
}

/// Is `dir` an extra library folder, a folder inside one, or a folder that holds one? Encore's own clean-ups
/// (a request's staging folder, rejected downloads) leave such a folder alone: its songs are the library's.
/// Both the paths as written and as resolved are compared.
pub fn touches_extra(cfg: &Config, dir: &Path) -> bool {
    let extra = extra_folders(cfg);
    if extra.is_empty() {
        return false;
    }
    let both = |p: &Path| [p.to_path_buf(), std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())];
    let dirs = both(dir);
    extra.iter().any(|e| both(e).iter().any(|x| dirs.iter().any(|d| x.starts_with(d) || d.starts_with(x))))
}

/// The Trash's folder in `library_path`: its name starts with a dot, so the scan never reads its songs in.
pub const TRASH_NAME: &str = ".encore-trash";
/// Days the Trash keeps what it holds when the setting says nothing usable.
pub const TRASH_DAYS: i64 = 30;

/// Does a delete keep the files in the Trash (`trash_keep`, off by default)? Off: deletes are for good.
pub fn trash_keep(cfg: &Config) -> bool {
    cfg.bool("trash_keep")
}

/// Days the Trash keeps what it holds (`trash_days`, 1 to 3650; 30 when unset).
pub fn trash_days(cfg: &Config) -> i64 {
    match cfg.int("trash_days") {
        d if d < 1 => TRASH_DAYS,
        d => d.min(3650),
    }
}

/// `<config>/trash`: the Trash before 0.38 put it on the music disk, and its place while there is no library folder.
pub fn old_trash(data_dir: &Path) -> PathBuf {
    data_dir.join("trash")
}

/// Encore's Trash: [`TRASH_NAME`] in `library_path`, so a delete there is a rename on the music disk and needs no
/// room of its own (made when it isn't there, with .plexignore and .ndignore so media servers leave it alone);
/// `<config>/trash` while the library folder isn't set or can't hold it.
pub fn trash_dir(cfg: &Config, data_dir: &Path) -> PathBuf {
    let lib = cfg.str("library_path");
    let lib = Path::new(lib.trim());
    if !lib.as_os_str().is_empty() && lib.is_dir() {
        let t = lib.join(TRASH_NAME);
        if t.is_dir() || std::fs::create_dir(&t).is_ok() {
            for f in [".plexignore", ".ndignore"] {
                if !t.join(f).exists() {
                    let _ = std::fs::write(t.join(f), "*\n");
                }
            }
            return t;
        }
    }
    old_trash(data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trash_is_on_the_music_disk() {
        let dir = encore_core::tmp::dir("folders-trash");
        let cfg = Config::load_with(encore_db::Db::open(&dir).unwrap(), |_| None).unwrap();
        assert!(!trash_keep(&cfg), "deletes are for good until the setting is on");
        assert_eq!(trash_days(&cfg), 30);
        cfg.set_many(&serde_json::json!({ "library_path": dir.join("nowhere").to_string_lossy(), "trash_days": 0 }));
        assert_eq!(trash_dir(&cfg, &dir), dir.join("trash"), "no library folder: the config folder's");
        assert_eq!(trash_days(&cfg), 30);
        std::fs::create_dir_all(dir.join("music")).unwrap();
        cfg.set_many(&serde_json::json!({ "library_path": dir.join("music").to_string_lossy(), "trash_days": 7, "trash_keep": true }));
        assert_eq!(trash_dir(&cfg, &dir), dir.join("music/.encore-trash"));
        assert!(dir.join("music/.encore-trash/.plexignore").is_file());
        assert!(trash_keep(&cfg) && trash_days(&cfg) == 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_folders_are_read_one_per_line() {
        assert_eq!(parse_extra("/a\n\n  /b/ \nrelative\n/a\n/"), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert!(parse_extra("").is_empty());
        let dir = encore_core::tmp::dir("folders-extra");
        let cfg = Config::load_with(encore_db::Db::open(&dir).unwrap(), |_| None).unwrap();
        assert!(!touches_extra(&cfg, Path::new("/staging/request-1")));
        cfg.set_many(&serde_json::json!({ "library_extra_roots": "/staging/request-1/Kept\n/dl/x" }));
        for (d, hit) in [("/staging/request-1", true), ("/staging/request-1/Kept/A", true), ("/staging", true), ("/staging/request-2", false), ("/dl/xy", false)] {
            assert_eq!(touches_extra(&cfg, Path::new(d)), hit, "{d}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
