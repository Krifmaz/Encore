package app.encore.android.features.requests

import app.encore.android.core.EncoreJson
import app.encore.android.core.decodeModel
import app.encore.android.data.repo.LinkProbe
import app.encore.android.data.repo.LinkResult
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.RequestRow
import app.encore.android.data.repo.RetryFailedResult
import app.encore.android.data.repo.SlskTransfer
import app.encore.android.data.repo.StorageHealth
import app.encore.android.data.repo.statusKind
import app.encore.android.data.repo.statusLabel
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
    // ---- R1: waiting requests, kept files, the storage banner, Copy ------------------------------------------------

    /** A row whose import waits for the storage, as S1's server sends it (the same row as the web's test). */
    private val waiting = """{"id": 12, "kind": "album", "status": "waiting", "title": "Illuminate EP", "artist": "Duumu", "album": "Illuminate EP",
        "source": "streamrip", "username": "caz", "created_at": 0, "attempts": 3, "picked": true,
        "message": "Waiting: library storage error – import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5). Next try at 14:20",
        "wait_reason": "storage_import", "next_try_at": 1792427497.2, "has_files": true, "kept_path": "/downloads/encore/request-12/streamrip",
        "error": {"step": "import", "op": "mkdir", "path": "/music/Duumu/Illuminate EP", "to": null, "errno": 5,
            "error": "Input/output error (os error 5)", "storage": true,
            "text": "import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5) – storage problem, not the source"},
        "output": {"source": "streamrip", "url": "https://www.deezer.com/album/1", "text": "Downloading Illuminate EP\n4 tracks done\n", "ok": true, "at": 1792427000.1},
        "log": ["slskd: error: Soulseek isn't connected yet", "attempt 2 failed: streamrip: Input/output error (os error 5)"],
        "candidate": null}"""

    private val date: (Double) -> String = { t -> "<${t.toLong()}>" }

    @Test
    fun theWaitingFieldsAreRead() {
        val r = row(waiting)
        assertEquals("storage_import", r.waitReason)
        assertEquals(1792427497.2, r.nextTryAt, 0.001)
        assertTrue(r.hasFiles)
        assertEquals("/downloads/encore/request-12/streamrip", r.keptPath)
        val e = r.error!!
        assertEquals("import", e.step)
        assertEquals("mkdir", e.op)
        assertEquals("/music/Duumu/Illuminate EP", e.path)
        assertEquals("", e.to)
        assertEquals("5", e.errno)
        assertTrue(e.storage)
        assertEquals("streamrip", r.output?.source)
        assertTrue(r.output?.ok == true)
        // an older server: none of them, and odd values never cost the row
        val old = row("""{"id": 8, "status": "queued"}""")
        assertEquals("", old.waitReason)
        assertFalse(old.hasFiles)
        assertNull(old.error)
        assertNull(old.output)
        val odd = row("""{"id": 9, "status": "failed", "error": "boom", "output": [1], "has_files": null, "next_try_at": null}""")
        assertNull(odd.error)
        assertNull(odd.output)
        assertFalse(odd.hasFiles)
        assertEquals(0.0, odd.nextTryAt, 0.0)
    }

    @Test
    fun waitingReadsWaitingAndCanBeCancelled() {
        assertEquals("Waiting", statusLabel("waiting"))
        assertEquals("warn", statusKind("waiting"))
        val r = row(waiting)
        // nothing runs while it waits: no progress, but Cancel, and ⋯ has Remove from list (the web's row)
        assertFalse(r.isActive)
        assertTrue(RequestRules.canCancel(r))
        assertTrue(RequestRules.canRemove(r))
        assertEquals("the downloaded files are kept and the import waits for the storage", RequestRules.waitWords("storage_import"))
        assertEquals("Soulseek isn't connected yet", RequestRules.waitWords("soulseek"))
        assertEquals("the download waits for the staging folder", RequestRules.waitWords("storage_download"))
        assertEquals("", RequestRules.waitWords(""))
        assertEquals("Waiting – the downloaded files are kept and the import waits for the storage", RequestRules.statusText(r))
        assertEquals("Failed", RequestRules.statusText(row("""{"id": 2, "status": "failed", "wait_reason": "soulseek"}""")))
    }

    @Test
    fun importAgainTakesRetrysPlace() {
        val admin = Me(role = "admin")
        val r = row(waiting)
        assertTrue(RequestRules.canImportAgain(r))
        assertFalse(RequestRules.canRetry(r, admin))
        // a failed import whose files are kept: Import again too
        val failedKept = row("""{"id": 3, "kind": "album", "status": "failed", "has_files": true}""")
        assertTrue(RequestRules.canImportAgain(failedKept))
        assertFalse(RequestRules.canRetry(failedKept, admin))
        // a link's request that kept the files it could not import: Import again for them
        assertTrue(RequestRules.canImportAgain(row("""{"id": 6, "kind": "url", "status": "completed", "has_files": true}""")))
        assertFalse(RequestRules.canImportAgain(row("""{"id": 7, "kind": "url", "status": "cancelled", "has_files": true}""")))
        // a plain failure keeps Retry
        val failed = row("""{"id": 4, "kind": "album", "status": "failed", "has_files": false}""")
        assertFalse(RequestRules.canImportAgain(failed))
        assertTrue(RequestRules.canRetry(failed, admin))
        // a Soulseek wait has no files: no Import again, no Retry, but Cancel
        val soulseek = row("""{"id": 5, "kind": "track", "status": "waiting", "wait_reason": "soulseek", "has_files": false, "message": "Waiting for Soulseek – next attempt at 14:20"}""")
        assertFalse(RequestRules.canImportAgain(soulseek))
        assertFalse(RequestRules.canRetry(soulseek, admin))
        assertTrue(RequestRules.canCancel(soulseek))
        // the admin's files entry and its confirm say the files are kept
        assertEquals("Delete downloaded files", RequestRules.filesLabel("failed", kept = true))
        assertEquals("Delete leftover files", RequestRules.filesLabel("failed", kept = false))
        assertEquals("Delete the downloaded files kept for this request? They can't be imported afterwards.", RequestRules.filesConfirm("waiting", kept = true))
        assertEquals("Delete the leftover/partial files of this download?", RequestRules.filesConfirm("failed"))
        assertEquals("Permanently delete the songs this request downloaded from the library?", RequestRules.filesConfirm("completed", kept = true))
    }

    @Test
    fun theStorageBannerForAdminsAndMembers() {
        val admin = StorageHealth(ok = false, since = 1792426891.6, path = "/music", op = "mkdir", error = "Input/output error (os error 5)", errno = "5", waiting = 3,
            text = "Imports are paused: /music is not writable – mkdir /music/.encore-probe-12-3: Input/output error (os error 5). Checked 14:10, next check 14:20")
        assertEquals("Imports are paused: /music is not writable – mkdir /music/.encore-probe-12-3: Input/output error (os error 5). Checked 14:10, next check 14:20", RequestRules.storageBanner(admin, admin = true))
        val member = StorageHealth(ok = false, since = 1792426891.6, text = "Imports are paused because of a storage problem on the server. Your downloads are kept and will be added when it is fixed.")
        assertEquals(RequestRules.PLAIN_BANNER, RequestRules.storageBanner(member, admin = false))
        // no problem, an older server (no answer) or a missing `ok`: no banner
        assertNull(RequestRules.storageBanner(StorageHealth(ok = true), admin = true))
        assertNull(RequestRules.storageBanner(null, admin = false))
        assertNull(RequestRules.storageBanner(StorageHealth(), admin = true))
        // no text: the client says it in the same words
        val bare = StorageHealth(ok = false, path = "/music", op = "mkdir", error = "Read-only file system (os error 30)")
        assertEquals("Imports are paused: /music is not writable – mkdir: Read-only file system (os error 30).", RequestRules.storageBanner(bare, admin = true))
        assertEquals(RequestRules.PLAIN_BANNER, RequestRules.storageBanner(bare, admin = false))
        assertEquals("Imports are still paused" to "warn", RequestRules.checkToast(bare))
        assertEquals("Imports resumed" to "ok", RequestRules.checkToast(StorageHealth(ok = true)))
        // the server's JSON as it comes
        val decoded = (EncoreJson.parseToJsonElement("""{"ok": false, "since": 1792426891.6, "checked_at": 1792426891.6, "next_check_at": 1792427491.6, "path": "/music", "op": "mkdir",
            "error": "Input/output error (os error 5)", "errno": 5, "step": "import", "waiting": 3, "text": null}""") as JsonObject).decodeModel(StorageHealth.serializer())!!
        assertTrue(decoded.paused)
        assertEquals("5", decoded.errno)
        assertEquals(3, decoded.waiting)
        assertEquals("Imports are paused: /music is not writable – mkdir: Input/output error (os error 5).", RequestRules.storageBanner(decoded, admin = true))
        val ok = (EncoreJson.parseToJsonElement("""{"ok": true, "since": null, "text": null}""") as JsonObject).decodeModel(StorageHealth.serializer())!!
        assertFalse(ok.paused)
    }

    @Test
    fun theCopiedTextIsTheWebs() {
        // the same lines as web-rs/src/pages/common.rs `request_text` (its test `the_details_hold_what_a_bug_report_needs`)
        val want = "Encore request #12\n" +
            "Title: Illuminate EP\n" +
            "Artist: Duumu\n" +
            "Kind: album\n" +
            "Status: Waiting – the downloaded files are kept and the import waits for the storage\n" +
            "Message: Waiting: library storage error – import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5). Next try at 14:20\n" +
            "Next try: <1792427497>\n" +
            "Attempts: 3\n" +
            "Source: streamrip\n" +
            "Picked by hand: yes\n" +
            "Error: import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5) – storage problem, not the source\n" +
            "Step: import\n" +
            "Operation: mkdir\n" +
            "Path: /music/Duumu/Illuminate EP\n" +
            "Errno: 5\n" +
            "System error: Input/output error (os error 5)\n" +
            "Storage problem: yes – not the source\n" +
            "Downloaded files kept in: /downloads/encore/request-12/streamrip\n" +
            "\n" +
            "Log (every attempt, oldest first):\n" +
            "slskd: error: Soulseek isn't connected yet\n" +
            "attempt 2 failed: streamrip: Input/output error (os error 5)\n" +
            "\n" +
            "streamrip output (https://www.deezer.com/album/1 · produced files · <1792427000>):\n" +
            "Downloading Illuminate EP\n" +
            "4 tracks done\n" +
            "\n" +
            "Requested by: caz\n" +
            "Request id: 12\n" +
            "Server version: 0.29.0"
        assertEquals(want, RequestRules.requestText(row(waiting), "0.29.0", date))
        // a link that failed with nothing kept: no empty lines for what it doesn't have
        val link = row("""{"id": 7, "kind": "url", "status": "failed", "url": "https://www.youtube.com/watch?v=x", "title": "", "created_at": 1700000000,
            "message": "Downloaded, but nothing could be imported: …", "attempts": 1, "error": null, "output": null, "log": [], "has_files": false}""")
        assertEquals(
            "Encore request #7\nLink: https://www.youtube.com/watch?v=x\nKind: url\nStatus: Failed\nMessage: Downloaded, but nothing could be imported: …\nAttempts: 1\n\nRequested: <1700000000>\nRequest id: 7\nServer version: unknown",
            RequestRules.requestText(link, "", date),
        )
        // every attempt's whole error stays, however long (the row cuts it, the copy never does)
        val long = "streamrip: " + "x".repeat(5000)
        assertTrue(RequestRules.requestText(row("""{"id": 9, "status": "failed", "log": ["$long"], "updated_at": 1700000100}"""), "1.0", date).contains("\n$long\n"))
        assertTrue(RequestRules.requestText(row("""{"id": 9, "status": "failed", "updated_at": 1700000100}"""), "1.0", date).contains("\nUpdated: <1700000100>\n"))
        // the candidate being fetched, and a failed output without a link
        val c = row("""{"id": 10, "kind": "track", "title": "Blow", "album": "Cannibal", "status": "downloading", "candidate": {"username": "alice", "format": "flac", "directory": "Cannibal", "files": 4},
            "output": {"source": "streamrip", "text": "Traceback\nOSError: [Errno 5]", "ok": false, "at": 0}}""")
        val t = RequestRules.requestText(c, "1.0", date)
        assertTrue(t, t.contains("\nAlbum: Cannibal\n"))
        assertTrue(t, t.contains("\nDownloading: from alice: Cannibal (FLAC, 4 files)\n"))
        assertTrue(t, t.contains("\nstreamrip output (failed):\nTraceback\nOSError: [Errno 5]\n"))
        assertEquals("Illuminate EP – Duumu", RequestRules.requestWhat(row(waiting)))
        assertEquals("https://www.youtube.com/watch?v=x", RequestRules.requestWhat(link))
        assertEquals("Duumu – full discography", RequestRules.requestWhat(row("""{"id": 1, "kind": "artist", "artist": "Duumu"}""")))
    }

    @Test
    fun theDetailsSheetShowsTheWaitTheErrorAndTheOutput() {
        val d = RequestRules.details(row(waiting), admin = true, ago = "8m ago", date = date)
        assertEquals("Waiting – the downloaded files are kept and the import waits for the storage", d.status)
        assertEquals(
            listOf("Requested or retried 3 times", "via streamrip", "Picked by hand", "Next try: <1792427497>", "Downloaded files kept in: /downloads/encore/request-12/streamrip", "by caz", "8m ago"),
            d.facts,
        )
        assertEquals("import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5) – storage problem, not the source", d.error)
        assertEquals(
            listOf("Step: import", "Operation: mkdir", "Path: /music/Duumu/Illuminate EP", "Errno: 5", "System error: Input/output error (os error 5)", "Storage problem: yes – not the source"),
            d.errorFacts,
        )
        assertEquals("streamrip output", d.outputTitle)
        assertEquals("https://www.deezer.com/album/1 · produced files · <1792427000>", d.outputHead)
        assertEquals("Downloading Illuminate EP\n4 tracks done", d.output)
        // a plain failure has none of it; a rename names its other path; an error without a sentence uses the system's words
        val plain = RequestRules.details(row(illuminate), admin = false, ago = "8m ago", date = date)
        assertEquals("", plain.status)
        assertEquals("", plain.error)
        assertEquals("", plain.output)
        val mv = row("""{"id": 3, "status": "failed", "has_files": true, "error": {"step": "import", "op": "rename", "path": "/a", "to": "/b", "errno": 13, "error": "Permission denied (os error 13)", "storage": false}}""")
        val e = RequestRules.details(mv, admin = false, ago = "1m ago", date = date)
        assertEquals("Permission denied (os error 13)", e.error)
        assertEquals(listOf("Step: import", "Operation: rename", "Path: /a", "Other path: /b", "Errno: 13"), e.errorFacts)
        assertTrue(e.facts.contains("Downloaded files kept in: the server"))
    }

    @Test
    fun activityLinesAndTransferErrorsAsText() {
        assertEquals("warning · <date>\nstorage: /music not writable – mkdir /music/.encore-probe-1-1: Input/output error (os error 5); imports paused",
            RequestRules.logText("warning", "storage: /music not writable – mkdir /music/.encore-probe-1-1: Input/output error (os error 5); imports paused", "<date>"))
        assertEquals("alice\n01 - Intro.flac\nTransfer rejected: File not shared.",
            RequestRules.transferErrorText(SlskTransfer(username = "alice", filename = "01 - Intro.flac", error = "Transfer rejected: File not shared.")))
        assertEquals(RequestRules.IMPORT_AGAIN_HELP, "Import the downloaded files again – nothing is downloaded again")
    }

    // ---- R2: downloads the audio check turned away, Keep anyway / Download anyway ------------------------------------

    /** A row of GET /api/requests after the audio check turned the download away (package A's answer; the web's `rejected_row`). */
    private val rejected = """{"id": 21, "kind": "track", "status": "failed", "title": "Moonlight Shadow (nightcore)", "artist": "Machine Time", "album": "Moonlight Shadow (nightcore)",
        "source": "ytdlp", "username": "alice", "created_at": 0, "attempts": 10, "picked": false, "has_files": false, "skip_check": false,
        "message": "All sources exhausted (ytdlp: YouTube match was the wrong audio (audio is “Diase – Sun” (different title)))",
        "rejected": [
            {"id": 1, "source": "streamrip", "via": "Deezer", "detail": "via streamrip", "url": "https://www.deezer.com/track/3135556", "file": "01. Moonlight Shadow.flac",
             "count": 1, "check": "audio", "reason": "audio is “Diase – Sun” (different title)", "heard": "Diase – Sun",
             "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)", "at": 1760890000.5, "has_file": false, "path": null},
            {"id": 2, "source": "ytdlp", "via": "YouTube Music", "detail": null, "url": "https://music.youtube.com/watch?v=x", "file": "Moonlight Shadow.opus",
             "count": 1, "check": "audio", "reason": "audio is “Diase – Sun” (different title)", "heard": "Diase – Sun",
             "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)", "at": 1760890100.0, "has_file": true, "path": "/downloads/encore/rejected-21/2"}
        ],
        "anyway": {"action": "keep", "id": 2, "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)"},
        "log": ["ytdlp: YouTube match was the wrong audio"]}"""

    private val because = "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)"

    @Test
    fun theRejectedDownloadsAreRead() {
        val r = row(rejected)
        assertFalse(r.skipCheck)
        assertEquals(2, r.rejected.size)
        val (deezer, yt) = r.rejected
        assertEquals(1L, deezer.id)
        assertEquals("Deezer", deezer.via)
        assertEquals("via streamrip", deezer.detail)
        assertEquals(1760890000.5, deezer.at, 0.001)
        assertFalse(deezer.hasFile)
        assertEquals("", deezer.path)
        assertEquals("", yt.detail)
        assertTrue(yt.hasFile)
        assertEquals("/downloads/encore/rejected-21/2", yt.path)
        assertEquals("Diase – Sun", yt.heard)
        assertEquals("keep", r.anyway?.action)
        assertEquals(2L, r.anyway?.id)
        // an older server, or odd values: nothing offered, never a lost row
        val old = row("""{"id": 8, "status": "failed"}""")
        assertEquals(0, old.rejected.size)
        assertNull(old.anyway)
        assertFalse(old.skipCheck)
        val odd = row("""{"id": 9, "status": "failed", "rejected": "x", "anyway": "keep", "skip_check": null}""")
        assertEquals(0, odd.rejected.size)
        assertNull(odd.anyway)
        assertNull(RequestRules.anyway(odd))
        assertTrue(row("""{"id": 9, "skip_check": true}""").skipCheck)
    }

    @Test
    fun aDownloadTheAudioCheckTurnedAwayOffersKeepAnywayWithTheReasonInFull() {
        val r = row(rejected)
        val keep = RequestRules.anyway(r)!!
        assertEquals("keep-anyway", keep.action)
        assertTrue(keep.keep)
        assertEquals("Keep anyway", keep.label)
        assertEquals(2L, keep.rejected)
        assertEquals(because, keep.text)
        assertEquals("It will be imported without the audio check", keep.done)
        assertEquals("Import the download the audio check turned away, without the check", keep.help)
        // the reason in full on the row's own line
        assertEquals(because, RequestRules.anywayLine(r))
        // Download anyway when the kept files are gone
        val gone = row(rejected.replace(""""anyway": {"action": "keep", "id": 2""", """"anyway": {"action": "download", "id": 1"""))
        val download = RequestRules.anyway(gone)!!
        assertEquals("download-anyway", download.action)
        assertFalse(download.keep)
        assertEquals("Download anyway", download.label)
        assertEquals(1L, download.rejected)
        assertEquals("Downloading it again without the audio check", download.done)
        assertEquals("Download it again from the same place, without the audio check", download.help)
        // no offer: no line and no button
        val none = row(rejected.replace(""""anyway": {"action": "keep", "id": 2, "text": "$because"}""", """"anyway": null"""))
        assertNull(none.anyway)
        assertNull(RequestRules.anyway(none))
        assertEquals("", RequestRules.anywayLine(none))
        assertFalse(RequestRules.canTakeAnyway(none, Me(role = "admin")))
        // an action this phone doesn't know
        assertNull(RequestRules.anyway(row("""{"id": 3, "anyway": {"action": "later", "id": 1}}""")))
        // without the server's text, the rejected download's own; without that, the check's words
        assertEquals("The audio sounds like X", RequestRules.anyway(row("""{"id": 4, "anyway": {"action": "keep", "id": 4}, "rejected": [{"id": 4, "text": "The audio sounds like X", "reason": "audio is “X – Y”"}]}"""))?.text)
        assertEquals("audio is “X – Y”", RequestRules.anyway(row("""{"id": 4, "anyway": {"action": "keep", "id": 4}, "rejected": [{"id": 4, "reason": "audio is “X – Y”"}]}"""))?.text)
        // no id: the server's newest (the body goes without `rejected`)
        assertEquals(0L, RequestRules.anyway(row("""{"id": 4, "anyway": {"action": "keep"}}"""))?.rejected)
    }

    @Test
    fun keepAnywayIsTakenAsRetryIs() {
        val r = row(rejected)
        // admins, and members with the fix permission; others see the reason and no button
        assertTrue(RequestRules.mayFix(Me(role = "admin")))
        assertTrue(RequestRules.mayFix(Me(role = "owner")))
        assertTrue(RequestRules.mayFix(Me(role = "user", perms = mapOf("fix" to true))))
        assertFalse(RequestRules.mayFix(Me(role = "user", perms = mapOf("pick" to true))))
        assertFalse(RequestRules.mayFix(null))
        assertTrue(RequestRules.canTakeAnyway(r, Me(role = "user", perms = mapOf("fix" to true))))
        assertFalse(RequestRules.canTakeAnyway(r, Me(role = "user")))
        assertEquals(because, RequestRules.anywayLine(r))
        // Retry stays beside it on a failed row
        assertTrue(RequestRules.canRetry(r, Me(role = "admin")))
    }

    @Test
    fun aHandPicksReasonShowsOnce() {
        // a hand pick the check turned away has the reason as its message: shown on the row's own line, not twice
        val pick = row(rejected.replace(""""message": "All sources exhausted (ytdlp: YouTube match was the wrong audio (audio is “Diase – Sun” (different title)))"""", """"message": "$because""""))
        assertEquals("10 attempts · via ytdlp · by alice · 8m ago", RequestRules.statusLine(pick, admin = true, ago = "8m ago"))
        assertEquals(because, RequestRules.anywayLine(pick))
        val d = RequestRules.details(pick, admin = true, ago = "8m ago", date = date)
        assertEquals(because, d.anyway)
        assertEquals(because, d.message)   // the sheet shows it once, in the box at the top
        // another message stays on the status line
        assertEquals(
            "10 attempts · via ytdlp · All sources exhausted (ytdlp: YouTube match was the wrong audio (audio is “Diase – Sun” (different title))) · 8m ago",
            RequestRules.statusLine(row(rejected), admin = false, ago = "8m ago"),
        )
    }

    @Test
    fun theDetailsNameEveryDownloadTheAudioCheckTurnedAway() {
        // the same block as web-rs/src/pages/common.rs `request_text` (its test `the_details_name_every_download_the_audio_check_turned_away`)
        val r = row(rejected.replace(""""skip_check": false""", """"skip_check": true"""))
        val want = "\n" +
            "Turned away by the audio check (oldest first):\n" +
            "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)\n" +
            "  From: Deezer · via streamrip · https://www.deezer.com/track/3135556 · 01. Moonlight Shadow.flac\n" +
            "  Check: audio is “Diase – Sun” (different title)\n" +
            "  When: <1760890000>\n" +
            "  Files: gone\n" +
            "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)\n" +
            "  From: YouTube Music · https://music.youtube.com/watch?v=x · Moonlight Shadow.opus\n" +
            "  Check: audio is “Diase – Sun” (different title)\n" +
            "  When: <1760890100>\n" +
            "  Files: kept in /downloads/encore/rejected-21/2\n"
        val t = RequestRules.requestText(r, "0.31.0", date)
        assertTrue(t, t.contains("Audio check: off for this request\n$want"))
        assertTrue(t, t.contains(want + "\nLog (every attempt, oldest first):\nytdlp: YouTube match was the wrong audio\n"))
        // members get no server folder; an album names its first file and how many more
        val m = row("""{"id": 22, "kind": "album", "status": "failed", "rejected": [{"id": 2, "via": "YouTube Music", "url": "https://music.youtube.com/watch?v=x",
            "file": "Moonlight Shadow.opus", "count": 10, "text": "$because", "has_file": true, "path": null}]}""")
        val mt = RequestRules.requestText(m, "", date)
        assertTrue(mt, mt.contains("  From: YouTube Music · https://music.youtube.com/watch?v=x · Moonlight Shadow.opus and 9 more\n"))
        assertTrue(mt, mt.contains("  Files: kept on the server\n"))
        assertFalse(mt, mt.contains("Audio check: off"))
        // no rejected downloads: nothing more in the text
        assertFalse(RequestRules.requestText(row(waiting), "", date).contains("Turned away"))
        // an entry without a sentence: the check's words, no Check line; without either, the plain sentence
        val bare = RequestRules.rejectedLines(row("""{"id": 5, "rejected": [{"id": 1, "reason": "audio is “X – Y”", "has_file": true}, {"id": 2}]}"""), date)
        assertEquals(RejectedLine("audio is “X – Y”", "", "", "", "kept on the server"), bare[0])
        assertEquals(RejectedLine("The audio sounds like another song", "", "", "", "gone"), bare[1])
    }

    @Test
    fun afterKeepAnywayAndDownloadAnyway() {
        // Keep anyway taken (package A's final answer): waiting for the queue's next look, no storage reason
        // (wait_reason null), the kept files waiting, nothing more offered – plain "Waiting", no storage words
        val kept = row(rejected
            .replace(""""status": "failed"""", """"status": "waiting", "wait_reason": null""")
            .replace(""""has_files": false""", """"has_files": true""")
            .replace(""""skip_check": false""", """"skip_check": true""")
            .replace(""""anyway": {"action": "keep", "id": 2, "text": "$because"}""", """"anyway": null""")
            .replace(Regex(""""message": "[^"]*""""), """"message": "Waiting to import the kept download""""))
        assertEquals("Waiting", RequestRules.statusText(kept))
        assertNull(RequestRules.anyway(kept))
        assertEquals("", RequestRules.anywayLine(kept))
        assertFalse(RequestRules.canTakeAnyway(kept, Me(role = "admin")))
        // R1's Import again stands in Retry's place while the kept files wait, and Cancel stays
        assertTrue(RequestRules.canImportAgain(kept))
        assertFalse(RequestRules.canRetry(kept, Me(role = "admin")))
        assertTrue(RequestRules.canCancel(kept))
        val d = RequestRules.details(kept, admin = true, ago = "1m ago", date = date)
        assertEquals("", d.status)
        assertEquals("Waiting to import the kept download", d.message)
        assertTrue(d.facts.toString(), d.facts.contains(RequestRules.SKIP_CHECK))
        // the turned-away downloads stay listed (a later Retry keeps them)
        assertEquals(2, d.rejected.size)
        // a storage error recorded for that import: then it says so
        assertEquals("Waiting – the downloaded files are kept and the import waits for the storage",
            RequestRules.statusText(row(rejected.replace(""""status": "failed"""", """"status": "waiting", "wait_reason": "storage_import""""))))
        // Download anyway taken: queued again without the check; the status line carries the server's message
        val again = row(rejected
            .replace(""""status": "failed"""", """"status": "queued"""")
            .replace(""""skip_check": false""", """"skip_check": true""")
            .replace(""""anyway": {"action": "keep", "id": 2, "text": "$because"}""", """"anyway": null""")
            .replace(Regex(""""message": "[^"]*""""), """"message": "Downloading it again without the audio check""""))
        assertNull(RequestRules.anyway(again))
        assertEquals("10 attempts · via ytdlp · Downloading it again without the audio check · 8m ago", RequestRules.statusLine(again, admin = false, ago = "8m ago"))
        assertTrue(RequestRules.canCancel(again))
        assertFalse(RequestRules.canRetry(again, Me(role = "admin")))
        // a Keep anyway the quality check turned away: failed with its reason, nothing more offered, Retry back
        val refused = row(rejected
            .replace(""""anyway": {"action": "keep", "id": 2, "text": "$because"}""", """"anyway": null""")
            .replace(Regex(""""message": "[^"]*""""), """"message": "Kept anyway, but the quality check turned it away: Wrong version: 3:18 shorter than the recording""""))
        assertEquals("", RequestRules.anywayLine(refused))
        assertTrue(RequestRules.statusLine(refused, admin = false, ago = "8m ago").contains("Kept anyway, but the quality check turned it away: Wrong version"))
        assertTrue(RequestRules.canRetry(refused, Me(role = "user", perms = mapOf("fix" to true))))
        assertFalse(RequestRules.canRetry(refused, Me(role = "user")))
    }

    @Test
    fun theDetailsSheetShowsTheReasonAndWhereItCameFrom() {
        val r = row(rejected.replace(""""skip_check": false""", """"skip_check": true"""))
        val d = RequestRules.details(r, admin = true, ago = "8m ago", date = date)
        assertEquals(because, d.anyway)
        assertEquals("YouTube Music · https://music.youtube.com/watch?v=x · Moonlight Shadow.opus", d.anywayFrom)
        assertEquals(2, d.rejected.size)
        assertEquals(RejectedLine(because, "Deezer · via streamrip · https://www.deezer.com/track/3135556 · 01. Moonlight Shadow.flac", "audio is “Diase – Sun” (different title)", "<1760890000>", "gone"), d.rejected[0])
        assertEquals("kept in /downloads/encore/rejected-21/2", d.rejected[1].files)
        assertTrue(d.facts.toString(), d.facts.contains("Audio check: off for this request"))
        // nothing turned away: no box, no list
        val plain = RequestRules.details(row(illuminate), admin = false, ago = "8m ago", date = date)
        assertEquals("", plain.anyway)
        assertEquals("", plain.anywayFrom)
        assertEquals(0, plain.rejected.size)
        // an offer whose entry the list doesn't have: the reason without a source line
        val lone = RequestRules.details(row("""{"id": 6, "status": "failed", "anyway": {"action": "download", "id": 9, "text": "$because"}}"""), admin = false, ago = "1m ago", date = date)
        assertEquals(because, lone.anyway)
        assertEquals("", lone.anywayFrom)
    }
}
