# `:cadenza` – Cadenza on the phone

The Gradle library module that puts [cadenza-dsp](https://github.com/Krifmaz/cadenza) (the server's engine) into the
Android app. It is the app's only effects engine: the sound effects of Settings → Playback (preamp, EQ, crossfeed,
mono, balance) and the phone's own upsampling ("Cadenza engine · this phone").

| Path | What |
|---|---|
| `rust/` | Crate `encore-cadenza-jni` → `libencore_cadenza.so` (cdylib). Depends on `cadenza-dsp` at the **same git rev as `server/Cargo.toml`**. `engine.rs` is the effects graph (preamp → EQ → crossfeed → mono → balance → optional Cadenza upsampler + dither); `lib.rs` is the JNI surface; `build.rs` links Android builds with `-z max-page-size=16384` (16 KB pages). |
| `src/main/java/app/encore/android/cadenza/CadenzaNative.kt` | The JNI binding: `create / setParams / process / fetch / flush / signalPathJson / destroy`, `CadenzaNative.available`, `CadenzaNative.catalog()`. |
| `build.gradle.kts` | `cargoNdkBuild` (Exec, before `preBuild`): `cargo ndk -t arm64-v8a -t armeabi-v7a -P 26 -o build/jniLibs build --release`. **Skipped with a warning when `cargo-ndk` is not on the PATH**, so a build without Rust still succeeds; the APK then has no Cadenza library and plays without sound effects or phone upsampling. |
| `consumer-rules.pro` | Keeps the JNI class and its `native` methods under R8. |

The player's side is `app.encore.android.cadenza.CadenzaEngine` in the `:app` module, referenced directly (no
reflection): one instance in the playback service's effects processor (effects and, per song, the phone's
upsampling), another in the song transitions' second player (effects only). Without the library `configure()`
throws and the processor passes the sound through.

## Audio across JNI: direct buffers

`nativeProcess` works on direct, native-order `ByteBuffer`s of floats that the Kotlin side owns and reuses, so no
Java array is made or copied per buffer. The effects processor writes a chunk into `CadenzaEngine.input(n)`; Rust
reads it through `GetDirectBufferAddress` (the engine copies it into its own scratch first) and writes the output
back into the same buffer (in place: effects keep the length) or, while upsampling, into a second buffer (the output
is longer). When the output does not fit the second buffer, the call returns `-n`; the Kotlin side grows the buffer
and `nativeFetch` copies the same output again. Chunks with no engine come back untouched; a failed engine loses the
chunk (it could not go out at the input rate when the sink is configured for the output rate).

## JNI surface (`CadenzaNative`, instance methods unless noted)

| Kotlin | Rust symbol | Meaning |
|---|---|---|
| `nativeCreate(sampleRate, channels, filterId, ditherId, outRate, cacheDir): Long` | `Java_app_encore_android_cadenza_CadenzaNative_nativeCreate` | engine handle (0 = failed). `outRate` 0 or = input rate: no resampling. `cacheDir` "" = in-memory filter designs. |
| `nativeOutputRate(handle): Int` | `…_nativeOutputRate` | the rate the engine produces |
| `nativeSetParams(handle, json)` | `…_nativeSetParams` | `PlaybackPrefs.engineParams()` JSON: `fx_on, preamp_db, bands[{type,freq,gain,q}], crossfeed, balance, mono` |
| `nativeProcess(handle, io: ByteBuffer, frames, out: ByteBuffer?): Int` | `…_nativeProcess` | `frames` interleaved frames of floats in `io`; output in `io` (when `out` is null) or `out`. Returns the samples written, `-n` when `n` don't fit (then `nativeFetch`), `Int.MIN_VALUE` when the handle is dead, a buffer is not direct, or the engine panicked |
| `nativeFetch(handle, out: ByteBuffer): Int` | `…_nativeFetch` | the last output again, into a larger buffer; same answers |
| `nativeFlush(handle): FloatArray` | `…_nativeFlush` | the tail at the end of a stream (once per stream); resets filters |
| `nativeSignalPath(handle): String` | `…_nativeSignalPath` | JSON `{engine, summary, fx_on, input_rate, output_rate, latency_seconds, preamp_db, eq_bands, crossfeed, balance, mono, identity, filter, dither, pipeline}`; `summary` is the Signal path sheet's line, e.g. `Cadenza on this phone · EQ: 2 bands, preamp -6 dB · Crossfeed: medium` |
| `nativeCatalog(): String` (static) | `…_nativeCatalog` | JSON `{filters: [[id,label]…], dithers: [[id,label]…], default_filter, default_dither}` like `GET /api/playback/dsp` (`CadenzaCatalog` reads both) |
| `nativeDestroy(handle)` | `…_nativeDestroy` | frees the engine |

Handles name engines in a registry (never pointers); a stale handle finds nothing, and nothing unwinds into the JVM.
Crossfeed, balance and mono are implemented in the JNI crate (cadenza-dsp has no such stages at the pinned rev) and
copy the web player's graph: 700 Hz low-pass (Q 0.5 dB) + 0.3 ms delay at −9 / −6 / −4.5 dB (light / medium /
strong) trimmed by 1 / (1 + g); mono averages; balance is Web Audio's equal-power panner. Shelves use Web Audio's
fixed slope (Q = 1/√2) and low-/high-pass Q is read in dB, as in the browser.

## Building and testing

- Host (no Android SDK or NDK needed): `cargo build` and `cargo test` in `rust/` (the engine, the handle registry,
  in-place and second-buffer processing with `process_into` / `fetch_into`, the JNI symbol names).
- Phone libs by hand: `rustup target add aarch64-linux-android armv7-linux-androideabi`,
  `cargo install cargo-ndk --version 4.1.2 --locked`, an NDK (`ANDROID_NDK_HOME` or `$ANDROID_HOME/ndk/<version>`),
  then any Gradle build of `:app` runs `:cadenza:cargoNdkBuild`.
- CI (`.github/workflows/apps.yml`, job `android-native`) installs Rust, the two targets, cargo-ndk 4.1.2 and
  `ndk;29.0.13846066`, runs the crate's host tests, fails the build if a release APK lacks
  `lib/<abi>/libencore_cadenza.so`, and checks the 16 KB alignment (`llvm-readelf -l`: every LOAD segment at least
  0x4000; `zipalign -c -P 16 -v 4` on the APK).
