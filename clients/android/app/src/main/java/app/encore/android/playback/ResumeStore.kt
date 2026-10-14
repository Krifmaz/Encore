package app.encore.android.playback

import android.content.Context
import app.encore.android.data.api.Track
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * The last queue, position and index, kept on the phone so "resume" (lock screen, Android Auto, after a reboot)
 * needs no server round trip. Songs are stored with their metadata, not just ids.
 */
class ResumeStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("encore_playback", Context.MODE_PRIVATE)

    /** `savedAtSec`: when this phone last played from it (epoch seconds; 0 = unknown, e.g. saved by an older version). */
    data class Snapshot(val tracks: List<Track>, val index: Int, val positionMs: Long, val savedAtSec: Double = 0.0)

    /**
     * `touch`: this phone is playing from it, so it is newer than another device's spot from before now. A queue
     * put back paused at start saves without touching (it hasn't played here yet).
     */
    fun save(tracks: List<Track>, index: Int, positionMs: Long, touch: Boolean = true) {
        val arr = JsonArray(tracks.take(MAX).map { toJson(it) })
        val e = prefs.edit()
            .putString(KEY_QUEUE, arr.toString())
            .putInt(KEY_INDEX, index)
            .putLong(KEY_POSITION, positionMs)
        if (touch) e.putLong(KEY_SAVED_AT, System.currentTimeMillis())
        e.apply()
    }

    fun savePosition(index: Int, positionMs: Long, touch: Boolean = true) {
        val e = prefs.edit().putInt(KEY_INDEX, index).putLong(KEY_POSITION, positionMs)
        if (touch) e.putLong(KEY_SAVED_AT, System.currentTimeMillis())
        e.apply()
    }

    /** The `updated_at` of another device's spot you closed the "Continue …" bar for ("" = none). */
    var dismissedSpot: String
        get() = prefs.getString(KEY_DISMISSED, null) ?: ""
        set(v) { prefs.edit().putString(KEY_DISMISSED, v).apply() }

    fun load(): Snapshot? {
        val text = prefs.getString(KEY_QUEUE, null) ?: return null
        val arr = try { Json.parseToJsonElement(text) as? JsonArray } catch (_: Exception) { null } ?: return null
        val tracks = arr.mapNotNull { (it as? JsonObject)?.let { o -> Track.from(o) } }
        if (tracks.isEmpty()) return null
        val index = prefs.getInt(KEY_INDEX, 0).coerceIn(0, tracks.size - 1)
        return Snapshot(tracks, index, prefs.getLong(KEY_POSITION, 0L).coerceAtLeast(0L), prefs.getLong(KEY_SAVED_AT, 0L) / 1000.0)
    }

    fun clear() {
        prefs.edit().clear().apply()
    }

    private fun toJson(t: Track): JsonObject {
        val m = LinkedHashMap<String, JsonElement>()
        m["id"] = JsonPrimitive(t.id)
        m["title"] = JsonPrimitive(t.title)
        m["artist"] = JsonPrimitive(t.artist)
        m["album"] = JsonPrimitive(t.album)
        m["albumartist"] = JsonPrimitive(t.albumArtist)
        m["album_id"] = JsonPrimitive(t.albumId)
        m["track"] = JsonPrimitive(t.trackNo)
        m["disc"] = JsonPrimitive(t.disc)
        m["duration"] = JsonPrimitive(t.durationSec)
        m["format"] = JsonPrimitive(t.format)
        return JsonObject(m)
    }

    private companion object {
        const val MAX = 500
        const val KEY_QUEUE = "queue"
        const val KEY_INDEX = "index"
        const val KEY_POSITION = "position"
        const val KEY_SAVED_AT = "saved_at"
        const val KEY_DISMISSED = "spot_dismissed"
    }
}
