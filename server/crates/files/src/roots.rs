//! The folders the file manager may touch (`GET /api/files/roots`): the library (`library_path`), the staging
//! folder, Soulseek's download folder, every torrent and Usenet download folder and blackhole folder from the
//! settings that is there on the disk, and Trash (`<config>/trash`). Each has a stable id: `library`,
//! `staging`, `slskd`, `torrent-<n>`, `usenet-<n>`, `trash` (the n-th distinct torrent or Usenet folder, the
//! client in use first).
//!
//! The library's own folders – `library_path` and the extra library folders (`library_extra_roots`, one
//! folder per line, filled by an import that keeps the files where they are) – decide which files have rows
//! in the library ([`LibraryDirs`]).
use encore_config::Config;
use encore_core::error::{err, ApiError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// What a root is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootKind {
    /// a library folder
    Library,
    /// the staging folder new downloads wait in
    Staging,
    /// a download client's folder
    Download,
    /// Encore's Trash
    Trash,
}

/// One folder the file manager may touch.
#[derive(Clone, Debug)]
pub struct Root {
    /// its id in the API ("library", "staging", …)
    pub id: String,
    /// its name as shown
    pub name: String,
    /// the folder as the settings name it
    pub path: PathBuf,
    /// the folder with every link in its own path resolved (None while it isn't there)
    pub canon: Option<PathBuf>,
    /// what it is for
    pub kind: RootKind,
    /// Encore's config folder (database, backups, Trash), resolved, when it lies in this folder: nothing
    /// under it is listed, read or changed through this root
    pub fence: Option<PathBuf>,
}

impl Root {
    /// A root for `path`, resolved now.
    pub fn new(id: &str, name: &str, path: PathBuf, kind: RootKind) -> Root {
        let canon = std::fs::canonicalize(&path).ok().filter(|p| p.is_dir());
        Root { id: id.into(), name: name.into(), path, canon, kind, fence: None }
    }

    /// The root with Encore's config folder fenced off when it lies inside it (not for Trash, which is in it).
    pub fn fenced(mut self, data_dir: &Path) -> Root {
        let data = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        if self.kind != RootKind::Trash && self.canon.as_ref().is_some_and(|c| data.starts_with(c)) {
            self.fence = Some(data);
        }
        self
    }

    /// Is this resolved path Encore's config folder or in it (reached through this root)?
    pub fn fenced_off(&self, real: &Path) -> bool {
        self.fence.as_ref().is_some_and(|f| real.starts_with(f))
    }

    /// The folder, resolved; 404 while it isn't there.
    pub fn dir(&self) -> Result<&Path, ApiError> {
        self.canon.as_deref().ok_or_else(|| err(404, format!("The folder {} isn't there", self.path.display())))
    }

    /// `{id, name, path, writable, free_bytes}` for the API.
    pub fn json(&self) -> Value {
        let (writable, free) = match &self.canon {
            Some(c) => (self.kind != RootKind::Trash && crate::dirfd::writable(c), crate::dirfd::free_bytes(c)),
            None => (false, 0),
        };
        json!({ "id": self.id, "name": self.name, "path": self.path.to_string_lossy(), "writable": writable, "free_bytes": free })
    }
}

/// `<config>/trash`: where Move to trash puts things (each in its own `<unix>-<n>/` folder).
pub fn trash_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("trash")
}

/// The extra library folders: `library_extra_roots`, one absolute folder per line (blank lines and relative
/// paths are left out, each folder once). The library crate reads the setting, as the scan walks these folders.
pub fn extra_roots(cfg: &Config) -> Vec<PathBuf> {
    encore_library::folders::extra_folders(cfg)
}

pub use encore_library::folders::parse_extra;

