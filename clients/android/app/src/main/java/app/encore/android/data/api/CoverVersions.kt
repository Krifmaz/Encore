package app.encore.android.data.api

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import java.util.concurrent.ConcurrentHashMap

/**
 * The server's `cover_v` for each album, picked up from every JSON answer (albums, songs and artists carry
 * it, see server/src/covers.rs). [EncoreApi.coverUrl] adds it as `&v=`, which makes the cover URL change
 * when the artwork changes and lets the server answer with `immutable` caching, so Coil's disk cache can
 * keep covers for good.
 */
object CoverVersions {
    private val versions = ConcurrentHashMap<String, Long>()

    /** The known version of an album's cover (0 = unknown). */
    fun of(albumId: String): Long = versions[albumId] ?: 0L

    /** Records every `cover_v` in a JSON answer. */
    fun note(e: JsonElement?) {
        when (e) {
            is JsonArray -> e.forEach { note(it) }
            is JsonObject -> {
                val v = (e["cover_v"] as? JsonPrimitive)?.content?.toLongOrNull() ?: 0L
                if (v > 0L) albumOf(e)?.let { versions[it] = v }
                e.values.forEach { if (it is JsonObject || it is JsonArray) note(it) }
            }
            else -> {}
        }
    }

    private fun JsonObject.text(key: String): String = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: ""

    /** The album whose cover the object shows: `album_id`, an artist's `cover_album`, or an album's own `id`. */
    private fun albumOf(o: JsonObject): String? {
        o.text("album_id").takeIf { it.isNotEmpty() }?.let { return it }
        o.text("cover_album").takeIf { it.isNotEmpty() }?.let { return it }
        val id = o.text("id")
        return id.takeIf { it.length == 16 && it.all { c -> c.isDigit() || c in 'a'..'f' || c in 'A'..'F' } && ("album" in o || "albumartist" in o) }
    }
}
