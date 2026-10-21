//! Unpacking a zip or a .tar.gz into an empty folder, with the names checked: no absolute names, no `..`, no
//! write through a link, links only to places inside the archive, no device files, and a cap on what comes out
//! (bytes and entries), so a broken or hostile archive cannot write outside the folder or fill the disk.
//!
//! The rules, for every entry:
//! - its name is relative (`/etc/x` is refused, a leading `./` is dropped), has no `..` part and no NUL; a zip's
//!   backslashes count as folder separators;
//! - no folder on its way is a link or a file (so a link in the archive can never be written through);
//! - a symbolic link is relative, starts with at most as many `..` as it is deep, and has only plain names after
//!   them (`../lib/libx.so.1` from `bin/`, never `x/../../y`), so it points inside the folder however the links
//!   before it resolve;
//! - a hard link (tar) names a file the archive already wrote, and is copied;
//! - files keep only their execute bit (0755 or 0644: no set-user-id, nothing writable by others).
use encore_core::fsx::{self, Step};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

/// How a download is unpacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    TarGz,
}

impl Kind {
    /// "zip" / "tar.gz" (the manifest's `unpack` values; "file" is no archive).
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "zip" => Some(Kind::Zip),
            "tar.gz" | "tgz" => Some(Kind::TarGz),
            _ => None,
        }
    }

    /// From a file name or URL: `.zip`, `.tar.gz`, `.tgz`.
    pub fn of_name(name: &str) -> Option<Kind> {
        let name = name.split(['?', '#']).next().unwrap_or(name).to_ascii_lowercase();
        if name.ends_with(".zip") {
            Some(Kind::Zip)
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some(Kind::TarGz)
        } else {
            None
        }
    }

    /// From the first bytes of the file: `PK\x03\x04` (zip) or `\x1f\x8b` (gzip).
    pub fn sniff(path: &Path) -> Option<Kind> {
        let mut head = [0u8; 4];
        std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
        if head == *b"PK\x03\x04" || head == *b"PK\x05\x06" {
            Some(Kind::Zip)
        } else if head[..2] == [0x1f, 0x8b] {
            Some(Kind::TarGz)
        } else {
            None
        }
    }
}

/// The most an archive may put on the disk.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// bytes written in all
    pub bytes: u64,
    /// entries (files, folders, links)
    pub entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { bytes: 4 << 30, entries: 100_000 }
    }
}

/// What came out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Unpacked {
    pub files: usize,
    pub bytes: u64,
}

