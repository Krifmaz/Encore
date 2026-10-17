#!/usr/bin/env bash
# The app lines of a release's Install section, worked out from the files attached to it: the desktop installers
# (Omnistall branded with clients/installer/encore.toml.in), the Android APK, and the plain app files the installer
# downloads. The release workflow (.github/workflows/release.yml) writes them into the notes; scripts/release-apps.sh
# rewrites them after it attaches files to a published release.
#   scripts/release-notes-apps.sh <version> desktop|android|files|all <file or asset name>...
# Files of another version than <version> were carried over from an earlier release; the lines say so.
set -euo pipefail
V="${1:?version}"; WHICH="${2:?desktop|android|files|all}"; shift 2
NAMES=()
for f in "$@"; do NAMES+=("$(basename "$f")"); done

pick() {  # pick <glob>: the first attached name matching the pattern (nothing when none does)
  local n
  # shellcheck disable=SC2254  # the argument is a glob on purpose
  for n in "${NAMES[@]+"${NAMES[@]}"}"; do case "$n" in $1) echo "$n"; return ;; esac; done
}
ver_of() {  # the version in Encore-Installer-<v>-…, Encore-android-<v>.apk, encore-installer_<v>_amd64.deb
  echo "$1" | sed -E 's/^(Encore-Installer-|Encore-android-|encore-installer_)([^-_]+)[-_.].*/\2/'
}
join() { local IFS="$1"; shift; echo "$*"; }

desktop() {
  local W M L D IV lx parts=()
  W=$(pick 'Encore-Installer-*-windows-x86_64.exe'); M=$(pick 'Encore-Installer-*-macos-universal.dmg')
  L=$(pick 'Encore-Installer-*-linux-x86_64.AppImage'); D=$(pick 'encore-installer_*_amd64.deb')
  if [ -z "$W$M$L$D" ]; then
    echo '* **Desktop app**: no installer in this release – install one of the plain app files listed under "Files the installer downloads"'
    return
  fi
  [ -n "$W" ] && parts+=("**Windows**: \`$W\`")
  [ -n "$M" ] && parts+=("**macOS**: \`$M\` (right-click → Open the first time unless notarized)")
  if [ -n "$L$D" ]; then
    lx="**Linux**:"
    [ -n "$L" ] && lx="$lx \`$L\`"
    [ -n "$L" ] && [ -n "$D" ] && lx="$lx or"
    [ -n "$D" ] && lx="$lx \`$D\` (installs the package \`omnistall\`; \`sudo apt remove omnistall\` removes it)"
    parts+=("$lx")
  fi
  local line
  line="* **Desktop app** – one Install button; it downloads the app for your computer from this release, checks it against \`SHA256SUMS\` and sets up the shortcuts (run it again for Update, Repair or Uninstall): $(join '|' "${parts[@]}")"
  line="${line//|/ · }"
  IV=$(ver_of "${W:-${M:-${L:-$D}}}")
  [ "$IV" = "$V" ] || line="$line. These installers come from release $IV and install Encore $IV: the desktop app was not rebuilt for $V"
  echo "$line"
}

android() {
  local n A="" AV
  for n in "${NAMES[@]+"${NAMES[@]}"}"; do case "$n" in Encore-android-legacy-*) ;; Encore-android-*.apk) A="$n"; break ;; esac; done
  [ -n "$A" ] || return 0
  AV=$(ver_of "$A")
  if [ "$AV" = "$V" ]; then echo "* **Android**: \`$A\`"
  else echo "* **Android**: \`$A\` – from release $AV, the phone app was not rebuilt for $V"; fi
}

files() {
  local pat n sig P=()
  # the version starts with a digit (or "dev." on a manual run); Encore-Installer-… and Encore-android-… stay out
  for pat in 'Encore-Setup-[0-9d]*.exe' 'Encore-[0-9d]*-mac.dmg' 'Encore-[0-9d]*-linux-x86_64.AppImage' 'Encore-[0-9d]*-linux-amd64.deb' 'Encore-[0-9d]*-linux-x86_64.rpm'; do
    n=$(pick "$pat"); [ -n "$n" ] && P+=("\`$n\`")
  done
  if [ ${#P[@]} = 0 ]; then
    echo '* **Files the installer downloads (advanced)**: none attached – the desktop app was not built for this release and no earlier build was found'
    return
  fi
  local line
  line="* **Files the installer downloads (advanced)**: $(join '|' "${P[@]}") – the plain desktop app files, for installing by hand"
  line="${line//|/ · }"
  sig=$(pick '*.sig'); [ -n "$sig" ] && line="$line · the \`.sig\`, \`.app.tar.gz\` and \`latest.json\` files feed the app's self-update"
  echo "$line · \`SHA256SUMS\` lists the hash of every file here"
}

case "$WHICH" in
  desktop) desktop ;;
  android) android ;;
  files) files ;;
  all) desktop; android; files ;;
  *) echo "unknown section $WHICH (desktop|android|files|all)"; exit 1 ;;
esac
