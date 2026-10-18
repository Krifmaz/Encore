#!/usr/bin/env bash
# Build the desktop installers and/or the Android APK on this machine and attach them to a GitHub
# release – the same files the "apps" job of the release workflow would make, without its runner
# minutes. Each OS builds its own installers: the desktop app (clients/desktop, through
# scripts/desktop-package.sh: Windows → .exe, Linux → .AppImage/.deb, macOS → .dmg) and, for the 0.27
# line only, the classic Tauri app (clients/app) under its old names, its AppImage as Encore-classic-….
# Android builds anywhere the SDK + NDK are installed. SHA256SUMS is rewritten for the files replaced.
#
#   scripts/release-apps.sh                      # version of the newest release, desktop + Android, upload
#   scripts/release-apps.sh 0.20.3               # name the files after this version instead
#   scripts/release-apps.sh --android            # only the APK; --desktop for only the installer
#   scripts/release-apps.sh --no-upload          # just build into out/
#   scripts/release-apps.sh --dry-run            # show what would run
#
# Needs: rustup/cargo, the gh CLI signed in (gh auth login). Android also needs Java 17+ and the
# Android SDK with an NDK (ANDROID_HOME or ANDROID_SDK_ROOT set; Android Studio sets them).
# Windows: run from Git Bash, with NSIS (makensis). Linux: libasound2-dev libxkbcommon-dev dpkg-dev curl for
# the desktop app, and libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf
# for the classic one. macOS: Xcode command line tools.
set -euo pipefail
cd "$(dirname "$0")/.."

DESKTOP=1; ANDROID=1; UPLOAD=1; DRY=0; V=""
for a in "$@"; do
  case "$a" in
    --desktop) ANDROID=0 ;;
    --android) DESKTOP=0 ;;
    --no-upload) UPLOAD=0 ;;
    --dry-run) DRY=1 ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    -*) echo "unknown option $a"; exit 1 ;;
    *) V="${a#v}" ;;
  esac
done
run() { echo "+ $*"; [ "$DRY" = 1 ] || "$@"; }

# ---- version: given, or the newest release's (tag v0.20.3)
if [ -z "$V" ]; then
  V=$(gh release view --json tagName -q .tagName 2>/dev/null | sed 's/^v//') || true
  [ -n "$V" ] || { echo "No release found – pass the version, e.g. $0 0.20.3"; exit 1; }
fi
echo "$V" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { echo "bad version $V (want MAJOR.MINOR.PATCH)"; exit 1; }
IFS=. read -r A B C <<< "$V"
SEMVER="$V"                                                   # same rule as the release workflow
CODE=$(( 260000000 + A * 1000000 + B * 10000 + C * 10 ))      # Android versionCode
echo "Version $V (app version $SEMVER, Android code $CODE)"

# ---- tools ---------------------------------------------------------------------------------------
command -v cargo >/dev/null || { echo "cargo not found – install Rust from https://rustup.rs"; exit 1; }
if [ "$UPLOAD" = 1 ]; then
  command -v gh >/dev/null || { echo "gh not found – install the GitHub CLI and run: gh auth login"; exit 1; }
  gh auth status >/dev/null 2>&1 || { echo "gh is not signed in – run: gh auth login"; exit 1; }
fi
if ! cargo tauri --version 2>/dev/null | grep -q '^tauri-cli 2\.'; then
  run cargo install tauri-cli --version 2.12.2 --locked
fi
case "$(uname -s)" in
  Linux*)  OS=linux; BUNDLES=deb,appimage,rpm; ARGS=""; PKG_ARGS=(--features cuda)
           pkg-config --exists webkit2gtk-4.1 2>/dev/null || echo "note: webkit2gtk-4.1 not found – on Debian/Ubuntu: sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf" ;;
  Darwin*) OS=mac; BUNDLES=app; ARGS="--target universal-apple-darwin"; PKG_ARGS=(--universal)
           run rustup target add aarch64-apple-darwin x86_64-apple-darwin ;;
  *)       OS=windows; BUNDLES=nsis; ARGS=""; PKG_ARGS=(--features cuda) ;;
esac

mkdir -p out
export ENCORE_VERSION="$V"

