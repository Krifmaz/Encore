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
if [ "${EMULATOR:-0}" = 1 ]; then
  ARGS+=("-P$A.androidx.benchmark.suppressErrors=EMULATOR")
fi
if [ "${PROFILE:-0}" = 1 ]; then
  gradle "${ARGS[@]}" :app:generateBaselineProfile
fi
OUT=benchmark/build/outputs/connected_android_test_additional_output
rm -rf "$OUT"
STATUS=0
gradle "${ARGS[@]}" :benchmark:connectedBenchmarkReleaseAndroidTest || STATUS=$?
python3 benchmark/ci/summarize.py "$OUT" "${LABEL:-}" | tee benchmark/build/summary.md || true

if [ "${STRICTMODE:-0}" = 1 ]; then
  # the debug build (StrictMode and JankStats on, core/DevPerf.kt) through each journey once, uncompiled
  gradle --no-daemon :app:installDebug
  adb logcat -c
  gradle "${ARGS[@]}" :benchmark:connectedBenchmarkReleaseAndroidTest \
    "-P$A.class=app.encore.android.benchmark.JourneyBenchmarks" \
    "-P$A.encore.package=app.encore.client.debug" \
    "-P$A.encore.compilation=none" \
    "-P$A.encore.iterations=1" \
    "-P$A.androidx.benchmark.suppressErrors=EMULATOR,DEBUGGABLE" || echo "the debug pass failed (its log is still summarised)"
  adb logcat -d -v brief > benchmark/build/debug-logcat.txt || true
  python3 benchmark/ci/strictmode.py benchmark/build/debug-logcat.txt | tee benchmark/build/strictmode.md || true
fi
exit "$STATUS"
