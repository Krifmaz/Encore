package app.encore.android.features.library

import app.encore.android.data.api.PlaylistRow

/**
 * The missing songs "Download missing" can't ask for (the web's `cant_request` rows and "can't be requested"
 * button in web-rs/src/pages/playlists.rs). The server marks a row `cant_request` when it has no MusicBrainz id and
 * no artist and title to search by, even after reading "Artist - Song" out of its own text; the playlist page
 * badges those rows and copies them as a list, so they can be found and fixed by hand. Pure, so the unit tests
 * check it.
 */
object PlaylistMissingRules {
    /** The badge on a row that can't be requested. */
    const val BADGE = "can't request"

    /** The rows of [rows] that can't be requested (never one in the library). */
    fun cantRequest(rows: List<PlaylistRow>): List<PlaylistRow> = rows.filter { it.track == null && it.cantRequest.isNotEmpty() }

    /** The header's button: "324 can't be requested · Copy list". */
    fun buttonText(n: Int): String = "$n can't be requested · Copy list"

    /**
     * The copied list: one "Artist - Title" line per song, the title (or the artist) alone when the other is
     * empty, the album in brackets when known.
     */
    fun listText(rows: List<PlaylistRow>): String = rows.joinToString("\n") { r ->
        val line = when {
            r.artist.isEmpty() -> r.title
            r.title.isEmpty() -> r.artist
            else -> "${r.artist} - ${r.title}"
        }
        if (r.album.isNotEmpty()) "$line (${r.album})" else line
    }
}
