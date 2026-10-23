package app.encore.android.features.library

import app.encore.android.data.api.Playlist

/**
 * Merge into… and Import's Save to (web-rs/src/pages/playlists.rs `merge_into_modal`, `target_options`): a
 * playlist's songs go into another playlist or Liked Songs (POST /api/playlists/merge {ids, into, delete_sources});
 * songs it already has are skipped, songs not in the library come along as missing so they still download. Pure,
 * so the unit tests check it.
 */
object PlaylistMergeRules {
    const val TITLE = "Merge into…"
    const val DELETE_SOURCE = "Delete this playlist afterwards"
    const val NEW_PLAYLIST = "New playlist"

    fun hint(name: String): String =
        "The songs of “$name” are added to the playlist you pick. Songs it already has are skipped (remixes, live versions and covers stay separate); songs not in the library come along as missing, so they still download."

    /** The playlists that take songs, Liked Songs first, without [skip] (the playlist being merged). */
    fun targets(all: List<Playlist>, skip: Long? = null): List<Playlist> =
        all.filter { !it.readOnly && !it.isLinked && it.id != skip }.sortedByDescending { it.isLiked }

    /** A target's label: Liked Songs with its heart. */
    fun label(pl: Playlist): String = if (pl.isLiked) "♥ ${pl.name}" else pl.name

    /** The toast after a merge into a playlist. */
    fun mergedText(added: Int, total: Int, skipped: Int): String = "Added $added of $total tracks ($skipped already there)"
}
