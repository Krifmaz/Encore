package app.encore.android.cadenza

import app.encore.android.playback.DspEngine
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Cadenza on the phone as the player's [DspEngine]: the cadenza-dsp crate (the same EQ code as the server) behind
 * the JNI binding [CadenzaNative] from the :cadenza module. `DspEngines.create()` finds this class by name and
 * tries `configure()` once: without `libencore_cadenza.so` in the APK (a build without cargo-ndk) that throws, and
 * the player falls back to its Kotlin engine.
 *
 * Effects: preamp → EQ → crossfeed → mono → balance, exactly the web player's graph. The output rate is the
 * input rate unless [outputRate] asks for Cadenza upsampling (M3: "Cadenza engine" settings), in which case
 * [filterId] / [ditherId] pick the design and the filters are cached in [cacheDir].
 */
class CadenzaEngine : DspEngine {
    private val native = CadenzaNative()
    private var lastParams = ""

    override fun configure(sampleRate: Int, channels: Int): Int {
        if (!CadenzaNative.available) throw IllegalStateException("libencore_cadenza.so is not in this APK")
        val out = native.create(sampleRate, channels, filterId, ditherId, outputRate, cacheDir)
        if (out <= 0) throw IllegalStateException("Cadenza could not be built for $channels ch at $sampleRate Hz")
        if (lastParams.isNotEmpty()) native.setParams(lastParams)
        return out
    }

    override fun setParams(json: String) {
        lastParams = json
        native.setParams(json)
    }

    override fun process(input: FloatArray, frames: Int): FloatArray = native.process(input, frames)

    override fun flush(): FloatArray = native.flush()

    /** One line for the Signal path sheet, e.g. "Cadenza on this phone · EQ: 2 bands, preamp -6 dB · Crossfeed: medium". */
    override fun signalPath(): String {
        val json = native.signalPathJson()
        if (json.isEmpty()) return if (CadenzaNative.available) "Cadenza on this phone" else ""
        return try {
            val o = Json.parseToJsonElement(json) as? JsonObject
            (o?.get("summary") as? JsonPrimitive)?.content ?: "Cadenza on this phone"
        } catch (_: Exception) {
            "Cadenza on this phone"
        }
    }

    /** The whole path as JSON (`engine, summary, input_rate, output_rate, latency_seconds, eq_bands, crossfeed, balance, mono, pipeline`). */
    fun signalPathJson(): String = native.signalPathJson()

    override fun release() {
        native.destroy()
    }

    companion object {
        /** `true` when the native library loaded (APKs built with cargo-ndk). */
        val available: Boolean get() = CadenzaNative.available

        /** Output rate in Hz for Cadenza upsampling on the phone; 0 (default) keeps the source rate. Applied at the next configure(). */
        @Volatile var outputRate: Int = 0

        /** Upsampling filter id from [catalog] ("" = Cadenza's default, sinc-l-64k). */
        @Volatile var filterId: String = ""

        /** Dither / noise-shaping id from [catalog] ("" = TPDF). */
        @Volatile var ditherId: String = ""

        /** Directory for cached FIR designs (context.cacheDir/cadenza-fir); "" = memory only. */
        @Volatile var cacheDir: String = ""

        /** `{filters: [[id, label]…], dithers: [[id, label]…], default_filter, default_dither}` as JSON, "" without the library. */
        fun catalog(): String = CadenzaNative.catalog()
    }
}
