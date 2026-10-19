//! Scratch folders for tests.
use std::path::PathBuf;

/// A new, empty folder under the system's temp folder, its own for this call:
/// `encore-<name>-<process id>-<random>`. The caller removes it when done.
pub fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("encore-{name}-{}-{}", std::process::id(), crate::ids::random_hex(4)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a temp folder");
    d
}
