#!/usr/bin/env bash
# Fails when a Cargo.toml anywhere in the repo lists a dependency no code uses (the server's crates, the web app,
# the desktop app and the phone's engine): such a dependency only costs build time. cargo-machete reads the
# sources, it builds nothing, so the check takes seconds once the tool is there.
#
#   scripts/check-unused-deps.sh        exit 0: none unused; 1: cargo machete lists the unused ones
#
# Installs cargo-machete first when it is missing (CI: the kit's kit-ci runs this as an app check, after its own
# Rust toolchain step). Run by the test job of .github/workflows/release.yml and docker.yml.
set -euo pipefail
cd "$(dirname "$0")/.."
# the pinned version keeps CI from moving to a new release's rules without a change here
MACHETE_VERSION=0.9.2
if ! command -v cargo-machete >/dev/null 2>&1; then
  cargo install --locked --quiet cargo-machete --version "$MACHETE_VERSION"
fi
cargo machete