/// An entry's name as a path inside the folder (None: the archive's root itself, `./`).
pub fn entry_path(name: &str) -> anyhow::Result<Option<PathBuf>> {
    if name.contains('\0') {
        anyhow::bail!("the archive has a name with a NUL character in it");
    }
    if name.starts_with('/') {
        anyhow::bail!("the archive has an absolute name: {name}");
    }
    let mut out = PathBuf::new();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(x) => out.push(x),
            Component::CurDir => {}
            Component::ParentDir => anyhow::bail!("the archive has a name that leaves its folder: {name}"),
            Component::RootDir | Component::Prefix(_) => anyhow::bail!("the archive has an absolute name: {name}"),
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// A symbolic link's target, checked for a link at `rel` (a path inside the folder): relative, `..` only at its
/// start and at most as many as the link is deep, plain names after them.
pub fn link_ok(rel: &Path, target: &str) -> bool {
    if target.is_empty() || target.starts_with('/') || target.contains('\0') {
        return false;
    }
    let depth = rel.parent().map(|p| p.components().count()).unwrap_or(0);
    let mut ups = 0usize;
    let mut names = 0usize;
    for c in Path::new(target).components() {
        match c {
            Component::ParentDir if names == 0 => ups += 1,
            Component::ParentDir => return false,
            Component::CurDir => {}
            Component::Normal(_) => names += 1,
            _ => return false,
        }
    }
    ups <= depth
}

/// Unpack `archive` into the folder `dest` (made when missing; it should be empty).
pub fn unpack(archive: &Path, kind: Kind, dest: &Path, limits: Limits) -> anyhow::Result<Unpacked> {
    fsx::mkdir_all(dest, Step::Plugin)?;
    let mut w = Writer { dest: dest.to_path_buf(), limits, out: Unpacked::default(), entries: 0 };
    match kind {
        Kind::Zip => unzip(archive, &mut w)?,
        Kind::TarGz => untar(archive, &mut w)?,
    }
    Ok(w.out)
}

struct Writer {
    dest: PathBuf,
    limits: Limits,
    out: Unpacked,
    entries: usize,
}

impl Writer {
    fn count(&mut self) -> anyhow::Result<()> {
        self.entries += 1;
        if self.entries > self.limits.entries {
            anyhow::bail!("the archive has more than {} entries", self.limits.entries);
        }
        Ok(())
    }

    /// The folders on the way to `rel`, made when missing; none of them may be a link or a file.
    fn parents(&self, rel: &Path) -> anyhow::Result<()> {
        let mut at = self.dest.clone();
        if let Some(parent) = rel.parent() {
            for c in parent.components() {
                at.push(c);
                match std::fs::symlink_metadata(&at) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) => anyhow::bail!("the archive writes through a link or over a file: {}", rel.display()),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => fsx::mkdir(&at, Step::Plugin)?,
                    Err(e) => return Err(fsx::StepError::new(Step::Plugin, "stat", &at, None, e).into()),
                }
            }
        }
        Ok(())
    }

    /// Clear the place of a new file or link: whatever non-folder is there goes (never followed).
    fn clear(&self, path: &Path, rel: &Path) -> anyhow::Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => anyhow::bail!("the archive puts a file where it has a folder: {}", rel.display()),
            Ok(_) => fsx::remove_file(path, Step::Plugin)?,
            Err(_) => {}
        }
        Ok(())
    }

    fn dir(&mut self, rel: &Path) -> anyhow::Result<()> {
        self.count()?;
        self.parents(rel)?;
        let p = self.dest.join(rel);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => anyhow::bail!("the archive puts a folder where it has a file or link: {}", rel.display()),
            Err(_) => Ok(fsx::mkdir(&p, Step::Plugin)?),
        }
    }

    fn file(&mut self, rel: &Path, mode: u32, data: &mut dyn Read) -> anyhow::Result<()> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        self.count()?;
        self.parents(rel)?;
        let p = self.dest.join(rel);
        self.clear(&p, rel)?;
        let mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p).map_err(fsx::at(Step::Plugin, "open", &p))?;
        let room = self.limits.bytes.saturating_sub(self.out.bytes);
        let n = io::copy(&mut data.take(room.saturating_add(1)), &mut f).map_err(|e| {
            if e.kind() == io::ErrorKind::InvalidData || e.kind() == io::ErrorKind::UnexpectedEof {
                anyhow::anyhow!("the archive is damaged ({e})")
            } else {
                fsx::StepError::new(Step::Plugin, "write", &p, None, e).into()
            }
        })?;
        if n > room {
            drop(f);
            let _ = std::fs::remove_file(&p);
            anyhow::bail!("the archive unpacks to more than {} MB", self.limits.bytes >> 20);
        }
        f.flush().map_err(fsx::at(Step::Plugin, "write", &p))?;
        drop(f);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).map_err(fsx::at(Step::Plugin, "chmod", &p))?;
        self.out.files += 1;
        self.out.bytes += n;
        Ok(())
    }

    fn symlink(&mut self, rel: &Path, target: &str) -> anyhow::Result<()> {
        self.count()?;
        if !link_ok(rel, target) {
            anyhow::bail!("a link in the archive points outside it: {} -> {target}", rel.display());
        }
        self.parents(rel)?;
        let p = self.dest.join(rel);
        self.clear(&p, rel)?;
        std::os::unix::fs::symlink(target, &p).map_err(fsx::at(Step::Plugin, "link", &p))?;
        Ok(())
    }

    /// A tar hard link: a copy of a regular file the archive wrote before.
    fn hard_link(&mut self, rel: &Path, target: &Path) -> anyhow::Result<()> {
        self.count()?;
        let src = self.dest.join(target);
        // the source's own folders were checked when it was written; it has to be a plain file now
        self.parents(target).and_then(|_| match std::fs::symlink_metadata(&src) {
            Ok(m) if m.is_file() => Ok(()),
            _ => anyhow::bail!("a link in the archive names a file it does not have: {} -> {}", rel.display(), target.display()),
        })?;
        self.parents(rel)?;
        let p = self.dest.join(rel);
        self.clear(&p, rel)?;
        let n = fsx::copy(&src, &p, Step::Plugin)?;
        self.out.bytes += n;
        if self.out.bytes > self.limits.bytes {
            anyhow::bail!("the archive unpacks to more than {} MB", self.limits.bytes >> 20);
        }
        self.out.files += 1;
        Ok(())
    }
}

