#!/usr/bin/env bash
# Checks for the desktop packaging that need no build (the desktop job of .github/workflows/apps.yml runs them on
# Linux before it builds; run them by hand after changing names here or in the app):
#   - the release file names scripts/desktop-package.sh makes are the ones the in-app updater looks for
#     (src/desktop/updater.rs), and the Windows installer puts Encore where the updater starts it again;
#   - the uninstaller removes the "Start with the computer" entry under the name the app registers;
#   - the version rules: dev builds get a Debian version below the release, --stamp writes the version into
#     Cargo.toml and Cargo.lock (tried on copies).
# shellcheck disable=SC2016  # the $ in the single-quoted names below are the literal text of the files
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
CRATE="$ROOT/clients/desktop"
SCRIPT="$ROOT/scripts/desktop-package.sh"
UPDATER="$CRATE/src/desktop/updater.rs"
AUTOSTART="$CRATE/src/desktop/autostart.rs"
NSI="$CRATE/packaging/windows/encore.nsi"
FAILED=0

ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1"; FAILED=1; }
has() { # <what> <text> <file>
  if grep -qF -- "$2" "$3"; then ok "$1"; else bad "$1: $(basename "$3") has no $2"; fi
}

# ---- the names the updater asks the release for, and the ones the script makes
has "updater: Windows file"   'Encore-{version}-windows-x86_64.exe' "$UPDATER"
has "updater: macOS file"     'Encore-{version}-macos-universal.dmg' "$UPDATER"
has "updater: AppImage"       'Encore-{version}-linux-x86_64.AppImage' "$UPDATER"
has "updater: .deb"           'encore_{version}_amd64.deb' "$UPDATER"
has "script: Windows file"    'Encore-$V-windows-$ARCH.exe' "$SCRIPT"
has "script: macOS file"      'Encore-$V-macos-$arch.dmg' "$SCRIPT"
has "script: universal"       'arch=universal' "$SCRIPT"
has "script: AppImage"        'Encore-$V-linux-$ARCH.AppImage' "$SCRIPT"
has "script: .deb"            'encore_${V}_${DEBARCH}.deb' "$SCRIPT"

# ---- where the Windows installer puts Encore, and where the updater starts it after a silent install
has "installer: folder"       'InstallDir "$LOCALAPPDATA\Encore"' "$NSI"
has "installer: program"      '!define EXE "encore-desktop.exe"' "$NSI"
has "updater: program"        "'Encore\\\\encore-desktop.exe'" "$UPDATER"
has "updater: silent install" "-ArgumentList '/S'" "$UPDATER"
# an install in another folder: the updater reads InstallLocation from the installer's Apps & features entry
key=$(sed -n 's/^!define UNINSTKEY "\(.*\)"$/\1/p' "$NSI")
has "installer: InstallLocation" '"${UNINSTKEY}" "InstallLocation"' "$NSI"
if [ -n "$key" ]; then has "updater: the installer's entry" "HKCU:\\\\${key//\\/\\\\}" "$UPDATER"
else bad "encore.nsi has no UNINSTKEY"; fi

# ---- the autostart entry the uninstaller removes
entry=$(sed -n 's/^pub const ENTRY: &str = "\(.*\)";/\1/p' "$AUTOSTART")
if [ -n "$entry" ] && grep -qF "!define AUTOSTART \"$entry\"" "$NSI"; then ok "uninstaller: autostart entry \"$entry\""
else bad "uninstaller: the autostart entry \"$entry\" (src/desktop/autostart.rs) is not the one encore.nsi removes"; fi

# ---- versions (on a copy of the files the script reads and writes)
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/scripts" "$TMP/clients/desktop"
cp "$SCRIPT" "$TMP/scripts/"
cp "$CRATE/Cargo.toml" "$CRATE/Cargo.lock" "$TMP/clients/desktop/"
cp -R "$CRATE/packaging" "$TMP/clients/desktop/"
CARGO_V=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$CRATE/Cargo.toml" | head -1)
STANDIN="$TMP/stand-in"
printf '#!/bin/sh\n' > "$STANDIN" && chmod +x "$STANDIN"

line=$(bash "$TMP/scripts/desktop-package.sh" --dry-run --bin "$STANDIN" --version dev.5 --out "$TMP/out" | grep '^Encore desktop')
case "$line" in
  *"package version ${CARGO_V%%[-+]*}~dev.5"*) ok "a dev build's Debian version sorts below the release" ;;
  *) bad "dev version: $line" ;;
esac
if command -v dpkg >/dev/null; then
  if dpkg --compare-versions "${CARGO_V%%[-+]*}~dev.5" lt "${CARGO_V%%[-+]*}"; then ok "dpkg agrees"; else bad "dpkg orders ~dev above the release"; fi
fi

if [ "$(uname -s)" = Linux ]; then
  bash "$TMP/scripts/desktop-package.sh" --bin "$STANDIN" --only none --version 9.8.7 --stamp --out "$TMP/out" >/dev/null
  if [ "$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$TMP/clients/desktop/Cargo.toml" | head -1)" = 9.8.7 ]; then
    ok "--stamp: Cargo.toml"
  else bad "--stamp did not write Cargo.toml"; fi
  if grep -A1 '^name = "encore-desktop"$' "$TMP/clients/desktop/Cargo.lock" | grep -q '^version = "9.8.7"$'; then
    ok "--stamp: Cargo.lock"
  else bad "--stamp did not write Cargo.lock"; fi
  if [ "$(grep -c '^version = "9.8.7"' "$TMP/clients/desktop/Cargo.lock")" = 1 ]; then ok "--stamp: no other lock entry changed"
  else bad "--stamp changed more than encore-desktop in Cargo.lock"; fi
  if bash "$TMP/scripts/desktop-package.sh" --bin "$STANDIN" --only none --version dev.5 --stamp --out "$TMP/out" >/dev/null 2>&1; then
    bad "--stamp took a version that is not X.Y.Z"
  else ok "--stamp refuses dev.5"; fi
fi

if [ "$FAILED" != 0 ]; then echo "packaging checks failed"; exit 1; fi
echo "packaging checks passed"
