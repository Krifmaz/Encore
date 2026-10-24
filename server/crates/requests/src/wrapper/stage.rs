//! Getting wrapper-v2 ready to build: its source at the commit Encore was written for, the owner's own Apple
//! Music for Android bundle with the native libraries taken out of it, and the Docker build context made of
//! both.
//!
//! wrapper-v2 ships no Apple code, and nobody publishes an image of it: whoever runs it supplies the Apple
//! Music app (an `.apk`, or the `.apkm` bundle of split APKs APKMirror hands out) and builds the image. Its
//! two staging scripts (`tools/extract-libs.sh`, `tools/stage-system.sh`, which need bash, jq and unzip) are
//! done here in Rust, with the same rules:
//!
//! - the Apple libraries come from `lib/<abi>/` of the APK (or of the split in the bundle that has them: the
//!   one named for the architecture first), the folder that holds the sentinel `libandroidappmusic.so`;
//! - every library `LIBS_VERSION.json` pins (`.libs.<arch>`) must be there with the pinned SHA-256, so a
//!   different Apple Music version is refused before an hour of building, not after;
//! - the Android system files the libraries need are in the source (`vendor/android-system/<arch>/`) and are
//!   checked against `.android_system.<arch>` the same way, then copied into `rootfs/system/`.
//!
//! The Dockerfile uses BuildKit's cache mounts (`RUN --mount=type=cache`), which the Engine API's builder
//! (the one a plain HTTP client can drive) does not know: [`patch_dockerfile`] takes them out. They only keep
//! apt's downloads between builds.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// The library whose folder in an APK is the one with Apple's libraries.
pub const SENTINEL: &str = "libandroidappmusic.so";

/// The largest single member of a bundle read into memory (a split APK is tens of megabytes).
const MAX_SPLIT_BYTES: u64 = 1 << 30;

/// The two architectures wrapper-v2 builds for, by its names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    /// 64-bit Intel and AMD (`x86_64`, Docker's linux/amd64)
    X86_64,
    /// 64-bit ARM (`arm64-v8a`, Docker's linux/arm64)
    Arm64,
}

impl Arch {
    /// The architecture of a machine as Rust names it (`std::env::consts::ARCH`); None for one wrapper-v2
    /// does not build for.
    pub fn of_machine(machine: &str) -> Option<Arch> {
        match machine {
            "x86_64" | "amd64" => Some(Arch::X86_64),
            "aarch64" | "arm64" => Some(Arch::Arm64),
            _ => None,
        }
    }

    /// This server's.
    pub fn here() -> Option<Arch> {
        Arch::of_machine(std::env::consts::ARCH)
    }

    /// wrapper-v2's name: `TARGET_ARCH` of its Dockerfile, the key in `LIBS_VERSION.json`, the folder under
    /// `vendor/android-system/`.
    pub fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64-v8a",
        }
    }

    /// Docker's platform of the image (`RUNTIME_PLATFORM`).
    pub fn platform(self) -> &'static str {
        match self {
            Arch::X86_64 => "linux/amd64",
            Arch::Arm64 => "linux/arm64",
        }
    }

    /// The split APK of a bundle meant for this architecture (`split_config.x86_64.apk`).
    fn preferred_split(self) -> &'static str {
        match self {
            Arch::X86_64 => "split_config.x86_64.apk",
            Arch::Arm64 => "split_config.arm64_v8a.apk",
        }
    }

    /// The library folders of an APK to look in, best first.
    fn lib_dirs(self) -> [&'static str; 2] {
        match self {
            Arch::X86_64 => ["lib/x86_64", "lib/x86"],
            Arch::Arm64 => ["lib/arm64-v8a", "lib/arm64"],
        }
    }
}

/// What `LIBS_VERSION.json` pins for one architecture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pins {
    /// Apple's libraries: file name → SHA-256 (hex)
    pub libs: BTreeMap<String, String>,
    /// the Android system files: path under `rootfs/system/` (`bin/linker64`, `lib64/libc.so`) → SHA-256
    pub system: BTreeMap<String, String>,
    /// the Apple Music for Android version the libraries are from ("3.6.0-beta")
    pub apple_version: String,
    /// and its build number ("1109")
    pub apple_build: String,
}