# ---- desktop -------------------------------------------------------------------------------------
if [ "$DESKTOP" = 1 ]; then
  echo "== desktop app ($OS)"
  # the files the desktop job of apps.yml makes, built as $V: --stamp writes $V into clients/desktop/Cargo.toml
  # and Cargo.lock, which are put back afterwards (as in CI, where the checkout is thrown away)
  if [ "$DRY" = 0 ]; then
    KEEP=$(mktemp -d)
    cp clients/desktop/Cargo.toml clients/desktop/Cargo.lock "$KEEP/"
    trap 'for f in Cargo.toml Cargo.lock; do cmp -s "$KEEP/$f" "clients/desktop/$f" || cp "$KEEP/$f" "clients/desktop/$f"; done; rm -rf "$KEEP"' EXIT
  fi
  run bash scripts/desktop-package.sh --version "$V" --stamp "${PKG_ARGS[@]}" --out out
  echo "== classic desktop app ($OS)"
  # shellcheck disable=SC2086
  (cd clients/app && run cargo tauri build --ci --bundles "$BUNDLES" $ARGS --config "{\"version\":\"$SEMVER\"}")
  if [ "$OS" = mac ]; then
    # a plain hdiutil image of the .app (Tauri's own dmg step needs a Finder session)
    APP=$(find clients/app/target -path '*/release/bundle/macos/*.app' -maxdepth 6 -type d | head -1)
    STAGE=$(mktemp -d); cp -R "$APP" "$STAGE/"; ln -s /Applications "$STAGE/Applications"
    mkdir -p "$(dirname "$APP")/../dmg"
    run hdiutil create -volname "Encore" -srcfolder "$STAGE" -ov -format UDZO "$(dirname "$APP")/../dmg/Encore_${SEMVER}_universal.dmg"
  fi
  { find clients/app/target -path '*/release/bundle/*' -type f \( -name '*.AppImage' -o -name '*.deb' -o -name '*.rpm' -o -name '*-setup.exe' -o -name '*.dmg' \) -print 2>/dev/null || true; } | while read -r f; do
    case "$f" in
      *.AppImage) cp "$f" "out/Encore-classic-$V-linux-x86_64.AppImage" ;;   # the plain name is the new app's
      *.deb)      cp "$f" "out/Encore-$V-linux-amd64.deb" ;;
      *.rpm)      cp "$f" "out/Encore-$V-linux-x86_64.rpm" ;;
      *.exe)      cp "$f" "out/Encore-Setup-$V.exe" ;;
      *.dmg)      cp "$f" "out/Encore-$V-mac.dmg" ;;
    esac
  done
fi

# ---- android -------------------------------------------------------------------------------------
if [ "$ANDROID" = 1 ]; then
  echo "== android"
  SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
  [ -d "$SDK" ] || { echo "Android SDK not found – set ANDROID_HOME (Android Studio → SDK Manager; include an NDK)"; exit 1; }
  if [ -z "${NDK_HOME:-}" ]; then
    NDK_HOME=$(ls -d "$SDK"/ndk/* 2>/dev/null | sort -V | tail -1 || true)
    [ -d "$NDK_HOME" ] || { echo "No NDK under $SDK/ndk – install one in the SDK Manager (SDK Tools → NDK)"; exit 1; }
    export NDK_HOME
  fi
  command -v java >/dev/null || { echo "java not found – install a JDK 17 or newer"; exit 1; }
  run rustup target add aarch64-linux-android armv7-linux-androideabi
  # the Gradle release config falls back to the debug key; make sure one exists where Gradle looks
  export ANDROID_USER_HOME="${ANDROID_USER_HOME:-$HOME/.android}"
  if [ -z "${ANDROID_KEYSTORE_FILE:-}" ] && [ ! -f "$ANDROID_USER_HOME/debug.keystore" ]; then
    mkdir -p "$ANDROID_USER_HOME"
    run keytool -genkeypair -v -keystore "$ANDROID_USER_HOME/debug.keystore" -storepass android -alias androiddebugkey -keypass android \
      -dname "CN=Android Debug,O=Android,C=US" -keyalg RSA -keysize 2048 -validity 10000
  fi
  (cd clients/app && run cargo tauri android build --ci --apk --target aarch64 --target armv7 \
    --config "{\"version\":\"$SEMVER\",\"bundle\":{\"android\":{\"versionCode\":$CODE}}}")
  APK=$(find clients/app/gen/android/app/build/outputs/apk -name '*release*.apk' 2>/dev/null | head -1 || true)
  if [ -n "$APK" ]; then cp "$APK" "out/Encore-android-$V.apk"; elif [ "$DRY" = 0 ]; then echo "no APK was built"; exit 1; fi
fi

echo "== built"
ls -la out 2>/dev/null || true
[ "$UPLOAD" = 1 ] || exit 0

# ---- attach to the release of this version ------------------------------------------------------
TAG="v$V"
if ! gh release view "$TAG" >/dev/null 2>&1; then
  NOTES=(--generate-notes); [ -f "docs/releases/$V.md" ] && NOTES=(--notes-file "docs/releases/$V.md")
  run gh release create "$TAG" --title "Encore $V" "${NOTES[@]}" --latest
fi
FILES=(out/*"$V"*)
[ -e "${FILES[0]}" ] || { echo "nothing to upload"; exit 1; }
# older installers of other versions go: the release only ever carries one set
for old in $(gh release view "$TAG" --json assets -q '.assets[].name' | grep -E '^(Encore-(Setup-|android-|classic-|[0-9])|encore_)' | grep -v -- "$V" || true); do
  run gh release delete-asset "$TAG" "$old" --yes
done
run gh release upload "$TAG" "${FILES[@]}" --clobber

# SHA256SUMS covers every file of the release, and the desktop app installs an update only when its file is listed
# there with the same hash: write it again. Files uploaded now are hashed here, the others keep their line (a file
# without one is downloaded and hashed).
if [ "$DRY" = 0 ]; then
  sum256() { if command -v sha256sum >/dev/null; then sha256sum -- "$1"; else shasum -a 256 -- "$1"; fi | cut -d' ' -f1; }
  SUMS=$(mktemp -d)
  gh release download "$TAG" --dir "$SUMS" --pattern SHA256SUMS 2>/dev/null || : > "$SUMS/SHA256SUMS"
  mv "$SUMS/SHA256SUMS" "$SUMS/old"
  for n in $(gh release view "$TAG" --json assets -q '.assets[].name' | grep -vx SHA256SUMS); do
    if [ -f "out/$n" ]; then h=$(sum256 "out/$n")
    else
      h=$(awk -v n="$n" '{ f = $2; sub(/^\*/, "", f) } f == n { print $1; exit }' "$SUMS/old")
      if [ -z "$h" ]; then gh release download "$TAG" --dir "$SUMS" --pattern "$n" && h=$(sum256 "$SUMS/$n") && rm -f "$SUMS/$n"; fi
    fi
    [ -n "$h" ] || { echo "could not hash $n for SHA256SUMS" >&2; exit 1; }
    printf '%s  %s\n' "$h" "$n"
  done | sort -k2 > "$SUMS/SHA256SUMS"
  cat "$SUMS/SHA256SUMS"
  run gh release upload "$TAG" "$SUMS/SHA256SUMS" --clobber
  rm -rf "$SUMS"
