package app.encore.android.features.library

import app.encore.android.data.api.PlaylistRow
import app.encore.android.data.api.Track
import app.encore.android.data.api.Tr
import org.junit.Assert.assertEquals
import org.junit.Test

/** The same cases as the web's playlist_sort.rs tests, so both apps order a playlist alike. */
class PlaylistSortRulesTest {
    /** A library song with [title] (and its translated title [t], when given). */
    private fun track(title: String, t: String = "") =
        Track(1, title, "", "", "", "a1", 1, 1, "", 0.0, "flac", 0, titleTr = Tr(t = t))

    /** A playlist row with a title, artist, album, its own length and when it was added. */
    private fun row(title: String, artist: String, album: String, duration: Double, added: Double) =
        PlaylistRow(title, artist, null, album = album, addedAt = added, durationSec = duration)

    /** b (added last, longest), a (no album, unknown length), c, in playlist order. */
    private val rows = listOf(
        row("beta", "Zed", "Mid", 300.0, 30.0),
        row("Alpha", "abba", "", 0.0, 10.0),
        row("charlie", "Abba", "First", 120.0, 20.0),
    )

    @Test
    fun customOrderIsThePlaylistsAndReverses() {
        assertEquals(listOf(0, 1, 2), PlaylistSortRules.order(rows, PlSort()))
        assertEquals(listOf(2, 1, 0), PlaylistSortRules.order(rows, PlSort().reversed()))
    }

    @Test
    fun textSortsIgnoreCaseAndKeepTiesInPlaylistOrder() {
        assertEquals(listOf(1, 0, 2), PlaylistSortRules.order(rows, PlaylistSortRules.picked("title")))
        assertEquals(listOf(2, 0, 1), PlaylistSortRules.order(rows, PlaylistSortRules.picked("title").reversed()))
        assertEquals(listOf(1, 2, 0), PlaylistSortRules.order(rows, PlaylistSortRules.picked("artist")))
    }

    @Test
    fun rowsWithoutTheValueGoLastBothWays() {
        assertEquals(listOf(2, 0, 1), PlaylistSortRules.order(rows, PlaylistSortRules.picked("album")))
        assertEquals(listOf(0, 2, 1), PlaylistSortRules.order(rows, PlaylistSortRules.picked("album").reversed()))
        assertEquals(listOf(0, 2, 1), PlaylistSortRules.order(rows, PlaylistSortRules.picked("duration")))
        assertEquals(listOf(2, 0, 1), PlaylistSortRules.order(rows, PlaylistSortRules.picked("duration").reversed()))
    }

    @Test
    fun dateAddedStartsNewestFirst() {
        assertEquals(PlSort("added", true), PlaylistSortRules.picked("added"))
        assertEquals(listOf(0, 2, 1), PlaylistSortRules.order(rows, PlaylistSortRules.picked("added")))
        assertEquals(listOf(1, 2, 0), PlaylistSortRules.order(rows, PlaylistSortRules.picked("added").reversed()))
    }

    @Test
    fun theTranslatedTitleIsTheOneSortedBy() {
        val items = listOf(
            PlaylistRow("夜に駆ける", "YOASOBI", track("夜に駆ける", t = "Racing into the Night")),
            PlaylistRow("Monster", "YOASOBI", track("Monster")),
        )
        assertEquals(listOf(1, 0), PlaylistSortRules.order(items, PlaylistSortRules.picked("title")))
    }

    @Test
    fun theStoredTextRoundTripsAndJunkIsTheDefault() {
        val s = PlaylistSortRules.picked("title").reversed()
        assertEquals("title:desc", PlaylistSortRules.encode(s))
        assertEquals(s, PlaylistSortRules.decode("title:desc"))
        assertEquals(PlSort(), PlaylistSortRules.decode(null))
        assertEquals(PlSort(), PlaylistSortRules.decode("bogus:asc"))
        assertEquals(PlSort("album", false), PlaylistSortRules.decode("album"))
    }

    @Test
    fun theArrowSaysWhatItDoes() {
        assertEquals("Newest first", PlaylistSortRules.dirLabel(PlaylistSortRules.picked("added")))
        assertEquals("A to Z", PlaylistSortRules.dirLabel(PlaylistSortRules.picked("title")))
        assertEquals("Playlist order, reversed", PlaylistSortRules.dirLabel(PlSort().reversed()))
        assertEquals("Custom order", PlaylistSortRules.label("nope"))
    }
}
