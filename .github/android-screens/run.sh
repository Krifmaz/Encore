#!/usr/bin/env bash
# run.sh SIDE DIR OUT: build SIDE's (before|after) screenshot test into the checkout at DIR, run it on the running
# emulator, and pull the PNGs into OUT. Used by .github/workflows/android-screens.yml.
set -euo pipefail
side="$1"; dir="$2"; out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
cat "$here/gradle-additions.kts" >> "$dir/clients/android/app/build.gradle.kts"
python3 "$here/make.py" "$side" "$dir/clients/android/app/src/androidTest/java/app/encore/android/ScreensTest.kt"
adb shell rm -rf /sdcard/Android/data/app.encore.client.debug/files/screens || true
# keep the app installed after the run, or its files (the screenshots) go with it
gradle -p "$dir/clients/android" --no-daemon --stacktrace :app:connectedDebugAndroidTest \
  -Pandroid.injected.androidTest.leaveApksInstalledAfterRun=true
mkdir -p "$out"
adb pull /sdcard/Android/data/app.encore.client.debug/files/screens/. "$out"
adb uninstall app.encore.client.debug || true
adb uninstall app.encore.client.debug.test || true
ls -l "$out"
