//! Embeds the release notes (docs/releases/<MAJOR.MINOR.PATCH>.md) into changelog_gen.rs (in OUT_DIR): `NOTES`,
//! (version, markdown) pairs, which /api/changelog serves to the What's new popup and Settings → What's new. The
//! Docker build copies docs/releases to /docs/releases, the same place relative to this crate as in the repository.
//! Without the folder the list is empty.
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/releases");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut notes: Vec<(String, String)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let v = name.strip_suffix(".md")?;
                    // plain releases only: 0.36.0, not the old dated 2026.10.11.1 or the -edge builds
                    let parts: Vec<&str> = v.split('.').collect();
                    (parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
                        .then(|| (v.to_string(), e.path().canonicalize().unwrap_or(e.path()).display().to_string()))
                })
                .collect()
        })
        .unwrap_or_else(|_| {
            println!("cargo:warning=no release notes at {}: the changelog is empty", dir.display());
            Vec::new()
        });
    notes.sort();
    let mut code = String::from("pub static NOTES: &[(&str, &str)] = &[\n");
    for (v, path) in &notes {
        code.push_str(&format!("    ({v:?}, include_str!({path:?})),\n"));
    }
    code.push_str("];\n");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("changelog_gen.rs");
    std::fs::write(out, code).unwrap();
}
