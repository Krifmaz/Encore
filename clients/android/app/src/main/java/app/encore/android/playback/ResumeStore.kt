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

    data class Snapshot(val tracks: List<Track>, val index: Int, val positionMs: Long)

    fun save(tracks: List<Track>, index: Int, positionMs: Long) {
        val arr = JsonArray(tracks.take(MAX).map { toJson(it) })
        prefs.edit()
            .putString(KEY_QUEUE, arr.toString())
            .putInt(KEY_INDEX, index)
            .putLong(KEY_POSITION, positionMs)
            .apply()
    }

    fun savePosition(index: Int, positionMs: Long) {
        prefs.edit().putInt(KEY_INDEX, index).putLong(KEY_POSITION, positionMs).apply()
    }

    fun load(): Snapshot? {
        val text = prefs.getString(KEY_QUEUE, null) ?: return null
        val arr = try { Json.parseToJsonElement(text) as? JsonArray } catch (_: Exception) { null } ?: return null
        val tracks = arr.mapNotNull { (it as? JsonObject)?.let { o -> Track.from(o) } }
        if (tracks.isEmpty()) return null
        val index = prefs.getInt(KEY_INDEX, 0).coerceIn(0, tracks.size - 1)
        return Snapshot(tracks, index, prefs.getLong(KEY_POSITION, 0L).coerceAtLeast(0L))
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
    }
}
