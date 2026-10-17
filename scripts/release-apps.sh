#!/usr/bin/env bash
# Build the desktop app, its Omnistall installer and/or the Android APK on this machine and attach them to a
# GitHub release – the same files the "apps" job of the release workflow would make, without its runner
# minutes. Each OS builds its own app (Windows → .exe, Linux → .AppImage/.deb/.rpm, macOS → .dmg) and its own
# installer (Encore-Installer-<version>-windows-x86_64.exe / -macos-universal.dmg / -linux-x86_64.AppImage +
# encore-installer_<version>_amd64.deb); Android builds anywhere the SDK + NDK are installed.
#
#   scripts/release-apps.sh                      # version of the newest release, app + installer + Android, upload
#   scripts/release-apps.sh 0.20.3               # name the files after this version instead
#   scripts/release-apps.sh --installer          # only the Omnistall installer for this OS (--desktop, --android:
#                                                # only the app / only the APK; combine them as you like)
#   scripts/release-apps.sh --no-upload          # just build into out/
#   scripts/release-apps.sh --dry-run            # show what would run
#
# Needs: rustup/cargo, the gh CLI signed in (gh auth login). Android also needs Java 17+ and the
# Android SDK with an NDK (ANDROID_HOME or ANDROID_SDK_ROOT set; Android Studio sets them).
# Windows: run from Git Bash. Linux: libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev
# librsvg2-dev patchelf. macOS: Xcode command line tools.
# The installer is Omnistall (github.com/Krifmaz/Omnistall) built from the commit in clients/installer/OMNISTALL_REV
# (cloned into a temporary folder, or OMNISTALL_DIR when set, so the build cache survives between runs) and branded
# with clients/installer/encore.toml.in through scripts/installer-manifest.sh. Linux needs dpkg-deb for the .deb;
# appimagetool is downloaded when it is not on PATH. The installer downloads the app files from the release, so
# attach those first (or in the same run); SHA256SUMS on the release is updated to cover every file attached here.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"

DESKTOP=0; ANDROID=0; INSTALLER=0; UPLOAD=1; DRY=0; V=""
for a in "$@"; do
  case "$a" in
    --desktop) DESKTOP=1 ;;
    --android) ANDROID=1 ;;
    --installer) INSTALLER=1 ;;
    --no-upload) UPLOAD=0 ;;
    --dry-run) DRY=1 ;;
    -h|--help) sed -n '2,23p' "$0"; exit 0 ;;
    -*) echo "unknown option $a"; exit 1 ;;
    *) V="${a#v}" ;;
  esac
done
# nothing picked: everything
if [ "$DESKTOP$ANDROID$INSTALLER" = 000 ]; then DESKTOP=1; ANDROID=1; INSTALLER=1; fi
run() { echo "+ $*"; [ "$DRY" = 1 ] || "$@"; }
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

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
if [ "$DESKTOP$ANDROID" != 00 ] && ! cargo tauri --version 2>/dev/null | grep -q '^tauri-cli 2\.'; then
  run cargo install tauri-cli --version 2.12.2 --locked
fi
case "$(uname -s)" in
  Linux*)  OS=linux; BUNDLES=deb,appimage,rpm; ARGS=""
           [ "$DESKTOP" = 0 ] || pkg-config --exists webkit2gtk-4.1 2>/dev/null || echo "note: webkit2gtk-4.1 not found – on Debian/Ubuntu: sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf" ;;
  Darwin*) OS=mac; BUNDLES=app; ARGS="--target universal-apple-darwin"
           [ "$DESKTOP$INSTALLER" = 00 ] || run rustup target add aarch64-apple-darwin x86_64-apple-darwin ;;
  *)       OS=windows; BUNDLES=nsis; ARGS="" ;;
esac

mkdir -p out
export ENCORE_VERSION="$V"