/// The setting's text for a list of extra folders.
pub fn format_extra(dirs: &[PathBuf]) -> String {
    dirs.iter().map(|d| d.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n")
}

/// Why a folder can't be kept where it is as an extra library folder (None when it can): Encore's working
/// folders (Staging, where requests download and rejected downloads wait, and which Encore clears) and a
/// folder that holds the library folder, Staging or Encore's config folder.
pub fn keep_refusal(cfg: &Config, data_dir: &Path, dir: &Path) -> Option<String> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let staging = cfg.str("staging_path");
    let staging = staging.trim();
    if !staging.is_empty() && (dir.starts_with(canon(Path::new(staging))) || dir.starts_with(staging)) {
        return Some("Files in Staging can't be kept where they are: Encore clears that folder. Import them with “Keep the files where they are” off, so they move into the library".into());
    }
    let lib = cfg.str("library_path");
    for (what, p) in [("the library folder", lib.trim()), ("Staging", staging), ("Encore's config folder", &*data_dir.to_string_lossy())] {
        if !p.is_empty() && (canon(Path::new(p)).starts_with(dir) || Path::new(p).starts_with(dir)) {
            return Some(format!("That folder holds {what}, so it can't be an extra library folder: pick the album's own folder"));
        }
    }
    None
}

/// `library_extra_roots` with `dir` in it: left as it is when `dir` lies in the library or in an extra folder
/// already; an extra folder inside `dir` is replaced by it (no nested or duplicate entries). → None when
/// nothing changes.
pub fn with_extra(library: &Path, extra: &[PathBuf], dir: &Path) -> Option<Vec<PathBuf>> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if dir.starts_with(canon(library)) || dir.starts_with(library) || extra.iter().any(|e| dir.starts_with(e) || dir.starts_with(canon(e))) {
        return None;
    }
    let mut out: Vec<PathBuf> = extra.iter().filter(|e| !e.starts_with(dir) && !canon(e).starts_with(dir)).cloned().collect();
    out.push(dir.to_path_buf());
    Some(out)
}

/// The library's folders: `library_path` and the extra library folders, each as the settings name it and
/// resolved. A file is in the library when its resolved path lies in one of them; its row's path is then
/// the settings' form of that folder joined with the rest (what the scan writes).
#[derive(Clone, Debug, Default)]
pub struct LibraryDirs {
    /// each folder: (as the settings name it, resolved)
    dirs: Vec<(PathBuf, PathBuf)>,
}

impl LibraryDirs {
    /// The library folders the settings name.
    pub fn of(cfg: &Config) -> LibraryDirs {
        let mut all = vec![];
        let lib = cfg.str("library_path");
        if !lib.trim().is_empty() {
            all.push(PathBuf::from(lib.trim()));
        }
        all.extend(extra_roots(cfg));
        LibraryDirs::from(all)
    }

    /// These folders, resolved now.
    pub fn from(dirs: Vec<PathBuf>) -> LibraryDirs {
        let dirs = dirs.into_iter().map(|d| {
            let c = std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone());
            (d, c)
        }).collect();
        LibraryDirs { dirs }
    }

    /// The row path of a resolved path that lies in the library (None outside it).
    pub fn db_path(&self, real: &Path) -> Option<PathBuf> {
        self.dirs.iter()
            .filter_map(|(conf, canon)| real.strip_prefix(canon).ok().map(|rest| (conf, canon, rest)))
            // the deepest folder that holds it (an extra folder inside the library is still the library)
            .max_by_key(|(_, canon, _)| canon.components().count())
            .map(|(conf, _, rest)| if rest.as_os_str().is_empty() { conf.clone() } else { conf.join(rest) })
    }

    /// `real` (a resolved path) lies in the library.
    pub fn contains(&self, real: &Path) -> bool {
        self.db_path(real).is_some()
    }
}

/// A download folder setting: its key and the name it shows as.
struct Folder {
    /// the setting holding the path
    key: &'static str,
    /// the root's name
    name: &'static str,
}

