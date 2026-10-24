package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Test

/** "Get the rest of the album" for albums MusicBrainz doesn't list: the same answers and words as the web and the desktop. */
class RestOfAlbumRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json) as JsonObject

    @Test
    fun whereTheAlbumWasFound() {
        assertEquals(RestOfAlbumRules.Found.MUSICBRAINZ, RestOfAlbumRules.found(obj("""{"found":"musicbrainz","rgid":"rg-1"}""")))
        // an older server: only `rgid`
        assertEquals(RestOfAlbumRules.Found.MUSICBRAINZ, RestOfAlbumRules.found(obj("""{"rgid":"rg-1"}""")))
        val elsewhere = obj("""{"found":"elsewhere","rgid":null,"source":"Deezer","tracks":[{"title":"A"},{"title":"B"}]}""")
        assertEquals(RestOfAlbumRules.Found.ELSEWHERE, RestOfAlbumRules.found(elsewhere))
        assertEquals(2, RestOfAlbumRules.trackCount(elsewhere))
        assertEquals(RestOfAlbumRules.Found.NONE, RestOfAlbumRules.found(obj("""{"found":"none","rgid":null}""")))
        assertEquals(RestOfAlbumRules.Found.NONE, RestOfAlbumRules.found(obj("""{}""")))
    }

    @Test
    fun theWords() {
        assertEquals("Not on MusicBrainz – track list from Deezer.", RestOfAlbumRules.sourceNote("Deezer"))
        assertEquals("No catalogue lists “X”.", RestOfAlbumRules.notFound("X", ""))
        assertEquals("Request the 1 missing song", RestOfAlbumRules.requestAllLabel(1))
        assertEquals("Request the 3 missing songs", RestOfAlbumRules.requestAllLabel(3))
        assertEquals("Every missing song is already requested", RestOfAlbumRules.requestedText(obj("""{"requested":0,"already_requested":2}""")))
        assertEquals("Requested 1 song", RestOfAlbumRules.requestedText(obj("""{"requested":1,"status":"queued"}""")))
        assertEquals(
            "Requested 3 songs (1 already requested) – waiting for approval",
            RestOfAlbumRules.requestedText(obj("""{"requested":3,"already_requested":1,"status":"pending"}""")),
        )
    }
}
