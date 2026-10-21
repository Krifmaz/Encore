package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import app.encore.android.data.api.Playlist
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaylistLinkRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json) as JsonObject

    private val token = "AbC12_xY-9876543"

    @Test
    fun theTokenComesFromEveryLinkTheServerMakes() {
        val links = listOf(
            "https://music.example/s/$token",
            "https://example.org/encore/s/$token",
            "http://192.168.1.5:8080/s/$token",
            // the phone's share sheet text (ShareLinks.kt: "$text\n$url")
            "Road trip\nhttps://music.example/s/$token",
            "https://music.example/api/share/$token",
            "https://music.example/api/share/$token/cover?s=600",
            "https://music.example/api/share/$token/stream?track=3",
            "https://music.example/s/$token/",
            "https://music.example/s/$token?x=1",
            "https://music.example/s/$token#frag",
            "Listen: https://music.example/s/$token.",
            "https://music.example/s/$token, then more",
            "<https://music.example/s/$token>",
        )
        for (l in links) assertEquals(l, token, PlaylistLinkRules.shareToken(l))
    }

    @Test
    fun plainTextAndOtherLinksHaveNoToken() {
        // the phone needs a scheme: a bare token, a path or plain text is never taken for a link
        assertNull(PlaylistLinkRules.shareToken(token))
        assertNull(PlaylistLinkRules.shareToken("/s/$token"))
        assertNull(PlaylistLinkRules.shareToken("music.example/s/$token"))
        assertNull(PlaylistLinkRules.shareToken("hello"))
        assertNull(PlaylistLinkRules.shareToken(""))
        assertNull(PlaylistLinkRules.shareToken("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"))
        assertNull(PlaylistLinkRules.shareToken("https://music.example/s/"))
        assertNull(PlaylistLinkRules.shareToken("https://music.example/s/" + "a".repeat(65)))
        // a group invite is not a playlist link
        assertNull(PlaylistLinkRules.shareToken("https://music.example/#/group/ABCD12"))
        // another site's /s/ link looks like one: the server answers 404 and the phone falls back to the requests box
        assertEquals("30ABCdef", PlaylistLinkRules.shareToken("https://link.deezer.com/s/30ABCdef"))
    }

    @Test
    fun sameServerComparesHostAndPort() {
        assertTrue(PlaylistLinkRules.sameServer("https://music.example/s/$token", "https://music.example"))
        assertTrue(PlaylistLinkRules.sameServer("https://Music.Example/s/$token", "https://music.example/"))
        assertTrue(PlaylistLinkRules.sameServer("https://music.example:443/s/$token", "https://music.example"))
        assertTrue(PlaylistLinkRules.sameServer("https://music.example/s/$token", "https://music.example:443"))
        assertTrue(PlaylistLinkRules.sameServer("http://192.168.1.5:8080/s/$token", "http://192.168.1.5:8080"))
        assertTrue(PlaylistLinkRules.sameServer("https://example.org/encore/s/$token", "https://example.org/encore"))
        assertFalse(PlaylistLinkRules.sameServer("http://192.168.1.5:8081/s/$token", "http://192.168.1.5:8080"))
        assertFalse(PlaylistLinkRules.sameServer("http://music.example/s/$token", "https://music.example"))
        assertFalse(PlaylistLinkRules.sameServer("https://link.deezer.com/s/30ABCdef", "https://music.example"))
        assertFalse(PlaylistLinkRules.sameServer("https://music.example/s/$token", ""))
    }

    @Test
    fun theFirstLinkOfTheText() {
        assertEquals("https://music.example/s/$token", PlaylistLinkRules.firstLink("Road trip\nhttps://music.example/s/$token"))
        assertEquals("http://a.example/x", PlaylistLinkRules.firstLink("see http://a.example/x and https://b.example/y"))
        assertNull(PlaylistLinkRules.firstLink("no link here"))
    }

    @Test
    fun aFailedCheckToastsOrGoesToTheRequestsBox() {
        val base = "https://music.example"
        assertEquals(PlaylistLinkRules.CheckFailure.TOAST, PlaylistLinkRules.onCheckFailed(404, "https://music.example/s/$token", base))
        assertEquals(PlaylistLinkRules.CheckFailure.REQUEST, PlaylistLinkRules.onCheckFailed(404, "Listen https://link.deezer.com/s/30ABCdef", base))
        assertEquals(PlaylistLinkRules.CheckFailure.TOAST, PlaylistLinkRules.onCheckFailed(400, "https://music.example/s/$token", base))
        assertEquals(PlaylistLinkRules.CheckFailure.TOAST, PlaylistLinkRules.onCheckFailed(400, "https://link.deezer.com/s/30ABCdef", base))
        assertEquals(PlaylistLinkRules.CheckFailure.TOAST, PlaylistLinkRules.onCheckFailed(500, "https://link.deezer.com/s/30ABCdef", base))
    }

    @Test
    fun theTextsAreTheWebs() {
        // web-rs/src/pages/playlists.rs (ADD_SHARED_TITLE … LINKED_EMPTY, linked_kicker, linked_hint, linked_stopped,
        // linked_tile_line, remove_text, added_by_text, add_toast)
        assertEquals("Add a shared playlist", PlaylistLinkRules.ADD_SHARED_TITLE)
        assertEquals(
            "Paste a playlist link from someone on this server. You get their playlist as it is now, and it changes when they change it.",
            PlaylistLinkRules.ADD_SHARED_HINT,
        )
        assertEquals("https://…/s/…", PlaylistLinkRules.PLACEHOLDER)
        assertEquals("Add to my playlists", PlaylistLinkRules.ADD)
        assertEquals("Added to your playlists", PlaylistLinkRules.ADDED)
        assertEquals("Already in your playlists", PlaylistLinkRules.ALREADY)
        assertEquals("Remove from my playlists", PlaylistLinkRules.REMOVE)
        assertEquals("Removed from your playlists", PlaylistLinkRules.REMOVED)
        assertEquals("Save a copy", PlaylistLinkRules.SAVE_COPY)
        assertEquals("Saved to your playlists", PlaylistLinkRules.SAVED)
        assertEquals("No songs here right now", PlaylistLinkRules.EMPTY)
        assertEquals("Playlist · by anna", PlaylistLinkRules.kicker(false, "anna"))
        assertEquals("Liked songs · by anna", PlaylistLinkRules.kicker(true, "anna"))
        assertEquals("Changes anna makes show here. Save a copy to make your own.", PlaylistLinkRules.hint("anna"))
        assertEquals("anna stopped sharing this playlist", PlaylistLinkRules.stopped("anna"))
        assertEquals("12 tracks · by anna", PlaylistLinkRules.tileLine(12, "anna", true))
        assertEquals("anna stopped sharing this playlist", PlaylistLinkRules.tileLine(12, "anna", false))
        assertEquals("Remove “Road trip” from your playlists? You can add it again from its link.", PlaylistLinkRules.removeText("Road trip"))
        assertEquals("", PlaylistLinkRules.addedBy(0))
        assertEquals("added by 1 person", PlaylistLinkRules.addedBy(1))
        assertEquals("added by 3 people", PlaylistLinkRules.addedBy(3))
        assertEquals("Added to your playlists", PlaylistLinkRules.addToast(true))
        assertEquals("Already in your playlists", PlaylistLinkRules.addToast(false))
    }

    @Test
    fun anEndedPlaylistOnlyOffersRemove() {
        assertEquals(listOf("Save a copy", "Remove from my playlists"), PlaylistLinkRules.menuEntries(true).map { it.label })
        assertEquals(listOf("Remove from my playlists"), PlaylistLinkRules.menuEntries(false).map { it.label })
    }

    @Test
    fun coverPathsBecomeFullUrls() {
        val url = { p: String -> "https://music.example$p" }
        assertEquals("https://music.example/api/share/$token/cover", PlaylistLinkRules.absoluteCover("/api/share/$token/cover", url))
        assertEquals("https://img.example/a.jpg", PlaylistLinkRules.absoluteCover("https://img.example/a.jpg", url))
        assertEquals("", PlaylistLinkRules.absoluteCover("", url))
    }

    @Test
    fun androidAutoLeavesOutEndedOnes() {
        assertTrue(PlaylistLinkRules.inCar(isLinked = false, active = false))
        assertTrue(PlaylistLinkRules.inCar(isLinked = true, active = true))
        assertFalse(PlaylistLinkRules.inCar(isLinked = true, active = false))
    }

    @Test
    fun playlistsKnowWhetherTheyAreAddedFromALink() {
        val linked = Playlist.from(
            obj(
                """{"id": 41, "user_id": 7, "name": "Liked Songs · anna", "kind": "linked", "source": "share", "source_url": null,
                   "cover_url": "/api/share/$token/cover", "total": 12, "matched": 12, "share": null, "read_only": true,
                   "linked": {"owner": "anna", "active": true, "liked": true}}""",
            ),
        )!!
        assertTrue(linked.isLinked)
        assertTrue(linked.readOnly)
        assertFalse(linked.isLiked)
        assertFalse(linked.isShared)
        assertEquals("anna", linked.linkedOwner)
        assertTrue(linked.linkedActive)
        assertTrue(linked.linkedLiked)
        assertEquals(0, linked.addedBy)

        val ended = Playlist.from(obj("""{"id": 42, "name": "Road trip", "kind": "linked", "read_only": true, "linked": {"owner": "anna", "active": false, "liked": false}}"""))!!
        assertTrue(ended.isLinked)
        assertFalse(ended.linkedActive)
        assertFalse(ended.linkedLiked)

        val own = Playlist.from(
            obj("""{"id": 5, "name": "Road trip", "kind": "playlist", "read_only": false, "linked": null, "share": {"token": "t", "path": "/s/t", "url": "u", "added_by": 3}}"""),
        )!!
        assertFalse(own.isLinked)
        assertFalse(own.readOnly)
        assertEquals("", own.linkedOwner)
        assertEquals(3, own.addedBy)
        assertTrue(own.isShared)

        // an older server: no read_only, linked or added_by
        val older = Playlist.from(obj("""{"id": 6, "name": "Old", "kind": "playlist", "share": {"token": "t", "path": "/s/t"}}"""))!!
        assertFalse(older.readOnly)
        assertEquals(0, older.addedBy)
    }
}