/// The download clients' folders, by the setting that turns the client on.
const TORRENT: [(&str, &[Folder]); 5] = [
    ("qbittorrent", &[Folder { key: "qbit_download_path", name: "Torrents (qBittorrent)" }]),
    ("transmission", &[Folder { key: "transmission_download_path", name: "Torrents (Transmission)" }]),
    ("deluge", &[Folder { key: "deluge_download_path", name: "Torrents (Deluge)" }]),
    ("rtorrent", &[Folder { key: "rtorrent_download_path", name: "Torrents (rTorrent)" }]),
    ("blackhole", &[Folder { key: "torrent_blackhole_completed", name: "Finished torrents" }, Folder { key: "torrent_blackhole_watch", name: "Torrent watch folder" }]),
];

/// The Usenet clients' folders, by the setting that turns the client on.
const USENET: [(&str, &[Folder]); 3] = [
    ("sabnzbd", &[Folder { key: "sab_download_path", name: "Usenet (SABnzbd)" }]),
    ("nzbget", &[Folder { key: "nzbget_download_path", name: "Usenet (NZBGet)" }]),
    ("blackhole", &[Folder { key: "nzb_blackhole_completed", name: "Finished Usenet downloads" }, Folder { key: "nzb_blackhole_watch", name: "Usenet watch folder" }]),
];

/// The download clients' folders of one kind, the client in use first, each folder once and only when it is
/// there on the disk: (name, folder).
fn client_folders(cfg: &Config, table: &[(&str, &[Folder])], chosen_key: &str, taken: &mut Vec<PathBuf>) -> Vec<(&'static str, PathBuf)> {
    let chosen = cfg.str(chosen_key).trim().to_lowercase();
    let mut order: Vec<&[Folder]> = table.iter().filter(|(c, _)| *c == chosen).map(|(_, f)| *f).collect();
    order.extend(table.iter().filter(|(c, _)| *c != chosen).map(|(_, f)| *f));
    let mut out = vec![];
    for f in order.into_iter().flatten() {
        let v = cfg.str(f.key);
        let p = PathBuf::from(v.trim().trim_end_matches('/'));
        if v.trim().is_empty() || !p.is_absolute() || !p.is_dir() {
            continue;
        }
        let c = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        if taken.contains(&c) {
            continue;
        }
        taken.push(c);
        out.push((f.name, p));
    }
    out
}

/// Every root, Trash last. `slskd` is Soulseek's download folder in use (the managed slskd may have chosen
/// another one than the setting).
pub fn all(cfg: &Config, data_dir: &Path, slskd: &Path) -> Vec<Root> {
    let mut out = vec![];
    let mut taken: Vec<PathBuf> = vec![];
    let push = |out: &mut Vec<Root>, taken: &mut Vec<PathBuf>, r: Root| {
        if let Some(c) = &r.canon {
            if taken.contains(c) && r.kind == RootKind::Download {
                return;
            }
            taken.push(c.clone());
        }
        out.push(r);
    };
    let lib = cfg.str("library_path");
    if !lib.trim().is_empty() {
        push(&mut out, &mut taken, Root::new("library", "Library", PathBuf::from(lib.trim()), RootKind::Library));
    }
    let staging = cfg.str("staging_path");
    if !staging.trim().is_empty() {
        push(&mut out, &mut taken, Root::new("staging", "Staging", PathBuf::from(staging.trim()), RootKind::Staging));
    }
    if slskd.is_absolute() && slskd.is_dir() {
        push(&mut out, &mut taken, Root::new("slskd", "Soulseek downloads", slskd.to_path_buf(), RootKind::Download));
    }
    for (prefix, table, key) in [("torrent", &TORRENT[..], "torrent_client"), ("usenet", &USENET[..], "nzb_client")] {
        for (i, (name, p)) in client_folders(cfg, table, key, &mut taken).into_iter().enumerate() {
            out.push(Root::new(&format!("{prefix}-{}", i + 1), name, p, RootKind::Download));
        }
    }
    let trash = trash_dir(data_dir);
    let _ = std::fs::create_dir_all(&trash);
    out.push(Root::new("trash", "Trash", trash, RootKind::Trash));
    out.into_iter().map(|r| r.fenced(data_dir)).collect()
}

