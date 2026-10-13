package app.encore.android.playback

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * The DSP that runs on this phone between ExoPlayer's decoder and the audio output (shared seam, android-m2m3
 * contract). Interleaved float PCM in and out; `configure` returns the output sample rate (the input rate when
 * nothing resamples). The parameters are one JSON document, see [app.encore.android.data.prefs.PlaybackPrefs.engineParams]:
 *
 * `{"fx_on": bool, "preamp_db": number, "bands": [{"type","freq","gain","q"}…], "crossfeed": "off|light|medium|strong",
 *   "balance": -100..100, "mono": bool, "dsp_filter": "", "dsp_dither": "tpdf", "dsp_rate": "device|auto|<hz>"}`
 *
 * Implementations: [KotlinDspEngine] (always there) and `app.encore.android.cadenza.CadenzaEngine` (the Rust
 * cadenza-dsp crate through JNI, used when its native library is in the APK).
 */
interface DspEngine {
    /** Prepares for a stream of `channels` interleaved channels at `sampleRate` Hz; returns the output sample rate. */
    fun configure(sampleRate: Int, channels: Int): Int

    fun setParams(json: String)

    /** Processes `frames` frames from `input` (interleaved) and returns the processed frames (interleaved; may be a different length). */
    fun process(input: FloatArray, frames: Int): FloatArray

    /** Whatever is still in the engine (filter tails) at the end of a stream. */
    fun flush(): FloatArray

    /** A short description of what the engine does right now (for the Signal path sheet). */
    fun signalPath(): String

    fun release()
}

/** Picks the engine: Cadenza when its class and native library are available, the Kotlin one otherwise. */
object DspEngines {
    const val CADENZA_CLASS = "app.encore.android.cadenza.CadenzaEngine"

    fun create(): DspEngine {
        try {
            val cls = Class.forName(CADENZA_CLASS)
            val engine = cls.getDeclaredConstructor().newInstance() as DspEngine
            // a constructor that loads the library in try/catch leaves a dead engine behind: configure() tells
            engine.configure(48000, 2)
            engine.release()
            return cls.getDeclaredConstructor().newInstance() as DspEngine
        } catch (_: Throwable) {
            return KotlinDspEngine()
        }
    }
}

/**
 * The Kotlin implementation of the web player's effects graph (player.js applyFx): preamp → biquad EQ bands →
 * headphone crossfeed (a low-passed, 0.3 ms delayed, quieter copy of each channel into the other) → mono →
 * balance (Web Audio's equal-power stereo panner). No resampling: the output rate is the input rate.
 */
class KotlinDspEngine : DspEngine {
    private var rate = 48000
    private var channels = 2
    private var paramsJson = ""
    private var out = FloatArray(0)

    // settings
    private var on = false
    private var preamp = 1.0f
    private var bands: List<BandSpec> = emptyList()
    private var crossfeed = "off"
    private var balance = 0
    private var mono = false

    // state
    private var eq: Array<Biquad> = emptyArray()
    private var xfeedLp: Array<Biquad> = emptyArray()
    private var xfeedDelay: Array<FloatArray> = emptyArray()
    private var xfeedPos = 0
    private var xfeedGain = 0f
    private var xfeedTrim = 1f

    private data class BandSpec(val type: String, val freq: Double, val gain: Double, val q: Double)

    override fun configure(sampleRate: Int, channels: Int): Int {
        rate = sampleRate.coerceAtLeast(8000)
        this.channels = channels.coerceIn(1, 8)
        rebuild()
        return rate
    }

