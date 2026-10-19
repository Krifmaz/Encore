package app.encore.android.features.requests

import app.encore.android.core.EncoreJson
import app.encore.android.core.decodeModel
import app.encore.android.data.repo.LinkProbe
import app.encore.android.data.repo.LinkResult
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.RequestRow
import app.encore.android.data.repo.RetryFailedResult
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RequestRulesTest {
    private fun row(json: String): RequestRow =
        (EncoreJson.parseToJsonElement(json) as JsonObject).decodeModel(RequestRow.serializer())!!

    // the Illuminate EP row of the Oct 19 screenshot, as GET /api/requests sends it
    private val illuminate = """{"id": 41, "kind": "album", "artist": "Duumu", "title": "Illuminate EP", "status": "failed",
        "source": "streamrip", "message": "All sources exhausted (streamrip: Input/output error (os error 5))", "username": "Caz",
        "created_at": 1760870000, "attempts": 3, "picked": true, "candidate": null,
        "log": ["attempt 1 failed: All sources exhausted (slskd: error: Soulseek isn't connected yet; torznab: no matching release on your indexers)",
                "attempt 2 failed: Cancelled by Caz", "streamrip: Input/output error (os error 5)"]}"""

    @Test
    fun theNewFieldsAreRead() {
        val r = row(illuminate)
        assertEquals(3, r.attempts)
        assertTrue(r.picked)
        assertNull(r.candidate)
        assertEquals(3, r.log.size)
        assertEquals("streamrip: Input/output error (os error 5)", r.log[2])
        val c = row("""{"id": 7, "status": "downloading", "candidate": {"username": "alice", "format": "flac", "directory": "Illuminate EP", "files": 4}, "picked": 0}""")
        assertFalse(c.picked)
        assertEquals("alice", c.candidate?.username)
        assertEquals(4, c.candidate?.files)
        // an older server: no log, no picked, no candidate
        val old = row("""{"id": 8, "status": "queued"}""")
        assertEquals(emptyList<String>(), old.log)
        assertFalse(old.picked)
        assertNull(old.candidate)
        // odd values never cost the row
        val odd = row("""{"id": 9, "log": "not a list", "candidate": "x", "picked": null}""")
        assertEquals(emptyList<String>(), odd.log)
        assertNull(odd.candidate)
    }

    @Test
    fun statusLineIsTheWebs() {
        val r = row(illuminate)
        assertEquals("3 attempts · via streamrip · All sources exhausted (streamrip: Input/output error (os error 5)) · by Caz · 8m ago", RequestRules.statusLine(r, admin = true, ago = "8m ago"))
        assertEquals("3 attempts · via streamrip · All sources exhausted (streamrip: Input/output error (os error 5)) · 8m ago", RequestRules.statusLine(r, admin = false, ago = "8m ago"))
        assertEquals("just now", RequestRules.statusLine(row("""{"id": 2, "attempts": 1}"""), admin = true, ago = "just now"))
    }

    @Test
    fun logLinesNameTheSourceInPlainWords() {
        assertEquals(LogEntry("Torrents & Usenet", "no matching release on your indexers"), RequestRules.logEntry("torznab: no matching release on your indexers"))
        assertEquals(LogEntry("Soulseek", "error: Soulseek isn't connected yet"), RequestRules.logEntry("slskd_lossless: error: Soulseek isn't connected yet"))
        // ytdlp also fetches every direct link (SoundCloud, Bandcamp…): it keeps the name Settings gives it
        assertEquals(LogEntry("yt-dlp", "waiting for Soulseek (14:20)"), RequestRules.logEntry("ytdlp: waiting for Soulseek (14:20)"))
        // lines that don't start with a source stay whole
        assertEquals(LogEntry("", "attempt 2 failed: Cancelled by Caz"), RequestRules.logEntry("attempt 2 failed: Cancelled by Caz"))
        // multi-line output (streamrip's) keeps its lines
        assertEquals(LogEntry("streamrip", "output for https://x:\nline 1\nline 2"), RequestRules.logEntry("streamrip: output for https://x:\nline 1\nline 2"))
        // an unknown id is shown as it is
        assertEquals(LogEntry("newsource", "boom"), RequestRules.logEntry("newsource: boom"))
    }

    @Test
    fun withoutALogTheMessageIsSplitPerSource() {
        assertEquals(
            listOf("slskd: error: Soulseek isn't connected yet", "torznab: no matching release on yo…"),
            RequestRules.exhaustedParts("All sources exhausted (slskd: error: Soulseek isn't connected yet; torznab: no matching release on yo…)"),
        )
        assertEquals(emptyList<String>(), RequestRules.exhaustedParts("Downloaded 0 file(s)"))
        val r = row("""{"id": 3, "status": "failed", "message": "All sources exhausted (slskd_lossless: transfer failed; slskd: error: Soulseek isn't connected yet)"}""")
        assertEquals(
            listOf(LogEntry("Soulseek", "transfer failed"), LogEntry("Soulseek", "error: Soulseek isn't connected yet")),
            RequestRules.logEntries(r),
        )
        // with a log, the log wins
        assertEquals(3, RequestRules.logEntries(row(illuminate)).size)
    }

    @Test
    fun detailsHoldTheWholeMessageAndEveryFact() {
        val d = RequestRules.details(row(illuminate), admin = true, ago = "8m ago")
        assertEquals("Illuminate EP", d.title)
        assertEquals("Duumu", d.note)
        assertEquals("All sources exhausted (streamrip: Input/output error (os error 5))", d.message)
        assertEquals(listOf("Requested or retried 3 times", "via streamrip", "Picked by hand", "by Caz", "8m ago"), d.facts)
        assertEquals(LogEntry("streamrip", "Input/output error (os error 5)"), d.log.last())
        // a member doesn't see who asked; a first request has no attempts line
        val m = RequestRules.details(row("""{"id": 4, "kind": "track", "title": "Cannibal", "artist": "Ke${'$'}ha", "username": "Caz", "status": "queued", "message": "Waiting for Soulseek – next attempt at 14:20", "created_at": 1}"""), admin = false, ago = "5m ago")
        assertEquals("Cannibal", m.title)
        assertEquals("Ke\$ha", m.note)
        assertEquals("Waiting for Soulseek – next attempt at 14:20", m.message)
        assertEquals(listOf("5m ago"), m.facts)
        val c = row("""{"id": 7, "status": "downloading", "candidate": {"username": "alice", "format": "flac", "directory": "Illuminate EP", "files": 4}}""")
        assertEquals("From alice: Illuminate EP (FLAC, 4 files)", RequestRules.candidateLine(c))
        assertEquals("From bob: Album (1 file)", RequestRules.candidateLine(row("""{"id": 7, "candidate": {"username": "bob", "directory": "Album", "files": 1}}""")))
        assertEquals("", RequestRules.candidateLine(row("""{"id": 7}""")))
    }

    @Test
    fun buttonsFollowTheWebsGates() {
        val admin = Me(role = "admin")
        val member = Me(role = "user", perms = mapOf("pick" to true))
        val noPick = Me(role = "user")
        val failedAlbum = row("""{"id": 1, "kind": "album", "status": "failed"}""")
        val pending = row("""{"id": 2, "kind": "track", "status": "pending"}""")
        val done = row("""{"id": 3, "kind": "track", "status": "completed"}""")
        val downloading = row("""{"id": 4, "kind": "url", "status": "downloading"}""")
        val playlist = row("""{"id": 5, "kind": "playlist", "status": "completed"}""")
        assertTrue(RequestRules.canApprove(pending, admin = true))
        assertFalse(RequestRules.canApprove(pending, admin = false))
        assertTrue(RequestRules.canPick(failedAlbum, admin))
        assertTrue(RequestRules.canPick(failedAlbum, member))
        assertFalse(RequestRules.canPick(failedAlbum, noPick))
        assertFalse(RequestRules.canPick(done, admin))
        assertFalse(RequestRules.canPick(downloading, admin))
        assertTrue(RequestRules.canRetry(failedAlbum, admin))
        assertTrue(RequestRules.canRetry(playlist, admin))
        assertFalse(RequestRules.canRetry(done, admin))
        // members retry only with "fix" (the server answers "Ask an admin to retry"); a playlist's songs always
        val fixer = Me(role = "user", perms = mapOf("fix" to true))
        val cancelled = row("""{"id": 6, "kind": "track", "status": "cancelled"}""")
        assertTrue(RequestRules.canRetry(failedAlbum, fixer))
        assertTrue(RequestRules.canRetry(cancelled, fixer))
        assertFalse(RequestRules.canRetry(failedAlbum, noPick))
        assertFalse(RequestRules.canRetry(failedAlbum, null))
        assertTrue(RequestRules.canRetry(playlist, noPick))
        assertTrue(RequestRules.canCancel(pending))
        assertTrue(RequestRules.canCancel(downloading))
        assertFalse(RequestRules.canCancel(done))
        assertTrue(RequestRules.canRemove(done))
        assertFalse(RequestRules.canRemove(downloading))
        assertEquals("Delete downloaded files", RequestRules.filesLabel("completed"))
        assertEquals("Delete leftover files", RequestRules.filesLabel("failed"))
        assertEquals("Permanently delete the songs this request downloaded from the library?", RequestRules.filesConfirm("completed"))
    }

    @Test
    fun bulkToastsAreTheWebs() {
        assertEquals("No failed requests to retry" to "ok", RequestRules.retryFailedToast(RetryFailedResult(0, 0, "")))
        assertEquals("Retrying 1 request" to "ok", RequestRules.retryFailedToast(RetryFailedResult(1, 0, "")))
        assertEquals("Retrying 4 requests" to "ok", RequestRules.retryFailedToast(RetryFailedResult(4, 0, "")))
        assertEquals("Retrying 2 · 1 couldn't be retried (Ask an admin to retry)" to "warn", RequestRules.retryFailedToast(RetryFailedResult(2, 1, "Ask an admin to retry")))
        assertEquals("Retrying 0 · 3 couldn't be retried" to "warn", RequestRules.retryFailedToast(RetryFailedResult(0, 3, "")))
        assertEquals("5 cleared", RequestRules.clearToast(5, 0, files = false))
        assertEquals("5 cleared · 12 file(s) deleted", RequestRules.clearToast(5, 12, files = true))
    }

    @Test
    fun linkBoxFollowsTheDownloader() {
        assertTrue(RequestRules.metadataOnly("https://open.spotify.com/album/1"))
        assertTrue(RequestRules.metadataOnly("  https://music.apple.com/us/album/x "))
        assertFalse(RequestRules.metadataOnly("https://www.youtube.com/watch?v=x"))
        assertTrue(RequestRules.previewImports("https://www.deezer.com/album/1"))
        assertTrue(RequestRules.previewImports("https://open.spotify.com/track/1"))
        assertFalse(RequestRules.previewImports("https://soundcloud.com/a/b"))
        assertEquals(listOf("Just the song", "Get the whole album", "Admin default"), RequestRules.ALBUM_MODES.map { it.second })
        assertEquals("Downloading", RequestRules.linkToast(LinkResult(mode = "direct"), "song"))
        assertEquals("3 song(s) requested", RequestRules.linkToast(LinkResult(mode = "matched", requested = 3), "song"))
        assertEquals("3 song(s) requested, 2 already requested – whole albums will be fetched", RequestRules.linkToast(LinkResult(mode = "matched", requested = 3, alreadyRequested = 2), "album"))
        assertEquals("Youtube · Someone · 3:41", RequestRules.probeLine(LinkProbe(extractor = "Youtube", uploader = "Someone", duration = 221.0)))
        assertEquals("Youtube · 12 items", RequestRules.probeLine(LinkProbe(extractor = "Youtube", isPlaylist = true, count = 12, duration = 99.0)))
        assertEquals("1:01:05", RequestRules.duration(3665.0))
        assertEquals("0:59", RequestRules.duration(59.4))
    }

    @Test
    fun aLongMessageIsNeverCutInTheDetails() {
        val long = "All sources exhausted (" + (1..40).joinToString("; ") { "slskd: error number $it" } + ")"
        val d = RequestRules.details(row("""{"id": 6, "status": "failed", "message": "$long"}"""), admin = false, ago = "1h ago")
        assertEquals(long, d.message)
        assertEquals(40, d.log.size)
        assertNotNull(d.log.firstOrNull { it.text == "error number 40" })
    }
}
