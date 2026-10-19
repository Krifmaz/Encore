//! Finding a program the way a shell would.
use std::path::{Path, PathBuf};

/// The program `name` on PATH (or `name` itself when it is a path to a file).
pub fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    std::env::var("PATH").ok()?.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(name)).find(|p| p.is_file())
}
