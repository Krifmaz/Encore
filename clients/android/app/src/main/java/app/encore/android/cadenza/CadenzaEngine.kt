package app.encore.android.cadenza

import app.encore.android.core.parseJsonObject
import app.encore.android.core.str
import java.nio.Buffer
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer

/**
 * What the engine does to one stream besides the effects: upsample to [outRate] Hz (0 = keep the song's rate) with
 * the filter and dither ids of [CadenzaCatalog] ("" = Cadenza's defaults, sinc-l-64k and TPDF).
 */
data class EngineSetup(val outRate: Int = 0, val filterId: String = "", val ditherId: String = "")

/**
 * Cadenza on the phone: the sound effects of Settings → Playback (preamp → EQ → crossfeed → mono → balance, the web
 * player's graph) and, when the [EngineSetup] asks for it, upsampling ("Cadenza engine · this phone"), through the
 * cadenza-dsp crate (the server's engine) behind the JNI binding [CadenzaNative] of the :cadenza module. The playback
 * service's effects processor holds one; the song transitions' second player another (effects only).
 *
 * Without `libencore_cadenza.so` in the APK (a build without cargo-ndk) [configure] throws, and the processor passes
 * the sound through untouched: there is no second implementation of the effects.
 *
 * Audio goes through two direct buffers this object owns and reuses: the processor writes a chunk into [input], and
 * [process] runs it through the engine in place (effects only) or into a second buffer (upsampling, the output is
 * longer). Upsampled output is dithered by Cadenza to 24-bit steps (the processor hands it on as 32-bit PCM).
 * [configure], [input], [process] and [flush] run on the playback thread; [signalPath] and [release] may come from
 * another thread (the native side keeps engines in a registry, see CadenzaNative).
 */
class CadenzaEngine(
    /** where long filter designs are cached (context.cacheDir/cadenza-fir); "" = memory only */
    private val cacheDir: String = "",
) {
    private val native = CadenzaNative()
    @Volatile private var lastParams = ""
    private var channels = 2
    private var resampling = false

    private var io: ByteBuffer = direct(0)
    private var ioFloats: FloatBuffer = io.asFloatBuffer()
    private var out: ByteBuffer? = null
    private var outFloats: FloatBuffer? = null

    /**
     * Builds the engine for a stream of `channels` interleaved channels at `sampleRate` Hz and returns the output
     * rate. Throws when the library is missing from this APK or Cadenza can't be built for the format.
     */
    fun configure(sampleRate: Int, channels: Int, setup: EngineSetup = EngineSetup()): Int {
        if (!CadenzaNative.available) throw IllegalStateException("libencore_cadenza.so is not in this APK")
        val rate = native.create(sampleRate, channels, setup.filterId, setup.ditherId, setup.outRate, cacheDir)
        if (rate <= 0) throw IllegalStateException("Cadenza could not be built for $channels ch at $sampleRate Hz")
        val p = lastParams
        if (p.isNotEmpty()) native.setParams(p)
        this.channels = channels.coerceAtLeast(1)
        resampling = rate != sampleRate
        return rate
    }

    /** The effects as the JSON document `PlaybackPrefs.engineParams()` makes. */
    fun setParams(json: String) {
        lastParams = json
        native.setParams(json)
    }

    /**
     * The buffer to put the next chunk in: `samples` interleaved floats from index 0 (absolute puts). Valid until
     * the next call; grows (and is then reused) when a chunk is larger than any before.
     */
    fun input(samples: Int): FloatBuffer {
        if (ioFloats.capacity() < samples) {
            io = direct(samples)
            ioFloats = io.asFloatBuffer()
        }
        span(ioFloats, ioFloats.capacity())
        return ioFloats
    }

    /**
     * Runs the `frames` frames put into [input] through the engine. Returns the output as floats from index 0 to the
     * buffer's limit: the input buffer itself, processed in place, when the engine keeps the rate; a second buffer
     * when it upsamples. Without an engine (no library, or released) the input buffer comes back untouched (the same
     * object [input] returned); null when the engine failed or went away under us (the chunk is lost).
     */
    fun process(frames: Int): FloatBuffer? {
        val samples = frames * channels
        var target: ByteBuffer? = null
        if (resampling) {
            // the output is longer than the input: the buffer starts at twice the input; a longer block grows it
            // once (below) and the grown buffer is kept for the next chunks
            val o = out
            target = if (o != null && o.capacity() >= samples * 4) o else grow(samples * 2)
        }
        var n = native.process(io, frames, target)
        when {
            n == CadenzaNative.CLOSED -> {
                span(ioFloats, samples.coerceAtMost(ioFloats.capacity()))
                return ioFloats
            }
            n == CadenzaNative.FAILED -> return null
            n < 0 -> {
                // more output than the buffer holds (a resampler block): a larger buffer, then the same output again
                val b = grow(-n)
                target = b
                n = native.fetch(b)
                if (n < 0) return null
            }
        }
        val view = if (target == null) ioFloats else outFloats ?: return null
        span(view, n)
        return view
    }

    /** Whatever is still in the engine (filter tails) at the end of a stream; resets the filters. */
    fun flush(): FloatArray = native.flush()

    /** One line for the Signal path sheet, e.g. "Cadenza on this phone · EQ: 2 bands, preamp -6 dB · Crossfeed: medium". */
    fun signalPath(): String {
        val o = parseJsonObject(native.signalPathJson())
            ?: return if (CadenzaNative.available) DEFAULT_LINE else ""
        return o.str("summary").ifEmpty { DEFAULT_LINE }
    }

    fun release() {
        native.destroy()
    }

    /** A larger output buffer for at least `samples` floats (kept for the next chunks). */
    private fun grow(samples: Int): ByteBuffer {
        val b = direct(samples + samples / 2)
        out = b
        outFloats = b.asFloatBuffer()
        return b
    }

    companion object {
        private const val DEFAULT_LINE = "Cadenza on this phone"

        /** `true` when the native library loaded (APKs built with cargo-ndk). */
        val available: Boolean get() = CadenzaNative.available

        /** `{filters: [[id, label]…], dithers: [[id, label]…], default_filter, default_dither}` as JSON, "" without the library. */
        fun catalog(): String = CadenzaNative.catalog()

        /** The equalizer graph's curves (Cadenza's own response, so the graph shows what plays); "" without the library. */
        fun eqCurve(bandsJson: String, points: Int): String = CadenzaNative.eqCurve(bandsJson, points)

        /**
         * Position 0, limit `n`. Through java.nio.Buffer: the FloatBuffer overrides with their own return type exist
         * only from Android 14 on, so calling them would fail on older phones.
         */
        private fun span(b: Buffer, n: Int) {
            b.clear()
            b.limit(n)
        }

        /** A direct buffer for `floats` floats in the platform's byte order (what the native side reads). */
        private fun direct(floats: Int): ByteBuffer =
            ByteBuffer.allocateDirect(floats.coerceAtLeast(0) * 4).order(ByteOrder.nativeOrder())
    }
}
