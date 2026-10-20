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

#[cfg(test)]
mod tests {
    use super::*;

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
