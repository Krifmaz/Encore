package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import app.encore.android.core.decodeModel
import app.encore.android.data.repo.CatalogAlbum
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** [SingleRules]: a release with one song in the library is a single, played and described as a song. */
class SingleRulesTest {
    @Test
    fun oneSongIsASingleAndAnythingElseKeepsItsAlbumPage() {
        assertTrue(SingleRules.isSingle(1))
        // two songs or more have a page; 0 is "not known" (an older server, a row without a count)
        for (n in listOf(0, 2, 12)) assertFalse("$n songs", SingleRules.isSingle(n))
    }

    @Test
    fun aSinglesTileSaysSong() {
        assertEquals("Song · VALORANT & ericdoa · 2023", SingleRules.subtitle("VALORANT & ericdoa", "2023", 1))
        assertEquals("Song · ericdoa", SingleRules.subtitle("ericdoa", "", 1))
        assertEquals("Weezer · 1994", SingleRules.subtitle("Weezer", "1994", 10))
        assertEquals("", SingleRules.subtitle("", "", 0))
    }

    private fun release(json: String): CatalogAlbum = (EncoreJson.parseToJsonElement(json) as JsonObject).decodeModel(CatalogAlbum.serializer())!!

    @Test
    fun aDiscographyReleaseCarriesItsLibrarySongCount() {
        val a = release("""{"id":"rg","title":">one","in_library":true,"library_album_id":"a1","library_tracks":1}""")
        assertEquals(1, a.libraryTracks)
        assertTrue(SingleRules.isSingle(a.libraryTracks))
        // an older server sends no count: the release opens its page as before
        assertEquals(0, release("""{"id":"rg","library_album_id":"a1"}""").libraryTracks)
    }
}