fi

# the workflow's notes say the apps were not rebuilt when it ran without them – correct that line
[ "$DRY" = 1 ] && exit 0
BODY=$(gh release view "$TAG" --json body -q .body)
if echo "$BODY" | grep -q 'were not rebuilt for this release'; then
  ASSETS=$(gh release view "$TAG" --json assets -q '.assets[].name')
  has() { echo "$ASSETS" | grep -qx -- "$1"; }
  LINE="* **Apps**:"
  has "Encore-android-$V.apk" && LINE="$LINE **Android**: \`Encore-android-$V.apk\` ·"
  has "Encore-$V-windows-x86_64.exe" && LINE="$LINE **Desktop app, Windows**: \`Encore-$V-windows-x86_64.exe\` ·"
  has "Encore-$V-macos-universal.dmg" && LINE="$LINE **Desktop app, macOS**: \`Encore-$V-macos-universal.dmg\` (if macOS will not open it the first time: System Settings → Privacy & Security → Open Anyway) ·"
  has "Encore-$V-linux-x86_64.AppImage" && LINE="$LINE **Desktop app, Linux**: \`Encore-$V-linux-x86_64.AppImage\` or \`encore_${V}_amd64.deb\` ·"
  has "Encore-Setup-$V.exe" && LINE="$LINE **Classic desktop app** (last release), Windows: \`Encore-Setup-$V.exe\` ·"
  has "Encore-$V-mac.dmg" && LINE="$LINE **Classic desktop app**, macOS: \`Encore-$V-mac.dmg\` (unsigned: right-click → Open the first time) ·"
  has "Encore-classic-$V-linux-x86_64.AppImage" && LINE="$LINE **Classic desktop app**, Linux: \`Encore-classic-$V-linux-x86_64.AppImage\`, \`Encore-$V-linux-amd64.deb\` or .rpm ·"
  echo "$ASSETS" | grep -qx SHA256SUMS && LINE="$LINE checksums in \`SHA256SUMS\` ·"
  LINE="${LINE% ·}"
  LINE=$(printf '%s' "$LINE" | sed 's/[&|\\]/\\&/g')   # "Privacy & Security": & means the match to sed
  printf '%s\n' "$BODY" | sed "s|^\* \*\*Apps\*\*: the desktop and Android apps were not rebuilt for this release.*|$LINE|" > out/.notes.md
  run gh release edit "$TAG" --notes-file out/.notes.md
fi
echo "Done: https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/$TAG"
