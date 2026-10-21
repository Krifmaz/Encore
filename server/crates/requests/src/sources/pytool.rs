//! Running the downloaders of the zotify and gamdl plugins (Python programs in the plugin's venv): one run into
//! the request's folder, its output into the plugin's log (secrets masked, progress bars left out), the
//! progress bar from the files that appear and from the program's own lines, and the new audio files after.
use crate::Ctx;
use encore_core::audio::DOWNLOAD_EXTS;
use encore_plugins::{Code, Failure};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncReadExt;

/// The plugin's data folder (tokens, cookies, caches), made when missing.
pub fn data(st: &impl Ctx, id: &str) -> PathBuf {
    let d = st.plugins().folders(id).data();
    let _ = std::fs::create_dir_all(&d);
    d
}

/// The plugin's program in its venv, when installed.
pub fn program(st: &impl Ctx, id: &str) -> Option<PathBuf> {
    st.plugins().program(id).map(|(_, p)| p).filter(|p| p.is_file())
}

/// The venv's Python, beside the program.
pub fn python(st: &impl Ctx, id: &str) -> Option<PathBuf> {
    program(st, id).and_then(|p| p.parent().map(|d| d.join("python"))).filter(|p| p.exists())
}

pub fn not_installed(name: &str) -> Failure {
    Failure::new(Code::NotFound, format!("{name} is not installed here – Update on its card under Settings → Plugins installs it"))
}

/// Write a file only Encore's user can read (tokens, cookies), through a temporary name.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(text.as_bytes())?;
    drop(f);
    std::fs::rename(&tmp, path)
}

pub fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn write_json(path: &Path, v: &serde_json::Value) -> std::io::Result<()> {
    write_private(path, &serde_json::to_string_pretty(v).unwrap_or_default())
}

/// The audio files under `folder` with their change times (half-written ones left out).
pub fn audio_files(folder: &Path) -> HashMap<PathBuf, SystemTime> {
    let mut out = HashMap::new();
    let mut dirs = vec![folder.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(p);
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            let ext = name.rsplit_once('.').map(|(_, x)| x).unwrap_or("");
            if name.starts_with('.') || !DOWNLOAD_EXTS.contains(&ext) {
                continue;
            }
            out.insert(p, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
        }
    }
    out
}

/// What a line of the program's output means: a progress percent, and whether it goes to the log.
pub type Hook<'a> = &'a (dyn Fn(&str) -> (Option<f64>, bool) + Send + Sync);

pub struct Run {
    pub success: bool,
    pub code: Option<i32>,
    /// the new audio files, in name order
    pub files: Vec<PathBuf>,
    /// the last lines it printed
    pub tail: Vec<String>,
}

/// Mask what must not reach the log.
fn clean(line: &str, secrets: &[String]) -> String {
    let mut s = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // an ANSI escape: up to its final letter
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() || n == '~' {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() || c == '\t' {
            s.push(c);
        }
    }
    let mut s = s.trim().to_string();
    for secret in secrets.iter().filter(|x| x.len() >= 6) {
        s = s.replace(secret.as_str(), "***");
    }
    s
}

pub struct Spec<'a> {
    pub id: &'a str,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub folder: &'a Path,
    /// files the download should deliver (1 for a song, an album's track count)
    pub expected: usize,
    pub hook: Option<Hook<'a>>,
    pub secrets: Vec<String>,
    /// the cancel switch and the progress bar
    pub call: &'a encore_plugins::Call,
}