/// The library's root (`library_path`) on its own, for the import's own steps (None when it isn't set).
pub fn library(cfg: &Config, data_dir: &Path) -> Option<Root> {
    let lib = cfg.str("library_path");
    (!lib.trim().is_empty()).then(|| Root::new("library", "Library", PathBuf::from(lib.trim()), RootKind::Library).fenced(data_dir))
}

/// The root with this id.
pub fn find<'a>(roots: &'a [Root], id: &str) -> Result<&'a Root, ApiError> {
    roots.iter().find(|r| r.id == id).ok_or_else(|| err(404, "That folder isn't one Encore may use"))
}

/// The root that holds a resolved path (the deepest one), for links between roots.
pub fn holding<'a>(roots: &'a [Root], real: &Path) -> Option<&'a Root> {
    roots.iter().filter(|r| r.canon.as_deref().is_some_and(|c| real.starts_with(c))).max_by_key(|r| r.canon.as_ref().map(|c| c.components().count()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_folders_are_one_per_line_without_nesting() {
        assert_eq!(parse_extra("/a\n\n  /b/ \nrelative\n/a\n/"), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        let lib = Path::new("/nonexistent-music");
        let extra = vec![PathBuf::from("/dl/x/inner"), PathBuf::from("/other")];
        // inside the library or an extra folder: nothing to add
        assert_eq!(with_extra(lib, &extra, Path::new("/nonexistent-music/A")), None);
        assert_eq!(with_extra(lib, &extra, Path::new("/other/deep")), None);
        // a folder that holds an extra folder replaces it
        assert_eq!(with_extra(lib, &extra, Path::new("/dl/x")), Some(vec![PathBuf::from("/other"), PathBuf::from("/dl/x")]));
        assert_eq!(format_extra(&[PathBuf::from("/a"), PathBuf::from("/b")]), "/a\n/b");
    }

    /// A root that holds Encore's config folder is fenced off from it; Trash (inside it) is not.
    #[test]
    fn the_config_folder_is_fenced_off_in_a_root_that_holds_it() {
        let d = encore_core::tmp::dir("files-fence");
        std::fs::create_dir_all(d.join("cfg/trash")).unwrap();
        std::fs::create_dir_all(d.join("music")).unwrap();
        let data = d.join("cfg");
        let wide = Root::new("torrent-1", "Torrents", d.clone(), RootKind::Download).fenced(&data);
        let c = std::fs::canonicalize(&data).unwrap();
        assert!(wide.fenced_off(&c.join("encore.db")) && !wide.fenced_off(&std::fs::canonicalize(d.join("music")).unwrap()));
        let lib = Root::new("library", "Library", d.join("music"), RootKind::Library).fenced(&data);
        assert!(lib.fence.is_none());
        let trash = Root::new("trash", "Trash", d.join("cfg/trash"), RootKind::Trash).fenced(&data);
        assert!(trash.fence.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row_paths_use_the_settings_form_of_the_folder() {
        let d = encore_core::tmp::dir("files-libdirs");
        std::fs::create_dir_all(d.join("real/music/A")).unwrap();
        std::fs::create_dir_all(d.join("dl/kept")).unwrap();
        std::os::unix::fs::symlink(d.join("real/music"), d.join("music")).unwrap();
        let dirs = LibraryDirs::from(vec![d.join("music"), d.join("dl/kept")]);
        let real = std::fs::canonicalize(d.join("real/music/A")).unwrap();
        assert_eq!(dirs.db_path(&real.join("01.flac")), Some(d.join("music/A/01.flac")), "the row path goes through the link the setting names");
        let kept = std::fs::canonicalize(d.join("dl/kept")).unwrap();
        assert_eq!(dirs.db_path(&kept.join("x.flac")), Some(d.join("dl/kept/x.flac")));
        assert_eq!(dirs.db_path(&std::fs::canonicalize(d.join("dl")).unwrap().join("y.flac")), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
