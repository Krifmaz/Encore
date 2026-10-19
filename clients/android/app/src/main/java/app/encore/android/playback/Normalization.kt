package app.encore.android.playback

import app.encore.android.data.api.TrackGain
import app.encore.android.data.prefs.NormPrefs
import java.util.Locale
import kotlin.math.log10
import kotlin.math.pow

/**
 * What normalization does to one song: [db] applied (0 when nothing is), [basis] "track" | "album" ("" when nothing
 * is known or it is off), [limited] when "Prevent clipping" lowered it, [requestedDb] what the tags asked for.
 */
data class NormDecision(
    val on: Boolean,
    val db: Double,
    val basis: String,
    val limited: Boolean,
    val requestedDb: Double,
    val target: Int,
) {
    /** The factor the samples are multiplied by (1 = untouched). */
    val linear: Float get() = if (!on || basis.isEmpty() || db == 0.0) 1f else Normalization.linear(db).toFloat()

    /** Normalization is on but the server knows nothing about this song's loudness. */
    val noData: Boolean get() = on && basis.isEmpty()

    /** The Signal path's line: "−3.2 dB (album gain, target −14 LUFS)". */
    fun line(): String = if (!on) "" else if (basis.isEmpty()) "No loudness data for this song" else
        "${Normalization.dbText(db)} ($basis gain, target ${Normalization.lufsText(target)})"

    companion object {
        val OFF = NormDecision(on = false, db = 0.0, basis = "", limited = false, requestedDb = 0.0, target = NormPrefs.DEFAULT_TARGET)
    }
}

/**
 * The gain math, the same on the web (player/fx.rs), the desktop and here:
 *
 *     applied dB = rg_gain + (target − (−18))
 *
 * where rg_gain is the song's (or album's) ReplayGain in dB relative to −18 LUFS and target the chosen loudness in
 * LUFS: a song measured at −10 LUFS has rg_gain −8; at target −14 it is played at −8 + 4 = −4 dB, i.e. at −14 LUFS.
 * Album mode takes the album gain (the track gain when the album has none), Track mode the track gain (the album
 * gain when the track has none). "Prevent clipping" limits the gain so that peak × gain ≤ 1 (gain ≤ −20·log10(peak)
 * dB); with no peak known it allows no boost above 0 dB. The result is kept within −30…+15 dB, so a broken tag can't
 * blast the speakers.
 */
object Normalization {
    const val REFERENCE_LUFS = -18.0
    const val MIN_DB = -30.0
    const val MAX_DB = 15.0

    /** dB → the factor the samples are multiplied by. */
    fun linear(db: Double): Double = 10.0.pow(db / 20.0)

    /** The gain for a ReplayGain value at a target loudness (the formula above, before clipping and limits). */
    fun appliedDb(rgGain: Double, target: Int): Double = rgGain + (target - REFERENCE_LUFS)

    /** The most gain a song with this linear peak takes without clipping; 0 dB when the peak is unknown. */
    fun clipCeilingDb(peak: Double?): Double = if (peak == null || peak <= 0.0) 0.0 else -20.0 * log10(peak)

    fun decide(prefs: NormPrefs, gain: TrackGain?): NormDecision {
        if (!prefs.on) return NormDecision.OFF
        val p = prefs.clean()
        val g = gain ?: return NormDecision(on = true, db = 0.0, basis = "", limited = false, requestedDb = 0.0, target = p.target)
        val useAlbum = (p.mode == "album" && g.album != null) || g.track == null
        val rg = (if (useAlbum) g.album else g.track) ?: return NormDecision(on = true, db = 0.0, basis = "", limited = false, requestedDb = 0.0, target = p.target)
        val peak = if (useAlbum) (g.albumPeak ?: g.trackPeak) else (g.trackPeak ?: g.albumPeak)
        val wanted = appliedDb(rg, p.target)
        var db = wanted
        var limited = false
        if (p.clip) {
            val ceiling = clipCeilingDb(peak)
            if (db > ceiling) {
                db = ceiling
                limited = true
            }
        }
        db = db.coerceIn(MIN_DB, MAX_DB)
        return NormDecision(on = true, db = db, basis = if (useAlbum) "album" else "track", limited = limited, requestedDb = wanted, target = p.target)
    }

    /** −3.2 → "−3.2 dB", 4 → "+4.0 dB" (a real minus sign, one decimal). */
    fun dbText(db: Double): String {
        val r = Math.round(db * 10.0) / 10.0
        val s = String.format(Locale.US, "%.1f", Math.abs(r))
        return when {
            r > 0 -> "+$s dB"
            r < 0 -> "−$s dB"
            else -> "0.0 dB"
        }
    }

    /** −14 → "−14 LUFS". */
    fun lufsText(v: Int): String = (if (v < 0) "−${-v}" else "$v") + " LUFS"
}