# ---- desktop -------------------------------------------------------------------------------------
if [ "$DESKTOP" = 1 ]; then
  echo "== desktop ($OS)"
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
      *.AppImage) cp "$f" "out/Encore-$V-linux-x86_64.AppImage" ;;
      *.deb)      cp "$f" "out/Encore-$V-linux-amd64.deb" ;;
      *.rpm)      cp "$f" "out/Encore-$V-linux-x86_64.rpm" ;;
      *.exe)      cp "$f" "out/Encore-Setup-$V.exe" ;;
      *.dmg)      cp "$f" "out/Encore-$V-mac.dmg" ;;
    esac
  done
fi

# ---- installer (Omnistall, branded for Encore) --------------------------------------------------
if [ "$INSTALLER" = 1 ]; then
  echo "== installer ($OS)"
  REV=$(sed -n 's/^\([0-9a-f]\{40\}\)[[:space:]]*$/\1/p' clients/installer/OMNISTALL_REV | head -1)
  [ -n "$REV" ] || { echo "clients/installer/OMNISTALL_REV holds no 40-character commit sha"; exit 1; }
  if [ "$OS" = linux ] && [ "$(uname -m)" != x86_64 ]; then
    echo "note: the manifest has no Linux $(uname -m) app file (the desktop app is built for x86_64 only), so an installer for this machine would have nothing to install – skipped"
  else
    OMNI="${OMNISTALL_DIR:-$(mktemp -d)}"
    if [ -f "$OMNI/Cargo.toml" ]; then
      [ "$(git -C "$OMNI" rev-parse HEAD)" = "$REV" ] || { echo "$OMNI is not at Omnistall $REV (clients/installer/OMNISTALL_REV) – check it out there or unset OMNISTALL_DIR"; exit 1; }
    else
      echo "Omnistall $REV → $OMNI"
      run git init -q "$OMNI"
      run git -C "$OMNI" fetch -q --depth 1 https://github.com/Krifmaz/Omnistall "$REV"
      run git -C "$OMNI" checkout -q FETCH_HEAD
    fi
    MANIFEST="$OMNI/encore-$V.toml"
    run bash scripts/installer-manifest.sh "$V" "$MANIFEST"
    # the xtask alias (cargo run -p xtask) only resolves inside the Omnistall checkout
    (cd "$OMNI" && run cargo xtask validate "$MANIFEST")
    case "$OS" in
      linux)
        (cd "$OMNI" && run cargo build --release --locked -p installer-gui)
        BIN="$OMNI/target/release/omnistall"
        (cd "$OMNI" && run cargo xtask appdir --binary "$BIN" --out "$OMNI/Encore-Installer.AppDir" --manifest "$MANIFEST" --name "Encore Installer")
        TOOL=$(command -v appimagetool || true)
        if [ -z "$TOOL" ]; then
          TOOL="$OMNI/appimagetool"
          run curl -sSL -o "$TOOL" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
          run chmod +x "$TOOL"
        fi
        run env ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$OMNI/Encore-Installer.AppDir" "$ROOT/out/Encore-Installer-$V-linux-x86_64.AppImage"
        if command -v dpkg-deb >/dev/null; then
          (cd "$OMNI" && run cargo xtask deb --binary "$BIN" --out "$ROOT/out/encore-installer_${V}_amd64.deb" --version "$V" --arch amd64 --manifest "$MANIFEST")
        else
          echo "note: dpkg-deb not found – no encore-installer_${V}_amd64.deb (sudo apt install dpkg)"
        fi ;;
      mac)
        (cd "$OMNI" && run cargo build --release --locked -p installer-gui --target aarch64-apple-darwin \
                    && run cargo build --release --locked -p installer-gui --target x86_64-apple-darwin)
        run lipo -create -output "$OMNI/omnistall-universal" "$OMNI/target/aarch64-apple-darwin/release/omnistall" "$OMNI/target/x86_64-apple-darwin/release/omnistall"
        APP="$OMNI/Encore Installer.app"
        (cd "$OMNI" && run cargo xtask app-bundle --binary "$OMNI/omnistall-universal" --out "$APP" --version "$V" --manifest "$MANIFEST" --name "Encore Installer")
        # Developer ID when MACOS_SIGN_IDENTITY is set, ad-hoc otherwise (Apple Silicon runs no unsigned code)
        if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then run codesign --force --deep --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$APP"
        else run codesign --force --deep --sign - "$APP"; fi
        STAGE=$(mktemp -d); [ "$DRY" = 1 ] || { cp -R "$APP" "$STAGE/"; ln -s /Applications "$STAGE/Applications"; }
        run hdiutil create -volname "Encore Installer" -srcfolder "$STAGE" -ov -format UDZO "$ROOT/out/Encore-Installer-$V-macos-universal.dmg" ;;
      windows)
        (cd "$OMNI" && run cargo build --release --locked -p installer-gui)
        # brand (append the manifest) before any signing: Authenticode covers the whole file
        (cd "$OMNI" && run cargo xtask brand --installer "$OMNI/target/release/omnistall.exe" --manifest "$MANIFEST" --out "$ROOT/out/Encore-Installer-$V-windows-x86_64.exe") ;;
    esac
  fi
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
# older app files and installers of other versions go: the release only ever carries one set
OLD=()
for old in $(gh release view "$TAG" --json assets -q '.assets[].name' | grep -E '^Encore-(Setup-|android-|Installer-|[0-9])|^encore-installer_' | grep -v -- "$V" || true); do
  run gh release delete-asset "$TAG" "$old" --yes
  OLD+=("$old")