impl Pins {
    /// The pins of `arch` in the text of `LIBS_VERSION.json`.
    pub fn parse(text: &str, arch: Arch) -> Result<Pins> {
        let v: Value = serde_json::from_str(text).context("LIBS_VERSION.json does not read as JSON")?;
        // the `$comment` keys some sections carry are notes, not files
        let table = |section: &str| -> BTreeMap<String, String> {
            v[section][arch.name()].as_object().map(|o| {
                o.iter().filter(|(k, _)| !k.starts_with('$')).filter_map(|(k, h)| h.as_str().map(|h| (k.clone(), h.to_ascii_lowercase()))).collect()
            }).unwrap_or_default()
        };
        let pins = Pins {
            libs: table("libs"),
            system: table("android_system"),
            apple_version: v["apple_music"]["version"].as_str().unwrap_or("").to_string(),
            apple_build: v["apple_music"]["build"].as_str().unwrap_or("").to_string(),
        };
        if pins.libs.is_empty() {
            bail!("LIBS_VERSION.json pins no Apple libraries for {}", arch.name());
        }
        if pins.system.is_empty() {
            bail!("LIBS_VERSION.json pins no Android system files for {}", arch.name());
        }
        Ok(pins)
    }

    /// "Apple Music for Android 3.6.0-beta (build 1109)": the app version the owner has to bring.
    pub fn wanted(&self) -> String {
        match (self.apple_version.is_empty(), self.apple_build.is_empty()) {
            (true, _) => "the Apple Music for Android version wrapper-v2 pins".into(),
            (false, true) => format!("Apple Music for Android {}", self.apple_version),
            (false, false) => format!("Apple Music for Android {} (build {})", self.apple_version, self.apple_build),
        }
    }
}

/// The SHA-256 of some bytes, as lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// How well a member name of a bundle fits `arch`, as extract-libs.sh scores it: the split named for the
/// architecture 100 (plus the name bonus), a name that mentions it 50 (x86 another 10), `base.apk` 20 less.
/// A score of 0 or less is not worth opening first.
pub fn split_score(name: &str, arch: Arch) -> i32 {
    let base = name.rsplit('/').next().unwrap_or(name);
    let mut score = if base == arch.preferred_split() { 100 } else { 0 };
    match arch {
        Arch::X86_64 => {
            if base.contains("x86_64") || base.contains("x86-64") {
                score += 50;
            }
            if base.contains("x86") {
                score += 10;
            }
        }
        Arch::Arm64 => {
            if base.contains("arm64_v8a") || base.contains("arm64-v8a") || base.contains("arm64") {
                score += 50;
            }
        }
    }
    if base == "base.apk" {
        score -= 20;
    }
    score
}

/// The folder of an APK (given by its member names) that holds Apple's libraries: the architecture's own
/// folders first, then any `lib/<abi>/` with the sentinel. None when the APK has no such folder.
pub fn lib_dir(names: &[String], arch: Arch) -> Option<String> {
    for dir in arch.lib_dirs() {
        if names.iter().any(|n| *n == format!("{dir}/{SENTINEL}")) {
            return Some(dir.to_string());
        }
    }
    // a folder named differently: the first lib/<abi>/ that has the sentinel
    names.iter().filter_map(|n| n.strip_suffix(&format!("/{SENTINEL}"))).find(|d| d.starts_with("lib/") && d.matches('/').count() == 1).map(String::from)
}

/// The member names of a zip (an APK, or a bundle of APKs), as the zip stores them.
fn names_of<R: Read + std::io::Seek>(z: &zip::ZipArchive<R>) -> Vec<String> {
    z.file_names().filter_map(|n| n.ok()).map(|n| n.into_owned()).collect()
}

/// The bytes of one member of a zip, refused past [`MAX_SPLIT_BYTES`].
fn read_member<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let f = z.by_name(name).with_context(|| format!("{name} is not in the bundle"))?;
    if f.size() > MAX_SPLIT_BYTES {
        bail!("{name} is larger than {} MB", MAX_SPLIT_BYTES >> 20);
    }
    let mut out = Vec::with_capacity(f.size() as usize);
    f.take(MAX_SPLIT_BYTES + 1).read_to_end(&mut out)?;
    Ok(out)
}

