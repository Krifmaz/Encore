#!/usr/bin/env bash
# Build the desktop installers and/or the Android APK on this machine and attach them to a GitHub
# release – the same files the "apps" job of the release workflow would make, without its runner
# minutes. Each OS builds its own installers: the desktop app (clients/desktop, through
# scripts/desktop-package.sh: Windows → .exe, Linux → .AppImage/.deb, macOS → .dmg). The Android APK is the
# native app (clients/android), built with Gradle anywhere the SDK + NDK are installed, as apps.yml's
# android-native job builds it. SHA256SUMS is rewritten for the files replaced.
#
#   scripts/release-apps.sh                      # version of the newest release, desktop + Android, upload
#   scripts/release-apps.sh 0.20.3               # name the files after this version instead
#   scripts/release-apps.sh --android            # only the APK; --desktop for only the installer
#   scripts/release-apps.sh --no-upload          # just build into out/
#   scripts/release-apps.sh --dry-run            # show what would run
#
# Needs: rustup/cargo, the gh CLI signed in (gh auth login). Android also needs Java 21, the Android SDK with
# an NDK (ANDROID_HOME or ANDROID_SDK_ROOT set; Android Studio sets them), cargo-ndk (Cadenza in the APK) and
# Gradle: clients/android/gradlew when you generated it, otherwise `gradle` at the version
# clients/android/gradle/wrapper/gradle-wrapper.properties names (clients/android/README.md, Building). An APK
# that is uploaded must be signed with the release key, from the same variables apps.yml sets:
# ANDROID_KEYSTORE_PATH (or ANDROID_KEYSTORE_BASE64), ANDROID_KEYSTORE_PASSWORD, ANDROID_KEY_ALIAS and
# ANDROID_KEY_PASSWORD; with --no-upload it may fall back to the debug key.
# Windows: run from Git Bash, with NSIS (makensis). Linux: libasound2-dev libxkbcommon-dev dpkg-dev curl.
# macOS: Xcode command line tools.
set -euo pipefail
cd "$(dirname "$0")/.."

DESKTOP=1; ANDROID=1; UPLOAD=1; DRY=0; V=""
for a in "$@"; do
  case "$a" in
    --desktop) ANDROID=0 ;;
    --android) DESKTOP=0 ;;
    --no-upload) UPLOAD=0 ;;
    --dry-run) DRY=1 ;;
    -h|--help) sed -n '2,23p' "$0"; exit 0 ;;
    -*) echo "unknown option $a"; exit 1 ;;
    *) V="${a#v}" ;;
  esac
done
run() { echo "+ $*"; [ "$DRY" = 1 ] || "$@"; }
# what to undo when the script ends, however it ends (each entry is one shell command)
CLEANUP=()
cleanup() { local c; for c in ${CLEANUP[@]+"${CLEANUP[@]}"}; do eval "$c"; done; }
trap cleanup EXIT

# ---- version: given, or the newest release's (tag v0.20.3)
if [ -z "$V" ]; then
  V=$(gh release view --json tagName -q .tagName 2>/dev/null | sed 's/^v//') || true
  [ -n "$V" ] || { echo "No release found – pass the version, e.g. $0 0.20.3"; exit 1; }
fi
echo "$V" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { echo "bad version $V (want MAJOR.MINOR.PATCH)"; exit 1; }
IFS=. read -r A B C <<< "$V"
CODE=$(( 260000000 + A * 1000000 + B * 10000 + C * 10 ))      # Android versionCode
echo "Version $V (Android code $CODE)"

# ---- tools ---------------------------------------------------------------------------------------
command -v cargo >/dev/null || { echo "cargo not found – install Rust from https://rustup.rs"; exit 1; }
if [ "$UPLOAD" = 1 ]; then
  command -v gh >/dev/null || { echo "gh not found – install the GitHub CLI and run: gh auth login"; exit 1; }
  gh auth status >/dev/null 2>&1 || { echo "gh is not signed in – run: gh auth login"; exit 1; }
