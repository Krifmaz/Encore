package app.encore.android.features.library

import app.encore.android.data.api.ArtistCredit
import app.encore.android.data.api.Playlist
import app.encore.android.data.api.PlaylistRow
import app.encore.android.data.api.Track
import app.encore.android.data.api.Tr
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaylistSearchRulesTest {
    private fun track(title: String, artist: String, album: String, artists: List<ArtistCredit> = emptyList(), titleTr: Tr = Tr.NONE) =
        Track(1, title, artist, album, artist, "a1", 1, 1, "", 150.0, "flac", 0, artists = artists, titleTr = titleTr)

    private val wompum = PlaylistRow("Wompum VIP", "BARELY ALIVE", track("Wompum VIP", "BARELY ALIVE", "Head to Head, Vol. 1"))
    private val missing = PlaylistRow("Obsidian Vortex", "ATLiens", null, album = "Obsidian Vortex")

    @Test
    fun aSongIsFoundByTitleArtistOrAlbum() {
        val hay = PlaylistSearchRules.haystack(wompum)
        for (q in listOf("wompum", "WOMPUM vip", "barely", "head to head", "vip alive", "", "  ")) {
            assertTrue(q, PlaylistSearchRules.matches(hay, q))
        }
        assertFalse(PlaylistSearchRules.matches(hay, "wompum obsidian"))
        // a row not in the library is found by the playlist's own words
        assertTrue(PlaylistSearchRules.matches(PlaylistSearchRules.haystack(missing), "atliens vortex"))
    }

    @Test
    fun everyArtistAndTheRomanizedTitleCount() {
        val row = PlaylistRow("夜に駆ける", "YOASOBI", track("夜に駆ける", "YOASOBI", "", listOf(ArtistCredit("Ayase"), ArtistCredit("ikura")), Tr(r = "Yoru ni Kakeru")))
        val hay = PlaylistSearchRules.haystack(row)
        for (q in listOf("yoru", "ayase", "ikura", "駆ける")) assertTrue(q, PlaylistSearchRules.matches(hay, q))
    }

    @Test
    fun filterKeepsPlaylistOrder() {
        val rows = listOf(wompum, missing, wompum)
        assertEquals(listOf(0, 1, 2), PlaylistSearchRules.filter(rows, ""))
        assertEquals(listOf(0, 2), PlaylistSearchRules.filter(rows, "alive"))
        assertEquals(emptyList<Int>(), PlaylistSearchRules.filter(rows, "zzz"))
    }

    @Test
    fun playlistsAreFoundByNameAndOwner() {
        val liked = Playlist(1, "Liked Songs", "liked", 10, 8, "")
        val gym = Playlist(2, "Gym", "playlist", 3, 3, "")
        val linked = Playlist(3, "Road trip", "linked", 4, 4, "", linkedOwner = "anna")
        assertEquals(listOf(liked), PlaylistSearchRules.filterPlaylists(listOf(liked, gym, linked), "liked"))
        assertEquals(listOf(linked), PlaylistSearchRules.filterPlaylists(listOf(liked, gym, linked), "road anna"))
        assertEquals(3, PlaylistSearchRules.filterPlaylists(listOf(liked, gym, linked), " ").size)
    }

    @Test
    fun theFoundLine() {
        assertEquals("", PlaylistSearchRules.foundText(5263, 5263, "", "song", "songs"))
        assertEquals("12 of 5263 songs", PlaylistSearchRules.foundText(12, 5263, "atl", "song", "songs"))
        assertEquals("1 of 1 playlist", PlaylistSearchRules.foundText(1, 1, "x", "playlist", "playlists"))
        assertEquals("No playlists match “zzz”", PlaylistSearchRules.foundText(0, 40, " zzz ", "playlist", "playlists"))
    }
}
