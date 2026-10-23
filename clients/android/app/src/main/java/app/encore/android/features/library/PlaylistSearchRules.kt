package app.encore.android.features.library

import app.encore.android.data.api.Playlist
import app.encore.android.data.api.PlaylistRow

/**
 * Search inside a playlist and on the Playlists screen (the web's `search_box_html` / `query_matches` in
 * web-rs/src/pages/playlists.rs): the 🔍 in the top bar opens a box that filters the rows as you type. A row is
 * kept when every word of the search appears in its title, artist or album, in any order and in any case.
 * Pure, so the unit tests check it.
 */
object PlaylistSearchRules {
    /** The placeholder of the box inside a playlist. */
    const val SONG_PLACEHOLDER = "Search title, artist or album"
    /** The placeholder of the box on the Playlists screen. */
    const val PLAYLIST_PLACEHOLDER = "Search playlists"
    /** The top bar's 🔍 inside a playlist, for TalkBack. */
    const val OPEN_SONG_SEARCH = "Search in this playlist"
    /** The top bar's 🔍 on the Playlists screen, for TalkBack. */
    const val OPEN_PLAYLIST_SEARCH = "Search playlists"

    /**
     * What a playlist row is found by, lowercased: its title (with the translated and romanized title, so a search
     * works whichever one shows), every artist and the album. Rows not in the library have only the playlist's
     * own title, artist and album.
     */
    fun haystack(row: PlaylistRow): String {
        val parts = mutableListOf(row.title, row.artist, row.album)
        row.track?.let { t ->
            parts += listOf(t.title, t.titleTr.t, t.titleTr.r, t.artist, t.album)
            parts += t.artists.map { it.name }
        }
        return parts.filter { it.isNotEmpty() }.joinToString(" ").lowercase()
    }

    /** What a playlist tile is found by, lowercased: its name, and for someone else's playlist whose it is. */
    fun haystack(pl: Playlist): String = (if (pl.isLinked) "${pl.name} ${pl.linkedOwner}" else pl.name).lowercase()

    /** Every word of [query] appears in [hay] (already lowercased). A blank query matches everything. */
    fun matches(hay: String, query: String): Boolean =
        query.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.all { hay.contains(it.lowercase()) }

    /** The positions of the rows that match [query], in playlist order (every position while it is blank). */
    fun filter(rows: List<PlaylistRow>, query: String): List<Int> =
        if (query.isBlank()) rows.indices.toList()
        else rows.indices.filter { matches(haystack(rows[it]), query) }

    /** The playlists that match [query] by name (all of them while it is blank). */
    fun filterPlaylists(playlists: List<Playlist>, query: String): List<Playlist> =
        if (query.isBlank()) playlists else playlists.filter { matches(haystack(it), query) }

    /**
     * The line under the box: "" while it is blank, "12 of 5263 songs" while it finds some, "No songs match “x”"
     * when it finds none. [one] and [many] are the noun ("song", "songs").
     */
    fun foundText(shown: Int, total: Int, query: String, one: String, many: String): String {
        val q = query.trim()
        if (q.isEmpty()) return ""
        if (shown == 0) return "No $many match “$q”"
        return "$shown of $total ${if (total == 1) one else many}"
    }
}
