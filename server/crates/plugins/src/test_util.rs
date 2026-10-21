//! Helpers of the plugin tests: a stand-in web server, archive builders, the test plugin, the sandbox levels.
use crate::sandbox::{Level, Sandbox};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// path → (status, body, extra headers)
type Routes = Arc<Mutex<HashMap<String, (u16, Vec<u8>, Vec<(String, String)>)>>>;

/// A web server on 127.0.0.1 answering fixed paths: (status, body, extra headers). Paths can be added later.
#[derive(Clone)]
pub struct StandIn {
    pub base: String,
    routes: Routes,
    /// every path asked for, in order
    pub hits: Arc<Mutex<Vec<String>>>,
}

impl StandIn {
    pub async fn start() -> StandIn {
        let routes: Routes = Arc::default();
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let (r2, h2) = (routes.clone(), hits.clone());
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let (routes, hits) = (r2.clone(), h2.clone());
            async move {
                let key = uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default();
                hits.lock().unwrap().push(key.clone());
                let found = {
                    let r = routes.lock().unwrap();
                    r.get(&key).cloned().or_else(|| r.get(uri.path()).cloned())
                };
                let (code, body, headers) = found.unwrap_or((404, b"{\"message\":\"Not Found\"}".to_vec(), vec![]));
                let mut resp = axum::response::Response::builder().status(code);
                for (k, v) in headers {
                    resp = resp.header(k, v);
                }
                resp.body(axum::body::Body::from(body)).unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        StandIn { base: format!("http://{addr}"), routes, hits }
    }

    pub fn put(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.routes.lock().unwrap().insert(path.to_string(), (200, body.into(), vec![]));
    }

    pub fn put_status(&self, path: &str, code: u16, body: impl Into<Vec<u8>>, headers: &[(&str, &str)]) {
        self.routes.lock().unwrap().insert(path.to_string(), (code, body.into(), headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()));
    }

    /// How many times a path was asked for.
    pub fn hits_of(&self, path: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|h| h.as_str() == path).count()
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

/// A zip of (name, contents, unix mode); a mode with 0o120000 makes a symbolic link (contents: its target).
pub fn zip_of(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data, mode) in entries {
        let o = zip::write::SimpleFileOptions::default().unix_permissions(*mode & 0o7777);
        if mode & 0o170000 == 0o120000 {
            z.add_symlink(*name, String::from_utf8_lossy(data), o).unwrap();
        } else if name.ends_with('/') {
            z.add_directory(*name, o).unwrap();
        } else {
            z.start_file(*name, o).unwrap();
            z.write_all(data).unwrap();
        }
    }
    z.finish().unwrap().into_inner()
}

/// One tar entry: (name, contents, mode, type flag, link name).
pub struct TarEntry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
    pub mode: u32,
    pub kind: u8,
    pub link: &'a str,
}

pub fn file<'a>(name: &'a str, data: &'a [u8], mode: u32) -> TarEntry<'a> {
    TarEntry { name, data, mode, kind: b'0', link: "" }
}

pub fn link<'a>(name: &'a str, target: &'a str) -> TarEntry<'a> {
    TarEntry { name, data: b"", mode: 0o777, kind: b'2', link: target }
}

fn header(name: &[u8], size: usize, mode: u32, kind: u8, link: &[u8]) -> [u8; 512] {
    let mut h = [0u8; 512];
    let n = name.len().min(100);
    h[..n].copy_from_slice(&name[..n]);
    h[100..107].copy_from_slice(format!("{mode:07o}").as_bytes());
    h[108..115].copy_from_slice(b"0000000");
    h[116..123].copy_from_slice(b"0000000");
    h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
    h[136..147].copy_from_slice(b"00000000000");
    h[156] = kind;
    let l = link.len().min(100);
    h[157..157 + l].copy_from_slice(&link[..l]);
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    h
}

/// A .tar.gz of the entries (names longer than 100 bytes get a GNU long-name entry before them).
pub fn tar_gz(entries: &[TarEntry]) -> Vec<u8> {
    let mut tar = Vec::new();
    let push = |tar: &mut Vec<u8>, h: [u8; 512], data: &[u8]| {
        tar.extend_from_slice(&h);
        tar.extend_from_slice(data);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    };
    for e in entries {
        if e.name.len() > 100 {
            let mut long = e.name.as_bytes().to_vec();
            long.push(0);
            push(&mut tar, header(b"././@LongLink", long.len(), 0o644, b'L', b""), &long);
        }
        push(&mut tar, header(e.name.as_bytes(), e.data.len(), e.mode, e.kind, e.link.as_bytes()), e.data);
    }
    tar.extend_from_slice(&[0u8; 1024]);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

/// The repository's root (server/crates/plugins → ../../..).
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

/// The SDK file.
pub fn sdk() -> Vec<u8> {
    std::fs::read(repo().join("plugins/sdk/encore_plugin.py")).unwrap()
}

/// The test plugin's files (testdata/test-tone + the SDK), with its manifest changed by `edit`.
pub fn tone_files(edit: impl Fn(String) -> String) -> Vec<(String, Vec<u8>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test-tone");
    vec![
        ("plugin.toml".into(), edit(std::fs::read_to_string(dir.join("plugin.toml")).unwrap()).into_bytes()),
        ("plugin.py".into(), std::fs::read(dir.join("plugin.py")).unwrap()),
        ("encore_plugin.py".into(), sdk()),
    ]
}

/// The test plugin as a zip (in a top folder `top` when given).
pub fn tone_zip(top: Option<&str>, edit: impl Fn(String) -> String) -> Vec<u8> {
    let files = tone_files(edit);
    let names: Vec<String> = files.iter().map(|(n, _)| top.map(|t| format!("{t}/{n}")).unwrap_or_else(|| n.clone())).collect();
    let entries: Vec<(&str, &[u8], u32)> = files.iter().zip(&names).map(|((_, d), n)| (n.as_str(), d.as_slice(), 0o644)).collect();
    zip_of(&entries)
}

/// A Python 3.11 or newer for the SDK (None: the Python tests are skipped, saying so).
pub fn python() -> Option<PathBuf> {
    let p = crate::process::find_python()?;
    let ok = std::process::Command::new(&p).args(["-c", "import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)"]).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("skipped: no Python 3.11 or newer here");
        return None;
    }
    Some(p)
}

/// The sandbox levels to test: limits only always, full isolation when this host allows it.
pub fn levels() -> Vec<Sandbox> {
    let mut out = vec![Sandbox::limits_only()];
    let found = Sandbox::detect();
    if matches!(found.level, Level::Full { .. }) {
        eprintln!("full isolation is available here: {:?}", found.level);
        out.push(found);
    } else {
        eprintln!("full isolation is not available here ({}): only limits-only runs", found.reason.unwrap_or_default());
    }
    out
}

/// A shell script made executable (written under a temporary name and renamed, so no open write handle points at it).
pub fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tmp = path.with_extension("tmp-script");
    std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}
