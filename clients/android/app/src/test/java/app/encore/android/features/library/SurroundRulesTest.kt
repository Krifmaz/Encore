package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import app.encore.android.data.api.Album
import app.encore.android.data.api.SurroundAlbums
import app.encore.android.data.api.SurroundRequest
import app.encore.android.data.api.SurroundState
import app.encore.android.data.api.SurroundVersion
import app.encore.android.data.api.Track
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SurroundRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json) as JsonObject
    private fun track(json: String): Track = Track.from(obj(json))!!

    @Test
    fun surroundIds() {
        assertTrue(SurroundRules.isSurroundTrack(1_000_000_000L))
        assertFalse(SurroundRules.isSurroundTrack(999_999_999L))
        assertTrue(SurroundRules.isSurroundAlbum("s0123456789abcde"))
        assertFalse(SurroundRules.isSurroundAlbum("0123456789abcdef"))
    }

    @Test
    fun surroundSongs() {
        val t = track(
            """{"id": 1000000004, "title": "Money", "album_id": "s0123456789abcde", "format": "eac3", "codec": "eac3", "channels": 6,
               "atmos": 1, "quality": "Dolby Atmos (E-AC-3 5.1)", "stereo_track_id": 52, "stereo_album_id": "ab12", "class": "surround"}""",
        )
        assertTrue(t.isSurround)
        assertTrue(t.atmos)
        assertEquals("Dolby Atmos (E-AC-3 5.1)", t.quality)
        assertEquals("ab12", t.stereoAlbumId)
        assertEquals("eac3", t.format)
        // after a trip through the player (no class): told by its id
        assertTrue(t.copy(trackClass = "").isSurround)
    }

    @Test
    fun stereoSongsNameTheirSurroundVersion() {
        val t = track("""{"id": 52, "title": "Money", "surround_id": 1000000004}""")
        assertFalse(t.isSurround)
        assertEquals(1_000_000_004L, t.surroundId)
        // older servers: none of it
        val old = track("""{"id": 53, "title": "Time"}""")
        assertEquals(0L, old.surroundId)
        assertEquals("", old.quality)
        assertFalse(old.isSurround)
    }

    @Test
    fun theSurroundAlbumsPage() {
        val page = SurroundAlbums.from(
            obj(
                """{"albums": [
                     {"id": "s0123456789abcde", "album": "The Dark Side of the Moon", "albumartist": "Pink Floyd", "year": 1973, "tracks": 10,
                      "duration": 2580.5, "atmos": true, "quality": "Dolby Atmos (E-AC-3 5.1)", "stereo_album_id": "ab12", "class": "surround"},
                     {"id": "s0123456789abcdf", "album": "Tubular Bells", "albumartist": "Mike Oldfield", "year": "1973", "tracks": 2,
                      "atmos": 0, "quality": "FLAC 5.1", "stereo_album_id": null, "class": "surround"}
                   ],
                   "stats": {"tracks": 12, "albums": 2, "atmos": 1, "size": 2000000000, "duration": 5000}}""",
            ),
        )
        assertEquals(2, page.albums.size)
        val dsotm = page.albums[0]
        assertTrue(dsotm.isSurround)
        assertTrue(dsotm.atmos)
        assertEquals("1973", dsotm.year)
        assertEquals("ab12", dsotm.stereoAlbumId)
        assertEquals("Dolby Atmos", SurroundRules.badge(dsotm.atmos, dsotm.quality))
        val bells = page.albums[1]
        assertFalse(bells.atmos)
        assertEquals("", bells.stereoAlbumId)
        assertEquals("FLAC 5.1", SurroundRules.badge(bells.atmos, bells.quality))
        assertEquals(2, page.stats.albums)
        assertEquals(1, page.stats.atmos)
        assertEquals(12, page.stats.tracks)
        assertEquals(2_000_000_000L, page.stats.size)
        // a stereo album is no surround one
        assertFalse(Album.from(obj("""{"id": "ab12", "album": "The Dark Side of the Moon"}"""))!!.isSurround)
        // an empty or odd answer
        assertTrue(SurroundAlbums.from(obj("{}")).albums.isEmpty())
    }

    @Test
    fun aStereoAlbumsSurroundState() {
        val s = SurroundState.from(
            obj(
                """{"have": [{"album_id": "s0123456789abcde", "tracks": 10, "quality": "Dolby Atmos (E-AC-3 5.1)"}, {"album_id": "", "tracks": 1}],
                   "request": {"id": 77, "status": "downloading"}, "available": true, "can_fetch": true}""",
            ),
        )!!
        assertEquals(1, s.have.size)
        assertEquals("s0123456789abcde", s.have[0].albumId)
        assertEquals(10, s.have[0].tracks)
        assertEquals(SurroundRequest(77L, "downloading"), s.request)
        assertEquals(true, s.available)
        assertTrue(s.canFetch)

        val none = SurroundState.from(obj("""{"have": [], "request": null, "available": null, "can_fetch": false}"""))!!
        assertNull(none.request)
        assertNull(none.available)
        assertEquals(false, SurroundState.from(obj("""{"available": false}"""))!!.available)
        // older servers send no `surround`
        assertNull(SurroundState.from(null))
    }

    @Test
    fun theAlbumPagesChips() {
        assertEquals("Dolby Atmos version", SurroundRules.versionLabel("Dolby Atmos (E-AC-3 5.1)"))
        assertEquals("E-AC-3 5.1 version", SurroundRules.versionLabel("E-AC-3 5.1"))
        assertEquals("Surround version", SurroundRules.versionLabel(""))
        assertEquals("Dolby Atmos version: downloading", SurroundRules.requestLabel(SurroundRequest(77L, "downloading")))
    }

    @Test
    fun getDolbyAtmosVersion() {
        val open = SurroundState(emptyList(), null, null, canFetch = true)
        assertTrue(SurroundRules.canFetch(open, admin = true))
        assertFalse(SurroundRules.canFetch(open, admin = false))
        // gamdl can't fetch Atmos, one is here already, or one is coming
        assertFalse(SurroundRules.canFetch(open.copy(canFetch = false), admin = true))
        assertFalse(SurroundRules.canFetch(open.copy(have = listOf(SurroundVersion("s1", 3, "E-AC-3 5.1"))), admin = true))
        assertFalse(SurroundRules.canFetch(open.copy(request = SurroundRequest(5L, "pending")), admin = true))
        assertFalse(SurroundRules.canFetch(null, admin = true))
        // asked before and Apple Music had none: it may be asked again
        assertTrue(SurroundRules.canFetch(open.copy(available = false), admin = true))

        assertEquals("Dolby Atmos version requested (#77)", SurroundRules.requestedToast(77L))
        assertEquals("warn", SurroundRules.errorKind(404))
        assertEquals("err", SurroundRules.errorKind(409))
    }
}
