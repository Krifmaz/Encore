package app.encore.android.features.library

import app.encore.android.data.api.PlaylistRow

/**
 * Sorting a playlist's songs (a playlist, Liked Songs and someone else's playlist added from a link), the same rules as
 * the web's web-rs/src/pages/playlist_sort.rs: the Sort chip under the Play buttons reorders the rows on this phone
 * only, the playlist's own order on the server never changes, so *Custom order* always brings it back. Play, Shuffle
 * and a tap on a song follow the order shown. Pure, so the unit tests check it; [PlaylistSorts] keeps the pick.
 */
object PlaylistSortRules {
    /** The sorts a playlist offers: the stored key and the menu's label. The first is the default. */
    val SORTS: List<Pair<String, String>> = listOf(
        "custom" to "Custom order",
        "added" to "Date added",
        "title" to "Title",
        "artist" to "Artist",
        "album" to "Album",
        "duration" to "Duration",
    )

    /** The menu entry that turns the current sort the other way round. */
    const val REVERSE = "Reverse order"

    /** Whether [key] starts newest / longest first when it is picked; the rest start A–Z (or in playlist order). */
    fun descFirst(key: String): Boolean = key == "added" || key == "duration"

    /** [key] in the direction it starts in when picked from the menu (see [descFirst]). */
    fun picked(key: String): PlSort = PlSort(key, descFirst(key))

    /** The menu label of [key] ("Custom order" for anything unknown). */
    fun label(key: String): String = SORTS.firstOrNull { it.first == key }?.second ?: SORTS.first().second

    /** The chip's arrow, read by TalkBack, in words that fit the sort ("A to Z", "Newest first" …). */
    fun dirLabel(s: PlSort): String = when (s.key) {
        "custom" -> if (s.desc) "Playlist order, reversed" else "Playlist order"
        "added" -> if (s.desc) "Newest first" else "Oldest first"
        "duration" -> if (s.desc) "Longest first" else "Shortest first"
        else -> if (s.desc) "Z to A" else "A to Z"
    }

    /** The stored text of a sort ("title:desc"). */
    fun encode(s: PlSort): String = "${s.key}:${if (s.desc) "desc" else "asc"}"

    /** A stored sort read back; null, unknown or malformed text is the default. */
    fun decode(text: String?): PlSort {
        val key = text?.substringBefore(':') ?: return PlSort()
        if (SORTS.none { it.first == key }) return PlSort()
        return PlSort(key, text.substringAfter(':', "") == "desc")
    }

    /**
     * The text a row is sorted by for a text sort, lowercased so case doesn't split the list; null when it has none,
     * so empty rows go last either way. The title is the translated one when the server sent it, like the web.
     */
    private fun textKey(row: PlaylistRow, key: String): String? {
        val s = when (key) {
            "title" -> row.track?.titleTr?.t?.takeIf { it.isNotBlank() } ?: row.title
            "artist" -> row.artist.ifBlank { row.track?.artist.orEmpty() }
            else -> row.album.ifBlank { row.track?.album.orEmpty() }
        }.trim().lowercase()
        return s.ifEmpty { null }
    }

    /** The number a row is sorted by for date added and duration; null when unknown (0), so those go last. */
    private fun numKey(row: PlaylistRow, key: String): Double? {
        val n = if (key == "added") row.addedAt else row.durationSec.takeIf { it > 0 } ?: (row.track?.durationSec ?: 0.0)
        return n.takeIf { it > 0 }
    }

    /** Compares two optional keys with missing ones last whatever the direction; [desc] flips only present keys. */
    private fun <T : Comparable<T>> cmpOpt(a: T?, b: T?, desc: Boolean): Int = when {
        a != null && b != null -> if (desc) b.compareTo(a) else a.compareTo(b)
        a != null -> -1
        b != null -> 1
        else -> 0
    }

    /**
     * The positions of [rows] (the playlist in its own order) in the order [sort] shows them. Ties keep the playlist's
     * order among themselves, and rows without the value sorted by (no album, unknown length) go last.
     */
    fun order(rows: List<PlaylistRow>, sort: PlSort): List<Int> {
        val idx = rows.indices.toList()
        return when (sort.key) {
            // sortedWith is stable, so ties stay in playlist order
            "added", "duration" -> idx.sortedWith { a, b -> cmpOpt(numKey(rows[a], sort.key), numKey(rows[b], sort.key), sort.desc) }
            "title", "artist", "album" -> idx.sortedWith { a, b -> cmpOpt(textKey(rows[a], sort.key), textKey(rows[b], sort.key), sort.desc) }
            // custom (and anything unknown): the playlist's own order, reversed when descending
            else -> if (sort.desc) idx.reversed() else idx
        }
    }
}

/**
 * A playlist's sort: one of [PlaylistSortRules.SORTS]'s keys, and [desc] for descending (Z–A, newest first, longest
 * first, the playlist reversed). The default is the playlist's own order, first song first.
 */
data class PlSort(val key: String = "custom", val desc: Boolean = false) {
    /** The same sort the other way round. */
    fun reversed(): PlSort = copy(desc = !desc)
}