    override fun setParams(json: String) {
        if (json == paramsJson) return
        paramsJson = json
        try {
            val o = Json.parseToJsonElement(json) as? JsonObject ?: JsonObject(emptyMap())
            on = (o["fx_on"] as? JsonPrimitive)?.content == "true"
            preamp = 10.0.pow(((o["preamp_db"] as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0) / 20.0).toFloat()
            bands = (o["bands"] as? JsonArray)?.mapNotNull { e ->
                val b = e as? JsonObject ?: return@mapNotNull null
                BandSpec(
                    (b["type"] as? JsonPrimitive)?.content ?: "peaking",
                    (b["freq"] as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 1000.0,
                    (b["gain"] as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0,
                    (b["q"] as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 1.0,
                )
            } ?: emptyList()
            crossfeed = (o["crossfeed"] as? JsonPrimitive)?.content ?: "off"
            balance = ((o["balance"] as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0).toInt().coerceIn(-100, 100)
            mono = (o["mono"] as? JsonPrimitive)?.content == "true"
        } catch (_: Exception) {
            on = false
        }
        rebuild()
    }

    private fun rebuild() {
        eq = Array(bands.size * channels) { i -> Biquad().also { it.set(bands[i / channels], rate) } }
        val stereo = channels == 2 && crossfeed != "off" && !mono
        if (stereo) {
            xfeedLp = Array(2) { Biquad().also { it.set(BandSpec("lowpass", 700.0, 0.0, 0.5), rate) } }
            val delay = Math.round(0.0003 * rate).toInt().coerceAtLeast(1)
            xfeedDelay = Array(2) { FloatArray(delay) }
            xfeedPos = 0
            val db = when (crossfeed) { "light" -> -9.0; "strong" -> -4.5; else -> -6.0 }
            xfeedGain = 10.0.pow(db / 20.0).toFloat()
            xfeedTrim = 1f / (1f + xfeedGain)
        } else {
            xfeedLp = emptyArray()
            xfeedDelay = emptyArray()
        }
    }

    override fun process(input: FloatArray, frames: Int): FloatArray {
        val n = frames * channels
        if (out.size < n) out = FloatArray(n)
        if (!on) {
            System.arraycopy(input, 0, out, 0, n)
            return if (out.size == n) out else out.copyOf(n)
        }
        val ch = channels
        val nb = bands.size
        val pan = balance / 100.0
        val x = if (pan <= 0) pan + 1 else pan
        val gl = cos(x * PI / 2).toFloat()
        val gr = sin(x * PI / 2).toFloat()
        val xf = xfeedLp.isNotEmpty()
        val dl = if (xf) xfeedDelay[0].size else 0
        var i = 0
        for (f in 0 until frames) {
            // preamp + EQ, per channel
            for (c in 0 until ch) {
                var v = input[i + c] * preamp
                for (b in 0 until nb) v = eq[b * ch + c].tick(v)
                out[i + c] = v
            }
            if (ch == 2) {
                var l = out[i]
                var r = out[i + 1]
                if (xf) {
                    // each channel gets a low-passed, slightly delayed, quieter copy of the other one
                    val dL = xfeedDelay[0]
                    val dR = xfeedDelay[1]
                    val oldL = dL[xfeedPos]
                    val oldR = dR[xfeedPos]
                    dL[xfeedPos] = xfeedLp[0].tick(l)
                    dR[xfeedPos] = xfeedLp[1].tick(r)
                    xfeedPos = (xfeedPos + 1) % dl
                    l = (l + oldR * xfeedGain) * xfeedTrim
                    r = (r + oldL * xfeedGain) * xfeedTrim
                }
                if (mono) {
                    val m = (l + r) * 0.5f
                    l = m
                    r = m
                }
                if (balance != 0) {
                    // Web Audio StereoPannerNode, stereo input (equal power)
                    if (pan <= 0) {
                        val nl = l + r * gl
                        val nr = r * gr
                        l = nl; r = nr
                    } else {
                        val nr = r + l * gr
                        val nl = l * gl
                        l = nl; r = nr
                    }
                }
                out[i] = l
                out[i + 1] = r
            }
            i += ch
        }
        return if (out.size == n) out else out.copyOf(n)
    }

    override fun flush(): FloatArray {
        for (b in eq) b.reset()
        for (b in xfeedLp) b.reset()
        for (d in xfeedDelay) d.fill(0f)
        return FloatArray(0)
    }

    override fun signalPath(): String = if (on) "Effects in Kotlin on this phone" else "Effects off"

    override fun release() {
        eq = emptyArray()
        xfeedLp = emptyArray()
        xfeedDelay = emptyArray()
    }

    /** Direct form I biquad with the Web Audio (RBJ cookbook) coefficient formulas. */
    private class Biquad {
        private var b0 = 1.0; private var b1 = 0.0; private var b2 = 0.0; private var a1 = 0.0; private var a2 = 0.0
        private var x1 = 0.0; private var x2 = 0.0; private var y1 = 0.0; private var y2 = 0.0

        fun reset() { x1 = 0.0; x2 = 0.0; y1 = 0.0; y2 = 0.0 }

        fun tick(input: Float): Float {
            val x0 = input.toDouble()
            val y0 = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
            x2 = x1; x1 = x0; y2 = y1; y1 = y0
            return y0.toFloat()
        }

        fun set(b: BandSpec, rate: Int) {
            val freq = b.freq.coerceIn(10.0, rate / 2.0 - 1.0)
            val w0 = 2.0 * PI * freq / rate
            val cw = cos(w0)
            val sw = sin(w0)
            val a = 10.0.pow(b.gain / 40.0)
            var nb0: Double; var nb1: Double; var nb2: Double; var na0: Double; var na1: Double; var na2: Double
            when (b.type) {
                "lowpass", "highpass" -> {
                    // Web Audio: Q in dB for these two
                    val q = 10.0.pow(b.q / 20.0).coerceAtLeast(1e-4)
                    val alpha = sw / (2 * q)
                    if (b.type == "lowpass") {
                        nb0 = (1 - cw) / 2; nb1 = 1 - cw; nb2 = (1 - cw) / 2
                    } else {
                        nb0 = (1 + cw) / 2; nb1 = -(1 + cw); nb2 = (1 + cw) / 2
                    }
                    na0 = 1 + alpha; na1 = -2 * cw; na2 = 1 - alpha
                }
                "lowshelf", "highshelf" -> {
                    // shelves ignore Q (slope S = 1)
                    val alpha = sw / 2 * sqrt((a + 1 / a) * (1 / 1.0 - 1) + 2)
                    val sa = 2 * sqrt(a) * alpha
                    if (b.type == "lowshelf") {
                        nb0 = a * ((a + 1) - (a - 1) * cw + sa)
                        nb1 = 2 * a * ((a - 1) - (a + 1) * cw)
                        nb2 = a * ((a + 1) - (a - 1) * cw - sa)
                        na0 = (a + 1) + (a - 1) * cw + sa
                        na1 = -2 * ((a - 1) + (a + 1) * cw)
                        na2 = (a + 1) + (a - 1) * cw - sa
                    } else {
                        nb0 = a * ((a + 1) + (a - 1) * cw + sa)
                        nb1 = -2 * a * ((a - 1) + (a + 1) * cw)
                        nb2 = a * ((a + 1) + (a - 1) * cw - sa)
                        na0 = (a + 1) - (a - 1) * cw + sa
                        na1 = 2 * ((a - 1) - (a + 1) * cw)
                        na2 = (a + 1) - (a - 1) * cw - sa
                    }
                }
                "notch" -> {
                    val alpha = sw / (2 * b.q.coerceAtLeast(1e-4))
                    nb0 = 1.0; nb1 = -2 * cw; nb2 = 1.0
                    na0 = 1 + alpha; na1 = -2 * cw; na2 = 1 - alpha
                }
                else -> {
                    // peaking
                    val alpha = sw / (2 * b.q.coerceAtLeast(1e-4))
                    nb0 = 1 + alpha * a; nb1 = -2 * cw; nb2 = 1 - alpha * a
                    na0 = 1 + alpha / a; na1 = -2 * cw; na2 = 1 - alpha / a
                }
            }
            b0 = nb0 / na0; b1 = nb1 / na0; b2 = nb2 / na0; a1 = na1 / na0; a2 = na2 / na0
            reset()
        }
    }
}
