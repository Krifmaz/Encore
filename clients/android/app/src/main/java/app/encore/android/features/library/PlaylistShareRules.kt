package app.encore.android.features.library

import app.encore.android.core.int
import app.encore.android.core.obj
import app.encore.android.core.str
import kotlinx.serialization.json.JsonObject

/**
 * Sharing a whole playlist, Liked Songs included (web-rs/src/pages/playlists.rs): Share makes the public link
 * (POST /api/playlists/{id}/share), which always shows the playlist as it is now; Copy link and Stop sharing
 * (DELETE /api/playlists/{id}/share) are in the playlist's ⋯ while it is shared. Pure, so the unit tests check it.
 */
object PlaylistShareRules {
    /** The toast after Share copied the link (the web's `share_playlist`). */
    const val LINK_COPIED = "Link copied – anyone with it can play this playlist"
    /** The toast once the link is gone. */
    const val STOPPED = "Sharing stopped"
    /** The Stop sharing confirm: its title, which is also its button. */
    const val STOP_TITLE = "Stop sharing"
    const val STOP_TEXT = "The link stops working for everyone who has it. Sharing again makes a new link."

    /** "12 tracks · 80% in library", plus " · shared" (the web's `pl_card`). */
    fun tileSubtitle(total: Int, matched: Int, shared: Boolean): String {
        val pct = if (total > 0) Math.round(matched * 100.0 / total).toInt() else 0
        return "$total tracks · $pct% in library" + if (shared) " · shared" else ""
    }

    /** An entry a shared playlist adds to its ⋯ (PlaylistMenu builds them from [shareEntries], so the tests see what it shows). */
    enum class ShareEntry(val label: String) { COPY_LINK("Copy link"), STOP(STOP_TITLE) }

    /** The ⋯ entries a shared playlist adds, in order. */
    fun shareEntries(shared: Boolean): List<ShareEntry> = if (shared) listOf(ShareEntry.COPY_LINK, ShareEntry.STOP) else emptyList()

    /** `share.path` of a playlist object ("/s/<token>"), "" when it is not shared. */
    fun sharePathOf(o: JsonObject): String = o.obj("share")?.str("path") ?: ""

    /** `share.added_by` of an own playlist: how many people added it to their playlists (0 when missing). */
    fun addedByOf(o: JsonObject): Int = o.obj("share")?.int("added_by")?.coerceAtLeast(0) ?: 0
}
