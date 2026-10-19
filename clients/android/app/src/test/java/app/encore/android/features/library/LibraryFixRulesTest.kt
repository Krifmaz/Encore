package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import app.encore.android.data.api.Track
import app.encore.android.data.repo.Me
import app.encore.android.features.requests.RequestRules
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LibraryFixRulesTest {
    private fun track(json: String): Track = Track.from(EncoreJson.parseToJsonElement(json) as JsonObject)!!

    @Test
    fun songsCarryTheirRequestAndCheck() {
        val t = track("""{"id": 5, "title": "Cannibal", "request_id": 41, "verify_status": "manual"}""")
        assertEquals(41L, t.requestId)
        assertEquals("manual", t.verifyStatus)
        val plain = track("""{"id": 6, "title": "Blow", "request_id": null}""")
        assertEquals(0L, plain.requestId)
        assertEquals("", plain.verifyStatus)
    }

    @Test
    fun whoMayCorrectASong() {
        val fromRequest = track("""{"id": 5, "title": "Cannibal", "request_id": 41}""")
        val scanned = track("""{"id": 6, "title": "Blow"}""")
        val admin = Me(role = "owner")
        val fixer = Me(role = "user", perms = mapOf("fix" to true))
        val member = Me(role = "user")
        assertTrue(LibraryFixRules.canFix(scanned, admin))
        assertTrue(LibraryFixRules.canFix(fromRequest, fixer))
        // a member with "fix" only for songs a request brought in
        assertFalse(LibraryFixRules.canFix(scanned, fixer))
        assertFalse(LibraryFixRules.canFix(fromRequest, member))
        assertFalse(LibraryFixRules.canFix(fromRequest, null))
    }

    @Test
    fun theAlbumPageNamesWhoRequestedASong() {
        val mine = track("""{"id": 5, "title": "Cannibal", "request_id": 41, "requested_by": "Caz"}""")
        val theirs = track("""{"id": 7, "title": "C U Next Tuesday", "request_id": 42, "requested_by": "alice"}""")
        val fixer = Me(username = "Caz", role = "user", perms = mapOf("fix" to true))
        assertEquals("Caz", mine.requestedBy)
        assertTrue(LibraryFixRules.canFix(mine, fixer))
        // the server answers 403 "You can only remove songs you requested"
        assertFalse(LibraryFixRules.canFix(theirs, fixer))
        assertTrue(LibraryFixRules.canFix(theirs, Me(username = "Caz", role = "admin")))
    }

    @Test
    fun theAudioCheckBadge() {
        val t = track("""{"id": 5, "title": "Cannibal", "verify_status": "mismatch", "verify_detail": "AcoustID says: Kesha – Blow"}""")
        assertEquals("AcoustID says: Kesha – Blow", t.verifyDetail)
        assertEquals("wrong audio" to "err", LibraryFixRules.verifyBadge(t.verifyStatus))
        assertEquals("✓ verified" to "ok", LibraryFixRules.verifyBadge("verified"))
        assertEquals("✓ confirmed" to "ok", LibraryFixRules.verifyBadge("manual"))
        assertNull(LibraryFixRules.verifyBadge(""))
        assertNull(LibraryFixRules.verifyBadge("error"))
    }

    @Test
    fun matchScores() {
        assertEquals("ok", LibraryFixRules.matchKind(0.9))
        assertEquals("warn", LibraryFixRules.matchKind(0.85))
        assertEquals("warn", LibraryFixRules.matchKind(0.61))
        assertEquals("err", LibraryFixRules.matchKind(0.6))
        assertEquals(86L, LibraryFixRules.pct(0.857))
        assertEquals(100L, LibraryFixRules.pct(1.0))
    }

    @Test
    fun pastedReleaseLinks() {
        assertEquals("0383dadf-2a4e-4d10-a46a-e9e041da8eb3", LibraryFixRules.releaseIdOf("https://musicbrainz.org/release/0383dadf-2a4e-4d10-a46a-e9e041da8eb3"))
        assertEquals("deezer:302127", LibraryFixRules.releaseIdOf("https://www.deezer.com/en/album/302127"))
        assertEquals("deezer:302127", LibraryFixRules.releaseIdOf("https://www.deezer.com/album/302127"))
        assertEquals("apple:1440857781", LibraryFixRules.releaseIdOf("https://music.apple.com/us/album/illuminate-ep/1440857781"))
        assertEquals("discogs:r249504", LibraryFixRules.releaseIdOf("https://www.discogs.com/release/249504-Rick-Astley"))
        assertEquals("discogs:m96559", LibraryFixRules.releaseIdOf("https://www.discogs.com/master/96559"))
        assertNull(LibraryFixRules.releaseIdOf(""))
        assertNull(LibraryFixRules.releaseIdOf("Illuminate EP"))
    }

    @Test
    fun transferStates() {
        assertEquals("ok", RequestRules.transferKind("Completed, Succeeded"))
        assertEquals("err", RequestRules.transferKind("Completed, Rejected"))
        assertEquals("err", RequestRules.transferKind("Completed, TimedOut"))
        assertEquals("info", RequestRules.transferKind("InProgress"))
    }
}
