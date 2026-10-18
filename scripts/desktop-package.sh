#!/usr/bin/env bash
# Build and package the Encore desktop app (clients/desktop: Iced, pure Rust, Cadenza playing on
# this computer) for the OS this runs on:
#   Linux    Encore-<v>-linux-x86_64.AppImage (appimagetool) and encore_<v>_amd64.deb (dpkg-deb)
#   macOS    Encore-<v>-macos-universal.dmg: Encore.app, both Mac targets joined with lipo (--universal)
#   Windows  Encore-<v>-windows-x86_64.exe: the NSIS installer (clients/desktop/packaging/windows/encore.nsi),
#            per user under %LOCALAPPDATA%\Encore; run this from Git Bash
# The desktop job of .github/workflows/apps.yml runs it; it works the same on your own machine.
#
#   scripts/desktop-package.sh                          # release build named after Cargo.toml's version, into out/
#   scripts/desktop-package.sh --version 0.27.1         # name the files after this version
#   scripts/desktop-package.sh --version 0.27.1 --stamp # and build the app as 0.27.1: writes the version into
#                                                       # clients/desktop/Cargo.toml and Cargo.lock (CI does this)
#   --features cuda    cargo features for the build (CI: cuda on Windows and Linux)
#   --universal        macOS: build aarch64 and x86_64 and join them with lipo (CI does this)
#   --bin <file>       package this binary instead of building one
#   --only <kinds>     appimage,deb on Linux (both by default)
#   --out <dir>        where the files go (default: out/ at the top of the repository)
#   --dry-run          show what would run
#
# Signing, when these are set (the CI job sets them from the repository secrets):
#   macOS    MACOS_SIGN_IDENTITY: a Developer ID in the keychain (otherwise the app is signed ad hoc);
#            with APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD the .dmg is notarized and stapled
#   Windows  WINDOWS_CERT_PFX (the path of a .pfx file) and WINDOWS_CERT_PASSWORD: signtool signs the app,
#            then the installer
#
# Needs cargo, and: Linux – dpkg-deb (dpkg-shlibdeps from dpkg-dev for the Depends line), curl (appimagetool is
# fetched once into ~/.cache/encore-packaging unless APPIMAGETOOL names one) and the build libraries listed in
# clients/desktop/README.md; macOS – the Xcode command line tools; Windows – NSIS (makensis) and Git Bash.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CRATE="$ROOT/clients/desktop"
PKG="$CRATE/packaging"
NAME=encore-desktop

die() { echo "error: $*" >&2; exit 1; }
need_value() { [ "$2" -ge 2 ] || die "$1 needs a value (see --help)"; }

V=""; STAMP=0; FEATURES=""; UNIVERSAL=0; BIN=""; ONLY=""; OUT="$ROOT/out"; DRY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --version)   need_value "$1" $#; V="${2#v}"; shift 2 ;;
    --stamp)     STAMP=1; shift ;;
    --features)  need_value "$1" $#; FEATURES="$2"; shift 2 ;;
    --universal) UNIVERSAL=1; shift ;;
    --bin)       need_value "$1" $#; BIN="$2"; shift 2 ;;
    --only)      need_value "$1" $#; ONLY="$2"; shift 2 ;;
    --out)       need_value "$1" $#; OUT="$2"; shift 2 ;;
    --dry-run)   DRY=1; shift ;;
    -h|--help)   sed -n '2,29p' "$0"; exit 0 ;;
    *)           die "unknown option $1 (see --help)" ;;
  esac
done

run() { echo "+ $*"; [ "$DRY" = 1 ] || "$@"; }
# a command whose arguments hold a password: say what it does instead of echoing it
run_secret() { local what="$1"; shift; echo "+ $what"; [ "$DRY" = 1 ] || "$@"; }
want() { [ -z "$ONLY" ] || [[ ",$ONLY," == *",$1,"* ]]; }
is_semver() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; }

# ---- versions ------------------------------------------------------------------------------------
# V names the files; NUM (numbers only) goes where a package wants X.Y.Z (Info.plist, the installer's file
# version); PKGV is the .deb Version, which must start with a digit (dev.12 → 0.27.0~dev.12, below 0.27.0).
CARGO_V=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$CRATE/Cargo.toml" | head -1)
[ -n "$CARGO_V" ] || die "no version in $CRATE/Cargo.toml"
[ -n "$V" ] || V="$CARGO_V"
[[ "$V" =~ ^[A-Za-z0-9][A-Za-z0-9.+~-]*$ ]] || die "bad version $V"
if is_semver "$V"; then
  NUM="$V"; PKGV="$V"
else
  NUM="${CARGO_V%%[-+]*}"; PKGV="$NUM~$V"
