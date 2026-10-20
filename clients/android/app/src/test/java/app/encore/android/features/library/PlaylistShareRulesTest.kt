package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import app.encore.android.data.api.Playlist
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaylistShareRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json) as JsonObject

    @Test
    fun theTileSaysShared() {
        assertEquals("10 tracks · 80% in library", PlaylistShareRules.tileSubtitle(10, 8, false))
        assertEquals("10 tracks · 80% in library · shared", PlaylistShareRules.tileSubtitle(10, 8, true))
        assertEquals("0 tracks · 0% in library · shared", PlaylistShareRules.tileSubtitle(0, 0, true))
        assertEquals("3 tracks · 67% in library", PlaylistShareRules.tileSubtitle(3, 2, false))
    }

    @Test
    fun aSharedPlaylistAddsCopyLinkAndStopSharing() {
        assertEquals(listOf("Copy link", "Stop sharing"), PlaylistShareRules.shareEntries(true).map { it.label })
        assertTrue(PlaylistShareRules.shareEntries(false).isEmpty())
    }

    @Test
    fun theSharePathComesFromTheShareObject() {
        assertEquals("/s/abc", PlaylistShareRules.sharePathOf(obj("""{"share": {"token": "abc", "path": "/s/abc", "url": "https://m.example/s/abc"}}""")))
        assertEquals("", PlaylistShareRules.sharePathOf(obj("""{"share": null}""")))
        assertEquals("", PlaylistShareRules.sharePathOf(obj("""{"name": "Road trip"}""")))
    }

    @Test
    fun playlistsKnowWhetherTheyAreShared() {
        val shared = Playlist.from(obj("""{"id": 4, "name": "Liked Songs", "kind": "liked", "total": 3, "matched": 3, "share": {"token": "abc", "path": "/s/abc", "url": "u"}}"""))!!
        assertTrue(shared.isShared)
        assertEquals("/s/abc", shared.sharePath)
        assertTrue(shared.isLiked)
        val plain = Playlist.from(obj("""{"id": 5, "name": "Road trip", "kind": "playlist", "total": 2, "matched": 1, "share": null}"""))!!
        assertFalse(plain.isShared)
        assertEquals("", plain.sharePath)
        val older = Playlist.from(obj("""{"id": 6, "name": "From an older server"}"""))!!
        assertFalse(older.isShared)
    }

    @Test
    fun theTextsAreTheWebs() {
        // web-rs/src/ui.rs share_playlist and web-rs/src/pages/playlists.rs (the Stop sharing confirm and its toast)
        assertEquals("Link copied – anyone with it can play this playlist", PlaylistShareRules.LINK_COPIED)
        assertEquals("Sharing stopped", PlaylistShareRules.STOPPED)
        assertEquals("Stop sharing", PlaylistShareRules.STOP_TITLE)
        assertEquals("The link stops working for everyone who has it. Sharing again makes a new link.", PlaylistShareRules.STOP_TEXT)
    }
}
