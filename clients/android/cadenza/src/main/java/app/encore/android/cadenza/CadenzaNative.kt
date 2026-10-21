package app.encore.android.cadenza

import java.nio.ByteBuffer

/**
 * The JNI binding of `libencore_cadenza.so` (clients/android/cadenza/rust): cadenza-dsp on the phone. One
 * instance owns one native engine for one stream format; [create] builds it, [destroy] frees it. The JNI symbol
 * names (`Java_app_encore_android_cadenza_CadenzaNative_native…`) are pinned by a test in the Rust crate, so this
 * class and package must not be renamed without it.
 *
 * The library may be missing from the APK (a build without cargo-ndk), so [available] is checked first and
 * every call on an instance without a handle is a no-op. The player's side is `app.encore.android.cadenza.CadenzaEngine`
 * in the app module.
 *
 * Audio crosses as direct, native-order `ByteBuffer`s of floats that the caller owns and reuses ([process]): the
 * native side reads and writes their memory directly, so no array is made or copied per buffer.
 */
class CadenzaNative {
    /**
     * The native engine's handle. Read once per call: the playback thread (process, configure) and the main thread
     * (Signal path, release) use it at the same time. The native side keeps engines in a registry and never
     * dereferences a handle, so a call with a handle another thread just freed finds nothing instead of freed
     * memory (lib.rs).
     */
    @Volatile private var handle: Long = 0L
    private val lock = Any()

    /** `true` while a native engine exists. */
    val isOpen: Boolean get() = handle != 0L

    /**
     * Builds an engine for `channels` interleaved channels at `sampleRate` Hz. `outRate` 0 (or the input rate)
     * means no resampling; otherwise Cadenza upsamples with `filterId` / `ditherId` (ids from [catalog]; "" =
     * defaults) and designs long filters once into `cacheDir` ("" = memory only). Returns the output rate, or 0
     * when the engine could not be built.
     */
    fun create(sampleRate: Int, channels: Int, filterId: String, ditherId: String, outRate: Int, cacheDir: String): Int {
        destroy()
        if (!available) return 0
        // built without the lock (designing a long filter takes a while; release() on the main thread must not wait)
        val h = nativeCreate(sampleRate, channels, filterId, ditherId, outRate, cacheDir)
        if (h == 0L) return 0
        val rate = nativeOutputRate(h)
        if (rate <= 0) {
            nativeDestroy(h)
            return 0
        }
        val old = synchronized(lock) { val o = handle; handle = h; o }
        if (old != 0L) nativeDestroy(old)
        return rate
    }

    /** The effects as the JSON document `PlaybackPrefs.engineParams()` makes. */
    fun setParams(json: String) {
        val h = handle
        if (h != 0L) nativeSetParams(h, json)
    }

    /**
     * Runs `frames` interleaved frames of float PCM through the engine. `io` holds them (a direct buffer in native
     * byte order, from index 0); the output goes back into `io` (in place) when `out` is null, else into `out`
     * (the second buffer for upsampling, whose output is longer). Returns the number of float samples written;
     * a negative `-n` when the output is `n` samples and the target buffer is smaller (grow it, then [fetch]);
     * [CLOSED] without an engine (nothing was touched); [FAILED] when the engine went away under us or failed
     * (the chunk is lost: passed through it would go out at the input rate where the sink was configured for the
     * engine's output rate).
     */
    fun process(io: ByteBuffer, frames: Int, out: ByteBuffer?): Int {
        val h = handle
        if (h == 0L) return CLOSED
        return nativeProcess(h, io, frames.coerceAtLeast(0), out)
    }

    /** The output of the last [process] again, into `out` (after a negative answer): as [process] answers. */
    fun fetch(out: ByteBuffer): Int {
        val h = handle
        if (h == 0L) return CLOSED
        return nativeFetch(h, out)
    }

    /** The tail at the end of a stream; resets the filters. */
    fun flush(): FloatArray {
        val h = handle
        if (h == 0L) return EMPTY
        return nativeFlush(h) ?: EMPTY
    }

    /** JSON: `{engine, summary, fx_on, input_rate, output_rate, latency_seconds, eq_bands, crossfeed, balance, mono, pipeline}`. */
    fun signalPathJson(): String {
        val h = handle
        if (h == 0L) return ""
        return nativeSignalPath(h) ?: ""
    }

    fun destroy() {
        val h = synchronized(lock) { val o = handle; handle = 0L; o }
        if (h != 0L) nativeDestroy(h)
    }

    private external fun nativeCreate(sampleRate: Int, channels: Int, filterId: String, ditherId: String, outRate: Int, cacheDir: String): Long
    private external fun nativeOutputRate(handle: Long): Int
    private external fun nativeSetParams(handle: Long, json: String)
    private external fun nativeProcess(handle: Long, io: ByteBuffer, frames: Int, out: ByteBuffer?): Int
    private external fun nativeFetch(handle: Long, out: ByteBuffer): Int
    private external fun nativeFlush(handle: Long): FloatArray?
    private external fun nativeSignalPath(handle: Long): String?
    private external fun nativeDestroy(handle: Long)

    companion object {
        const val LIBRARY = "encore_cadenza"
        private val EMPTY = FloatArray(0)

        /** [process] / [fetch]: the engine failed or went away (lib.rs `FAILED`). */
        const val FAILED = Int.MIN_VALUE

        /** [process] / [fetch]: no engine on this instance (nothing was processed). */
        const val CLOSED = Int.MIN_VALUE + 1

        /** `true` when `libencore_cadenza.so` loaded (it is only in APKs built with cargo-ndk). */
        @JvmStatic
        val available: Boolean = try {
            System.loadLibrary(LIBRARY)
            true
        } catch (_: Throwable) {
            false
        }

        /** The filters and dithers on offer as JSON `{filters: [[id, label]…], dithers: [[id, label]…], default_filter, default_dither}`; "" without the library. */
        @JvmStatic
        fun catalog(): String = if (available) (try { nativeCatalog() } catch (_: Throwable) { null }) ?: "" else ""

        /** Static JNI entry point (public only because JNI links to the outer class's static method). */
        @JvmStatic
        external fun nativeCatalog(): String?

        /**
         * The equalizer graph's curves for `bandsJson` (`[{type, freq, gain, q}]`) at `points` frequencies from 20 Hz to
         * 20 kHz: `{freqs, total, bands: [[dB…]…], handles: [dB…]}` as JSON; "" without the library.
         */
        @JvmStatic
        fun eqCurve(bandsJson: String, points: Int): String =
            if (available) (try { nativeEqCurve(bandsJson, points) } catch (_: Throwable) { null }) ?: "" else ""

        /** Static JNI entry point (public only because JNI links to the outer class's static method). */
        @JvmStatic
        external fun nativeEqCurve(bandsJson: String, points: Int): String?
    }
}