// ---- zip ------------------------------------------------------------------------------------------

fn unzip(archive: &Path, w: &mut Writer) -> anyhow::Result<()> {
    let file = fsx::open(archive, Step::Plugin)?;
    let mut a = zip::ZipArchive::new(io::BufReader::new(file)).map_err(|e| anyhow::anyhow!("not a zip file ({e})"))?;
    if a.len() > w.limits.entries {
        anyhow::bail!("the archive has more than {} entries", w.limits.entries);
    }
    for i in 0..a.len() {
        let mut f = a.by_index(i).map_err(|e| anyhow::anyhow!("the zip file is damaged ({e})"))?;
        if f.encrypted() {
            anyhow::bail!("the zip file is encrypted");
        }
        let name = f.name().map_err(|e| anyhow::anyhow!("the zip file is damaged ({e})"))?.replace('\\', "/");
        let Some(rel) = entry_path(&name)? else { continue };
        let mode = f.unix_mode().unwrap_or(0o644);
        if f.is_dir() || name.ends_with('/') {
            w.dir(&rel)?;
        } else if f.is_symlink() {
            let mut target = String::new();
            f.by_ref().take(4096).read_to_string(&mut target).map_err(|e| anyhow::anyhow!("the zip file is damaged ({e})"))?;
            w.symlink(&rel, &target)?;
        } else {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 {
                anyhow::bail!("the archive holds a special file: {name}");
            }
            w.file(&rel, mode, &mut f)?;
        }
    }
    Ok(())
}

// ---- tar.gz -----------------------------------------------------------------------------------------

/// A header field as text (up to its first NUL).
fn text(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn octal(b: &[u8]) -> anyhow::Result<u64> {
    if b.first().is_some_and(|c| c & 0x80 != 0) {
        anyhow::bail!("the archive has an entry too large to unpack");
    }
    let t = text(b);
    let t = t.trim_matches(|c: char| c == ' ' || c == '\0');
    if t.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(t, 8).map_err(|_| anyhow::anyhow!("the archive is damaged (a header field reads {t:?})"))
}

/// The header's checksum holds (the sum of its bytes with the checksum field as spaces).
fn checksum_ok(h: &[u8; 512]) -> bool {
    let Ok(want) = octal(&h[148..156]) else { return false };
    let sum: u64 = h.iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { *b as u64 }).sum();
    sum == want
}

/// PAX records: `<len> <key>=<value>\n`.
fn pax(data: &[u8]) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut at = 0usize;
    while at < data.len() {
        let Some(sp) = data[at..].iter().position(|c| *c == b' ') else { break };
        let Ok(len) = std::str::from_utf8(&data[at..at + sp]).unwrap_or("").parse::<usize>() else { break };
        if len == 0 || at + len > data.len() {
            break;
        }
        let rec = &data[at + sp + 1..at + len];
        let rec = rec.strip_suffix(b"\n").unwrap_or(rec);
        if let Some(eq) = rec.iter().position(|c| *c == b'=') {
            out.push((String::from_utf8_lossy(&rec[..eq]).into_owned(), String::from_utf8_lossy(&rec[eq + 1..]).into_owned()));
        }
        at += len;
    }
    out
}

fn skip(r: &mut impl Read, n: u64) -> anyhow::Result<()> {
    let got = io::copy(&mut r.take(n), &mut io::sink()).map_err(|e| anyhow::anyhow!("the archive is damaged ({e})"))?;
    if got != n {
        anyhow::bail!("the archive is cut off");
    }
    Ok(())
}

fn read_small(r: &mut impl Read, n: u64) -> anyhow::Result<Vec<u8>> {
    if n > 1 << 20 {
        anyhow::bail!("the archive has an oversized header");
    }
    let mut v = vec![0u8; n as usize];
    r.read_exact(&mut v).map_err(|_| anyhow::anyhow!("the archive is cut off"))?;
    Ok(v)
}

