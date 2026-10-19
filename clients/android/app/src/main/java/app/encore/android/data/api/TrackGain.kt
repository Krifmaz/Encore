package app.encore.android.data.api

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * A song's loudness as the server knows it: the `gain` object of a track row (`{track, album, track_peak,
 * album_peak}`, sent only when something is known; each field may be missing). Gains are ReplayGain dB relative to
 * the −18 LUFS reference (R128 tags are converted by the server); peaks are linear (1.0 = full scale).
 */
data class TrackGain(val track: Double?, val album: Double?, val trackPeak: Double?, val albumPeak: Double?) {
    /** The four numbers for a Bundle (MediaItems extras); NaN = unknown. */
    fun toArray(): DoubleArray = doubleArrayOf(track ?: Double.NaN, album ?: Double.NaN, trackPeak ?: Double.NaN, albumPeak ?: Double.NaN)

    /** The row's own shape again (the resume snapshot keeps it). */
    fun toJson(): JsonObject {
        val m = LinkedHashMap<String, JsonElement>()
        track?.let { m["track"] = JsonPrimitive(it) }
        album?.let { m["album"] = JsonPrimitive(it) }
        trackPeak?.let { m["track_peak"] = JsonPrimitive(it) }
        albumPeak?.let { m["album_peak"] = JsonPrimitive(it) }
        return JsonObject(m)
    }

    companion object {
        private fun num(e: JsonElement?): Double? =
            (e as? JsonPrimitive)?.takeIf { it !is JsonNull && !it.isString }?.content?.toDoubleOrNull()?.takeIf { !it.isNaN() && !it.isInfinite() }

        /** The row's `gain` object; null when it is missing (an older server, or a song not measured yet) or empty. */
        fun from(e: JsonElement?): TrackGain? {
            val o = e as? JsonObject ?: return null
            val g = TrackGain(num(o["track"]), num(o["album"]), num(o["track_peak"]), num(o["album_peak"]))
            return if (g.track == null && g.album == null) null else g
        }

        /** What [toArray] wrote; null for a missing or empty array. */
        fun fromArray(a: DoubleArray?): TrackGain? {
            if (a == null || a.size < 4) return null
            fun v(x: Double): Double? = if (x.isNaN() || x.isInfinite()) null else x
            val g = TrackGain(v(a[0]), v(a[1]), v(a[2]), v(a[3]))
            return if (g.track == null && g.album == null) null else g
        }
    }
}
