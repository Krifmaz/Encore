#!/usr/bin/env bash
# Runs the performance journeys on the connected phone or emulator (PERFORMANCE.md): optionally regenerates the
# Baseline Profile first, then runs every Macrobenchmark and prints the result tables.
#
# Environment (all optional but the server and account, which a signed-out app needs):
#   ENCORE_SERVER, ENCORE_USER, ENCORE_PASSWORD  the server and account to sign in with
#   COMPILATION   partial (default: the shipped Baseline Profile, as on a fresh install) or none
#   PROFILE       1: regenerate the Baseline Profile (and startup profile) into app/src/main/generated first
#   EMULATOR      1: let Macrobenchmark run on an emulator (its numbers only compare with the same emulator)
#   ITERATIONS, STARTUP_ITERATIONS  iterations of the frame-timing journeys (5) and of each startup (10)
#   LABEL         a name for the tables (e.g. the commit)
#   STRICTMODE    1: afterwards, drive the debug build through every journey once and summarise what StrictMode and
#                 JankStats logged (benchmark/build/strictmode.md)
set -euo pipefail
cd "$(dirname "$0")/../.."   # clients/android
A=android.testInstrumentationRunnerArguments
ARGS=(--no-daemon --stacktrace
  "-P$A.encore.server=${ENCORE_SERVER:-}"
  "-P$A.encore.user=${ENCORE_USER:-}"
  "-P$A.encore.password=${ENCORE_PASSWORD:-}"
  "-P$A.encore.compilation=${COMPILATION:-partial}"
  "-P$A.encore.iterations=${ITERATIONS:-5}"
  "-P$A.encore.startupIterations=${STARTUP_ITERATIONS:-10}")
# errors Macrobenchmark lets pass: an emulator in CI (the debug pass adds DEBUGGABLE); benchmark/build.gradle.kts
# reads encore.suppressErrors
SUPPRESS=""
if [ "${EMULATOR:-0}" = 1 ]; then SUPPRESS=EMULATOR; fi
if [ -n "$SUPPRESS" ]; then ARGS+=("-Pencore.suppressErrors=$SUPPRESS"); fi
if [ "${PROFILE:-0}" = 1 ]; then
  mkdir -p benchmark/build
  gradle "${ARGS[@]}" :app:generateBaselineProfile > benchmark/build/gradle-profile.log 2>&1 \
    || { grep -E -A3 "What went wrong|FAILED|AssertionError|Exception: " benchmark/build/gradle-profile.log | grep -v "^\s*at " | head -60
         python3 benchmark/ci/failures.py baselineprofile/build/outputs/androidTest-results baselineprofile/build/outputs || true
         exit 1; }
fi
OUT=benchmark/build/outputs/connected_android_test_additional_output
RESULTS=benchmark/build/outputs/androidTest-results
rm -rf "$OUT" "$RESULTS"
mkdir -p benchmark/build

# Runs Gradle with its output in a log file ($1), printing only the lines that say what went wrong; returns its status.
quiet_gradle() {
  local log=$1; shift
  local status=0
  gradle "$@" > "$log" 2>&1 || status=$?
  if [ "$status" != 0 ]; then
    grep -E -A3 "What went wrong|^e: |FAILED|AssertionError|Exception: " "$log" | grep -v "^\s*at " | head -60 || true
  fi
  return "$status"
}

STATUS=0
quiet_gradle benchmark/build/gradle-benchmark.log "${ARGS[@]}" :benchmark:connectedBenchmarkReleaseAndroidTest || STATUS=$?
python3 benchmark/ci/failures.py "$RESULTS" "$OUT" || true
python3 benchmark/ci/summarize.py "$OUT" "${LABEL:-}" | tee benchmark/build/summary.md || true

if [ "${STRICTMODE:-0}" = 1 ]; then
  # the debug build (StrictMode and JankStats on, core/DevPerf.kt) through each journey once, uncompiled
  quiet_gradle benchmark/build/gradle-install-debug.log --no-daemon :app:installDebug || true
  # the whole log of the pass, streamed to a file (the device's ring buffer would drop the start of a long pass)
  adb logcat -c
  adb logcat -v brief > benchmark/build/debug-logcat.txt 2>/dev/null &
  LOGCAT_PID=$!
  rm -rf "$RESULTS" "$OUT"
  # the arguments this pass sets itself are left out of ARGS (Gradle keeps the first of a repeated -P)
  DEBUG_ARGS=()
  for a in "${ARGS[@]}"; do
    case "$a" in *suppressErrors=*|*encore.compilation=*|*encore.iterations=*) ;; *) DEBUG_ARGS+=("$a") ;; esac
  done
  quiet_gradle benchmark/build/gradle-debug-pass.log "${DEBUG_ARGS[@]}" :benchmark:connectedBenchmarkReleaseAndroidTest \
    "-P$A.class=app.encore.android.benchmark.JourneyBenchmarks" \
    "-P$A.encore.package=app.encore.client.debug" \
    "-P$A.encore.compilation=none" \
    "-P$A.encore.iterations=1" \
    "-Pencore.suppressErrors=${SUPPRESS:+$SUPPRESS,}DEBUGGABLE" || echo "the debug pass failed (its log is still summarised)"
  python3 benchmark/ci/failures.py "$RESULTS" "$OUT" || true
  kill "$LOGCAT_PID" 2>/dev/null || true
  echo "debug pass log: $(wc -l < benchmark/build/debug-logcat.txt) lines, $(grep -c StrictMode benchmark/build/debug-logcat.txt || true) StrictMode, $(grep -c EncoreJank benchmark/build/debug-logcat.txt || true) EncoreJank"
  python3 benchmark/ci/strictmode.py benchmark/build/debug-logcat.txt | tee benchmark/build/strictmode.md || true
fi
exit "$STATUS"
