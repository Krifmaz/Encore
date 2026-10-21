package app.encore.android.features.library

import app.encore.android.core.bool
import app.encore.android.core.obj
import app.encore.android.core.str
import kotlinx.serialization.json.JsonObject
import java.net.URI

/**
 * Someone else's shared playlist in my playlists (web-rs/src/pages/playlists.rs, the `linked` texts): "Add to my
 * playlists" puts the owner's playlist, live and read-only, in the signed-in user's Playlists through its share link
 * (POST /api/playlists/shared {link}). It always shows the owner's playlist as it is now; Stop sharing, a deleted
 * playlist or a disabled owner ends it, and the entry stays with "<owner> stopped sharing this playlist" and Remove.
 * Save a copy (POST /api/playlists/{id}/copy) makes an editable snapshot. Pure, so the unit tests check it.
 */
object PlaylistLinkRules {
    const val ADD_SHARED_TITLE = "Add a shared playlist"
    const val ADD_SHARED_HINT = "Paste a playlist link from someone on this server. You get their playlist as it is now, and it changes when they change it."
    const val PLACEHOLDER = "https://…/s/…"
    const val ADD = "Add to my playlists"
    const val ADDED = "Added to your playlists"
    const val ALREADY = "Already in your playlists"
    const val REMOVE = "Remove from my playlists"
    const val REMOVED = "Removed from your playlists"
    const val SAVE_COPY = "Save a copy"
    const val SAVED = "Saved to your playlists"
    const val EMPTY = "No songs here right now"

    /** "Playlist · by anna" / "Liked songs · by anna" (the phone shows it in capitals). */
    fun kicker(liked: Boolean, owner: String) = "${if (liked) "Liked songs" else "Playlist"} · by $owner"

    /** The line under the count while the link works. */
    fun hint(owner: String) = "Changes $owner makes show here. Save a copy to make your own."

    /** The entry once the link ended (stop sharing, a deleted playlist, a disabled or deleted owner). */
    fun stopped(owner: String) = "$owner stopped sharing this playlist"

    /** The Playlists tile's line: "12 tracks · by anna", or the ended line. */
    fun tileLine(total: Int, owner: String, active: Boolean) = if (active) "$total tracks · by $owner" else stopped(owner)

    /** The Remove confirm's text. */
    fun removeText(name: String) = "Remove “$name” from your playlists? You can add it again from its link."

    /** The owner's count beside the Shared mark: "" / "added by 1 person" / "added by 3 people". */
    fun addedBy(n: Int) = when {
        n <= 0 -> ""
        n == 1 -> "added by 1 person"
        else -> "added by $n people"
    }

    /** The toast after adding: a second add does nothing new. */
    fun addToast(added: Boolean) = if (added) ADDED else ALREADY

    /** An entry of an added playlist's ⋯ (LinkedPlaylistMenu builds them from [menuEntries], so the tests see what it shows). */
    enum class LinkedEntry(val label: String) { SAVE_COPY(PlaylistLinkRules.SAVE_COPY), REMOVE(PlaylistLinkRules.REMOVE) }

    /** The ⋯ of an added playlist, in order: an ended one has nothing to copy. */
    fun menuEntries(active: Boolean): List<LinkedEntry> =
        if (active) listOf(LinkedEntry.SAVE_COPY, LinkedEntry.REMOVE) else listOf(LinkedEntry.REMOVE)

    private val TOKEN = Regex("""https?://\S*?/(?:s|api/share)/([A-Za-z0-9_-]{4,64})(?![A-Za-z0-9_-])""")
    private val LINK = Regex("""https?://\S+""")

    /**
     * The token of an Encore link in shared text: https?://…/(s|api/share)/<token>; null when there is none. The phone
     * needs a scheme, so plain text is never taken for a link here (the server's parser is the one that counts).
     */
    fun shareToken(text: String): String? = TOKEN.find(text)?.groupValues?.get(1)

    /** Same host and port (the default port of the scheme when none is given) as the signed-in server. */
    fun sameServer(link: String, base: String): Boolean {
        val a = hostPort(link) ?: return false
        val b = hostPort(base) ?: return false
        return a == b
    }

    private fun hostPort(url: String): Pair<String, Int>? = try {
        val u = URI(url.trim())
        val host = u.host?.lowercase()?.takeIf { it.isNotEmpty() }
        val port = when {
            u.port >= 0 -> u.port
            u.scheme.equals("https", ignoreCase = true) -> 443
            u.scheme.equals("http", ignoreCase = true) -> 80
            else -> -1
        }
        if (host == null || port < 0) null else host to port
    } catch (_: Exception) {
        null
    }

    /** The first http(s) link in the text. */
    fun firstLink(text: String): String? = LINK.find(text)?.value

    /** A cover path the server gives ("/api/share/<token>/cover") as a full URL; full URLs and "" stay as they are. */
    fun absoluteCover(cover: String, url: (String) -> String): String = if (cover.startsWith("/")) url(cover) else cover

    /** `linked.{owner, active, liked}` and `read_only` of a playlist object (an own playlist: "", false, false, false). */
    data class Linked(val owner: String, val active: Boolean, val liked: Boolean, val readOnly: Boolean)

    fun linkedOf(o: JsonObject): Linked {
        val l = o.obj("linked")
        return Linked(l?.str("owner") ?: "", l?.bool("active") ?: false, l?.bool("liked") ?: false, o.bool("read_only"))
    }

    /** Android Auto's Playlists: an added playlist whose link ended has nothing to play, so it is left out. */
    fun inCar(isLinked: Boolean, active: Boolean): Boolean = !isLinked || active

    /** What a failed check of a shared link does: say why, or send the link to the requests box as before. */
    enum class CheckFailure { TOAST, REQUEST }

    /**
     * 404 for a link of another site that has `/s/` (Deezer's `link.deezer.com/s/…`) goes to the requests box; a 404 for
     * a link of this server ("This link doesn't work any more"), a 400 ("Only playlists can be added") and anything
     * else are toasted.
     */
    fun onCheckFailed(code: Int, link: String, base: String): CheckFailure {
        if (code != 404) return CheckFailure.TOAST
        val first = firstLink(link) ?: return CheckFailure.TOAST
        return if (sameServer(first, base)) CheckFailure.TOAST else CheckFailure.REQUEST
    }
}