/// What [`extract_libs`] did.
#[derive(Debug, Default, PartialEq)]
pub struct Extracted {
    /// the libraries written
    pub libs: usize,
    /// the APK they came from (the bundle's member, or "the APK" for a plain APK)
    pub from: String,
    /// the folder inside it (`lib/x86_64`)
    pub dir: String,
}

/// Take Apple's libraries for `arch` out of `bundle` (an `.apk` or an `.apkm` of split APKs: told apart by
/// what is inside, not by the name) into `out`, each one checked against `pins`. Refused, with nothing
/// written, when any pinned library is missing or differs: then the app is not the version wrapper-v2 needs.
pub fn extract_libs(bundle: &Path, arch: Arch, pins: &Pins, out: &Path) -> Result<Extracted> {
    let file = std::fs::File::open(bundle).with_context(|| format!("can't open {}", bundle.display()))?;
    let mut outer = zip::ZipArchive::new(file).context("The uploaded file is not an APK or an APKM bundle (it does not open as a zip)")?;
    let names = names_of(&outer);
    // a plain APK has its libraries itself; a bundle has APKs (the splits) instead
    let (apk_bytes, from, dir) = match lib_dir(&names, arch) {
        Some(dir) => (None, "the APK".to_string(), dir),
        None => {
            let mut splits: Vec<(i32, String)> = names.iter().filter(|n| n.to_ascii_lowercase().ends_with(".apk")).map(|n| (split_score(n, arch), n.clone())).collect();
            if splits.is_empty() {
                bail!("The uploaded APK has no {SENTINEL} for {} (is it Apple Music for Android, and the build for this machine?)", arch.name());
            }
            // the best-scoring splits first; the others only when none of those has the libraries
            splits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let mut found = None;
            for (_, name) in &splits {
                let bytes = read_member(&mut outer, name)?;
                let Ok(inner) = zip::ZipArchive::new(Cursor::new(bytes.as_slice())) else { continue };
                if let Some(dir) = lib_dir(&names_of(&inner), arch) {
                    found = Some((Some(bytes), name.clone(), dir));
                    break;
                }
            }
            found.ok_or_else(|| anyhow::anyhow!("No APK in the bundle has {SENTINEL} for {} (is it Apple Music for Android?)", arch.name()))?
        }
    };
    // read and check every pinned library before writing any
    let mut libs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    {
        let mut take = |z: &mut dyn FnMut(&str) -> Option<Result<Vec<u8>>>| {
            for (so, want) in &pins.libs {
                match z(&format!("{dir}/{so}")) {
                    None => problems.push(format!("{so} is missing")),
                    Some(Err(e)) => problems.push(format!("{so}: {e}")),
                    Some(Ok(bytes)) if sha256_hex(&bytes) != *want => problems.push(format!("{so} differs")),
                    Some(Ok(bytes)) => libs.push((so.clone(), bytes)),
                }
            }
        };
        match &apk_bytes {
            Some(bytes) => {
                let mut inner = zip::ZipArchive::new(Cursor::new(bytes.as_slice()))?;
                let inner_names = names_of(&inner);
                take(&mut |name| inner_names.iter().any(|n| n == name).then(|| read_member(&mut inner, name)));
            }
            None => take(&mut |name| names.iter().any(|n| n == name).then(|| read_member(&mut outer, name))),
        }
    }
    if !problems.is_empty() {
        let shown: Vec<String> = problems.iter().take(4).cloned().collect();
        let more = if problems.len() > 4 { format!(" and {} more", problems.len() - 4) } else { String::new() };
        bail!("This is not the Apple Music version wrapper-v2 needs ({}): {}{more}. Upload {} instead.", from, shown.join(", "), pins.wanted());
    }
    std::fs::create_dir_all(out)?;
    for (so, bytes) in &libs {
        write_file(&out.join(so), bytes, 0o644)?;
    }
    Ok(Extracted { libs: libs.len(), from, dir })
}

