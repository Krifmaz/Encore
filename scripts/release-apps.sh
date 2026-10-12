#!/usr/bin/env bash
# Build the desktop installer and/or the Android APK on this machine and attach them to a GitHub
# release – the same files the "apps" job of the release workflow would make, without its runner
# minutes. Each OS builds its own installer (Windows → .exe, Linux → .AppImage/.deb/.rpm,
# macOS → .dmg); Android builds anywhere the SDK + NDK are installed.
#
#   scripts/release-apps.sh                      # newest release on GitHub, desktop + Android, upload
#   scripts/release-apps.sh 2026.10.12.1-edge    # a specific version (the release is created if missing)
#   scripts/release-apps.sh --android            # only the APK; --desktop for only the installer
#   scripts/release-apps.sh --no-upload          # just build into out/
#   scripts/release-apps.sh --dry-run            # show what would run
#
# Needs: rustup/cargo, the gh CLI signed in (gh auth login). Android also needs Java 17+ and the
# Android SDK with an NDK (ANDROID_HOME or ANDROID_SDK_ROOT set; Android Studio sets them).
# Windows: run from Git Bash. Linux: libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev
# librsvg2-dev patchelf. macOS: Xcode command line tools.
set -euo pipefail
cd "$(dirname "$0")/.."

DESKTOP=1; ANDROID=1; UPLOAD=1; DRY=0; V=""
for a in "$@"; do
  case "$a" in
    --desktop) ANDROID=0 ;;
    --android) DESKTOP=0 ;;
    --no-upload) UPLOAD=0 ;;
    --dry-run) DRY=1 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    -*) echo "unknown option $a"; exit 1 ;;
    *) V="${a#v}" ;;
  esac
done
run() { echo "+ $*"; [ "$DRY" = 1 ] || "$@"; }

# ---- version: given, or the newest release on GitHub -------------------------------------------
if [ -z "$V" ]; then
  V=$(gh release list --limit 1 --json tagName -q '.[0].tagName' 2>/dev/null | sed 's/^v//') || true
  [ -n "$V" ] || { echo "No release found – pass the version, e.g. $0 2026.10.12.1-edge"; exit 1; }
fi
echo "$V" | grep -Eq '^[0-9]{4}\.[0-9]{2}\.[0-9]{2}\.[0-9]+(-[a-z0-9.]+)?$' || { echo "bad version $V (want YYYY.MM.DD.N[-edge])"; exit 1; }
BASE="${V%%-*}"; SUFFIX="${V#"$BASE"}"
IFS=. read -r Y M D N <<< "$BASE"
SEMVER="$Y.$((10#$M)).$((10#$D * 100 + N))$SUFFIX"                          # same rule as the release workflow
CODE=$(printf '%d%02d%02d%02d' $((Y - 2000)) $((10#$M)) $((10#$D)) $N)      # Android versionCode
PRE=$([ -n "$SUFFIX" ] && echo 1 || echo 0)
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
  Linux*)  OS=linux; BUNDLES=deb,appimage,rpm; ARGS=""
           pkg-config --exists webkit2gtk-4.1 2>/dev/null || echo "note: webkit2gtk-4.1 not found – on Debian/Ubuntu: sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf" ;;
  Darwin*) OS=mac; BUNDLES=dmg; ARGS="--target universal-apple-darwin"
           run rustup target add aarch64-apple-darwin x86_64-apple-darwin ;;
  *)       OS=windows; BUNDLES=nsis; ARGS="" ;;
esac

mkdir -p out
export ENCORE_VERSION="$V"

# ---- desktop -------------------------------------------------------------------------------------
if [ "$DESKTOP" = 1 ]; then
  echo "== desktop ($OS)"
  # shellcheck disable=SC2086
  (cd clients/app && run cargo tauri build --ci --bundles "$BUNDLES" $ARGS --config "{\"version\":\"$SEMVER\"}")
  { find clients/app/target -path '*/release/bundle/*' -type f \( -name '*.AppImage' -o -name '*.deb' -o -name '*.rpm' -o -name '*-setup.exe' -o -name '*.dmg' \) -print 2>/dev/null || true; } | while read -r f; do
    case "$f" in
      *.AppImage) cp "$f" "out/Encore-$V-linux-x86_64.AppImage" ;;
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
  # the Gradle release config falls back to the debug key; make sure one exists
  if [ -z "${ANDROID_KEYSTORE_FILE:-}" ] && [ ! -f "$HOME/.android/debug.keystore" ]; then
    mkdir -p "$HOME/.android"
    run keytool -genkeypair -v -keystore "$HOME/.android/debug.keystore" -storepass android -alias androiddebugkey -keypass android \
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

# ---- attach to the release -----------------------------------------------------------------------
TAG="v$V"
if ! gh release view "$TAG" >/dev/null 2>&1; then
  NOTES=(--generate-notes); [ -f "docs/releases/$V.md" ] && NOTES=(--notes-file "docs/releases/$V.md")
  FLAGS=(); [ "$PRE" = 1 ] && FLAGS=(--prerelease)
  run gh release create "$TAG" --title "Encore $V" "${NOTES[@]}" "${FLAGS[@]}"
fi
FILES=(out/*"$V"*)
[ -e "${FILES[0]}" ] || { echo "nothing to upload"; exit 1; }
run gh release upload "$TAG" "${FILES[@]}" --clobber

# the workflow's notes say the apps were not rebuilt when it ran without them – correct that line
[ "$DRY" = 1 ] && exit 0
BODY=$(gh release view "$TAG" --json body -q .body)
if echo "$BODY" | grep -q 'were not rebuilt for this release'; then
  ASSETS=$(gh release view "$TAG" --json assets -q '.assets[].name')
  LINE="* **Apps**:"
  echo "$ASSETS" | grep -q "^Encore-android-" && LINE="$LINE **Android**: \`Encore-android-$V.apk\` ·"
  echo "$ASSETS" | grep -q "^Encore-Setup-" && LINE="$LINE **Windows**: \`Encore-Setup-$V.exe\` ·"
  echo "$ASSETS" | grep -q -- "-mac.dmg$" && LINE="$LINE **macOS**: the .dmg (unsigned: right-click → Open the first time) ·"
  echo "$ASSETS" | grep -q -- "-linux-" && LINE="$LINE **Linux desktop**: .AppImage or .deb ·"
  LINE="${LINE% ·}"
  printf '%s\n' "$BODY" | sed "s|^\* \*\*Apps\*\*: the desktop and Android apps were not rebuilt for this release.*|$LINE|" > out/.notes.md
  run gh release edit "$TAG" --notes-file out/.notes.md
fi
echo "Done: https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/$TAG"
