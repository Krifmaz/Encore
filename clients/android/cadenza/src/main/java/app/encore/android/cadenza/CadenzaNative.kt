package app.encore.android.cadenza

/**
 * The JNI binding of `libencore_cadenza.so` (clients/android/cadenza/rust): cadenza-dsp on the phone. One
 * instance owns one native engine for one stream format; [create] builds it, [destroy] frees it. The JNI symbol
 * names (`Java_app_encore_android_cadenza_CadenzaNative_native…`) are pinned by a test in the Rust crate, so this
 * class and package must not be renamed without it.
 *
 * The library may be missing from the APK (a build without cargo-ndk), so [available] is checked first and
 * every call on an instance without a handle is a no-op. The DspEngine adapter the player uses is
 * `app.encore.android.cadenza.CadenzaEngine` in the app module.
 */
class CadenzaNative {
    private var handle: Long = 0L

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
        handle = nativeCreate(sampleRate, channels, filterId, ditherId, outRate, cacheDir)
        if (handle == 0L) return 0
        val rate = nativeOutputRate(handle)
        if (rate <= 0) {
            destroy()
            return 0
        }
        return rate
    }

    /** The effects as the JSON document `PlaybackPrefs.engineParams()` makes. */
    fun setParams(json: String) {
        if (handle != 0L) nativeSetParams(handle, json)
    }

    /** Interleaved float PCM in, interleaved float PCM (at the output rate) out; `input` itself when closed. */
    fun process(input: FloatArray, frames: Int): FloatArray {
        if (handle == 0L) return input
        return nativeProcess(handle, input, frames) ?: EMPTY
    }

    /** The tail at the end of a stream; resets the filters. */
    fun flush(): FloatArray {
        if (handle == 0L) return EMPTY
        return nativeFlush(handle) ?: EMPTY
    }

    /** JSON: `{engine, summary, fx_on, input_rate, output_rate, latency_seconds, eq_bands, crossfeed, balance, mono, pipeline}`. */
    fun signalPathJson(): String {
        if (handle == 0L) return ""
        return nativeSignalPath(handle) ?: ""
    }

    fun destroy() {
        val h = handle
        handle = 0L
        if (h != 0L) nativeDestroy(h)
    }

    private external fun nativeCreate(sampleRate: Int, channels: Int, filterId: String, ditherId: String, outRate: Int, cacheDir: String): Long
    private external fun nativeOutputRate(handle: Long): Int
    private external fun nativeSetParams(handle: Long, json: String)
    private external fun nativeProcess(handle: Long, input: FloatArray, frames: Int): FloatArray?
    private external fun nativeFlush(handle: Long): FloatArray?
    private external fun nativeSignalPath(handle: Long): String?
    private external fun nativeDestroy(handle: Long)

    companion object {
        const val LIBRARY = "encore_cadenza"
        private val EMPTY = FloatArray(0)

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
    }
}
