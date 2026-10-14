package app.encore.android.data.prefs

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.math.roundToInt

// A port of web/js/playback.js: the playback preferences, their validation (clean), the EQ presets and the
// adaptive-quality ladder. Field names mirror the JSON the web player stores on the account (prefs.playback).

/** One parametric EQ band: `type` is one of [EQ_TYPES], `freq` in Hz, `gain` in dB, `q` (dB for low/high-pass, as in Web Audio). */
data class EqBand(val type: String, val freq: Int, val gain: Double, val q: Double)

data class EqPreset(val id: String, val label: String, val preamp: Double, val bands: List<EqBand>)

/**
 * quality: "auto" – the original file; Encore measures the connection while it plays and steps the stream down
 *          (256 → 160 → 96 → 64 → 32 kbps) as far as it needs to keep playing, then back up
 *          "original" – always the original file, whatever the connection does
 *          "saver" – always the small 96 kbps stream (mobile data, slow links)
 * preload: buffer the next song while this one plays (off on very slow connections)
 * autoplay: when the queue ends, keep going with a shuffle of the whole library
 * resume: when the app opens, pick up where you left off (this phone's queue and spot, or another device's)
 * dsp: have the server upsample streams with Cadenza (only where an admin switched it on and allowed the account)
 * dspFilter: Cadenza upsampling filter id ("" = the server's default) · dspDither: dither / noise shaping id
 * dspRate: "device" (this device's output rate), "auto" (highest the server allows) or a rate in Hz as text
 * fxOn: sound effects on this device: the master switch for everything below except crossfade
 * eqPreset / eqBands / eqPreamp: parametric EQ (a named preset, or "custom" with your own bands), preamp in dB
 * crossfeed: headphone crossfeed "off" | "light" | "medium" | "strong"
 * balance: -100 (left) … +100 (right) · mono: sum both channels
 * crossfade: seconds the end of one song blends into the next (0 = off) · crossfadeAlbumGapless: no fade within an album
 */
