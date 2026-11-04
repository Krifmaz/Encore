//! Embeds the release notes (docs/releases/<MAJOR.MINOR.PATCH>.md) into changelog_gen.rs (in OUT_DIR): `NOTES`,
//! (version, markdown) pairs, which /api/changelog serves to the What's new popup and Settings → What's new. The
//! Docker build copies docs/releases to /docs/releases, the same place relative to this crate as in the repository.
//! Without the folder the list is empty. The embedding is the kit's (`kit_server_changelog::build`).
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/releases");
    kit_server_changelog::build::embed(&dir, "changelog_gen.rs");
}