/// Copy the Android system files of `arch` from the source's `vendor/android-system/<arch>/` into
/// `<rootfs>/system/`, each checked against `pins` (programs under `bin/` executable). The number copied.
pub fn stage_system(src: &Path, arch: Arch, pins: &Pins, rootfs: &Path) -> Result<usize> {
    let vendor = src.join("vendor/android-system").join(arch.name());
    if !vendor.is_dir() {
        bail!("The wrapper-v2 source has no Android system files for {}", arch.name());
    }
    let mut problems = Vec::new();
    let mut files = Vec::new();
    for (rel, want) in &pins.system {
        // the pins name plain relative paths; anything else is not followed
        if rel.starts_with('/') || rel.split('/').any(|p| p == ".." || p.is_empty()) {
            problems.push(format!("{rel} is not a plain path"));
            continue;
        }
        match std::fs::read(vendor.join(rel)) {
            Err(_) => problems.push(format!("{rel} is missing")),
            Ok(bytes) if sha256_hex(&bytes) != *want => problems.push(format!("{rel} differs")),
            Ok(bytes) => files.push((rel.clone(), bytes)),
        }
    }
    if !problems.is_empty() {
        bail!("The wrapper-v2 source's Android system files do not match its pins: {}", problems.join(", "));
    }
    for (rel, bytes) in &files {
        let mode = if rel.starts_with("bin/") { 0o755 } else { 0o644 };
        write_file(&rootfs.join("system").join(rel), bytes, mode)?;
    }
    Ok(files.len())
}

/// Write `bytes` to `path` (its folders made) with the permission bits `mode`.
fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::File::create(path).with_context(|| format!("can't write {}", path.display()))?;
    f.write_all(bytes)?;
    f.set_permissions(std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// The Dockerfile without BuildKit's `--mount=…` flags (see the module's notes): `RUN --mount=a \` becomes
/// `RUN \`, and a continuation line that held only a flag goes.
pub fn patch_dockerfile(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if !line.contains("--mount=") {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        let words: Vec<&str> = line.split_whitespace().filter(|w| !w.starts_with("--mount=")).collect();
        // a continuation line that held only flags (and its backslash) adds nothing
        if words.is_empty() || words == ["\\"] {
            continue;
        }
        out.push_str(indent);
        out.push_str(&words.join(" "));
        out.push('\n');
    }
    out
}

/// Whether a path of the source belongs in the build context: the repository's own folders (`.git`,
/// `.github`), the Android files once copied into the rootfs (`vendor`) and the notes stay out, as its
/// `.dockerignore` would keep most of them out anyway.
pub fn in_context(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    !matches!(first, ".git" | ".github" | "vendor" | "data" | "build" | ".tmp" | ".cache")
        && !rel.to_ascii_lowercase().ends_with(".apk")
        && !rel.to_ascii_lowercase().ends_with(".apkm")
}

/// One file of a tar, in the ustar format Docker reads: a 512-byte header and the bytes, padded to 512.
fn tar_header(name: &str, size: u64, mode: u32, kind: u8, link: &str) -> Result<[u8; 512]> {
    let mut h = [0u8; 512];
    // names past 100 bytes go into the 155-byte prefix, split at a slash
    let (prefix, base) = if name.len() <= 100 {
        ("", name)
    } else {
        let cut = name[..name.len().min(156)].rfind('/').filter(|i| name.len() - i - 1 <= 100).ok_or_else(|| anyhow::anyhow!("{name}: too long a name for the build context"))?;
        (&name[..cut], &name[cut + 1..])
    };
    if link.len() > 100 {
        bail!("{name}: too long a link target for the build context");
    }
    h[..base.len()].copy_from_slice(base.as_bytes());
    let octal = |field: &mut [u8], v: u64| {
        let s = format!("{v:0width$o}", width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
    };
    octal(&mut h[100..108], mode as u64);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], 0);
    h[156] = kind;
    h[157..157 + link.len()].copy_from_slice(link.as_bytes());
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    // the checksum is counted with its own field as spaces
    h[148..156].copy_from_slice(b"        ");
    let sum: u64 = h.iter().map(|b| *b as u64).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    Ok(h)
}

