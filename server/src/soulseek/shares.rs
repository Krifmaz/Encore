//! What we share on Soulseek: the Encore library, as the network expects it – a flat list of folders
//! with backslash paths under one virtual root ("@@encore\Artist\Album\01 - Song.flac"), each file
//! with its size and the bitrate/duration attributes other clients rank by. Built from the library
//! table, so it needs no second scan and follows the library as it changes.
use super::proto::{Dir, FileEntry, ATTR_BITRATE, ATTR_DURATION};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const ROOT: &str = "@@encore";

#[derive(Default, Clone)]
pub struct Shares {
    /// Real library root the virtual paths hang off.
    root: PathBuf,
    dirs: Vec<Dir>,
    /// lower-cased virtual name → (dir index, file index)
    index: HashMap<String, (usize, usize)>,
    /// words of every file's virtual path, lower-cased, for search
    words: Vec<(usize, usize, String)>,
    file_count: usize,
}

/// One library row as the index needs it.
pub struct Item {
    pub path: String,
    pub size: u64,
    pub bitrate: u32,
    pub duration: u32,
}

impl Shares {
    pub fn is_empty(&self) -> bool { self.file_count == 0 }
    pub fn dir_count(&self) -> usize { self.dirs.len() }
    pub fn file_count(&self) -> usize { self.file_count }

    /// Build from library rows. Paths outside `root` are skipped (nothing escapes the library).
    pub fn build(root: &Path, items: impl IntoIterator<Item = Item>) -> Shares {
        let root = root.to_path_buf();
        let mut by_dir: Vec<(String, Vec<FileEntry>)> = vec![];
        let mut pos: HashMap<String, usize> = HashMap::new();
        for it in items {
            let p = Path::new(&it.path);
            let Ok(rel) = p.strip_prefix(&root) else { continue };
            let rel = rel.to_string_lossy().replace('/', "\\");
            let Some((dir, _)) = rel.rsplit_once('\\') else { continue };
            let vdir = format!("{ROOT}\\{dir}");
            let name = format!("{ROOT}\\{rel}");
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let mut attrs = vec![];
            if it.bitrate > 0 {
                attrs.push((ATTR_BITRATE, it.bitrate));
            }
            if it.duration > 0 {
                attrs.push((ATTR_DURATION, it.duration));
            }
            let f = FileEntry { name, size: it.size, ext, attrs };
            let i = *pos.entry(vdir.clone()).or_insert_with(|| { by_dir.push((vdir, vec![])); by_dir.len() - 1 });
            by_dir[i].1.push(f);
        }
        by_dir.sort_by(|a, b| a.0.cmp(&b.0));
        let mut s = Shares { root, ..Default::default() };
        for (name, mut files) in by_dir {
            files.sort_by(|a, b| a.name.cmp(&b.name));
            let di = s.dirs.len();
            for (fi, f) in files.iter().enumerate() {
                let low = f.name.to_lowercase();
                s.index.insert(low.clone(), (di, fi));
                s.words.push((di, fi, low));
                s.file_count += 1;
            }
            s.dirs.push(Dir { name, files });
        }
        s
    }

    /// The whole list, for GetShareFileList.
    pub fn list(&self) -> &[Dir] { &self.dirs }

    /// One folder (and nothing below it – Soulseek folders are flat), for FolderContentsRequest.
    pub fn folder(&self, name: &str) -> Vec<Dir> {
        let want = name.trim_end_matches('\\').to_lowercase();
        self.dirs.iter().filter(|d| d.name.to_lowercase() == want).cloned().collect()
    }

    /// Search the way Soulseek clients do: every word must appear somewhere in the path, words
    /// starting with "-" must not. Case-insensitive; at most `limit` files.
    pub fn search(&self, query: &str, limit: usize) -> Vec<FileEntry> {
        let (mut want, mut exclude) = (vec![], vec![]);
        for w in query.split_whitespace() {
            let w = w.to_lowercase();
            if let Some(x) = w.strip_prefix('-') {
                if !x.is_empty() {
                    exclude.push(x.to_string());
                }
            } else {
                want.push(w);
            }
        }
        if want.is_empty() {
            return vec![];
        }
        let mut out = vec![];
        for (di, fi, low) in &self.words {
            if want.iter().all(|w| low.contains(w.as_str())) && !exclude.iter().any(|x| low.contains(x.as_str())) {
                out.push(self.dirs[*di].files[*fi].clone());
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    /// Real path of a shared file, by its virtual name as a peer sends it back to us.
    pub fn resolve(&self, name: &str) -> Option<(PathBuf, u64)> {
        let (di, fi) = *self.index.get(&name.to_lowercase())?;
        let rel = self.dirs[di].files[fi].name.strip_prefix(ROOT)?.trim_start_matches('\\').replace('\\', "/");
        Some((self.root.join(rel), self.dirs[di].files[fi].size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Shares {
        Shares::build(Path::new("/music"), vec![
            Item { path: "/music/Daft Punk/Discovery/01 - One More Time.flac".into(), size: 30_000_000, bitrate: 900, duration: 320 },
            Item { path: "/music/Daft Punk/Discovery/02 - Aerodynamic.flac".into(), size: 25_000_000, bitrate: 900, duration: 212 },
            Item { path: "/music/Justice/Cross/01 - Genesis.mp3".into(), size: 9_000_000, bitrate: 320, duration: 234 },
            Item { path: "/elsewhere/x.mp3".into(), size: 1, bitrate: 0, duration: 0 },
        ])
    }

    #[test]
    fn builds_virtual_paths() {
        let s = sample();
        assert_eq!((s.dir_count(), s.file_count()), (2, 3));
        assert_eq!(s.list()[0].name, "@@encore\\Daft Punk\\Discovery");
        assert_eq!(s.list()[0].files[0].name, "@@encore\\Daft Punk\\Discovery\\01 - One More Time.flac");
        assert_eq!(s.list()[0].files[0].ext, "flac");
        assert_eq!(s.list()[0].files[0].attrs, vec![(ATTR_BITRATE, 900), (ATTR_DURATION, 320)]);
    }

    #[test]
    fn searches_like_a_client() {
        let s = sample();
        assert_eq!(s.search("daft punk aerodynamic", 50).len(), 1);
        assert_eq!(s.search("DISCOVERY", 50).len(), 2);
        assert_eq!(s.search("discovery -aerodynamic", 50).len(), 1);
        assert_eq!(s.search("discovery", 1).len(), 1);
        assert!(s.search("", 50).is_empty());
        assert!(s.search("nothing here", 50).is_empty());
    }

    #[test]
    fn resolves_and_lists_folders() {
        let s = sample();
        let (p, size) = s.resolve("@@ENCORE\\daft punk\\discovery\\02 - aerodynamic.flac").unwrap();
        assert_eq!(p, PathBuf::from("/music/Daft Punk/Discovery/02 - Aerodynamic.flac"));
        assert_eq!(size, 25_000_000);
        assert!(s.resolve("@@encore\\nope.flac").is_none());
        assert_eq!(s.folder("@@encore\\Justice\\Cross").len(), 1);
        assert_eq!(s.folder("@@encore\\Justice\\Cross\\")[0].files.len(), 1);
        assert!(s.folder("@@encore\\Justice").is_empty());
    }
}