fi

# ---- the version inside the app (api::VERSION is Cargo.toml's): --stamp writes V into the manifest and the
# lock, so `cargo build --locked` still passes and the in-app updater compares against the right number
stamp() {
  is_semver "$V" || die "--stamp needs a version like 0.27.1 (got $V)"
  [ "$V" != "$CARGO_V" ] || return 0
  echo "+ stamp version $V into clients/desktop/Cargo.toml and Cargo.lock (was $CARGO_V)"
  [ "$DRY" = 1 ] && return 0
  # the [package] version is the first line that starts with `version =`; the lock's is the line after
  # `name = "encore-desktop"`. A checkout with CRLF line ends keeps them.
  awk -v v="$V" '
    !done && /^version *= *"/ { cr = /\r$/ ? "\r" : ""; print "version = \"" v "\"" cr; done = 1; next }
    { print }' "$CRATE/Cargo.toml" > "$CRATE/Cargo.toml.stamp"
  mv "$CRATE/Cargo.toml.stamp" "$CRATE/Cargo.toml"
  awk -v v="$V" -v n="$NAME" '
    hit && /^version = "/ { cr = /\r$/ ? "\r" : ""; print "version = \"" v "\"" cr; hit = 0; next }
    { line = $0; sub(/\r$/, "", line); if (line == "name = \"" n "\"") hit = 1; print }' \
    "$CRATE/Cargo.lock" > "$CRATE/Cargo.lock.stamp"
  mv "$CRATE/Cargo.lock.stamp" "$CRATE/Cargo.lock"
  grep -q "^version = \"$V\"" "$CRATE/Cargo.toml" || die "the version was not written into Cargo.toml"
  CARGO_V="$V"
}

# ---- build ---------------------------------------------------------------------------------------
TARGET_DIR="${CARGO_TARGET_DIR:-$CRATE/target}"
cargo_build() { # [target triple]
  local args=(build --release --locked --manifest-path "$CRATE/Cargo.toml")
  [ -z "$FEATURES" ] || args+=(--features "$FEATURES")
  [ -z "${1:-}" ] || args+=(--target "$1")
  run cargo "${args[@]}"
}
built() { # [target triple] → the release binary's path
  local dir="$TARGET_DIR${1:+/$1}/release" exe=""
  [ "$OS" = windows ] && exe=.exe
  echo "$dir/$NAME$exe"
}

case "$(uname -s)" in
  Linux*)  OS=linux ;;
  Darwin*) OS=macos ;;
  MINGW*|MSYS*|CYGWIN*) OS=windows ;;
  *) die "unsupported system $(uname -s)" ;;
esac
case "$(uname -m)" in
  x86_64|amd64)  ARCH=x86_64; DEBARCH=amd64 ;;
  aarch64|arm64) ARCH=aarch64; DEBARCH=arm64 ;;
  *) die "unsupported machine $(uname -m)" ;;
esac

command -v cargo >/dev/null || [ -n "$BIN" ] || die "cargo not found: install Rust from https://rustup.rs"
[ "$STAMP" = 0 ] || stamp
echo "Encore desktop $V ($OS $ARCH; package version $PKGV${FEATURES:+; features $FEATURES})"
[ "$DRY" = 1 ] || mkdir -p "$OUT"
OUT="$(cd "$OUT" 2>/dev/null && pwd || echo "$OUT")"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ---- Linux ---------------------------------------------------------------------------------------
icons_into() { # <dir>: hicolor icons named encore-desktop.png
  local s
  for s in 32 64 128 256 512; do
    install -Dm644 "$PKG/icons/${s}x${s}.png" "$1/usr/share/icons/hicolor/${s}x${s}/apps/$NAME.png"
  done
}

appimagetool_path() {
  if [ -n "${APPIMAGETOOL:-}" ]; then echo "$APPIMAGETOOL"; return; fi
  if command -v appimagetool >/dev/null; then command -v appimagetool; return; fi
  local cache="${XDG_CACHE_HOME:-$HOME/.cache}/encore-packaging"
  local f="$cache/appimagetool-$ARCH.AppImage"
  local url="${APPIMAGETOOL_URL:-https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage}"
  if [ ! -x "$f" ]; then
    mkdir -p "$cache"
    echo "+ curl -fsSL -o $f $url" >&2
    curl -fsSL --retry 3 -o "$f.part" "$url" || die "could not download appimagetool from $url"
    chmod +x "$f.part" && mv "$f.part" "$f"
  fi
  echo "$f"
}