/// Write the folder `dir` as an uncompressed tar to `out` (the build context Docker's `/build` takes): files
/// with their execute bit, folders, symbolic links; only the paths [`in_context`] keeps. The bytes written.
pub fn write_tar(dir: &Path, out: &Path) -> Result<u64> {
    use std::os::unix::fs::PermissionsExt;
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).with_context(|| format!("can't write {}", out.display()))?);
    let mut entries: Vec<PathBuf> = walkdir::WalkDir::new(dir).min_depth(1).sort_by_file_name().into_iter()
        .filter_entry(|e| e.path().strip_prefix(dir).map(|r| in_context(&r.to_string_lossy())).unwrap_or(false))
        .filter_map(|e| e.ok()).map(|e| e.into_path()).collect();
    entries.sort();
    let mut total = 0u64;
    for path in entries {
        let rel = path.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path)?.to_string_lossy().into_owned();
            w.write_all(&tar_header(&rel, 0, 0o777, b'2', &target)?)?;
            total += 512;
        } else if meta.is_dir() {
            w.write_all(&tar_header(&format!("{rel}/"), 0, 0o755, b'5', "")?)?;
            total += 512;
        } else if meta.is_file() {
            let mode = if meta.permissions().mode() & 0o111 != 0 { 0o755 } else { 0o644 };
            let size = meta.len();
            w.write_all(&tar_header(&rel, size, mode, b'0', "")?)?;
            let mut f = std::fs::File::open(&path)?;
            let copied = std::io::copy(&mut (&mut f).take(size), &mut w)?;
            if copied != size {
                bail!("{rel} changed while the build context was written");
            }
            let pad = (512 - size % 512) % 512;
            w.write_all(&vec![0u8; pad as usize])?;
            total += 512 + size + pad;
        }
    }
    // the end: two empty blocks
    w.write_all(&[0u8; 1024])?;
    w.flush()?;
    Ok(total + 1024)
}

/// The source's one top folder after a GitHub tarball is unpacked (`wrapper-v2-<commit>/`), or the folder
/// itself when it has no single top folder.
pub fn top_folder(dir: &Path) -> PathBuf {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    match entries.as_slice() {
        [one] if one.is_dir() => one.clone(),
        _ => dir.to_path_buf(),
    }
}

/// What [`prepare`] made ready.
#[derive(Debug)]
pub struct Prepared {
    /// the Apple libraries copied
    pub libs: Extracted,
    /// the Android system files copied
    pub system: usize,
}

/// Make the unpacked source at `src` ready to build for `arch`: Apple's libraries from `bundle` into
/// `rootfs/system/lib64/`, the Android system files into `rootfs/system/`, the Dockerfile without its cache
/// mounts. Run once per fresh copy of the source.
pub fn prepare(src: &Path, bundle: &Path, arch: Arch) -> Result<Prepared> {
    let text = std::fs::read_to_string(src.join("LIBS_VERSION.json")).context("The wrapper-v2 source has no LIBS_VERSION.json")?;
    let pins = Pins::parse(&text, arch)?;
    let libs = extract_libs(bundle, arch, &pins, &src.join("rootfs/system/lib64"))?;
    let system = stage_system(src, arch, &pins, &src.join("rootfs"))?;
    let dockerfile = src.join("Dockerfile");
    let text = std::fs::read_to_string(&dockerfile).context("The wrapper-v2 source has no Dockerfile")?;
    std::fs::write(&dockerfile, patch_dockerfile(&text))?;
    Ok(Prepared { libs, system })
}

/// What kind of file the owner uploaded, by its contents: "APK" (a zip with `lib/`), "APKM bundle" (a zip
/// of APKs), or None when it is neither.
pub fn bundle_kind(path: &Path) -> Option<&'static str> {
    let z = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
    let names = names_of(&z);
    if names.iter().any(|n| n.to_ascii_lowercase().ends_with(".apk")) {
        Some("APKM bundle")
    } else if names.iter().any(|n| n.starts_with("lib/") || n == "AndroidManifest.xml") {
        Some("APK")
    } else {
        None
    }
}