/// Run the program into `spec.folder`.
pub async fn run(st: &impl Ctx, spec: Spec<'_>) -> Result<Run, Failure> {
    let before = audio_files(spec.folder);
    let data = data(st, spec.id);
    let log = st.plugins().log(spec.id);
    let mut env = encore_plugins::sandbox::program_env();
    env.extend([("PYTHONUNBUFFERED".into(), "1".into()), ("NO_COLOR".into(), "1".into()), ("TERM".into(), "dumb".into()), ("COLUMNS".into(), "120".into())]);
    let spawn = encore_plugins::sandbox::Spawn::new(&encore_plugins::sandbox::Sandbox::limits_only(), &spec.program)
        .args(spec.args.iter().cloned())
        .env(env)
        .cwd(&data)
        .low_priority(true);
    let mut cmd = spawn.tokio_command();
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| Failure::internal(format!("could not start {}: {e}", spec.program.display())))?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for mut pipe in [out.map(|o| Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), err.map(|e| Box::new(e) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)].into_iter().flatten() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let (mut buf, mut chunk) = (Vec::<u8>::new(), [0u8; 4096]);
            loop {
                let n = match pipe.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                // a progress bar redraws with \r: a line end too
                while let Some(i) = buf.iter().position(|b| *b == b'\n' || *b == b'\r') {
                    let line: Vec<u8> = buf.drain(..=i).collect();
                    let _ = tx.send(String::from_utf8_lossy(&line).into_owned());
                }
            }
            if !buf.is_empty() {
                let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
            }
        });
    }
    drop(tx);
    let (mut tail, mut hinted, mut status) = (Vec::<String>::new(), 0.0f64, None);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut lines_open = true;
    loop {
        tokio::select! {
            line = rx.recv(), if lines_open => match line {
                Some(raw) => {
                    let line = clean(&raw, &spec.secrets);
                    if line.is_empty() {
                        continue;
                    }
                    let (pct, keep) = spec.hook.map(|h| h(&line)).unwrap_or((None, true));
                    if let Some(p) = pct {
                        hinted = hinted.max(p);
                    }
                    if keep {
                        log.line("download", &line);
                        tail.push(line);
                        if tail.len() > 40 {
                            tail.remove(0);
                        }
                    }
                }
                None => lines_open = false,
            },
            s = child.wait(), if status.is_none() => {
                status = Some(s.map_err(|e| Failure::internal(e.to_string()))?);
            }
            _ = tick.tick() => {
                if let Some(cb) = &spec.call.progress {
                    let done = audio_files(spec.folder).keys().filter(|p| !before.contains_key(*p)).count();
                    let by_files = if spec.expected > 0 { done as f64 / spec.expected as f64 * 100.0 } else { 0.0 };
                    cb(by_files.max(hinted).min(99.0), (spec.expected > 1).then(|| format!("{done}/{} files", spec.expected)));
                }
            }
            _ = spec.call.cancel.cancelled() => {
                let _ = child.start_kill();
                return Err(Failure::cancelled());
            }
        }
        if status.is_some() && !lines_open {
            break;
        }
    }
    let status = status.expect("waited above");
    let after = audio_files(spec.folder);
    let mut files: Vec<PathBuf> = after.into_iter().filter(|(p, m)| before.get(p) != Some(m)).map(|(p, _)| p).collect();
    files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    if let Some(cb) = &spec.call.progress {
        cb(100.0, None);
    }
    Ok(Run { success: status.success(), code: status.code(), files, tail })
}

/// The last line worth showing from a run's output.
pub fn last_message(tail: &[String]) -> String {
    tail.iter().rev().map(|l| l.trim_matches(|c: char| c == '#' || c == '=' || c.is_whitespace())).find(|l| !l.is_empty()).map(|l| l.chars().take(300).collect()).unwrap_or_else(|| "no output".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_cleaned_and_secrets_masked() {
        let secret = vec!["abcdefSECRET".to_string()];
        assert_eq!(clean("\u{1b}[32mtoken abcdefSECRET ok\u{1b}[0m", &secret), "token *** ok");
        assert_eq!(clean("  plain  ", &[]), "plain");
        assert_eq!(clean("short abc", &["abc".into()]), "short abc", "too short to be a secret");
    }

    #[test]
    fn the_last_message_skips_decoration() {
        let tail = vec!["first".to_string(), "###   NO SESSION CREATED, EXITING PROGRAM   ###".to_string(), "".to_string()];
        assert_eq!(last_message(&tail), "NO SESSION CREATED, EXITING PROGRAM");
        assert_eq!(last_message(&[]), "no output");
    }

    #[test]
    fn audio_files_are_found_in_subfolders() {
        let dir = std::env::temp_dir().join(format!("encore-pytool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Artist/Album")).unwrap();
        for f in ["Artist/Album/01 Song.m4a", "Artist/Album/01 Song.lrc", "Artist/Album/02 Song.ogg", "Artist/Album/.hidden.mp3", "x.part"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let mut found: Vec<String> = audio_files(&dir).keys().map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned()).collect();
        found.sort();
        assert_eq!(found, ["Artist/Album/01 Song.m4a", "Artist/Album/02 Song.ogg"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