appimage() { # <binary>
  local dir="$WORK/Encore.AppDir" out="$OUT/Encore-$V-linux-$ARCH.AppImage" tool
  echo "== AppImage → $out"
  [ "$DRY" = 1 ] && return 0
  rm -rf "$dir"
  install -Dm755 "$1" "$dir/usr/bin/$NAME"
  install -Dm644 "$PKG/linux/$NAME.desktop" "$dir/usr/share/applications/$NAME.desktop"
  icons_into "$dir"
  cp "$PKG/linux/$NAME.desktop" "$dir/$NAME.desktop"
  cp "$PKG/icons/256x256.png" "$dir/$NAME.png"
  ln -s "$NAME.png" "$dir/.DirIcon"
  cat > "$dir/AppRun" <<'EOF'
#!/bin/sh
# The AppImage's entry point: Encore from the mounted image (the runtime sets $APPIMAGE, which the app's
# updater and "Start with the computer" use).
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/encore-desktop" "$@"
EOF
  chmod 755 "$dir/AppRun"
  chmod -R u+rwX,go+rX,go-w "$dir"   # readable by everyone whatever the umask
  tool="$(appimagetool_path)"
  # APPIMAGE_EXTRACT_AND_RUN: the tool runs without FUSE (CI runners and containers have none)
  rm -f "$out"
  run env ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$dir" "$out"
  [ -s "$out" ] || die "appimagetool made no $out"
  chmod 755 "$out"
}

deb_depends() { # <binary>: the shared libraries it links (dpkg-shlibdeps), plus what winit loads at run time
  local d deps=""
  if command -v dpkg-shlibdeps >/dev/null; then
    d="$(mktemp -d)"
    mkdir -p "$d/debian"
    printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$NAME" "$NAME" > "$d/debian/control"
    deps=$(cd "$d" && dpkg-shlibdeps -O --ignore-missing-info -e"$1" 2>/dev/null | sed -n 's/^shlibs:Depends=//p') || deps=""
    rm -rf "$d"
  fi
  [ -n "$deps" ] || deps="libc6 (>= 2.35), libgcc-s1, libasound2"
  # the keyboard libraries are opened with dlopen, so dpkg-shlibdeps cannot see them; without them the window
  # does not open
  echo "$deps, libxkbcommon0, libxkbcommon-x11-0"
}

deb() { # <binary>
  local root="$WORK/deb" out="$OUT/encore_${V}_${DEBARCH}.deb" size
  echo "== .deb → $out"
  [ "$DRY" = 1 ] && return 0
  command -v dpkg-deb >/dev/null || die "dpkg-deb not found (Debian/Ubuntu: it comes with dpkg)"
  rm -rf "$root"
  install -Dm755 "$1" "$root/usr/bin/$NAME"
  install -Dm644 "$PKG/linux/$NAME.desktop" "$root/usr/share/applications/$NAME.desktop"
  icons_into "$root"
  mkdir -p "$root/usr/share/doc/$NAME" "$root/DEBIAN"
  cat > "$root/usr/share/doc/$NAME/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Encore
Source: https://github.com/Krifmaz/Encore

Files: *
Copyright: Encore contributors
License: MIT
EOF
  chmod -R u+rwX,go+rX,go-w "$root"
  size=$(du -sk "$root/usr" | cut -f1)
  cat > "$root/DEBIAN/control" <<EOF
Package: $NAME
Version: $PKGV
Architecture: $DEBARCH
Maintainer: Encore <Krifmaz@users.noreply.github.com>
Installed-Size: $size
Depends: $(deb_depends "$root/usr/bin/$NAME")
Recommends: libvulkan1, libegl1
Section: sound
Priority: optional
Homepage: https://github.com/Krifmaz/Encore
Description: Encore desktop app with Cadenza playing on this computer
 Your Encore music server in a native window: the same pages as the web app,
 the player with lyrics and queue, other devices, offline songs, tray and
 media keys. Every song plays through the Cadenza engine built into the app.
EOF
  # xz, not the zstd of newer dpkg: every Debian and Ubuntu still in use reads it
  run dpkg-deb --root-owner-group -Zxz --build "$root" "$out"
  dpkg-deb --info "$out" | sed -n '/Package:/,/Description:/p'
}

linux() {
  local bin="$BIN"
  if [ -z "$bin" ]; then cargo_build; bin="$(built)"; fi
  [ "$DRY" = 1 ] || [ -x "$bin" ] || die "no binary at $bin"
  if want appimage; then appimage "$bin"; fi
  if want deb; then deb "$bin"; fi
}

