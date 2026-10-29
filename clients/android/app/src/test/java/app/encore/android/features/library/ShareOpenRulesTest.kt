package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** [ShareOpenRules]: the app links the server's share page sends, the browser fallback, and the server's answers. */
class ShareOpenRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json) as JsonObject

    private val token = "AbC12_xY-9876543"

    @Test
    fun theTokenComesFromEveryAppLinkTheServerSends() {
        for (data in listOf(
            "encore://music.example/s/$token",
            "encore://music.example/s/$token/",
            "encore://example.org/encore/s/$token",
            "encore://192.168.1.5:8080/s/$token?x=1",
            "ENCORE://music.example/s/$token#frag",
        )) {
            assertEquals(data, token, ShareOpenRules.tokenOf(data))
        }
    }

    @Test
    fun otherLinksAreNotShareLinks() {
        for (data in listOf(null, "", "https://music.example/s/$token", "encore://music.example/", "encore://music.example/s/", "encore://music.example/#/group/abc", "encore://m/s/a b")) {
            assertNull(data, ShareOpenRules.tokenOf(data))
        }
    }

    @Test
    fun theBrowserGetsTheIntentsOwnLinkElseAnHttpsOne() {
        val data = "encore://192.168.1.5:8080/s/$token"
        // the server's S.link extra keeps http for a LAN server
        assertEquals("http://192.168.1.5:8080/s/$token", ShareOpenRules.linkOf(data, token, "http://192.168.1.5:8080/s/$token"))
        // no extra, or one for another token: the app link made https
        assertEquals("https://192.168.1.5:8080/s/$token", ShareOpenRules.linkOf(data, token, null))
        assertEquals("https://192.168.1.5:8080/s/$token", ShareOpenRules.linkOf(data, token, "https://evil.example/s/other"))
        assertEquals("https://example.org/encore/s/$token", ShareOpenRules.pendingOf("encore://example.org/encore/s/$token", null)?.link)
    }

    @Test
    fun theFallbackSaysNoAppOnce() {
        assertEquals("https://m.example/s/t?app=0", ShareOpenRules.noAppLink("https://m.example/s/t"))
        assertEquals("https://m.example/s/t?x=1&app=0", ShareOpenRules.noAppLink("https://m.example/s/t?x=1"))
        assertEquals("https://m.example/s/t?app=0#f", ShareOpenRules.noAppLink("https://m.example/s/t#f"))
        assertEquals("https://m.example/s/t?app=0", ShareOpenRules.noAppLink("https://m.example/s/t?app=0"))
    }

    @Test
    fun theServersAnswerOpensThePage() {
        assertEquals(ShareOpenRules.Go.Track("a1", 7L), ShareOpenRules.goOf(obj("""{"kind":"track","track_id":7,"album_id":"a1"}""")))
        assertEquals(ShareOpenRules.Go.Album("a1"), ShareOpenRules.goOf(obj("""{"kind":"album","album_id":"a1"}""")))
        assertEquals(ShareOpenRules.Go.Artist("Coldplay"), ShareOpenRules.goOf(obj("""{"kind":"artist","name":"Coldplay"}""")))
        assertEquals(ShareOpenRules.Go.Playlist(4L, "Road trip"), ShareOpenRules.goOf(obj("""{"kind":"playlist","own":true,"added":false,"playlist_id":4,"name":"Road trip"}""")))
        assertEquals(ShareOpenRules.Go.Playlist(9L, "Road trip"), ShareOpenRules.goOf(obj("""{"kind":"playlist","own":false,"added":true,"playlist_id":9,"name":"Road trip"}""")))
        // someone else's playlist not added yet: "Add to my playlists?"
        assertEquals(ShareOpenRules.Go.AddPlaylist, ShareOpenRules.goOf(obj("""{"kind":"playlist","own":false,"added":false,"playlist_id":null,"name":"Road trip"}""")))
        // nothing to open: the browser
        assertEquals(ShareOpenRules.Go.Browser, ShareOpenRules.goOf(obj("""{"kind":"track","track_id":7,"album_id":""}""")))
        assertEquals(ShareOpenRules.Go.Browser, ShareOpenRules.goOf(obj("""{"kind":"what"}""")))
    }

    @Test
    fun aSharedSongStartsWhereItIsOnItsAlbum() {
        assertEquals(2, ShareOpenRules.startOf(listOf(5L, 6L, 7L), 7L))
        // not on the album as loaded: it plays on its own
        assertEquals(-1, ShareOpenRules.startOf(listOf(5L, 6L), 7L))
        assertEquals(-1, ShareOpenRules.startOf(emptyList(), 7L))
    }
}