done
run gh release upload "$TAG" "${FILES[@]}" --clobber

# SHA256SUMS: the installers check every download against it, so it must list the files just attached (and no
# longer the removed ones). Lines are `hash  name`, the format sha256sum writes and Omnistall reads.
NEW=(); for f in "${FILES[@]}"; do NEW+=("$(basename "$f")"); done
SUMS="$ROOT/out/.sums"; rm -rf "$SUMS"; mkdir -p "$SUMS"
gh release download "$TAG" --pattern SHA256SUMS --output "$SUMS/old" 2>/dev/null || : > "$SUMS/old"
printf '%s\n' "${NEW[@]}" "${OLD[@]+"${OLD[@]}"}" > "$SUMS/drop"
awk 'NR == FNR { drop[$0] = 1; next } !($2 in drop)' "$SUMS/drop" "$SUMS/old" > "$SUMS/SHA256SUMS"
(cd out && sha256 "${NEW[@]}") >> "$SUMS/SHA256SUMS"
[ "$DRY" = 1 ] && { echo "+ SHA256SUMS would become:"; cat "$SUMS/SHA256SUMS"; }
run gh release upload "$TAG" "$SUMS/SHA256SUMS" --clobber
[ "$DRY" = 1 ] && exit 0

# the notes' app lines (desktop installers, Android, the files the installer downloads) describe what the workflow
# attached when it ran – rewrite them from the files attached now (scripts/release-notes-apps.sh)
BODY=$(gh release view "$TAG" --json body -q .body)
ASSETS=$(gh release view "$TAG" --json assets -q '.assets[].name')
# shellcheck disable=SC2086
bash scripts/release-notes-apps.sh "$V" all $ASSETS > out/.app-lines.md
printf '%s\n' "$BODY" | awk '
  NR == FNR { if (match($0, /^\* \*\*[^*]+\*\*/)) { k = substr($0, RSTART, RLENGTH); line[k] = $0; order[++n] = k }; next }
  { if (match($0, /^\* \*\*[^*]+\*\*/) && (substr($0, RSTART, RLENGTH) in line)) { k = substr($0, RSTART, RLENGTH); print line[k]; done[k] = 1; next } }
  /were not rebuilt for this release/ { next }   # the pre-0.27 wording
  { print }
  END { for (i = 1; i <= n; i++) if (!(order[i] in done)) print line[order[i]] }
' out/.app-lines.md - > out/.notes.md
run gh release edit "$TAG" --notes-file out/.notes.md
echo "Done: https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/$TAG"