# ---- macOS ---------------------------------------------------------------------------------------
macos() {
  local bin="$BIN" arch app="$WORK/Encore.app" dmg stage="$WORK/dmg"
  export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-10.15}"
  if [ -n "$bin" ]; then
    arch=$(lipo -archs "$bin" 2>/dev/null | grep -q ' ' && echo universal || echo "$ARCH")
  elif [ "$UNIVERSAL" = 1 ]; then
    if command -v rustup >/dev/null; then run rustup target add aarch64-apple-darwin x86_64-apple-darwin; fi
    cargo_build aarch64-apple-darwin
    cargo_build x86_64-apple-darwin
    bin="$WORK/$NAME"
    run lipo -create -output "$bin" "$(built aarch64-apple-darwin)" "$(built x86_64-apple-darwin)"
    arch=universal
  else
    cargo_build
    bin="$(built)"
    arch="$ARCH"
  fi
  dmg="$OUT/Encore-$V-macos-$arch.dmg"
  echo "== Encore.app → $dmg"
  [ "$DRY" = 1 ] && return 0
  [ -x "$bin" ] || die "no binary at $bin"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$bin" "$app/Contents/MacOS/$NAME"
  chmod 755 "$app/Contents/MacOS/$NAME"
  sed -e "s/@VERSION@/$NUM/g" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/g" "$PKG/macos/Info.plist" > "$app/Contents/Info.plist"
  printf 'APPL????' > "$app/Contents/PkgInfo"
  cp "$PKG/icons/encore.icns" "$app/Contents/Resources/encore.icns"
  plutil -lint "$app/Contents/Info.plist"
  if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    run codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$app"
  else
    echo "No MACOS_SIGN_IDENTITY: signed ad hoc (the first start needs System Settings → Privacy & Security → Open Anyway)"
    run codesign --force --sign - "$app"
  fi
  run codesign --verify --strict --verbose=2 "$app"
  mkdir -p "$stage"
  cp -R "$app" "$stage/"
  ln -s /Applications "$stage/Applications"
  rm -f "$dmg"
  run hdiutil create -volname "Encore" -srcfolder "$stage" -ov -format UDZO "$dmg"
  if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    run codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$dmg"
    if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
      run_secret "xcrun notarytool submit $dmg --wait (Apple ID $APPLE_ID, team $APPLE_TEAM_ID)" \
        xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
      run xcrun stapler staple "$dmg"
    fi
  fi
}

# ---- Windows -------------------------------------------------------------------------------------
winpath() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }

find_tool() { # <name> <glob>…: on the PATH, else the newest match of the globs
  local name="$1" f found=""; shift
  if command -v "$name" >/dev/null; then command -v "$name"; return; fi
  for f in "$@"; do [ -f "$f" ] && found="$f"; done
  [ -n "$found" ] || return 1
  echo "$found"
}

win_sign() { # <file>
  [ -n "${WINDOWS_CERT_PFX:-}" ] || return 0
  local signtool
  signtool=$(find_tool signtool.exe "/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/signtool.exe) \
    || die "WINDOWS_CERT_PFX is set but signtool.exe was not found (Windows SDK)"
  # MSYS2_ARG_CONV_EXCL: Git Bash must not turn signtool's /switches into paths
  MSYS2_ARG_CONV_EXCL='*' run_secret "signtool sign $1" \
    "$signtool" sign /f "$(winpath "$WINDOWS_CERT_PFX")" /p "${WINDOWS_CERT_PASSWORD:-}" /fd SHA256 \
      /tr http://timestamp.digicert.com /td SHA256 "$(winpath "$1")"
}

windows() {
  local bin="$BIN" exe="$WORK/$NAME.exe" out="$OUT/Encore-$V-windows-$ARCH.exe" makensis
  if [ -z "$bin" ]; then cargo_build; bin="$(built)"; fi
  echo "== installer → $out"
  [ "$DRY" = 1 ] && return 0
  [ -f "$bin" ] || die "no binary at $bin"
  makensis=$(find_tool makensis "/c/Program Files (x86)/NSIS/makensis.exe" "/c/Program Files/NSIS/makensis.exe") \
    || die "makensis not found: install NSIS (https://nsis.sourceforge.io, or: choco install nsis)"
  cp "$bin" "$exe"
  win_sign "$exe"
  rm -f "$out"
  MSYS2_ARG_CONV_EXCL='*' run "$makensis" -V2 "-DVERSION=$V" "-DNUMVER=$NUM" \
    "-DAPP_EXE=$(winpath "$exe")" "-DICON=$(winpath "$PKG/icons/encore.ico")" "-DOUTFILE=$(winpath "$out")" \
    "$(winpath "$PKG/windows/encore.nsi")"
  [ -s "$out" ] || die "makensis made no $out"
  win_sign "$out"
}

"$OS"
echo "== made"
ls -la "$OUT" 2>/dev/null || true