data class PlaybackPrefs(
    val quality: String = "auto",
    val preload: Boolean = true,
    val autoplay: Boolean = true,
    val resume: Boolean = true,
    val dsp: Boolean = false,
    val dspFilter: String = "",
    val dspDither: String = "tpdf",
    val dspRate: String = "device",
    val fxOn: Boolean = false,
    val eqPreset: String = "flat",
    val eqBands: List<EqBand> = emptyList(),
    val eqPreamp: Double = 0.0,
    val crossfeed: String = "off",
    val balance: Int = 0,
    val mono: Boolean = false,
    val crossfade: Int = 0,
    val crossfadeAlbumGapless: Boolean = true,
) {
    /** The bands that play: a named preset's own, or your custom ones. */
    fun bands(): List<EqBand> =
        if (eqPreset == "custom") eqBands else (EQ_PRESETS.firstOrNull { it.id == eqPreset } ?: EQ_PRESETS[0]).bands

    /** Does any effect change the sound? (the master switch on and something set) */
    fun fxActive(): Boolean {
        if (!fxOn) return false
        return bands().isNotEmpty() || eqPreamp != 0.0 || crossfeed != "off" || balance != 0 || mono
    }

    /** "EQ: Bass boost, preamp -6 dB · Crossfeed: medium · Balance: L10 · Mono · Crossfade: 6 s" – "" when nothing applies. */
    fun effectsSummary(withCrossfade: Boolean = true): String {
        val parts = ArrayList<String>()
        if (fxActive()) {
            if (bands().isNotEmpty() || eqPreamp != 0.0) parts.add("EQ: ${presetLabel(eqPreset)}" + (if (eqPreamp != 0.0) ", preamp ${db(eqPreamp)}" else ""))
            if (crossfeed != "off" && !mono) parts.add("Crossfeed: $crossfeed")
            if (balance != 0) parts.add("Balance: ${balanceText(balance)}")
            if (mono) parts.add("Mono")
        }
        if (withCrossfade && crossfade > 0) parts.add("Crossfade: $crossfade s")
        return parts.joinToString(" · ")
    }

    /** The JSON the account stores (`prefs.playback`), the same keys as the web player. */
    fun toJson(): JsonObject {
        val m = LinkedHashMap<String, JsonElement>()
        m["quality"] = JsonPrimitive(quality)
        m["preload"] = JsonPrimitive(preload)
        m["autoplay"] = JsonPrimitive(autoplay)
        m["resume"] = JsonPrimitive(resume)
        m["dsp"] = JsonPrimitive(dsp)
        m["dsp_filter"] = JsonPrimitive(dspFilter)
        m["dsp_dither"] = JsonPrimitive(dspDither)
        m["dsp_rate"] = dspRate.toIntOrNull()?.let { JsonPrimitive(it) } ?: JsonPrimitive(dspRate)
        m["fx_on"] = JsonPrimitive(fxOn)
        m["eq_preset"] = JsonPrimitive(eqPreset)
        m["eq_bands"] = JsonArray(eqBands.map { b ->
            JsonObject(mapOf("type" to JsonPrimitive(b.type), "freq" to JsonPrimitive(b.freq), "gain" to JsonPrimitive(b.gain), "q" to JsonPrimitive(b.q)))
        })
        m["eq_preamp"] = JsonPrimitive(eqPreamp)
        m["crossfeed"] = JsonPrimitive(crossfeed)
        m["balance"] = JsonPrimitive(balance)
        m["mono"] = JsonPrimitive(mono)
        m["crossfade"] = JsonPrimitive(crossfade)
        m["crossfade_album_gapless"] = JsonPrimitive(crossfadeAlbumGapless)
        return JsonObject(m)
    }

    /** What the DSP engine gets (DspEngine.setParams): the effects as one JSON string. */
    fun engineParams(): String {
        val m = LinkedHashMap<String, JsonElement>()
        m["fx_on"] = JsonPrimitive(fxActive())
        m["preamp_db"] = JsonPrimitive(if (fxOn) eqPreamp else 0.0)
        m["bands"] = JsonArray((if (fxOn) bands() else emptyList()).map { b ->
            JsonObject(mapOf("type" to JsonPrimitive(b.type), "freq" to JsonPrimitive(b.freq), "gain" to JsonPrimitive(b.gain), "q" to JsonPrimitive(b.q)))
        })
        m["crossfeed"] = JsonPrimitive(if (fxOn && !mono) crossfeed else "off")
        m["balance"] = JsonPrimitive(if (fxOn) balance else 0)
        m["mono"] = JsonPrimitive(fxOn && mono)
        m["dsp_filter"] = JsonPrimitive(dspFilter)
        m["dsp_dither"] = JsonPrimitive(dspDither)
        m["dsp_rate"] = JsonPrimitive(dspRate)
        return JsonObject(m).toString()
    }

    companion object {
        val EQ_TYPES: List<Pair<String, String>> = listOf(
            "peaking" to "Peaking", "lowshelf" to "Low shelf", "highshelf" to "High shelf",
            "lowpass" to "Low-pass", "highpass" to "High-pass", "notch" to "Notch",
        )

        private fun b(type: String, freq: Int, gain: Double, q: Double) = EqBand(type, freq, gain, q)

        /** EQ presets: id, label, preamp (dB, keeps boosts from clipping) and bands – the web player's table. */
        val EQ_PRESETS: List<EqPreset> = listOf(
            EqPreset("flat", "Flat", 0.0, emptyList()),
            EqPreset("bass", "Bass boost", -6.0, listOf(b("lowshelf", 105, 6.0, 0.7), b("peaking", 60, 2.0, 1.0))),
            EqPreset("treble", "Treble boost", -5.0, listOf(b("highshelf", 6000, 5.0, 0.7))),
            EqPreset("vocal", "Vocal", -3.0, listOf(b("highpass", 80, 0.0, 0.7), b("peaking", 250, -2.0, 1.0), b("peaking", 3000, 3.0, 1.0), b("peaking", 5000, 2.0, 1.2))),
            EqPreset("loudness", "Loudness (V-shape)", -5.0, listOf(b("lowshelf", 90, 5.0, 0.7), b("peaking", 1000, -3.0, 0.7), b("highshelf", 10000, 4.0, 0.7))),
            EqPreset("classical", "Classical", -2.0, listOf(b("lowshelf", 80, 2.0, 0.7), b("peaking", 500, -1.0, 0.8), b("highshelf", 12000, 2.0, 0.7))),
            EqPreset("electronic", "Electronic", -5.0, listOf(b("peaking", 60, 5.0, 1.0), b("peaking", 400, -2.0, 1.0), b("peaking", 2500, 1.0, 1.0), b("highshelf", 10000, 4.0, 0.7))),
            EqPreset("podcast", "Podcast / voice", -4.0, listOf(b("highpass", 90, 0.0, 0.7), b("peaking", 300, -3.0, 1.0), b("peaking", 3000, 4.0, 1.0), b("lowpass", 12000, 0.0, 0.7))),
        )

        val CROSSFEED: List<Pair<String, String>> = listOf("off" to "Off", "light" to "Light", "medium" to "Medium", "strong" to "Strong")

        private val QUALITIES = setOf("auto", "original", "saver")
        private val TYPES = EQ_TYPES.map { it.first }.toSet()
        private val PRESET_IDS = EQ_PRESETS.map { it.id }.toSet() + "custom"

        fun presetLabel(id: String): String =
            if (id == "custom") "Custom" else (EQ_PRESETS.firstOrNull { it.id == id } ?: EQ_PRESETS[0]).label

        fun balanceText(v: Int): String = if (v == 0) "Centre" else if (v < 0) "L${-v}" else "R$v"

        private fun db(v: Double): String = "${if (v > 0) "+" else ""}${fmt(v)} dB"

        /** 6.0 → "6", -2.5 → "-2.5" (how JavaScript prints these numbers). */
        fun fmt(v: Double): String = if (v == v.toLong().toDouble()) v.toLong().toString() else v.toString()

        private fun num(v: JsonElement?, lo: Double, hi: Double, dflt: Double, step: Double = 0.0): Double {
            val n = (v as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content?.toDoubleOrNull() ?: return dflt
            if (n.isNaN() || n.isInfinite()) return dflt
            val c = n.coerceIn(lo, hi)
            return if (step > 0) Math.round(c / step) * step else c
        }

        private fun round(v: Double, digits: Int): Double {
            var f = 1.0
            repeat(digits) { f *= 10.0 }
            return Math.round(v * f) / f
        }

        private fun bool(v: JsonElement?): Boolean? = ((v as? JsonPrimitive)?.takeIf { it !is JsonNull })?.content?.let {
            when (it) { "true" -> true; "false" -> false; else -> null }
        }

        private fun text(v: JsonElement?): String? = (v as? JsonPrimitive)?.takeIf { it !is JsonNull && it.isString }?.content

        private fun cleanBand(e: JsonElement): EqBand {
            val x = e as? JsonObject ?: JsonObject(emptyMap())
            val type = text(x["type"])?.takeIf { it in TYPES } ?: "peaking"
            return EqBand(
                type = type,
                freq = num(x["freq"], 20.0, 20000.0, 1000.0).roundToInt(),
                gain = round(num(x["gain"], -24.0, 24.0, 0.0), 1),
                q = round(num(x["q"], 0.1, 18.0, 1.0), 2),
            )
        }

        /** playback.js `clean()`: defaults for what is missing, limits for what is there. */
        fun clean(p: JsonObject?): PlaybackPrefs {
            val o = p ?: JsonObject(emptyMap())
            val d = PlaybackPrefs()
            val rateRaw = o["dsp_rate"] as? JsonPrimitive
            val rate = when {
                rateRaw == null || rateRaw is JsonNull -> d.dspRate
                rateRaw.content == "device" || rateRaw.content == "auto" -> rateRaw.content
                else -> rateRaw.content.toDoubleOrNull()?.let { r -> if (r > 0 && r == Math.floor(r)) r.toLong().toString() else null } ?: d.dspRate
            }
            return PlaybackPrefs(
                quality = text(o["quality"])?.takeIf { it in QUALITIES } ?: d.quality,
                preload = bool(o["preload"]) != false,
                autoplay = bool(o["autoplay"]) != false,
                resume = bool(o["resume"]) != false,
                dsp = bool(o["dsp"]) == true,
                dspFilter = text(o["dsp_filter"]) ?: d.dspFilter,
                dspDither = text(o["dsp_dither"])?.takeIf { it.isNotEmpty() } ?: d.dspDither,
                dspRate = rate,
                fxOn = bool(o["fx_on"]) == true,
                eqPreset = text(o["eq_preset"])?.takeIf { it in PRESET_IDS } ?: "flat",
                eqBands = (o["eq_bands"] as? JsonArray)?.take(10)?.map { cleanBand(it) } ?: emptyList(),
                eqPreamp = round(num(o["eq_preamp"], -24.0, 12.0, 0.0), 1),
                crossfeed = text(o["crossfeed"])?.takeIf { c -> CROSSFEED.any { it.first == c } } ?: "off",
                balance = num(o["balance"], -100.0, 100.0, 0.0).roundToInt(),
                mono = bool(o["mono"]) == true,
                crossfade = num(o["crossfade"], 0.0, 12.0, 0.0).roundToInt(),
                crossfadeAlbumGapless = bool(o["crossfade_album_gapless"]) != false,
            )
        }
    }
}

/** Adaptive quality (Auto): the ladder and the choice of rung from the estimated bandwidth (playback.js). */
object Quality {
    /** Rungs in kbps, best first; 0 = the original file. */
    val RUNGS: List<Int> = listOf(0, 256, 160, 96, 64, 32)
    const val SAVER_KBPS = 96
    const val EWMA_ALPHA = 0.3

    /** The rungs a song can use: the original, then only the converted streams smaller than it. */
    fun ladder(origKbps: Int): List<Int> = RUNGS.filter { r -> r == 0 || origKbps <= 0 || r < origKbps }

    /** kbit/s a rung needs: the rung itself, the original's bitrate for the original (1411 when unknown). */
    fun needed(rung: Int, origKbps: Int): Int = if (rung == 0) (if (origKbps > 0) origKbps else 1411) else rung

    /** Highest rung that fits in 75 % of the estimate; the original while there is no estimate yet; 32 kbps at worst. */
    fun chooseRung(estimate: Double?, origKbps: Int): Int {
        val l = ladder(origKbps)
        if (estimate == null) return 0
        if (!(estimate > 0)) return l.last()
        return l.firstOrNull { r -> needed(r, origKbps) <= 0.75 * estimate } ?: l.last()
    }

    /** One rung down from `rung` (stays at the bottom). */
    fun stepDown(rung: Int, origKbps: Int): Int {
        val l = ladder(origKbps)
        val i = l.indexOf(rung)
        return l[minOf(l.size - 1, (if (i < 0) 0 else i) + 1)]
    }

    /** Exponentially weighted moving average of throughput samples (kbit/s); the first sample sets it. */
    fun ewma(prev: Double?, sample: Double): Double = if (prev == null) sample else prev + EWMA_ALPHA * (sample - prev)

    /** kbit/s from bytes moved in `seconds`; null for transfers under 0.25 s (too short to say anything). */
    fun rateOf(bytes: Long, seconds: Double): Double? = if (seconds < 0.25) null else (bytes * 8.0 / 1000.0) / seconds

    /** kbit/s → "0.25" / "4.2" / "12" (Mbit/s) */
    fun mbit(kbps: Double): String = when {
        kbps < 1000 -> String.format(java.util.Locale.US, "%.2f", kbps / 1000)
        kbps < 10000 -> String.format(java.util.Locale.US, "%.1f", kbps / 1000)
        else -> Math.round(kbps / 1000).toString()
    }

    /** 44100 → "44.1", 96000 → "96" */
    fun khz(hz: Int): String = PlaybackPrefs.fmt(Math.round(hz / 100.0) / 10.0)
}