fi
case "$(uname -s)" in
  Linux*)  OS=linux; PKG_ARGS=(--features cuda) ;;
  Darwin*) OS=mac; PKG_ARGS=(--universal)
           [ "$DESKTOP" = 0 ] || run rustup target add aarch64-apple-darwin x86_64-apple-darwin ;;
  *)       OS=windows; PKG_ARGS=(--features cuda) ;;
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
    CLEANUP+=('for f in Cargo.toml Cargo.lock; do cmp -s "$KEEP/$f" "clients/desktop/$f" || cp "$KEEP/$f" "clients/desktop/$f"; done; rm -rf "$KEEP"')
  fi
  run bash scripts/desktop-package.sh --version "$V" --stamp "${PKG_ARGS[@]}" --out out
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
  export ANDROID_HOME="$SDK"
  [ -n "${ANDROID_NDK_HOME:-}" ] || export ANDROID_NDK_HOME="$NDK_HOME"
  command -v java >/dev/null || { echo "java not found – install a JDK 21"; exit 1; }
  # Cadenza (libencore_cadenza.so) is built by the Gradle task :cadenza:cargoNdkBuild; without cargo-ndk that
  # task is skipped and the APK would ship without its playback engine
  command -v cargo-ndk >/dev/null || { echo "cargo-ndk not found – cargo install cargo-ndk --version 4.1.2 --locked"; exit 1; }
  run rustup target add aarch64-linux-android armv7-linux-androideabi
  # Gradle: the wrapper when it was generated locally, else the installed one (no wrapper jar is committed)
  if [ -x clients/android/gradlew ]; then GRADLE=(clients/android/gradlew -p clients/android)
  else
    command -v gradle >/dev/null || { echo "gradle not found – install the Gradle clients/android/gradle/wrapper/gradle-wrapper.properties names, or generate the wrapper (clients/android/README.md)"; exit 1; }
    GRADLE=(gradle -p clients/android)
  fi
  # signing: the same variables as apps.yml's android-native job (a base64 keystore is written to a temp file)
  if [ -z "${ANDROID_KEYSTORE_PATH:-}" ] && [ -n "${ANDROID_KEYSTORE_BASE64:-}" ]; then
    ANDROID_KEYSTORE_PATH="$(mktemp)"; export ANDROID_KEYSTORE_PATH
    CLEANUP+=('rm -f "$ANDROID_KEYSTORE_PATH"')
    printf '%s' "$ANDROID_KEYSTORE_BASE64" | base64 -d > "$ANDROID_KEYSTORE_PATH"
  fi
  if [ -n "${ANDROID_KEYSTORE_PATH:-}" ] && [ -n "${ANDROID_KEYSTORE_PASSWORD:-}" ] && [ -n "${ANDROID_KEY_ALIAS:-}" ] && [ -n "${ANDROID_KEY_PASSWORD:-}" ]; then
    echo "Release key: $ANDROID_KEYSTORE_PATH (alias $ANDROID_KEY_ALIAS)"
  elif [ "$UPLOAD" = 1 ]; then
    echo "A released APK must be signed with the release key (phones can't update otherwise): set ANDROID_KEYSTORE_PATH or"
    echo "ANDROID_KEYSTORE_BASE64, ANDROID_KEYSTORE_PASSWORD, ANDROID_KEY_ALIAS and ANDROID_KEY_PASSWORD, or build with --no-upload"
    exit 1
  else
    echo "note: no release key set – the APK is signed with the debug key (it can't update a release build)"
    # the release build type falls back to the debug key; make sure one exists where Gradle looks
    export ANDROID_USER_HOME="${ANDROID_USER_HOME:-$HOME/.android}"
    if [ ! -f "$ANDROID_USER_HOME/debug.keystore" ]; then
      run mkdir -p "$ANDROID_USER_HOME"
      run keytool -genkeypair -v -keystore "$ANDROID_USER_HOME/debug.keystore" -storepass android -alias androiddebugkey -keypass android \
        -dname "CN=Android Debug,O=Android,C=US" -keyalg RSA -keysize 2048 -validity 10000
    fi
  fi
  run "${GRADLE[@]}" --no-daemon :app:assembleRelease "-PencoreVersionName=$V" "-PencoreVersionCode=$CODE"
  APK=$(find clients/android/app/build/outputs/apk/release -name '*.apk' ! -name '*unsigned*' 2>/dev/null | head -1 || true)
  if [ -n "$APK" ]; then cp "$APK" "out/Encore-android-$V.apk"; elif [ "$DRY" = 0 ]; then echo "no signed APK was built"; exit 1; fi
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
  echo "$ASSETS" | grep -qx SHA256SUMS && LINE="$LINE checksums in \`SHA256SUMS\` ·"
  LINE="${LINE% ·}"
  LINE=$(printf '%s' "$LINE" | sed 's/[&|\\]/\\&/g')   # "Privacy & Security": & means the match to sed
  printf '%s\n' "$BODY" | sed "s|^\* \*\*Apps\*\*: the desktop and Android apps were not rebuilt for this release.*|$LINE|" > out/.notes.md
  run gh release edit "$TAG" --notes-file out/.notes.md
fi
echo "Done: https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/$TAG"
