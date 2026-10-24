package app.encore.android.features.library

import app.encore.android.core.int
import app.encore.android.core.str
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject

/**
 * "Get the rest of the album" (the album page's ⋯ menu) for albums MusicBrainz doesn't list: the pure parts – where the
 * lookup found the album, the release a catalogue's answer opens as, and the words of the toasts. The same words as
 * web-rs/src/pages/rest_of_album.rs and the desktop's rest_of_album.rs.
 */
object RestOfAlbumRules {
    /** Where GET /api/discover/album-lookup found the album. */
    enum class Found {
        /** a MusicBrainz release group (`rgid`): its catalog page */
        MUSICBRAINZ,
        /** Deezer, Apple Music or Discogs: that catalogue's track list */
        ELSEWHERE,
        /** no catalogue: type the songs in, or search the download sources for the album */
        NONE,
    }

    /** Where the lookup's answer [r] says the album was found (an older server's answer: by its `rgid` alone). */
    fun found(r: JsonObject): Found = when {
        r.str("rgid").isNotEmpty() -> Found.MUSICBRAINZ
        r.str("found") == "elsewhere" -> Found.ELSEWHERE
        else -> Found.NONE
    }

    /** How many songs a catalogue's answer [r] lists. */
    fun trackCount(r: JsonObject): Int = (r["tracks"] as? JsonArray)?.size ?: 0

    /** The note above a catalogue's track list: where it is from. */
    fun sourceNote(source: String): String = "Not on MusicBrainz – track list from ${source.ifEmpty { "another catalogue" }}."

    /** The words when no catalogue has [album] and the server sent no [message]. */
    fun notFound(album: String, message: String): String = message.ifEmpty { "No catalogue lists “$album”." }

    /** The button that requests every missing song. */
    fun requestAllLabel(missing: Int): String = "Request the $missing missing song${if (missing == 1) "" else "s"}"

    /** The toast for POST /api/discover/album-rest/request's answer [r]. */
    fun requestedText(r: JsonObject): String {
        val n = r.int("requested")
        val already = r.int("already_requested")
        if (n == 0) return "Every missing song is already requested"
        val more = if (already > 0) " ($already already requested)" else ""
        val wait = if (r.str("status") == "pending") " – waiting for approval" else ""
        return "Requested $n song${if (n == 1) "" else "s"}$more$wait"
    }
}
