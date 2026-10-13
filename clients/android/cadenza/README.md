# `:cadenza` – Cadenza on the phone

The Gradle library module that puts [cadenza-dsp](https://github.com/Krifmaz/cadenza) (the server's engine) into the
Android app (docs/android-native/PLAN.md §6.3).

| Path | What |
|---|---|
| `rust/` | Crate `encore-cadenza-jni` → `libencore_cadenza.so` (cdylib). Depends on `cadenza-dsp` at the **same git rev as `server/Cargo.toml`**. `engine.rs` is the effects graph (preamp → EQ → crossfeed → mono → balance → optional Cadenza upsampler + dither); `lib.rs` is the JNI surface. |
| `src/main/java/app/encore/android/cadenza/CadenzaNative.kt` | The JNI binding: `create / setParams / process / flush / signalPathJson / destroy`, `CadenzaNative.available`, `CadenzaNative.catalog()`. |
| `build.gradle.kts` | `cargoNdkBuild` (Exec, before `preBuild`): `cargo ndk -t arm64-v8a -t armeabi-v7a -P 26 -o build/jniLibs build --release`. **Skipped with a warning when `cargo-ndk` is not on the PATH**, so a build without Rust still succeeds (the APK then has no Cadenza library and the player uses its Kotlin DSP). |
| `consumer-rules.pro` | Keeps the JNI class and its `native` methods under R8. |

The player's adapter is `app.encore.android.cadenza.CadenzaEngine : DspEngine` in the `:app` module (it needs the
app's `DspEngine` interface, which a library module cannot see). `DspEngines.create()` finds it by name and falls
back to the Kotlin engine when `configure()` throws (no `.so` in the APK).

## JNI surface (`CadenzaNative`, instance methods unless noted)

| Kotlin | Rust symbol | Meaning |
|---|---|---|
| `nativeCreate(sampleRate, channels, filterId, ditherId, outRate, cacheDir): Long` | `Java_app_encore_android_cadenza_CadenzaNative_nativeCreate` | engine handle (0 = failed). `outRate` 0 or = input rate: no resampling. `cacheDir` "" = in-memory filter designs. |
| `nativeOutputRate(handle): Int` | `…_nativeOutputRate` | the rate the engine produces |
| `nativeSetParams(handle, json)` | `…_nativeSetParams` | `PlaybackPrefs.engineParams()` JSON: `fx_on, preamp_db, bands[{type,freq,gain,q}], crossfeed, balance, mono` |
| `nativeProcess(handle, FloatArray, frames): FloatArray` | `…_nativeProcess` | interleaved float PCM in / out (may be empty while the upsampler gathers a block) |
| `nativeFlush(handle): FloatArray` | `…_nativeFlush` | the tail at the end of a stream; resets filters |
| `nativeSignalPath(handle): String` | `…_nativeSignalPath` | JSON `{engine, summary, fx_on, input_rate, output_rate, latency_seconds, preamp_db, eq_bands, crossfeed, balance, mono, identity, filter, dither, pipeline}`; `summary` is the Signal path sheet's line, e.g. `Cadenza on this phone · EQ: 2 bands, preamp -6 dB · Crossfeed: medium` |
| `nativeCatalog(): String` (static) | `…_nativeCatalog` | JSON `{filters: [[id,label]…], dithers: [[id,label]…], default_filter, default_dither}` like `GET /api/playback/dsp` |
| `nativeDestroy(handle)` | `…_nativeDestroy` | frees the engine |

Crossfeed, balance and mono are implemented in the JNI crate (cadenza-dsp has no such stages at the pinned rev) and
copy `web/js/player.js` applyFx: 700 Hz low-pass (Q 0.5 dB) + 0.3 ms delay at −9 / −6 / −4.5 dB (light / medium /
strong) trimmed by 1 / (1 + g); mono averages; balance is Web Audio's equal-power panner. Shelves use Web Audio's
fixed slope (Q = 1/√2) and low-/high-pass Q is read in dB, as in the browser.

## Building and testing

- Host (no Android SDK or NDK needed): `cargo build` and `cargo test` in `rust/`.
- Phone libs by hand: `rustup target add aarch64-linux-android armv7-linux-androideabi`,
  `cargo install cargo-ndk --version 4.1.2 --locked`, an NDK (`ANDROID_NDK_HOME` or `$ANDROID_HOME/ndk/<version>`),
  then any Gradle build of `:app` runs `:cadenza:cargoNdkBuild`.
- CI (`.github/workflows/apps.yml`, job `android-native`) installs Rust, the two targets, cargo-ndk 4.1.2 and
  `ndk;29.0.13846066`, runs the crate's host tests and fails the build if a release APK lacks
  `lib/<abi>/libencore_cadenza.so`.