fn untar(archive: &Path, w: &mut Writer) -> anyhow::Result<()> {
    let file = fsx::open(archive, Step::Plugin)?;
    let mut r = flate2::read::MultiGzDecoder::new(io::BufReader::new(file));
    let mut long_name: Option<String> = None;
    let mut long_link: Option<String> = None;
    let mut pax_path: Option<String> = None;
    let mut pax_link: Option<String> = None;
    let mut pax_size: Option<u64> = None;
    let mut first = true;
    loop {
        let mut h = [0u8; 512];
        match r.read_exact(&mut h) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof && !first => break,
            Err(e) => anyhow::bail!("not a .tar.gz file ({e})"),
        }
        if h.iter().all(|b| *b == 0) {
            break;
        }
        if !checksum_ok(&h) {
            anyhow::bail!("{}", if first { "not a .tar.gz file" } else { "the archive is damaged (a header does not add up)" });
        }
        first = false;
        let kind = h[156];
        let mut size = octal(&h[124..136])?;
        if let Some(s) = pax_size.take() {
            size = s;
        }
        let pad = size.div_ceil(512) * 512 - size;
        let mut name = text(&h[0..100]);
        if &h[257..262] == b"ustar" && h[262] == 0 {
            let prefix = text(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        let mut link = text(&h[157..257]);
        match kind {
            b'L' | b'K' | b'x' => {
                let data = read_small(&mut r, size)?;
                skip(&mut r, pad)?;
                match kind {
                    b'L' => long_name = Some(text(&data)),
                    b'K' => long_link = Some(text(&data)),
                    _ => {
                        for (k, v) in pax(&data) {
                            match k.as_str() {
                                "path" => pax_path = Some(v),
                                "linkpath" => pax_link = Some(v),
                                "size" => pax_size = v.parse().ok(),
                                _ => {}
                            }
                        }
                    }
                }
                continue;
            }
            b'g' => {
                skip(&mut r, size + pad)?;
                continue;
            }
            _ => {}
        }
        if let Some(n) = long_name.take().or(pax_path.take()) {
            name = n;
        }
        if let Some(l) = long_link.take().or(pax_link.take()) {
            link = l;
        }
        let rel = entry_path(&name)?;
        match (kind, rel) {
            (b'5', Some(rel)) => {
                w.dir(&rel)?;
                skip(&mut r, size + pad)?;
            }
            (b'0' | 0 | b'7', Some(rel)) => {
                let mode = octal(&h[100..108]).unwrap_or(0o644) as u32;
                let mut body = (&mut r).take(size);
                w.file(&rel, mode, &mut body)?;
                let left = body.limit();
                if left > 0 {
                    anyhow::bail!("the archive is cut off");
                }
                skip(&mut r, pad)?;
            }
            (b'2', Some(rel)) => {
                w.symlink(&rel, &link)?;
                skip(&mut r, size + pad)?;
            }
            (b'1', Some(rel)) => {
                let target = entry_path(&link)?.ok_or_else(|| anyhow::anyhow!("a link in the archive names nothing: {name}"))?;
                w.hard_link(&rel, &target)?;
                skip(&mut r, size + pad)?;
            }
            (b'3' | b'4' | b'6', _) => anyhow::bail!("the archive holds a special file: {name}"),
            (b'S', _) => anyhow::bail!("the archive holds a sparse file, which Encore does not unpack: {name}"),
            (b'5' | b'0' | 0 | b'7' | b'2' | b'1', None) => skip(&mut r, size + pad)?,
            _ => skip(&mut r, size + pad)?,
        }
    }
    Ok(())
}

/// A program in an unpacked folder: `bin` as a path inside it (`build/bin/llama-server`), or, when it is a bare
/// file name, the first regular file of that name in the folder's first four levels (shallowest first).
pub fn find_bin(dir: &Path, bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let rel = entry_path(bin).ok().flatten()?;
        let p = dir.join(rel);
        return std::fs::symlink_metadata(&p).ok().filter(|m| m.is_file()).map(|_| p);
    }
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..4 {
        let mut next = vec![];
        let mut found = vec![];
        for d in &level {
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten() {
                let Ok(t) = e.file_type() else { continue };
                if t.is_file() && e.file_name() == bin {
                    found.push(e.path());
                } else if t.is_dir() {
                    next.push(e.path());
                }
            }
        }
        if !found.is_empty() {
            found.sort();
            return found.into_iter().next();
        }
        next.sort();
        level = next;
    }
    None
}
